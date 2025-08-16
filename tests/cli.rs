use clap::Parser;
use roughup::cli::{Cli, Commands, ContextArgs}; // adjust crate path if needed

#[test]
fn fail_signal_flag_parsing() {
    // Given
    let argv = vec![
        "rup",
        "context",
        "--budget",
        "2000",
        "--fail-signal",
        "tests/fixtures/rustc_error.log",
        "--template",
        "feature",
        "test_query",
    ];

    // When
    let cmd = Cli::parse_from(argv);

    // Then
    match cmd.command {
        Commands::Context(ContextArgs { budget, template, fail_signal, .. }) => {
            assert_eq!(budget, Some(2000));
            assert!(template.is_some());
            let p = fail_signal.expect("flag should be captured");
            assert!(p.to_string_lossy().ends_with("rustc_error.log"));
        }
        _ => panic!("expected Context command"),
    }
}