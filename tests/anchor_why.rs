//! Integration tests for anchor validation and why analysis.
//!
//! Tests the `rup anchor --why FILE:LINE` command with various scenarios
//! using insta snapshots for deterministic output verification.

use assert_cmd::prelude::*;
use predicates::prelude::*;
use serde_json::Value;
use std::process::Command;

/// Helper to run anchor command and capture output
fn anchor_cmd() -> Command {
    Command::cargo_bin("rup").expect("rup binary")
}

/// Test anchor validation on function start line - should return Good status
#[test]
fn anchor_good_start_json() {
    let assert = anchor_cmd()
        .args(["--quiet", "anchor", "--why", "src/main.rs:18", "--format", "json"])
        .assert()
        .success();
    let output = assert.get_output();

    // Parse JSON and redact dynamic fields for stable snapshots
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout.find('{').expect("JSON output");
    let json_str = &stdout[json_start..];
    
    let mut v: Value = serde_json::from_str(json_str).expect("valid json");
    
    // Redact confidence which may have minor floating-point variations
    if let Some(function) = v.get_mut("function") {
        if let Some(obj) = function.as_object_mut() {
            obj.insert("confidence".to_string(), Value::String("[redacted]".to_string()));
        }
    }

    insta::assert_yaml_snapshot!(v, @r#"
    factors:
      anchor_validity: perfect
      likely_relevance: 0.95
      structural_importance: high
    function:
      confidence: "[redacted]"
      end_line: 137
      file: src/main.rs
      kind: Function
      name: main
      qualified_name: "src::main::main"
      start_line: 18
    query: "src/main.rs:18"
    reason: Line is inside a function
    requested_line: 18
    schema_version: 1
    status: Good
    "#);
}

/// Test anchor validation inside function body - should return Good status
#[test]
fn anchor_good_inside_text() {
    anchor_cmd()
        .args(["anchor", "--why", "src/main.rs:25", "--format", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Good"));
}

/// Test anchor validation outside any function scope
#[test]
fn anchor_outside_scope_text() {
    anchor_cmd()
        .args(["anchor", "--why", "src/main.rs:1", "--format", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("OutsideScope"));
}

/// Test JSON output for outside scope case
#[test] 
fn anchor_outside_scope_json() {
    let assert = anchor_cmd()
        .args(["--quiet", "anchor", "--why", "src/main.rs:1", "--format", "json"])
        .assert()
        .success();
    let output = assert.get_output();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout.find('{').expect("JSON output");
    let json_str = &stdout[json_start..];
    
    let v: Value = serde_json::from_str(json_str).expect("valid json");

    // Verify schema structure
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["status"], "OutsideScope");
    assert_eq!(v["requested_line"], 1);
    assert!(v["nearest_functions"].is_array());
    assert!(v["function"].is_null());
}

/// Test error handling for non-existent file
#[test]
fn anchor_not_a_file() {
    anchor_cmd()
        .args(["anchor", "--why", "nonexistent.rs:1", "--format", "text"])
        .assert()
        .success() // Command succeeds but reports file issue
        .stdout(predicate::str::contains("NotAFile").or(predicate::str::contains("not exist")));
}

/// Test anchor command help output
#[test]
fn anchor_help() {
    anchor_cmd()
        .args(["anchor", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Validate anchor positions"))
        .stdout(predicate::str::contains("--why"));
}

/// Test context command integration with anchor hints
#[test]
fn context_with_anchor_hints() {
    anchor_cmd()
        .args([
            "--quiet",
            "context", 
            "--anchor", "src/main.rs", 
            "--anchor-line", "18",
            "--hint-anchors",
            "main"
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Anchor validated"));
}
