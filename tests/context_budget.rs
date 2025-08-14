// This file focuses on verifying that the token budgeting logic
// respects the requested cap and trims deterministically.
// We validate by observing JSON counters and by comparing the
// number of returned items under small vs. large budgets.
use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use serde_json::Value;
use std::process::Command;

// Create a larger fixture to force trimming at small budgets.
// We synthesize multiple moderately sized files to exceed tight
// budgets in a predictable way.
fn make_heavy_fixture() -> assert_fs::TempDir {
    // Initialize the temporary project root.
    let tmp = assert_fs::TempDir::new().expect("tempdir");

    // Generate several Rust files with repeated content lines to
    // inflate size while keeping parsing simple and stable.
    for i in 0..8 {
        // Compose a path like src/unit_i.rs for each file.
        let p = format!("src/unit_{i}.rs");

        // Generate repeated functions to approximate token load.
        let mut body = String::new();

        for j in 0..50 {
            // Add content that is code-like to simulate sources.
            body.push_str(&format!(
                "/// unit {i} fn {j}\n\
                 pub fn f_{i}_{j}() {{ /* body */ }}\n"
            ));
        }

        // Write the file into the fixture.
        tmp.child(&p).write_str(&body).expect("write");
    }

    // Include a root README to vary file types.
    tmp.child("README.md")
        .write_str("# Heavy Fixture\n\nDetails.\n")
        .expect("write readme");

    // Return the prepared directory to the caller.
    tmp
}

// Helper: run context with a budget and parse JSON output to a
// serde_json::Value for downstream assertions.
fn run_with_budget(dir: &assert_fs::TempDir, budget: u32) -> Value {
    // First create symbols index
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(dir.path())
        .arg("symbols")
        .assert()
        .success();

    // Launch the binary with the requested budget.
    let mut cmd = Command::cargo_bin("rup").expect("bin");

    let out = cmd
        .current_dir(dir.path())
        .arg("context")
        .arg("f_") // Query for functions starting with f_
        .arg("--json")
        .arg("--budget")
        .arg(budget.to_string())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // Parse JSON from stdout.
    serde_json::from_slice(&out).expect("json")
}

// Test: smaller budgets must not exceed the requested cap and
// should include fewer items than larger budgets.
#[test]
fn test_budget_enforcement_and_trimming() {
    // Build a heavy fixture that will trigger trimming.
    let tmp = make_heavy_fixture();

    // Run with a very small budget likely to trim aggressively.
    let small = run_with_budget(&tmp, 200);

    // Run with a larger budget that fits more content.
    let large = run_with_budget(&tmp, 2000);

    // Extract items arrays for both runs.
    let small_items = small.get("items").unwrap().as_array().unwrap();
    let large_items = large.get("items").unwrap().as_array().unwrap();

    // Ensure the larger budget yields at least as many items.
    assert!(
        large_items.len() >= small_items.len(),
        "larger budget should include >= items"
    );

    // If the schema reports total_tokens, assert it respects cap.
    if let Some(tt) = small.get("total_tokens").and_then(|v| v.as_u64()) {
        // Expect the reported token count to be <= requested cap.
        assert!(
            tt <= 200,
            "reported tokens {tt} should not exceed budget 200"
        );
    }
}

// Test: trimming must be deterministic — two runs at the same
// small budget should return identical JSON payloads.
#[test]
fn test_budget_trimming_deterministic() {
    // Create heavy test fixture content.
    let tmp = make_heavy_fixture();

    // First create symbols index
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Run once and capture stdout as string.
    let mut cmd = Command::cargo_bin("rup").expect("bin");
    let a = cmd
        .current_dir(tmp.path())
        .arg("context")
        .arg("f_") // Query for functions starting with f_
        .arg("--json")
        .arg("--budget")
        .arg("180")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    // Run a second time under identical conditions.
    let mut cmd2 = Command::cargo_bin("rup").expect("bin");
    let b = cmd2
        .current_dir(tmp.path())
        .arg("context")
        .arg("f_") // Same query
        .arg("--json")
        .arg("--budget")
        .arg("180")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    // Compare exact bytes for determinism.
    assert_eq!(a, b, "trimming must be stable across runs");
}
