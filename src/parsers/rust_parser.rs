//! Filepath: src/parsers/rust_extractor.rs

use anyhow::{Context, Result};
use std::path::Path;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::core::symbols::{
    Symbol, SymbolExtractor, SymbolKind, Visibility, build_qualified_name, parse_visibility,
};
// Reuse the shared helper to avoid drift
use crate::infra::utils::TsNodeUtils;

pub struct RustExtractor {
    language: Language,
    // One resilient query that captures item *nodes* only; we compute names/vis later.
    items_query: Query,
}

impl RustExtractor {
    pub fn new() -> Result<Self> {
        let language = tree_sitter_rust::LANGUAGE.into();

        // Avoid field-name constraints like `visibility:` which differ across grammar versions.
        // We capture the item nodes themselves (and inner function_item inside impl/trait for methods).
        let items_query_src = r#"
            (function_item) @function
            (struct_item)   @struct
            (enum_item)     @enum
            (trait_item)    @trait
            (type_item)     @type_alias
            (const_item)    @constant
            (static_item)   @static
            (mod_item)      @module

            (impl_item
              (declaration_list (function_item) @method))
            (trait_item
              (declaration_list (function_item) @trait_method))
        "#;

        let items_query =
            Query::new(&language, items_query_src).context("create Rust items query")?;
        Ok(Self {
            language,
            items_query,
        })
    }
}

impl SymbolExtractor for RustExtractor {
    fn extract_symbols(&self, content: &str, file_path: &Path) -> Result<Vec<Symbol>> {
        let mut parser = Parser::new();
        parser.set_language(&self.language)?;

        let tree = parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse Rust source"))?;
        let bytes = content.as_bytes();

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.items_query, tree.root_node(), bytes);

        let cap_names: Vec<&str> = self.items_query.capture_names().to_vec();

        let mut out = Vec::new();

        while let Some(m) = matches.next() {
            let mut picked: Option<(&str, Node)> = None;

            for cap in m.captures {
                let cname = cap_names[cap.index as usize];

                if matches!(
                    cname,
                    "function"
                        | "struct"
                        | "enum"
                        | "trait"
                        | "type_alias"
                        | "constant"
                        | "static"
                        | "module"
                        | "method"
                        | "trait_method"
                ) {
                    picked = Some((cname, cap.node));
                    break;
                }
            }

            let Some((cname, node)) = picked else {
                continue;
            };

            let is_in_impl = TsNodeUtils::has_ancestor(node, "impl_item");
            let is_in_trait = TsNodeUtils::has_ancestor(node, "trait_item");

            let kind = match (cname, is_in_impl, is_in_trait) {
                ("function", false, false) => Some(SymbolKind::Function),

                ("method" | "trait_method", _, _) => Some(SymbolKind::Method),

                // Skip: Treated as method
                ("function", true, _) => None,

                // Skip: Treated as method
                ("function", _, true) => None,

                ("struct", ..) => Some(SymbolKind::Struct),

                ("enum", ..) => Some(SymbolKind::Enum),

                ("trait", ..) => Some(SymbolKind::Trait),

                ("type_alias", ..) => Some(SymbolKind::TypeAlias),

                ("constant", ..) => Some(SymbolKind::Constant),

                ("static", ..) => Some(SymbolKind::Variable),

                ("module", ..) => Some(SymbolKind::Module),

                _ => None,
            };

            if let Some(kind) = kind
                && let Some(sym) = build_symbol(kind, node, bytes, file_path)
            {
                out.push(sym);
            }
        }

        Ok(out)
    }
}

fn build_symbol(kind: SymbolKind, node: Node, bytes: &[u8], file: &Path) -> Option<Symbol> {
    let name = name_of(node, bytes)?;
    let visibility = visibility_of(node, bytes);

    // Qualified name assembly:
    // - Methods: use owner from enclosing impl/trait.
    // - Others: prefix with enclosing module path (crate::a::b::Name).
    let qualified_name = if kind == SymbolKind::Method {
        if let Some(owner) = owner_of_method(node, bytes) {
            build_qualified_name(&[&owner, &name])
        } else {
            name.clone()
        }
    } else {
        let module_path = enclosing_module_path(node, bytes);
        if module_path.is_empty() {
            name.clone()
        } else {
            build_qualified_name(&[&module_path, &name])
        }
    };

    let doc = gather_leading_rust_docs(node, bytes);

    let start = node.start_position();
    let end = node.end_position();

    Some(Symbol {
        file: file.to_path_buf(),
        lang: "rust".to_string(),
        kind,
        name,
        qualified_name,
        byte_start: node.start_byte(),
        byte_end: node.end_byte(),
        start_line: start.row + 1,
        end_line: end.row + 1,
        visibility,
        doc,
    })
}

fn first_named_child_text<'a>(node: Node<'a>, bytes: &[u8], kinds: &[&str]) -> Option<String> {
    for i in 0..node.named_child_count() {
        let c = node.named_child(i)?;
        if kinds.contains(&c.kind()) {
            return Some(c.utf8_text(bytes).ok()?.to_string());
        }
    }
    None
}

fn name_of(node: Node, bytes: &[u8]) -> Option<String> {
    if let Some(n) = node.child_by_field_name("name") {
        return n.utf8_text(bytes).ok().map(|s| s.to_string());
    }
    // Fallbacks for common declaration shapes
    first_named_child_text(node, bytes, &["identifier", "type_identifier"])
}

fn visibility_of(node: Node, bytes: &[u8]) -> Option<Visibility> {
    for i in 0..node.named_child_count() {
        let c = node.named_child(i)?;
        if c.kind() == "visibility_modifier"
            && let Ok(t) = c.utf8_text(bytes)
        {
            return parse_visibility(t);
        }
    }
    None
}

fn owner_of_method(mut node: Node, bytes: &[u8]) -> Option<String> {
    // Walk up to impl_item or trait_item, then extract the type or trait name.
    while let Some(p) = node.parent() {
        match p.kind() {
            "impl_item" => {
                if let Some(t) = p.child_by_field_name("type") {
                    return t.utf8_text(bytes).ok().map(|s| s.to_string());
                }
                // Fallbacks to get something readable like `u32`, `a::b::Baz<T>`, etc.
                return first_named_child_text(
                    p,
                    bytes,
                    &[
                        "type_identifier",
                        "scoped_type_identifier",
                        "generic_type",
                        "primitive_type",
                        "tuple_type",
                        "reference_type",
                    ],
                );
            }
            "trait_item" => {
                if let Some(n) = p.child_by_field_name("name") {
                    return n.utf8_text(bytes).ok().map(|s| s.to_string());
                }
                return first_named_child_text(p, bytes, &["type_identifier"]);
            }
            _ => {
                node = p;
            }
        }
    }
    None
}

fn enclosing_module_path(mut node: Node, bytes: &[u8]) -> String {
    let mut parts = Vec::new();
    while let Some(parent) = node.parent() {
        if parent.kind() == "mod_item"
            && let Some(name_node) = parent.child_by_field_name("name")
            && let Ok(t) = name_node.utf8_text(bytes)
            && !t.is_empty()
        {
            parts.push(t.to_string());
        }
        node = parent;
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(
            "crate::{}",
            parts.into_iter().rev().collect::<Vec<_>>().join("::")
        )
    }
}

fn gather_leading_rust_docs(node: Node, bytes: &[u8]) -> Option<String> {
    let mut acc: Vec<String> = Vec::new();
    let mut cur = node;
    while let Some(prev) = cur.prev_sibling() {
        match prev.kind() {
            "line_comment" => {
                let t = prev.utf8_text(bytes).unwrap_or_default().trim();
                if let Some(stripped) = t.strip_prefix("///") {
                    acc.push(stripped.trim().to_string());
                    cur = prev;
                    continue;
                }
            }
            "block_comment" => {
                let t = prev.utf8_text(bytes).unwrap_or_default();
                if t.starts_with("/**") {
                    let body = t.trim_start_matches("/**").trim_end_matches("*/").trim();
                    if !body.is_empty() {
                        acc.push(body.to_string());
                    }
                    cur = prev;
                    continue;
                }
            }
            _ => {}
        }
        break;
    }
    if acc.is_empty() {
        None
    } else {
        acc.reverse();
        Some(acc.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::core::symbols::{Symbol, SymbolKind};

    fn has(sym: &Symbol, kind: SymbolKind, name: &str) -> bool {
        sym.kind == kind && sym.name == name
    }

    fn get<'a>(syms: &'a [Symbol], kind: SymbolKind, name: &str) -> &'a Symbol {
        syms.iter()
            .find(|s| has(s, kind.clone(), name))
            .expect("symbol not found")
    }

    #[test]
    fn rust_functions_and_docs() -> Result<()> {
        let extractor = RustExtractor::new()?;
        let src = r#"
/// First Line
/// Second Line
pub fn hello_world() {}

fn private_fn() {}
"#;
        let file = PathBuf::from("test.rs");
        let mut syms = extractor.extract_symbols(src, &file)?;
        syms.sort_by_key(|s| (s.start_line, s.name.clone()));

        let pub_fn = get(&syms, SymbolKind::Function, "hello_world");
        assert_eq!(pub_fn.doc.as_deref(), Some("First Line\nSecond Line"));
        assert!(pub_fn.start_line >= 1 && pub_fn.end_line >= pub_fn.start_line);

        let priv_fn = get(&syms, SymbolKind::Function, "private_fn");
        assert!(priv_fn.doc.is_none());

        Ok(())
    }

    #[test]
    fn rust_struct_and_docs() -> Result<()> {
        let extractor = RustExtractor::new()?;
        let src = r#"
/**
Block doc A
Block doc B
*/
struct S;
"#;
        let file = PathBuf::from("test.rs");
        let syms = extractor.extract_symbols(src, &file)?;
        let s = get(&syms, SymbolKind::Struct, "S");
        assert_eq!(s.doc.as_deref(), Some("Block doc A\nBlock doc B"));
        Ok(())
    }

    #[test]
    fn symbol_kinds_covered() -> Result<()> {
        let extractor = RustExtractor::new()?;
        let src = r#"
struct A;
enum E { X }
trait T {}
type Alias = u32;
const C: u8 = 1;
static S0: i32 = 0;
mod m {}
"#;
        let file = PathBuf::from("test.rs");
        let syms = extractor.extract_symbols(src, &file)?;
        assert!(syms.iter().any(|s| has(s, SymbolKind::Struct, "A")));
        assert!(syms.iter().any(|s| has(s, SymbolKind::Enum, "E")));
        assert!(syms.iter().any(|s| has(s, SymbolKind::Trait, "T")));
        assert!(syms.iter().any(|s| has(s, SymbolKind::TypeAlias, "Alias")));
        assert!(syms.iter().any(|s| has(s, SymbolKind::Constant, "C")));
        assert!(syms.iter().any(|s| has(s, SymbolKind::Variable, "S0")));
        assert!(syms.iter().any(|s| has(s, SymbolKind::Module, "m")));
        Ok(())
    }

    #[test]
    fn impl_methods_and_qualification_variants() -> Result<()> {
        let extractor = RustExtractor::new()?;
        let src = r#"
mod a { pub mod b { pub struct Baz<T>(T); } }
impl<T> a::b::Baz<T> {
    pub fn y() {} 
    fn z() {}
}
impl u32 {
    fn x() {}
}
"#;
        let file = PathBuf::from("test.rs");
        let syms = extractor.extract_symbols(src, &file)?;

        let y = syms
            .iter()
            .find(|s| s.kind == SymbolKind::Method && s.name == "y")
            .unwrap();
        assert!(y.qualified_name.contains("Baz"));

        let z = syms
            .iter()
            .find(|s| s.kind == SymbolKind::Method && s.name == "z")
            .unwrap();
        assert!(z.qualified_name.contains("Baz"));

        let x = syms
            .iter()
            .find(|s| s.kind == SymbolKind::Method && s.name == "x")
            .unwrap();
        assert!(x.qualified_name.contains("u32"));
        Ok(())
    }

    #[test]
    fn trait_default_methods() -> Result<()> {
        let extractor = RustExtractor::new()?;
        let src = r#"
trait MyTrait {
    fn defaulted() {}
}
"#;
        let file = PathBuf::from("test.rs");
        let syms = extractor.extract_symbols(src, &file)?;
        let m = syms
            .iter()
            .find(|s| s.kind == SymbolKind::Method && s.name == "defaulted")
            .unwrap();
        assert!(m.qualified_name.contains("MyTrait"));
        Ok(())
    }

    #[test]
    fn nested_modules_qualification() -> Result<()> {
        let extractor = RustExtractor::new()?;
        let src = r#"
mod a { mod b { fn f() {} } }
"#;
        let file = PathBuf::from("test.rs");
        let syms = extractor.extract_symbols(src, &file)?;
        let f = get(&syms, SymbolKind::Function, "f");
        assert!(f.qualified_name.ends_with("crate::a::b::f"));
        Ok(())
    }
}
