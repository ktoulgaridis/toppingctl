//! `toppingctl`: with no arguments, the TUI for the TOPPING E2x2; with a
//! command, the CLI (`toppingctl --help`).

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use toppingctl::cli::{self, Env};
use toppingctl::conn::{Connection, HidTransport};
use toppingctl::error::{message, Platform};
use toppingctl::open::open_e2x2;
use toppingctl::sync::Store;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !args.is_empty() {
        let stdin = io::stdin();
        let mut env = Env {
            store: Store::from_env(),
            stdin_is_tty: stdin.is_terminal(),
            stdin: &mut stdin.lock(),
            stdout: &mut io::stdout(),
            stderr: &mut io::stderr(),
            open: Box::new(|| {
                let device = open_e2x2().map_err(|e| message(&e, Platform::current()))?;
                Ok(Connection::spawn(HidTransport::new(device)))
            }),
        };
        return ExitCode::from(cli::run(&args, &mut env));
    }
    match toppingctl::tui::run::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("toppingctl: {e:#}");
            ExitCode::FAILURE
        }
    }
}
