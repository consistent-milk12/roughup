//! Filepath: src/infra/walk.rs
//! Gitignore-aware file walker with optional extras.
//! - Respects .gitignore, .git/info/exclude, and global gitignore
//! - Extra ignore globs (early prune + late filter)
//! - Optional file type filtering (e.g., "rust", "python")
//! - Optional hidden file policy, following symlinks, and max depth
//! - Deterministic ordering for stable tests/CI
//!
//! Backed by ripgrep's `ignore` crate and `globset`.
//!
//! Notes on precedence (summarized):
//!   1) Glob overrides → 2) ignore files → 3) file types → 4) hidden.
//! See `ignore::WalkBuilder` docs for full details.
//!
//! This module preserves the public API of the previous version
//! (`FileWalker::new`, `walk_files`, `walk_with_filter`) while
//! adding builder-style opt-ins for future needs.

use std::path::{Path, PathBuf};

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::{
    DirEntry, WalkBuilder,
    types::{Types, TypesBuilder},
};

/// Gitignore-aware walker with optional extra ignore globs and filters.
/// Extra globs are applied in two places:
///   1) Early: prune directories during traversal (filter_entry).
///   2) Late: filter out files that still slipped through.
pub struct FileWalker
{
    /// Compiled set of additional ignore patterns
    ignore_patterns: GlobSet,

    /// Optional file type matcher ("rust", "python", etc.)
    file_types: Option<Types>,

    /// Include hidden (dot) files; default true (match ripgrep’s configurability)
    include_hidden: bool,

    /// Follow symbolic links; default false
    follow_symlinks: bool,

    /// Maximum recursion depth; default None (unbounded)
    max_depth: Option<usize>,
}

impl FileWalker
{
    /// Build a walker with additional ignore patterns (e.g., "target/**",
    /// "node_modules/**", "**/*.min.js"). Patterns match on (relative) paths.
    pub fn new(additional_ignores: &[String]) -> Result<Self>
    {
        let mut builder = GlobSetBuilder::new();

        for pattern in additional_ignores
        {
            builder.add(Glob::new(pattern)?);
        }

        Ok(Self {
            ignore_patterns: builder.build()?,
            file_types: None,
            include_hidden: true,
            follow_symlinks: false,
            max_depth: None,
        })
    }

    /// (Optional) Restrict walking to a set of **default** file types by name.
    /// Example: `with_default_types(&["rust","python"])`
    pub fn with_default_types(
        mut self,
        names: &[&str],
    ) -> Result<Self>
    {
        let mut tb = TypesBuilder::new();

        // Load default file types
        tb.add_defaults();

        for n in names
        {
            tb.select(n);
        }

        self.file_types = Some(tb.build()?);

        Ok(self)
    }

    /// (Optional) Supply a pre-built `Types` matcher (advanced usage).
    pub fn with_types(
        mut self,
        types: Types,
    ) -> Self
    {
        self.file_types = Some(types);
        self
    }

    /// (Optional) Include or exclude hidden files (dotfiles).
    /// When `include_hidden == true`, hidden files are included.
    pub fn with_include_hidden(
        mut self,
        include_hidden: bool,
    ) -> Self
    {
        self.include_hidden = include_hidden;
        self
    }

    /// (Optional) Follow or skip symbolic links (default false).
    pub fn with_follow_symlinks(
        mut self,
        follow: bool,
    ) -> Self
    {
        self.follow_symlinks = follow;
        self
    }

    /// (Optional) Limit recursion depth (`None` = unbounded).
    pub fn with_max_depth(
        mut self,
        depth: Option<usize>,
    ) -> Self
    {
        self.max_depth = depth;
        self
    }

    /// Internal: construct a configured WalkBuilder for `root`.
    fn build_walk(
        &self,
        root: &Path,
    ) -> WalkBuilder
    {
        let mut b = WalkBuilder::new(root);

        // Hidden files policy:
        //   WalkBuilder::hidden(true)  => *skip* dotfiles
        //   WalkBuilder::hidden(false) => include dotfiles
        b.hidden(!self.include_hidden); // invert our flag for builder

        // Respect .ignore/.gitignore/.git/info/exclude and global gitignore
        b.git_ignore(true);
        b.git_global(true);
        b.git_exclude(true);

        // Follow symlinks / set max depth as requested
        b.follow_links(self.follow_symlinks);
        b.max_depth(self.max_depth);

        // Early directory pruning using extra ignores (fast short-circuit).
        let extra = self
            .ignore_patterns
            .clone();
        b.filter_entry(move |ent: &DirEntry| {
            // Be conservative on unknown types.
            let is_dir = ent
                .file_type()
                .map(|ft| ft.is_dir())
                .unwrap_or(false);

            if is_dir && extra.is_match(ent.path())
            {
                return false;
            }
            true
        });

        // Optional file type filtering (runs after ignore checks).
        if let Some(t) = &self.file_types
        {
            b.types(t.clone());
        }

        b
    }

    /// Traverse files under `root`, respecting ignore rules and extra globs.
    /// Returns a **sorted** list of file paths for determinism.
    pub fn walk_files<P: AsRef<Path>>(
        &self,
        root: P,
    ) -> Vec<PathBuf>
    {
        let root_path = root.as_ref();
        let walker = self
            .build_walk(root_path)
            .build();

        let mut out: Vec<PathBuf> = walker
            // Drop entries with IO errors (could be collected/logged later)
            .filter_map(|res| res.ok())
            // Keep only regular files
            .filter(|entry| {
                entry
                    .file_type()
                    .is_some_and(|ft| ft.is_file())
            })
            // Convert to owned path
            .map(|entry| entry.into_path())
            // Late file-level extra ignore filtering using RELATIVE path
            .filter(|abs| {
                let rel = abs
                    .strip_prefix(root_path)
                    .unwrap_or(abs);
                !self
                    .ignore_patterns
                    .is_match(rel)
            })
            .collect();

        // Deterministic order (stable CLI & tests)
        out.sort();

        out
    }

    /// Traverse and then apply a caller-provided filter predicate.
    /// This runs after git/extra ignore filtering.
    pub fn walk_with_filter<P, F>(
        &self,
        root: P,
        filter: F,
    ) -> Vec<PathBuf>
    where
        P: AsRef<Path>,
        F: Fn(&Path) -> bool,
    {
        self.walk_files(root)
            .into_iter()
            .filter(|p| filter(p))
            .collect()
    }
}

#[cfg(test)]
mod tests
{
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    /// Create a file with parent dirs as needed
    fn write_file(
        root: &Path,
        rel: &str,
        contents: &str,
    ) -> Result<()>
    {
        let path = root.join(rel);
        if let Some(parent) = path.parent()
        {
            std::fs::create_dir_all(parent)?;
        }
        fs::write(path, contents)?;
        Ok(())
    }

    #[test]
    fn test_file_walking_simple() -> Result<()>
    {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        write_file(root, "test.rs", "fn main() {}")?;
        write_file(root, "README.md", "# Test")?;

        let walker = FileWalker::new(&[])?;
        let files = walker.walk_files(root);

        assert_eq!(files.len(), 2);
        assert!(
            files
                .iter()
                .any(|p| {
                    p.file_name()
                        .unwrap()
                        == "README.md"
                })
        );
        assert!(
            files
                .iter()
                .any(|p| {
                    p.file_name()
                        .unwrap()
                        == "test.rs"
                })
        );
        assert!(
            files
                .windows(2)
                .all(|w| w[0] <= w[1])
        );
        Ok(())
    }

    #[test]
    fn test_respects_gitignore() -> Result<()>
    {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        // init git repo so .gitignore applies in some environments
        let _ = std::process::Command::new("git")
            .args(["init"])
            .current_dir(root)
            .output();

        write_file(root, ".gitignore", "README.md")?;
        write_file(root, "README.md", "# Ignored by gitignore")?;
        write_file(root, "keep.txt", "keep")?;

        let walker = FileWalker::new(&[])?;
        let files = walker.walk_files(root);

        let user_files: Vec<_> = files
            .iter()
            .filter(|f| {
                let path_str = f.to_string_lossy();
                !path_str.contains("/.git/")
                    && f.file_name()
                        .and_then(|n| n.to_str())
                        != Some(".gitignore")
            })
            .collect();

        assert_eq!(
            user_files.len(),
            1,
            "Expected 1 user file, found: {:?}",
            files
        );
        assert_eq!(
            user_files[0]
                .file_name()
                .unwrap(),
            "keep.txt"
        );
        Ok(())
    }

    #[test]
    fn test_additional_globs_prune_and_filter() -> Result<()>
    {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        write_file(root, "target/build/a.o", "bin")?;
        write_file(root, "node_modules/pkg/index.js", "js")?;
        write_file(root, "src/lib.rs", "pub fn x() {}")?;

        let ignores = vec!["target/**".to_string(), "node_modules/**".to_string()];
        let walker = FileWalker::new(&ignores)?;
        let files = walker.walk_files(root);

        assert_eq!(files.len(), 1, "unexpected files: {files:?}");
        assert_eq!(
            files[0]
                .strip_prefix(root)
                .unwrap(),
            Path::new("src/lib.rs")
        );
        Ok(())
    }

    #[test]
    fn test_hidden_files_included_unless_ignored() -> Result<()>
    {
        let temp_dir = TempDir::new()?;
        let root = temp_dir.path();

        write_file(root, ".hidden.txt", "h")?;
        write_file(root, "visible.txt", "v")?;

        // Default includes hidden files
        let walker = FileWalker::new(&[])?;
        let mut files = walker.walk_files(root);
        for p in &mut files
        {
            *p = p
                .strip_prefix(root)
                .unwrap()
                .to_path_buf();
        }
        assert!(files.contains(&PathBuf::from(".hidden.txt")));
        assert!(files.contains(&PathBuf::from("visible.txt")));

        // Now exclude hidden
        let walker = FileWalker::new(&[])?.with_include_hidden(false);
        let mut files = walker.walk_files(root);
        for p in &mut files
        {
            *p = p
                .strip_prefix(root)
                .unwrap()
                .to_path_buf();
        }
        assert!(!files.contains(&PathBuf::from(".hidden.txt")));
        assert!(files.contains(&PathBuf::from("visible.txt")));
        Ok(())
    }

    #[test]
    fn test_types_filter_rust_only() -> Result<()>
    {
        let tmp = TempDir::new()?;
        let root = tmp.path();

        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(root.join("src/lib.rs"), "pub fn x() {}")?;
        std::fs::write(root.join("README.md"), "# readme")?;
        std::fs::write(root.join("script.py"), "print('hi')")?;

        let walker = FileWalker::new(&[])?.with_default_types(&["rust"])?;
        let mut files = walker.walk_files(root);
        files
            .iter_mut()
            .for_each(|p| {
                *p = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_path_buf()
            });

        assert_eq!(files, vec![PathBuf::from("src/lib.rs")]);
        Ok(())
    }

    #[test]
    fn test_max_depth_and_symlinks() -> Result<()>
    {
        let tmp = TempDir::new()?;
        let root = tmp.path();

        // depth 1: just root files (depth 0 would traverse nothing)
        std::fs::write(root.join("a.txt"), "a")?;
        std::fs::create_dir_all(root.join("sub"))?;
        std::fs::write(root.join("sub/b.txt"), "b")?;

        // Without following symlinks and max depth 1, only a.txt (depth 0 means traverse nothing)
        let walker = FileWalker::new(&[])?.with_max_depth(Some(1));
        let mut files = walker.walk_files(root);
        files
            .iter_mut()
            .for_each(|p| {
                *p = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_path_buf()
            });
        assert!(files.contains(&PathBuf::from("a.txt")));
        assert!(!files.contains(&PathBuf::from("sub/b.txt")));

        // (symlink test is platform specific; skip if creation fails)
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(root.join("sub/b.txt"), root.join("link_to_b")).ok();
            let walker = FileWalker::new(&[])?.with_follow_symlinks(true);
            let files = walker.walk_files(root);
            assert!(
                files
                    .iter()
                    .any(|p| {
                        p.file_name()
                            .unwrap()
                            == "link_to_b"
                    })
            );
        }

        Ok(())
    }
}
