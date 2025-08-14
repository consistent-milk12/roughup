//! Integration tests for backup list/show (Phase B2 read-only)

use anyhow::Result;
use std::fs;
use tempfile::TempDir;

use roughup::core::backup::{BackupManager, list_sessions};
use roughup::core::backup_ops::{ListRequest, ShowRequest, list_sessions_filtered, show_session};

fn setup_repo() -> Result<TempDir> {
    let temp = TempDir::new()?;
    let root = temp.path();

    // minimal git init to simulate typical usage
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(root)
        .output()?;
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(root)
        .output()?;
    std::process::Command::new("git")
        .args(["config", "user.email", "t@example.com"])
        .current_dir(root)
        .output()?;

    Ok(temp)
}

#[test]
fn test_list_engine_filter_case_insensitive_and_aliases() -> Result<()> {
    let temp = setup_repo()?;
    let root = temp.path();

    // Create two sessions: one incomplete latest, one completed earlier
    // First, create a completed session with engine "Auto"
    let mut m1 = BackupManager::begin(root, "Auto")?;
    // Create file and back it up
    let f1 = root.join("a.txt");
    fs::write(&f1, "alpha")?;
    m1.backup_file(std::path::Path::new("a.txt"))?;
    m1.finalize(true)?;

    // Then, create a second session but do not finalize to simulate incomplete
    let mut m2 = BackupManager::begin(root, "internal")?;
    let f2 = root.join("b.txt");
    fs::write(&f2, "bravo")?;
    m2.backup_file(std::path::Path::new("b.txt"))?;
    // Intentionally drop without finalize; Drop will finalize(false)
    drop(m2);

    // List sessions by engine filter (case-insensitive)
    let req = ListRequest {
        successful: false,
        engine: Some("INTERNAL".to_string()),
        since: None,
        limit: 50,
        sort_desc: true,
    };
    let listed = list_sessions_filtered(root, req)?;
    // Should include the Internal session even with uppercase filter
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].engine.to_ascii_lowercase(), "internal");

    // Verify alias resolution for last-successful selects the successful Auto session
    let resp = show_session(
        root,
        ShowRequest {
            id: "last-successful".to_string(),
            verbose: false,
        },
    )?;
    assert_eq!(resp.manifest.engine.to_ascii_lowercase(), "auto");

    Ok(())
}

#[test]
fn test_show_verbose_payload_size_excludes_metadata() -> Result<()> {
    let temp = setup_repo()?;
    let root = temp.path();

    // Create a file with known size
    let content = "0123456789"; // 10 bytes
    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("src").join("lib.rs"), content)?;

    // Backup and finalize
    let mut m = BackupManager::begin(root, "internal")?;
    m.backup_file(std::path::Path::new("src/lib.rs"))?;
    m.finalize(true)?;

    // Show with verbose to compute total_size
    let sessions = list_sessions(root)?;
    assert_eq!(sessions.len(), 1);
    let id = &sessions[0].id;
    let resp = show_session(
        root,
        ShowRequest {
            id: id.clone(),
            verbose: true,
        },
    )?;

    // total_size should equal the backed up file size (10), excluding manifest.json and DONE
    assert_eq!(resp.total_size, Some(10));

    Ok(())
}
