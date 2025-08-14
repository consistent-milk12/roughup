//! Centralized backup system with session-scoped mirrored directory structure
//!
//! Provides atomic backup operations that mirror the project structure within
//! timestamped session directories under `.rup/backups/`. Each session includes
//! a manifest with metadata and a DONE marker for crash safety.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// Represent a single file's backup metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBackupMeta {
    // Original project-relative path
    pub original_path: PathBuf,
    // Session-relative path (mirrors project tree)
    pub rel_path: PathBuf,
    // File size in bytes
    pub size_bytes: u64,
    // Last modification time (seconds since UNIX_EPOCH)
    pub last_modified: u64,
    // Optional checksum (blake3)
    pub checksum: Option<String>,
    // Symlink marker and target if applicable
    pub symlink: bool,
    pub link_target: Option<PathBuf>,
    // Fallback for platform-specific path issues
    pub fallback_hashed_name: Option<String>,
}

// Git snapshot details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitSnapshot {
    pub commit: String,
    pub branch: Option<String>,
    pub dirty: bool,
    pub staged: bool,
}

// Represent a session manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManifest {
    // Session id like "2025-08-14T10-30-15Z_a9Jh5"
    pub id: String,
    // RFC 3339 timestamp
    pub timestamp: String,
    // Pointer to parent for nested applies
    pub parent_session_id: Option<String>,
    // Operation type
    pub operation: String,
    // Engine used (internal/git/auto)
    pub engine: String,
    // Short hash of the edit spec
    pub edit_spec_hash: Option<String>,
    // Git snapshot info (optional)
    pub git: Option<GitSnapshot>,
    // Command line arguments used
    pub args: Vec<String>,
    // True if all backups succeeded
    pub success: bool,
    // Last updated timestamp
    pub last_updated: String,
    // File entries
    pub files: Vec<FileBackupMeta>,
}

// Index entry for quick session lookup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIndexEntry {
    pub id: String,
    pub timestamp: String,
    pub success: bool,
    pub files: usize,
    pub engine: String,
}

// Public manager used by apply/preview flows
#[derive(Debug)]
pub struct BackupManager {
    // Repo root
    repo_root: PathBuf,
    // Session directory (in tmp before finalize)
    session_tmp_dir: PathBuf,
    // Final session id and path (when finalized)
    session_id: String,
    session_final_dir: PathBuf,
    // Manifest under construction
    manifest: SessionManifest,
    // Track whether we've been finalized
    finalized: bool,
}

impl BackupManager {
    // Create a new manager with a fresh session id, under .rup/tmp/
    pub fn begin(repo_root: &Path, engine: &str) -> Result<Self> {
        let rup_root = repo_root.join(".rup");
        let backups_dir = rup_root.join("backups");
        let tmp_dir = rup_root.join("tmp");
        let locks_dir = rup_root.join("locks");

        // Create directory structure
        fs::create_dir_all(&backups_dir)
            .with_context(|| format!("Failed to create backups directory: {:?}", backups_dir))?;
        fs::create_dir_all(&tmp_dir)
            .with_context(|| format!("Failed to create tmp directory: {:?}", tmp_dir))?;
        fs::create_dir_all(&locks_dir)
            .with_context(|| format!("Failed to create locks directory: {:?}", locks_dir))?;

        // Generate session ID with timestamp and short random suffix
        let session_id = generate_session_id();
        let session_tmp_dir = tmp_dir.join(format!("session-{}", session_id));
        let session_final_dir = backups_dir.join(&session_id);

        // Create session temp directory
        fs::create_dir_all(&session_tmp_dir)
            .with_context(|| format!("Failed to create session tmp dir: {:?}", session_tmp_dir))?;

        // Initialize manifest
        let timestamp = chrono::Utc::now().to_rfc3339();
        let manifest = SessionManifest {
            id: session_id.clone(),
            timestamp: timestamp.clone(),
            parent_session_id: None,        // TODO: Support nested sessions
            operation: "apply".to_string(), // TODO: Make configurable
            engine: engine.to_string(),
            edit_spec_hash: None, // TODO: Add spec hash
            git: capture_git_snapshot(repo_root).ok(),
            args: std::env::args().collect(), // Capture command line args
            success: false,                   // Will be set during finalize
            last_updated: timestamp,
            files: Vec::new(),
        };

        Ok(BackupManager {
            repo_root: repo_root.to_path_buf(),
            session_tmp_dir,
            session_id,
            session_final_dir,
            manifest,
            finalized: false,
        })
    }

    // Back up a single file, creating subdirectories and copying bytes
    pub fn backup_file(&mut self, rel_path: &Path) -> Result<()> {
        let source_path = self.repo_root.join(rel_path);
        let backup_path = self.session_tmp_dir.join(rel_path);

        // Create parent directories in backup location
        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create backup parent dirs: {:?}", parent))?;
        }

        // Use symlink_metadata to avoid following links for detection.
        // Then decide how to copy. Current policy: copy target contents.
        let meta = fs::symlink_metadata(&source_path)
            .with_context(|| format!("Failed to read symlink metadata for: {:?}", source_path))?;

        let ftype = meta.file_type();
        let is_symlink = ftype.is_symlink();

        let (symlink, link_target) = if is_symlink {
            let target = fs::read_link(&source_path)
                .with_context(|| format!("Failed to read symlink: {:?}", source_path))?;
            (true, Some(target))
        } else {
            (false, None)
        };

        let size_bytes = meta.len();
        let last_modified = meta
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Copy contents: follow link for content backup, but record link.
        if symlink {
            // Canonicalize and copy the target bytes
            let resolved = fs::canonicalize(&source_path)
                .with_context(|| format!("Failed to resolve symlink: {:?}", source_path))?;
            fs::copy(&resolved, &backup_path)
                .with_context(|| format!("Failed to backup symlink target: {:?}", resolved))?;
        } else {
            fs::copy(&source_path, &backup_path)
                .with_context(|| format!("Failed to backup file: {:?}", source_path))?;
        }

        // Calculate checksum (optional for Phase 1, using simple approach)
        let checksum = calculate_file_checksum(&backup_path).ok();

        // Create backup metadata
        let file_meta = FileBackupMeta {
            original_path: rel_path.to_path_buf(),
            rel_path: rel_path.to_path_buf(),
            size_bytes,
            last_modified,
            checksum,
            symlink,
            link_target,
            fallback_hashed_name: None, // Phase 1: no fallback needed with mirrored paths
        };

        self.manifest.files.push(file_meta);
        self.manifest.last_updated = chrono::Utc::now().to_rfc3339();

        Ok(())
    }

    /// Get the session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get the session directory (temporary before finalize, final after finalize)
    pub fn session_dir(&self) -> &Path {
        if self.finalized {
            &self.session_final_dir
        } else {
            &self.session_tmp_dir
        }
    }

    /// Get the number of backed up files
    pub fn file_count(&self) -> usize {
        self.manifest.files.len()
    }

    // Write manifest.json and atomically rename tmp→backups/<id>/, then create DONE marker
    // Change signature to take &mut self, not self
    // This allows calling from Drop without moving out of self.
    pub fn finalize(&mut self, success: bool) -> Result<()> {
        // Make idempotent via the flag
        if self.finalized {
            return Ok(());
        }
        self.finalized = true;

        self.manifest.success = success;
        self.manifest.last_updated = chrono::Utc::now().to_rfc3339();

        // Write manifest to tmp directory
        let manifest_path = self.session_tmp_dir.join("manifest.json");
        let manifest_content = serde_json::to_string_pretty(&self.manifest)
            .with_context(|| "Failed to serialize manifest")?;

        fs::write(&manifest_path, manifest_content)
            .with_context(|| format!("Failed to write manifest: {:?}", manifest_path))?;

        // Fsync the manifest file
        let manifest_file = File::open(&manifest_path)
            .with_context(|| format!("Failed to open manifest for sync: {:?}", manifest_path))?;
        manifest_file
            .sync_all()
            .with_context(|| "Failed to sync manifest file")?;

        // After writing and fsyncing manifest file:
        let session_tmp_dir_fd = File::open(&self.session_tmp_dir)?;
        session_tmp_dir_fd.sync_all()?;

        // Rename tmp → final
        fs::rename(&self.session_tmp_dir, &self.session_final_dir).with_context(|| {
            format!(
                "Failed to rename session from {:?} to {:?}",
                self.session_tmp_dir, self.session_final_dir
            )
        })?;

        // Fsync the parent dir of the destination to persist the rename
        let backups_dir = self.repo_root.join(".rup").join("backups");
        if let Ok(dirfd) = File::open(&backups_dir) {
            let _ = dirfd.sync_all(); // ignore on platforms that do not support it
        }

        // Create DONE and fsync it
        let done_path = self.session_final_dir.join("DONE");
        fs::write(&done_path, "")
            .with_context(|| format!("Failed to create DONE marker: {:?}", done_path))?;
        if let Ok(donefd) = File::open(&done_path) {
            let _ = donefd.sync_all();
        }

        // Finally, fsync the final session directory to flush directory entries
        if let Ok(final_dirfd) = File::open(&self.session_final_dir) {
            let _ = final_dirfd.sync_all();
        }

        // Append to index with lock
        self.append_to_index()?;

        Ok(())
    }

    // Append session info to index.jsonl with locking
    fn append_to_index(&self) -> Result<()> {
        let index_path = self
            .repo_root
            .join(".rup")
            .join("backups")
            .join("index.jsonl");
        let lock_path = self
            .repo_root
            .join(".rup")
            .join("locks")
            .join("backups.lock");

        // Keep the guard in scope until after writes
        let _guard = acquire_lock(&lock_path)?;

        let index_entry = SessionIndexEntry {
            id: self.manifest.id.clone(),
            timestamp: self.manifest.timestamp.clone(),
            success: self.manifest.success,
            files: self.manifest.files.len(),
            engine: self.manifest.engine.clone(),
        };

        let entry_line = serde_json::to_string(&index_entry)
            .with_context(|| "Failed to serialize index entry")?;

        // Append to index file
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&index_path)
            .with_context(|| format!("Failed to open index file: {:?}", index_path))?;

        writeln!(file, "{}", entry_line).with_context(|| "Failed to write to index file")?;

        file.sync_all()
            .with_context(|| "Failed to sync index file")?;

        // guard drops here and removes the lock file
        Ok(())
    }
}

// In Drop, best-effort finalize with failure
impl Drop for BackupManager {
    fn drop(&mut self) {
        if !self.finalized {
            // Avoid panicking in Drop; ignore errors deliberately.
            let _ = self.finalize(false);
        }
    }
}

// Generate a session ID with RFC3339-ish timestamp and random suffix.
fn generate_session_id() -> String {
    // Use a filesystem-safe timestamp (no ':')
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();

    // 10 chars of base62 is enough uniqueness for this scope
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let suffix: String = (0..10)
        .map(|_| {
            let i = rng.gen_range(0..ALPHABET.len());
            ALPHABET[i] as char
        })
        .collect();

    format!("{}_{}", ts, suffix)
}

// Capture current Git state (optional)
fn capture_git_snapshot(repo_root: &Path) -> Result<GitSnapshot> {
    // Simple git command execution (could be enhanced with git2 crate)
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()
        .and_then(|output| {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                Err(std::io::Error::other("git command failed"))
            }
        })
        .unwrap_or_else(|_| "unknown".to_string());

    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo_root)
        .output()
        .map(|output| {
            if output.status.success() {
                let branch_name = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if branch_name == "HEAD" {
                    None
                } else {
                    Some(branch_name)
                }
            } else {
                None
            }
        })
        .unwrap_or(None);

    // Check for dirty working directory
    let dirty = std::process::Command::new("git")
        .args(["diff", "--quiet"])
        .current_dir(repo_root)
        .status()
        .map(|status| !status.success())
        .unwrap_or(false);

    // Check for staged changes
    let staged = std::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(repo_root)
        .status()
        .map(|status| !status.success())
        .unwrap_or(false);

    Ok(GitSnapshot {
        commit,
        branch,
        dirty,
        staged,
    })
}

// Calculate file checksum (blake3 for Phase 1)
fn calculate_file_checksum(path: &Path) -> Result<String> {
    // Read all bytes; for very large files, switch to streaming
    // Hasher::update() on a FileBufReader.
    let bytes =
        fs::read(path).with_context(|| format!("Failed to read file for checksum: {:?}", path))?;

    let hash = blake3::hash(&bytes);
    Ok(format!("blake3:{}", hash.to_hex()))
}

// Acquire a lock, returning a guard that deletes the lock on Drop.
struct LockGuard {
    path: PathBuf,
    _file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Best-effort removal; ignore errors
        let _ = fs::remove_file(&self.path);
    }
}

// Simple file-based lock acquisition with auto-release
fn acquire_lock(lock_path: &Path) -> Result<LockGuard> {
    let file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            anyhow::bail!("Backup operation already in progress (lock file exists)");
        }
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to acquire lock: {:?}", lock_path));
        }
    };

    // Write PID for diagnostics
    {
        let mut writer = OpenOptions::new()
            .write(true)
            .open(lock_path)
            .with_context(|| "Failed to reopen lock file for writing")?;
        writeln!(writer, "{}", std::process::id())
            .with_context(|| "Failed to write PID to lock file")?;
        writer
            .sync_all()
            .with_context(|| "Failed to sync lock file")?;
    }

    Ok(LockGuard {
        path: lock_path.to_path_buf(),
        _file: file,
    })
}

// Read all sessions from index
pub fn list_sessions(repo_root: &Path) -> Result<Vec<SessionIndexEntry>> {
    let index_path = repo_root.join(".rup").join("backups").join("index.jsonl");

    if !index_path.exists() {
        return Ok(Vec::new());
    }

    let file = File::open(&index_path)
        .with_context(|| format!("Failed to open index file: {:?}", index_path))?;
    let reader = BufReader::new(file);

    let mut sessions = Vec::new();
    for (line_num, line) in reader.lines().enumerate() {
        let line =
            line.with_context(|| format!("Failed to read line {} from index", line_num + 1))?;
        if line.trim().is_empty() {
            continue;
        }

        let entry: SessionIndexEntry = serde_json::from_str(&line).with_context(|| {
            format!(
                "Failed to parse index entry at line {}: {}",
                line_num + 1,
                line
            )
        })?;
        sessions.push(entry);
    }

    Ok(sessions)
}

// Read a specific session manifest
pub fn read_session_manifest(repo_root: &Path, session_id: &str) -> Result<SessionManifest> {
    let manifest_path = repo_root
        .join(".rup")
        .join("backups")
        .join(session_id)
        .join("manifest.json");

    // Check for DONE marker first
    let done_path = repo_root
        .join(".rup")
        .join("backups")
        .join(session_id)
        .join("DONE");

    if !done_path.exists() {
        anyhow::bail!("Session {} is incomplete (no DONE marker)", session_id);
    }

    let content = fs::read_to_string(&manifest_path)
        .with_context(|| format!("Failed to read manifest: {:?}", manifest_path))?;

    let manifest: SessionManifest = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse manifest: {:?}", manifest_path))?;

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_backup_manager_basic_flow() {
        let temp_dir = TempDir::new().unwrap();
        let repo_root = temp_dir.path();

        // Create a test file
        let test_file = repo_root.join("test.txt");
        fs::write(&test_file, "test content").unwrap();

        // Create backup manager
        let mut manager = BackupManager::begin(repo_root, "test").unwrap();

        // Backup the file
        manager.backup_file(Path::new("test.txt")).unwrap();

        // Finalize
        manager.finalize(true).unwrap();

        // Verify session was created
        let sessions = list_sessions(repo_root).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].success);
        assert_eq!(sessions[0].files, 1);

        // Verify manifest
        let manifest = read_session_manifest(repo_root, &sessions[0].id).unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].original_path, Path::new("test.txt"));
    }

    #[test]
    fn test_mirrored_directory_structure() {
        let temp_dir = TempDir::new().unwrap();
        let repo_root = temp_dir.path();

        // Create nested test file
        let nested_dir = repo_root.join("src").join("core");
        fs::create_dir_all(&nested_dir).unwrap();
        let test_file = nested_dir.join("test.rs");
        fs::write(&test_file, "fn main() {}").unwrap();

        let mut manager = BackupManager::begin(repo_root, "test").unwrap();
        manager.backup_file(Path::new("src/core/test.rs")).unwrap();
        manager.finalize(true).unwrap();

        // Verify mirrored structure exists in backup
        let sessions = list_sessions(repo_root).unwrap();
        let session_dir = repo_root.join(".rup").join("backups").join(&sessions[0].id);
        let backup_file = session_dir.join("src").join("core").join("test.rs");

        assert!(backup_file.exists());
        assert_eq!(fs::read_to_string(&backup_file).unwrap(), "fn main() {}");
    }
}
