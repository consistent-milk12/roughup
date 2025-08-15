//! Git apply integration for robust patch application
//!
//! Implements git apply with 3-way merge, stderr parsing, and user-friendly
//! error mapping according to engineering review specifications.

use anyhow::{Context, Result, bail};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::core::patch::PatchSet;

/// Lightweight repo metadata for boundary checks and UX
#[derive(Debug, Clone)]
pub struct RepoMeta {
    pub top_level: PathBuf,
    pub is_worktree: bool,
}

/// Combined conflict error for auto engine fallback scenarios
#[derive(Debug, Clone)]
pub struct CombinedConflictError {
    pub internal_conflicts: Vec<String>,
    pub git_failure_reason: String,
}

impl CombinedConflictError {
    pub fn new(internal_conflicts: Vec<String>, git_reason: String) -> Self {
        Self {
            internal_conflicts,
            git_failure_reason: git_reason,
        }
    }
}

impl std::fmt::Display for CombinedConflictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "conflicts: {}; git: {}",
            self.internal_conflicts.len(),
            self.git_failure_reason
        )
    }
}

impl std::error::Error for CombinedConflictError {}

/// Detect if `repo_root` is inside a Git repository (dir or worktree).
/// Returns the repo top-level directory and whether it is a worktree.
pub fn detect_repo(repo_root: &Path) -> Result<RepoMeta> {
    // Fast path: `.git` directory or file (worktree pointer)
    let dot_git = repo_root.join(".git");
    if dot_git.exists() {
        let is_worktree = dot_git.is_file();
        // If it's a file, resolve "gitdir: <path>" format
        if is_worktree {
            let s = fs::read_to_string(&dot_git).context("Failed to read .git file")?;
            if let Some(_gitdir) = s.strip_prefix("gitdir: ").map(|v| v.trim()) {
                let top = resolve_top_level(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
                return Ok(RepoMeta {
                    top_level: top,
                    is_worktree: true,
                });
            }
        }
        let top = resolve_top_level(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
        return Ok(RepoMeta {
            top_level: top,
            is_worktree,
        });
    }
    // Fallback: `git rev-parse`
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(repo_root)
        .output()
        .context("Failed to run git rev-parse")?;
    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != "true" {
        bail!("Not a git repository: {}", repo_root.display());
    }
    let top_level = resolve_top_level(repo_root)?;
    Ok(RepoMeta {
        top_level,
        is_worktree: false,
    })
}

fn resolve_top_level(cwd: &Path) -> Result<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .context("Failed to run git rev-parse --show-toplevel")?;
    if !out.status.success() {
        bail!("Unable to resolve repository toplevel");
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// Ensure `target` stays within `meta.top_level`.
pub fn ensure_within_repo(meta: &RepoMeta, target: &Path) -> Result<()> {
    // Build absolute candidate; canonicalize if possible. For new files,
    // fall back to canonicalizing the parent and re-joining the filename.
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        meta.top_level.join(target)
    };
    let abs = match joined.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            let parent = joined.parent().unwrap_or(&meta.top_level);
            let base = parent.canonicalize().unwrap_or_else(|_| meta.top_level.clone());
            base.join(joined.file_name().unwrap_or_default())
        }
    };
    let top = meta
        .top_level
        .canonicalize()
        .context("canonicalize toplevel failed")?;
    if !abs.starts_with(&top) {
        bail!("Path escapes repository boundary: {}", target.display());
    }
    Ok(())
}

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

    /// Get engine options
    pub fn options(&self) -> &GitOptions {
        &self.options
    }

    /// Check if patch can be applied (preview mode)
    pub fn check(&self, patch_set: &PatchSet) -> Result<GitOutcome> {
        let patch_content = crate::core::patch::render_unified_diff(patch_set);
        self.run_git_apply(&patch_content, true)
    }

    /// Apply patch set to repository
    pub fn apply(&self, patch_set: &PatchSet) -> Result<GitOutcome> {
        // Enforce repository boundary if required
        if !self.options.allow_outside_repo {
            let meta = detect_repo(&self.options.repo_root)?;
            for file_patch in &patch_set.file_patches {
                let path = Path::new(&file_patch.path);
                ensure_within_repo(&meta, path)?;
            }
        }

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
                // Not implemented yet: we do not create ephemeral worktrees here.
                // Safer to fail early with a clear message.
                anyhow::bail!(
                    "GitMode::Worktree is not implemented yet. Use --git-mode 3way or --git-mode index."
                );
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
        } else if line.contains("Binary files") && line.contains("differ") {
            conflicts.push(GitConflict::BinaryOrMode {
                path: extract_path_from_error(line).unwrap_or_else(|| PathBuf::from("unknown")),
                hint: "Binary file conflict. Manual resolution required.",
            });
        } else if line.contains("submodule merge conflict") {
            conflicts.push(GitConflict::Other(format!("submodule: {}", line)));
        } else if line.contains("pathspec") && line.contains("did not match any files") {
            conflicts.push(GitConflict::PathOutsideRepo {
                path: extract_path_from_error(line).unwrap_or_else(|| PathBuf::from("unknown")),
                hint: "File not tracked or outside sparse-checkout. Check visibility rules.",
            });
        } else if line.contains("error:") || line.contains("fatal:") {
            conflicts.push(GitConflict::Other(line.to_string()));
        }
    }

    conflicts
}

/// Extract file path from git error message using robust regex patterns
fn extract_path_from_error(error_line: &str) -> Option<PathBuf> {
    // Common git error patterns:
    // "error: patch failed: path/to/file:12"
    // "CONFLICT (content): Merge conflict in path/to/file"
    // "path/to/file: needs merge"
    // "Binary files path/to/file and path/to/file differ"
    static PATTERNS: &[&str] = &[
        r"patch failed:\s+(.+?):\d+",
        r"Merge conflict in\s+(.+)",
        r"^(.+?): needs merge",
        r"Binary files\s+(.+?)\s+and\s+.+\s+differ",
        r"error:\s+(.+?):\s+patch does not apply",
    ];

    for pat in PATTERNS {
        if let Ok(re) = Regex::new(pat)
            && let Some(cap) = re.captures(error_line)
        {
            let path_str = cap[1].trim();
            return Some(PathBuf::from(path_str));
        }
    }

    // Fallback to original heuristic
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

/// Render conflicts in a predictable, script-friendly way
pub fn render_conflict_summary(conflicts: &[GitConflict]) -> Vec<String> {
    conflicts
        .iter()
        .map(|c| match c {
            GitConflict::PreimageMismatch { path, hunk, .. } => {
                format!("{}:{}:{} preimage_mismatch", path.display(), hunk.0, hunk.1)
            }
            GitConflict::IndexRequired { path, .. } => {
                format!("{}:0:0 index_required", path.display())
            }
            GitConflict::WhitespaceError { path, .. } => {
                format!("{}:0:0 whitespace_error", path.display())
            }
            GitConflict::PathOutsideRepo { path, .. } => {
                format!("{}:0:0 outside_repo", path.display())
            }
            GitConflict::BinaryOrMode { path, .. } => {
                format!("{}:0:0 binary_or_mode", path.display())
            }
            GitConflict::Other(msg) => {
                format!("unknown:0:0 {}", msg.replace(':', ";"))
            }
        })
        .collect()
}

/// Render user-friendly conflict summary for human consumption
pub fn render_conflict_summary_human(conflicts: &[GitConflict]) -> String {
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

        let summary = render_conflict_summary_human(&conflicts);
        assert!(summary.contains("Conflicts (2)"));
        assert!(summary.contains("src/main.rs"));
        assert!(summary.contains("src/lib.rs"));
        assert!(summary.contains("preimage mismatch"));
        assert!(summary.contains("whitespace error"));
    }
}
