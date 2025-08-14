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

#[test]
fn test_enhanced_priority_system() {
    use roughup::core::budgeter::{ContextFactors, SymbolRanker};
    use roughup::core::symbols::{Symbol, SymbolKind, Visibility};
    use std::path::PathBuf;

    // Create test symbol
    let symbol = Symbol {
        file: PathBuf::from("src/lib.rs"),
        lang: "rust".to_string(),
        kind: SymbolKind::Function,
        name: "test_function".to_string(),
        qualified_name: "module::test_function".to_string(),
        byte_start: 100,
        byte_end: 200,
        start_line: 10,
        end_line: 15,
        visibility: Some(Visibility::Public),
        doc: None,
    };

    let anchor = PathBuf::from("src/lib.rs");
    let ranker = SymbolRanker::new(Some(&anchor), Some(10));
    let factors = ContextFactors::default();

    let priority = ranker.calculate_priority(&symbol, "test_function", &factors);

    // Should have high relevance due to exact match
    assert_eq!(priority.relevance, 1.0);

    // Should have high proximity due to same file and line
    assert!(priority.proximity > 0.4);

    // Should have elevated level due to public function in anchor file
    assert!(priority.level > 100);
}

#[test]
fn test_priority_backward_compatibility() {
    use roughup::core::budgeter::Priority;

    let high = Priority::high();
    let medium = Priority::medium();
    let low = Priority::low();

    // Test backward compatibility conversion
    let high_u8: u8 = high.into();
    let medium_u8: u8 = medium.into();
    let low_u8: u8 = low.into();

    assert_eq!(high_u8, 2);
    assert_eq!(medium_u8, 1);
    assert_eq!(low_u8, 0);

    // Test ordering is preserved (now deterministic with total_cmp)
    assert!(high > medium);
    assert!(medium > low);
}

#[test]
fn test_deterministic_ordering() {
    use roughup::core::budgeter::Priority;

    let p1 = Priority::custom(100, 0.5, 0.5);
    let p2 = Priority::custom(100, 0.5, 0.5);

    // Equal priorities should compare as equal
    assert_eq!(p1.cmp(&p2), std::cmp::Ordering::Equal);

    // Test NaN safety
    let p_nan = Priority::custom(100, f32::NAN, 0.5);
    assert!(!p_nan.relevance.is_nan()); // Should be sanitized to 0.0
}

#[test]
fn test_budget_expansion_hard_items() {
    use roughup::core::budgeter::{Budgeter, Item, Priority};

    let budgeter = Budgeter::new("gpt-4o").unwrap();

    // Hard item with small min_tokens but large full content
    let hard_item = Item {
        id: "hard1".to_string(),
        content: "fn test() {\n    // This is a longer function with more content\n    println!(\"hello\");\n    println!(\"world\");\n}".to_string(),
        priority: Priority::high(),
        hard: true,
        min_tokens: 5, // Very small minimum
    };

    let budget = 100; // Enough to expand
    let result = budgeter.fit(vec![hard_item], budget).unwrap();

    // Hard item should expand beyond min_tokens when budget allows
    assert!(result.items[0].tokens > 5);
}
