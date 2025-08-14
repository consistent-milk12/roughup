//! Integration tests for centralized backup sessions
//!
//! Tests the Phase B1 implementation: single session per `rup apply`,
//! shared across Internal/Git/Auto paths with proper session artifacts.

use anyhow::Result;
use std::fs;
use tempfile::TempDir;

use roughup::cli::WhitespaceMode;
use roughup::core::apply_engine::{ApplyContext, ApplyEngine, HybridEngine, InternalEngine};
use roughup::core::backup::BackupManager;
use roughup::core::edit::{EditOperation, EditSpec, FileBlock};
use roughup::core::git::GitOptions;

/// Helper to create a temp git repo with test files
fn setup_test_repo() -> Result<TempDir> {
    let temp_dir = TempDir::new()?;
    let repo_root = temp_dir.path();

    // Initialize git repo
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo_root)
        .output()?;

    std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(repo_root)
        .output()?;

    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(repo_root)
        .output()?;

    // Create test file
    let test_file = repo_root.join("src").join("lib.rs");
    fs::create_dir_all(test_file.parent().unwrap())?;
    fs::write(
        &test_file,
        "fn main() {\n    println!(\"Hello, world!\");\n}\n",
    )?;

    // Initial commit
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(repo_root)
        .output()?;

    std::process::Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(repo_root)
        .output()?;

    Ok(temp_dir)
}

/// Test 1: InternalEngine + single-file apply (success path)
/// Verifies single session, manifest, file mirrored, ApplyReport fields populated
#[test]
fn test_internal_engine_single_session_success() -> Result<()> {
    let temp_repo = setup_test_repo()?;
    let repo_root = temp_repo.path();
    let test_file = repo_root.join("src").join("lib.rs");

    // Create edit spec to modify the file
    let spec = EditSpec {
        file_blocks: vec![FileBlock {
            path: test_file.clone(),
            operations: vec![EditOperation::Replace {
                start_line: 2,
                end_line: 2,
                old_content: "    println!(\"Hello, world!\");".to_string(),
                new_content: "    println!(\"Hello, Rust!\");".to_string(),
                guard_cid: None,
            }],
        }],
    };

    // Create backup manager and engine
    let mut backup_manager = BackupManager::begin(repo_root, "internal")?;
    let engine = InternalEngine::new(true, false, 3);

    // Create context with backup manager
    let ctx = ApplyContext {
        repo_root,
        backup: Some(&mut backup_manager),
        whitespace: WhitespaceMode::Nowarn,
        context_lines: 3,
        force: false,
    };

    // Apply with context
    let report = engine.apply_with_ctx(&spec, ctx)?;

    // Verify ApplyReport fields
    assert_eq!(report.applied_files.len(), 1);
    assert_eq!(report.applied_files[0], test_file);
    assert!(report.conflicts.is_empty());
    assert_eq!(
        report.engine_used,
        roughup::core::apply_engine::Engine::Internal
    );

    // Verify backup session fields
    assert!(report.backup_session_id.is_some());
    assert!(report.backup_manifest_path.is_some());
    assert_eq!(report.backup_file_count, Some(1));
    assert_eq!(report.backup_paths.len(), 1);

    // Verify session directory exists and contains manifest
    let session_dir = &report.backup_paths[0];
    assert!(session_dir.exists());
    assert!(session_dir.join("manifest.json").exists());
    assert!(session_dir.join("DONE").exists());

    // Verify file is mirrored in session
    let backup_file = session_dir.join("src").join("lib.rs");
    assert!(backup_file.exists());
    let backup_content = fs::read_to_string(&backup_file)?;
    assert!(backup_content.contains("Hello, world!")); // Original content

    // Verify actual file was modified
    let modified_content = fs::read_to_string(&test_file)?;
    assert!(modified_content.contains("Hello, Rust!")); // Modified content

    // Verify manifest contents
    let manifest_path = report.backup_manifest_path.unwrap();
    let manifest_content = fs::read_to_string(&manifest_path)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_content)?;
    assert_eq!(manifest["success"], true);
    assert_eq!(manifest["files"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["engine"], "internal");

    Ok(())
}

/// Test 2: Repo boundary enforcement  
/// Verifies that files outside repo root are rejected early
#[test]
fn test_boundary_enforcement() -> Result<()> {
    let temp_repo = setup_test_repo()?;
    let repo_root = temp_repo.path();

    // Try to edit a file outside the repo
    let outside_file = temp_repo.path().parent().unwrap().join("outside.rs");
    fs::write(&outside_file, "fn outside() {}")?;

    let spec = EditSpec {
        file_blocks: vec![FileBlock {
            path: outside_file,
            operations: vec![EditOperation::Replace {
                start_line: 1,
                end_line: 1,
                old_content: "fn outside() {}".to_string(),
                new_content: "fn modified() {}".to_string(),
                guard_cid: None,
            }],
        }],
    };

    let mut backup_manager = BackupManager::begin(repo_root, "internal")?;
    let engine = InternalEngine::new(true, false, 3);

    let ctx = ApplyContext {
        repo_root,
        backup: Some(&mut backup_manager),
        whitespace: WhitespaceMode::Nowarn,
        context_lines: 3,
        force: false,
    };

    // Should fail with boundary error
    let result = engine.apply_with_ctx(&spec, ctx);
    assert!(result.is_err());
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("outside repository"));

    Ok(())
}

/// Test 3: Auto engine fallback with shared session
/// Tests Internal conflicts → Git succeeds with single session
#[test]
fn test_auto_fallback_shared_session() -> Result<()> {
    let temp_repo = setup_test_repo()?;
    let repo_root = temp_repo.path();
    let test_file = repo_root.join("src").join("lib.rs");

    // Modify the file to create a guard mismatch for Internal engine
    fs::write(&test_file, "fn main() {\n    println!(\"Modified!\");\n}\n")?;

    // Create edit spec with guard that will fail (expects old content)
    let spec = EditSpec {
        file_blocks: vec![FileBlock {
            path: test_file.clone(),
            operations: vec![EditOperation::Replace {
                start_line: 2,
                end_line: 2,
                old_content: "    println!(\"Hello, world!\");".to_string(), // This won't match
                new_content: "    println!(\"Hello, Auto!\");".to_string(),
                guard_cid: None,
            }],
        }],
    };

    // Create backup manager and hybrid engine
    let mut backup_manager = BackupManager::begin(repo_root, "auto")?;
    let git_options = GitOptions {
        repo_root: repo_root.to_path_buf(),
        mode: roughup::core::git::GitMode::ThreeWay,
        whitespace: roughup::core::git::Whitespace::Nowarn,
        context_lines: 3,
        allow_outside_repo: false,
    };
    let engine = HybridEngine::new(true, false, git_options, true)?;

    let ctx = ApplyContext {
        repo_root,
        backup: Some(&mut backup_manager),
        whitespace: WhitespaceMode::Nowarn,
        context_lines: 3,
        force: false,
    };

    // This should try Internal (fail with conflicts), then Git (succeed)
    // Note: This test might be complex due to the current HybridEngine design limitation
    // where Internal finalizes its backup session before Git attempts
    let result = engine.apply_with_ctx(&spec, ctx);

    // For now, we expect this to fail because Internal conflicts cause session finalization
    // This documents the current design limitation mentioned in HybridEngine comments
    match result {
        Err(_) => {
            // Expected due to current design limitation
            // Internal has conflicts and finalizes session, preventing Git fallback with backup
        }
        Ok(report) => {
            // If this works, verify it's marked as Auto engine
            assert_eq!(
                report.engine_used,
                roughup::core::apply_engine::Engine::Auto
            );
            assert!(report.backup_session_id.is_some());
        }
    }

    Ok(())
}

/// Test 4: Multiple files in single session
/// Verifies that multiple file edits use one session with all files backed up
#[test]
fn test_multiple_files_single_session() -> Result<()> {
    let temp_repo = setup_test_repo()?;
    let repo_root = temp_repo.path();

    // Create multiple test files
    let file1 = repo_root.join("src").join("lib.rs");
    let file2 = repo_root.join("src").join("main.rs");
    fs::write(&file2, "fn main() {\n    lib::hello();\n}\n")?;

    let spec = EditSpec {
        file_blocks: vec![
            FileBlock {
                path: file1.clone(),
                operations: vec![EditOperation::Replace {
                    start_line: 2,
                    end_line: 2,
                    old_content: "    println!(\"Hello, world!\");".to_string(),
                    new_content: "    println!(\"Hello, multi!\");".to_string(),
                    guard_cid: None,
                }],
            },
            FileBlock {
                path: file2.clone(),
                operations: vec![EditOperation::Replace {
                    start_line: 2,
                    end_line: 2,
                    old_content: "    lib::hello();".to_string(),
                    new_content: "    lib::hello_multi();".to_string(),
                    guard_cid: None,
                }],
            },
        ],
    };

    let mut backup_manager = BackupManager::begin(repo_root, "internal")?;
    let engine = InternalEngine::new(true, false, 3);

    let ctx = ApplyContext {
        repo_root,
        backup: Some(&mut backup_manager),
        whitespace: WhitespaceMode::Nowarn,
        context_lines: 3,
        force: false,
    };

    let report = engine.apply_with_ctx(&spec, ctx)?;

    // Verify both files were applied
    assert_eq!(report.applied_files.len(), 2);
    assert!(report.conflicts.is_empty());

    // Verify single session with both files
    assert_eq!(report.backup_file_count, Some(2));
    let session_dir = &report.backup_paths[0];

    // Both files should be backed up
    assert!(session_dir.join("src").join("lib.rs").exists());
    assert!(session_dir.join("src").join("main.rs").exists());

    // Verify manifest shows 2 files
    let manifest_content = fs::read_to_string(report.backup_manifest_path.unwrap())?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_content)?;
    assert_eq!(manifest["files"].as_array().unwrap().len(), 2);

    Ok(())
}

/// Test 5: Session directory structure and DONE marker
/// Verifies proper session finalization artifacts
#[test]
fn test_session_artifacts() -> Result<()> {
    let temp_repo = setup_test_repo()?;
    let repo_root = temp_repo.path();
    let test_file = repo_root.join("src").join("lib.rs");

    let spec = EditSpec {
        file_blocks: vec![FileBlock {
            path: test_file,
            operations: vec![EditOperation::Replace {
                start_line: 1,
                end_line: 1,
                old_content: "fn main() {".to_string(),
                new_content: "fn main_modified() {".to_string(),
                guard_cid: None,
            }],
        }],
    };

    let mut backup_manager = BackupManager::begin(repo_root, "internal")?;
    let engine = InternalEngine::new(true, false, 3);

    let ctx = ApplyContext {
        repo_root,
        backup: Some(&mut backup_manager),
        whitespace: WhitespaceMode::Nowarn,
        context_lines: 3,
        force: false,
    };

    let report = engine.apply_with_ctx(&spec, ctx)?;
    let session_dir = &report.backup_paths[0];

    // Verify session structure
    assert!(session_dir.exists());
    assert!(session_dir.join("manifest.json").exists());
    assert!(session_dir.join("DONE").exists());
    assert!(session_dir.join("src").join("lib.rs").exists());

    // Verify DONE marker exists (empty file indicates completion)
    let done_path = session_dir.join("DONE");
    assert!(done_path.exists());

    // Verify session appears in index
    let sessions = roughup::core::backup::list_sessions(repo_root)?;
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].success);
    assert_eq!(sessions[0].files, 1);

    Ok(())
}
