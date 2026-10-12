//! Behaviour of the `tau` command-line surface.

use tau_coding::{CliOutcome, VERSION, parse_args};

#[test]
fn version_flags_report_the_application_version() {
    for flag in ["--version", "-V"] {
        assert_eq!(
            parse_args([flag]),
            CliOutcome::Version(VERSION.to_string()),
            "the {flag} flag must report the version"
        );
    }
}

#[test]
fn help_flag_requests_usage() {
    for flag in ["--help", "-h"] {
        assert_eq!(
            parse_args([flag]),
            CliOutcome::Help,
            "the {flag} flag must request usage"
        );
    }
}

#[test]
fn no_arguments_start_the_interactive_shell() {
    let args: [&str; 0] = [];
    assert_eq!(parse_args(args), CliOutcome::StartInteractive);
}

#[test]
fn unknown_arguments_are_reported_rather_than_ignored() {
    assert_eq!(
        parse_args(["--definitely-not-a-flag"]),
        CliOutcome::Unsupported("--definitely-not-a-flag".to_string())
    );
}
