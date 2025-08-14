//! Centralized backup system with session-scoped mirrored directory structure.
//!
//! Creates timestamped sessions under `.rup/backups/<ID>` with a manifest and a
//! DONE marker for crash safety. Writes occur in `.rup/backups/tmp/<ID>` and are
//! atomically renamed into place on finalize.

use anyhow::{Context, Result, bail};
use blake3::Hasher as Blake3;
use chrono::Utc;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Per-file metadata recorded in the session manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBackupMeta {
    pub original_path: PathBuf,   // repo-relative
    pub rel_path: PathBuf,        // session-relative (mirrors repo tree)
    pub size_bytes: u64,          // backed-up content size
    pub last_modified: u64,       // secs since UNIX_EPOCH (source file)
    pub checksum: Option<String>, // blake3:<hex>
    pub symlink: bool,            // whether source was a symlink
    pub link_target: Option<PathBuf>, // recorded link target (if any)
                                  // Note: no hashed-fallback needed when mirroring the tree
}

/// Git snapshot captured at session start (best-effort).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitSnapshot {
    pub commit: String,
    pub branch: Option<String>,
    pub dirty: bool,
    pub staged: bool,
}

/// Manifest describing a completed or in-progress session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManifest {
    pub id: String,        // e.g., 2025-08-14T10-30-15Z_a9Jh5
    pub timestamp: String, // RFC3339 creation time
    pub parent_session_id: Option<String>,
    pub operation: String,              // e.g., "apply", "restore"
    pub engine: String,                 // "internal" | "git" | "auto"
    pub edit_spec_hash: Option<String>, // short hash if available
    pub git: Option<GitSnapshot>,
    pub args: Vec<String>,    // CLI args snapshot
    pub success: bool,        // set on finalize
    pub last_updated: String, // RFC3339
    pub files: Vec<FileBackupMeta>,
}

/// Lightweight index record for quick session listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIndexEntry {
    pub id: String,
    pub timestamp: String,
    pub success: bool,
    pub files: usize,
    pub engine: String,
}

/// Manager creating a single session; stage in tmp, then finalize.
#[derive(Debug)]
pub struct BackupManager {
    repo_root: PathBuf,
    sessions_dir: PathBuf, // .../.rup/backups
    // tmp_sessions_dir: PathBuf, // .../.rup/backups/tmp
    locks_dir: PathBuf, // .../.rup/locks
    session_id: String,
    session_tmp_dir: PathBuf,   // .../tmp/<id>
    session_final_dir: PathBuf, // .../backups/<id>
    manifest: SessionManifest,
    finalized: bool,
}

impl BackupManager {
    /// Start a new session under `.rup/backups/tmp/<ID>`.
    pub fn begin(repo_root: &Path, engine: &str) -> Result<Self> {
        let rup_root = repo_root.join(".rup");
        let sessions_dir = rup_root.join("backups");
        let tmp_sessions_dir = sessions_dir.join("tmp");
        let locks_dir = rup_root.join("locks");

        fs::create_dir_all(&sessions_dir)
            .with_context(|| format!("create backups dir: {}", sessions_dir.display()))?;
        fs::create_dir_all(&tmp_sessions_dir)
            .with_context(|| format!("create tmp dir: {}", tmp_sessions_dir.display()))?;
        fs::create_dir_all(&locks_dir)
            .with_context(|| format!("create locks dir: {}", locks_dir.display()))?;

        let session_id = generate_session_id();
        let session_tmp_dir = tmp_sessions_dir.join(&session_id);
        let session_final_dir = sessions_dir.join(&session_id);

        fs::create_dir_all(&session_tmp_dir)
            .with_context(|| format!("create session tmp: {}", session_tmp_dir.display()))?;

        let now = Utc::now().to_rfc3339();
        let manifest = SessionManifest {
            id: session_id.clone(),
            timestamp: now.clone(),
            parent_session_id: None,
            operation: "apply".into(), // configurable by caller if needed
            engine: engine.into(),
            edit_spec_hash: None,
            git: capture_git_snapshot(repo_root).ok(),
            args: std::env::args().collect(),
            success: false,
            last_updated: now,
            files: Vec::new(),
        };

        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            sessions_dir,
            // tmp_sessions_dir,
            locks_dir,
            session_id,
            session_tmp_dir,
            session_final_dir,
            manifest,
            finalized: false,
        })
    }

    /// Back up a single repo-relative file; follows symlinks for content.
    pub fn backup_file(&mut self, rel_path: &Path) -> Result<()> {
        let rel = validate_repo_rel(rel_path)?;
        let source_path = self.repo_root.join(&rel);
        let backup_path = self.session_tmp_dir.join(&rel);

        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create backup parent: {}", parent.display()))?;
        }

        let meta = fs::symlink_metadata(&source_path)
            .with_context(|| format!("stat source: {}", source_path.display()))?;
        let ty = meta.file_type();
        let is_symlink = ty.is_symlink();

        // Fix #9: Validate file type - only regular files and symlinks are supported
        if !(ty.is_file() || ty.is_symlink()) {
            bail!("unsupported file type for backup: {}", rel.display());
        }

        let (symlink, link_target) = if is_symlink {
            let t = fs::read_link(&source_path)
                .with_context(|| format!("readlink: {}", source_path.display()))?;
            (true, Some(t))
        } else {
            (false, None)
        };

        // Copy content: follow symlink to target bytes (policy).
        if symlink {
            // Fix #8: Better error handling for broken symlinks
            let resolved = fs::canonicalize(&source_path).with_context(|| {
                format!(
                    "resolve symlink target (broken?): {}",
                    source_path.display()
                )
            })?;
            fs::copy(&resolved, &backup_path)
                .with_context(|| format!("copy target to backup: {}", backup_path.display()))?;
        } else {
            fs::copy(&source_path, &backup_path)
                .with_context(|| format!("copy file to backup: {}", backup_path.display()))?;
        }

        // Content-based accounting from the backup copy.
        let size_bytes = fs::metadata(&backup_path)
            .with_context(|| format!("stat backup: {}", backup_path.display()))?
            .len();
        let last_modified = meta
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let checksum = Some(stream_blake3(&backup_path)?);

        self.manifest.files.push(FileBackupMeta {
            original_path: rel.clone(),
            rel_path: rel,
            size_bytes,
            last_modified,
            checksum,
            symlink,
            link_target,
        });
        self.manifest.last_updated = Utc::now().to_rfc3339();
        Ok(())
    }

    /// Session identifier.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Session directory (tmp while unfinalized; final after finalize).
    pub fn session_dir(&self) -> &Path {
        if self.finalized {
            &self.session_final_dir
        } else {
            &self.session_tmp_dir
        }
    }

    /// Number of files recorded so far.
    pub fn file_count(&self) -> usize {
        self.manifest.files.len()
    }

    /// Write manifest, atomically rename tmp→final, create DONE, append index.
    pub fn finalize(&mut self, success: bool) -> Result<()> {
        if self.finalized {
            return Ok(());
        }
        // Don't set finalized = true yet - move it after successful rename + DONE creation

        self.manifest.success = success;
        self.manifest.last_updated = Utc::now().to_rfc3339();

        // Fix #4: Atomic manifest write via temp file
        let manifest_path = self.session_tmp_dir.join("manifest.json");
        let manifest_tmp = self.session_tmp_dir.join("manifest.json.tmp");
        let manifest_text =
            serde_json::to_string_pretty(&self.manifest).context("serialize manifest")?;
        fs::write(&manifest_tmp, &manifest_text)
            .with_context(|| format!("write manifest tmp: {}", manifest_tmp.display()))?;
        File::open(&manifest_tmp)?.sync_all().ok();
        fs::rename(&manifest_tmp, &manifest_path)?;
        let _ = sync_dir(&self.session_tmp_dir);

        // Atomic rename from tmp to final.
        fs::rename(&self.session_tmp_dir, &self.session_final_dir).with_context(|| {
            format!(
                "rename {} → {}",
                self.session_tmp_dir.display(),
                self.session_final_dir.display()
            )
        })?;

        // Durably record the rename.
        let _ = sync_dir(&self.sessions_dir);

        // Create DONE and sync it + final dir.
        let done_path = self.session_final_dir.join("DONE");
        fs::write(&done_path, "")
            .with_context(|| format!("create DONE: {}", done_path.display()))?;
        File::open(&done_path)?.sync_all().ok();
        let _ = sync_dir(&self.session_final_dir);

        // Fix #3: Mark finalized only after successful rename + DONE creation
        self.finalized = true;

        // Append to index under lock.
        self.append_to_index()?;
        Ok(())
    }

    fn append_to_index(&self) -> Result<()> {
        let index_path = self.sessions_dir.join("index.jsonl");
        let lock_path = self.locks_dir.join("backups.lock");
        let _guard = acquire_lock(&lock_path)?;

        let entry = SessionIndexEntry {
            id: self.manifest.id.clone(),
            timestamp: self.manifest.timestamp.clone(),
            success: self.manifest.success,
            files: self.manifest.files.len(),
            engine: self.manifest.engine.clone(),
        };
        let line = serde_json::to_string(&entry).context("serialize index entry")?;

        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&index_path)
            .with_context(|| format!("open index: {}", index_path.display()))?;
        writeln!(f, "{line}").context("append index")?;
        f.sync_all().ok();

        Ok(())
    }
}

impl Drop for BackupManager {
    fn drop(&mut self) {
        if !self.finalized {
            let _ = self.finalize(false); // best-effort failure finalize
        }
    }
}

/// Cross-platform directory fsync helper.
#[cfg(unix)]
fn sync_dir(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY)
        .open(p)?;
    f.sync_all()
}

#[cfg(windows)]
fn sync_dir(_p: &Path) -> std::io::Result<()> {
    // Windows does not expose a reliable directory fsync; best-effort no-op.
    Ok(())
}

/// Generate a sortable, filesystem-safe session ID.
fn generate_session_id() -> String {
    let ts = Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::rng();
    let suffix: String = (0..10)
        .map(|_| {
            let idx = rng.random_range(0..alphabet.len());
            alphabet[idx] as char
        })
        .collect();
    format!("{}_{}", ts, suffix)
}

/// Capture git status (best-effort; falls back to "unknown").
fn capture_git_snapshot(repo_root: &Path) -> Result<GitSnapshot> {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".into());

    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo_root)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s == "HEAD" { None } else { Some(s) }
            } else {
                None
            }
        });

    let dirty = std::process::Command::new("git")
        .args(["diff", "--quiet"])
        .current_dir(repo_root)
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    let staged = std::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(repo_root)
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    Ok(GitSnapshot {
        commit,
        branch,
        dirty,
        staged,
    })
}

/// Stream a file into a blake3 digest as `blake3:<hex>`.
fn stream_blake3(path: &Path) -> Result<String> {
    let mut f =
        File::open(path).with_context(|| format!("open for checksum: {}", path.display()))?;
    let mut hasher = Blake3::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("blake3:{}", hasher.finalize().to_hex()))
}

/// Acquire a simple file lock; guard deletes the lock on drop.
struct LockGuard {
    path: PathBuf,
    file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.file.sync_all();
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_lock(lock_path: &Path) -> Result<LockGuard> {
    // First try to create the lock file
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
    {
        Ok(mut file) => {
            writeln!(file, "pid={}", std::process::id()).ok();
            file.sync_all().ok();
            Ok(LockGuard {
                path: lock_path.to_path_buf(),
                file,
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Fix #5: Handle stale locks - check if lock is old (>60 seconds)
            if let Ok(meta) = fs::metadata(lock_path)
                && let Ok(modified) = meta.modified()
                && let Ok(elapsed) = modified.elapsed()
                && elapsed.as_secs() > 60
            {
                // Lock is stale, try to remove and retry once
                if fs::remove_file(lock_path).is_ok() {
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(lock_path)
                        .with_context(|| {
                            format!("acquire lock after stale cleanup: {}", lock_path.display())
                        })?;

                    writeln!(file, "pid={}", std::process::id()).ok();

                    file.sync_all().ok();

                    return Ok(LockGuard {
                        path: lock_path.to_path_buf(),
                        file,
                    });
                }
            }
            Err(anyhow::Error::new(e).context(format!("acquire lock: {}", lock_path.display())))
        }
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("acquire lock: {}", lock_path.display())))
        }
    }
}

/// Read the append-only index; ignores malformed lines.
pub fn list_sessions(repo_root: &Path) -> Result<Vec<SessionIndexEntry>> {
    let index_path = repo_root.join(".rup").join("backups").join("index.jsonl");
    if !index_path.exists() {
        return Ok(Vec::new());
    }

    let file =
        File::open(&index_path).with_context(|| format!("open index: {}", index_path.display()))?;
    let reader = BufReader::new(file);

    let mut out = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("read index line {}", i + 1))?;
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        match serde_json::from_str::<SessionIndexEntry>(t) {
            Ok(e) => out.push(e),
            Err(_) => continue, // tolerate partial/corrupt lines
        }
    }
    Ok(out)
}

/// Load a session manifest; requires DONE to be present.
pub fn read_session_manifest(repo_root: &Path, session_id: &str) -> Result<SessionManifest> {
    let base = repo_root.join(".rup").join("backups").join(session_id);
    let done = base.join("DONE");
    if !done.exists() {
        bail!("Session {} is incomplete (missing DONE)", session_id);
    }
    let manifest_path = base.join("manifest.json");
    let s = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read manifest: {}", manifest_path.display()))?;
    let m: SessionManifest = serde_json::from_str(&s)
        .with_context(|| format!("parse manifest: {}", manifest_path.display()))?;
    Ok(m)
}

/// Validate that the given path is repo-relative and non-escaping.
fn validate_repo_rel(p: &Path) -> Result<PathBuf> {
    if p.is_absolute() {
        bail!("path must be repo-relative: {}", p.display());
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => bail!("path escapes repo: {}", p.display()),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir => {
                bail!("path must be repo-relative: {}", p.display())
            }
            _ => out.push(c.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        bail!("empty path");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn basic_session_flow() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path();
        fs::write(repo.join("file.txt"), "hello").unwrap();

        let mut mgr = BackupManager::begin(repo, "internal").unwrap();
        mgr.backup_file(Path::new("file.txt")).unwrap();
        mgr.finalize(true).unwrap();

        let idx = list_sessions(repo).unwrap();
        assert_eq!(idx.len(), 1);
        assert!(idx[0].success);
        assert_eq!(idx[0].files, 1);

        let m = read_session_manifest(repo, &idx[0].id).unwrap();
        assert_eq!(m.files.len(), 1);
        assert_eq!(m.files[0].original_path, Path::new("file.txt"));
    }

    #[test]
    fn preserves_mirrored_tree() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path();
        fs::create_dir_all(repo.join("src/core")).unwrap();
        fs::write(repo.join("src/core/x.rs"), "fn main(){}").unwrap();

        let mut mgr = BackupManager::begin(repo, "auto").unwrap();
        mgr.backup_file(Path::new("src/core/x.rs")).unwrap();
        mgr.finalize(true).unwrap();

        let id = list_sessions(repo).unwrap()[0].id.clone();
        let backed = repo.join(".rup/backups").join(id).join("src/core/x.rs");
        assert!(backed.exists());
        assert_eq!(fs::read_to_string(backed).unwrap(), "fn main(){}");
    }
}
