//! Filepath: src/symbols.rs
//! End-to-end symbol extraction pipeline organized into
//! small structs with associated functions only. This
//! module is feature-gated by `symbols` where relevant,
//! and keeps responsibilities explicit and testable.
use std::{
    collections::HashSet,   // Fast language filter
    fs::File,               // Output file handle
    io::{BufWriter, Write}, // Buffered writer
    path::{Path, PathBuf},  // Paths
};

use anyhow::{Context, Result}; // Error handling
use rayon::prelude::*; // Parallelism
use serde::{Deserialize, Serialize}; // JSONL records

use crate::{
    infra::walk::FileWalker,
    parsers::{PythonExtractor, RustExtractor},
};

/// Configuration options for symbol extraction (future-proof extension point)
#[derive(Debug, Clone, Default)]
pub struct ExtractOptions
{
    /// Future use: include private, doc scraping, node caps, etc.
    pub include_private: bool,
}

/// Public CLI entry point expected by the command layer
pub fn run(
    args: crate::cli::SymbolsArgs,
    ctx: &crate::cli::AppContext,
) -> Result<()>
{
    // Load configuration with graceful fallback
    let config = crate::infra::config::load_config().unwrap_or_default();

    // Build a Gitignore-aware file walker with extra globs
    let walker = FileWalker::new(&config.ignore_patterns)?;

    // Resolve target languages from args or config
    let langs = LanguageSelector::resolve(&args, &config);

    // Collect files under root filtered by language
    let files = FileCollector::collect(&walker, &args.path, &langs);

    // Early exit if nothing to do
    if files.is_empty()
    {
        if !ctx.quiet
        {
            println!(
                "No files found for languages: {:?} (root: {})",
                langs.as_vec(),
                args.path
                    .display()
            );
        }
        return Ok(());
    }

    // Inform the user how many files will be processed
    if !ctx.quiet
    {
        println!("Extracting symbols from {} files...", files.len());
    }

    // Extract symbols in parallel and aggregate results
    let mut all: Vec<Symbol> = SymbolsExecutor::extract_parallel(&files, &args)?;

    // Optionally filter private symbols based on flag
    if !args.include_private
    {
        VisibilityFilter::retain_public(&mut all);
    }

    // Compute line numbers efficiently for each file's symbols
    LineNumberMapper::fill_lines(&mut all, &args.path)?;

    // Ensure deterministic output order across platforms/runs
    all.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(
                a.start_line
                    .cmp(&b.start_line),
            )
            .then(
                a.byte_start
                    .cmp(&b.byte_start),
            )
            .then(
                a.name
                    .cmp(&b.name),
            )
    });

    // Write symbols to JSONL destination
    JsonlWriter::write(&all, &args.output)?;

    // Print a success message with the output path
    if !ctx.quiet
    {
        println!(
            "✓ Extracted {} symbols to {}",
            all.len(),
            args.output
                .display()
        );
    }

    // Done
    Ok(())
}

/// Normalized symbol record optimized for LLM consumption
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Symbol
{
    /// File path relative to project root
    pub file: PathBuf,

    /// Programming language label
    pub lang: String,

    /// Normalized symbol kind
    pub kind: SymbolKind,

    /// Simple declared name
    pub name: String,

    /// Qualified name (language-appropriate)
    pub qualified_name: String,

    /// Start byte in the file
    pub byte_start: usize,

    /// End byte in the file
    pub byte_end: usize,

    /// 1-based start line (computed post-extraction)
    pub start_line: usize,

    /// 1-based end line (computed post-extraction)
    pub end_line: usize,

    /// Optional visibility information
    pub visibility: Option<Visibility>,

    /// Optional documentation preview
    pub doc: Option<String>,
}

/// Normalized symbol kinds across languages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind
{
    /// Free-standing function
    Function,

    /// Class/impl/trait method
    Method,

    /// Rust struct or similar
    Struct,

    /// Rust/TypeScript enum
    Enum,

    /// Rust trait
    Trait,

    /// Python/TS/Java class
    Class,

    /// Interface-like construct
    Interface,

    /// Rust impl block
    Impl,

    /// Type alias / typedef
    TypeAlias,

    /// Module / namespace
    Module,

    /// Package / top-level unit
    Package,

    /// Local or field variable
    Variable,

    /// Constant definition
    Constant,
}

/// Normalized visibility levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Visibility
{
    /// Publicly visible
    Public,

    /// Private/internal
    Private,

    /// Protected (OO languages)
    Protected,

    /// Internal (e.g., C#)
    Internal,
}

/// Internal helper that selects target languages
struct LanguageSelector
{
    /// Canonical lowercase language labels
    set: HashSet<String>,

    /// Original order for user messages
    ordered: Vec<String>,
}

impl LanguageSelector
{
    /// Construct from CLI args and config fallback
    fn resolve(
        args: &crate::cli::SymbolsArgs,
        cfg: &crate::infra::config::Config,
    ) -> Self
    {
        // Choose CLI-provided list or config default
        let ordered = if args
            .languages
            .is_empty()
        {
            cfg.symbols
                .languages
                .clone()
        }
        else
        {
            args.languages
                .clone()
        };

        // Normalize to lowercase for matching
        let set = ordered
            .iter()
            .map(|s| s.to_lowercase())
            .collect::<HashSet<_>>();

        // Return the selector
        Self { set, ordered }
    }

    /// Test if a language label is selected
    fn contains(
        &self,
        lang: &str,
    ) -> bool
    {
        self.set
            .contains(lang)
    }

    /// Expose ordered languages for messages
    fn as_vec(&self) -> Vec<String>
    {
        self.ordered
            .clone()
    }
}

/// File collection based on Gitignore-aware walking
struct FileCollector;

impl FileCollector
{
    /// Walk the tree and retain files that match selected languages
    fn collect(
        walker: &FileWalker,
        root: &Path,
        langs: &LanguageSelector,
    ) -> Vec<(PathBuf, String)>
    {
        // Walk all files under root
        let files = walker.walk_files(root);

        // Detect languages and retain matched pairs (only for supported AND selected languages)
        files
            .into_iter()
            .filter_map(|path| {
                LanguageDetector::detect(&path).and_then(|lang| {
                    if langs.contains(&lang) && is_supported_language(&lang)
                    {
                        Some((path, lang))
                    }
                    else
                    {
                        None
                    }
                })
            })
            .collect()
    }
}

/// Simple extension-based language detector
struct LanguageDetector;

impl LanguageDetector
{
    /// Map file extensions to canonical language labels
    fn detect(path: &Path) -> Option<String>
    {
        // Get extension as lowercase string
        let ext = path
            .extension()?
            .to_str()?
            .to_lowercase();

        // Map common extensions to languages
        let lang = match ext.as_str()
        {
            "rs" => "rust",
            "py" => "python",
            "js" | "jsx" => "javascript",
            "ts" | "tsx" => "typescript",
            "go" => "go",
            "c" | "h" => "c",
            "cpp" | "cxx" | "cc" | "hpp" => "cpp",
            _ => return None,
        };

        // Return owned language string
        Some(lang.to_string())
    }
}

/// Parallel symbol extraction coordinator
struct SymbolsExecutor;

impl SymbolsExecutor
{
    /// Extract symbols from all files using rayon
    fn extract_parallel(
        files: &[(PathBuf, String)],
        args: &crate::cli::SymbolsArgs,
    ) -> Result<Vec<Symbol>>
    {
        // Convert to parallel iterator over file-language pairs
        let results: Vec<Result<Vec<Symbol>>> = files
            .par_iter()
            .map(|(file, lang)| Self::extract_one(file, lang, &args.path))
            .collect();

        // Aggregate, short-circuiting on first error
        let mut out = Vec::new();
        for r in results
        {
            // Propagate any error
            let mut v = r?;
            // Append the file’s symbols
            out.append(&mut v);
        }

        // Return the aggregated symbols
        Ok(out)
    }

    /// Extract symbols for a single file
    fn extract_one(
        file_path: &Path,
        lang: &str,
        root: &Path,
    ) -> Result<Vec<Symbol>>
    {
        // Read file contents as a single UTF-8 String
        let content = std::fs::read_to_string(file_path)
            .with_context(|| format!("Failed to read {}", file_path.display()))?;

        // Compute the path relative to the root, if possible
        let rel = file_path
            .strip_prefix(root)
            .unwrap_or(file_path)
            .to_path_buf();

        // Acquire a language-specific extractor
        let extractor = get_extractor(lang)?;

        // Run the extractor to produce raw symbols
        let mut symbols = extractor.extract_symbols(&content, &rel)?;

        // NEW: canonicalize ordering right after extraction
        extractor.postprocess(&mut symbols);

        // Populate language labels consistently
        for s in &mut symbols
        {
            s.lang = lang.to_string();
        }

        // Return the file’s symbols
        Ok(symbols)
    }
}

/// Map byte offsets to line numbers efficiently
struct LineNumberMapper;

impl LineNumberMapper
{
    /// Compute and assign line numbers for all symbols
    fn fill_lines(
        symbols: &mut [Symbol],
        root: &Path,
    ) -> Result<()>
    {
        // Group symbols by file to avoid re-indexing the same file
        let mut by_file: std::collections::BTreeMap<PathBuf, Vec<usize>> =
            std::collections::BTreeMap::new();

        // Index into `symbols` for each file
        for (i, s) in symbols
            .iter()
            .enumerate()
        {
            by_file
                .entry(
                    s.file
                        .clone(),
                )
                .or_default()
                .push(i);
        }

        // For each file, build an index and set line numbers
        for (file, idxs) in by_file
        {
            // Re-read using absolute path: root.join(relative)
            let abs = if file.is_absolute()
            {
                file.clone()
            }
            else
            {
                root.join(&file)
            };
            let content = std::fs::read_to_string(&abs)
                .with_context(|| format!("Failed to re-read {}", abs.display()))?;

            // Build line index from content
            let li = LineIndex::new(&content);

            // Assign each symbol’s line range
            for i in idxs
            {
                let start = symbols[i].byte_start;
                let end = symbols[i].byte_end;
                let (sl, el) = li.byte_span_to_lines(start, end);
                symbols[i].start_line = sl;
                symbols[i].end_line = el;
            }
        }

        // Done
        Ok(())
    }
}

/// Immutable index of line start byte offsets
struct LineIndex
{
    /// Full text slice for length context
    text_len: usize,

    /// Byte offsets for each line start
    starts: Vec<usize>,
}

impl LineIndex
{
    /// Build a line index by scanning once
    fn new(text: &str) -> Self
    {
        // Initialize with first line at byte 0
        let mut starts = Vec::with_capacity(128);
        starts.push(0);

        // Record every newline boundary
        for (i, ch) in text.char_indices()
        {
            if ch == '\n'
            {
                starts.push(i + 1);
            }
        }

        // Construct the index
        Self { text_len: text.len(), starts }
    }

    /// Convert a byte span into 1-based line numbers
    fn byte_span_to_lines(
        &self,
        s: usize,
        e: usize,
    ) -> (usize, usize)
    {
        // Clamp to valid range
        let start = s.min(self.text_len);
        let end = e.min(self.text_len);

        // Find start line via binary search
        let sl = self.byte_to_line(start);
        // Find end line via binary search
        let el = self.byte_to_line(end.saturating_sub(1));

        // Return 1-based line numbers
        (sl, el)
    }

    /// Convert a single byte offset to a 1-based line number
    fn byte_to_line(
        &self,
        b: usize,
    ) -> usize
    {
        // Binary search the greatest line start <= b
        match self
            .starts
            .binary_search(&b)
        {
            Ok(idx) => idx + 1,
            Err(pos) => pos.max(1),
        }
    }
}

/// Filter utilities for visibility post-processing
struct VisibilityFilter;

impl VisibilityFilter
{
    /// Keep only symbols that are public or unspecified
    fn retain_public(v: &mut Vec<Symbol>)
    {
        // Retain when visibility is None or explicitly Public
        v.retain(|s| matches!(&s.visibility, None | Some(Visibility::Public)));
    }
}

/// Stream symbols to a JSON Lines file
struct JsonlWriter;

impl JsonlWriter
{
    /// Write one JSON object per line into `output_path`
    fn write(
        symbols: &[Symbol],
        output_path: &Path,
    ) -> Result<()>
    {
        // Create parent directories if needed
        if let Some(parent) = output_path.parent()
            && !parent
                .as_os_str()
                .is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }

        // Create the destination file
        let file = File::create(output_path)
            .with_context(|| format!("Failed to create {}", output_path.display()))?;

        // Use a buffered writer for throughput
        let mut writer = BufWriter::new(file);

        // Serialize and write each symbol as a single line
        for s in symbols
        {
            let json = serde_json::to_string(s).context("Failed to serialize symbol")?;
            writer
                .write_all(json.as_bytes())
                .context("Failed to write symbol")?;
            writer
                .write_all(b"\n")
                .context("Failed to write newline")?;
        }

        // Ensure all bytes are flushed to disk
        writer
            .flush()
            .context("Failed to flush output")?;

        // Done
        Ok(())
    }
}

// Check if a language has an available extractor
fn is_supported_language(lang: &str) -> bool
{
    matches!(lang, "rust" | "python")
}

// Simple extractor registry
pub fn get_extractor(lang: &str) -> anyhow::Result<Box<dyn SymbolExtractor + Send + Sync>>
{
    match lang
    {
        "rust" => Ok(Box::new(RustExtractor::new()?)),
        "python" => Ok(Box::new(PythonExtractor::new()?)),
        _ => Err(anyhow::anyhow!("Unsupported language: {}", lang)),
    }
}

pub trait SymbolExtractor: Send + Sync
{
    /// Main extraction entrypoint (existing behavior).
    fn extract_symbols(
        &self,
        content: &str,
        file_path: &std::path::Path,
    ) -> anyhow::Result<Vec<Symbol>>;

    /// Optional, extended entrypoint (defaults to legacy).
    fn extract_symbols_with(
        &self,
        content: &str,
        file_path: &std::path::Path,
        _opts: &ExtractOptions,
    ) -> anyhow::Result<Vec<Symbol>>
    {
        self.extract_symbols(content, file_path)
    }

    /// Post-process extracted symbols; default enforces deterministic order.
    /// Sort by (file asc, byte_start asc, name asc).
    fn postprocess(
        &self,
        syms: &mut Vec<Symbol>,
    )
    {
        syms.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then(
                    a.byte_start
                        .cmp(&b.byte_start),
                )
                .then(
                    a.name
                        .cmp(&b.name),
                )
        });
    }
}

// Helper function for qualified name building
pub fn build_qualified_name(parts: &[&str]) -> String
{
    parts.join("::")
}

// Helper function for visibility parsing
pub fn parse_visibility(text: &str) -> Option<Visibility>
{
    match text.trim()
    {
        "pub" | "public" => Some(Visibility::Public),
        "private" | "priv" => Some(Visibility::Private),
        "protected" => Some(Visibility::Protected),
        _ => None,
    }
}

#[cfg(test)]
mod tests
{
    // Bring everything from the outer scope
    use super::*;

    /// Verify extension-to-language mapping
    #[test]
    fn language_detection_matrix()
    {
        // Basic positive cases
        assert_eq!(
            LanguageDetector::detect(Path::new("a.rs")),
            Some("rust".into())
        );
        assert_eq!(
            LanguageDetector::detect(Path::new("b.py")),
            Some("python".into())
        );
        assert_eq!(
            LanguageDetector::detect(Path::new("c.tsx")),
            Some("typescript".into())
        );
        assert_eq!(
            LanguageDetector::detect(Path::new("d.jsx")),
            Some("javascript".into())
        );

        // Negative case
        assert_eq!(LanguageDetector::detect(Path::new("e.unknown")), None);
    }

    /// Verify line mapping on a small sample
    #[test]
    fn line_index_maps_spans()
    {
        // Build a small 3-line text
        let text = "line1\nline2\nline3\n";

        // Create index
        let idx = LineIndex::new(text);

        // Span for "line1"
        assert_eq!(idx.byte_span_to_lines(0, 5), (1, 1));

        // Span for "line2"
        assert_eq!(idx.byte_span_to_lines(6, 11), (2, 2));

        // Span covering lines 2..3
        assert_eq!(idx.byte_span_to_lines(6, 17), (2, 3));
    }

    /// Verify JSONL writer produces one line per symbol
    #[test]
    fn jsonl_writer_emits_one_line_per_symbol() -> Result<()>
    {
        // Build a couple of simple symbols
        let base = Symbol {
            file: PathBuf::from("src/lib.rs"),
            lang: "rust".into(),
            kind: SymbolKind::Function,
            name: "f".into(),
            qualified_name: "crate::f".into(),
            byte_start: 0,
            byte_end: 10,
            start_line: 1,
            end_line: 1,
            visibility: Some(Visibility::Public),
            doc: None,
        };

        // Clone with small changes
        let mut items = vec![base.clone()];
        let mut b = base.clone();
        b.name = "g".into();
        b.qualified_name = "crate::g".into();
        items.push(b);

        // Write to a temp file
        let dir = tempfile::TempDir::new()?;
        let out = dir
            .path()
            .join("symbols.jsonl");
        JsonlWriter::write(&items, &out)?;

        // Read back and count lines
        let data = std::fs::read_to_string(&out)?;
        let n = data
            .lines()
            .count();

        // Expect one line per symbol
        assert_eq!(n, 2);

        // Done
        Ok(())
    }
}
