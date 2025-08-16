//  File: tests/call_distance.rs

//  Bring in filesystem helpers.
use std::fs;
//  Create and join paths for a temp repo.
use std::path::Path;

//  Import the functions we just added.
use roughup::context::{CallGraph, CallGraphHopper};

//  Basic monotonicity checks for the hop→affinity transform.
#[test]
fn test_call_distance_decay_monotone_01()
{
    //  hop 0 must have maximal affinity.
    assert!(
        CallGraphHopper::call_distance_from_hop(0) > CallGraphHopper::call_distance_from_hop(1)
    );

    //  hop 1 must exceed hop 2.
    assert!(
        CallGraphHopper::call_distance_from_hop(1) > CallGraphHopper::call_distance_from_hop(2)
    );

    //  hop 2 must exceed hop 3.
    assert!(
        CallGraphHopper::call_distance_from_hop(2) > CallGraphHopper::call_distance_from_hop(3)
    );
}

//  Construct a tiny repo with two functions where `a` calls `b`,
//  then verify hop discovery and scoring behavior.
#[test]
fn test_min_hop_and_score_by_fn_and_span_02()
{
    //  Create a temp directory to act as the repo root.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    //  Create a single Rust file with two functions.
    let file = root.join("mini.rs");
    let src = r#"
        fn b() { }
        fn a() {
            b();
        }
    "#;

    //  Write the source to disk.
    fs::write(&file, src).unwrap();

    //  Anchor inside function `a` (line numbers are 1-based).
    //  The snippet puts `fn b` on line 2 and `fn a` on line 3.
    let anchor_line = 3usize;

    //  Extract the anchor function name to validate assumptions.
    let anchor_fn = CallGraph::extract_function_name_at(root, Path::new("mini.rs"), anchor_line)
        .expect("expected to find anchor fn");
    assert_eq!(anchor_fn, "a");

    //  Collect hop distances with depth 1.
    let hops = CallGraphHopper::collect_callgraph_hops(
        root,
        Path::new("mini.rs"),
        anchor_line,
        &anchor_fn,
        1,
    );

    //  `a` should be present at hop 0.
    assert_eq!(
        hops.get("a")
            .copied(),
        Some(0)
    );
    //  `b` should be discovered at hop 1.
    assert_eq!(
        hops.get("b")
            .copied(),
        Some(1)
    );

    //  Score by function name with weight 0.10 (clamped later).
    let s_a = CallGraphHopper::score_from_call_distance_for_fn("a", &hops, 0.10);
    let s_b = CallGraphHopper::score_from_call_distance_for_fn("b", &hops, 0.10);
    //  Anchor `a` should score higher than `b`.
    assert!(s_a > s_b);
    //  Unknown function should contribute zero.
    let s_x = CallGraphHopper::score_from_call_distance_for_fn("x", &hops, 0.10);
    assert_eq!(s_x, 0.0);

    //  Now score by span (derive owner function at each line).
    let s_span_a = CallGraphHopper::score_from_call_distance_for_span(
        root,
        Path::new("mini.rs"),
        3, //  line inside `a`
        &hops,
        0.10,
    );

    let s_span_b = CallGraphHopper::score_from_call_distance_for_span(
        root,
        Path::new("mini.rs"),
        2, //  line inside `b`
        &hops,
        0.10,
    );

    assert!(s_span_a > s_span_b);
}
