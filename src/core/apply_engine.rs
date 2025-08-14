//! Unified apply engine trait for hybrid EBNF→Git architecture
//!
//! Provides common interface for internal and git engines with
//! automatic fallback and user-friendly reporting.

use anyhow::Result;
use std::path::PathBuf;

use crate::cli::{ApplyEngine as EngineChoice, GitMode, WhitespaceMode};
use crate::core::edit::EditSpec;
use crate::core::git::{GitEngine, GitOptions};
use crate::core::patch::{PatchConfig, generate_patches};

/// Engine selection for apply operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Engine {
    Internal,
    Git,
    Auto,
}

/// Apply operation preview
#[derive(Debug)]
pub struct Preview {
    pub patch_content: String,
    pub summary: String,
    pub conflicts: Vec<String>,
    pub engine_used: Engine,
}

/// Apply operation result
#[derive(Debug)]
pub struct ApplyReport {
    pub applied_files: Vec<PathBuf>,
    pub conflicts: Vec<String>,
    pub engine_used: Engine,
    pub backup_paths: Vec<PathBuf>,
}

/// Unified apply engine trait
pub trait ApplyEngine {
    /// Check if edit spec can be applied (preview mode)
    fn check(&self, spec: &EditSpec) -> Result<Preview>;

    /// Apply edit specification
    fn apply(&self, spec: &EditSpec) -> Result<ApplyReport>;
}

/// Internal engine implementation
pub struct InternalEngine {
    backup_enabled: bool,
    force_mode: bool,
    context_lines: usize,
}

impl InternalEngine {
    pub fn new(backup_enabled: bool, force_mode: bool, context_lines: usize) -> Self {
        Self {
            backup_enabled,
            force_mode,
            context_lines,
        }
    }
}

impl ApplyEngine for InternalEngine {
    fn check(&self, spec: &EditSpec) -> Result<Preview> {
        let engine = crate::core::edit::EditEngine::new()
            .with_preview(true)
            .with_force(self.force_mode);

        let result = engine.apply(spec)?;

        // Generate patch for preview with configured context lines
        let config = PatchConfig {
            context_lines: self.context_lines,
            ..PatchConfig::default()
        };
        let patch_set = generate_patches(spec, &config)?;
        let patch_content = crate::core::patch::render_unified_diff(&patch_set);

        let conflicts = crate::core::git::render_conflict_summary(
            &result
                .conflicts
                .iter()
                .map(|c| match c {
                    crate::core::edit::EditConflict::FileNotFound(path) => {
                        crate::core::git::GitConflict::Other(format!(
                            "file not found: {}",
                            path.display()
                        ))
                    }
                    crate::core::edit::EditConflict::ContentMismatch {
                        file,
                        expected_cid: _,
                        actual_cid: _,
                    } => crate::core::git::GitConflict::PreimageMismatch {
                        path: file.clone(),
                        hunk: (0, 0),
                        hint: "CID mismatch - content changed",
                    },
                    crate::core::edit::EditConflict::SpanOutOfRange { file, span, .. } => {
                        crate::core::git::GitConflict::Other(format!(
                            "span out of range: {}:{}-{}",
                            file.display(),
                            span.0,
                            span.1
                        ))
                    }
                    crate::core::edit::EditConflict::OldContentMismatch { file, span } => {
                        crate::core::git::GitConflict::PreimageMismatch {
                            path: file.clone(),
                            hunk: (span.0 as u32, span.1 as u32),
                            hint: "OLD content mismatch",
                        }
                    }
                })
                .collect::<Vec<_>>(),
        );

        let summary = format!(
            "Preview: {} file(s), {} operation(s), {} conflict(s)",
            spec.file_blocks.len(),
            spec.file_blocks
                .iter()
                .map(|fb| fb.operations.len())
                .sum::<usize>(),
            result.conflicts.len()
        );

        Ok(Preview {
            patch_content,
            summary,
            conflicts,
            engine_used: Engine::Internal,
        })
    }

    fn apply(&self, spec: &EditSpec) -> Result<ApplyReport> {
        let engine = crate::core::edit::EditEngine::new()
            .with_backup(self.backup_enabled)
            .with_force(self.force_mode);

        let result = engine.apply(spec)?;

        let conflicts = crate::core::git::render_conflict_summary(
            &result
                .conflicts
                .iter()
                .map(|c| match c {
                    crate::core::edit::EditConflict::FileNotFound(path) => {
                        crate::core::git::GitConflict::Other(format!(
                            "file not found: {}",
                            path.display()
                        ))
                    }
                    crate::core::edit::EditConflict::ContentMismatch {
                        file,
                        expected_cid: _,
                        actual_cid: _,
                    } => crate::core::git::GitConflict::PreimageMismatch {
                        path: file.clone(),
                        hunk: (0, 0),
                        hint: "CID mismatch - content changed",
                    },
                    crate::core::edit::EditConflict::SpanOutOfRange { file, span, .. } => {
                        crate::core::git::GitConflict::Other(format!(
                            "span out of range: {}:{}-{}",
                            file.display(),
                            span.0,
                            span.1
                        ))
                    }
                    crate::core::edit::EditConflict::OldContentMismatch { file, span } => {
                        crate::core::git::GitConflict::PreimageMismatch {
                            path: file.clone(),
                            hunk: (span.0 as u32, span.1 as u32),
                            hint: "OLD content mismatch",
                        }
                    }
                })
                .collect::<Vec<_>>(),
        );

        Ok(ApplyReport {
            applied_files: result.applied_files,
            conflicts,
            engine_used: Engine::Internal,
            backup_paths: result.backup_paths,
        })
    }
}

/// Git engine implementation
pub struct GitEngineWrapper {
    git_engine: GitEngine,
}

impl GitEngineWrapper {
    pub fn new(git_options: GitOptions) -> Result<Self> {
        let git_engine = GitEngine::new(git_options)?;
        Ok(Self { git_engine })
    }
}

impl ApplyEngine for GitEngineWrapper {
    fn check(&self, spec: &EditSpec) -> Result<Preview> {
        let config = PatchConfig {
            context_lines: self.git_engine.options().context_lines as usize,
            ..PatchConfig::default()
        };
        let patch_set = generate_patches(spec, &config)?;
        let patch_content = crate::core::patch::render_unified_diff(&patch_set);

        let outcome = self.git_engine.check(&patch_set)?;

        let conflicts = crate::core::git::render_conflict_summary(&outcome.conflicts);

        let summary = format!(
            "Git Preview: {} file(s), {} conflict(s)",
            patch_set.file_patches.len(),
            outcome.conflicts.len()
        );

        Ok(Preview {
            patch_content,
            summary,
            conflicts,
            engine_used: Engine::Git,
        })
    }

    fn apply(&self, spec: &EditSpec) -> Result<ApplyReport> {
        let config = PatchConfig {
            context_lines: self.git_engine.options().context_lines as usize,
            ..PatchConfig::default()
        };
        let patch_set = generate_patches(spec, &config)?;

        let outcome = self.git_engine.apply(&patch_set)?;

        let conflicts = crate::core::git::render_conflict_summary(&outcome.conflicts);

        Ok(ApplyReport {
            applied_files: outcome.applied_files,
            conflicts,
            engine_used: Engine::Git,
            backup_paths: Vec::new(), // Git doesn't create backups
        })
    }
}

/// Hybrid engine with automatic fallback
pub struct HybridEngine {
    internal: InternalEngine,      // always available
    git: Option<GitEngineWrapper>, // lazy, only if repo exists
}

impl HybridEngine {
    pub fn new(
        backup_enabled: bool,
        force_mode: bool,
        git_options: GitOptions,
        repo_present: bool,
    ) -> Result<Self> {
        let context_lines = git_options.context_lines as usize;
        let internal = InternalEngine::new(backup_enabled, force_mode, context_lines);
        let git = if repo_present {
            Some(GitEngineWrapper::new(git_options)?)
        } else {
            None
        };
        Ok(Self { internal, git })
    }
}

impl ApplyEngine for HybridEngine {
    fn check(&self, spec: &EditSpec) -> Result<Preview> {
        // Try internal first
        match self.internal.check(spec) {
            Ok(preview) if preview.conflicts.is_empty() => Ok(preview),
            Ok(mut preview) => {
                // Internal has conflicts, also show git preview if available
                if let Some(git) = &self.git {
                    let git_preview = git.check(spec)?;
                    preview.summary.push_str(&format!(
                        " | Git: {} conflict(s)",
                        git_preview.conflicts.len()
                    ));
                    // concatenate machine-readable conflicts for scripts
                    if !git_preview.conflicts.is_empty() {
                        preview.conflicts.extend(git_preview.conflicts);
                    }
                }
                Ok(preview)
            }
            Err(_) => {
                // Internal failed, try git if available
                if let Some(git) = &self.git {
                    git.check(spec)
                } else {
                    Err(anyhow::anyhow!(
                        "Internal engine failed and git not available"
                    ))
                }
            }
        }
    }

    fn apply(&self, spec: &EditSpec) -> Result<ApplyReport> {
        // Try internal first
        match self.internal.apply(spec) {
            Ok(report) if report.conflicts.is_empty() => Ok(report),
            Ok(internal_report) => {
                // Internal has conflicts; attempt git if available
                if let Some(git) = &self.git {
                    match git.apply(spec) {
                        Ok(mut git_report) => {
                            git_report.engine_used = Engine::Auto;
                            // Optionally surface that internal had conflicts but git resolved them:
                            if !internal_report.conflicts.is_empty() {
                                git_report.conflicts.extend(internal_report.conflicts);
                            }
                            Ok(git_report)
                        }
                        Err(err) => {
                            // Return a combined conflict report via error so CLI can emit exit 2
                            let combined = crate::core::git::CombinedConflictError::new(
                                internal_report.conflicts.clone(),
                                format!("git_apply_failed: {err}"),
                            );
                            Err(combined.into())
                        }
                    }
                } else {
                    // No git available; escalate as conflicts so CLI returns code 2
                    let combined = crate::core::git::CombinedConflictError::new(
                        internal_report.conflicts.clone(),
                        "git_unavailable".into(),
                    );
                    Err(combined.into())
                }
            }
            Err(e) => {
                // Internal failed; try git if available, otherwise propagate
                if let Some(git) = &self.git {
                    git.apply(spec)
                        .map(|mut r| {
                            r.engine_used = Engine::Auto;
                            r
                        })
                        .or(Err(e))
                } else {
                    Err(e)
                }
            }
        }
    }
}

/// Create appropriate engine based on user choice
pub fn create_engine(
    engine_choice: &EngineChoice,
    git_mode: &GitMode,
    whitespace: &WhitespaceMode,
    backup_enabled: bool,
    force_mode: bool,
    repo_root: PathBuf,
    context_lines: usize,
) -> Result<Box<dyn ApplyEngine>> {
    let git_options = GitOptions {
        repo_root,
        mode: match git_mode {
            GitMode::ThreeWay => crate::core::git::GitMode::ThreeWay,
            GitMode::Index => crate::core::git::GitMode::Index,
            GitMode::Worktree => crate::core::git::GitMode::Worktree,
        },
        whitespace: match whitespace {
            WhitespaceMode::Nowarn => crate::core::git::Whitespace::Nowarn,
            WhitespaceMode::Warn => crate::core::git::Whitespace::Warn,
            WhitespaceMode::Fix => crate::core::git::Whitespace::Fix,
        },
        context_lines: context_lines as u8,
        allow_outside_repo: false,
    };

    match engine_choice {
        EngineChoice::Internal => Ok(Box::new(InternalEngine::new(
            backup_enabled,
            force_mode,
            context_lines,
        ))),
        EngineChoice::Git => Ok(Box::new(GitEngineWrapper::new(git_options)?)),
        EngineChoice::Auto => {
            // Detect repo once here; do NOT fail auto if absent
            let repo_present = crate::core::git::detect_repo(&git_options.repo_root).is_ok();
            let mut auto_git_options = git_options;
            auto_git_options.allow_outside_repo = true; // Allow auto to work outside repos
            Ok(Box::new(HybridEngine::new(
                backup_enabled,
                force_mode,
                auto_git_options,
                repo_present,
            )?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::edit::{EditOperation, FileBlock};
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_internal_engine() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "line 1").unwrap();
        writeln!(temp_file, "line 2").unwrap();

        let spec = EditSpec {
            file_blocks: vec![FileBlock {
                path: temp_file.path().to_path_buf(),
                operations: vec![EditOperation::Replace {
                    start_line: 2,
                    end_line: 2,
                    old_content: "line 2".to_string(),
                    new_content: "modified line 2".to_string(),
                    guard_cid: None,
                }],
            }],
        };

        let engine = InternalEngine::new(false, false, 3);
        let preview = engine.check(&spec).unwrap();

        assert_eq!(preview.engine_used, Engine::Internal);
        assert!(preview.patch_content.contains("modified line 2"));
        assert!(preview.conflicts.is_empty());
    }
}
