//! Filepath: src/infra/walk.rs

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::{DirEntry, WalkBuilder};
use std::path::{Path, PathBuf};

/// Gitignore-aware walker with optional extra ignore globs.
/// Extra globs are applied in two places:
///   1) Early: prune directories during traversal (filter_entry).
///   2) Late: filter out files that still slipped through.
pub struct FileWalker {
    /// Compiled set of additional ignore patterns
    ignore_patterns: GlobSet,
}

impl FileWalker {
    /// Build a walker with additional ignore patterns (e.g., "target/**",
    /// "node_modules/**", "**/*.min.js"). These are matched on full paths.
    pub fn new(additional_ignores: &[String]) -> Result<Self> {
        // Create a builder for the glob set
        let mut builder = GlobSetBuilder::new();

        // Add all caller-provided patterns
        for pattern in additional_ignores {
            // Compile each glob; return early on invalid patterns
            let glob = Glob::new(pattern)?;
            builder.add(glob);
        }

        // Build the compiled set (empty if no patterns provided)
        let ignore_patterns = builder.build()?;

        // Return the configured walker
        Ok(Self { ignore_patterns })
    }

    /// Traverse files under `root`, respecting .gitignore and extra globs.
    /// Returns a sorted list of file paths for determinism.
    pub fn walk_files<P: AsRef<Path>>(&self, root: P) -> Vec<PathBuf> {
        let root_path = root.as_ref();
        // Create a builder so we can attach a directory-pruning predicate
        let mut builder = WalkBuilder::new(root.as_ref());

        // Include hidden files; rely on .gitignore for policy
        builder.hidden(false);

        // Respect all gitignore sources (local, global, excludes)
        builder.git_ignore(true);
        builder.git_exclude(true);
        builder.git_global(true);

        // Prune directories that match additional ignore patterns early.
        // This prevents descending into large ignored trees (e.g., target/, node_modules/).
        //
        // Note: we only prune directories here; file-level ignores are handled later.
        let extra = self.ignore_patterns.clone();
        builder.filter_entry(move |ent: &DirEntry| {
            // If we cannot determine the type, keep it (be conservative)
            let is_dir = ent.file_type().map(|ft| ft.is_dir()).unwrap_or(false);

            // If this is a directory and it matches an extra ignore, prune it now
            if is_dir && extra.is_match(ent.path()) {
                return false;
            }

            // Otherwise keep traversing
            true
        });

        // Build the iterator
        let walker = builder.build();

        // Collect only files, excluding those matched by extra globs
        let mut out: Vec<PathBuf> = walker
            // Drop entries with IO errors; production code could collect them
            .filter_map(|res| res.ok())
            // Keep only regular files
            .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
            // Convert to owned path
            .map(|entry| entry.into_path())
            // Apply file-level extra ignore filtering using relative paths
            .filter(|path| {
                let rel_path = path.strip_prefix(root_path).unwrap_or(path);
                !self.ignore_patterns.is_match(rel_path)
            })
            // Collect into a vector
            .collect();

        // Ensure deterministic output order (useful for tests and stable CLI)
        out.sort();

        // Return file list
        out
    }

    /// Traverse and then apply a caller-provided filter predicate.
    /// This runs after git/extra ignore filtering.
    #[allow(dead_code)]
    pub fn walk_with_filter<P, F>(&self, root: P, filter: F) -> Vec<PathBuf>
    where
        P: AsRef<Path>,
        F: Fn(&Path) -> bool,
    {
        // Run the main traversal
        let files = self.walk_files(root);

        // Apply the caller's predicate
        files.into_iter().filter(|p| filter(p)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Create a file with parent dirs as needed
    fn write_file(root: &Path, rel: &str, contents: &str) -> Result<()> {
        // Build absolute path
        let path = root.join(rel);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write file contents
        fs::write(path, contents)?;

        // Done
        Ok(())
    }

    #[test]
    fn test_file_walking_simple() -> Result<()> {
        // Create an isolated temporary directory
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        // Create two simple files
        write_file(root, "test.rs", "fn main() {}")?;
        write_file(root, "README.md", "# Test")?;

        // Build a walker with no extra ignores
        let walker = FileWalker::new(&[])?;

        // Walk files
        let files = walker.walk_files(root);

        // We expect exactly these two files
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|p| p.file_name().unwrap() == "README.md"));
        assert!(files.iter().any(|p| p.file_name().unwrap() == "test.rs"));

        // Sorted determinism: README.md before test.rs in most locales
        assert!(files.windows(2).all(|w| w[0] <= w[1]));

        // Done
        Ok(())
    }

    #[test]
    fn test_respects_gitignore() -> Result<()> {
        // Create temp directory
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        // Initialize git repo (required for gitignore to work with ignore crate)
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root)
            .output()
            .ok();

        // Create a .gitignore that ignores README.md
        write_file(root, ".gitignore", "README.md")?;

        // Create files — one ignored, one not
        write_file(root, "README.md", "# Ignored by gitignore")?;
        write_file(root, "keep.txt", "keep")?;

        // No extra ignores
        let walker = FileWalker::new(&[])?;

        // Walk files (should honor .gitignore)
        let files = walker.walk_files(root);

        // Filter out .git directory files and .gitignore itself for the test assertion
        let user_files: Vec<_> = files
            .iter()
            .filter(|f| {
                let path_str = f.to_string_lossy();
                !path_str.contains("/.git/")
                    && f.file_name().and_then(|n| n.to_str()) != Some(".gitignore")
            })
            .collect();

        // Only keep.txt should remain (README.md should be ignored by .gitignore)
        assert_eq!(
            user_files.len(),
            1,
            "Expected 1 user file, found: {:?}",
            files
        );
        assert_eq!(user_files[0].file_name().unwrap(), "keep.txt");

        // Done
        Ok(())
    }

    #[test]
    fn test_additional_globs_prune_and_filter() -> Result<()> {
        // Create temp directory
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        // Create a heavy directory tree and a normal file
        write_file(root, "target/build/a.o", "bin")?;
        write_file(root, "node_modules/pkg/index.js", "js")?;
        write_file(root, "src/lib.rs", "pub fn x() {}")?;

        // Provide extra ignores that target directories
        let ignores = vec!["target/**".to_string(), "node_modules/**".to_string()];

        // Build walker with extra ignores
        let walker = FileWalker::new(&ignores)?;

        // Walk files
        let files = walker.walk_files(root);

        // Only src/lib.rs should remain (others pruned/filtered)
        assert_eq!(files.len(), 1, "unexpected files: {files:?}");
        assert_eq!(
            files[0].strip_prefix(root).unwrap(),
            Path::new("src/lib.rs")
        );

        // Done
        Ok(())
    }

    #[test]
    fn test_hidden_files_included_unless_ignored() -> Result<()> {
        // Create temp directory
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        // Create hidden and normal files
        write_file(root, ".hidden.txt", "h")?;
        write_file(root, "visible.txt", "v")?;

        // No .gitignore and no extra ignores → hidden should be included
        let walker = FileWalker::new(&[])?;
        let mut files = walker.walk_files(root);

        // Normalize relative paths for assertion clarity
        for p in &mut files {
            *p = p.strip_prefix(root).unwrap().to_path_buf();
        }

        // Expect both files
        assert!(files.contains(&PathBuf::from(".hidden.txt")));
        assert!(files.contains(&PathBuf::from("visible.txt")));

        // Done
        Ok(())
    }
}
