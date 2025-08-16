use std::process::Command;

use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use predicates::prelude::*;
use serde_json::Value;

/// Build a tiny, hermetic fixture repo.
fn make_fixture() -> assert_fs::TempDir
{
    let tmp = assert_fs::TempDir::new().expect("tempdir");
    tmp.child("src/lib.rs")
        .write_str("// demo lib\npub fn alpha() {}\nmod inner { pub fn beta() {} }\n")
        .expect("write lib.rs");
    tmp.child("utils/helper.rs")
        .write_str("// helper utils\npub fn gamma() {}\n")
        .expect("write helper.rs");
    tmp.child("README.md")
        .write_str("# Demo Project\n\nSome context here.\n")
        .expect("write README.md");
    tmp
}

/// Run `rup symbols` once to prebuild the index.
fn prebuild_symbols(tmp: &assert_fs::TempDir)
{
    Command::cargo_bin("rup")
        .expect("bin")
        .current_dir(tmp.path())
        .arg("symbols")
        .assert()
        .success();
}

/// Extract the JSON payload from stdout even if there's a non-JSON prelude.
/// (First '{' to end). Panics if no JSON object is found.
fn extract_json_from_stdout(s: &str) -> &str
{
    let start = s
        .find('{')
        .expect("no JSON object in stdout");
    &s[start..]
}

/// Convenience: run `rup context` and return (stdout, stderr) as UTF-8 strings.
fn run_context_json(
    tmp: &assert_fs::TempDir,
    extra: &[&str],
) -> (String, String)
{
    let mut cmd = Command::cargo_bin("rup").expect("bin");
    let assert = cmd
        .current_dir(tmp.path())
        .arg("context")
        .args(extra)
        .assert()
        .success();

    let out = String::from_utf8(
        assert
            .get_output()
            .stdout
            .clone(),
    )
    .expect("utf8 stdout");
    let err = String::from_utf8(
        assert
            .get_output()
            .stderr
            .clone(),
    )
    .expect("utf8 stderr");
    (out, err)
}

/// Smoke: JSON structure exists and items array is non-empty.
#[test]
fn test_context_smoke_json_output()
{
    let tmp = make_fixture();

    // Intentionally do NOT prebuild to exercise auto-index path.
    let (stdout, _stderr) = run_context_json(&tmp, &["alpha", "--json", "--budget", "2000"]);

    let json = extract_json_from_stdout(&stdout);
    let v: Value = serde_json::from_str(json).expect("valid json");

    assert!(
        v.get("items")
            .is_some(),
        "missing items array"
    );
    assert!(
        v.get("model")
            .is_some(),
        "missing model field"
    );
    assert!(
        v.get("budget")
            .is_some(),
        "missing budget field"
    );
    assert!(
        v.get("total_tokens")
            .is_some(),
        "missing total_tokens field"
    );

    let items = v
        .get("items")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(!items.is_empty(), "expected non-empty items");
}

/// Quiet mode: progress and chatter should not appear on stderr.
#[test]
fn test_quiet_flag_suppresses_progress()
{
    let tmp = make_fixture();
    // No need to prebuild; we just assert minimal stderr.
    let mut cmd = Command::cargo_bin("rup").expect("bin");
    cmd.current_dir(tmp.path())
        .arg("--quiet")
        .arg("context")
        .arg("alpha")
        .arg("--json")
        .arg("--budget")
        .arg("2000")
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
}

/// Determinism: same inputs → identical JSON on stdout.
/// We prebuild the index and disable auto-index so both runs are identical.
#[test]
fn test_deterministic_output_across_runs()
{
    let tmp = make_fixture();
    prebuild_symbols(&tmp); // ensure index exists

    let run_once = || {
        let mut cmd = Command::cargo_bin("rup").expect("bin");
        let assert = cmd
            .current_dir(tmp.path())
            .env("ROUGHUP_NO_AUTO_INDEX", "1") // prevent background regen / chatter
            .arg("context")
            .arg("alpha")
            .arg("--json")
            .arg("--budget")
            .arg("400")
            // lock down knobs that might change with future tier logic
            .arg("--limit")
            .arg("256")
            .arg("--top-per-query")
            .arg("8")
            .assert()
            .success();

        let stdout = String::from_utf8(
            assert
                .get_output()
                .stdout
                .clone(),
        )
        .expect("utf8");
        // When index is present, stdout should be pure JSON (no prelude),
        // but we still normalize via extract_json_from_stdout for robustness.
        extract_json_from_stdout(&stdout).to_owned()
    };

    let a = run_once();
    let b = run_once();
    assert_eq!(a, b, "context output should be deterministic");
}

/// Preferred behavior (to enable after product change):
/// Auto-index progress should go to stderr only; stdout stays pure JSON.
/// Marked ignored until symbols::run moves chatter off stdout.
#[test]
#[ignore = "enable after routing auto-index progress to stderr or quiet mode"]
fn test_auto_index_logs_to_stderr_only()
{
    let tmp = make_fixture();
    // Deliberately do not prebuild.
    let (stdout, stderr) = run_context_json(&tmp, &["alpha", "--json", "--budget", "400"]);

    // stdout must be a clean JSON object.
    assert!(
        stdout
            .trim_start()
            .starts_with('{'),
        "stdout must start with JSON"
    );
    let _v: Value = serde_json::from_str(stdout.trim()).expect("valid json");

    // stderr should contain the indexing chatter.
    assert!(
        stderr.contains("symbols index") || stderr.contains("Extract"),
        "expected auto-index chatter in stderr"
    );
}

/// With an existing index, stdout should be pure JSON (no leading chatter).
#[test]
fn test_context_emits_pure_json_when_index_present()
{
    let tmp = make_fixture();
    prebuild_symbols(&tmp);

    let (stdout, _stderr) = run_context_json(&tmp, &["alpha", "--json", "--budget", "400"]);

    assert!(
        stdout
            .trim_start()
            .starts_with('{'),
        "stdout should be pure JSON when index exists"
    );
    let _v: Value = serde_json::from_str(stdout.trim()).expect("valid json");
}
