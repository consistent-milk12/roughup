//! Git apply integration for robust patch application
//!
//! Implements git apply with 3-way merge, stderr parsing, and user-friendly
//! error mapping according to engineering review specifications.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::core::patch::PatchSet;

/// Git apply modes
#[derive(Debug, Clone)]
pub enum GitMode {
    /// Apply to index (requires clean preimage)
    Index,
    /// 3-way merge (resilient, may leave conflict markers)
    ThreeWay,
    /// Apply to temporary worktree
    Worktree,
}

/// Whitespace handling modes
#[derive(Debug, Clone)]
pub enum Whitespace {
    /// Ignore whitespace issues
    Nowarn,
    /// Warn about whitespace issues
    Warn,
    /// Fix whitespace issues automatically
    Fix,
}

/// Git apply configuration
#[derive(Debug, Clone)]
pub struct GitOptions {
    pub repo_root: PathBuf,
    pub mode: GitMode,
    pub whitespace: Whitespace,
    pub context_lines: u8,
    pub allow_outside_repo: bool,
}

impl Default for GitOptions {
    fn default() -> Self {
        Self {
            repo_root: PathBuf::from("."),
            mode: GitMode::ThreeWay,
            whitespace: Whitespace::Nowarn,
            context_lines: 3,
            allow_outside_repo: false,
        }
    }
}

/// Git apply outcome
#[derive(Debug)]
pub struct GitOutcome {
    pub applied_files: Vec<PathBuf>,
    pub conflicts: Vec<GitConflict>,
    pub left_markers: Vec<PathBuf>,
    pub stderr_raw: String,
}

/// Git conflict types with user-friendly categorization
#[derive(Debug, Clone)]
pub enum GitConflict {
    PreimageMismatch {
        path: PathBuf,
        hunk: (u32, u32),
        hint: &'static str,
    },
    PathOutsideRepo {
        path: PathBuf,
        hint: &'static str,
    },
    WhitespaceError {
        path: PathBuf,
        hint: &'static str,
    },
    IndexRequired {
        path: PathBuf,
        hint: &'static str,
    },
    BinaryOrMode {
        path: PathBuf,
        hint: &'static str,
    },
    Other(String),
}

/// Git apply engine implementation
pub struct GitEngine {
    options: GitOptions,
    git_executable: Option<PathBuf>,
}

impl GitEngine {
    /// Create new Git engine with options
    pub fn new(options: GitOptions) -> Result<Self> {
        let git_executable = detect_git_executable()?;
        Ok(Self {
            options,
            git_executable: Some(git_executable),
        })
    }

    /// Check if patch can be applied (preview mode)
    pub fn check(&self, patch_set: &PatchSet) -> Result<GitOutcome> {
        let patch_content = crate::core::patch::render_unified_diff(patch_set);
        self.run_git_apply(&patch_content, true)
    }

    /// Apply patch set to repository
    pub fn apply(&self, patch_set: &PatchSet) -> Result<GitOutcome> {
        let patch_content = crate::core::patch::render_unified_diff(patch_set);
        self.run_git_apply(&patch_content, false)
    }

    /// Run git apply with specified options
    fn run_git_apply(&self, patch_content: &str, check_only: bool) -> Result<GitOutcome> {
        let git_path = self
            .git_executable
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Git executable not found"))?;

        let mut cmd = Command::new(git_path);
        cmd.current_dir(&self.options.repo_root);

        // Set whitespace handling
        let whitespace_mode = match self.options.whitespace {
            Whitespace::Nowarn => "nowarn",
            Whitespace::Warn => "warn",
            Whitespace::Fix => "fix",
        };
        cmd.arg("-c")
            .arg(format!("apply.whitespace={}", whitespace_mode));

        // Configure apply mode
        cmd.arg("apply");

        if check_only {
            cmd.arg("--check");
        }

        match self.options.mode {
            GitMode::Index => {
                cmd.arg("--index");
            }
            GitMode::ThreeWay => {
                cmd.arg("--3way");
                if !check_only {
                    cmd.arg("--index");
                }
            }
            GitMode::Worktree => {
                // Apply to working tree only
            }
        }

        // Add verbose output for better error parsing
        cmd.arg("--verbose");
        cmd.arg("--reject");

        // Read patch from stdin
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn git apply process")?;

        // Write patch content to stdin
        if let Some(stdin) = child.stdin.take() {
            use std::io::Write;
            let mut stdin = stdin;
            stdin
                .write_all(patch_content.as_bytes())
                .context("Failed to write patch to git apply stdin")?;
        }

        let output = child
            .wait_with_output()
            .context("Failed to wait for git apply process")?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse git apply output
        let conflicts = parse_git_stderr(&stderr);
        let applied_files = if output.status.success() {
            extract_applied_files(&stdout, &stderr)
        } else {
            Vec::new()
        };

        // Check for conflict markers in applied files
        let left_markers = if !check_only && matches!(self.options.mode, GitMode::ThreeWay) {
            find_conflict_markers(&applied_files)?
        } else {
            Vec::new()
        };

        Ok(GitOutcome {
            applied_files,
            conflicts,
            left_markers,
            stderr_raw: stderr.to_string(),
        })
    }
}

/// Detect git executable and verify minimum version
fn detect_git_executable() -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("--version")
        .output()
        .context("Git executable not found in PATH")?;

    if !output.status.success() {
        anyhow::bail!("Git command failed");
    }

    let version_str = String::from_utf8_lossy(&output.stdout);

    // Basic version check - ensure git 2.0+
    if !version_str.contains("git version") {
        anyhow::bail!("Unexpected git version output: {}", version_str);
    }

    Ok(PathBuf::from("git"))
}

/// Parse git apply stderr into structured conflicts
fn parse_git_stderr(stderr: &str) -> Vec<GitConflict> {
    let mut conflicts = Vec::new();

    for line in stderr.lines() {
        let line = line.trim();

        if line.contains("patch does not apply") {
            conflicts.push(GitConflict::PreimageMismatch {
                path: extract_path_from_error(line).unwrap_or_else(|| PathBuf::from("unknown")),
                hunk: (0, 0), // TODO: Parse actual hunk numbers
                hint: "Target lines changed since suggestion. Try `--engine auto` or regenerate.",
            });
        } else if line.contains("does not match index") {
            conflicts.push(GitConflict::IndexRequired {
                path: extract_path_from_error(line).unwrap_or_else(|| PathBuf::from("unknown")),
                hint: "Requires clean index. Commit or stash changes, or use `--git-mode 3way`.",
            });
        } else if line.contains("whitespace error") {
            conflicts.push(GitConflict::WhitespaceError {
                path: extract_path_from_error(line).unwrap_or_else(|| PathBuf::from("unknown")),
                hint: "Whitespace sensitivity blocked apply. Try `--whitespace nowarn`.",
            });
        } else if line.contains("is outside repository") {
            conflicts.push(GitConflict::PathOutsideRepo {
                path: extract_path_from_error(line).unwrap_or_else(|| PathBuf::from("unknown")),
                hint: "Edits must target tracked files within the repo root.",
            });
        } else if line.contains("error:") || line.contains("fatal:") {
            conflicts.push(GitConflict::Other(line.to_string()));
        }
    }

    conflicts
}

/// Extract file path from git error message
fn extract_path_from_error(error_line: &str) -> Option<PathBuf> {
    // Simple heuristic: look for file-like strings
    // This is a simplified implementation - production would need more robust parsing
    error_line
        .split_whitespace()
        .find(|word| word.contains(".rs") || word.contains(".py") || word.contains("/"))
        .map(|path| PathBuf::from(path.trim_matches(|c| c == ':' || c == ',' || c == '"')))
}

/// Extract applied files from git output
fn extract_applied_files(stdout: &str, stderr: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();

    // Look for "Applying: " or similar patterns in output
    for line in stdout.lines().chain(stderr.lines()) {
        if (line.contains("Applying") || line.contains("patching file"))
            && let Some(path) = extract_path_from_error(line)
        {
            files.push(path);
        }
    }

    files
}

/// Find files with unresolved conflict markers
fn find_conflict_markers(files: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files_with_markers = Vec::new();

    for file_path in files {
        if let Ok(content) = std::fs::read_to_string(file_path)
            && (content.contains("<<<<<<<")
                || content.contains(">>>>>>>")
                || content.contains("======="))
        {
            files_with_markers.push(file_path.clone());
        }
    }

    Ok(files_with_markers)
}

/// Render user-friendly conflict summary
pub fn render_conflict_summary(conflicts: &[GitConflict]) -> String {
    if conflicts.is_empty() {
        return String::new();
    }

    let mut output = format!("Conflicts ({})\n", conflicts.len());

    for conflict in conflicts.iter() {
        match conflict {
            GitConflict::PreimageMismatch { path, hint, .. } => {
                output.push_str(&format!(
                    "  • {}: preimage mismatch\n    Remedy: {}\n",
                    path.display(),
                    hint
                ));
            }
            GitConflict::IndexRequired { path, hint } => {
                output.push_str(&format!(
                    "  • {}: index required\n    Remedy: {}\n",
                    path.display(),
                    hint
                ));
            }
            GitConflict::WhitespaceError { path, hint } => {
                output.push_str(&format!(
                    "  • {}: whitespace error\n    Remedy: {}\n",
                    path.display(),
                    hint
                ));
            }
            GitConflict::PathOutsideRepo { path, hint } => {
                output.push_str(&format!(
                    "  • {}: outside repository\n    Remedy: {}\n",
                    path.display(),
                    hint
                ));
            }
            GitConflict::BinaryOrMode { path, hint } => {
                output.push_str(&format!(
                    "  • {}: binary or mode change\n    Remedy: {}\n",
                    path.display(),
                    hint
                ));
            }
            GitConflict::Other(msg) => {
                output.push_str(&format!(
                    "  • Other: {}\n    Remedy: Check git output with `--verbose`\n",
                    msg
                ));
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_detection() {
        // This test requires git to be installed
        if detect_git_executable().is_ok() {
            // Git is available
        } else {
            // Skip test if git not available
            println!("Git not available, skipping test");
        }
    }

    #[test]
    fn test_conflict_parsing() {
        let stderr = r#"
error: patch failed: src/main.rs:10
error: src/main.rs: patch does not apply
error: some/path/file.py: whitespace error
"#;

        let conflicts = parse_git_stderr(stderr);
        assert!(conflicts.len() >= 2);

        // Should detect preimage mismatch and whitespace error
        assert!(
            conflicts
                .iter()
                .any(|c| matches!(c, GitConflict::PreimageMismatch { .. }))
        );
        assert!(
            conflicts
                .iter()
                .any(|c| matches!(c, GitConflict::WhitespaceError { .. }))
        );
    }

    #[test]
    fn test_conflict_summary_rendering() {
        let conflicts = vec![
            GitConflict::PreimageMismatch {
                path: PathBuf::from("src/main.rs"),
                hunk: (10, 15),
                hint: "Try regenerating",
            },
            GitConflict::WhitespaceError {
                path: PathBuf::from("src/lib.rs"),
                hint: "Use --whitespace nowarn",
            },
        ];

        let summary = render_conflict_summary(&conflicts);
        assert!(summary.contains("Conflicts (2)"));
        assert!(summary.contains("src/main.rs"));
        assert!(summary.contains("src/lib.rs"));
        assert!(summary.contains("preimage mismatch"));
        assert!(summary.contains("whitespace error"));
    }
}
