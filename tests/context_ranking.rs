use std::process::Command;

use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use serde_json::Value;

fn make_layout() -> assert_fs::TempDir
{
    let tmp = assert_fs::TempDir::new().expect("tempdir");
    tmp.child("src/core/a.rs")
        .write_str("pub fn a() {}")
        .unwrap();
    tmp.child("src/core/b.rs")
        .write_str("pub fn b() {}")
        .unwrap();
    // can be `fn main() {}` too; we’ll query “main”
    tmp.child("examples/demo/main.rs")
        .write_str("pub fn main() {}")
        .unwrap();
    tmp
}

fn run_context_json(
    tmp: &assert_fs::TempDir,
    queries: &[&str],
    extra_args: &[&str],
) -> Value
{
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    let mut cmd = Command::cargo_bin("rup").expect("bin");
    let out = cmd
        .current_dir(tmp.path())
        .arg("context")
        .args(queries) // <-- pass multiple symbol-name queries
        .args(extra_args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    serde_json::from_slice(&out).expect("valid json")
}

fn extract_ids(v: &Value) -> Vec<String>
{
    v["items"]
        .as_array()
        .expect("items array")
        .iter()
        .filter_map(|it| {
            it.get("id")
                .and_then(|p| p.as_str())
        })
        .filter(|id| *id != "__template__")
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn test_proximity_scope_influence_on_order()
{
    let tmp = make_layout();

    // Query by actual symbol names in each file
    let v = run_context_json(&tmp, &["a", "b", "main"], &[
        "--json",
        "--budget",
        "800",
        "--anchor",
        "src/core/a.rs",
    ]);
    let ids = extract_ids(&v);

    assert!(!ids.is_empty(), "Expected non-empty items, got: {:?}", v);

    // Use relative order (first occurrence) instead of fixed indices
    let ia = ids
        .iter()
        .position(|p| p.contains("src/core/a.rs"))
        .expect("a.rs missing");
    let ib = ids
        .iter()
        .position(|p| p.contains("src/core/b.rs"))
        .expect("b.rs missing");
    let im = ids
        .iter()
        .position(|p| p.contains("examples/demo/main.rs"))
        .expect("main.rs missing");

    assert!(ia < ib, "anchor file should come before sibling: {:?}", ids);
    assert!(
        ib < im,
        "sibling (scope) should come before outside file: {:?}",
        ids
    );
}

#[test]
fn test_scope_bonus_applies_to_file_level_slices()
{
    let tmp = make_layout();

    // Same idea: ensure all three files have matching symbols
    let v = run_context_json(&tmp, &["a", "b", "main"], &[
        "--json",
        "--budget",
        "800",
        "--anchor",
        "src/core/a.rs",
    ]);
    let ids = extract_ids(&v);

    let ia = ids
        .iter()
        .position(|p| p.contains("src/core/a.rs"))
        .expect("a.rs missing");
    let ib = ids
        .iter()
        .position(|p| p.contains("src/core/b.rs"))
        .expect("b.rs missing");
    let im = ids
        .iter()
        .position(|p| p.contains("examples/demo/main.rs"))
        .expect("main.rs missing");

    assert!(ia < ib, "anchor file should rank before sibling: {:?}", ids);
    assert!(ib < im, "sibling should outrank outside file: {:?}", ids);
}
