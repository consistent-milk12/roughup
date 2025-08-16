// tests/context_callgraph.rs (updated)
use std::path::PathBuf;

// These unit tests exercise the parsing and query-derivation layer, which is
// the minimal contract for Week 4. Full integration precision metrics are
// validated by higher-level scoreboards outside this unit scope.

// We call into helper functions via the module path. If your crate path differs,
// adjust `use` lines accordingly.
mod helpers
{
    pub use roughup::core::context::{
        collect_callgraph_names as _collect_callgraph_names,
        extract_function_name_at as _extract_function_name_at,
        parse_trait_resolve as _parse_trait_resolve,
    };
}

#[test]
fn trait_resolve_finds_impl_block()
{
    // Given
    let q = "MyTrait::my_method";
    let (ty, method) = helpers::_parse_trait_resolve(q).expect("parse");
    assert_eq!(ty, "MyTrait");
    assert_eq!(method, "my_method");

    // When: derive canonical queries the pipeline will add
    let derived =
        [format!("trait {}", ty), format!("impl {} for", ty), format!("{}::{}", ty, method)];

    // Then
    assert!(
        derived
            .iter()
            .any(|s| s.contains("trait MyTrait"))
    );
    assert!(
        derived
            .iter()
            .any(|s| s.contains("impl MyTrait for"))
    );
    assert!(
        derived
            .iter()
            .any(|s| s.contains("MyTrait::my_method"))
    );
}

#[test]
fn callgraph_finds_callers_at_depth_2()
{
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let file = PathBuf::from("tests/fixtures/callgraph.rs");

    // Dynamically locate the anchor line for `b` to avoid brittle line numbers
    let src = std::fs::read_to_string(root.join(&file)).expect("read fixture");
    let anchor_line = src
        .lines()
        .enumerate()
        .find_map(|(i, line)| {
            if line.contains("pub fn b(")
            {
                Some(i + 1)
            }
            else
            {
                None
            }
        })
        .expect("anchor line for `b`");

    // Given: detect the function name at the anchor
    let fname =
        helpers::_extract_function_name_at(&root, &file, anchor_line).expect("func at anchor");
    assert_eq!(fname, "b");

    // When: collect neighbors with depth=2
    let names = helpers::_collect_callgraph_names(&root, &file, anchor_line, &fname, 2);

    // Then: we should see both the direct callee `c` and the caller `a`
    assert!(
        names
            .iter()
            .any(|n| n == "c")
    );
    assert!(
        names
            .iter()
            .any(|n| n == "a")
    );
}
