// This file verifies that an external template can be supplied
// and that template placeholders are honored in the output.
// It assumes your CLI supports --template <file> and includes
// template metadata back into the JSON summary for parity.
use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use predicates::prelude::*;
use std::process::Command;

// Test: the provided template should influence output in a
// detectable way (e.g., a marker string included in stdout).
#[test]
fn test_template_override_is_applied() {
    // Create a small fixture project to feed the command.
    let tmp = assert_fs::TempDir::new().expect("tempdir");
    tmp.child("src/main.rs")
        .write_str("fn main() { println!(\"hi\"); }")
        .expect("write");
    // Create a simple template file with a unique marker token.
    let tpl = tmp.child("ctx.tpl");
    tpl.write_str("HEADER: __TEMPLATE_MARKER__\n{{items}}")
        .expect("write tpl");
    // Run the CLI using the template to render content.
    let mut cmd = Command::cargo_bin("rup").expect("bin");
    let assert = cmd
        .current_dir(tmp.path())
        .arg("context")
        .arg("test_query") // Add required query argument
        .arg("--template")
        .arg(tpl.path())
        .arg("--budget")
        .arg("400")
        .assert();
    // The process should complete successfully and stdout should contain the unique template marker string.
    assert
        .success()
        .stdout(predicate::str::contains("__TEMPLATE_MARKER__"));
}
