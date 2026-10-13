//! The `tau` command line as the Python reference actually behaves.
//!
//! Every expectation below was **observed**, not inferred, by driving the real
//! typer app from the reference with
//! `typer.testing.CliRunner().invoke(tau_coding.cli.app, argv)`, stubbing
//! `run_openai_tui`, `run_openai_print_mode` and `_startup_update_notice` the
//! way `tau/tests/test_cli.py` itself stubs them. Observed with tau 0.1.3,
//! typer 0.27.0, click 8.4.2 and Python 3.12.10 on Windows.
//!
//! Two observations shape this port:
//!
//! - the reference spells version `-v` and `--version`. `-V` and `-h` are not
//!   flags at all: `allow_extra_args` with `ignore_unknown_options`
//!   (`cli.py:122`) turns them into the initial prompt;
//! - an unknown word or an unknown option is never an error. The port's earlier
//!   `tau: unsupported argument:` message with exit code 2 had no counterpart
//!   in the reference at all and is gone.
//!
//! The reference's exit codes: `0` for version, help and the normal path, `2`
//! for a usage error (Click's `UsageError`), `1` for a failed install, a failed
//! update or a failed print run.

use std::path::PathBuf;
use std::process::Command;

use tau_coding::VERSION;
use tau_coding::cli::{
    CliOptions, CliOutcome, PrintOutputMode, PseudoCommand, TrustOverride, USAGE, UsageError,
    parse_args,
};

/// The parsed options of an invocation that reached the interactive path.
fn interactive(argv: &[&str]) -> CliOptions {
    match parse_args(argv) {
        CliOutcome::Interactive(options) => *options,
        other => panic!("expected the interactive path for {argv:?}, got {other:?}"),
    }
}

/// The usage error of an invocation the reference rejects.
fn usage(argv: &[&str]) -> UsageError {
    match parse_args(argv) {
        CliOutcome::Usage(error) => error,
        other => panic!("expected a usage error for {argv:?}, got {other:?}"),
    }
}

#[test]
fn no_arguments_start_the_interactive_shell_without_a_prompt() {
    // Observed: exit 0, no output, and `run_openai_tui` called with
    // `initial_prompt=None`.
    let options = interactive(&[]);
    assert_eq!(options.initial_prompt(), None);
    assert_eq!(options.command(), None);
    assert!(!options.print_mode);
}

#[test]
fn the_v_flags_report_the_application_version() {
    // Observed: exit 0 and stdout `tau 0.1.3\n` for both spellings.
    for flag in ["-v", "--version"] {
        assert_eq!(
            parse_args([flag]),
            CliOutcome::Version,
            "the reference accepts {flag} for version"
        );
    }
    assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
}

#[test]
fn the_uppercase_v_is_not_a_version_flag() {
    // Observed: `tau -V` starts the interactive shell with the prompt `-V`.
    let options = interactive(&["-V"]);
    assert_eq!(options.initial_prompt().as_deref(), Some("-V"));
    assert!(!options.version);
}

#[test]
fn the_help_flag_requests_help() {
    // Observed: exit 0 and the generated help on stdout.
    assert_eq!(parse_args(["--help"]), CliOutcome::Help);
}

#[test]
fn the_h_short_flag_is_not_a_help_flag() {
    // Observed: `tau -h` starts the interactive shell with the prompt `-h`.
    let options = interactive(&["-h"]);
    assert_eq!(options.initial_prompt().as_deref(), Some("-h"));
}

#[test]
fn a_bare_unknown_word_becomes_the_initial_prompt() {
    // Observed: `tau hello` starts the interactive shell with `hello`.
    let options = interactive(&["hello"]);
    assert_eq!(options.initial_prompt().as_deref(), Some("hello"));
}

#[test]
fn an_unknown_flag_becomes_the_initial_prompt() {
    // Observed: `tau --definitely-not-a-flag` starts the interactive shell with
    // the flag text as the prompt. `ignore_unknown_options` makes this legal.
    let options = interactive(&["--definitely-not-a-flag"]);
    assert_eq!(
        options.initial_prompt().as_deref(),
        Some("--definitely-not-a-flag")
    );
}

#[test]
fn several_positionals_are_joined_into_one_prompt() {
    // Observed: `tau hello world` calls the TUI runner with `hello world`.
    let options = interactive(&["hello", "world"]);
    assert_eq!(options.initial_prompt().as_deref(), Some("hello world"));
    assert_eq!(options.command(), Some("hello"));
}

#[test]
fn a_known_flag_with_a_bad_value_is_a_usage_error_with_exit_code_two() {
    // Observed: exit 2 and, on stderr, the usage block followed by
    // "Invalid value for '--max-retries': 'abc' is not a valid int."
    let error = usage(&["--max-retries", "abc"]);
    assert_eq!(
        error.message,
        "Invalid value for '--max-retries': 'abc' is not a valid int."
    );
    assert_eq!(error.exit_code(), 2);
    let rendered = error.render();
    assert!(rendered.starts_with(USAGE), "rendered: {rendered}");
    assert!(rendered.contains("Try 'tau --help' for help."));
    assert!(
        rendered.contains("Invalid value for '--max-retries': 'abc' is not a valid int."),
        "rendered: {rendered}"
    );
}

#[test]
fn a_pseudo_command_name_is_dispatched_rather_than_treated_as_a_prompt() {
    // Observed: `tau install` with no source is exit 2 with
    // "Invalid value: Usage: tau install <source> [--force]"; with a source it
    // runs the installer (and exits 1 when the source cannot be fetched).
    assert_eq!(
        usage(&["install"]).message,
        "Invalid value: Usage: tau install <source> [--force]"
    );
    assert_eq!(
        parse_args(["install", "git:github.com/example/demo", "--force"]),
        CliOutcome::Command(PseudoCommand::Install {
            source: "git:github.com/example/demo".to_string(),
            force: true,
        })
    );
    // Observed: `tau sessions` prints "No sessions found." and exits 0.
    assert_eq!(
        parse_args(["sessions"]),
        CliOutcome::Command(PseudoCommand::Sessions)
    );
    // Observed: `tau providers` prints the provider list and exits 0.
    assert_eq!(
        parse_args(["providers"]),
        CliOutcome::Command(PseudoCommand::Providers)
    );
    // Observed: `tau update` runs the updater (exit 1 on this host, because tau
    // was installed from a local URL); `tau update --models` refreshes catalogs.
    assert_eq!(
        parse_args(["update"]),
        CliOutcome::Command(PseudoCommand::Update { models: false })
    );
    assert_eq!(
        parse_args(["update", "--models"]),
        CliOutcome::Command(PseudoCommand::Update { models: true })
    );
}

#[test]
fn a_pseudo_command_name_with_extra_positionals_falls_through_to_the_prompt() {
    // Observed: `tau sessions extra` and `tau setup extra` start the interactive
    // shell with `sessions extra` / `setup extra`, because the reference guards
    // each pseudo-command on `len(positional_args) == 1`.
    assert_eq!(
        interactive(&["sessions", "extra"])
            .initial_prompt()
            .as_deref(),
        Some("sessions extra")
    );
    assert_eq!(
        interactive(&["providers", "extra"])
            .initial_prompt()
            .as_deref(),
        Some("providers extra")
    );
    assert_eq!(
        interactive(&["setup", "extra"]).initial_prompt().as_deref(),
        Some("setup extra")
    );
}

#[test]
fn the_print_flag_selects_print_mode_with_the_text_output() {
    // Observed: `tau -p hello` calls the print runner with prompt `hello` and
    // `PrintOutputMode.text`.
    match parse_args(["-p", "hello"]) {
        CliOutcome::Print(request) => {
            assert_eq!(request.prompt, "hello");
            assert_eq!(request.mode, PrintOutputMode::Text);
        }
        other => panic!("expected print mode, got {other:?}"),
    }
    // Observed: `tau --mode json hello` calls the print runner with `json`.
    match parse_args(["--mode", "json", "hello"]) {
        CliOutcome::Print(request) => assert_eq!(request.mode, PrintOutputMode::Json),
        other => panic!("expected print mode, got {other:?}"),
    }
    // Observed: `tau --mode rpc` never reaches the print runner.
    match parse_args(["--mode", "rpc"]) {
        CliOutcome::Print(request) => assert_eq!(request.mode, PrintOutputMode::Rpc),
        other => panic!("expected RPC mode, got {other:?}"),
    }
}

#[test]
fn parsing_stops_at_the_first_positional_argument() {
    // Observed: `tau -p hi --session-id ok-id` calls the print runner with
    // prompt `hi --session-id ok-id` and no session id, because typer sets
    // `allow_interspersed_args = False` (`typer/core.py:999`). The option is
    // only recognised when it precedes the first positional.
    match parse_args(["-p", "hi", "--session-id", "ok-id"]) {
        CliOutcome::Print(request) => {
            assert_eq!(request.prompt, "hi --session-id ok-id");
            assert_eq!(request.options.session_id, None);
        }
        other => panic!("expected print mode, got {other:?}"),
    }
    match parse_args(["--session-id", "ok-id", "-p", "hi"]) {
        CliOutcome::Print(request) => {
            assert_eq!(request.prompt, "hi");
            assert_eq!(request.options.session_id.as_deref(), Some("ok-id"));
        }
        other => panic!("expected print mode, got {other:?}"),
    }
}

#[test]
fn a_double_dash_ends_option_parsing() {
    // Observed: `tau --print -- -p` sends the literal `-p` as the prompt, and
    // `tau -- --print hello` starts the interactive shell with
    // `--print hello`.
    match parse_args(["--print", "--", "-p"]) {
        CliOutcome::Print(request) => assert_eq!(request.prompt, "-p"),
        other => panic!("expected print mode, got {other:?}"),
    }
    assert_eq!(
        interactive(&["--", "--print", "hello"])
            .initial_prompt()
            .as_deref(),
        Some("--print hello")
    );
}

#[test]
fn help_wins_over_body_validation_and_version_does_not() {
    // Observed: `tau --max-retries abc --help` prints help and exits 0, because
    // Click's help option is eager and runs before value conversion, while
    // `tau -v --max-retries abc` exits 2 with the conversion error, because the
    // version check lives in the callback body.
    assert_eq!(
        parse_args(["--max-retries", "abc", "--help"]),
        CliOutcome::Help
    );
    assert_eq!(
        parse_args(["--help", "--max-retries", "abc"]),
        CliOutcome::Help
    );
    assert_eq!(parse_args(["-v", "--max-retries", "abc"]), {
        CliOutcome::Usage(usage(&["--max-retries", "abc"]))
    });
    // Observed: the version check comes before every body validation.
    assert_eq!(parse_args(["-v", "--models"]), CliOutcome::Version);
    assert_eq!(
        parse_args(["-v", "--approve", "--no-approve"]),
        CliOutcome::Version
    );
}

#[test]
fn a_structural_parse_error_wins_over_help() {
    // Observed: `tau --help --model` exits 2 with
    // "Option '--model' requires an argument." and prints no help, because the
    // parser raises before any option is processed. The message carries no
    // usage block: the reference renders one only when the error knows its
    // context, and these parser errors do not.
    let error = usage(&["--help", "--model"]);
    assert_eq!(error.message, "Option '--model' requires an argument.");
    assert!(!error.usage);
    // Observed: the reference renders this inside its `Error` panel alone, with
    // no usage block and no advice line. The port's plain-text reduction prints
    // the message alone.
    assert_eq!(error.render(), "Option '--model' requires an argument.\n");
    // Observed: `-e` reports the token that was typed.
    assert_eq!(usage(&["-e"]).message, "Option '-e' requires an argument.");
    // Observed: `--print=true` is a structural error too.
    assert_eq!(
        usage(&["--print=true"]).message,
        "Option '--print' does not take a value."
    );
}

#[test]
fn value_conversion_failures_follow_the_invocation_order() {
    // Observed: `tau --mode bogus --max-retries abc` reports `--mode`, and
    // `tau --max-retries abc --mode bogus` reports `--max-retries`, because
    // Click converts parameters in the order they appeared on the command line.
    assert_eq!(
        usage(&["--mode", "bogus", "--max-retries", "abc"]).message,
        "Invalid value for '--mode': 'bogus' is not one of 'text', 'json', 'transcript', 'rpc'."
    );
    assert_eq!(
        usage(&["--max-retries", "abc", "--mode", "bogus"]).message,
        "Invalid value for '--max-retries': 'abc' is not a valid int."
    );
    // Observed: a conversion failure names every declared token of the option,
    // joined by " / ", so the hidden `-o` alias appears in the hint.
    assert_eq!(
        usage(&["-o", "nope"]).message,
        "Invalid value for '--output' / '-o': 'nope' is not one of 'text', 'json', 'transcript', 'rpc'."
    );
    assert_eq!(
        usage(&["--timeout-seconds", "z"]).message,
        "Invalid value for '--timeout-seconds': 'z' is not a valid float."
    );
}

#[test]
fn the_click_token_rules_are_reproduced() {
    // Observed: a value-taking option consumes the next argument even when it
    // looks like an option, so `-m -x` sets the model to `-x`.
    assert_eq!(
        interactive(&["-m", "-x", "hello"]).model.as_deref(),
        Some("-x")
    );
    // Observed: short flags combine, and the last one in the run takes the next
    // argument: `-at high --new-session` is `--approve` with thinking `high`.
    let combined = interactive(&["-at", "high", "--new-session"]);
    assert!(combined.approve);
    assert_eq!(combined.thinking.as_deref(), Some("high"));
    // Observed: `-pt high x` is a combination in print mode, with the prompt `x`.
    match parse_args(["-pt", "high", "x"]) {
        CliOutcome::Print(request) => {
            assert!(request.options.print_mode);
            assert_eq!(
                request.options.thinking_level,
                Some(tau_coding::cli::ThinkingLevel::High)
            );
            assert_eq!(request.prompt, "x");
        }
        other => panic!("expected print mode, got {other:?}"),
    }
    // Observed: `--model=x` attaches the value, `-m=x` does not.
    assert_eq!(interactive(&["--model=x"]).model.as_deref(), Some("x"));
    assert_eq!(interactive(&["-m=x"]).model.as_deref(), Some("=x"));
    // Observed: `-na` is a single long-style token, so it is `--no-approve` and
    // does not decompose into `-n` plus `-a`.
    let declined = interactive(&["-na", "--new-session"]);
    assert!(declined.no_approve);
    assert!(!declined.approve);
    assert_eq!(declined.trust_override(), Some(TrustOverride::Decline));
    // Observed: repeatable options accumulate in order, and the `--` separator
    // still ends option parsing.
    let repeated = interactive(&[
        "--no-extensions",
        "--project-extensions",
        "-e",
        "a",
        "-e",
        "b",
        "--",
        "x",
    ]);
    assert!(repeated.no_extensions);
    assert!(repeated.project_extensions);
    assert_eq!(repeated.extension, [PathBuf::from("a"), PathBuf::from("b")]);
    assert_eq!(repeated.initial_prompt().as_deref(), Some("x"));
}

#[test]
fn the_binary_exits_with_the_observed_codes() {
    let binary = env!("CARGO_BIN_EXE_tau");

    let version = Command::new(binary)
        .arg("--version")
        .output()
        .expect("tau --version runs");
    assert_eq!(version.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim_end(),
        format!("tau {VERSION}")
    );

    let help = Command::new(binary)
        .arg("--help")
        .output()
        .expect("tau --help runs");
    assert_eq!(help.status.code(), Some(0));

    // Observed exit code 2 for every Click usage error.
    let bad_value = Command::new(binary)
        .args(["--max-retries", "abc"])
        .output()
        .expect("tau --max-retries abc runs");
    assert_eq!(bad_value.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&bad_value.stderr)
            .contains("Invalid value for '--max-retries': 'abc' is not a valid int."),
        "stderr: {}",
        String::from_utf8_lossy(&bad_value.stderr)
    );
}

#[test]
fn the_unported_paths_report_what_they_wait_for() {
    let binary = env!("CARGO_BIN_EXE_tau");

    // The reference starts the Textual TUI here (exit 0). `tau-tui` is not
    // ported, so the port names the module it waits for in one line instead of
    // pretending the shell started.
    let interactive_run = Command::new(binary)
        .env_remove("TAU_HOME")
        .output()
        .expect("tau runs with no arguments");
    assert_eq!(interactive_run.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&interactive_run.stderr);
    assert_eq!(stderr.lines().count(), 1, "stderr: {stderr}");
    assert!(stderr.contains("not implemented yet"), "stderr: {stderr}");
    assert!(stderr.contains("tau-tui"), "stderr: {stderr}");
}

/// Observed: the two Click error shapes reach stderr in full and stdout stays
/// empty. A parser-structural error prints the message alone; a type or body
/// error prints the usage block, then `Try 'tau --help' for help.`, then the
/// message. Both exit 2.
#[test]
fn the_two_usage_error_shapes_reach_stderr_in_full() {
    let binary = env!("CARGO_BIN_EXE_tau");

    let structural = Command::new(binary)
        .args(["--max-retries"])
        .env_remove("TAU_HOME")
        .output()
        .expect("tau --max-retries runs");
    assert_eq!(structural.status.code(), Some(2));
    assert!(structural.stdout.is_empty(), "stdout: {structural:?}");
    assert_eq!(
        String::from_utf8_lossy(&structural.stderr),
        "Option '--max-retries' requires an argument.\n"
    );

    let typed = Command::new(binary)
        .args(["--max-retries", "abc"])
        .env_remove("TAU_HOME")
        .output()
        .expect("tau --max-retries abc runs");
    assert_eq!(typed.status.code(), Some(2));
    assert!(typed.stdout.is_empty(), "stdout: {typed:?}");
    assert_eq!(
        String::from_utf8_lossy(&typed.stderr),
        format!(
            "{USAGE}\nTry 'tau --help' for help.\n\
             Invalid value for '--max-retries': 'abc' is not a valid int.\n"
        )
    );
}

/// Mirror of `tests/test_cli.py::test_relative_tau_home_reports_actionable_cli_error`,
/// driven through the built binary so the check is observed on the real entry
/// point, not only through the library call.
#[test]
fn a_bad_tau_home_exits_two_with_the_reference_message() {
    let binary = env!("CARGO_BIN_EXE_tau");
    let output = Command::new(binary)
        .args(["providers"])
        .env("TAU_HOME", ".tau-personal")
        .output()
        .expect("tau providers runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "stdout: {output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "{USAGE}\nTry 'tau --help' for help.\n\
             Invalid value for TAU_HOME: \
             TAU_HOME must be an absolute path after '~' expansion\n"
        )
    );
}
