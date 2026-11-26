//! `toppingctl`: with no arguments, the TUI for the TOPPING E2x2; with a
//! command, the CLI (`toppingctl --help`). `--experimental` (anywhere, or
//! `TOPPINGCTL_EXPERIMENTAL=1`) lets it recognise the E4x4 Pre / E8x8 Pre.

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use toppingctl::cli::{self, Env, OpenError, EXPERIMENTAL_ENV};
use toppingctl::conn::{Connection, HidTransport};
use toppingctl::error::Platform;
use toppingctl::official_app::{self, System};
use toppingctl::open;
use toppingctl::sync::Store;
use toppingctl::tui::run::Refused;

fn main() -> ExitCode {
    let (args, experimental) = cli::split_experimental(
        std::env::args_os().skip(1).collect(),
        std::env::var_os(EXPERIMENTAL_ENV),
    );
    if !args.is_empty() {
        let stdin = io::stdin();
        let mut env = Env {
            store: Store::from_env(),
            stdin_is_tty: stdin.is_terminal(),
            stdin: &mut stdin.lock(),
            stdout: &mut io::stdout(),
            stderr: &mut io::stderr(),
            open: Box::new(|| {
                if let Some(w) = official_app::warning(&System) {
                    eprintln!("toppingctl: warning: {w}");
                }
                let (_model, device) = open::open(experimental)
                    .map_err(|e| OpenError::from_failure(&e, Platform::current()))?;
                Ok(Connection::spawn(HidTransport::new(device)))
            }),
        };
        return ExitCode::from(cli::run(&args, &mut env));
    }
    match toppingctl::tui::run::run(experimental) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("toppingctl: {e:#}");
            if e.downcast_ref::<Refused>().is_some_and(Refused::is_usage) {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
