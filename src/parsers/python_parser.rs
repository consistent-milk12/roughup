//! Filepath: src/parsers/python_parser.rs
//! ------------------------------------------------------------------
//! Python symbol extractor built on Tree-sitter 0.25.x.
//! Goals:
//!   - Use broad, stable queries (no fragile field predicates).
//!   - Classify methods by ancestry (avoid duplicate matches).
//!   - Extract PEP 257 docstrings (first statement string).
//!   - Build qualified names for methods (A::B::m).
//!   - Be careful with allocations and streaming iteration.
//!
//! Notes:
//!   - We only query for functions and classes. Methods are
//!     determined by detecting a surrounding class_definition.
//!   - We always pass the same byte slice that Parser parsed.
//!   - We rely on tree_sitter::StreamingIterator for matches.
//!   - Docstrings support single/triple quotes and common
//!     prefixes (r, u, f, fr, rf). Dedent is applied for
//!     triple-quoted docs. Concatenated string docstrings are
//!     joined segment-wise.
//! ------------------------------------------------------------------

use anyhow::{Context, Result, anyhow};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::core::symbols::{Symbol, SymbolExtractor, SymbolKind, Visibility};

/// Extracts Python symbols (functions, classes, methods).
pub struct PythonExtractor {
    /// Python language handle for Tree-sitter.
    language: Language,
    /// Broad, stable query capturing defs and class defs.
    query: Query,
}

impl PythonExtractor {
    /// Construct a new extractor with a broad query that
    /// captures function_definition and class_definition.
    pub fn new() -> Result<Self> {
        // Obtain the Tree-sitter language for Python.
        let language = tree_sitter_python::LANGUAGE.into();

        // Keep queries broad; avoid grammar field predicates
        // that tend to change across minor versions.
        let query_src = r#"
            (function_definition
              name: (identifier) @name) @item

            (class_definition
              name: (identifier) @name) @item
        "#;

        // Compile the query once for reuse in extraction.
        let query = Query::new(&language, query_src).context("create Python query")?;

        Ok(Self { language, query })
    }
}

impl SymbolExtractor for PythonExtractor {
    /// Parse `content`, run the query, derive symbol data, and
    /// return a flat list of symbols defined in the file.
    fn extract_symbols(&self, content: &str, file_path: &Path) -> Result<Vec<Symbol>> {
        // Create a parser instance and set the language.
        let mut parser = Parser::new();
        parser
            .set_language(&self.language)
            .context("set Python language")?;

        // Parse the source; fail if no tree is produced.
        let tree = parser
            .parse(content, None)
            .ok_or_else(|| anyhow!("Failed to parse Python source"))?;

        // Use the same bytes slice for all utf8_text calls.
        let bytes = content.as_bytes();

        // Prepare a query cursor and stream matches.
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), bytes);

        // Capture names vector for fast index lookup.
        let cap_names: Vec<&str> = self.query.capture_names().to_vec();

        // Pre-allocate a small buffer to reduce reallocations.
        let mut out = Vec::with_capacity(16);

        // Iterate streaming matches properly with .next().
        while let Some(m) = matches.next() {
            // Selected captured node of interest and its name.
            let mut picked: Option<Node> = None;
            let mut name_text: Option<String> = None;

            // Process all captures in this match.
            for cap in m.captures {
                // Map capture index to its name string.
                let cname = cap_names[cap.index as usize];

                // The structural node (function/class) is @item.
                if cname == "item" {
                    picked = Some(cap.node);
                    continue;
                }

                // The identifier for the symbol is @name.
                if cname == "name" {
                    name_text = cap.node.utf8_text(bytes).ok().map(|s| s.to_string());
                    continue;
                }
            }

            // Skip malformed matches lacking structure or name.
            let Some(node) = picked else { continue };
            let Some(name) = name_text else { continue };

            // Classify as Method if nested in a class; else
            // Function for function_definition or Class for
            // class_definition. Avoid duplicates by not having
            // a separate "method" query pattern.
            let kind = match node.kind() {
                "function_definition" => {
                    if has_ancestor(node, "class_definition") {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    }
                }
                "class_definition" => SymbolKind::Class,
                _ => continue,
            };

            // Build a qualified name. For methods we climb the
            // class chain. For top-level items, keep simple name.
            let qualified_name = if matches!(kind, SymbolKind::Method) {
                python_qualified_name_method(node, bytes, &name)
            } else {
                name.clone()
            };

            // Python visibility by leading underscore policy.
            let visibility = if name.starts_with('_') {
                Some(Visibility::Private)
            } else {
                Some(Visibility::Public)
            };

            // Compute line and byte spans.
            let start = node.start_position();
            let end = node.end_position();

            // Collect PEP 257-style docstring where present.
            let doc = python_docstring_extract(node, bytes);

            // Push the assembled symbol entry.
            out.push(Symbol {
                file: file_path.to_path_buf(),
                lang: "python".to_string(),
                kind,
                name,
                qualified_name,
                byte_start: node.start_byte(),
                byte_end: node.end_byte(),
                start_line: start.row + 1,
                end_line: end.row + 1,
                visibility,
                doc,
            });
        }

        // Return the final symbol list.
        Ok(out)
    }
}

/// Build qualified method names of the form
/// `Outer::Inner::method`, climbing ancestor classes.
fn python_qualified_name_method(mut node: Node, bytes: &[u8], method_name: &str) -> String {
    // Start with the innermost method name.
    let mut parts: Vec<String> = vec![method_name.to_string()];

    // Walk parents and collect class_definition names.
    while let Some(parent) = node.parent() {
        if parent.kind() == "class_definition"
            && let Some(name_node) = parent.child_by_field_name("name")
            && let Ok(cls) = name_node.utf8_text(bytes)
        {
            parts.push(cls.to_string());
        }
        node = parent;
    }

    // Reverse to outer-to-inner order and join.
    parts.reverse();
    parts.join("::")
}

/// Extract a PEP 257 docstring from a function/class:
/// first statement in the body must be a string literal.
/// Supports single/triple quotes, r/u/f prefixes, and
/// concatenated string sequences.
fn python_docstring_extract(node: Node, bytes: &[u8]) -> Option<String> {
    // Obtain the block/suite node that contains statements.
    let body = node.child_by_field_name("body")?;

    // In current grammar, the body node itself is a "block"
    // (or "suite" in older variants). Use it directly.
    let block = if body.kind() == "block" || body.kind() == "suite" {
        body
    } else {
        // Fallback: some grammars may nest blocks; try first
        // named child that is a block/suite.
        let mut blk = None;
        for i in 0..body.named_child_count() {
            let c = body.named_child(i)?;
            if c.kind() == "block" || c.kind() == "suite" {
                blk = Some(c);
                break;
            }
        }
        blk?
    };

    // Grab the first *named* statement (skips newlines/indent).
    let first_stmt = block.named_child(0)?;
    if first_stmt.kind() != "expression_statement" {
        return None;
    }

    // The first expression should be a string literal or a
    // concatenated_string (implicit adjacent literal concat).
    let lit = first_stmt.named_child(0)?;
    match lit.kind() {
        "string" => {
            let raw = lit.utf8_text(bytes).ok()?;
            let text = unquote_python_string(raw);
            Some(text)
        }
        "concatenated_string" => {
            // Join each string segment after unquoting.
            let mut acc = String::new();
            for i in 0..lit.named_child_count() {
                let seg = lit.named_child(i)?;
                if seg.kind() != "string" {
                    // Non-string in concatenation invalidates
                    // docstring per PEP 257 expectations.
                    return None;
                }
                let raw = seg.utf8_text(bytes).ok()?;
                acc.push_str(&unquote_python_string(raw));
            }
            if acc.is_empty() { None } else { Some(acc) }
        }
        _ => None,
    }
}

/// Strip Python string prefixes/quotes and perform a light
/// unescape plus dedent for triple-quoted strings.
fn unquote_python_string(s: &str) -> String {
    // Trim leading/trailing whitespace around the literal.
    let ss = s.trim();

    // Compute prefix length (r, u, f, fr, rf; case-insensitive).
    let pref_len = leading_alpha_len(ss);
    let (prefix, rest) = ss.split_at(pref_len);

    // Determine if raw (contains 'r' or 'R').
    let is_raw = prefix.chars().any(|c| c == 'r' || c == 'R');

    // Work with the remainder for quote detection.
    let s2 = rest;

    // Handle triple quotes first.
    if s2.len() >= 6 {
        if s2.starts_with(r#"""""#) && s2.ends_with(r#"""""#) {
            let inner = &s2[3..s2.len() - 3];
            return dedent_and_unescape(inner, is_raw);
        }
        if s2.starts_with("'''") && s2.ends_with("'''") {
            let inner = &s2[3..s2.len() - 3];
            return dedent_and_unescape(inner, is_raw);
        }
    }

    // Handle single-quoted strings.
    if s2.len() >= 2
        && ((s2.starts_with('"') && s2.ends_with('"'))
            || (s2.starts_with('\'') && s2.ends_with('\'')))
    {
        let inner = &s2[1..s2.len() - 1];
        return basic_unescape(inner, is_raw);
    }

    // Fallback: return as-is.
    s2.to_string()
}

/// Return the count of leading ASCII alphabetic chars.
/// Used to slice off string literal prefixes.
fn leading_alpha_len(s: &str) -> usize {
    let mut i = 0;
    for ch in s.chars() {
        if ch.is_ascii_alphabetic() {
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    i
}

/// Dedent triple-quoted content and unescape if not raw.
/// Also strips a single leading/trailing blank line.
fn dedent_and_unescape(s: &str, is_raw: bool) -> String {
    // Split into lines and drop symmetric blank edges.
    let mut lines: Vec<&str> = s.lines().collect();
    if !lines.is_empty() && lines[0].trim().is_empty() {
        lines.remove(0);
    }
    if !lines.is_empty() && lines[lines.len() - 1].trim().is_empty() {
        lines.pop();
    }

    // Compute common leading spaces across non-empty lines.
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ').count())
        .min()
        .unwrap_or(0);

    // Dedent and join with newlines.
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if !out.is_empty() {
            out.push('\n');
        }
        if l.len() >= indent {
            out.push_str(&l[indent..]);
        } else {
            out.push_str(l);
        }
        // Continue to next line.
        let _ = i;
    }

    // Apply basic unescape only if not raw.
    if is_raw {
        out
    } else {
        basic_unescape(&out, false)
    }
}

/// Minimal unescape for common sequences when not raw.
/// Intended for docstrings, not general Python parsing.
fn basic_unescape(s: &str, is_raw: bool) -> String {
    if is_raw {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            if let Some(n) = it.next() {
                match n {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    _ => {
                        out.push('\\');
                        out.push(n);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Return true if `node` has an ancestor of the given kind.
fn has_ancestor(mut node: Node, kind: &str) -> bool {
    while let Some(p) = node.parent() {
        if p.kind() == kind {
            return true;
        }
        node = p;
    }
    false
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::core::symbols::{Symbol, SymbolKind, Visibility};

    /// Helper: predicate for kind+name match.
    fn has(sym: &Symbol, kind: SymbolKind, name: &str) -> bool {
        sym.kind == kind && sym.name == name
    }

    /// Helper: fetch a single symbol by kind+name.
    fn get<'a>(syms: &'a [Symbol], kind: SymbolKind, name: &str) -> &'a Symbol {
        syms.iter()
            .find(|s| has(s, kind.clone(), name))
            .expect("symbol not found")
    }

    #[test]
    fn python_functions_public_private_and_docstring() -> Result<()> {
        let ex = PythonExtractor::new()?;
        let src = r#"
def hello():
    """Greeting"""
    return 1

def _hidden():
    return 2
"#;
        let file = PathBuf::from("test.py");
        let mut syms = ex.extract_symbols(src, &file)?;
        syms.sort_by_key(|s| (s.start_line, s.name.clone()));

        let hello = get(&syms, SymbolKind::Function, "hello");
        assert_eq!(hello.visibility, Some(Visibility::Public));
        assert_eq!(hello.doc.as_deref(), Some("Greeting"));

        let hidden = get(&syms, SymbolKind::Function, "_hidden");
        assert_eq!(hidden.visibility, Some(Visibility::Private));
        Ok(())
    }

    #[test]
    fn python_class_and_methods_with_qualified_names() -> Result<()> {
        let ex = PythonExtractor::new()?;
        let src = r#"
class MyClass:
    """C doc"""
    def method(self):
        """M doc"""
        pass

    def _private(self):
        pass
"#;
        let file = PathBuf::from("t.py");
        let syms = ex.extract_symbols(src, &file)?;

        assert!(syms.iter().any(|s| has(s, SymbolKind::Class, "MyClass")));

        let m = syms
            .iter()
            .find(|s| s.kind == SymbolKind::Method && s.name == "method")
            .unwrap();
        assert_eq!(m.qualified_name, "MyClass::method");
        assert_eq!(m.doc.as_deref(), Some("M doc"));

        let p = syms
            .iter()
            .find(|s| s.kind == SymbolKind::Method && s.name == "_private")
            .unwrap();
        assert_eq!(p.visibility, Some(Visibility::Private));
        Ok(())
    }

    #[test]
    fn python_nested_classes_qualified_names() -> Result<()> {
        let ex = PythonExtractor::new()?;
        let src = r#"
class Outer:
    class Inner:
        def m(self):
            pass
"#;
        let file = PathBuf::from("t.py");
        let syms = ex.extract_symbols(src, &file)?;
        let m = syms
            .iter()
            .find(|s| s.kind == SymbolKind::Method && s.name == "m")
            .unwrap();
        assert_eq!(m.qualified_name, "Outer::Inner::m");
        Ok(())
    }

    #[test]
    fn python_non_first_string_is_not_docstring() -> Result<()> {
        let ex = PythonExtractor::new()?;
        let src = r#"
def f():
    x = 1
    "not a docstring"
    return x
"#;
        let file = PathBuf::from("t.py");
        let syms = ex.extract_symbols(src, &file)?;
        let f = get(&syms, SymbolKind::Function, "f");
        assert!(f.doc.is_none());
        Ok(())
    }
}
