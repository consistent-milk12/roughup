//! Edit format parsing and application system
//!
//! Implements the EBNF edit format from Suggestions.md:
//! - FILE: path blocks with REPLACE/INSERT/DELETE operations
//! - GUARD-CID system for change detection
//! - Safe atomic file operations with preview/backup

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::cli::{
    AppContext, ApplyArgs, BackupArgs, BackupCleanupArgs, BackupListArgs, BackupRestoreArgs,
    BackupShowArgs, BackupSubcommand, CheckSyntaxArgs, PreviewArgs,
};
use crate::core::apply_engine::create_engine;
use crate::core::backup_ops::{
    CleanupRequest, ListRequest, RestoreRequest, SessionInfo, ShowRequest, cleanup_sessions,
    list_sessions_filtered, restore_session, show_session,
};

/// Content ID for change detection (xxh64 hash)
pub type ContentId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    Line,
    Token,
    Auto,
}

/// Shared normalizer for both CID and OLD comparisons  
pub fn normalize_for_cid(s: &str) -> String {
    // Split into lines, remove trailing spaces and '\r'
    s.lines()
        .map(|l| l.trim_end_matches(&[' ', '\t', '\r'][..]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate deterministic content ID using xxh64 with fixed seed
pub fn generate_cid(content: &str) -> ContentId {
    let normalized = normalize_for_cid(content);
    let h = xxhash_rust::xxh64::xxh64(normalized.as_bytes(), 0);
    format!("{:016x}", h)
}

/// Edit operation types
#[derive(Debug, Clone, PartialEq)]
pub enum EditOperation {
    Replace {
        start_line: usize, // 1-based inclusive
        end_line: usize,   // 1-based inclusive
        old_content: String,
        new_content: String,
        guard_cid: Option<ContentId>,
    },
    Insert {
        at_line: usize, // 1-based, insert after this line (0 = beginning)
        new_content: String,
    },
    Delete {
        start_line: usize, // 1-based inclusive
        end_line: usize,   // 1-based inclusive
    },
}

/// File block containing path and operations
#[derive(Debug, Clone)]
pub struct FileBlock {
    pub path: PathBuf,
    pub operations: Vec<EditOperation>,
}

/// Complete edit specification
#[derive(Debug, Clone)]
pub struct EditSpec {
    pub file_blocks: Vec<FileBlock>,
}

/// Edit parsing errors
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Invalid FILE block: {0}")]
    InvalidFileBlock(String),
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("Invalid line number: {0}")]
    InvalidLineNumber(String),
    #[error("Invalid span format: {0}")]
    InvalidSpan(String),
}

/// Edit conflict types
#[derive(Debug, Clone)]
pub enum EditConflict {
    FileNotFound(PathBuf),

    SpanOutOfRange {
        file: PathBuf,
        span: (usize, usize),
        file_lines: usize,
    },

    ContentMismatch {
        file: PathBuf,
        expected_cid: ContentId,
        actual_cid: ContentId,
    },

    OldContentMismatch {
        file: PathBuf,
        span: (usize, usize),
    },
}

/// Edit application result
#[derive(Debug)]
pub struct EditResult {
    pub applied_files: Vec<PathBuf>,
    pub conflicts: Vec<EditConflict>,
    pub backup_paths: Vec<PathBuf>,
}

/// Domain-specific error taxonomy for exit-code mapping
#[derive(thiserror::Error, Debug, Clone)]
pub enum ApplyCliError {
    /// Unusable or malformed EBNF input
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// No repository, invalid repo state, boundary violations, etc.
    #[error("repository issue: {0}")]
    Repo(String),

    /// Merge conflicts or unapplyable hunks
    #[error("conflicts: {0}")]
    Conflicts(String),

    /// Internal engine failures or unexpected bugs
    #[error("internal error: {0}")]
    Internal(String),
}

/// Typed error for apply operations with structured conflict details
#[derive(Debug)]
pub enum ApplyErr {
    InvalidSpec(String),
    RepoIssue(String),
    Conflicts { details: Vec<String> },
    Internal(anyhow::Error),
}

impl std::fmt::Display for ApplyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplyErr::InvalidSpec(msg) => write!(f, "Invalid specification: {}", msg),
            ApplyErr::RepoIssue(msg) => write!(f, "Repository issue: {}", msg),
            ApplyErr::Conflicts { details } => {
                write!(f, "Conflicts detected ({})", details.len())?;
                for detail in details {
                    write!(f, "\n  • {}", detail)?;
                }
                Ok(())
            }
            ApplyErr::Internal(e) => write!(f, "Internal error: {:#}", e),
        }
    }
}

impl std::error::Error for ApplyErr {}

impl From<ApplyErr> for ApplyCliError {
    fn from(a: ApplyErr) -> Self {
        match a {
            ApplyErr::InvalidSpec(m) => ApplyCliError::InvalidInput(m),
            ApplyErr::RepoIssue(m) => ApplyCliError::Repo(m),
            ApplyErr::Conflicts { details } => ApplyCliError::Conflicts(details.join("\n")),
            ApplyErr::Internal(e) => ApplyCliError::Internal(format!("{:#}", e)),
        }
    }
}

/// Explicit run-mode computed from flags
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum RunMode {
    Preview,
    Apply,
}

/// Converts errors to the Phase-2 exit codes
/// 0=success, 2=conflict, 3=invalid, 4=repo, 5=internal
pub fn exit_code_for(e: &ApplyCliError) -> i32 {
    match e {
        ApplyCliError::InvalidInput(_) => 3,
        ApplyCliError::Repo(_) => 4,
        ApplyCliError::Conflicts(_) => 2,
        ApplyCliError::Internal(_) => 5,
    }
}

/// Exit code mapping for typed ApplyErr
pub fn exit_code_for_typed(e: &ApplyErr) -> i32 {
    match e {
        ApplyErr::InvalidSpec(_) => 3,
        ApplyErr::RepoIssue(_) => 4,
        ApplyErr::Conflicts { .. } => 2,
        ApplyErr::Internal(_) => 5,
    }
}

/// Discover the git repo root with multiple fallback strategies
/// Returns Ok(None) when no repo is found. Callers must decide
/// whether None is acceptable based on engine choice.
pub fn discover_repo_root(explicit: Option<PathBuf>, start: &Path) -> Result<Option<PathBuf>> {
    // 1) explicit override wins (canonicalize for stable prefix math)
    if let Some(root) = explicit {
        return Ok(Some(root.canonicalize().unwrap_or(root)));
    }

    // 2) git rev-parse (worktree top-level), canonicalized
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        && output.status.success()
    {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !s.is_empty() {
            let p = PathBuf::from(s);
            return Ok(Some(p.canonicalize().unwrap_or(p)));
        }
    }

    // 3) ascend to find .git (directory or worktree file), canonicalized
    let mut cur = Some(start);
    while let Some(dir) = cur {
        let git_path = dir.join(".git");
        if git_path.exists() && (git_path.is_dir() || git_path.is_file()) {
            let d = dir.to_path_buf();
            return Ok(Some(d.canonicalize().unwrap_or(d)));
        }
        cur = dir.parent();
    }

    Ok(None)
}

/// Map crate/engine errors to ApplyCliError
pub fn normalize_err(e: anyhow::Error) -> ApplyCliError {
    let msg = format!("{e:#}");

    // Simple string-match classification that can be refined later
    if msg.contains("conflict") || msg.contains("merge") {
        ApplyCliError::Conflicts(msg)
    } else if msg.contains("git") || msg.contains("repo") {
        ApplyCliError::Repo(msg)
    } else if msg.contains("EBNF") || msg.contains("syntax") || msg.contains("parse") {
        ApplyCliError::InvalidInput(msg)
    } else {
        ApplyCliError::Internal(msg)
    }
}

/// Enhanced error normalization with proper type classification
pub fn normalize_err_typed(e: anyhow::Error) -> (ApplyErr, i32) {
    // Parse error classification
    let msg = format!("{e:#}");

    // Check for specific error patterns first
    if let Some(cc) = e.downcast_ref::<crate::core::git::CombinedConflictError>() {
        let details = cc.internal_conflicts.clone();
        let err = ApplyErr::Conflicts { details };
        return (err, 2);
    }

    if msg.contains("Parse error") || msg.contains("EBNF") || msg.contains("syntax") {
        return (ApplyErr::InvalidSpec(msg), 3);
    }

    if msg.contains("repository") || msg.contains("git") || msg.contains("boundary") {
        return (ApplyErr::RepoIssue(msg), 4);
    }

    if msg.contains("conflict") || msg.contains("merge") || msg.contains("mismatch") {
        return (
            ApplyErr::Conflicts {
                details: vec![msg.clone()],
            },
            2,
        );
    }

    // Default to internal error
    let err = ApplyErr::Internal(e);
    (err, 5)
}

/// Core edit engine
#[derive(Default)]
pub struct EditEngine {
    preview_mode: bool,
    backup_enabled: bool,
    force_mode: bool,
}

impl EditEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_preview(mut self, enabled: bool) -> Self {
        self.preview_mode = enabled;
        self
    }

    pub fn with_backup(mut self, enabled: bool) -> Self {
        self.backup_enabled = enabled;
        self
    }

    pub fn with_force(mut self, enabled: bool) -> Self {
        self.force_mode = enabled;
        self
    }

    /// Parse edit specification from text
    pub fn parse_edit_spec(&self, input: &str) -> Result<EditSpec, ParseError> {
        // Normalize CRLF and allow leading BOM on first line
        let input = input.replace('\r', "");
        let mut file_blocks = Vec::new();
        let lines: Vec<&str> = input.lines().collect();
        let mut i = 0;

        while i < lines.len() {
            // Trim and strip BOM once if present
            let line = if i == 0 {
                lines[i].trim_start_matches('\u{FEFF}').trim()
            } else {
                lines[i].trim()
            };

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                i += 1;
                continue;
            }

            // Parse FILE block
            if line.starts_with("FILE:") {
                let path_str = line.strip_prefix("FILE:").unwrap().trim();
                if path_str.is_empty() {
                    return Err(ParseError::InvalidFileBlock("Empty file path".to_string()));
                }

                let path = PathBuf::from(path_str);
                i += 1;

                // Parse operations for this file
                let mut operations = Vec::new();
                while i < lines.len() {
                    let op_line = lines[i].trim();

                    // Break if next FILE begins
                    if op_line.starts_with("FILE:") {
                        break;
                    }

                    // Skip blanks and comments between operations
                    if op_line.is_empty() || op_line.starts_with('#') {
                        i += 1;
                        continue;
                    }

                    let before = i;
                    match self.parse_operation(&lines, &mut i)? {
                        Some(op) => operations.push(op),
                        None => {
                            // parse_operation advanced i itself; if not, advance by one to avoid a stall
                            if i == before {
                                i += 1;
                            }
                        }
                    }
                }

                file_blocks.push(FileBlock { path, operations });
            } else {
                i += 1;
            }
        }

        Ok(EditSpec { file_blocks })
    }

    /// Parse single operation starting at current line
    fn parse_operation(
        &self,
        lines: &[&str],
        i: &mut usize,
    ) -> Result<Option<EditOperation>, ParseError> {
        if *i >= lines.len() {
            return Ok(None);
        }

        let line = lines[*i].trim();

        // Check for GUARD-CID first
        let guard_cid = if line.starts_with("GUARD-CID:") {
            let cid: String = line.strip_prefix("GUARD-CID:").unwrap().trim().to_string();

            *i += 1;

            if *i >= lines.len() {
                return Err(ParseError::InvalidOperation(
                    "GUARD-CID without operation".to_string(),
                ));
            }

            Some(cid)
        } else {
            None
        };

        let op_line = lines[*i].trim();

        if op_line.starts_with("REPLACE lines") {
            self.parse_replace_operation(lines, i, guard_cid)
        } else if op_line.starts_with("INSERT at") {
            self.parse_insert_operation(lines, i)
        } else if op_line.starts_with("DELETE lines") {
            self.parse_delete_operation(lines, i)
        } else if !op_line.is_empty() {
            Err(ParseError::InvalidOperation(format!(
                "Unknown directive: {}",
                op_line
            )))
        } else {
            *i += 1;
            Ok(None)
        }
    }

    /// Parse REPLACE operation
    fn parse_replace_operation(
        &self,
        lines: &[&str],
        i: &mut usize,
        guard_cid: Option<ContentId>,
    ) -> Result<Option<EditOperation>, ParseError> {
        let op_line = lines[*i].trim();

        // Extract span from "REPLACE lines 10-15:"
        let span_part = op_line
            .strip_prefix("REPLACE lines")
            .and_then(|s| s.strip_suffix(":"))
            .ok_or_else(|| {
                ParseError::InvalidOperation(format!("Invalid REPLACE syntax: {}", op_line))
            })?
            .trim();

        let (start_line, end_line) = self.parse_span(span_part)?;
        *i += 1;

        // Local helpers so we don't rely on a strict fenced-only parser.
        fn is_marker(s: &str, needle: &str) -> bool {
            s.trim_start().starts_with(needle)
        }

        fn is_any_op_start(s: &str) -> bool {
            let t = s.trim_start();
            t.starts_with("FILE:")
                || t.starts_with("REPLACE lines")
                || t.starts_with("INSERT at")
                || t.starts_with("DELETE lines")
                || t.starts_with("GUARD-CID:")
                || t.starts_with("OLD:")
                || t.starts_with("NEW:")
        }
        fn strip_crlf(s: &str) -> String {
            s.replace('\r', "")
        }

        // Read a content block following the given marker.
        fn read_block(lines: &[&str], i: &mut usize, marker: &str) -> Result<String, ParseError> {
            if *i >= lines.len() || !is_marker(lines[*i], marker) {
                return Err(ParseError::InvalidOperation(format!(
                    "Expected {} at line {}",
                    marker,
                    *i + 1
                )));
            }
            // Consume the marker line.
            *i += 1;

            // Optional single blank line after marker.
            if *i < lines.len() && lines[*i].trim().is_empty() {
                *i += 1;
            }

            // EOF ⇒ empty block.
            if *i >= lines.len() {
                return Ok(String::new());
            }

            // Fenced?
            let mut body: Vec<String> = Vec::new();
            let t = lines[*i].trim_start();
            if t.starts_with("```") {
                // Consume opening fence.
                *i += 1;
                // Collect until closing fence.
                while *i < lines.len() {
                    let ln = lines[*i];
                    if ln.trim_start().starts_with("```") {
                        // Consume closing fence and stop.
                        *i += 1;
                        break;
                    } else {
                        body.push(ln.to_string());
                        *i += 1;
                    }
                }
                // If EOF without closing fence, we still accept what we have.
                return Ok(strip_crlf(&body.join("\n")));
            }

            // Unfenced:
            // OLD: stops at the next NEW:; NEW: stops at the next op/file marker.
            while *i < lines.len() {
                let ln = lines[*i];
                let t = ln.trim_start();

                if marker == "OLD:" && t.starts_with("NEW:") {
                    break;
                }
                if marker == "NEW:" && is_any_op_start(ln) {
                    break;
                }

                body.push(ln.to_string());
                *i += 1;
            }

            Ok(strip_crlf(&body.join("\n")))
        }

        let old_content = read_block(lines, i, "OLD:")?;
        let new_content = read_block(lines, i, "NEW:")?;

        Ok(Some(EditOperation::Replace {
            start_line,
            end_line,
            old_content,
            new_content,
            guard_cid,
        }))
    }

    /// Parse INSERT operation
    fn parse_insert_operation(
        &self,
        lines: &[&str],
        i: &mut usize,
    ) -> Result<Option<EditOperation>, ParseError> {
        let op_line = lines[*i].trim();

        // Extract line from "INSERT at 10:"
        let line_part = op_line
            .strip_prefix("INSERT at")
            .and_then(|s| s.strip_suffix(":"))
            .ok_or_else(|| {
                ParseError::InvalidOperation(format!("Invalid INSERT syntax: {}", op_line))
            })?
            .trim();

        let at_line = line_part
            .parse::<usize>()
            .map_err(|_| ParseError::InvalidLineNumber(line_part.to_string()))?;
        *i += 1;

        // Parse NEW block
        let new_content = self.parse_content_block(lines, i, "NEW:")?;

        Ok(Some(EditOperation::Insert {
            at_line,
            new_content,
        }))
    }

    /// Parse DELETE operation
    fn parse_delete_operation(
        &self,
        lines: &[&str],
        i: &mut usize,
    ) -> Result<Option<EditOperation>, ParseError> {
        let op_line = lines[*i].trim();

        // Extract span from "DELETE lines 10-15"
        let span_part = op_line
            .strip_prefix("DELETE lines")
            .ok_or_else(|| {
                ParseError::InvalidOperation(format!("Invalid DELETE syntax: {}", op_line))
            })?
            .trim();

        let (start_line, end_line) = self.parse_span(span_part)?;
        *i += 1;

        Ok(Some(EditOperation::Delete {
            start_line,
            end_line,
        }))
    }

    /// Parse line span "10-15" or single line "10"
    fn parse_span(&self, span_str: &str) -> Result<(usize, usize), ParseError> {
        if span_str.contains('-') {
            let parts: Vec<&str> = span_str.split('-').collect();
            if parts.len() != 2 {
                return Err(ParseError::InvalidSpan(span_str.to_string()));
            }

            let start = parts[0]
                .trim()
                .parse::<usize>()
                .map_err(|_| ParseError::InvalidLineNumber(parts[0].to_string()))?;
            let end = parts[1]
                .trim()
                .parse::<usize>()
                .map_err(|_| ParseError::InvalidLineNumber(parts[1].to_string()))?;

            if start == 0 || end == 0 || start > end {
                return Err(ParseError::InvalidSpan(format!(
                    "Invalid range: {}-{}",
                    start, end
                )));
            }

            Ok((start, end))
        } else {
            let line = span_str
                .trim()
                .parse::<usize>()
                .map_err(|_| ParseError::InvalidLineNumber(span_str.to_string()))?;

            if line == 0 {
                return Err(ParseError::InvalidLineNumber(
                    "Line numbers are 1-based".to_string(),
                ));
            }

            Ok((line, line))
        }
    }

    /// Parse content block after `OLD:` or `NEW:`.
    /// Accepts:
    ///   - optional blank line after the marker
    ///   - fenced body (``` or ```lang … ```), or
    ///   - unfenced body (terminated by the next marker)
    fn parse_content_block(
        &self,
        lines: &[&str],
        i: &mut usize,
        header: &str,
    ) -> Result<String, ParseError> {
        // Expect the header at the current line.
        if *i >= lines.len() || !lines[*i].trim().starts_with(header) {
            return Err(ParseError::MissingField(header.to_string()));
        }
        // Consume the header line.
        *i += 1;

        // Optional single blank line after the header.
        if *i < lines.len() && lines[*i].trim().is_empty() {
            *i += 1;
        }

        // EOF ⇒ empty block.
        if *i >= lines.len() {
            return Ok(String::new());
        }

        // Helper to detect the start of another directive/file block.
        fn is_any_op_start(s: &str) -> bool {
            let t = s.trim_start();
            t.starts_with("FILE:")
                || t.starts_with("REPLACE lines")
                || t.starts_with("INSERT at")
                || t.starts_with("DELETE lines")
                || t.starts_with("GUARD-CID:")
                || t.starts_with("OLD:")
                || t.starts_with("NEW:")
        }

        // If next line is a fence, read fenced body.
        let next_trim = lines[*i].trim_start();
        if next_trim.starts_with("```") {
            // Opening fence; allow language tag.
            let fence_line = lines[*i].trim();
            let fence_len = fence_line.chars().take_while(|&c| c == '`').count();
            let closing = "`".repeat(fence_len);
            *i += 1;

            let mut content_lines = Vec::new();
            while *i < lines.len() {
                let ln = lines[*i];
                let t = ln.trim_start();
                // Close on a line that begins with the same number of backticks.
                if t.starts_with(&closing) && t.chars().all(|c| c == '`' || c.is_whitespace()) {
                    *i += 1; // consume closing fence
                    break;
                }
                content_lines.push(ln.to_string());
                *i += 1;
            }
            return Ok(content_lines.join("\n").replace('\r', ""));
        }

        // Otherwise, read an unfenced body to the next marker.
        let mut body = Vec::new();
        while *i < lines.len() {
            let ln = lines[*i];
            let t = ln.trim_start();
            if header == "OLD:" && t.starts_with("NEW:") {
                break; // OLD ends where NEW begins
            }
            if header == "NEW:" && is_any_op_start(ln) {
                break; // NEW ends at the next directive or FILE
            }
            body.push(ln.to_string());
            *i += 1;
        }
        Ok(body.join("\n").replace('\r', ""))
    }

    /// Apply edit specification
    pub fn apply(&self, spec: &EditSpec) -> Result<EditResult> {
        let mut applied_files = Vec::new();
        let mut conflicts = Vec::new();
        let mut backup_paths = Vec::new();

        // First pass: validate all operations
        for file_block in &spec.file_blocks {
            if !file_block.path.exists() {
                conflicts.push(EditConflict::FileNotFound(file_block.path.clone()));
                continue;
            }

            // Load file content for validation
            let content = fs::read_to_string(&file_block.path)
                .with_context(|| format!("Failed to read file: {:?}", file_block.path))?;
            let file_lines: Vec<&str> = content.lines().collect();

            // Validate each operation
            for op in &file_block.operations {
                match self.validate_operation(op, &file_lines, &file_block.path) {
                    Ok(()) => {}
                    Err(conflict) => {
                        conflicts.push(conflict);
                    }
                }
            }
        }

        // Stop if conflicts found and not in force mode
        if !conflicts.is_empty() && !self.force_mode {
            return Ok(EditResult {
                applied_files,
                conflicts,
                backup_paths,
            });
        }

        // Preview mode: just show what would be done
        if self.preview_mode {
            // TODO: Generate and display unified diff
            return Ok(EditResult {
                applied_files,
                conflicts,
                backup_paths,
            });
        }

        // Apply operations to each file
        for file_block in &spec.file_blocks {
            if conflicts.iter().any(|c| match c {
                EditConflict::FileNotFound(path) => path == &file_block.path,
                _ => false,
            }) {
                continue; // Skip files that don't exist
            }

            // Create backup if requested
            if self.backup_enabled {
                let backup_path = self.create_backup(&file_block.path)?;
                backup_paths.push(backup_path);
            }

            // Apply operations to this file
            self.apply_file_operations(&file_block.path, &file_block.operations)?;
            applied_files.push(file_block.path.clone());
        }

        Ok(EditResult {
            applied_files,
            conflicts,
            backup_paths,
        })
    }

    /// Validate single operation against file content
    fn validate_operation(
        &self,
        op: &EditOperation,
        file_lines: &[&str],
        file_path: &Path,
    ) -> Result<(), EditConflict> {
        match op {
            EditOperation::Replace {
                start_line,
                end_line,
                old_content,
                guard_cid,
                ..
            } => {
                // Check span bounds
                if *start_line == 0
                    || *end_line == 0
                    || *start_line > file_lines.len()
                    || *end_line > file_lines.len()
                {
                    return Err(EditConflict::SpanOutOfRange {
                        file: file_path.to_path_buf(),
                        span: (*start_line, *end_line),
                        file_lines: file_lines.len(),
                    });
                }

                // Extract actual content in span (convert to 0-based indexing)
                let actual_lines = &file_lines[(*start_line - 1)..*end_line];
                let actual_content = actual_lines.join("\n");

                // Check GUARD-CID if present; else fall back to OLD content compare
                if let Some(expected_cid) = guard_cid {
                    // Compute once and compare once
                    let actual_cid = generate_cid(&actual_content);
                    if expected_cid != &actual_cid {
                        return Err(EditConflict::ContentMismatch {
                            file: file_path.to_path_buf(),
                            expected_cid: expected_cid.clone(),
                            actual_cid,
                        });
                    }
                } else {
                    // No guard: normalize and compare the OLD payload
                    if normalize_for_cid(old_content) != normalize_for_cid(&actual_content) {
                        return Err(EditConflict::OldContentMismatch {
                            file: file_path.to_path_buf(),
                            span: (*start_line, *end_line),
                        });
                    }
                }
            }
            EditOperation::Insert { at_line, .. } => {
                // Check line bounds (0 is valid for insert at beginning)
                if *at_line > file_lines.len() {
                    return Err(EditConflict::SpanOutOfRange {
                        file: file_path.to_path_buf(),
                        span: (*at_line, *at_line),
                        file_lines: file_lines.len(),
                    });
                }
            }
            EditOperation::Delete {
                start_line,
                end_line,
            } => {
                // Check span bounds
                if *start_line == 0
                    || *end_line == 0
                    || *start_line > file_lines.len()
                    || *end_line > file_lines.len()
                {
                    return Err(EditConflict::SpanOutOfRange {
                        file: file_path.to_path_buf(),
                        span: (*start_line, *end_line),
                        file_lines: file_lines.len(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Create backup file with timestamp, preserving original extension
    fn create_backup(&self, file_path: &Path) -> Result<PathBuf> {
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs();

        let backup_name = {
            let orig = file_path
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("Invalid file name"))?;
            let stem = Path::new(orig).file_stem().unwrap_or(orig);
            let ext = Path::new(orig).extension();
            match ext {
                Some(e) => format!(
                    "{}.rup.bak.{}.{}",
                    stem.to_string_lossy(),
                    ts,
                    e.to_string_lossy()
                ),
                None => format!("{}.rup.bak.{}", stem.to_string_lossy(), ts),
            }
        };

        let backup_path = file_path.with_file_name(backup_name);

        fs::copy(file_path, &backup_path)
            .with_context(|| format!("Failed to create backup: {:?}", backup_path))?;

        Ok(backup_path)
    }

    /// Apply operations to a single file
    fn apply_file_operations(&self, file_path: &Path, operations: &[EditOperation]) -> Result<()> {
        // Load file content
        let content = fs::read_to_string(file_path)
            .with_context(|| format!("Failed to read file: {:?}", file_path))?;

        // Detect original newline style and EOF newline presence
        fn detect_nl(s: &str) -> (&'static str, bool) {
            for w in s.as_bytes().windows(2) {
                if w[1] == b'\n' {
                    return (
                        if w[0] == b'\r' { "\r\n" } else { "\n" },
                        s.ends_with('\n') || s.ends_with("\r\n"),
                    );
                }
            }
            ("\n", s.ends_with('\n') || s.ends_with("\r\n"))
        }
        let (nl, had_final_nl) = detect_nl(&content);
        let use_crlf = nl == "\r\n";

        // Build mutable lines without trailing '\r'
        let mut file_lines: Vec<String> = content
            .lines()
            .map(|s| s.trim_end_matches('\r').to_string())
            .collect();

        // Check for overlapping operations
        let mut ranges = Vec::new();
        for op in operations {
            match op {
                EditOperation::Replace {
                    start_line,
                    end_line,
                    ..
                }
                | EditOperation::Delete {
                    start_line,
                    end_line,
                } => {
                    ranges.push((*start_line, *end_line));
                }
                _ => {}
            }
        }

        // Sort and detect overlaps between range ops
        ranges.sort_by_key(|(s, e)| (*s, *e));
        let mut problems: Vec<String> = Vec::new();

        for w in ranges.windows(2) {
            let (a_s, a_e) = w[0];
            let (b_s, b_e) = w[1];
            if b_s <= a_e {
                problems.push(format!(
                    "Overlapping edits: {}-{} with {}-{}",
                    a_s, a_e, b_s, b_e
                ));
            }
        }

        // INSERTs inside any range
        for op in operations {
            if let EditOperation::Insert { at_line, .. } = op {
                for (s, e) in &ranges {
                    if *at_line >= *s && *at_line <= *e {
                        problems.push(format!(
                            "Insert at {} overlaps with edit span {}-{}",
                            at_line, s, e
                        ));
                    }
                }
            }
        }

        if !problems.is_empty() {
            return Err(anyhow::anyhow!(
                "Operation overlaps: {}",
                problems.join("; ")
            ));
        }

        // Stable sort with tie-breakers
        let mut sorted_ops = operations.to_vec();
        sorted_ops.sort_by(|a, b| {
            let key = |op: &EditOperation| -> (usize, u8, usize) {
                match op {
                    EditOperation::Delete {
                        start_line,
                        end_line,
                    } => (*start_line, 0, *end_line),
                    EditOperation::Replace {
                        start_line,
                        end_line,
                        ..
                    } => (*start_line, 1, *end_line),
                    EditOperation::Insert { at_line, .. } => (*at_line, 2, *at_line),
                }
            };
            let (as_, ak, ae) = key(a);
            let (bs_, bk, be) = key(b);
            // Desc by start, then by kind, then by end desc
            bs_.cmp(&as_).then(ak.cmp(&bk)).then(be.cmp(&ae))
        });

        // Apply operations
        // Apply operations with token-aware relocation
        let matcher = TokenMatcher::new().ok(); // optional; None => line mode only

        for op in sorted_ops {
            match op {
                EditOperation::Replace {
                    start_line,
                    end_line,
                    old_content,
                    new_content,
                    ..
                } => {
                    // Try to re-locate by tokens first (exact token subsequence),
                    // then fall back to original line span.
                    let (s, e) = if let Some(m) = &matcher {
                        m.locate_exact(&file_lines, &old_content)
                            .or(Some((start_line, end_line)))
                    } else {
                        Some((start_line, end_line))
                    }
                    .unwrap();

                    let start_idx = s.saturating_sub(1);
                    let end_idx = e;

                    let new_lines: Vec<String> =
                        new_content.lines().map(|s| s.to_string()).collect();
                    file_lines.splice(start_idx..end_idx, new_lines);
                }
                EditOperation::Insert {
                    at_line,
                    new_content,
                } => {
                    // Insert after re-located token match of the previous line if possible.
                    // For v1 we keep the given at_line to avoid guessy behavior.
                    let insert_idx = at_line;
                    let new_lines: Vec<String> =
                        new_content.lines().map(|s| s.to_string()).collect();
                    for (i, line) in new_lines.into_iter().enumerate() {
                        file_lines.insert(insert_idx + i, line);
                    }
                }
                EditOperation::Delete {
                    start_line,
                    end_line,
                } => {
                    // For deletes, stick to provided span in v1 (token delete can come next)
                    let start_idx = start_line - 1;
                    let end_idx = end_line;
                    file_lines.drain(start_idx..end_idx);
                }
            }
        }

        // Reassemble with original newline style
        let nl = if use_crlf { "\r\n" } else { "\n" };
        let mut updated_content = file_lines.join(nl);
        if had_final_nl {
            updated_content.push_str(nl);
        }

        // Atomic write with robust temp file strategy
        write_atomic(file_path, updated_content.as_bytes())?;

        Ok(())
    }
}

/// Command handlers for CLI integration
/// Apply edit specification with unified preview/apply flow using ApplyEngine trait
pub fn apply_run(args: ApplyArgs, ctx: &AppContext) -> Result<()> {
    // 1) Parse input (file or clipboard)
    let ebnf = if let Some(file_path) = &args.edit_file {
        fs::read_to_string(file_path)
            .with_context(|| format!("Failed to read edit file: {:?}", file_path))?
    } else if args.from_clipboard {
        get_clipboard_content()?
    } else {
        return Err(ApplyCliError::InvalidInput(
            "Must specify either --edit-file or --from-clipboard".to_string(),
        )
        .into());
    };

    // 2) Build edit specification
    let legacy_engine = EditEngine::new();
    let input = normalize_edit_spec_text(&ebnf);

    let spec = legacy_engine
        .parse_edit_spec(&input)
        .map_err(|e| ApplyCliError::InvalidInput(format!("Parse error: {}", e)))?;

    // 3) Decide run mode: safe default is preview unless --apply was passed
    let run_mode = if args.apply {
        RunMode::Apply
    } else {
        if !ctx.quiet && !args.preview {
            eprintln!("Safety mode: showing preview only. Use --apply to write changes.");
        }
        RunMode::Preview
    };

    // 4) Detect repo root (auto-detect with optional override)
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let repo_root = discover_repo_root(args.repo_root.clone(), &cwd)
        .context("Failed to detect repository root")?;

    // 5) Create engine via factory with auto-fallback support
    let engine: Box<dyn crate::core::apply_engine::ApplyEngine> =
        match (&args.engine, repo_root.clone()) {
            (crate::cli::ApplyEngine::Git, None) => {
                return Err(ApplyCliError::Repo(
                    "Git engine requires a repository. Use --engine=internal or init a repo."
                        .to_string(),
                )
                .into());
            }
            (crate::cli::ApplyEngine::Auto, None) => {
                // Degrade gracefully to internal-only auto
                if !ctx.quiet {
                    eprintln!("No git repository found, using internal engine for --engine=auto");
                }
                create_engine(
                    &crate::cli::ApplyEngine::Internal,
                    &args.git_mode,
                    &args.whitespace,
                    args.backup,
                    args.force,
                    cwd.clone(),
                    args.context_lines,
                )
                .map_err(|e| ApplyCliError::Internal(format!("Engine creation failed: {}", e)))?
            }
            _ => create_engine(
                &args.engine,
                &args.git_mode,
                &args.whitespace,
                args.backup,
                args.force,
                repo_root.clone().unwrap_or_else(|| cwd.clone()),
                args.context_lines,
            )
            .map_err(|e| ApplyCliError::Internal(format!("Engine creation failed: {}", e)))?,
        };

    // 6) Always check() first for consistent preview
    let preview = engine.check(&spec).map_err(|e| {
        let (kind, _code) = normalize_err_typed(e);
        ApplyCliError::from(kind)
    })?;

    // 7) Render preview (unified diff) unless --quiet
    if !ctx.quiet {
        if !preview.patch_content.is_empty() {
            println!("{}", preview.patch_content);
        }
        if args.verbose {
            println!("{}", preview.summary);
        }
    }

    // 8) Check for conflicts and exit if in preview mode
    if !preview.conflicts.is_empty() {
        if !ctx.quiet {
            eprintln!("Found {} conflicts:", preview.conflicts.len());
            for conflict in &preview.conflicts {
                eprintln!("  • {}", conflict);
            }
            eprintln!("Suggestion: Use --engine=git --mode=3way for robust conflict resolution");
        }

        if !args.force {
            return Err(ApplyCliError::Conflicts(format!(
                "{} conflicts detected. Use --force to apply despite conflicts.",
                preview.conflicts.len()
            ))
            .into());
        }
    }

    // 9) Stop here if Preview mode
    if run_mode == RunMode::Preview {
        return Ok(());
    }

    // 10) Apply for real - set up backup session if enabled
    let report = if args.backup {
        // Create backup manager and use contextual API
        let mut backup_manager = crate::core::backup::BackupManager::begin(
            repo_root.as_ref().unwrap_or(&cwd),
            match args.engine {
                crate::cli::ApplyEngine::Internal => "internal",
                crate::cli::ApplyEngine::Git => "git",
                crate::cli::ApplyEngine::Auto => "auto",
            },
        )
        .map_err(|e| ApplyCliError::Internal(format!("Backup setup failed: {}", e)))?;

        let apply_ctx = crate::core::apply_engine::ApplyContext {
            repo_root: repo_root.as_ref().unwrap_or(&cwd),
            backup: Some(&mut backup_manager),
            whitespace: args.whitespace,
            context_lines: args.context_lines,
            force: args.force,
        };

        engine.apply_with_ctx(&spec, apply_ctx).map_err(|e| {
            let (kind, _code) = normalize_err_typed(e);
            ApplyCliError::from(kind)
        })?
    } else {
        // No backup - use legacy API
        engine.apply(&spec).map_err(|e| {
            let (kind, _code) = normalize_err_typed(e);
            ApplyCliError::from(kind)
        })?
    };

    // 11) Report results with session-based backup info
    if args.json {
        // JSON output (single line for machine parsing)
        let json_output = serde_json::to_string(&report)
            .map_err(|e| ApplyCliError::Internal(format!("JSON serialization failed: {}", e)))?;
        println!("{}", json_output);
    } else if !ctx.quiet {
        // Human-friendly output
        if !report.applied_files.is_empty() {
            println!(
                "Applied {} files with engine={:?}",
                report.applied_files.len(),
                report.engine_used
            );
            if args.verbose {
                for file in &report.applied_files {
                    println!("  • {}", file.display());
                }
            }
        }

        // Show session-based backup info
        if let Some(_session_id) = &report.backup_session_id
            && let Some(session_dir) = report.backup_paths.first()
        {
            println!("Backups: {}", session_dir.display());

            if let Some(manifest_path) = &report.backup_manifest_path {
                println!("Manifest: {}", manifest_path.display());
            }
        }
    }

    Ok(())
}

struct TokenMatcher {
    bpe: tiktoken_rs::CoreBPE,
}

impl TokenMatcher {
    fn new() -> anyhow::Result<Self> {
        // Reuse your existing encodings; prefer o200k_base for breadth
        let bpe = tiktoken_rs::o200k_base().context("token matcher bpe")?;
        Ok(Self { bpe })
    }

    /// Find the first exact token-subsequence match of `needle_text` in `file_lines`.
    /// Returns 1-based (start_line, end_line) on success.
    fn locate_exact(&self, file_lines: &[String], needle_text: &str) -> Option<(usize, usize)> {
        let hay = file_lines.join("\n");
        let norm = normalize_for_cid(needle_text);
        let hay_ids = self.bpe.encode_ordinary(&normalize_for_cid(&hay));
        let nee_ids = self.bpe.encode_ordinary(&norm);
        if nee_ids.is_empty() || hay_ids.len() < nee_ids.len() {
            return None;
        }
        // Rabin-Karp style scan on token IDs (simple slice compare for v1)
        // (Could be optimized with rolling hash later.)
        for i in 0..=(hay_ids.len() - nee_ids.len()) {
            if &hay_ids[i..i + nee_ids.len()] == nee_ids.as_slice() {
                // Map back to line numbers by counting '\n' in the byte space of the match.
                // To avoid re-tokenizing to bytes, slice on text using a cheap string search first.
                if let Some(byte_lo) = hay.find(&norm) {
                    let byte_hi = byte_lo + norm.len();
                    let start_line = hay[..byte_lo].chars().filter(|&c| c == '\n').count() + 1;
                    let end_line =
                        start_line + hay[byte_lo..byte_hi].chars().filter(|&c| c == '\n').count();
                    return Some((start_line, end_line.max(start_line)));
                }
                // Fallback: if normalized text find fails (rare), bail to None.
                return None;
            }
        }
        None
    }
}

/// Convert Result<()> to exit codes for CLI harness
/// Keep the mapping centralized for CI predictability
pub fn finish_with_exit(result: Result<()>) -> ! {
    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            // Prefer typed mapping; fall back to legacy
            let (typed, code) = normalize_err_typed(e);
            eprintln!("{}", typed);
            std::process::exit(code);
        }
    }
}

/// Preview edit changes without applying them using unified engine architecture
pub fn preview_run(args: PreviewArgs, ctx: &AppContext) -> Result<()> {
    let input = if args.from_clipboard {
        get_clipboard_content()?
    } else if let Some(file_path) = args.edit_file {
        fs::read_to_string(&file_path)
            .with_context(|| format!("Failed to read edit file: {:?}", file_path))?
    } else {
        anyhow::bail!("Must specify either --from-clipboard or provide edit file");
    };

    let legacy_engine = EditEngine::new();
    // Accept specs with fenced OLD/NEW and CRLF endings
    let input = normalize_edit_spec_text(&input);
    let spec = legacy_engine
        .parse_edit_spec(&input)
        .context("Failed to parse edit specification")?;

    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let repo_root = discover_repo_root(args.repo_root.clone(), &cwd)
        .context("Failed to detect repository root")?;

    // Use same engine logic as apply_run for consistency
    let engine: Box<dyn crate::core::apply_engine::ApplyEngine> =
        match (&args.engine, repo_root.clone()) {
            (crate::cli::ApplyEngine::Git, None) => {
                return Err(ApplyCliError::Repo(
                    "Git engine requires a repository. Use --engine=internal or init a repo."
                        .to_string(),
                )
                .into());
            }
            (crate::cli::ApplyEngine::Auto, None) => {
                if !ctx.quiet {
                    eprintln!("No git repository found, using internal engine for --engine=auto");
                }
                create_engine(
                    &crate::cli::ApplyEngine::Internal,
                    &args.git_mode,
                    &args.whitespace,
                    false, // backup
                    args.force,
                    cwd.clone(),
                    args.context_lines,
                )
                .map_err(|e| ApplyCliError::Internal(format!("Engine creation failed: {}", e)))?
            }
            _ => create_engine(
                &args.engine,
                &args.git_mode,
                &args.whitespace,
                false, // backup
                args.force,
                repo_root.unwrap_or_else(|| cwd.clone()),
                args.context_lines,
            )
            .map_err(|e| ApplyCliError::Internal(format!("Engine creation failed: {}", e)))?,
        };

    let preview = engine.check(&spec).map_err(|e| {
        let (kind, _code) = normalize_err_typed(e);
        ApplyCliError::from(kind)
    })?;

    if !ctx.quiet {
        if args.show_diff && !preview.patch_content.is_empty() {
            println!("{}", preview.patch_content);
        }
        println!("{}", preview.summary);
        if !preview.conflicts.is_empty() {
            eprintln!("Found {} conflicts:", preview.conflicts.len());
            for conflict in &preview.conflicts {
                eprintln!("  • {}", conflict);
            }
            eprintln!("Suggestion: Use --engine=git --mode=3way for robust conflict resolution");
        }
    }

    Ok(())
}

/// Validate edit syntax without applying changes
pub fn check_syntax_run(args: CheckSyntaxArgs, ctx: &AppContext) -> Result<()> {
    let input = fs::read_to_string(&args.edit_file)
        .with_context(|| format!("Failed to read edit file: {:?}", args.edit_file))?;

    let engine = EditEngine::new();

    match engine.parse_edit_spec(&input) {
        Ok(spec) => {
            if !ctx.quiet {
                println!("Edit syntax is valid");
                println!(
                    "   {} file blocks with {} total operations",
                    spec.file_blocks.len(),
                    spec.file_blocks
                        .iter()
                        .map(|fb| fb.operations.len())
                        .sum::<usize>()
                );
            }

            // Check if referenced files exist
            let mut missing_files = Vec::new();
            for file_block in &spec.file_blocks {
                if !file_block.path.exists() {
                    missing_files.push(&file_block.path);
                }
            }

            if !missing_files.is_empty() {
                println!("Referenced files not found:");
                for file in missing_files {
                    println!("   • {}", file.display());
                }
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Edit syntax error: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Backup management subcommands (Phase B2 - read-only for now)
pub fn backup_run(args: BackupArgs, ctx: &AppContext) -> Result<()> {
    // Use current working directory as repo root for backup store
    let repo_root = std::env::current_dir()?;

    match args.command {
        BackupSubcommand::List(list_args) => backup_list(&repo_root, &list_args, ctx),
        BackupSubcommand::Show(show_args) => backup_show(&repo_root, &show_args, ctx),
        BackupSubcommand::Restore(restore_args) => backup_restore(&repo_root, &restore_args, ctx),
        BackupSubcommand::Cleanup(cleanup_args) => backup_cleanup(&repo_root, &cleanup_args, ctx),
    }
}

fn backup_list(repo_root: &Path, a: &BackupListArgs, ctx: &AppContext) -> Result<()> {
    let sort_desc = !a.sort.eq_ignore_ascii_case("asc");
    let req = ListRequest {
        successful: a.successful,
        engine: a.engine.clone(),
        since: a.since.clone(),
        limit: a.limit,
        sort_desc,
    };

    let sessions = list_sessions_filtered(repo_root, req)?;

    if a.json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }

    if sessions.is_empty() {
        if !ctx.quiet {
            println!("No backup sessions found.");
        }
        return Ok(());
    }

    for s in sessions {
        print_session_line(&s);
    }
    Ok(())
}

fn print_session_line(s: &SessionInfo) {
    let status = if s.success { "success" } else { "failed" };
    let samples = if s.sample_paths.is_empty() {
        String::new()
    } else {
        format!("  [{}]", s.sample_paths.join(", "))
    };
    println!(
        "{timestamp:<19} {id:<12} {engine:<10} files={files:>4} {status:<7}{samples}",
        timestamp = s.timestamp,
        id = s.id,
        engine = s.engine,
        files = s.files,
        status = status,
        samples = samples
    );
}

fn backup_show(repo_root: &Path, a: &BackupShowArgs, ctx: &AppContext) -> Result<()> {
    let resp = show_session(
        repo_root,
        ShowRequest {
            id: a.id.clone(),
            verbose: a.verbose,
        },
    )?;

    if a.json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return Ok(());
    }

    let m = &resp.manifest;
    println!("id: {}", m.id);
    println!("timestamp: {}", m.timestamp);
    println!("engine: {}", m.engine);
    println!("success: {}", m.success);
    println!("files: {}", m.files.len());
    println!("session_path: {}", resp.session_path.display());
    if let Some(sz) = resp.total_size {
        println!("payload_size_bytes: {}", sz);
    }

    if a.verbose {
        println!("file entries:");
        for f in &m.files {
            println!("  - {} ({} bytes)", f.rel_path.display(), f.size_bytes);
        }
    } else if !ctx.quiet {
        // show up to 3 samples for quick glance
        for f in m.files.iter().take(3) {
            println!("  - {}", f.rel_path.display());
        }
        if m.files.len() > 3 {
            println!("  … and {} more", m.files.len() - 3);
        }
    }

    Ok(())
}

fn backup_restore(repo_root: &Path, a: &BackupRestoreArgs, ctx: &AppContext) -> Result<()> {
    let is_dry_run = a.dry_run || ctx.dry_run; // Honor global dry-run flag
    let req = RestoreRequest {
        session_id: a.session.clone(),
        path: a.path.clone(),
        dry_run: is_dry_run,
        force: a.force,
        show_diff: a.show_diff,
        verify_checksum: a.verify_checksum,
        backup_current: a.backup_current,
    };

    let result = restore_session(repo_root, req)?;

    if a.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    // Human-friendly output
    if !ctx.quiet {
        println!("Session: {}", result.session_id);
    }

    if is_dry_run && !ctx.quiet {
        println!("DRY RUN - no files were modified");
    }

    // Show conflicts
    if !result.conflicts.is_empty() {
        if !ctx.quiet {
            println!("Conflicts detected in {} file(s):", result.conflicts.len());
            for conflict_path in &result.conflicts {
                println!("  - {}", conflict_path.display());
            }
        }

        // Show diffs if requested and available
        if let Some(ref diffs) = result.diffs {
            for diff in diffs {
                println!("\nDiff for {}:", diff.path.display());
                println!("{}", diff.unified);
            }
        }

        if !is_dry_run {
            println!(
                "\nUse --force to overwrite conflicting files, or --dry-run to preview changes."
            );
        }
        return Ok(());
    }

    // Show restored files
    if !result.restored.is_empty() && !ctx.quiet {
        println!("Restored {} file(s):", result.restored.len());
        for restored_path in &result.restored {
            println!("  - {}", restored_path.display());
        }
    }

    // Show backup information if current files were backed up
    if result.backed_up_current
        && let Some(ref backup_session) = result.backup_session_id
        && !ctx.quiet
    {
        println!("Current files backed up to session: {}", backup_session);
    }

    Ok(())
}

fn backup_cleanup(repo_root: &Path, a: &BackupCleanupArgs, ctx: &AppContext) -> Result<()> {
    let is_dry_run = a.dry_run || ctx.dry_run; // Honor global dry-run flag
    let req = CleanupRequest {
        older_than: a.older_than.clone(),
        keep_latest: a.keep_latest,
        include_incomplete: a.include_incomplete,
        dry_run: is_dry_run,
    };

    let result = cleanup_sessions(repo_root, req)?;

    if a.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    // Human-friendly output
    if is_dry_run && !ctx.quiet {
        println!("DRY RUN - no sessions were deleted");
    }

    if result.sessions_removed.is_empty() {
        if !ctx.quiet {
            println!("No sessions matched cleanup criteria");
        }
        return Ok(());
    }

    if !ctx.quiet {
        let action = if is_dry_run {
            "Would remove"
        } else {
            "Removed"
        };
        println!("{} {} session(s):", action, result.sessions_removed.len());
        for session_id in &result.sessions_removed {
            println!("  - {}", session_id);
        }

        if result.bytes_freed > 0 {
            println!("Space freed: {} bytes", result.bytes_freed);
        }
    }

    Ok(())
}

/// Get content from system clipboard
fn get_clipboard_content() -> Result<String> {
    use arboard::Clipboard;
    let mut clipboard = Clipboard::new().context("Failed to access clipboard")?;
    clipboard
        .get_text()
        .context("Failed to get text from clipboard")
}

/// Atomic write with robust temp file strategy
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    // Prefer same-dir tempfile; fall back to OS temp on EPERM/ENOENT
    let dir = path.parent().unwrap_or_else(|| Path::new("."));

    // Preserve original permissions
    #[cfg(unix)]
    let perms = fs::metadata(path)
        .map(|m| m.permissions())
        .unwrap_or_else(|_| std::os::unix::fs::PermissionsExt::from_mode(0o644));
    #[cfg(not(unix))]
    let perms = fs::metadata(path).map(|m| m.permissions()).ok();

    let tmp = match tempfile::NamedTempFile::new_in(dir) {
        Ok(t) => t,
        Err(_) => tempfile::NamedTempFile::new()?, // fallback to /tmp
    };

    // Write the content fully
    use std::io::Write;
    let mut file = tmp.as_file();
    file.set_len(0)?;
    file.write_all(data)?;
    file.sync_all()?;

    // Apply permissions to the temp file (best effort)
    #[cfg(unix)]
    fs::set_permissions(tmp.path(), perms).context("set temp permissions")?;
    #[cfg(not(unix))]
    if let Some(perms) = perms {
        fs::set_permissions(tmp.path(), perms).context("set temp permissions")?;
    }

    // fsync parent dir to ensure durability on Unix
    #[cfg(unix)]
    {
        if let Ok(parent_file) = std::fs::File::open(dir) {
            let _ = parent_file.sync_all();
        }
    }

    // Atomically replace the destination
    match tmp.persist(path) {
        Ok(_) => {}
        Err(e) => {
            // Different filesystem? Try copy fallback
            std::fs::copy(e.file.path(), path)?;
        }
    }

    Ok(())
}

fn normalize_edit_spec_text(src: &str) -> String {
    // 1) Normalize line endings to LF
    let src = src.replace('\r', "");

    // 2) Strip ``` fences that immediately follow OLD:/NEW: markers
    // State machine over lines
    let mut out = String::with_capacity(src.len());
    #[derive(Copy, Clone, PartialEq)]
    enum Block {
        None,
        Old,
        New,
    }
    let mut blk = Block::None;
    let mut in_fence = false;

    for line in src.lines() {
        let t = line.trim_start();

        // Enter markers
        if t.eq_ignore_ascii_case("OLD:") {
            blk = Block::Old;
            in_fence = false;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.eq_ignore_ascii_case("NEW:") {
            blk = Block::New;
            in_fence = false;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Right after OLD:/NEW:, skip a leading fence if present
        if !in_fence && (blk == Block::Old || blk == Block::New) && t.starts_with("```") {
            // Begin fenced body; do not emit the fence line
            in_fence = true;
            continue;
        }

        // Inside a fenced OLD/NEW body: drop the closing fence line
        if in_fence && t.starts_with("```") {
            in_fence = false;
            // Keep block mode (still inside OLD/NEW) but do not emit fence
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_generate_cid() {
        let content1 = "fn test() {\n    println!(\"hello\");\n}";
        let content2 = "fn test() {\n    println!(\"hello\");\n}";
        let content3 = "fn test() {\n    println!(\"world\");\n}";

        assert_eq!(generate_cid(content1), generate_cid(content2));
        assert_ne!(generate_cid(content1), generate_cid(content3));
    }

    #[test]
    fn test_parse_span() {
        let engine = EditEngine::new();

        assert_eq!(engine.parse_span("10").unwrap(), (10, 10));
        assert_eq!(engine.parse_span("10-15").unwrap(), (10, 15));
        assert!(engine.parse_span("0").is_err());
        assert!(engine.parse_span("15-10").is_err());
    }

    #[test]
    fn test_parse_simple_replace() {
        let engine = EditEngine::new();
        let input = r#"
FILE: test.rs
REPLACE lines 1-2:
OLD:
```rust
fn old_function() {
    println!("old");
}
```
NEW:
```rust
fn new_function() {
    println!("new");
}
```
"#;

        let spec = engine.parse_edit_spec(input).unwrap();
        assert_eq!(spec.file_blocks.len(), 1);
        assert_eq!(spec.file_blocks[0].path, PathBuf::from("test.rs"));
        assert_eq!(spec.file_blocks[0].operations.len(), 1);

        match &spec.file_blocks[0].operations[0] {
            EditOperation::Replace {
                start_line,
                end_line,
                old_content,
                new_content,
                guard_cid,
            } => {
                assert_eq!(*start_line, 1);
                assert_eq!(*end_line, 2);
                assert!(old_content.contains("old_function"));
                assert!(new_content.contains("new_function"));
                assert!(guard_cid.is_none());
            }
            _ => panic!("Expected Replace operation"),
        }
    }

    #[test]
    fn test_create_and_apply_backup() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "original content").unwrap();
        let temp_path = temp_file.path().to_path_buf();

        let engine = EditEngine::new().with_backup(true);
        let backup_path = engine.create_backup(&temp_path).unwrap();

        assert!(backup_path.exists());
        let backup_content = fs::read_to_string(&backup_path).unwrap();
        assert_eq!(backup_content.trim(), "original content");

        // Cleanup
        fs::remove_file(backup_path).unwrap();
    }

    #[test]
    fn test_blank_lines_between_ops() {
        let engine = EditEngine::new();
        let input = r#"
FILE: test.rs
REPLACE lines 1:
OLD:
```rust
old line
```
NEW:
```rust
new line
```

INSERT at 2:
NEW:
```rust
inserted line
```
"#;

        let spec = engine.parse_edit_spec(input).unwrap();
        assert_eq!(spec.file_blocks.len(), 1);
        assert_eq!(spec.file_blocks[0].operations.len(), 2);
    }

    #[test]
    fn test_fence_run_robustness() {
        let engine = EditEngine::new();
        let input = r#"
FILE: test.rs
REPLACE lines 1:
OLD:
````rust
fn test() {
    // nested ```
}
````
NEW:
````rust
fn test() {
    // updated nested ```
}
````
"#;

        let spec = engine.parse_edit_spec(input).unwrap();
        let op = &spec.file_blocks[0].operations[0];
        match op {
            EditOperation::Replace {
                old_content,
                new_content,
                ..
            } => {
                assert!(old_content.contains("nested ```"));
                assert!(new_content.contains("updated nested ```"));
            }
            _ => panic!("Expected Replace operation"),
        }
    }

    #[test]
    fn test_crlf_preservation() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        // Create CRLF file with trailing newline
        let crlf_content = "line1\r\nline2\r\nline3\r\n";
        fs::write(&file_path, crlf_content).unwrap();

        let engine = EditEngine::new();
        let operations = vec![EditOperation::Replace {
            start_line: 2,
            end_line: 2,
            old_content: "line2".to_string(),
            new_content: "modified line2".to_string(),
            guard_cid: None,
        }];

        engine
            .apply_file_operations(&file_path, &operations)
            .unwrap();

        let result = fs::read_to_string(&file_path).unwrap();
        assert!(result.contains("\r\n"), "CRLF should be preserved");
        assert!(
            result.ends_with("\r\n"),
            "Final newline should be preserved"
        );
        assert!(result.contains("modified line2"));
    }

    #[test]
    fn test_deterministic_cid() {
        let content = "fn test() {\n    println!(\"hello\");\n}";
        let cid1 = generate_cid(content);
        let cid2 = generate_cid(content);
        assert_eq!(cid1, cid2, "CID should be deterministic");

        // Different content should have different CID
        let different_content = "fn test() {\n    println!(\"world\");\n}";
        let cid3 = generate_cid(different_content);
        assert_ne!(cid1, cid3, "Different content should have different CID");
    }

    #[test]
    fn test_unknown_directive_fails() {
        let engine = EditEngine::new();
        let input = r#"
FILE: test.rs
UPDATE lines 1-2:
OLD:
```rust
old code
```
NEW:
```rust
new code
```
"#;

        let result = engine.parse_edit_spec(input);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unknown directive: UPDATE")
        );
    }
}
