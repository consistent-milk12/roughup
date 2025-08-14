// Imports used by all tests in this file
// We use assert_cmd for spawning the compiled binary and
// capturing stdout/stderr in a platform-agnostic way.
use assert_cmd::prelude::*;
// We use Command from std::process to launch the binary.
use std::process::Command;
// We create temporary on-disk fixtures with assert_fs so tests
// are hermetic and do not rely on the developer's filesystem.
use assert_fs::prelude::*;
// We need serde_json to parse the tool's JSON output safely and
// assert on structural invariants rather than raw strings.
use serde_json::Value;
// We use predicates to make concise assertions about stdout and
// stderr content when string matching is enough.
use predicates::prelude::*;

// Helper: build a minimal source tree that exercises symbol
// discovery and context assembly without depending on any
// external services or network access.
fn make_fixture() -> assert_fs::TempDir {
    // Create an ephemeral temp directory that is auto-cleaned.
    let tmp = assert_fs::TempDir::new().expect("tempdir");
    // Write a small Rust source file to test language handling.
    tmp.child("src/lib.rs")
        .write_str(
            "// demo lib\n\
             pub fn alpha() {}\n\
             mod inner { pub fn beta() {} }\n",
        )
        .expect("write lib.rs");
    // Write a second file with different path depth to test
    // proximity/scope ranking when working directory varies.
    tmp.child("utils/helper.rs")
        .write_str(
            "// helper utils\n\
             pub fn gamma() {}\n",
        )
        .expect("write helper.rs");
    // Provide a README to test generic text handling paths.
    tmp.child("README.md")
        .write_str("# Demo Project\n\nSome context here.\n")
        .expect("write README.md");
    // Return the prepared directory to the caller.
    tmp
}

// Test: basic smoke run with JSON output and a generous budget.
// Asserts the command exits successfully and returns well-formed
// JSON with expected top-level keys and non-empty items.
#[test]
fn test_context_smoke_json_output() {
    // Create the test fixture repository.
    let tmp = make_fixture();

    // First create symbols index
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();

    // Prepare the command to run the compiled binary.
    let mut cmd = Command::cargo_bin("rup").expect("bin");
    // Point the command at the fixture and request JSON output.
    let assert = cmd
        .current_dir(tmp.path())
        .arg("context")
        .arg("alpha") // Query for alpha function
        .arg("--json")
        .arg("--budget")
        .arg("2000")
        .assert()
        .success();
    // Capture stdout as a UTF-8 string for JSON parsing.
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    // Parse JSON into a serde_json::Value for structural checks.
    let v: Value = serde_json::from_str(&stdout).expect("json");
    // Assert the presence of expected keys in our JSON schema
    assert!(v.get("items").is_some(), "missing items array");
    assert!(v.get("model").is_some(), "missing model field");
    assert!(v.get("budget").is_some(), "missing budget field");
    assert!(
        v.get("total_tokens").is_some(),
        "missing total_tokens field"
    );
    // Verify items is a non-empty array to confirm discovery.
    let items = v.get("items").unwrap().as_array().unwrap();
    assert!(!items.is_empty(), "expected non-empty items");
}

// Test: quiet mode should suppress progress bars and chatter on
// stderr while still producing valid JSON on stdout.
#[test]
fn test_quiet_flag_suppresses_progress() {
    // Create fixture with a couple of files.
    let tmp = make_fixture();
    // Launch the binary with --quiet to minimize stderr noise.
    let mut cmd = Command::cargo_bin("rup").expect("bin");
    let _ = cmd
        .current_dir(tmp.path())
        .arg("--quiet")
        .arg("context")
        .arg("alpha") // Query for alpha function
        .arg("--json")
        .arg("--budget")
        .arg("2000")
        .assert()
        .success()
        .stderr(
            predicate::str::is_empty().or(predicate::str::contains("progress")
                .not()
                .and(predicate::str::contains("█").not())),
        );
}

// Test: running the exact same command twice on the same inputs
// should yield identical JSON when nondeterminism is disabled.
// This validates deterministic ordering and stable trimming.
#[test]
fn test_deterministic_output_across_runs() {
    // Prepare fixture repository with a few files.
    let tmp = make_fixture();
    // Helper to run once and capture stdout as string.
    let run_once = || {
        let mut cmd = Command::cargo_bin("rup").expect("bin");
        let out = cmd
            .current_dir(tmp.path())
            .arg("context")
            .arg("alpha") // Query for alpha function
            .arg("--json")
            .arg("--budget")
            .arg("400")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).expect("utf8")
    };
    // Execute two times to compare for perfect equality.
    let a = run_once();
    let b = run_once();
    // Assert byte-for-byte equality of the JSON payloads.
    assert_eq!(a, b, "context output should be deterministic");
}
