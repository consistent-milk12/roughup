//! Filepath: src/core/tree.rs
//! Tree view that appends per-file total line counts as `name:lines`
//! e.g., `main.rs:100`. Directories are displayed as before.
//!
//! Performance notes:
//! - Counts lines by scanning bytes for '\n' (CRLF-safe).
//! - Memory-mapped for files > 1MB (configurable here).
//! - Uses BTreeMap for deterministic ordering.

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use ptree::TreeBuilder;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::{AppContext, TreeArgs};
use crate::infra::config::load_config;
use crate::infra::walk::FileWalker;

const MMAP_THRESHOLD_BYTES: u64 = 1_048_576; // 1 MiB

pub fn run(args: TreeArgs, ctx: &AppContext) -> Result<()> {
    let config = load_config().unwrap_or_default();

    // Combine config ignore patterns with CLI args
    let mut ignore_patterns = config.ignore_patterns.clone();
    ignore_patterns.extend(args.ignore);

    let walker = FileWalker::new(&ignore_patterns)?;

    if ctx.dry_run {
        if !ctx.quiet {
            println!("{}", "DRY RUN: Would scan:".yellow());
            println!("  Root: {}", args.path.display());
            println!("  Max depth: {:?}", args.depth);
            println!("  Ignore patterns: {:?}", ignore_patterns);
        }
        return Ok(());
    }

    // Build file tree with per-file line counts
    let tree = build_tree_with_counts(&args.path, &walker, args.depth)?;

    // Print tree (unless quiet)
    if !ctx.quiet {
        print_tree(&tree)?;
    }

    Ok(())
}

#[derive(Debug)]
struct TreeNode {
    name: String,

    #[expect(unused, reason = "TODO: MARKED FOR USE")]
    path: PathBuf,
    is_dir: bool,
    /// For files, total line count; None for directories.
    line_count: Option<usize>,
    children: BTreeMap<String, TreeNode>,
}

impl TreeNode {
    fn new(name: String, path: PathBuf, is_dir: bool) -> Self {
        Self {
            name,
            path,
            is_dir,
            line_count: None,
            children: BTreeMap::new(),
        }
    }

    /// Insert a path into the tree. If `file_lines` is Some(_),
    /// it is applied to the leaf file node.
    fn insert_path(
        &mut self,
        full_path: &Path,
        relative_path: &Path,
        max_depth: Option<usize>,
        current_depth: usize,
        file_lines: Option<usize>,
    ) {
        if let Some(max_depth) = max_depth
            && current_depth >= max_depth
        {
            return;
        }
        if relative_path.components().count() == 0 {
            return;
        }

        let mut components = relative_path.components();
        let first_component = components.next().unwrap();
        let remaining: PathBuf = components.collect();

        let component_name = first_component.as_os_str().to_string_lossy().to_string();
        let component_path = full_path
            .parent()
            .unwrap_or(full_path)
            .join(&component_name);
        let is_dir = component_path.is_dir();

        let entry = self
            .children
            .entry(component_name.clone())
            .or_insert_with(|| {
                TreeNode::new(component_name.clone(), component_path.clone(), is_dir)
            });

        if remaining.as_os_str().is_empty() {
            // Leaf reached. If it's a file and we were given a count, set it.
            if !entry.is_dir
                && let Some(n) = file_lines
            {
                entry.line_count = Some(n);
            }
        } else {
            entry.insert_path(
                &component_path,
                &remaining,
                max_depth,
                current_depth + 1,
                file_lines,
            );
        }
    }
}

/// Build the tree and attach line counts to file leaf nodes.
fn build_tree_with_counts(
    root: &Path,
    walker: &FileWalker,
    max_depth: Option<usize>,
) -> Result<TreeNode> {
    let mut tree = TreeNode::new(
        root.file_name()
            .unwrap_or(root.as_os_str())
            .to_string_lossy()
            .to_string(),
        root.to_path_buf(),
        true,
    );

    // Walk all files once, compute counts, and insert.
    for file_path in walker.walk_files(root) {
        // Compute total lines for this file quickly.
        let lines = count_lines_fast(&file_path)
            .with_context(|| format!("counting lines for {}", file_path.display()))?;

        if let Ok(relative_path) = file_path.strip_prefix(root) {
            tree.insert_path(&file_path, relative_path, max_depth, 0, Some(lines));
        }

        // Also insert parent directories to ensure they exist in the tree.
        let mut current = file_path.parent();
        while let Some(parent) = current {
            if parent == root {
                break;
            }
            if let Ok(relative_path) = parent.strip_prefix(root) {
                tree.insert_path(parent, relative_path, max_depth, 0, None);
            }
            current = parent.parent();
        }
    }

    Ok(tree)
}

/// Print the tree with formatted labels. Files show `name:lines`.
fn print_tree(tree: &TreeNode) -> Result<()> {
    let mut builder = TreeBuilder::new(format_node_label(tree));

    add_children_to_builder(&mut builder, &tree.children);

    let tree = builder.build();
    ptree::print_tree(&tree)?;

    Ok(())
}

fn add_children_to_builder(builder: &mut TreeBuilder, children: &BTreeMap<String, TreeNode>) {
    for child in children.values() {
        if child.children.is_empty() {
            builder.add_empty_child(format_node_label(child));
        } else {
            builder.begin_child(format_node_label(child));
            add_children_to_builder(builder, &child.children);
            builder.end_child();
        }
    }
}

/// Format node label with colors and, for files, appended `:lines`.
fn format_node_label(node: &TreeNode) -> String {
    if node.is_dir {
        format!("{}/", node.name.blue())
    } else {
        let colored = color_by_ext(&node.name);
        match node.line_count {
            Some(n) => format!("{}:{}", colored, n),
            None => colored,
        }
    }
}

fn color_by_ext(name: &str) -> String {
    if let Some(ext) = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
    {
        match ext {
            "rs" => name.yellow().to_string(),
            "py" => name.green().to_string(),
            "js" | "jsx" | "ts" | "tsx" => name.cyan().to_string(),
            "go" => name.magenta().to_string(),
            "c" | "cpp" | "cc" | "cxx" | "h" | "hpp" => name.red().to_string(),
            "md" | "txt" | "readme" => name.white().to_string(),
            "toml" | "yaml" | "yml" | "json" => name.bright_blue().to_string(),
            _ => name.to_string(),
        }
    } else {
        name.to_string()
    }
}

/// Fast, CRLF-safe total line counting.
/// Counts '\n' bytes and adds one if the file is non-empty and doesn't end with '\n'.
fn count_lines_fast(path: &Path) -> Result<usize> {
    use memchr::memchr_iter;
    use memmap2::Mmap;
    use std::fs::File;
    use std::io::Read;

    let meta = fs::metadata(path)?;
    if !meta.is_file() {
        return Ok(0);
    }
    let len = meta.len();
    if len == 0 {
        return Ok(0);
    }

    if len >= MMAP_THRESHOLD_BYTES {
        // Memory-map large files
        let file = File::open(path)?;
        // SAFETY: read-only map of an existing regular file
        let mmap = unsafe { Mmap::map(&file)? };
        let bytes = &mmap[..];

        let nl = memchr_iter(b'\n', bytes).count();
        // If the file doesn't end with '\n', last line has no newline terminator
        Ok(if bytes.ends_with(b"\n") { nl } else { nl + 1 })
    } else {
        // Small files: read into memory
        let mut buf = Vec::with_capacity(len as usize);
        File::open(path)?.read_to_end(&mut buf)?;
        let nl = memchr_iter(b'\n', &buf).count();
        Ok(if buf.ends_with(b"\n") { nl } else { nl + 1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_tree_building_and_counts() -> Result<()> {
        let tmp = TempDir::new()?;
        let root = tmp.path();

        // dirs
        fs::create_dir_all(root.join("src"))?;

        // files
        fs::write(root.join("src/main.rs"), b"fn main() {}\n")?; // 1 line (ends with \n)
        fs::write(root.join("README.md"), b"# Test\nSecond line")?; // 2 lines (no trailing \n)

        let walker = FileWalker::new(&[])?;
        let tree = build_tree_with_counts(root, &walker, None)?;

        // Ensure structure
        let src = tree.children.get("src").expect("src dir present");
        let main = src.children.get("main.rs").expect("main.rs present");
        assert_eq!(main.line_count, Some(1));

        let readme = tree.children.get("README.md").expect("README.md present");
        assert_eq!(readme.line_count, Some(2));

        Ok(())
    }
}
