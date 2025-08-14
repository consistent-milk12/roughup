//! EBNF to unified diff patch converter
//!
//! Converts our human-readable edit format into standard Git patches
//! for robust application with context matching and 3-way merging.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::core::edit::{EditOperation, EditSpec, generate_cid, normalize_for_cid};

/// A single hunk in a unified diff
#[derive(Debug, Clone)]
pub struct Hunk {
    pub old_start: usize, // 1-based line number in old file
    pub old_count: usize, // Number of lines in old version
    pub new_start: usize, // 1-based line number in new file
    pub new_count: usize, // Number of lines in new version
    pub lines: Vec<HunkLine>,
}

/// A line in a hunk with its change type
#[derive(Debug, Clone)]
pub enum HunkLine {
    Context(String), // Unchanged line (starts with ' ')
    Remove(String),  // Removed line (starts with '-')
    Add(String),     // Added line (starts with '+')
}

/// A complete patch for one file
#[derive(Debug, Clone)]
pub struct FilePatch {
    pub path: String,
    pub hunks: Vec<Hunk>,
    pub metadata: PatchMetadata,
}

/// Patch metadata for traceability
#[derive(Debug, Clone)]
pub struct PatchMetadata {
    pub source_cid: Option<String>, // CID of source EBNF operation
    pub context_lines: usize,
    pub engine: String,
}

/// Complete patch set
#[derive(Debug, Clone)]
pub struct PatchSet {
    pub file_patches: Vec<FilePatch>,
}

/// Patch generation configuration
pub struct PatchConfig {
    pub context_lines: usize,
    pub validate_guards: bool,
    pub merge_adjacent: bool,
}

impl Default for PatchConfig {
    fn default() -> Self {
        Self {
            context_lines: 3,
            validate_guards: true,
            merge_adjacent: true,
        }
    }
}

/// Convert EBNF edit specification to unified diff patches
pub fn generate_patches(spec: &EditSpec, config: &PatchConfig) -> Result<PatchSet> {
    let mut file_patches = Vec::new();

    // Group operations by file
    let mut ops_by_file: BTreeMap<String, Vec<&EditOperation>> = BTreeMap::new();
    for file_block in &spec.file_blocks {
        let path_str = file_block.path.to_string_lossy().to_string();
        ops_by_file
            .entry(path_str)
            .or_default()
            .extend(&file_block.operations);
    }

    // Generate patch for each file
    for (path_str, operations) in ops_by_file {
        let file_patch = generate_file_patch(&path_str, operations, config)
            .with_context(|| format!("Failed to generate patch for {}", path_str))?;
        file_patches.push(file_patch);
    }

    Ok(PatchSet { file_patches })
}

/// Generate unified diff patch for a single file
fn generate_file_patch(
    path_str: &str,
    operations: Vec<&EditOperation>,
    config: &PatchConfig,
) -> Result<FilePatch> {
    let path = Path::new(path_str);

    // Read current file content
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read file: {}", path_str))?;
    let file_lines: Vec<&str> = content.lines().collect();

    // Convert operations to hunks
    let hunks = operations_to_hunks(&file_lines, &operations, config)?;

    // Merge adjacent/overlapping hunks if requested
    let merged_hunks = if config.merge_adjacent {
        merge_adjacent_hunks(hunks, config.context_lines)
    } else {
        hunks
    };

    // Sort hunks by line number
    let mut sorted_hunks = merged_hunks;
    sorted_hunks.sort_by_key(|h| h.old_start);

    // Generate metadata
    let source_cid = operations.first().and_then(|op| match op {
        EditOperation::Replace {
            guard_cid: Some(cid),
            ..
        } => Some(cid.clone()),
        _ => None,
    });

    let metadata = PatchMetadata {
        source_cid,
        context_lines: config.context_lines,
        engine: "rup".to_string(),
    };

    Ok(FilePatch {
        path: path_str.to_string(),
        hunks: sorted_hunks,
        metadata,
    })
}

/// Convert edit operations to hunks with context
fn operations_to_hunks(
    file_lines: &[&str],
    operations: &[&EditOperation],
    config: &PatchConfig,
) -> Result<Vec<Hunk>> {
    let mut hunks = Vec::new();

    for op in operations {
        let hunk = operation_to_hunk(file_lines, op, config)?;
        hunks.push(hunk);
    }

    Ok(hunks)
}

/// Convert a single operation to a hunk
fn operation_to_hunk(
    file_lines: &[&str],
    operation: &EditOperation,
    config: &PatchConfig,
) -> Result<Hunk> {
    match operation {
        EditOperation::Replace {
            start_line,
            end_line,
            old_content,
            new_content,
            guard_cid,
        } => {
            // Validate against file content if guard present
            if config.validate_guards {
                validate_operation_content(
                    file_lines,
                    *start_line,
                    *end_line,
                    old_content,
                    guard_cid,
                )?;
            }

            let old_start = *start_line;
            let old_end = *end_line;
            let _old_count = old_end - old_start + 1;

            // Parse new content into lines
            let new_lines: Vec<&str> = new_content.lines().collect();
            let new_count = new_lines.len();

            // Build hunk with context
            let context_start = old_start.saturating_sub(config.context_lines).max(1);
            let context_end = (old_end + config.context_lines).min(file_lines.len());

            let mut hunk_lines = Vec::new();

            // Add leading context
            for line_num in context_start..old_start {
                let line = file_lines[line_num - 1]; // Convert to 0-based
                hunk_lines.push(HunkLine::Context(line.to_string()));
            }

            // Add removed lines
            for line_num in old_start..=old_end {
                let line = file_lines[line_num - 1]; // Convert to 0-based
                hunk_lines.push(HunkLine::Remove(line.to_string()));
            }

            // Add new lines
            for new_line in &new_lines {
                hunk_lines.push(HunkLine::Add(new_line.to_string()));
            }

            // Add trailing context
            for line_num in (old_end + 1)..=context_end {
                if line_num <= file_lines.len() {
                    let line = file_lines[line_num - 1]; // Convert to 0-based
                    hunk_lines.push(HunkLine::Context(line.to_string()));
                }
            }

            // Calculate new start position (accounting for context)
            let new_start = context_start;
            let hunk_new_count = (context_start..old_start).count()
                + new_count
                + (old_end + 1..=context_end).count();

            Ok(Hunk {
                old_start: context_start,
                old_count: (context_end - context_start + 1)
                    .min(file_lines.len() - context_start + 1),
                new_start,
                new_count: hunk_new_count,
                lines: hunk_lines,
            })
        }
        EditOperation::Insert {
            at_line,
            new_content,
        } => {
            let insert_pos = *at_line; // 0 means beginning, N means after line N
            let new_lines: Vec<&str> = new_content.lines().collect();

            // Context around insertion point
            let context_start = insert_pos.saturating_sub(config.context_lines).max(1);
            let context_end = (insert_pos + config.context_lines).min(file_lines.len());

            let mut hunk_lines = Vec::new();

            // Add leading context
            for line_num in context_start..=insert_pos.min(file_lines.len()) {
                if line_num > 0 && line_num <= file_lines.len() {
                    let line = file_lines[line_num - 1];
                    hunk_lines.push(HunkLine::Context(line.to_string()));
                }
            }

            // Add new lines
            for new_line in &new_lines {
                hunk_lines.push(HunkLine::Add(new_line.to_string()));
            }

            // Add trailing context
            for line_num in (insert_pos + 1)..=context_end {
                if line_num <= file_lines.len() {
                    let line = file_lines[line_num - 1];
                    hunk_lines.push(HunkLine::Context(line.to_string()));
                }
            }

            Ok(Hunk {
                old_start: context_start,
                old_count: context_end - context_start + 1,
                new_start: context_start,
                new_count: (context_end - context_start + 1) + new_lines.len(),
                lines: hunk_lines,
            })
        }
        EditOperation::Delete {
            start_line,
            end_line,
        } => {
            let delete_count = end_line - start_line + 1;

            // Context around deletion
            let context_start = start_line.saturating_sub(config.context_lines).max(1);
            let context_end = (end_line + config.context_lines).min(file_lines.len());

            let mut hunk_lines = Vec::new();

            // Add leading context
            for line_num in context_start..*start_line {
                let line = file_lines[line_num - 1];
                hunk_lines.push(HunkLine::Context(line.to_string()));
            }

            // Add deleted lines
            for line_num in *start_line..=*end_line {
                let line = file_lines[line_num - 1];
                hunk_lines.push(HunkLine::Remove(line.to_string()));
            }

            // Add trailing context
            for line_num in (*end_line + 1)..=context_end {
                if line_num <= file_lines.len() {
                    let line = file_lines[line_num - 1];
                    hunk_lines.push(HunkLine::Context(line.to_string()));
                }
            }

            Ok(Hunk {
                old_start: context_start,
                old_count: context_end - context_start + 1,
                new_start: context_start,
                new_count: (context_end - context_start + 1) - delete_count,
                lines: hunk_lines,
            })
        }
    }
}

/// Validate operation content against file using same logic as EditEngine
fn validate_operation_content(
    file_lines: &[&str],
    start_line: usize,
    end_line: usize,
    old_content: &str,
    guard_cid: &Option<String>,
) -> Result<()> {
    // Extract actual content from file
    let actual_lines = &file_lines[(start_line - 1)..end_line]; // Convert to 0-based
    let actual_content = actual_lines.join("\n");

    // Use same validation logic as EditEngine
    if let Some(expected_cid) = guard_cid {
        let actual_cid = generate_cid(&actual_content);
        if expected_cid != &actual_cid {
            anyhow::bail!(
                "Content mismatch: expected CID {}, got {}",
                expected_cid,
                actual_cid
            );
        }
    } else {
        // Compare normalized content
        if normalize_for_cid(old_content) != normalize_for_cid(&actual_content) {
            anyhow::bail!("OLD content mismatch at lines {}-{}", start_line, end_line);
        }
    }

    Ok(())
}

/// Merge adjacent hunks to reduce patch complexity
fn merge_adjacent_hunks(hunks: Vec<Hunk>, context_lines: usize) -> Vec<Hunk> {
    if hunks.len() <= 1 {
        return hunks;
    }

    let mut merged = Vec::new();
    let mut current = hunks[0].clone();

    for next in hunks.into_iter().skip(1) {
        // Check if hunks are close enough to merge
        let current_end = current.old_start + current.old_count;
        let gap = next.old_start.saturating_sub(current_end);

        if gap <= context_lines * 2 {
            // Merge hunks
            current = merge_two_hunks(current, next);
        } else {
            // Too far apart, keep separate
            merged.push(current);
            current = next;
        }
    }

    merged.push(current);
    merged
}

/// Merge two adjacent hunks
fn merge_two_hunks(mut first: Hunk, second: Hunk) -> Hunk {
    // Extend first hunk to include second
    first.old_count = (second.old_start + second.old_count) - first.old_start;
    first.new_count = (second.new_start + second.new_count) - first.new_start;

    // Merge lines (simplified - in production you'd handle overlapping context)
    first.lines.extend(second.lines);

    first
}

/// Render patch set as unified diff string
pub fn render_unified_diff(patch_set: &PatchSet) -> String {
    let mut output = String::new();

    for file_patch in &patch_set.file_patches {
        render_file_patch(&mut output, file_patch);
    }

    output
}

/// Render a single file patch
fn render_file_patch(output: &mut String, file_patch: &FilePatch) {
    // Add metadata comment
    output.push_str(&format!(
        "# RUP: CID={} CONTEXT={} ENGINE={}\n",
        file_patch.metadata.source_cid.as_deref().unwrap_or("none"),
        file_patch.metadata.context_lines,
        file_patch.metadata.engine
    ));

    // Standard git diff header
    output.push_str(&format!(
        "diff --git a/{} b/{}\n",
        file_patch.path, file_patch.path
    ));
    output.push_str(&format!("--- a/{}\n", file_patch.path));
    output.push_str(&format!("+++ b/{}\n", file_patch.path));

    // Render each hunk
    for hunk in &file_patch.hunks {
        render_hunk(output, hunk);
    }
}

/// Render a single hunk
fn render_hunk(output: &mut String, hunk: &Hunk) {
    // Hunk header
    output.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
    ));

    // Render lines
    for line in &hunk.lines {
        match line {
            HunkLine::Context(content) => output.push_str(&format!(" {}\n", content)),
            HunkLine::Remove(content) => output.push_str(&format!("-{}\n", content)),
            HunkLine::Add(content) => output.push_str(&format!("+{}\n", content)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::edit::{EditOperation, EditSpec, FileBlock};
    // use std::path::PathBuf; // Not needed in test
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_simple_replace_patch() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "line 1").unwrap();
        writeln!(temp_file, "line 2").unwrap();
        writeln!(temp_file, "line 3").unwrap();

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

        let config = PatchConfig::default();
        let patch_set = generate_patches(&spec, &config).unwrap();

        assert_eq!(patch_set.file_patches.len(), 1);
        let file_patch = &patch_set.file_patches[0];
        assert_eq!(file_patch.hunks.len(), 1);

        let diff = render_unified_diff(&patch_set);
        assert!(diff.contains("diff --git"));
        assert!(diff.contains("-line 2"));
        assert!(diff.contains("+modified line 2"));
    }

    #[test]
    fn test_insert_patch() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "line 1").unwrap();
        writeln!(temp_file, "line 2").unwrap();

        let spec = EditSpec {
            file_blocks: vec![FileBlock {
                path: temp_file.path().to_path_buf(),
                operations: vec![EditOperation::Insert {
                    at_line: 1,
                    new_content: "inserted line".to_string(),
                }],
            }],
        };

        let config = PatchConfig::default();
        let patch_set = generate_patches(&spec, &config).unwrap();
        let diff = render_unified_diff(&patch_set);

        assert!(diff.contains("+inserted line"));
    }
}
