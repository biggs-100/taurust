//! The portable subset of the reference's own CLI contract file.
//!
//! `tau/tests/test_cli.py` is 2140 lines and 89 test functions driving the real
//! typer app. Only the subset that exercises parsing, help, version and usage
//! errors can be ported here: the rest of that file asserts behaviour whose
//! module (`session_manager`, `rendering`, `updater`, `tui`, `extensions`, …)
//! has not been ported yet.
//!
//! Each test names the reference test it mirrors. Where the port cannot yet
//! reproduce the mirrored assertion, the difference is stated in the test
//! rather than silently narrowed.
//!
//! Everything asserted here was re-observed against the running reference at
//! tau 0.1.3 / typer 0.27.0 / click 8.4.2 before it was written down.

use std::path::PathBuf;

use tau_coding::VERSION;
use tau_coding::cli::{
    CliOptions, CliOutcome, PrintOutputMode, PseudoCommand, ThinkingLevel, USAGE, UsageError,
    merge_stdin_prompt, parse_args,
};

fn interactive(argv: &[&str]) -> CliOptions {
    match parse_args(argv) {
        CliOutcome::Interactive(options) => *options,
        other => panic!("expected the interactive path for {argv:?}, got {other:?}"),
    }
}

fn print_request(argv: &[&str]) -> (CliOptions, PrintOutputMode, String) {
    match parse_args(argv) {
        CliOutcome::Print(request) => (request.options, request.mode, request.prompt),
        other => panic!("expected print mode for {argv:?}, got {other:?}"),
    }
}

fn usage(argv: &[&str]) -> UsageError {
    match parse_args(argv) {
        CliOutcome::Usage(error) => error,
        other => panic!("expected a usage error for {argv:?}, got {other:?}"),
    }
}

/// Strip all whitespace, the way the reference's help tests do.
fn without_whitespace(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// Collapse every whitespace run to one space, the way `_panel_text` does.
fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Mirror of `tests/test_cli.py::test_version_command`.
#[test]
fn version_command_reports_the_application_version() {
    // The reference monkeypatches `_current_version` and asserts
    // `result.stdout.strip() == "tau 1.2.3"`; the port reads its own crate
    // version, so the assertion is that the version outcome carries it.
    assert_eq!(parse_args(["--version"]), CliOutcome::Version);
    assert!(!VERSION.is_empty());
}

/// Mirror of `tests/test_cli.py::test_version_short_flag_prints_version`.
#[test]
fn version_short_flag_prints_version() {
    assert_eq!(parse_args(["-v"]), CliOutcome::Version);
}

/// Mirror of `tests/test_cli.py::test_help_lists_system_prompt_options`.
#[test]
fn help_lists_system_prompt_options() {
    // The reference asserts these three fragments against the ANSI-stripped,
    // whitespace-collapsed rendering of `--help` at `COLUMNS=160`. The port
    // asserts the same fragments against its plain-text help. Byte-for-byte
    // reproduction of Click's and Rich's panel layout is deliberately out of
    // scope; the tokens, metavars, help strings, defaults and epilog are the
    // reference's own.
    let output = without_whitespace(tau_coding::cli::HELP);
    assert_eq!(parse_args(["--help"]), CliOutcome::Help);
    assert!(
        output.contains("--system-promptTEXT_OR_PATH"),
        "help: {output}"
    );
    assert!(
        output.contains("--append-system-promptTEXT_OR_PATH"),
        "help: {output}"
    );
    assert!(
        output.contains("tauinstallSOURCE[--force]"),
        "help: {output}"
    );
}

/// Mirror of `tests/test_cli.py::test_help_lists_thinking_option`.
#[test]
fn help_lists_thinking_option() {
    let output = without_whitespace(tau_coding::cli::HELP);
    assert!(output.contains("--thinking"), "help: {output}");
}

/// Pin the help rendering to the option registry.
///
/// The reference generates `--help` from its declarations, so a flag the table
/// declares can never be missing from the help. The port keeps the rendering as
/// one literal, which is why this test walks
/// [`tau_coding::cli::declared_options`] and checks the two agree: every visible
/// option's help string, metavar and default must appear, and every hidden
/// option must not.
#[test]
fn help_matches_the_declared_option_registry() {
    let help = tau_coding::cli::HELP;
    let collapsed_help = without_whitespace(help);
    let tokens: Vec<&str> = help.split_whitespace().collect();
    let declared = tau_coding::cli::declared_options();
    // `USAGE` is extracted from the rendering, so the same line is written twice
    // in the source; this keeps the two copies from drifting.
    assert!(help.starts_with(USAGE), "help: {help}");
    assert_eq!(
        declared.len(),
        31,
        "30 declared options plus the generated --help"
    );

    for option in declared {
        let visible = option.help.is_some();
        for token in option.tokens {
            assert_eq!(
                tokens.contains(token),
                visible,
                "{token} rendered={} but hidden={}",
                tokens.contains(token),
                !visible
            );
        }
        if let Some(help_text) = option.help {
            assert!(
                collapsed_help.contains(&without_whitespace(help_text)),
                "help string missing from --help: {help_text}"
            );
        }
        if let Some(metavar) = option.metavar {
            assert!(
                tokens.contains(&metavar),
                "metavar {metavar} missing from --help"
            );
        }
        if let Some(default) = option.default {
            assert!(
                collapsed_help.contains(&format!("[default:{default}]")),
                "default {default} missing from --help"
            );
        }
    }
}

/// Mirror of the hidden options declared in `cli.py:212`, `cli.py:288`,
/// `cli.py:305` and `cli.py:339`, none of which the observed `--help` renders.
#[test]
fn help_omits_the_removed_and_hidden_options() {
    let tokens: Vec<&str> = tau_coding::cli::HELP.split_whitespace().collect();
    for hidden in ["--prompt", "--resume", "--output", "-o", "-x"] {
        assert!(
            !tokens.contains(&hidden),
            "{hidden} must stay hidden, help: {}",
            tau_coding::cli::HELP
        );
    }
}

/// Mirror of `tests/test_cli.py::test_install_command_requires_exactly_one_source`.
#[test]
fn install_command_requires_exactly_one_source() {
    let error = usage(&["install"]);
    assert_eq!(error.exit_code(), 2);
    assert!(
        collapsed(&error.render()).contains("Usage: tau install <source> [--force]"),
        "rendered: {}",
        error.render()
    );
    // Observed: a second positional is the same error, and an unknown option
    // reports itself (`cli.py:281`).
    assert_eq!(
        usage(&["install", "a", "b"]).message,
        "Invalid value: Usage: tau install <source> [--force]"
    );
    assert_eq!(
        usage(&["install", "--bogus", "a"]).message,
        "Invalid value: Unknown option for `tau install`: --bogus"
    );
}

/// Mirror of `tests/test_cli.py::test_print_mode_rejects_session_and_new_session`.
#[test]
fn print_mode_rejects_session_and_new_session() {
    let error = usage(&[
        "--print",
        "--session",
        "session-123",
        "--new-session",
        "follow up",
    ]);
    assert_eq!(error.exit_code(), 2);
    assert!(
        error
            .render()
            .contains("--session and --new-session cannot be used together"),
        "rendered: {}",
        error.render()
    );
}

/// Mirror of `tests/test_cli.py::test_print_mode_rejects_session_and_session_id`.
#[test]
fn print_mode_rejects_session_and_session_id() {
    let error = usage(&[
        "--print",
        "--session",
        "session-123",
        "--session-id",
        "new-id",
        "follow up",
    ]);
    assert_eq!(error.exit_code(), 2);
    assert!(
        error
            .render()
            .contains("--session and --session-id cannot be used together"),
        "rendered: {}",
        error.render()
    );
}

/// Mirror of `tests/test_cli.py::test_print_mode_rejects_invalid_session_id`.
///
/// The reference validates with `session_manager.validate_session_id`. The
/// port carries the same rule and the same messages in `cli`, with the note
/// that they move to `tau_coding::session_manager` when that module lands.
#[test]
fn print_mode_rejects_invalid_session_id() {
    let long_id = "a".repeat(129);
    let cases: [(&str, &str); 6] = [
        ("", "Session id must be non-empty"),
        ("-bad", "Session id must be non-empty"),
        ("bad id", "Session id must be non-empty"),
        ("bad/", "Session id must be non-empty"),
        ("index", "Session id is reserved: index"),
        (&long_id, "Session id must be at most 128 bytes"),
    ];
    for (session_id, expected) in cases {
        let error = usage(&["-p", "--session-id", session_id, "hello"]);
        assert_eq!(error.exit_code(), 2, "session id {session_id:?}");
        assert!(
            error.render().contains(expected),
            "session id {session_id:?} rendered: {}",
            error.render()
        );
    }
}

/// Mirror of `tests/test_cli.py::test_session_id_is_print_mode_only`.
#[test]
fn session_id_is_print_mode_only() {
    let error = usage(&["--session-id", "worker-499"]);
    assert_eq!(error.exit_code(), 2);
    assert!(
        error
            .render()
            .contains("--session-id is only supported in print mode"),
        "rendered: {}",
        error.render()
    );
}

/// Mirror of `tests/test_cli.py::test_default_tui_rejects_session_with_new_session`.
#[test]
fn default_tui_rejects_session_with_new_session() {
    let error = usage(&["--cwd", ".", "--session", "session-1", "--new-session"]);
    assert_ne!(error.exit_code(), 0);
    assert!(
        error
            .render()
            .contains("--session and --new-session cannot be used together"),
        "rendered: {}",
        error.render()
    );
}

/// Mirror of `tests/test_cli.py::test_legacy_resume_flag_errors_with_migration_hint`.
#[test]
fn legacy_resume_flag_errors_with_migration_hint() {
    let error = usage(&["--cwd", ".", "--resume", "session-1"]);
    assert_eq!(error.exit_code(), 2);
    let rendered = error.render();
    assert!(
        rendered.contains("--resume was renamed to --session"),
        "{rendered}"
    );
    assert!(rendered.contains("session-1"), "{rendered}");
}

/// Mirror of `tests/test_cli.py::test_legacy_prompt_flag_errors_with_migration_hint`.
#[test]
fn legacy_prompt_flag_errors_with_migration_hint() {
    let error = usage(&["--prompt", "hello"]);
    assert_eq!(error.exit_code(), 2);
    let rendered = error.render();
    assert!(rendered.contains("--prompt was removed"), "{rendered}");
    assert!(rendered.contains("--print"), "{rendered}");
}

/// Mirror of `tests/test_cli.py::test_legacy_output_flag_errors_with_migration_hint`.
#[test]
fn legacy_output_flag_errors_with_migration_hint() {
    let error = usage(&["-p", "--output", "json", "hello"]);
    assert_eq!(error.exit_code(), 2);
    assert!(
        error.render().contains("--output was renamed to --mode"),
        "rendered: {}",
        error.render()
    );
}

/// Mirror of `tests/test_cli.py::test_legacy_extension_short_flag_errors_with_migration_hint`.
#[test]
fn legacy_extension_short_flag_errors_with_migration_hint() {
    let error = usage(&["-x", "."]);
    assert_eq!(error.exit_code(), 2);
    assert!(
        error.render().contains("-x was renamed to -e/--extension"),
        "rendered: {}",
        error.render()
    );
}

/// Mirror of `tests/test_cli.py::test_export_flag_rejects_combination_with_print`.
#[test]
fn export_flag_rejects_combination_with_print() {
    let error = usage(&["--export", "-p", "session-1"]);
    assert_ne!(error.exit_code(), 0);
    assert!(
        error
            .render()
            .contains("--export cannot be combined with --print/--mode"),
        "rendered: {}",
        error.render()
    );
    // Observed: the RPC combination has its own message (`cli.py:455`).
    assert_eq!(
        usage(&["--mode", "rpc", "--export", "x"]).message,
        "Invalid value: --export cannot be combined with --mode rpc"
    );
}

/// Mirror of `tests/test_cli.py::test_print_mode_requires_a_prompt`.
#[test]
fn print_mode_does_not_start_without_a_prompt() {
    // The reference merges piped stdin before this check, so the port exposes
    // the merge plus the check as one call rather than performing both during
    // parsing: `parse_args` must not read stdin.
    let error = merge_stdin_prompt("", None).expect_err("no prompt at all");
    // Observed: the reference wraps this usage text in `typer.BadParameter`, so
    // the rendered message reads "Invalid value: Usage: tau --print …".
    assert_eq!(
        error.message,
        "Invalid value: Usage: tau --print \"<prompt>\" (or --mode text|json|transcript \
         \"<prompt>\"); a prompt can also be piped in via stdin"
    );
    assert_eq!(error.exit_code(), 2);
    assert!(
        collapsed(&error.render()).contains("Usage: tau --print"),
        "rendered: {}",
        error.render()
    );
    assert_eq!(
        merge_stdin_prompt("hello", None).expect("a prompt"),
        "hello"
    );
}

/// Mirror of `tests/test_cli.py::test_print_mode_merges_piped_stdin_into_prompt`.
#[test]
fn print_mode_merges_piped_stdin_into_prompt() {
    // Observed: `-p Summarize` with stdin `piped content\n` calls the print
    // runner with `piped content\n\n\nSummarize`.
    assert_eq!(
        merge_stdin_prompt("Summarize", Some("piped content\n")).expect("a prompt"),
        "piped content\n\n\nSummarize"
    );
}

/// Mirror of `tests/test_cli.py::test_print_mode_accepts_stdin_only_prompt`.
#[test]
fn print_mode_accepts_stdin_only_prompt() {
    assert_eq!(
        merge_stdin_prompt("", Some("piped content\n")).expect("a prompt"),
        "piped content\n"
    );
}

/// Mirror of `tests/test_cli.py::test_thinking_flag_rejects_invalid_level`.
#[test]
fn thinking_flag_rejects_invalid_level() {
    let error = usage(&["--thinking", "maximum", "--new-session"]);
    assert_eq!(error.exit_code(), 2);
    assert!(
        error.render().contains("Unknown thinking mode: maximum"),
        "rendered: {}",
        error.render()
    );
    assert!(
        error
            .render()
            .contains("Available modes: off, minimal, low, medium, high, xhigh, max"),
        "rendered: {}",
        error.render()
    );
}

/// Mirror of `tests/test_cli.py::test_thinking_flag_accepts_case_insensitive_values`.
#[test]
fn thinking_flag_accepts_case_insensitive_values() {
    // Observed: `-t HIGH` reaches the TUI runner with the override `high`.
    let options = interactive(&["-t", "HIGH", "--new-session"]);
    assert_eq!(options.thinking_level, Some(ThinkingLevel::High));
    assert_eq!(options.thinking.as_deref(), Some("HIGH"));
}

/// Mirror of `tests/test_cli.py::test_thinking_flag_absent_forwards_none`.
#[test]
fn thinking_flag_absent_forwards_none() {
    assert_eq!(interactive(&["--new-session"]).thinking_level, None);
}

/// Mirror of `tests/test_cli.py::test_mode_flag_alone_triggers_print_mode`.
#[test]
fn mode_flag_alone_triggers_print_mode() {
    let (_, mode, prompt) = print_request(&["--mode", "json", "hello"]);
    assert_eq!(mode, PrintOutputMode::Json);
    assert_eq!(prompt, "hello");
}

/// Mirror of `tests/test_cli.py::test_system_prompt_flags_are_parsed_before_positional_prompt`.
///
/// The reference resolves each value as an existing UTF-8 file and joins the
/// appended ones with a blank line; that resolution belongs to the system-prompt
/// assembly module and is not ported, so this test asserts the parse order and
/// the untouched literals.
#[test]
fn system_prompt_flags_are_parsed_before_positional_prompt() {
    let base = "/tmp/base.md";
    let append = "/tmp/append.md";
    let (options, _, prompt) = print_request(&[
        "--print",
        "--system-prompt",
        base,
        "--append-system-prompt",
        "first",
        "--append-system-prompt",
        append,
        "explain",
        "this",
    ]);
    assert_eq!(prompt, "explain this");
    assert_eq!(options.system_prompt.as_deref(), Some(base));
    assert_eq!(options.append_system_prompt, ["first", append]);
}

/// Mirror of `tests/test_cli.py::test_default_tui_invokes_tui_runner_with_flags`.
#[test]
fn default_tui_carries_the_parsed_flags() {
    let options = interactive(&[
        "--cwd",
        "work",
        "--model",
        "fake",
        "--provider",
        "local",
        "--session",
        "session-1",
        "--auto-compact-threshold",
        "1000",
    ]);
    assert_eq!(options.cwd, Some(PathBuf::from("work")));
    assert_eq!(options.model.as_deref(), Some("fake"));
    assert_eq!(options.provider.as_deref(), Some("local"));
    assert_eq!(options.session.as_deref(), Some("session-1"));
    assert_eq!(options.auto_compact_threshold, Some(1000));
}

/// Mirror of `tests/test_cli.py::test_install_command_installs_extension`, reduced
/// to the parse surface: the source, the `--force` flag and the dispatch.
#[test]
fn install_command_carries_the_source_and_force() {
    for argv in [
        vec!["install", "git:github.com/example/demo", "--force"],
        vec!["install", "--force", "git:github.com/example/demo"],
    ] {
        assert_eq!(
            parse_args(&argv),
            CliOutcome::Command(PseudoCommand::Install {
                source: "git:github.com/example/demo".to_string(),
                force: true,
            }),
            "argv: {argv:?}"
        );
    }
}

/// Mirror of `tests/test_cli.py::test_export_command_accepts_format_option`.
#[test]
fn export_command_carries_the_format_and_destination() {
    assert_eq!(
        parse_args(["export", "session-1", "--format", "jsonl"]),
        CliOutcome::Command(PseudoCommand::Export(tau_coding::cli::ExportRequest {
            session_ref: "session-1".to_string(),
            output_path: None,
            export_format: Some("jsonl".to_string()),
        }))
    );
    assert_eq!(
        parse_args(["export", "session-1", "out.jsonl"]),
        CliOutcome::Command(PseudoCommand::Export(tau_coding::cli::ExportRequest {
            session_ref: "session-1".to_string(),
            output_path: Some(PathBuf::from("out.jsonl")),
            export_format: None,
        }))
    );
}

/// Mirror of `tests/test_cli.py::test_setup_command_writes_provider_settings`,
/// reduced to the defaults `tau setup` is invoked with.
#[test]
fn setup_command_carries_the_provider_defaults() {
    let CliOutcome::Command(PseudoCommand::Setup(setup)) = parse_args(["setup"]) else {
        panic!("expected the setup command");
    };
    assert_eq!(setup.provider_name, "openai");
    assert_eq!(setup.base_url, "https://api.openai.com/v1");
    assert_eq!(setup.api_key_env, "OPENAI_API_KEY");
    assert_eq!(setup.model, "gpt-5.4");
    assert_eq!(setup.timeout_seconds, 60.0);
    assert_eq!(setup.max_retries, 2);
    assert_eq!(setup.max_retry_delay_seconds, 1.0);
    assert!(setup.set_default);
}

/// Mirror of the reference's `--models` guard at `cli.py:462` and its update
/// usage check at `cli.py:460`, both observed live.
#[test]
fn update_usage_errors_match_the_reference() {
    assert_eq!(
        usage(&["--models"]).message,
        "Invalid value: --models is only supported with `tau update`"
    );
    assert_eq!(
        usage(&["update", "--bogus"]).message,
        "Invalid value: Usage: tau update [--models]"
    );
    assert_eq!(
        usage(&["export"]).message,
        "Invalid value: Usage: tau export <session-id-or-jsonl> [--format html|jsonl] [output]"
    );
    assert_eq!(
        USAGE,
        "Usage: tau [OPTIONS] [prompt_args]... COMMAND [ARGS]..."
    );
}
