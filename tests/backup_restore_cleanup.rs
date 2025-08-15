//! Integration tests for backup restore and cleanup flows.

use std::{fs, path::Path, time::Duration};

use roughup::core::{
    backup::BackupManager,
    backup_ops::{CleanupRequest, RestoreRequest, cleanup_sessions, restore_session},
};
use tempfile::tempdir;

/// Create a simple text file under repo_root.
fn write_file(
    repo: &Path,
    rel: &str,
    body: &str,
)
{
    let p = repo.join(rel);
    if let Some(parent) = p.parent()
    {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body.as_bytes()).unwrap();
}

/// Read a file as UTF-8 string.
fn read_file(
    repo: &Path,
    rel: &str,
) -> String
{
    fs::read_to_string(repo.join(rel)).unwrap()
}

/// Create a completed backup session by backing up `rel` file.
fn make_session(
    repo: &Path,
    rel: &str,
    engine_tag: &str,
) -> String
{
    let mut mgr = BackupManager::begin(repo, engine_tag).unwrap();
    mgr.backup_file(Path::new(rel))
        .unwrap();
    mgr.finalize(true)
        .unwrap();
    mgr.session_id()
        .to_string()
}

/// Patch a session manifest timestamp to a specific RFC3339 instant.
/// This helps exercise age-based cleanup deterministically.
fn overwrite_manifest_timestamp(
    repo: &Path,
    sess: &str,
    rfc3339: &str,
)
{
    let manifest = repo
        .join(".rup/backups")
        .join(sess)
        .join("manifest.json");
    let s = fs::read_to_string(&manifest).unwrap();
    // Replace "timestamp":"...".
    let patched = regex::Regex::new(r#""timestamp"\s*:\s*"[^\"]+""#)
        .unwrap()
        .replace(&s, format!(r#""timestamp":"{}""#, rfc3339));
    fs::write(&manifest, patched.as_bytes()).unwrap();
}

#[test]
fn test_restore_reports_conflict_on_dry_run()
{
    let tmp = tempdir().unwrap();
    let repo = tmp.path();

    // Prepare a file and back it up.
    write_file(repo, "src/lib.rs", "one\nold\nversion\n");
    let session_id = make_session(repo, "src/lib.rs", "apply");

    // Diverge current file.
    write_file(repo, "src/lib.rs", "one\nnew\nversion\n");

    // Dry-run restore without --force must report conflicts and no writes.
    let req = RestoreRequest {
        session_id: session_id.clone(),
        path: None,
        dry_run: true,
        force: false,
        show_diff: true,
        verify_checksum: true,
        backup_current: false,
    };
    let out = restore_session(repo, req).unwrap();

    assert_eq!(out.session_id, session_id);
    assert!(
        out.restored
            .is_empty()
    );
    assert_eq!(
        out.conflicts
            .len(),
        1
    );
    assert!(
        out.diffs
            .as_ref()
            .unwrap()[0]
            .unified
            .contains("@@")
    );
    // Content remains the divergent one.
    assert_eq!(read_file(repo, "src/lib.rs"), "one\nnew\nversion\n");
}

#[test]
fn test_restore_force_overwrites_and_backs_up_current()
{
    let tmp = tempdir().unwrap();
    let repo = tmp.path();

    // Original -> backup -> mutate current.
    write_file(repo, "src/lib.rs", "A\nB\nC\n");
    let session_id = make_session(repo, "src/lib.rs", "apply");
    write_file(repo, "src/lib.rs", "A\nX\nC\n"); // current changed

    let req = RestoreRequest {
        session_id: session_id.clone(),
        path: None,
        dry_run: false,
        force: true,
        show_diff: false,
        verify_checksum: true,
        backup_current: true,
    };
    let out = restore_session(repo, req).unwrap();

    assert_eq!(out.session_id, session_id);
    assert!(
        out.conflicts
            .is_empty()
    );
    assert_eq!(out.restored, vec![Path::new("src/lib.rs").to_path_buf()]);
    assert!(out.backed_up_current);
    assert!(
        out.backup_session_id
            .is_some()
    );

    // Current content matches the backed-up content ("A\nB\nC\n").
    assert_eq!(read_file(repo, "src/lib.rs"), "A\nB\nC\n");
}

#[test]
fn test_restore_single_path_filter()
{
    let tmp = tempdir().unwrap();
    let repo = tmp.path();

    // Back up two files in one session.
    write_file(repo, "a.txt", "alpha\n");
    write_file(repo, "b.txt", "beta\n");
    let mut mgr = BackupManager::begin(repo, "apply").unwrap();
    mgr.backup_file(Path::new("a.txt"))
        .unwrap();
    mgr.backup_file(Path::new("b.txt"))
        .unwrap();
    mgr.finalize(true)
        .unwrap();
    let session_id = mgr
        .session_id()
        .to_string();

    // Change only a.txt in working tree.
    write_file(repo, "a.txt", "ALPHA_MUT\n");

    // Restore only b.txt should be a no-op write but allowed.
    let req_b = RestoreRequest {
        session_id: session_id.clone(),
        path: Some(Path::new("b.txt").to_path_buf()),
        dry_run: false,
        force: false,
        show_diff: false,
        verify_checksum: true,
        backup_current: false,
    };
    let out_b = restore_session(repo, req_b).unwrap();
    assert!(
        out_b
            .conflicts
            .is_empty()
    );
    assert_eq!(out_b.restored, vec![Path::new("b.txt").to_path_buf()]);
    assert_eq!(read_file(repo, "b.txt"), "beta\n");

    // Restoring a.txt without force should report conflict.
    let req_a = RestoreRequest {
        session_id: session_id.clone(),
        path: Some(Path::new("a.txt").to_path_buf()),
        dry_run: true,
        force: false,
        show_diff: true,
        verify_checksum: true,
        backup_current: false,
    };
    let out_a = restore_session(repo, req_a).unwrap();
    assert_eq!(out_a.conflicts, vec![Path::new("a.txt").to_path_buf()]);
    assert!(
        out_a
            .diffs
            .unwrap()[0]
            .unified
            .contains("@@")
    );
}

#[test]
fn test_cleanup_by_age_and_keep_latest()
{
    let tmp = tempdir().unwrap();
    let repo = tmp.path();

    // Make three completed sessions.
    write_file(repo, "x.txt", "x\n");
    let s1 = make_session(repo, "x.txt", "apply");
    // Sleep to ensure distinct timestamps even if manifest parse fails.
    std::thread::sleep(Duration::from_millis(10));

    write_file(repo, "y.txt", "y\n");
    let s2 = make_session(repo, "y.txt", "apply");
    std::thread::sleep(Duration::from_millis(10));

    write_file(repo, "z.txt", "z\n");
    let s3 = make_session(repo, "z.txt", "apply");

    // Force timestamps: s1 very old, s2 old, s3 now.
    overwrite_manifest_timestamp(repo, &s1, "2020-01-01T00:00:00Z");
    overwrite_manifest_timestamp(repo, &s2, "2023-01-01T00:00:00Z");
    // s3 stays current.

    // Dry-run: remove older than 2024-01-01 and keep latest 2.
    // Dry-run case
    let req = CleanupRequest {
        older_than: Some("2024-01-01T00:00:00Z".to_string()),
        keep_latest: Some(2),
        include_incomplete: false,
        dry_run: true,
    };

    let out = cleanup_sessions(repo, req).unwrap();

    // s1 matches age filter; depending on keep_latest, s1 is targeted.
    assert!(
        out.sessions_removed
            .contains(&s1)
    );
    // Ensure s3 (latest) is not slated for removal.
    assert!(
        !out.sessions_removed
            .contains(&s3)
    );

    // Now actually delete only by keep_latest=1 to force removal of s1,s2.
    // Apply case
    let req2 = CleanupRequest {
        older_than: None,
        keep_latest: Some(1),
        include_incomplete: true,
        dry_run: false,
    };

    let out2 = cleanup_sessions(repo, req2).unwrap();
    assert!(
        out2.sessions_removed
            .contains(&s1)
    );
    assert!(
        out2.sessions_removed
            .contains(&s2)
    );
    assert!(
        !out2
            .sessions_removed
            .contains(&s3)
    );
    // The directory for s3 must still exist.
    assert!(
        repo.join(".rup/backups")
            .join(&s3)
            .exists()
    );
}
