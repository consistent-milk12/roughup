//! Unified apply engine trait for hybrid EBNF→Git architecture
//!
//! Provides common interface for internal and git engines with
//! automatic fallback and user-friendly reporting.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::{
    cli::{ApplyEngine as EngineChoice, GitMode, WhitespaceMode},
    core::{
        backup::BackupManager,
        edit::EditSpec,
        git::{GitEngine, GitOptions},
        patch::{PatchConfig, generate_patches},
    },
};

/// Engine selection for apply operations
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Engine
{
    Internal,
    Git,
    Auto,
}

/// Apply operation preview
#[derive(Debug)]
pub struct Preview
{
    pub patch_content: String,
    pub summary: String,
    pub conflicts: Vec<String>,
    pub engine_used: Engine,
}

/// Context for apply operations carrying repo, backup session, and runtime flags
#[derive(Debug)]
pub struct ApplyContext<'a>
{
    /// Absolute path to the repository root
    pub repo_root: &'a Path,
    /// Optional centralized backup session for this apply
    pub backup: Option<&'a mut BackupManager>,
    /// Whitespace handling mode
    pub whitespace: WhitespaceMode,
    /// Context lines for patches
    pub context_lines: usize,
    /// Force mode flag
    pub force: bool,
}

/// Apply operation result
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApplyReport
{
    pub applied_files: Vec<PathBuf>,
    pub conflicts: Vec<String>,
    pub engine_used: Engine,
    /// Legacy-compatible: now points to session directory
    pub backup_paths: Vec<PathBuf>,
    /// New: first-class session info
    pub backup_session_id: Option<String>,
    pub backup_manifest_path: Option<PathBuf>,
    pub backup_file_count: Option<usize>,
}

/// Unified apply engine trait
pub trait ApplyEngine
{
    /// Check if edit spec can be applied (preview mode)
    fn check(
        &self,
        spec: &EditSpec,
    ) -> Result<Preview>;

    /// New: contextful API (preferred) - apply edit specification with context
    fn apply_with_ctx(
        &self,
        spec: &EditSpec,
        ctx: ApplyContext<'_>,
    ) -> Result<ApplyReport>;

    /// Old: backward-compatible default impl forwards without backup/session
    fn apply(
        &self,
        spec: &EditSpec,
    ) -> Result<ApplyReport>
    {
        // Create default context without backup session
        let ctx = ApplyContext {
            repo_root: Path::new("."), // Default to current directory
            backup: None,
            whitespace: WhitespaceMode::Nowarn,
            context_lines: 3,
            force: false,
        };
        self.apply_with_ctx(spec, ctx)
    }
}

/// Turns absolute file path into repo-relative path or errors.
/// Enforces boundary to keep backups inside repo root.
fn make_relative_to_repo(
    file_path: &Path,
    repo_root: &Path,
) -> Result<PathBuf>
{
    // Normalize repo root (prefer canonical, fall back to given)
    let repo_abs = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());

    if file_path.is_absolute()
    {
        // Absolute target: canonicalize if possible, then strip prefix
        let abs = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());
        return abs
            .strip_prefix(&repo_abs)
            .map(|p| p.to_path_buf())
            .with_context(|| {
                format!(
                    "File {} is outside repository {}",
                    file_path.display(),
                    repo_abs.display()
                )
            });
    }

    // Relative target: treat as repo-relative without touching the FS.
    // Validate and normalize "..", "." etc. (disallow escape)
    crate::core::backup_ops::normalize_repo_rel(file_path)
}

/// Internal engine implementation
pub struct InternalEngine
{
    #[expect(unused, reason = "Moved to Centralized BackupManager")]
    backup_enabled: bool,
    force_mode: bool,
    context_lines: usize,
}

impl InternalEngine
{
    pub fn new(
        backup_enabled: bool,
        force_mode: bool,
        context_lines: usize,
    ) -> Self
    {
        Self { backup_enabled, force_mode, context_lines }
    }
}

impl ApplyEngine for InternalEngine
{
    fn check(
        &self,
        spec: &EditSpec,
    ) -> Result<Preview>
    {
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
                .map(|c| {
                    match c
                    {
                        crate::core::edit::EditConflict::FileNotFound(path) =>
                        {
                            crate::core::git::GitConflict::Other(format!(
                                "file not found: {}",
                                path.display()
                            ))
                        }
                        crate::core::edit::EditConflict::ContentMismatch {
                            file,
                            expected_cid: _,
                            actual_cid: _,
                        } =>
                        {
                            crate::core::git::GitConflict::PreimageMismatch {
                                path: file.clone(),
                                hunk: (0, 0),
                                hint: "CID mismatch - content changed",
                            }
                        }
                        crate::core::edit::EditConflict::SpanOutOfRange { file, span, .. } =>
                        {
                            crate::core::git::GitConflict::Other(format!(
                                "span out of range: {}:{}-{}",
                                file.display(),
                                span.0,
                                span.1
                            ))
                        }
                        crate::core::edit::EditConflict::OldContentMismatch { file, span } =>
                        {
                            crate::core::git::GitConflict::PreimageMismatch {
                                path: file.clone(),
                                hunk: (span.0 as u32, span.1 as u32),
                                hint: "OLD content mismatch",
                            }
                        }
                    }
                })
                .collect::<Vec<_>>(),
        );

        let summary = format!(
            "Preview: {} file(s), {} operation(s), {} conflict(s)",
            spec.file_blocks
                .len(),
            spec.file_blocks
                .iter()
                .map(|fb| {
                    fb.operations
                        .len()
                })
                .sum::<usize>(),
            result
                .conflicts
                .len()
        );

        Ok(Preview {
            patch_content,
            summary,
            conflicts,
            engine_used: Engine::Internal,
        })
    }

    fn apply_with_ctx(
        &self,
        spec: &EditSpec,
        mut ctx: ApplyContext<'_>,
    ) -> Result<ApplyReport>
    {
        let mut applied = Vec::new();

        // If centralized backup is enabled, back up files before modification
        if let Some(backup_manager) = ctx
            .backup
            .as_mut()
        {
            for file_block in &spec.file_blocks
            {
                // Check if this block will modify the file
                let will_modify = !file_block
                    .operations
                    .is_empty();
                if will_modify
                {
                    // Convert to repo-relative path for backup
                    let rel_path = make_relative_to_repo(&file_block.path, ctx.repo_root)?;
                    backup_manager.backup_file(&rel_path)?;
                }
            }
        }

        // Apply using existing edit engine (without its own backup since we handle centrally)
        let engine = crate::core::edit::EditEngine::new()
            .with_backup(false) // Disable internal backup, we handle it centrally
            .with_force(ctx.force);

        let result = engine.apply(spec)?;
        applied.extend(result.applied_files);

        let conflicts = crate::core::git::render_conflict_summary(
            &result
                .conflicts
                .iter()
                .map(|c| {
                    match c
                    {
                        crate::core::edit::EditConflict::FileNotFound(path) =>
                        {
                            crate::core::git::GitConflict::Other(format!(
                                "file not found: {}",
                                path.display()
                            ))
                        }
                        crate::core::edit::EditConflict::ContentMismatch {
                            file,
                            expected_cid: _,
                            actual_cid: _,
                        } =>
                        {
                            crate::core::git::GitConflict::PreimageMismatch {
                                path: file.clone(),
                                hunk: (0, 0),
                                hint: "CID mismatch - content changed",
                            }
                        }
                        crate::core::edit::EditConflict::SpanOutOfRange { file, span, .. } =>
                        {
                            crate::core::git::GitConflict::Other(format!(
                                "span out of range: {}:{}-{}",
                                file.display(),
                                span.0,
                                span.1
                            ))
                        }
                        crate::core::edit::EditConflict::OldContentMismatch { file, span } =>
                        {
                            crate::core::git::GitConflict::PreimageMismatch {
                                path: file.clone(),
                                hunk: (span.0 as u32, span.1 as u32),
                                hint: "OLD content mismatch",
                            }
                        }
                    }
                })
                .collect::<Vec<_>>(),
        );

        // Finalize backup session if present
        let (session_id, session_dir, file_count) = if let Some(backup_manager) = ctx
            .backup
            .as_mut()
        {
            let success = conflicts.is_empty(); // Success if no conflicts
            backup_manager.finalize(success)?;
            (
                Some(
                    backup_manager
                        .session_id()
                        .to_string(),
                ),
                Some(
                    backup_manager
                        .session_dir()
                        .to_path_buf(),
                ),
                Some(backup_manager.file_count()),
            )
        }
        else
        {
            (None, None, None)
        };

        Ok(ApplyReport {
            applied_files: applied,
            conflicts,
            engine_used: Engine::Internal,
            // Legacy-compatible: session directory in backup_paths
            backup_paths: session_dir
                .iter()
                .cloned()
                .collect(),
            // New fields
            backup_session_id: session_id,
            backup_manifest_path: session_dir
                .as_ref()
                .map(|d| d.join("manifest.json")),
            backup_file_count: file_count,
        })
    }
}

/// Git engine implementation
pub struct GitEngineWrapper
{
    git_engine: GitEngine,
}

impl GitEngineWrapper
{
    pub fn new(git_options: GitOptions) -> Result<Self>
    {
        let git_engine = GitEngine::new(git_options)?;
        Ok(Self { git_engine })
    }
}

impl ApplyEngine for GitEngineWrapper
{
    fn check(
        &self,
        spec: &EditSpec,
    ) -> Result<Preview>
    {
        let config = PatchConfig {
            context_lines: self
                .git_engine
                .options()
                .context_lines as usize,
            ..PatchConfig::default()
        };
        let patch_set = generate_patches(spec, &config)?;
        let patch_content = crate::core::patch::render_unified_diff(&patch_set);

        let outcome = self
            .git_engine
            .check(&patch_set)?;

        let conflicts = crate::core::git::render_conflict_summary(&outcome.conflicts);

        let summary = format!(
            "Git Preview: {} file(s), {} conflict(s)",
            patch_set
                .file_patches
                .len(),
            outcome
                .conflicts
                .len()
        );

        Ok(Preview {
            patch_content,
            summary,
            conflicts,
            engine_used: Engine::Git,
        })
    }

    fn apply_with_ctx(
        &self,
        spec: &EditSpec,
        mut ctx: ApplyContext<'_>,
    ) -> Result<ApplyReport>
    {
        let config = PatchConfig {
            context_lines: ctx.context_lines,
            ..PatchConfig::default()
        };
        let patch_set = generate_patches(spec, &config)?;

        // If centralized backup is enabled, back up files that git will modify
        if let Some(backup_manager) = ctx
            .backup
            .as_mut()
        {
            for file_patch in &patch_set.file_patches
            {
                let file_path = Path::new(&file_patch.path);
                let rel_path = make_relative_to_repo(file_path, ctx.repo_root)?;
                backup_manager.backup_file(&rel_path)?;
            }
        }

        let outcome = self
            .git_engine
            .apply(&patch_set)?;

        let conflicts = crate::core::git::render_conflict_summary(&outcome.conflicts);

        // Finalize backup session if present
        let (session_id, session_dir, file_count) = if let Some(backup_manager) = ctx
            .backup
            .as_mut()
        {
            let success = conflicts.is_empty(); // Success if no conflicts
            backup_manager.finalize(success)?;
            (
                Some(
                    backup_manager
                        .session_id()
                        .to_string(),
                ),
                Some(
                    backup_manager
                        .session_dir()
                        .to_path_buf(),
                ),
                Some(backup_manager.file_count()),
            )
        }
        else
        {
            (None, None, None)
        };

        Ok(ApplyReport {
            applied_files: outcome.applied_files,
            conflicts,
            engine_used: Engine::Git,
            // Legacy-compatible: session directory in backup_paths
            backup_paths: session_dir
                .iter()
                .cloned()
                .collect(),
            // New fields
            backup_session_id: session_id,
            backup_manifest_path: session_dir
                .as_ref()
                .map(|d| d.join("manifest.json")),
            backup_file_count: file_count,
        })
    }
}

/// Hybrid engine with automatic fallback
pub struct HybridEngine
{
    internal: InternalEngine,      // always available
    git: Option<GitEngineWrapper>, // lazy, only if repo exists
}

impl HybridEngine
{
    pub fn new(
        backup_enabled: bool,
        force_mode: bool,
        git_options: GitOptions,
        repo_present: bool,
    ) -> Result<Self>
    {
        let context_lines = git_options.context_lines as usize;
        let internal = InternalEngine::new(backup_enabled, force_mode, context_lines);
        let git = if repo_present
        {
            Some(GitEngineWrapper::new(git_options)?)
        }
        else
        {
            None
        };
        Ok(Self { internal, git })
    }
}

impl ApplyEngine for HybridEngine
{
    fn check(
        &self,
        spec: &EditSpec,
    ) -> Result<Preview>
    {
        // Try internal first
        match self
            .internal
            .check(spec)
        {
            Ok(preview)
                if preview
                    .conflicts
                    .is_empty() =>
            {
                Ok(preview)
            }
            Ok(mut preview) =>
            {
                // Internal has conflicts, also show git preview if available
                if let Some(git) = &self.git
                {
                    let git_preview = git.check(spec)?;
                    preview
                        .summary
                        .push_str(&format!(
                            " | Git: {} conflict(s)",
                            git_preview
                                .conflicts
                                .len()
                        ));
                    // concatenate machine-readable conflicts for scripts
                    if !git_preview
                        .conflicts
                        .is_empty()
                    {
                        preview
                            .conflicts
                            .extend(git_preview.conflicts);
                    }
                }
                Ok(preview)
            }
            Err(_) =>
            {
                // Internal failed, try git if available
                if let Some(git) = &self.git
                {
                    git.check(spec)
                }
                else
                {
                    Err(anyhow::anyhow!(
                        "Internal engine failed and git not available"
                    ))
                }
            }
        }
    }

    fn apply_with_ctx(
        &self,
        spec: &EditSpec,
        ctx: ApplyContext<'_>,
    ) -> Result<ApplyReport>
    {
        // For HybridEngine, we need to manually manage backup since engines finalize
        // independently This is a limitation of the current design that we document for
        // future improvements

        // Save context values before moving ctx (for fallback)
        let repo_root = ctx.repo_root;
        let whitespace = ctx.whitespace;
        let context_lines = ctx.context_lines;
        let force = ctx.force;

        // Try internal engine first (it will handle backup if needed)
        let internal_result = self
            .internal
            .apply_with_ctx(spec, ctx);

        match internal_result
        {
            Ok(report)
                if report
                    .conflicts
                    .is_empty() =>
            {
                // Internal succeeded without conflicts
                Ok(report)
            }
            Ok(internal_report) =>
            {
                // Internal has conflicts; attempt git fallback if available
                if let Some(git) = &self.git
                {
                    // Note: Internal engine has already finalized its backup session
                    // Git fallback runs without backup (design limitation)
                    let git_ctx = ApplyContext {
                        repo_root,
                        backup: None, // No backup for fallback since internal already handled it
                        whitespace,
                        context_lines,
                        force,
                    };

                    match git.apply_with_ctx(spec, git_ctx)
                    {
                        Ok(mut git_report) =>
                        {
                            // Git succeeded - combine reports
                            git_report.engine_used = Engine::Auto;
                            // Preserve internal's backup info
                            git_report.backup_paths = internal_report.backup_paths;
                            git_report.backup_session_id = internal_report.backup_session_id;
                            git_report.backup_manifest_path = internal_report.backup_manifest_path;
                            git_report.backup_file_count = internal_report.backup_file_count;

                            // Note: git_report.conflicts should be empty since Git succeeded
                            Ok(git_report)
                        }
                        Err(err) =>
                        {
                            // Both engines failed - return combined error
                            let combined = crate::core::git::CombinedConflictError::new(
                                internal_report
                                    .conflicts
                                    .clone(),
                                format!("git_apply_failed: {err}"),
                            );
                            Err(combined.into())
                        }
                    }
                }
                else
                {
                    // No git available; return internal conflicts as error
                    let combined = crate::core::git::CombinedConflictError::new(
                        internal_report
                            .conflicts
                            .clone(),
                        "git_unavailable".into(),
                    );
                    Err(combined.into())
                }
            }
            Err(e) =>
            {
                // Internal failed completely; try git if available
                // We can't use the moved ctx here, so create a new context
                if let Some(git) = &self.git
                {
                    let fallback_ctx = ApplyContext {
                        repo_root,
                        backup: None, // No backup available at this point
                        whitespace,
                        context_lines,
                        force,
                    };
                    git.apply_with_ctx(spec, fallback_ctx)
                        .map(|mut r| {
                            r.engine_used = Engine::Auto;
                            r
                        })
                        .or(Err(e))
                }
                else
                {
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
) -> Result<Box<dyn ApplyEngine>>
{
    let git_options = GitOptions {
        repo_root,
        mode: match git_mode
        {
            GitMode::ThreeWay => crate::core::git::GitMode::ThreeWay,
            GitMode::Index => crate::core::git::GitMode::Index,
            GitMode::Worktree => crate::core::git::GitMode::Worktree,
        },
        whitespace: match whitespace
        {
            WhitespaceMode::Nowarn => crate::core::git::Whitespace::Nowarn,
            WhitespaceMode::Warn => crate::core::git::Whitespace::Warn,
            WhitespaceMode::Fix => crate::core::git::Whitespace::Fix,
        },
        context_lines: context_lines as u8,
        allow_outside_repo: false,
    };

    match engine_choice
    {
        EngineChoice::Internal =>
        {
            Ok(Box::new(InternalEngine::new(
                backup_enabled,
                force_mode,
                context_lines,
            )))
        }
        EngineChoice::Git => Ok(Box::new(GitEngineWrapper::new(git_options)?)),
        EngineChoice::Auto =>
        {
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
mod tests
{
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;
    use crate::core::edit::{EditOperation, FileBlock};

    #[test]
    fn test_internal_engine()
    {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "line 1").unwrap();
        writeln!(temp_file, "line 2").unwrap();

        let spec = EditSpec {
            file_blocks: vec![FileBlock {
                path: temp_file
                    .path()
                    .to_path_buf(),
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
        let preview = engine
            .check(&spec)
            .unwrap();

        assert_eq!(preview.engine_used, Engine::Internal);
        assert!(
            preview
                .patch_content
                .contains("modified line 2")
        );
        assert!(
            preview
                .conflicts
                .is_empty()
        );
    }
}
