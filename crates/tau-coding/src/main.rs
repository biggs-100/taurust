//! The `tau` command-line entry point.

use std::process::ExitCode;

use tau_coding::VERSION;
use tau_coding::cli::{CliOutcome, HELP, PseudoCommand, UsageError};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match tau_coding::parse_args(args) {
        CliOutcome::Version => {
            println!("tau {VERSION}");
            ExitCode::SUCCESS
        }
        CliOutcome::Help => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        CliOutcome::Usage(error) => report_usage(&error),
        CliOutcome::Interactive(_) => unimplemented_line("the interactive shell", "tau-tui"),
        CliOutcome::Print(request) => {
            if request.mode == tau_coding::PrintOutputMode::Rpc {
                unimplemented_line("RPC mode", "tau_coding::rpc")
            } else {
                unimplemented_line("print mode", "tau_coding::rendering")
            }
        }
        CliOutcome::Command(command) => match command {
            PseudoCommand::Install { .. } => {
                unimplemented_line("`tau install`", "tau_coding::extension_installer")
            }
            PseudoCommand::Update { .. } => {
                unimplemented_line("`tau update`", "tau_coding::updater")
            }
            PseudoCommand::Sessions => {
                unimplemented_line("`tau sessions`", "tau_coding::session_manager")
            }
            PseudoCommand::Export(_) => {
                unimplemented_line("`tau export`", "tau_coding::session_export")
            }
            PseudoCommand::Providers => {
                unimplemented_line("`tau providers`", "tau_coding::rendering")
            }
            PseudoCommand::Setup(_) => {
                unimplemented_line("`tau setup`", "tau_coding::provider_config")
            }
        },
    }
}

/// Print a Click usage error and exit with Click's exit code.
fn report_usage(error: &UsageError) -> ExitCode {
    eprint!("{}", error.render());
    ExitCode::from(error.exit_code())
}

/// Report a path the port cannot take yet, in one line, naming what it waits for.
///
/// The exit code is the port's own marker, not a reference behaviour: the
/// reference takes these paths with exit code 0 or 1 and the port must not
/// pretend it did. The T22 parity gate has to know that until the named module
/// lands, these invocations diverge on purpose.
fn unimplemented_line(feature: &str, module: &str) -> ExitCode {
    eprintln!("tau: {feature} is not implemented yet; waiting for `{module}`");
    ExitCode::from(2)
}
