//! The `tau` command-line entry point.

use std::process::ExitCode;

use tau_coding::{CliOutcome, USAGE, parse_args};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match parse_args(args) {
        CliOutcome::Version(version) => {
            println!("tau {version}");
            ExitCode::SUCCESS
        }
        CliOutcome::Help => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        CliOutcome::Unsupported(argument) => {
            eprintln!("tau: unsupported argument: {argument}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        CliOutcome::StartInteractive => {
            eprintln!("tau: the interactive shell is not implemented yet");
            ExitCode::from(2)
        }
    }
}
