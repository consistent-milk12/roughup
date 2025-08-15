//! Tests for tier preset mapping and precedence
//!
//! Validates that --tier A/B/C sets appropriate defaults
//! and that explicit --budget and --limit override tier presets

use std::process::Command;

use assert_cmd::prelude::*; // Import AssertCmd traits
use serde_json::Value; // JSON parsing for CLI output // Spawn the binary

// Reuse existing helper fixture for consistency
mod util;
use util::make_heavy_fixture; // Bring fixture into scope

/// Verify that --tier A sets the JSON 'budget' to 1200 by default
#[test]
fn test_tier_a_sets_budget_1200()
{
    // Create a representative test workspace
    let tmp = make_heavy_fixture();

    // Ensure symbols index is generated
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Run context with --tier A and JSON output
    let out = Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("context")
        .arg("f_") // Use a permissive query
        .arg("--tier")
        .arg("A") // Apply preset A
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // Parse emitted JSON and assert the preset budget
    let v: Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(v["budget"], 1200, "tier A must set budget=1200");
    assert_eq!(v["tier"], "A", "tier label must be present in JSON");
    assert_eq!(
        v["effective_limit"], 96,
        "tier A must set effective_limit=96"
    );
    assert_eq!(
        v["effective_top_per_query"], 6,
        "tier A must set effective_top_per_query=6"
    );
}

/// Verify that --tier B sets the JSON 'budget' to 3000 by default
#[test]
fn test_tier_b_sets_budget_3000()
{
    // Create a representative test workspace
    let tmp = make_heavy_fixture();

    // Ensure symbols index is generated
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Run context with --tier B and JSON output
    let out = Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("context")
        .arg("f_")
        .arg("--tier")
        .arg("B")
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // Parse emitted JSON and assert the preset budget
    let v: Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(v["budget"], 3000, "tier B must set budget=3000");
    assert_eq!(v["tier"], "B", "tier label must be present in JSON");
    assert_eq!(
        v["effective_limit"], 192,
        "tier B must set effective_limit=192"
    );
    assert_eq!(
        v["effective_top_per_query"], 8,
        "tier B must set effective_top_per_query=8"
    );
}

/// Verify that --tier C sets the JSON 'budget' to 6000 by default
#[test]
fn test_tier_c_sets_budget_6000()
{
    // Create a representative test workspace
    let tmp = make_heavy_fixture();

    // Ensure symbols index is generated
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Run context with --tier C and JSON output
    let out = Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("context")
        .arg("f_")
        .arg("--tier")
        .arg("C")
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // Parse emitted JSON and assert the preset budget
    let v: Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(v["budget"], 6000, "tier C must set budget=6000");
    assert_eq!(v["tier"], "C", "tier label must be present in JSON");
    assert_eq!(
        v["effective_limit"], 256,
        "tier C must set effective_limit=256"
    );
    assert_eq!(
        v["effective_top_per_query"], 12,
        "tier C must set effective_top_per_query=12"
    );
}

/// Verify that explicit --budget overrides tier presets
#[test]
fn test_budget_overrides_tier()
{
    // Create a representative test workspace
    let tmp = make_heavy_fixture();

    // Ensure symbols index is generated
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Run with tier A but force a tiny explicit budget (180)
    let out = Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("context")
        .arg("f_")
        .arg("--tier")
        .arg("A")
        .arg("--budget")
        .arg("180") // Explicitly override preset
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // Ensure the explicit budget was honored over the preset
    let v: Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(v["budget"], 180, "explicit --budget must win");
    assert_eq!(v["tier"], "A", "tier label should still be present");
    // Tier limits should still apply when not explicitly overridden
    assert_eq!(v["effective_limit"], 96, "tier A limits should still apply");
    assert_eq!(
        v["effective_top_per_query"], 6,
        "tier A top-per-query should still apply"
    );
}

/// Verify that explicit --limit overrides tier limit presets
#[test]
fn test_limit_overrides_tier()
{
    // Create a representative test workspace
    let tmp = make_heavy_fixture();

    // Ensure symbols index is generated
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Run with tier A but explicitly override the limit
    let out = Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("context")
        .arg("f_")
        .arg("--tier")
        .arg("A")
        .arg("--limit")
        .arg("512") // Explicitly override tier limit
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // Ensure the explicit limit was honored over the preset
    let v: Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(v["budget"], 1200, "tier A budget should still apply");
    assert_eq!(v["tier"], "A", "tier label should still be present");
    assert_eq!(v["effective_limit"], 512, "explicit --limit must win");
    assert_eq!(
        v["effective_top_per_query"], 6,
        "tier A top-per-query should still apply"
    );
}

/// Verify that explicit --top-per-query overrides tier presets
#[test]
fn test_top_per_query_overrides_tier()
{
    // Create a representative test workspace
    let tmp = make_heavy_fixture();

    // Ensure symbols index is generated
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Run with tier A but explicitly override the top-per-query
    let out = Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("context")
        .arg("f_")
        .arg("--tier")
        .arg("A")
        .arg("--top-per-query")
        .arg("15") // Explicitly override tier top-per-query
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    // Ensure the explicit top-per-query was honored over the preset
    let v: Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(v["budget"], 1200, "tier A budget should still apply");
    assert_eq!(v["tier"], "A", "tier label should still be present");
    assert_eq!(v["effective_limit"], 96, "tier A limit should still apply");
    assert_eq!(
        v["effective_top_per_query"], 15,
        "explicit --top-per-query must win"
    );
}

/// Verify case insensitive tier parsing (a/B/c should work)
#[test]
fn test_tier_case_insensitive()
{
    // Create a representative test workspace
    let tmp = make_heavy_fixture();

    // Ensure symbols index is generated
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Test lowercase 'a'
    let out = Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("context")
        .arg("f_")
        .arg("--tier")
        .arg("a") // lowercase
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let v: Value = serde_json::from_slice(&out).expect("valid json");
    assert_eq!(v["tier"], "A", "lowercase 'a' should map to tier A");
    assert_eq!(v["budget"], 1200, "lowercase 'a' should set budget=1200");
}

/// Verify that tier presets work without --json (no crash in human output)
#[test]
fn test_tier_human_output()
{
    // Create a representative test workspace
    let tmp = make_heavy_fixture();

    // Ensure symbols index is generated
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Run with tier B but no --json flag (human output)
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("context")
        .arg("f_")
        .arg("--tier")
        .arg("B")
        // No --json flag here
        .assert()
        .success(); // Should not crash, should produce human-readable output
}
