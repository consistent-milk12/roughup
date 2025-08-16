//! Fast anchor detection and hints using ripgrep-grade building blocks.
//!
//! This module provides O(1) anchor validation and nearest-function detection
//! using grep-searcher for line-oriented scanning, regex-automata for DFA matching,
//! and Tree-sitter for precise validation when needed.

use std::{
    fs::File,
    io::{BufRead, BufReader},
};

use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, SearcherBuilder, Sink, SinkMatch};
use indexmap::IndexMap;
use memchr::memmem;
use miette::{Diagnostic, SourceSpan};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, trace};

/// A function hit with location and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FnHit
{
    pub name: String,
    pub qualified_name: String,
    pub file: String, // Changed from Utf8PathBuf for serde compatibility
    pub start_line: usize,
    pub end_line: usize,
    pub kind: FnKind,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FnKind
{
    Function,
    Method,
    Trait,
    Impl,
    Module,
}

impl std::fmt::Display for FnKind
{
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result
    {
        match self
        {
            FnKind::Function => write!(f, "function"),
            FnKind::Method => write!(f, "method"),
            FnKind::Trait => write!(f, "trait"),
            FnKind::Impl => write!(f, "impl"),
            FnKind::Module => write!(f, "module"),
        }
    }
}

/// Anchor hint states with actionable suggestions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum AnchorHints
{
    Good
    {
        function: FnHit,
    },
    OffByN
    {
        requested_line: usize,
        actual: FnHit,
        offset: isize,
    },
    OutsideScope
    {
        requested_line: usize,
        nearest: Vec<FnHit>,
    },
    NotAFile
    {
        path: Utf8PathBuf,
        reason: String,
    },
}

/// Diagnostic error for bad anchors with suggestions.
#[derive(Debug, Diagnostic, thiserror::Error)]
#[error("Invalid anchor at {file}:{line}")]
pub struct BadAnchorError
{
    pub file: Utf8PathBuf,
    pub line: usize,

    #[source_code]
    pub src: String,

    #[label("anchor points here")]
    pub anchor_span: SourceSpan,

    #[help]
    pub help: String,

    pub suggestions: Vec<(usize, String)>, // (line, description)
}

/// Fast pattern matcher for Rust function signatures.
struct FunctionMatcher
{
    regex: RegexMatcher,
    lang: tree_sitter::Language,
}

impl FunctionMatcher
{
    fn new() -> Result<Self>
    {
        // Fixed regex pattern - removed stray quote, non-capturing groups, proper anchoring
        let pattern = r#"(?m)^[[:space:]]*(?:pub(?:\([^)]*\))?[[:space:]]+)?(?:async[[:space:]]+)?(?:unsafe[[:space:]]+)?(?:const[[:space:]]+)?(?:extern(?:[[:space:]]+"[^"]+")?[[:space:]]+)?(?:fn|trait|impl|mod)[[:space:]]+[A-Za-z_][A-Za-z0-9_]*"#;

        let regex = RegexMatcherBuilder::new()
            .multi_line(true)
            .build(pattern)?;

        // Store Language instead of Parser for 0.25.8 compatibility
        let lang = tree_sitter_rust::LANGUAGE.into();

        Ok(Self { regex, lang })
    }
}

/// Line-oriented sink for collecting function signatures.
struct FunctionSink
{
    hits: IndexMap<usize, FnHit>,
    current_file: Utf8PathBuf,
    line_offset: usize,
}

impl Sink for FunctionSink
{
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        mat: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error>
    {
        let line_num = mat
            .line_number()
            .unwrap_or(1) as usize
            + self.line_offset;
        let bytes = mat.bytes();

        // Quick extraction of function name using memchr
        if let Some(name) = extract_function_name(bytes)
        {
            let hit = FnHit {
                name: name.to_string(),
                qualified_name: build_qualified_name(&self.current_file, name),
                file: self
                    .current_file
                    .as_str()
                    .to_owned(), // Convert to String for serde
                start_line: line_num,
                end_line: line_num, // Will be refined with Tree-sitter if needed
                kind: detect_kind(bytes),
                confidence: 0.9, // High confidence from regex match
            };

            self.hits
                .insert(line_num, hit);
        }

        Ok(true)
    }
}

/// Build a better qualified name using path structure instead of just file_stem
fn build_qualified_name(
    file_path: &Utf8Path,
    name: &str,
) -> String
{
    // Convert path to module-like qualifier
    let path_str = file_path.as_str();
    let module_path = if path_str.ends_with("/mod.rs")
    {
        // For mod.rs files, use parent directory
        path_str
            .trim_end_matches("/mod.rs")
            .replace('/', "::")
    }
    else if path_str.ends_with(".rs")
    {
        // For regular .rs files, use filename without extension
        path_str
            .trim_end_matches(".rs")
            .replace('/', "::")
    }
    else
    {
        // Fallback to file stem
        file_path
            .file_stem()
            .unwrap_or("")
            .to_string()
    };

    if module_path.is_empty()
    {
        name.to_string()
    }
    else
    {
        format!("{}::{}", module_path, name)
    }
}

/// Detect the kind of construct from the matched bytes.
fn detect_kind(bytes: &[u8]) -> FnKind
{
    // Note: This is line-local detection and may miss impl/trait context for methods
    if memmem::find(bytes, b"fn ").is_some()
    {
        FnKind::Function // May be Method in impl context, but requires Tree-sitter for precision
    }
    else if memmem::find(bytes, b"impl ").is_some()
    {
        FnKind::Impl
    }
    else if memmem::find(bytes, b"trait ").is_some()
    {
        FnKind::Trait
    }
    else if memmem::find(bytes, b"mod ").is_some()
    {
        FnKind::Module
    }
    else
    {
        FnKind::Method
    }
}

/// Extract function/decl name from a matched line using fast byte operations.
/// Supports `fn`, `trait`, `mod`, and robust `impl` forms.
fn extract_function_name(bytes: &[u8]) -> Option<&str>
{
    // Fast paths that always have a space before the identifier
    if let Some(pos) = memmem::find(bytes, b"fn ")
    {
        return take_ident(&bytes[pos + 3..]);
    }
    if let Some(pos) = memmem::find(bytes, b"trait ")
    {
        return take_ident(&bytes[pos + 6..]);
    }
    if let Some(pos) = memmem::find(bytes, b"mod ")
    {
        return take_ident(&bytes[pos + 4..]);
    }

    // `impl` often appears as `impl<T>` (no trailing space). Handle generously.
    if let Some(mut pos) = memmem::find(bytes, b"impl")
    {
        pos += 4; // after `impl`
        // Negative impl: `impl !Trait for Type {` – skip the '!'
        if pos < bytes.len() && bytes[pos] == b'!'
        {
            pos += 1;
        }
        // Skip whitespace
        while pos < bytes.len() && is_ws(bytes[pos])
        {
            pos += 1;
        }
        return extract_impl_name(&bytes[pos..]);
    }

    None
}

/// Extract the primary type name for an `impl` line.
/// Handles:
/// - inherent impls: `impl<T> Type<T> {`
/// - trait impls:   `impl<T> Trait for Type<T> {`
/// - paths:         `impl Trait for crate::a::b::Type<X> {`
fn extract_impl_name(bytes: &[u8]) -> Option<&str>
{
    let n = bytes.len();
    let mut i = 0;

    // 1) Skip leading generics: `<...>` (balanced angle brackets)
    if i < n && bytes[i] == b'<'
    {
        let mut depth: usize = 1;
        i += 1;
        while i < n && depth > 0
        {
            match bytes[i]
            {
                b'<' => depth += 1,
                b'>' => depth = depth.saturating_sub(1),
                _ =>
                {}
            }
            i += 1;
        }
        // Skip whitespace after generics
        while i < n && is_ws(bytes[i])
        {
            i += 1;
        }
    }

    // 2) Look for top-level `for` (only relevant for trait impls)
    let mut depth: usize = 0;
    let mut k = i;
    let mut for_pos: Option<usize> = None;
    while k < n
    {
        match bytes[k]
        {
            b'<' =>
            {
                depth += 1;
                k += 1;
            }
            b'>' =>
            {
                depth = depth.saturating_sub(1);
                k += 1;
            }
            b'f' if depth == 0 =>
            {
                if k + 3 <= n
                    && &bytes[k..k + 3] == b"for"
                    && (k == 0 || is_ws(bytes[k.saturating_sub(1)]))
                    && (k + 3 == n || is_ws(bytes[k + 3]))
                {
                    for_pos = Some(k);
                    break;
                }
                k += 1;
            }
            _ =>
            {
                k += 1;
            }
        }
    }

    // 3) Choose where the *type* begins
    let mut t = if let Some(fp) = for_pos { fp + 3 } else { i };
    while t < n && is_ws(bytes[t])
    {
        t += 1;
    }

    // 4) Skip leading reference/pointer sigils
    while t < n && (bytes[t] == b'&' || bytes[t] == b'*')
    {
        t += 1;
    }
    while t < n && is_ws(bytes[t])
    {
        t += 1;
    }

    // 5) Extract the **base** identifier of the type (last path segment before `<`/ws)
    take_base_type_ident(&bytes[t..])
}

/// Take an identifier directly at the head of the slice (ASCII, `_` + alnum).
fn take_ident(bytes: &[u8]) -> Option<&str>
{
    let mut i = 0;

    while i < bytes.len() && is_ws(bytes[i])
    {
        i += 1;
    }

    let start = i;

    if i < bytes.len() && is_ident_start(bytes[i])
    {
        i += 1;

        while i < bytes.len() && is_ident_continue(bytes[i])
        {
            i += 1;
        }

        return std::str::from_utf8(&bytes[start..i]).ok();
    }

    None
}

/// From a type expression head, return the **last** path segment's identifier.
/// Examples:
/// - `Container<T> {`       -> `Container`
/// - `crate::m::Type<X> {`  -> `Type`
/// - `&'a mut Foo`          -> `Foo`
fn take_base_type_ident(bytes: &[u8]) -> Option<&str>
{
    let n = bytes.len();
    let mut i = 0;

    // Fast skip whitespace
    while i < n && is_ws(bytes[i])
    {
        i += 1;
    }

    // Track the last seen identifier span
    let mut last_s = None;
    let mut last_e = 0usize;

    while i < n
    {
        let b = bytes[i];
        if is_ident_start(b)
        {
            let s = i;
            i += 1;
            while i < n && is_ident_continue(bytes[i])
            {
                i += 1;
            }
            last_s = Some(s);
            last_e = i;
        }
        else if b == b':' && i + 1 < n && bytes[i + 1] == b':'
        {
            // path separator
            i += 2; // keep going; next segment may carry the base name
            continue;
        }
        else
        {
            // Stop on common type terminators or whitespace
            if b == b'<' || b == b'{' || b == b'(' || b == b'[' || b == b',' || is_ws(b)
            {
                break;
            }
            i += 1; // advance over any other punctuation conservatively
        }
    }

    if let Some(s) = last_s
    {
        std::str::from_utf8(&bytes[s..last_e]).ok()
    }
    else
    {
        None
    }
}

#[inline]
fn is_ws(b: u8) -> bool
{
    b == b' ' || b == b'\t' || b == b'\n' || b == b'\r'
}
#[inline]
fn is_ident_start(b: u8) -> bool
{
    b == b'_' || (b as char).is_ascii_alphabetic()
}
#[inline]
fn is_ident_continue(b: u8) -> bool
{
    is_ident_start(b) || (b as char).is_ascii_digit()
}

/// Search within a specific line window for better performance.
fn search_window(
    file_path: &Utf8Path,
    start_line: usize,
    end_line: usize,
    matcher: &RegexMatcher,
    sink: &mut FunctionSink,
) -> Result<()>
{
    let file = File::open(file_path).with_context(|| format!("Failed to open {}", file_path))?;
    let reader = BufReader::new(file);

    let mut windowed_content = Vec::new();
    let mut _line_count = 0; // Track lines processed in window

    for (idx, line) in reader
        .lines()
        .enumerate()
    {
        let line_num = idx + 1;
        if line_num < start_line
        {
            continue;
        }
        if line_num > end_line
        {
            break;
        }

        let line = line?;
        windowed_content.extend_from_slice(line.as_bytes());
        windowed_content.push(b'\n');
        _line_count += 1;
    }

    // Set offset so line numbers are correct
    sink.line_offset = start_line.saturating_sub(1);

    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        // Note: Using default line terminator for cross-platform compatibility
        .build();

    searcher.search_slice(matcher, &windowed_content, sink)?;

    Ok(())
}

/// Calculate distance from a function to a target line.
fn calculate_distance(
    hit: &FnHit,
    line: usize,
) -> usize
{
    if hit.start_line <= line && line <= hit.end_line
    {
        0 // Inside the function
    }
    else if hit.start_line > line
    {
        hit.start_line - line
    }
    else
    {
        line - hit.end_line
    }
}

/// Find the enclosing function for a given line.
#[instrument(skip(root))]
pub fn enclosing_function(
    root: &Utf8Path,
    file: &Utf8Path,
    line: usize,
) -> Result<Option<FnHit>>
{
    let full_path = if file.is_absolute()
    {
        file.to_owned()
    }
    else
    {
        root.join(file)
    };

    trace!("Searching for enclosing function at {}:{}", full_path, line);

    let matcher = FunctionMatcher::new()?;
    let mut sink = FunctionSink {
        hits: IndexMap::new(),
        current_file: file.to_owned(),
        line_offset: 0,
    };

    // Actually use the computed window for performance
    let window_size = 100;
    let start_line = line.saturating_sub(window_size);
    let end_line = line + window_size;

    // Use windowed search instead of full file
    search_window(&full_path, start_line, end_line, &matcher.regex, &mut sink)?;

    // Load file content once for Tree-sitter refinement
    let code = std::fs::read_to_string(&full_path)
        .with_context(|| format!("Failed to read {}", full_path))?;

    // Find the function that contains our line
    let mut best_match: Option<FnHit> = None;
    for (fn_line, hit) in sink
        .hits
        .iter()
    {
        if *fn_line <= line
        {
            // Use Tree-sitter for precise end_line, fallback to heuristic
            let end = ts_function_end_line_for_start(&code, &matcher.lang, *fn_line)
                .or_else(|| estimate_function_end(&full_path, *fn_line).ok())
                .unwrap_or(*fn_line);

            let mut refined_hit = hit.clone();
            refined_hit.end_line = end;

            // Also refine kind if possible
            if let Some(precise_kind) = ts_detect_kind(&code, &matcher.lang, *fn_line)
            {
                refined_hit.kind = precise_kind;
            }

            if refined_hit.end_line >= line
            {
                best_match = Some(refined_hit);
            }
        }
    }

    debug!(
        "Found enclosing function: {:?}",
        best_match
            .as_ref()
            .map(|h| &h.name)
    );
    Ok(best_match)
}

/// Find the k nearest functions to a given line.
#[instrument(skip(root))]
pub fn nearest_functions(
    root: &Utf8Path,
    file: &Utf8Path,
    line: usize,
    k: usize,
) -> Result<Vec<FnHit>>
{
    let full_path = if file.is_absolute()
    {
        file.to_owned()
    }
    else
    {
        root.join(file)
    };

    let matcher = FunctionMatcher::new()?;
    let mut sink = FunctionSink {
        hits: IndexMap::new(),
        current_file: file.to_owned(),
        line_offset: 0,
    };

    let file_handle = File::open(&full_path)?;
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        // Note: Using default line terminator for cross-platform compatibility
        .build();

    searcher.search_file(&matcher.regex, &file_handle, &mut sink)?;

    // Refine end_line for candidates we'll return (fix from review)
    let mut functions: Vec<_> = sink
        .hits
        .into_values()
        .collect();

    // Refine end_line for top candidates
    let top_candidates = std::cmp::min(k * 2, functions.len());
    for hit in functions
        .iter_mut()
        .take(top_candidates)
    {
        // Use Tree-sitter for precise end_line when available
        hit.end_line = estimate_function_end(&full_path, hit.start_line).unwrap_or(hit.start_line);
    }

    // Sort by distance from target line with stable tie-breakers
    functions.sort_by(|a, b| {
        let dist_a = calculate_distance(a, line);
        let dist_b = calculate_distance(b, line);

        dist_a
            .cmp(&dist_b)
            .then_with(|| {
                a.start_line
                    .cmp(&b.start_line)
            })
            .then_with(|| {
                a.name
                    .cmp(&b.name)
            })
    });

    functions.truncate(k);

    debug!("Found {} nearest functions", functions.len());

    Ok(functions)
}

/// Tree-sitter based function end line detection for precise spans.
fn ts_function_end_line_for_start(
    code: &str,
    lang: &tree_sitter::Language,
    start_line_1b: usize,
) -> Option<usize>
{
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(lang)
        .is_err()
    {
        return None;
    }

    let tree = parser.parse(code, None)?;
    let root = tree.root_node();

    // Walk the tree to find function_item nodes
    let mut cursor = root.walk();

    fn find_function_recursive(
        cursor: &mut tree_sitter::TreeCursor,
        start_line_1b: usize,
    ) -> Option<usize>
    {
        loop
        {
            let node = cursor.node();

            if node.kind() == "function_item"
            {
                let node_start_line = node
                    .start_position()
                    .row
                    + 1;
                if node_start_line == start_line_1b
                {
                    return Some(
                        node.end_position()
                            .row
                            + 1,
                    );
                }
            }

            // Recurse into children
            if cursor.goto_first_child()
            {
                if let Some(result) = find_function_recursive(cursor, start_line_1b)
                {
                    return Some(result);
                }
                cursor.goto_parent();
            }

            // Move to next sibling
            if !cursor.goto_next_sibling()
            {
                break;
            }
        }
        None
    }

    find_function_recursive(&mut cursor, start_line_1b)
}

/// Detect precise function kind using Tree-sitter context.
fn ts_detect_kind(
    code: &str,
    lang: &tree_sitter::Language,
    start_line_1b: usize,
) -> Option<FnKind>
{
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(lang)
        .is_err()
    {
        return None;
    }

    let tree = parser.parse(code, None)?;
    let root = tree.root_node();

    let mut cursor = root.walk();

    fn find_function_kind_recursive(
        cursor: &mut tree_sitter::TreeCursor,
        start_line_1b: usize,
    ) -> Option<FnKind>
    {
        loop
        {
            let node = cursor.node();

            if node.kind() == "function_item"
            {
                let node_start_line = node
                    .start_position()
                    .row
                    + 1;
                if node_start_line == start_line_1b
                {
                    // Check parent context for precise classification
                    if let Some(parent) = node.parent()
                    {
                        match parent.kind()
                        {
                            "impl_item" => return Some(FnKind::Method),
                            "trait_item" => return Some(FnKind::Method),
                            _ => return Some(FnKind::Function),
                        }
                    }
                    return Some(FnKind::Function);
                }
            }

            // Recurse into children
            if cursor.goto_first_child()
            {
                if let Some(result) = find_function_kind_recursive(cursor, start_line_1b)
                {
                    return Some(result);
                }
                cursor.goto_parent();
            }

            // Move to next sibling
            if !cursor.goto_next_sibling()
            {
                break;
            }
        }
        None
    }

    find_function_kind_recursive(&mut cursor, start_line_1b)
}

/// Generate smart hints for anchor positioning.
#[instrument(skip(root))]
pub fn hint_anchors(
    root: &Utf8Path,
    file: &Utf8Path,
    line: usize,
) -> Result<AnchorHints>
{
    let full_path = if file.is_absolute()
    {
        file.to_owned()
    }
    else
    {
        root.join(file)
    };

    // Check if file exists
    if !full_path.exists()
    {
        return Ok(AnchorHints::NotAFile {
            path: file.to_owned(),
            reason: "File does not exist".to_string(),
        });
    }

    // Check if it's a file (not directory)
    if !full_path.is_file()
    {
        return Ok(AnchorHints::NotAFile {
            path: file.to_owned(),
            reason: "Path is not a file".to_string(),
        });
    }

    // Try to find enclosing function
    if let Some(func) = enclosing_function(root, file, line)?
    {
        // Improved "Good" classification - include inside function, not just start line
        if func.start_line <= line && line <= func.end_line
        {
            Ok(AnchorHints::Good { function: func })
        }
        else
        {
            let offset = line as isize - func.start_line as isize;
            Ok(AnchorHints::OffByN { requested_line: line, actual: func, offset })
        }
    }
    else
    {
        // Find nearest functions for suggestions
        let nearest = nearest_functions(root, file, line, 3)?;
        Ok(AnchorHints::OutsideScope { requested_line: line, nearest })
    }
}

/// Estimate function end using heuristics (can be refined with Tree-sitter).
fn estimate_function_end(
    file: &Utf8Path,
    start_line: usize,
) -> Result<usize>
{
    // Simple heuristic: scan for closing brace at indent level 0
    // In production, use Tree-sitter for accuracy
    let file = File::open(file)?;
    let reader = BufReader::new(file);
    let mut brace_depth: usize = 0;
    let mut in_function = false;

    for (idx, line) in reader
        .lines()
        .enumerate()
    {
        let line_num = idx + 1;
        if line_num < start_line
        {
            continue;
        }

        let line = line?;

        if line_num == start_line
        {
            in_function = true;
        }

        if in_function
        {
            for ch in line.chars()
            {
                match ch
                {
                    '{' => brace_depth += 1,
                    '}' =>
                    {
                        brace_depth = brace_depth.saturating_sub(1);
                        if brace_depth == 0 && line_num > start_line
                        {
                            return Ok(line_num);
                        }
                    }
                    _ =>
                    {}
                }
            }
        }
    }

    Ok(start_line + 50) // Fallback estimate
}

/// Convert line number to byte offset for SourceSpan (fix for BadAnchorError)
pub fn line_to_byte_offset(
    src: &str,
    line: usize,
) -> Option<(usize, usize)>
{
    let mut current_line = 1;
    let mut line_start = 0;

    for (byte_idx, ch) in src.char_indices()
    {
        if current_line == line
        {
            // Find end of line
            let line_end = src[byte_idx..]
                .find('\n')
                .map(|n| byte_idx + n)
                .unwrap_or(src.len());
            return Some((line_start, line_end));
        }

        if ch == '\n'
        {
            current_line += 1;
            line_start = byte_idx + 1;
        }
    }

    None
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn test_extract_function_name()
    {
        assert_eq!(
            extract_function_name(b"fn hello_world() {"),
            Some("hello_world")
        );
        assert_eq!(
            extract_function_name(b"pub async fn process() -> Result<()>"),
            Some("process")
        );
        assert_eq!(extract_function_name(b"trait MyTrait {"), Some("MyTrait"));
        assert_eq!(
            extract_function_name(b"impl<T> Container<T> {"),
            Some("Container")
        );
    }

    #[test]
    fn test_detect_kind()
    {
        assert_eq!(detect_kind(b"fn test()"), FnKind::Function);
        assert_eq!(detect_kind(b"trait Foo"), FnKind::Trait);
        assert_eq!(detect_kind(b"impl Bar"), FnKind::Impl);
        assert_eq!(detect_kind(b"mod baz"), FnKind::Module);
    }

    #[test]
    fn test_qualified_name_generation()
    {
        let path = Utf8Path::new("src/core/extract.rs");
        assert_eq!(
            build_qualified_name(path, "my_function"),
            "src::core::extract::my_function"
        );

        let mod_path = Utf8Path::new("src/anchor/mod.rs");
        assert_eq!(
            build_qualified_name(mod_path, "detect"),
            "src::anchor::detect"
        );
    }

    #[test]
    fn test_line_to_byte_offset()
    {
        let src = "line 1\nline 2\nline 3\n";
        assert_eq!(line_to_byte_offset(src, 1), Some((0, 6)));
        assert_eq!(line_to_byte_offset(src, 2), Some((7, 13)));
        assert_eq!(line_to_byte_offset(src, 3), Some((14, 20)));
        assert_eq!(line_to_byte_offset(src, 4), None);
    }

    #[test]
    fn test_extract_impl_names()
    {
        assert_eq!(
            extract_function_name(b"impl<T> Container<T> {"),
            Some("Container")
        );
        assert_eq!(
            extract_function_name(b"impl MyTrait for Container<T> {"),
            Some("Container")
        );
        assert_eq!(
            extract_function_name(b"impl<T> MyTrait for crate::m::Type<X> {"),
            Some("Type")
        );
    }
}
