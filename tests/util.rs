//! Shared test utilities for integration tests
//!
//! Provides common fixture creation and helper functions
//! used across multiple test files.

use assert_fs::prelude::*;

/// Create a larger fixture to force trimming at small budgets.
/// Synthesizes multiple moderately sized files to exceed tight
/// budgets in a predictable way.
pub fn make_heavy_fixture() -> assert_fs::TempDir
{
    // Initialize the temporary project root
    let tmp = assert_fs::TempDir::new().expect("tempdir");

    // Generate several Rust files with repeated content lines to
    // inflate size while keeping parsing simple and stable
    for i in 0..8
    {
        // Compose a path like src/unit_i.rs for each file
        let p = format!("src/unit_{i}.rs");

        // Generate repeated functions to approximate token load
        let mut body = String::new();

        for j in 0..50
        {
            // Add content that is code-like to simulate sources
            body.push_str(&format!(
                "/// unit {i} fn {j}\npub fn f_{i}_{j}() {{ /* body */ }}\n"
            ));
        }

        // Write the file into the fixture
        tmp.child(&p)
            .write_str(&body)
            .expect("write");
    }

    // Include a root README to vary file types
    tmp.child("README.md")
        .write_str("# Heavy Fixture\n\nDetails.\n")
        .expect("write readme");

    // Return the prepared directory to the caller
    tmp
}
