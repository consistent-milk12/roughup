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
}

impl InternalEngine {
    pub fn new(backup_enabled: bool, force_mode: bool) -> Self {
        Self {
            backup_enabled,
            force_mode,
        }
    }
}

impl ApplyEngine for InternalEngine {
    fn check(&self, spec: &EditSpec) -> Result<Preview> {
        let engine = crate::core::edit::EditEngine::new()
            .with_preview(true)
            .with_force(self.force_mode);

        let result = engine.apply(spec)?;

        // Generate patch for preview
        let config = PatchConfig::default();
        let patch_set = generate_patches(spec, &config)?;
        let patch_content = crate::core::patch::render_unified_diff(&patch_set);

        let conflicts: Vec<String> = result
            .conflicts
            .iter()
            .map(|c| format!("{:?}", c)) // TODO: Better formatting
            .collect();

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

        let conflicts: Vec<String> = result
            .conflicts
            .iter()
            .map(|c| format!("{:?}", c)) // TODO: Better formatting
            .collect();

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
        let config = PatchConfig::default();
        let patch_set = generate_patches(spec, &config)?;
        let patch_content = crate::core::patch::render_unified_diff(&patch_set);

        let outcome = self.git_engine.check(&patch_set)?;

        let conflicts: Vec<String> = outcome
            .conflicts
            .iter()
            .map(|c| format!("{:?}", c)) // TODO: Use render_conflict_summary
            .collect();

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
        let config = PatchConfig::default();
        let patch_set = generate_patches(spec, &config)?;

        let outcome = self.git_engine.apply(&patch_set)?;

        let conflicts: Vec<String> = outcome
            .conflicts
            .iter()
            .map(|c| format!("{:?}", c)) // TODO: Use render_conflict_summary
            .collect();

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
    internal: InternalEngine,
    git: GitEngineWrapper,
}

impl HybridEngine {
    pub fn new(backup_enabled: bool, force_mode: bool, git_options: GitOptions) -> Result<Self> {
        let internal = InternalEngine::new(backup_enabled, force_mode);
        let git = GitEngineWrapper::new(git_options)?;

        Ok(Self { internal, git })
    }
}

impl ApplyEngine for HybridEngine {
    fn check(&self, spec: &EditSpec) -> Result<Preview> {
        // Try internal first
        match self.internal.check(spec) {
            Ok(preview) if preview.conflicts.is_empty() => Ok(preview),
            Ok(mut preview) => {
                // Internal has conflicts, also show git preview
                let git_preview = self.git.check(spec)?;
                preview.summary.push_str(&format!(
                    " | Git: {} conflict(s)",
                    git_preview.conflicts.len()
                ));
                Ok(preview)
            }
            Err(_) => {
                // Internal failed, try git
                self.git.check(spec)
            }
        }
    }

    fn apply(&self, spec: &EditSpec) -> Result<ApplyReport> {
        // Try internal first
        match self.internal.apply(spec) {
            Ok(report) if report.conflicts.is_empty() => Ok(report),
            Ok(internal_report) => {
                // Internal has conflicts, retry with git
                println!("⚠️  Internal engine conflicts detected, retrying with git --3way");
                match self.git.apply(spec) {
                    Ok(mut git_report) => {
                        git_report.engine_used = Engine::Auto;
                        Ok(git_report)
                    }
                    Err(_) => {
                        // Git also failed, return internal result
                        Ok(internal_report)
                    }
                }
            }
            Err(e) => {
                // Internal failed completely, try git
                println!("⚠️  Internal engine failed, retrying with git");
                self.git.apply(spec).or(Err(e))
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
        context_lines: 3,
        allow_outside_repo: false,
    };

    match engine_choice {
        EngineChoice::Internal => Ok(Box::new(InternalEngine::new(backup_enabled, force_mode))),
        EngineChoice::Git => Ok(Box::new(GitEngineWrapper::new(git_options)?)),
        EngineChoice::Auto => Ok(Box::new(HybridEngine::new(
            backup_enabled,
            force_mode,
            git_options,
        )?)),
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

        let engine = InternalEngine::new(false, false);
        let preview = engine.check(&spec).unwrap();

        assert_eq!(preview.engine_used, Engine::Internal);
        assert!(preview.patch_content.contains("modified line 2"));
        assert!(preview.conflicts.is_empty());
    }
}
