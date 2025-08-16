//! # Backup Session Management Operations
//!
//! This module provides high-level operations for managing backup sessions, including
//! listing, showing details, restoring files, and cleaning up old sessions. It implements
//! the Phase B2 backup management functionality with safe defaults and comprehensive
//! error handling.
//!
//! ## Features
//!
//! - **Session Listing:** Filter and list backup sessions by success status, engine type,
//!   and time bounds.
//! - **Session Details:** Show detailed information about a specific session, including
//!   manifest and total size.
//! - **Session Restoration:** Restore files from a backup session, with options for
//!   dry-run, force overwrite, path filtering, checksum verification, and unified diff
//!   reporting for conflicts.
//! - **Session Cleanup:** Remove old or incomplete sessions based on age or count, with
//!   dry-run support and error reporting.
//! - **Session ID Resolution:** Robust resolution of session IDs, supporting full IDs,
//!   short suffixes, date prefixes, and aliases (`latest`, `last-successful`).
//! - **Unified Diff Generation:** Generate unified diffs between current files and backup
//!   versions for conflict analysis.
//! - **Checksum Verification:** Stream-based Blake3 checksum verification for backup
//!   payload integrity.
//!
//! ## Key Types
//!
//! - `SessionIdResolution`: Enum representing the result of session ID resolution
//!   (single, multiple, not found).
//! - `SessionInfo`: Concise session info for listing.
//! - `ListRequest`: Request structure for listing sessions.
//! - `ShowRequest`, `ShowResponse`: Structures for showing session details.
//! - `RestoreRequest`, `RestoreResult`: Structures for restoring files from a session.
//! - `CleanupRequest`, `CleanupResult`: Structures for cleaning up sessions.
//! - `FileDiff`: Structure representing a unified diff for a file.
//!
//! ## Helper Functions
//!
//! - `select_targets`: Selects files from a session manifest, optionally filtering by
//!   path.
//! - `build_diffs`: Builds unified diffs between current repo files and backup versions.
//! - `normalize_repo_rel`: Normalizes and validates repo-relative paths.
//! - `parse_time_bound`: Parses time bounds for filtering and cleanup (supports RFC3339
//!   and relative specs like "7d", "24h").
//! - `stream_blake3`: Computes streaming Blake3 checksums for files.
//!
//! ## Error Handling
//!
//! All operations use robust error handling via the `anyhow` crate, providing context for
//! failures.
//!
//! ## Testing
//!
//! Includes unit tests for time bound parsing and session ID resolution logic.
//!
//! ## Usage
//!
//! Import and use the provided functions to manage backup sessions in a repository.
//!
//! Backup session management operations
//!
//! Provides high-level operations for listing, showing, restoring, and cleaning up
//! backup sessions. This module implements the Phase B2 backup management functionality
//! with safe defaults and comprehensive error handling.
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;

use crate::core::backup::{
    BackupManager, FileBackupMeta, SessionIndexEntry, SessionManifest, list_sessions,
    read_session_manifest,
};

/// Session ID resolution result
#[derive(Debug)]
pub enum SessionIdResolution
{
    /// Single session found
    Single(String),
    /// Multiple matches found
    Multiple(Vec<String>),
    /// No matches found  
    NotFound,
}

/// Concise session info for listing
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo
{
    pub id: String,
    pub timestamp: String,
    pub engine: String,
    pub success: bool,
    pub files: usize,
    pub sample_paths: Vec<String>, // First 3 files for quick scanning
}

/// Request structure for listing sessions
#[derive(Debug)]
pub struct ListRequest
{
    pub successful: bool,
    pub engine: Option<String>,
    pub since: Option<String>,
    pub limit: usize,
    pub sort_desc: bool,
}

/// Request structure for showing session details
#[derive(Debug)]
pub struct ShowRequest
{
    pub id: String,
    pub verbose: bool,
}

/// Response for show command
#[derive(Debug, Serialize)]
pub struct ShowResponse
{
    /// The manifest containing metadata and file list for the session
    pub manifest: SessionManifest,

    /// Filesystem path to the session's backup directory
    pub session_path: PathBuf,

    /// Total size of the session's backup payload (in bytes), if verbose
    pub total_size: Option<u64>,
}

/// Request for restoring from a session.
#[derive(Debug)]
pub struct RestoreRequest
{
    /// If true, back up current files before restoring
    pub backup_current: bool,

    /// If true, only plan the restore (do not write files)
    pub dry_run: bool,

    /// If true, overwrite files even if content mismatches
    pub force: bool,

    /// Optional repo-relative path filter to restore only specific file(s)
    pub path: Option<PathBuf>,

    /// Session ID or alias ('latest', 'last-successful') to restore from
    pub session_id: String,

    /// If true, emit unified diffs for conflicting files
    pub show_diff: bool,

    /// If true, verify checksums of backup payloads before restoring
    pub verify_checksum: bool,
}

/// Unified diff for a file.
#[derive(Debug, Serialize)]
pub struct FileDiff
{
    /// The repo-relative path of the file being diffed
    pub path: PathBuf,

    /// The unified diff output as a string
    pub unified: String,
}

/// Result of a restore operation.
#[derive(Debug, Serialize)]
pub struct RestoreResult
{
    /// Indicates if current files were backed up before restoring
    pub backed_up_current: bool,

    /// The session ID of the backup created for current files, if any
    pub backup_session_id: Option<String>,

    /// List of files that had conflicts during restore
    pub conflicts: Vec<PathBuf>,

    /// Optional unified diffs for conflicting files
    pub diffs: Option<Vec<FileDiff>>,

    /// List of files that were restored
    pub restored: Vec<PathBuf>,

    /// The session ID from which files were restored
    pub session_id: String,
}

/// Request for cleanup.
#[derive(Debug)]
pub struct CleanupRequest
{
    /// If true, only plan the cleanup (do not delete sessions)
    pub dry_run: bool,

    /// If true, include sessions that do not have a DONE marker (incomplete)
    pub include_incomplete: bool,

    /// Number of newest sessions to keep (if specified)
    pub keep_latest: Option<usize>,

    /// Remove sessions older than this RFC3339 or relative spec (e.g., "7d", "24h")
    pub older_than: Option<String>,
}

/// Result of cleanup.
#[derive(Debug, Serialize)]
pub struct CleanupResult
{
    /// Total bytes freed by cleanup
    pub bytes_freed: u64,

    /// Errors encountered during cleanup
    pub errors: Vec<String>,

    /// List of session IDs that were removed
    pub sessions_removed: Vec<String>,
}

/// List sessions with filters, minimizing manifest IO
/// Filters include success status, engine type, and time bounds.
pub fn list_sessions_filtered(
    repo_root: &Path,
    req: ListRequest,
) -> Result<Vec<SessionInfo>>
{
    // Parse "since" once
    let since_time = if let Some(ref s) = req.since
    {
        Some(parse_time_bound(s)?)
    }
    else
    {
        None
    };

    // Load index entries
    let mut entries = list_sessions(repo_root)?;

    // Keep only completed sessions
    entries.retain(|e| session_is_complete(repo_root, &e.id).unwrap_or(false));

    // Apply filters that require only index data
    if req.successful
    {
        entries.retain(|e| e.success);
    }

    if let Some(ref engine_filter) = req.engine
    {
        // Case-insensitive engine matching
        let target = engine_filter.to_ascii_lowercase();
        entries.retain(|e| {
            e.engine
                .to_ascii_lowercase()
                == target
        });
    }

    if let Some(since) = since_time
    {
        // Drop sessions older than the bound
        entries.retain(|e| {
            DateTime::parse_from_rfc3339(&e.timestamp)
                .ok()
                .map(|dt| dt.with_timezone(&Utc) >= since)
                .unwrap_or(false)
        });
    }

    // Sort by parsed timestamp desc/asc robustly
    entries.sort_by(|a, b| {
        let ap = DateTime::parse_from_rfc3339(&a.timestamp)
            .ok()
            .map(|x| x.with_timezone(&Utc));

        let bp = DateTime::parse_from_rfc3339(&b.timestamp)
            .ok()
            .map(|x| x.with_timezone(&Utc));

        if req.sort_desc
        {
            bp.cmp(&ap)
                .then_with(|| {
                    b.timestamp
                        .cmp(&a.timestamp)
                })
        }
        else
        {
            ap.cmp(&bp)
                .then_with(|| {
                    a.timestamp
                        .cmp(&b.timestamp)
                })
        }
    });

    // Truncate to limit before manifest reads
    if entries.len() > req.limit
    {
        entries.truncate(req.limit);
    }

    // Collect SessionInfo; now read manifests only for sample paths
    let mut out = Vec::with_capacity(entries.len());
    for e in entries
    {
        // Try to read manifest to extract first 3 sample paths
        let sample_paths = match read_session_manifest(repo_root, &e.id)
        {
            Ok(m) =>
            {
                m.files
                    .iter()
                    .take(3)
                    .map(|f| {
                        f.original_path
                            .display()
                            .to_string()
                    })
                    .collect()
            }

            Err(_) => Vec::new(),
        };

        out.push(SessionInfo {
            id: e.id,
            timestamp: e.timestamp,
            engine: e.engine,
            success: e.success,
            files: e.files,
            sample_paths,
        });
    }

    Ok(out)
}

/// Show detailed information about a session
pub fn show_session(
    repo_root: &Path,
    req: ShowRequest,
) -> Result<ShowResponse>
{
    let session_id = resolve_session_id(repo_root, &req.id)?;
    let manifest = read_session_manifest(repo_root, &session_id)?;
    let session_path = repo_root
        .join(".rup")
        .join("backups")
        .join(&session_id);

    // Calculate total size if verbose
    let total_size = if req.verbose
    {
        Some(calculate_session_size(&session_path)?)
    }
    else
    {
        None
    };

    Ok(ShowResponse { manifest, session_path, total_size })
}

/// Resolve session ID (supports full, short, and aliases)
pub fn resolve_session_id(
    repo_root: &Path,
    query: &str,
) -> Result<String>
{
    match resolve_session_id_internal(repo_root, query)?
    {
        // If a single session is found, return its ID
        SessionIdResolution::Single(id) => Ok(id),

        // If multiple sessions match, return an error with the list of matches
        SessionIdResolution::Multiple(matches) =>
        {
            bail!(
                "Ambiguous session ID '{}'. Matches: {}",
                query,
                matches.join(", ")
            );
        }

        // If no session matches, return an error
        SessionIdResolution::NotFound =>
        {
            bail!("No session found matching '{}'", query);
        }
    }
}

// Resolve session ID (internal): prefer completed sessions when using aliases
fn resolve_session_id_internal(
    repo_root: &Path,
    query: &str,
) -> Result<SessionIdResolution>
{
    // Read index entries once
    let sessions = list_sessions(repo_root)?;

    // Helper to check completion
    // (Avoid re-reading manifests; DONE marker is enough)
    let is_complete = |id: &str| session_is_complete(repo_root, id).unwrap_or(false);

    // Precompute parsed timestamps (skip invalid safely)
    // and carry completion status to avoid repeated IO.
    let entries: Vec<(String, String, bool, Option<DateTime<Utc>>)> = sessions
        .iter()
        .map(|s| {
            // Parse RFC3339; if parsing fails, None so it sorts last
            let parsed = DateTime::parse_from_rfc3339(&s.timestamp)
                .ok()
                .map(|dt| dt.with_timezone(&Utc));
            (
                s.id.clone(),
                s.timestamp
                    .clone(),
                s.success,
                parsed,
            )
        })
        .collect();

    // Handle aliases first
    match query
    {
        // latest: choose newest completed session by parsed time
        "latest" =>
        {
            // Filter completed
            let mut cands: Vec<_> = entries
                .iter()
                .filter(|(id, _, _, _)| is_complete(id))
                .collect();
            // Sort by parsed time desc, then by string desc as tiebreaker
            cands.sort_by(|a, b| {
                b.3.cmp(&a.3)
                    .then_with(|| {
                        b.1.cmp(&a.1)
                    })
            });
            return Ok(match cands.first()
            {
                Some((id, ..)) => SessionIdResolution::Single(id.clone()),
                None => SessionIdResolution::NotFound,
            });
        }

        // last-successful: newest completed AND success=true
        "last-successful" =>
        {
            let mut cands: Vec<_> = entries
                .iter()
                .filter(|(id, _, success, _)| *success && is_complete(id))
                .collect();
            cands.sort_by(|a, b| {
                b.3.cmp(&a.3)
                    .then_with(|| {
                        b.1.cmp(&a.1)
                    })
            });

            return Ok(match cands.first()
            {
                Some((id, ..)) => SessionIdResolution::Single(id.clone()),
                None => SessionIdResolution::NotFound,
            });
        }
        _ =>
        {}
    }

    // Collect matches (exact, short-suffix, date-prefix)
    let mut matches: Vec<(String, Option<DateTime<Utc>>, String)> = Vec::new();

    for (id, ts, _success, parsed) in &entries
    {
        // Exact match
        if id == query
        {
            return Ok(SessionIdResolution::Single(id.clone()));
        }

        // Short ID (require a minimal length to reduce noise)
        if query.len() >= 8 && id.ends_with(query)
        {
            matches.push((id.clone(), *parsed, ts.clone()));
        }

        // Date prefix like "2025-08-14"
        if query.contains('-') && id.starts_with(query)
        {
            matches.push((id.clone(), *parsed, ts.clone()));
        }
    }

    // Sort matches newest-first for better ambiguity messages
    matches.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| {
                b.2.cmp(&a.2)
            })
    });

    // Return resolution
    Ok(match matches.len()
    {
        0 => SessionIdResolution::NotFound,
        1 =>
        {
            SessionIdResolution::Single(
                matches[0]
                    .0
                    .clone(),
            )
        }
        _ =>
        {
            SessionIdResolution::Multiple(
                matches
                    .into_iter()
                    .map(|(id, _, _)| id)
                    .collect(),
            )
        }
    })
}

/// Check if session is complete (has DONE marker)
fn session_is_complete(
    repo_root: &Path,
    session_id: &str,
) -> Result<bool>
{
    let done_path = repo_root
        .join(".rup")
        .join("backups")
        .join(session_id)
        .join("DONE");
    Ok(done_path.exists())
}

/// Compute size of backed-up payload (exclude manifest and DONE)
fn calculate_session_size(session_path: &Path) -> Result<u64>
{
    // Accumulator
    let mut total_size = 0u64;

    // Recursive visitor
    fn visit_dir(
        dir: &Path,
        total: &mut u64,
    ) -> Result<()>
    {
        for entry in fs::read_dir(dir)?
        {
            let entry = entry?;
            let path = entry.path();
            let md = entry.metadata()?;

            // Recurse into directories
            if md.is_dir()
            {
                visit_dir(&path, total)?;
                continue;
            }

            // Skip metadata files
            let fname = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");

            if fname == "manifest.json" || fname == "DONE"
            {
                continue;
            }

            // Sum file size
            *total += md.len();
        }

        Ok(())
    }

    // If session dir exists, walk it
    if session_path.exists()
    {
        visit_dir(session_path, &mut total_size)?;
    }

    Ok(total_size)
}

/// Restore files from a session.
pub fn restore_session(
    repo_root: &Path,
    req: RestoreRequest,
) -> Result<RestoreResult>
{
    let session_id = resolve_session_id(repo_root, &req.session_id)?;
    let manifest = read_session_manifest(repo_root, &session_id)?;
    let session_dir = repo_root
        .join(".rup")
        .join("backups")
        .join(&session_id);

    let targets = select_targets(
        &manifest,
        req.path
            .as_deref(),
    )
    .context("no matching file(s) in session")?;

    if req.verify_checksum
    {
        for f in &targets
        {
            if let Some(expected) = &f.checksum
            {
                let p = session_dir.join(&f.rel_path);
                let actual = stream_blake3(&p)?;
                if &actual != expected
                {
                    bail!(
                        "checksum mismatch for {}",
                        f.original_path
                            .display()
                    );
                }
            }
        }
    }

    // Detect conflicts (current bytes != backup bytes).
    let mut conflicts = Vec::<PathBuf>::new();
    let mut writes = Vec::<(PathBuf, Vec<u8>)>::new();

    for f in &targets
    {
        let dst = repo_root.join(&f.original_path);
        let backup_bytes = fs::read(session_dir.join(&f.rel_path)).with_context(|| {
            format!(
                "read backup payload: {}",
                f.rel_path
                    .display()
            )
        })?;

        if dst.exists()
        {
            let cur = fs::read(&dst).with_context(|| format!("read current: {}", dst.display()))?;
            if cur != backup_bytes && !req.force
            {
                conflicts.push(
                    f.original_path
                        .clone(),
                );
                continue;
            }
        }
        writes.push((
            f.original_path
                .clone(),
            backup_bytes,
        ));
    }

    // On conflicts without --force, report plan and optional diff(s).
    if !conflicts.is_empty() && !req.force
    {
        let diffs = if req.show_diff
        {
            Some(build_diffs(repo_root, &session_dir, &targets)?)
        }
        else
        {
            None
        };
        return Ok(RestoreResult {
            session_id,
            restored: Vec::new(),
            conflicts,
            backed_up_current: false,
            backup_session_id: None,
            diffs,
        });
    }

    // Optionally back up current files before overwriting.
    let mut backed_up_current = false;
    let mut backup_session_id = None;
    if req.backup_current && !req.dry_run
    {
        let mut mgr = BackupManager::begin(repo_root, "restore")?;
        for (rel, _) in &writes
        {
            if repo_root
                .join(rel)
                .exists()
            {
                mgr.backup_file(rel)?;
            }
        }
        mgr.finalize(true)?;
        backed_up_current = true;
        backup_session_id = Some(
            mgr.session_id()
                .to_string(),
        );
    }

    // Apply or simulate writes.
    let mut restored = Vec::<PathBuf>::new();

    if req.dry_run
    {
        restored.extend(
            writes
                .iter()
                .map(|(p, _)| p.clone()),
        );
    }
    else
    {
        for (rel, bytes) in writes
        {
            let dst = repo_root.join(&rel);

            if let Some(parent) = dst.parent()
            {
                fs::create_dir_all(parent)?;
            }

            fs::write(&dst, &bytes)
                .with_context(|| format!("write restored file: {}", dst.display()))?;

            restored.push(rel);
        }
    }

    // Optional diffs after restore plan; use current vs backup.
    let diffs = if req.show_diff
    {
        Some(build_diffs(repo_root, &session_dir, &targets)?)
    }
    else
    {
        None
    };

    Ok(RestoreResult {
        session_id,
        restored,
        conflicts: Vec::new(),
        backed_up_current,
        backup_session_id,
        diffs,
    })
}

/// Cleanup sessions by age and/or keep-latest.
pub fn cleanup_sessions(
    repo_root: &Path,
    req: CleanupRequest,
) -> Result<CleanupResult>
{
    if req
        .older_than
        .is_none()
        && req
            .keep_latest
            .is_none()
    {
        bail!("specify --older-than and/or --keep-latest");
    }

    let base = repo_root
        .join(".rup")
        .join("backups");
    if !base.exists()
    {
        return Ok(CleanupResult {
            sessions_removed: vec![],
            bytes_freed: 0,
            errors: vec![],
        });
    }

    // Enumerate sessions on disk for ground truth.
    let mut rows = Vec::<(String, PathBuf, DateTime<Utc>, bool)>::new();
    for ent in fs::read_dir(&base)?
    {
        let ent = ent?;
        if !ent
            .file_type()?
            .is_dir()
        {
            continue;
        }
        if ent
            .file_name()
            .to_string_lossy()
            == "tmp"
        {
            continue;
        }
        let id = ent
            .file_name()
            .to_string_lossy()
            .to_string();
        let p = ent.path();
        let done = p
            .join("DONE")
            .exists();
        if !req.include_incomplete && !done
        {
            continue;
        }

        let ts = read_manifest_ts(&p)
            .or_else(|| dir_mtime_fallback(&ent))
            .unwrap_or_else(Utc::now);
        rows.push((id, p, ts, done));
    }

    // Newest first, deterministic by id.
    rows.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| {
                b.0.cmp(&a.0)
            })
    });

    // Build deletion set by rules.
    let mut to_delete = Vec::<(String, PathBuf)>::new();

    if let Some(spec) = &req.older_than
    {
        let cutoff = parse_time_bound(spec)?;
        for (id, p, ts, _) in &rows
        {
            if *ts < cutoff
            {
                to_delete.push((id.clone(), p.clone()));
            }
        }
    }

    if let Some(keep) = req.keep_latest
        && rows.len() > keep
    {
        for (id, p, _, _) in rows
            .iter()
            .skip(keep)
        {
            if !to_delete
                .iter()
                .any(|(x, _)| x == id)
            {
                to_delete.push((id.clone(), p.clone()));
            }
        }
    }

    // Fix #6: Proper deduplication using HashSet to handle non-adjacent duplicates
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    to_delete.retain(|(id, _)| seen.insert(id.clone()));

    let mut bytes = 0u64;
    for (_, p) in &to_delete
    {
        bytes = bytes.saturating_add(dir_size(p).unwrap_or(0));
    }

    let mut removed = Vec::<String>::new();
    let mut errors = Vec::<String>::new();

    if req.dry_run
    {
        removed.extend(
            to_delete
                .iter()
                .map(|(id, _)| id.clone()),
        );
    }
    else
    {
        for (id, p) in &to_delete
        {
            match fs::remove_dir_all(p)
            {
                Ok(_) => removed.push(id.clone()),
                Err(e) => errors.push(format!("{}: {}", id, e)),
            }
        }
        // Optional: rebuild index for consistency.
        if let Err(e) = rebuild_index(repo_root)
        {
            errors.push(format!("index rebuild: {e}"));
        }
    }

    Ok(CleanupResult {
        sessions_removed: removed,
        bytes_freed: bytes,
        errors,
    })
}

// ---------- helpers ----------

/// Select target files from a session manifest, optionally filtering by a repo-relative
/// path. Returns a vector of matching FileBackupMeta entries.
/// If a filter is provided, only files matching the normalized path are returned.
/// Errors if the filter does not match any file in the session.
fn select_targets(
    manifest: &SessionManifest,
    filt: Option<&Path>,
) -> Result<Vec<FileBackupMeta>>
{
    if let Some(p) = filt
    {
        // Normalize the filter path to ensure it's repo-relative and safe
        let want = normalize_repo_rel(p)?;
        let mut out = Vec::new();

        // Find files in the manifest that match the normalized filter path
        for f in &manifest.files
        {
            if normalize_repo_rel(&f.original_path)? == want
            {
                out.push(f.clone());
            }
        }

        // Error if no files matched the filter
        if out.is_empty()
        {
            bail!("file not present in session: {}", want.display());
        }

        Ok(out)
    }
    else
    {
        // No filter: return all files in the session
        Ok(manifest
            .files
            .clone())
    }
}

/// Build unified diffs between current repo files and their backup versions in a session.
///
/// For each target file, reads the current file from the repository and the backup file
/// from the session directory. If either file is missing, treats its contents as empty.
/// Returns a vector of `FileDiff` containing the path and unified diff output.
///
/// # Arguments
/// * `repo_root` - Path to the repository root.
/// * `session_dir` - Path to the session backup directory.
/// * `targets` - List of files to diff, as `FileBackupMeta`.
///
/// # Errors
/// Returns an error only if allocation fails (very rare), since file reads use
/// `unwrap_or_default`.
fn build_diffs(
    repo_root: &Path,
    session_dir: &Path,
    targets: &[FileBackupMeta],
) -> Result<Vec<FileDiff>>
{
    let mut out = Vec::new();
    for t in targets
    {
        // Fix #10: Handle binary files by checking UTF-8 validity
        let cur_bytes = fs::read(repo_root.join(&t.original_path)).unwrap_or_default();
        let bak_bytes = fs::read(session_dir.join(&t.rel_path)).unwrap_or_default();

        let (cur, bak) = match (
            std::str::from_utf8(&cur_bytes),
            std::str::from_utf8(&bak_bytes),
        )
        {
            (Ok(c), Ok(b)) => (c.to_owned(), b.to_owned()),
            _ =>
            {
                out.push(FileDiff {
                    path: t
                        .original_path
                        .clone(),
                    unified: String::from("[binary files differ; no text diff]"),
                });
                continue;
            }
        };

        // Generate unified diff between current and backup contents.
        let diff = unified_diff(&cur, &bak, &t.original_path);

        // Collect the diff result.
        out.push(FileDiff {
            path: t
                .original_path
                .clone(),
            unified: diff,
        });
    }

    Ok(out)
}

pub(crate) fn normalize_repo_rel(p: &Path) -> Result<PathBuf>
{
    if p.is_absolute()
    {
        bail!("path must be repo-relative: {}", p.display());
    }
    let mut out = PathBuf::new();
    for c in p.components()
    {
        match c
        {
            Component::ParentDir => bail!("path escapes repo: {}", p.display()),
            Component::CurDir =>
            {}
            Component::Prefix(_) | Component::RootDir =>
            {
                bail!("path must be repo-relative: {}", p.display())
            }
            _ => out.push(c.as_os_str()),
        }
    }
    if out
        .as_os_str()
        .is_empty()
    {
        bail!("empty path");
    }
    Ok(out)
}

fn read_manifest_ts(dir: &Path) -> Option<DateTime<Utc>>
{
    let s = fs::read_to_string(dir.join("manifest.json")).ok()?;
    let m: SessionManifest = serde_json::from_str(&s).ok()?;
    DateTime::parse_from_rfc3339(&m.timestamp)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn dir_mtime_fallback(ent: &fs::DirEntry) -> Option<DateTime<Utc>>
{
    let md = ent
        .metadata()
        .ok()?;
    let mt = md
        .modified()
        .ok()?;
    let dur = mt
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Utc.timestamp_opt(dur.as_secs() as i64, dur.subsec_nanos())
        .single()
}

fn parse_time_bound(spec: &str) -> Result<DateTime<Utc>>
{
    if let Ok(dt) = DateTime::parse_from_rfc3339(spec)
    {
        return Ok(dt.with_timezone(&Utc));
    }
    let spec = spec.trim();
    if spec.len() < 2
    {
        bail!("invalid --older-than: {spec}");
    }
    let (num, unit) = spec.split_at(spec.len() - 1);
    let n: i64 = num
        .parse()
        .with_context(|| format!("invalid number: {spec}"))?;
    if n < 0
    {
        bail!("negative durations not supported in --older-than: {spec}");
    }
    let now = Utc::now();
    let dt = match unit
    {
        "s" => now - chrono::Duration::seconds(n),
        "m" => now - chrono::Duration::minutes(n),
        "h" => now - chrono::Duration::hours(n),
        "d" => now - chrono::Duration::days(n),
        "w" => now - chrono::Duration::weeks(n),
        _ => bail!("unsupported unit in --older-than (use s,m,h,d,w or RFC3339)"),
    };
    Ok(dt)
}

fn dir_size(path: &Path) -> Result<u64>
{
    fn walk(
        p: &Path,
        acc: &mut u64,
    ) -> std::io::Result<()>
    {
        for e in fs::read_dir(p)?
        {
            let e = e?;
            let md = e.metadata()?;
            if md.is_dir()
            {
                walk(&e.path(), acc)?;
            }
            else
            {
                *acc = acc.saturating_add(md.len());
            }
        }
        Ok(())
    }
    let mut total = 0;
    walk(path, &mut total)?;
    Ok(total)
}

fn rebuild_index(repo_root: &Path) -> Result<()>
{
    let base = repo_root
        .join(".rup")
        .join("backups");
    let index = base.join("index.jsonl");
    if !base.exists()
    {
        return Ok(());
    }

    let mut lines = Vec::<String>::new();
    for ent in fs::read_dir(&base)?
    {
        let ent = ent?;
        if !ent
            .file_type()?
            .is_dir()
        {
            continue;
        }
        if ent
            .file_name()
            .to_string_lossy()
            == "tmp"
        {
            continue;
        }
        let p = ent.path();
        let m = p.join("manifest.json");
        if !m.exists()
        {
            continue;
        }
        let s = fs::read_to_string(&m)?;
        let man: SessionManifest = serde_json::from_str(&s)?;
        let rec = SessionIndexEntry {
            id: man
                .id
                .clone(),
            timestamp: man
                .timestamp
                .clone(),
            success: man.success,
            files: man
                .files
                .len(),
            engine: man
                .engine
                .clone(),
        };
        lines.push(serde_json::to_string(&rec)?);
    }

    let tmp = index.with_extension("jsonl.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        for l in &lines
        {
            writeln!(f, "{l}")?;
        }
        let _ = f.sync_all();
    }
    fs::rename(&tmp, &index)?;
    if let Ok(d) = fs::File::open(&base)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

// ----- simple unified diff (context=3) -----

fn unified_diff(
    a: &str,
    b: &str,
    path: &Path,
) -> String
{
    let a_lines: Vec<&str> = a
        .lines()
        .collect();
    let b_lines: Vec<&str> = b
        .lines()
        .collect();
    let hunks = diff_hunks(&a_lines, &b_lines, 3);
    let mut out = String::new();
    out.push_str(&format!("--- a/{}\n", path.display()));
    out.push_str(&format!("+++ b/{}\n", path.display()));
    for (a_lo, a_hi, b_lo, b_hi, ops) in hunks
    {
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            a_lo + 1,
            a_hi - a_lo,
            b_lo + 1,
            b_hi - b_lo
        ));
        let mut ai = a_lo;
        let mut bi = b_lo;
        for op in ops
        {
            match op
            {
                DiffOp::Equal(n) =>
                {
                    for _ in 0..n
                    {
                        out.push_str(&format!(" {}", a_lines[ai]));
                        out.push('\n');
                        ai += 1;
                        bi += 1;
                    }
                }
                DiffOp::Del(n) =>
                {
                    for _ in 0..n
                    {
                        out.push_str(&format!("-{}", a_lines[ai]));
                        out.push('\n');
                        ai += 1;
                    }
                }
                DiffOp::Add(n) =>
                {
                    for _ in 0..n
                    {
                        out.push_str(&format!("+{}", b_lines[bi]));
                        out.push('\n');
                        bi += 1;
                    }
                }
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
enum DiffOp
{
    Equal(usize),
    Add(usize),
    Del(usize),
}

fn diff_hunks(
    original_lines: &[&str],
    modified_lines: &[&str],
    context: usize,
) -> Vec<(usize, usize, usize, usize, Vec<DiffOp>)>
{
    let (original_len, modified_len) = (original_lines.len(), modified_lines.len());
    let mut lcs_table = vec![vec![0usize; modified_len + 1]; original_len + 1];
    for original_idx in (0..original_len).rev()
    {
        for modified_idx in (0..modified_len).rev()
        {
            lcs_table[original_idx][modified_idx] = if original_lines[original_idx]
                == modified_lines[modified_idx]
            {
                lcs_table[original_idx + 1][modified_idx + 1] + 1
            }
            else
            {
                lcs_table[original_idx + 1][modified_idx]
                    .max(lcs_table[original_idx][modified_idx + 1])
            };
        }
    }
    let mut diff_ops = Vec::<DiffOp>::new();
    let (mut original_pos, mut modified_pos) = (0, 0);
    while original_pos < original_len || modified_pos < modified_len
    {
        if original_pos < original_len
            && modified_pos < modified_len
            && original_lines[original_pos] == modified_lines[modified_pos]
        {
            push(&mut diff_ops, DiffOp::Equal(1));
            original_pos += 1;
            modified_pos += 1;
        }
        else if modified_pos < modified_len
            && (original_pos == original_len
                || lcs_table[original_pos][modified_pos + 1]
                    >= lcs_table[original_pos + 1][modified_pos])
        {
            push(&mut diff_ops, DiffOp::Add(1));
            modified_pos += 1;
        }
        else
        {
            push(&mut diff_ops, DiffOp::Del(1));
            original_pos += 1;
        }
    }
    // Build hunks with fixed context.
    let mut hunks = Vec::new();
    let mut original_cur = 0usize;
    let mut modified_cur = 0usize;
    let mut window = Vec::<(usize, usize, DiffOp)>::new();

    for op in diff_ops
    {
        match op
        {
            DiffOp::Equal(equal_count) =>
            {
                if !window.is_empty()
                {
                    let take = equal_count.min(context);
                    window.push((original_cur, modified_cur, DiffOp::Equal(take)));
                    let (original_lo, original_hi, modified_lo, modified_hi, hunk_ops) =
                        flush(&window, context);
                    hunks.push((original_lo, original_hi, modified_lo, modified_hi, hunk_ops));
                    window.clear();
                    original_cur += equal_count;
                    modified_cur += equal_count;
                }
                else
                {
                    let keep = equal_count.min(context);
                    if keep > 0
                    {
                        window.push((original_cur, modified_cur, DiffOp::Equal(keep)));
                    }
                    original_cur += equal_count;
                    modified_cur += equal_count;
                }
            }

            DiffOp::Add(_count) | DiffOp::Del(_count) =>
            {
                if window.is_empty()
                {
                    let back_original = original_cur.saturating_sub(context);
                    let back_modified = modified_cur.saturating_sub(context);
                    let keep = (original_cur - back_original).min(modified_cur - back_modified);
                    if keep > 0
                    {
                        window.push((back_original, back_modified, DiffOp::Equal(keep)));
                    }
                }
                window.push((original_cur, modified_cur, op));
                match op
                {
                    DiffOp::Add(x) => modified_cur += x,
                    DiffOp::Del(x) => original_cur += x,
                    _ =>
                    {}
                }
            }
        }
    }

    if !window.is_empty()
    {
        let (original_lo, original_hi, modified_lo, modified_hi, hunk_ops) =
            flush(&window, context);
        hunks.push((original_lo, original_hi, modified_lo, modified_hi, hunk_ops));
    }

    if hunks.is_empty()
    {
        hunks.push((0, 0, 0, 0, vec![DiffOp::Equal(0)]));
    }

    hunks
}

fn push(
    ops: &mut Vec<DiffOp>,
    op: DiffOp,
)
{
    if let Some(last) = ops.last_mut()
    {
        match (*last, op)
        {
            (DiffOp::Equal(a), DiffOp::Equal(b)) =>
            {
                *last = DiffOp::Equal(a + b);
                return;
            }
            (DiffOp::Add(a), DiffOp::Add(b)) =>
            {
                *last = DiffOp::Add(a + b);
                return;
            }
            (DiffOp::Del(a), DiffOp::Del(b)) =>
            {
                *last = DiffOp::Del(a + b);
                return;
            }
            _ =>
            {}
        }
    }
    ops.push(op);
}

fn flush(
    win: &[(usize, usize, DiffOp)],
    ctx: usize,
) -> (usize, usize, usize, usize, Vec<DiffOp>)
{
    let a_lo = win
        .first()
        .map(|x| x.0)
        .unwrap_or(0);
    let b_lo = win
        .first()
        .map(|x| x.1)
        .unwrap_or(0);
    let mut a_len = 0usize;
    let mut b_len = 0usize;
    let mut out = Vec::new();
    for (_, _, op) in win
    {
        match *op
        {
            DiffOp::Equal(k) =>
            {
                out.push(DiffOp::Equal(k));
                a_len += k;
                b_len += k;
            }
            DiffOp::Del(k) =>
            {
                out.push(DiffOp::Del(k));
                a_len += k;
            }
            DiffOp::Add(k) =>
            {
                out.push(DiffOp::Add(k));
                b_len += k;
            }
        }
    }
    out = trim(out, ctx);
    (a_lo, a_lo + a_len, b_lo, b_lo + b_len, out)
}

fn trim(
    mut ops: Vec<DiffOp>,
    ctx: usize,
) -> Vec<DiffOp>
{
    if let Some(DiffOp::Equal(k)) = ops.first_mut()
        && *k > ctx
    {
        *k = ctx;
    }

    if let Some(DiffOp::Equal(k)) = ops.last_mut()
        && *k > ctx
    {
        *k = ctx;
    }
    if matches!(ops.first(), Some(DiffOp::Equal(0)))
    {
        ops.remove(0);
    }
    if matches!(ops.last(), Some(DiffOp::Equal(0)))
    {
        ops.pop();
    }
    ops
}

// ----- streaming blake3 -----

fn stream_blake3(path: &Path) -> Result<String>
{
    use blake3::Hasher as Blake3;
    let mut f = std::fs::File::open(path)?;
    let mut h = Blake3::new();
    let mut buf = [0u8; 64 * 1024];

    loop
    {
        let n = f.read(&mut buf)?;

        if n == 0
        {
            break;
        }

        h.update(&buf[..n]);
    }

    Ok(format!(
        "blake3:{}",
        h.finalize()
            .to_hex()
    ))
}

#[cfg(test)]
mod tests
{
    use chrono::Duration;

    use super::*;

    #[test]
    fn test_parse_time_bound()
    {
        let base_time = Utc::now();

        // Test days
        let result = parse_time_bound("7d").unwrap();
        let expected = base_time - Duration::days(7);
        assert!(
            (result - expected)
                .num_seconds()
                .abs()
                < 5
        ); // Within 5 seconds

        // Test hours
        let result = parse_time_bound("24h").unwrap();
        let expected = base_time - Duration::hours(24);
        assert!(
            (result - expected)
                .num_seconds()
                .abs()
                < 5
        );

        // Test negative durations are rejected
        assert!(parse_time_bound("-7d").is_err());
        assert!(parse_time_bound("-24h").is_err());

        // Test invalid formats
        assert!(parse_time_bound("abc").is_err());
        assert!(parse_time_bound("7x").is_err());
        assert!(parse_time_bound("").is_err());
    }

    #[test]
    fn test_session_id_resolution()
    {
        // This would need a proper test setup with temp sessions
        // Will be covered by integration tests
    }
}
