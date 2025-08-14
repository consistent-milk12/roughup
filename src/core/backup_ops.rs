//! Backup session management operations
//!
//! Provides high-level operations for listing, showing, restoring, and cleaning up
//! backup sessions. This module implements the Phase B2 backup management functionality
//! with safe defaults and comprehensive error handling.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::backup::{SessionManifest, list_sessions, read_session_manifest};

/// Session ID resolution result
#[derive(Debug)]
pub enum SessionIdResolution {
    /// Single session found
    Single(String),
    /// Multiple matches found
    Multiple(Vec<String>),
    /// No matches found  
    NotFound,
}

/// Concise session info for listing
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub timestamp: String,
    pub engine: String,
    pub success: bool,
    pub files: usize,
    pub sample_paths: Vec<String>, // First 3 files for quick scanning
}

/// Request structure for listing sessions
#[derive(Debug)]
pub struct ListRequest {
    pub successful: bool,
    pub engine: Option<String>,
    pub since: Option<String>,
    pub limit: usize,
    pub sort_desc: bool,
}

/// Request structure for showing session details
#[derive(Debug)]
pub struct ShowRequest {
    pub id: String,
    pub verbose: bool,
}

/// Response for show command
#[derive(Debug, Serialize)]
pub struct ShowResponse {
    pub manifest: SessionManifest,
    pub session_path: PathBuf,
    pub total_size: Option<u64>,
}

/// Request structure for restore operations
#[derive(Debug)]
pub struct RestoreRequest {
    pub session_id: String,
    pub path: Option<PathBuf>,
    pub dry_run: bool,
    pub show_diff: bool,
    pub force: bool,
    pub verify_checksum: bool,
    pub backup_current: bool,
}

/// Result of restore operation
#[derive(Debug, Serialize)]
pub struct RestoreResult {
    pub session_id: String,
    pub restored: Vec<PathBuf>,
    pub conflicts: Vec<PathBuf>,
    pub backed_up_current: bool,
    pub backup_session_id: Option<String>,
}

/// Request structure for cleanup operations
#[derive(Debug)]
pub struct CleanupRequest {
    pub older_than: Option<String>,
    pub keep_latest: Option<usize>,
    pub dry_run: bool,
    pub include_incomplete: bool,
}

/// Result of cleanup operation
#[derive(Debug, Serialize)]
pub struct CleanupResult {
    pub sessions_removed: Vec<String>,
    pub bytes_freed: u64,
    pub errors: Vec<String>,
}

/// List sessions with filters, minimizing manifest IO
pub fn list_sessions_filtered(repo_root: &Path, req: ListRequest) -> Result<Vec<SessionInfo>> {
    // Parse "since" once
    let since_time = if let Some(ref s) = req.since {
        Some(parse_relative_time(s)?)
    } else {
        None
    };

    // Load index entries
    let mut entries = list_sessions(repo_root)?;

    // Keep only completed sessions
    entries.retain(|e| session_is_complete(repo_root, &e.id).unwrap_or(false));

    // Apply filters that require only index data
    if req.successful {
        entries.retain(|e| e.success);
    }

    if let Some(ref engine_filter) = req.engine {
        // Case-insensitive engine matching
        let target = engine_filter.to_ascii_lowercase();
        entries.retain(|e| e.engine.to_ascii_lowercase() == target);
    }

    if let Some(since) = since_time {
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
        if req.sort_desc {
            bp.cmp(&ap).then_with(|| b.timestamp.cmp(&a.timestamp))
        } else {
            ap.cmp(&bp).then_with(|| a.timestamp.cmp(&b.timestamp))
        }
    });

    // Truncate to limit before manifest reads
    if entries.len() > req.limit {
        entries.truncate(req.limit);
    }

    // Collect SessionInfo; now read manifests only for sample paths
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        // Try to read manifest to extract first 3 sample paths
        let sample_paths = match read_session_manifest(repo_root, &e.id) {
            Ok(m) => m
                .files
                .iter()
                .take(3)
                .map(|f| f.rel_path.display().to_string())
                .collect(),
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
pub fn show_session(repo_root: &Path, req: ShowRequest) -> Result<ShowResponse> {
    let session_id = resolve_session_id(repo_root, &req.id)?;
    let manifest = read_session_manifest(repo_root, &session_id)?;
    let session_path = repo_root.join(".rup").join("backups").join(&session_id);

    // Calculate total size if verbose
    let total_size = if req.verbose {
        Some(calculate_session_size(&session_path)?)
    } else {
        None
    };

    Ok(ShowResponse {
        manifest,
        session_path,
        total_size,
    })
}

/// Resolve session ID (supports full, short, and aliases)
pub fn resolve_session_id(repo_root: &Path, query: &str) -> Result<String> {
    match resolve_session_id_internal(repo_root, query)? {
        SessionIdResolution::Single(id) => Ok(id),
        SessionIdResolution::Multiple(matches) => {
            bail!(
                "Ambiguous session ID '{}'. Matches: {}",
                query,
                matches.join(", ")
            );
        }
        SessionIdResolution::NotFound => {
            bail!("No session found matching '{}'", query);
        }
    }
}

// Resolve session ID (internal): prefer completed sessions when using aliases
fn resolve_session_id_internal(repo_root: &Path, query: &str) -> Result<SessionIdResolution> {
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
            (s.id.clone(), s.timestamp.clone(), s.success, parsed)
        })
        .collect();

    // Handle aliases first
    match query {
        // latest: choose newest completed session by parsed time
        "latest" => {
            // Filter completed
            let mut cands: Vec<_> = entries
                .iter()
                .filter(|(id, _, _, _)| is_complete(id))
                .collect();
            // Sort by parsed time desc, then by string desc as tiebreaker
            cands.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| b.1.cmp(&a.1)));
            return Ok(match cands.first() {
                Some((id, ..)) => SessionIdResolution::Single(id.clone()),
                None => SessionIdResolution::NotFound,
            });
        }
        // last-successful: newest completed AND success=true
        "last-successful" => {
            let mut cands: Vec<_> = entries
                .iter()
                .filter(|(id, _, success, _)| *success && is_complete(id))
                .collect();
            cands.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| b.1.cmp(&a.1)));
            return Ok(match cands.first() {
                Some((id, ..)) => SessionIdResolution::Single(id.clone()),
                None => SessionIdResolution::NotFound,
            });
        }
        _ => {}
    }

    // Collect matches (exact, short-suffix, date-prefix)
    let mut matches: Vec<(String, Option<DateTime<Utc>>, String)> = Vec::new();

    for (id, ts, _success, parsed) in &entries {
        // Exact match
        if id == query {
            return Ok(SessionIdResolution::Single(id.clone()));
        }
        // Short ID (require a minimal length to reduce noise)
        if query.len() >= 8 && id.ends_with(query) {
            matches.push((id.clone(), *parsed, ts.clone()));
        }
        // Date prefix like "2025-08-14"
        if query.contains('-') && id.starts_with(query) {
            matches.push((id.clone(), *parsed, ts.clone()));
        }
    }

    // Sort matches newest-first for better ambiguity messages
    matches.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));

    // Return resolution
    Ok(match matches.len() {
        0 => SessionIdResolution::NotFound,
        1 => SessionIdResolution::Single(matches[0].0.clone()),
        _ => SessionIdResolution::Multiple(matches.into_iter().map(|(id, _, _)| id).collect()),
    })
}

/// Check if session is complete (has DONE marker)
fn session_is_complete(repo_root: &Path, session_id: &str) -> Result<bool> {
    let done_path = repo_root
        .join(".rup")
        .join("backups")
        .join(session_id)
        .join("DONE");
    Ok(done_path.exists())
}

/// Parse relative time specifications like "7d", "24h"
fn parse_relative_time(time_str: &str) -> Result<DateTime<Utc>> {
    // Trim and validate
    let time_str = time_str.trim();
    if time_str.is_empty() {
        bail!("Empty time specification");
    }

    // Split number and unit
    let (number_str, unit) = match time_str.chars().last() {
        Some('d' | 'h' | 'm' | 's') => (
            &time_str[..time_str.len() - 1],
            time_str.chars().last().unwrap(),
        ),
        _ => bail!("Invalid time unit in '{}'. Use d, h, m, or s", time_str),
    };

    // Parse and reject negatives
    let number: i64 = number_str
        .parse()
        .with_context(|| format!("Invalid number '{}' in time specification", number_str))?;
    if number < 0 {
        bail!("Negative durations are not allowed: '{}'", time_str);
    }

    // Map to chrono::Duration
    let duration = match unit {
        'd' => Duration::days(number),
        'h' => Duration::hours(number),
        'm' => Duration::minutes(number),
        's' => Duration::seconds(number),
        _ => unreachable!(),
    };

    // Compute bound
    Ok(Utc::now() - duration)
}

/// Compute size of backed-up payload (exclude manifest and DONE)
fn calculate_session_size(session_path: &Path) -> Result<u64> {
    // Accumulator
    let mut total_size = 0u64;

    // Recursive visitor
    fn visit_dir(dir: &Path, total: &mut u64) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let md = entry.metadata()?;

            // Recurse into directories
            if md.is_dir() {
                visit_dir(&path, total)?;
                continue;
            }

            // Skip metadata files
            let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if fname == "manifest.json" || fname == "DONE" {
                continue;
            }

            // Sum file size
            *total += md.len();
        }
        Ok(())
    }

    // If session dir exists, walk it
    if session_path.exists() {
        visit_dir(session_path, &mut total_size)?;
    }

    Ok(total_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_relative_time() {
        let base_time = Utc::now();

        // Test days
        let result = parse_relative_time("7d").unwrap();
        let expected = base_time - Duration::days(7);
        assert!((result - expected).num_seconds().abs() < 5); // Within 5 seconds

        // Test hours
        let result = parse_relative_time("24h").unwrap();
        let expected = base_time - Duration::hours(24);
        assert!((result - expected).num_seconds().abs() < 5);

        // Test negative durations are rejected
        assert!(parse_relative_time("-7d").is_err());
        assert!(parse_relative_time("-24h").is_err());

        // Test invalid formats
        assert!(parse_relative_time("abc").is_err());
        assert!(parse_relative_time("7x").is_err());
        assert!(parse_relative_time("").is_err());
    }

    #[test]
    fn test_session_id_resolution() {
        // This would need a proper test setup with temp sessions
        // Will be covered by integration tests
    }
}
