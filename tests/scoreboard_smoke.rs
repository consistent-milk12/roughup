//! Smoke test for the scoreboard harness.
//!
//! It builds a temporary heavy fixture, writes a minimal
//! plan pointing at that fixture, invokes the scoreboard,
//! and validates the presence and sanity of metrics.

use std::fs; // read/write files
use std::process::Command; // spawn

use assert_cmd::prelude::*; // assert helpers
use serde_json::Value; // parse JSONL lines

mod util; // reuse heavy fixture helpers
use util::make_heavy_fixture; // same as in context_budget.rs

#[test]
fn scoreboard_runs_and_emits_metrics()
{
    // Create heavy test fixture
    let tmp = make_heavy_fixture();

    // Ensure symbols exist to avoid warm-start variance
    // Use unique symbols file name to prevent race conditions between parallel tests
    let symbols_path = tmp.path().join("symbols.jsonl");
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .args(["symbols", "--output", symbols_path.to_str().unwrap()])
        .assert()
        .success();

    // Write a tiny plan into the temp fixture
    let plan = r#"
    {
      "encoding": "o200k_base",
      "scenarios": [
        {
          "name": "tierB_double",
          "fixture_path": ".",
          "queries": ["f_"],
          "tier": "B",
          "budget": null,
          "probe_first": false,
          "runs": [
            {"args": ["--tier","B","--json"], "label": "first"},
            {"args": ["--tier","B","--json"], "label": "second"}
          ]
        }
      ]
    }"#;

    // Persist the plan next to the fixture
    let plan_path = tmp
        .path()
        .join("plan.json");
    fs::write(&plan_path, plan).expect("write plan");

    // Choose output path for scoreboard.jsonl
    let out_path = tmp
        .path()
        .join("scoreboard.jsonl");

    // Run the scoreboard binary in the test fixture directory
    Command::cargo_bin("scoreboard")
        .expect("bin")
        .current_dir(tmp.path())
        .args([
            "--plan",
            plan_path
                .to_str()
                .unwrap(),
            "--out",
            out_path
                .to_str()
                .unwrap(),
        ])
        .assert()
        .success();

    // Read one JSONL row and validate key metrics
    let text = fs::read_to_string(&out_path).expect("read jsonl");
    let line = text
        .lines()
        .next()
        .expect("one line");
    let v: Value = serde_json::from_str(line).expect("json");

    // Must contain basic fields with sane ranges
    assert!(
        v["cef"]
            .as_f64()
            .unwrap()
            >= 1.0,
        "CEF should be >= 1.0"
    );
    assert!(
        v["dcr"]
            .as_f64()
            .unwrap()
            >= 0.0,
        "DCR should be >= 0.0"
    );
    assert!(
        v["dcr"]
            .as_f64()
            .unwrap()
            <= 1.0,
        "DCR should be <= 1.0"
    );
    assert!(
        v["actual_tokens"]
            .as_u64()
            .unwrap()
            > 0,
        "actual_tokens should be > 0"
    );
    assert!(
        v["baseline_tokens"]
            .as_u64()
            .unwrap()
            > 0,
        "baseline_tokens should be > 0"
    );
    assert!(
        v["deterministic_json_equal"]
            .as_bool()
            .unwrap(),
        "deterministic_json_equal should be true"
    );
    assert_eq!(v["name"], "tierB_double", "scenario name should match");
    assert_eq!(v["tier"], "B", "tier should be recorded");
    assert_eq!(v["encoding"], "o200k_base", "encoding should match");
    assert_eq!(v["pfr"], 0.0, "probe_first should be false (0.0)");
}

#[test]
fn scoreboard_handles_probe_first_scenario()
{
    // Create heavy test fixture
    let tmp = make_heavy_fixture();

    // Ensure symbols exist
    // Use unique symbols file name to prevent race conditions between parallel tests
    let symbols_path = tmp.path().join("symbols.jsonl");
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .args(["symbols", "--output", symbols_path.to_str().unwrap()])
        .assert()
        .success();

    // Write plan with probe_first: true
    let plan = r#"
    {
      "encoding": "o200k_base",
      "scenarios": [
        {
          "name": "tierA_probe",
          "fixture_path": ".",
          "queries": ["f_"],
          "tier": "A",
          "budget": null,
          "probe_first": true,
          "runs": [
            {"args": ["--tier","A","--json"], "label": "probe"}
          ]
        }
      ]
    }"#;

    // Persist the plan next to the fixture
    let plan_path = tmp
        .path()
        .join("plan.json");
    fs::write(&plan_path, plan).expect("write plan");

    // Choose output path for scoreboard.jsonl
    let out_path = tmp
        .path()
        .join("scoreboard.jsonl");

    // Run the scoreboard binary in the test fixture directory
    Command::cargo_bin("scoreboard")
        .expect("bin")
        .current_dir(tmp.path())
        .args([
            "--plan",
            plan_path
                .to_str()
                .unwrap(),
            "--out",
            out_path
                .to_str()
                .unwrap(),
        ])
        .assert()
        .success();

    // Validate PFR flag is set
    let text = fs::read_to_string(&out_path).expect("read jsonl");
    let line = text
        .lines()
        .next()
        .expect("one line");
    let v: Value = serde_json::from_str(line).expect("json");

    assert_eq!(v["pfr"], 1.0, "probe_first should be true (1.0)");
    assert_eq!(v["name"], "tierA_probe", "scenario name should match");
    assert_eq!(v["tier"], "A", "tier should be A");
}

#[test]
fn scoreboard_explicit_budget_scenario()
{
    // Create heavy test fixture
    let tmp = make_heavy_fixture();

    // Ensure symbols exist
    // Use unique symbols file name to prevent race conditions between parallel tests
    let symbols_path = tmp.path().join("symbols.jsonl");
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .args(["symbols", "--output", symbols_path.to_str().unwrap()])
        .assert()
        .success();

    // Write plan with explicit budget (no tier)
    let plan = r#"
    {
      "encoding": "o200k_base",
      "scenarios": [
        {
          "name": "explicit_180",
          "fixture_path": ".",
          "queries": ["f_"],
          "tier": null,
          "budget": 180,
          "probe_first": false,
          "runs": [
            {"args": ["--budget","180","--json"], "label": "tight"}
          ]
        }
      ]
    }"#;

    // Persist the plan next to the fixture
    let plan_path = tmp
        .path()
        .join("plan.json");
    fs::write(&plan_path, plan).expect("write plan");

    // Choose output path for scoreboard.jsonl
    let out_path = tmp
        .path()
        .join("scoreboard.jsonl");

    // Run the scoreboard binary in the test fixture directory
    Command::cargo_bin("scoreboard")
        .expect("bin")
        .current_dir(tmp.path())
        .args([
            "--plan",
            plan_path
                .to_str()
                .unwrap(),
            "--out",
            out_path
                .to_str()
                .unwrap(),
        ])
        .assert()
        .success();

    // Validate explicit budget was applied
    let text = fs::read_to_string(&out_path).expect("read jsonl");
    let line = text
        .lines()
        .next()
        .expect("one line");
    let v: Value = serde_json::from_str(line).expect("json");

    assert_eq!(v["budget"], 180, "budget should be 180");
    assert!(v["tier"].is_null(), "tier should be null");
    assert_eq!(v["name"], "explicit_180", "scenario name should match");

    // With tight budget, should get high CEF (lots of baseline vs small actual)
    let cef = v["cef"]
        .as_f64()
        .unwrap();
    assert!(
        cef >= 2.0,
        "CEF should be high with tight budget, got {}",
        cef
    );
}
