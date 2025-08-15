// This file asserts that the --json output is structurally
// equivalent to a machine-readable projection of the default
// human output, when such parity is promised.
// If your CLI prints only JSON when --json is passed, this test
// simply validates the JSON contains essential fields.
use std::process::Command;

use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use serde_json::Value;

// Test: verify essential JSON fields exist and basic types are
// as expected to ensure downstream tooling stability.
#[test]
fn test_json_schema_core_fields()
{
    // Prepare a tiny project fixture in a temp directory.
    let tmp = assert_fs::TempDir::new().expect("tempdir");
    tmp.child("src/lib.rs")
        .write_str("pub fn ok() {}")
        .expect("write");

    // First create symbols index
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Run the CLI in JSON mode with a moderate budget.
    let mut cmd = Command::cargo_bin("rup").expect("bin");
    let out = cmd
        .current_dir(tmp.path())
        .arg("context")
        .arg("ok") // Query for the ok function
        .arg("--json")
        .arg("--budget")
        .arg("600")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    // Parse stdout JSON to a Value for schema checks.
    let v: Value = serde_json::from_slice(&out).expect("json");
    // Check our actual JSON schema: items[] with each having id, content, and tokens
    let items = v
        .get("items")
        .and_then(|i| i.as_array())
        .unwrap();
    assert!(
        !items.is_empty(),
        "items array should contain at least one entry"
    );
    for it in items
    {
        // Each item should have an id string for identification
        assert!(
            it.get("id")
                .and_then(|p| p.as_str())
                .is_some()
        );
        // Each item should have content text
        assert!(
            it.get("content")
                .and_then(|c| c.as_str())
                .is_some()
        );
        // Each item should have token count
        assert!(
            it.get("tokens")
                .and_then(|t| t.as_u64())
                .is_some()
        );
    }

    // Check top-level fields match our schema
    assert!(
        v.get("model")
            .and_then(|m| m.as_str())
            .is_some()
    );
    assert!(
        v.get("budget")
            .and_then(|b| b.as_u64())
            .is_some()
    );
    assert!(
        v.get("total_tokens")
            .and_then(|t| t.as_u64())
            .is_some()
    );
}
