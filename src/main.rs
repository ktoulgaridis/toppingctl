//! `toppingctl`: with no arguments, the TUI for the TOPPING E2x2.

use std::process::ExitCode;

const USAGE: &str = "\
Usage: toppingctl

Controls the TOPPING E2x2 USB audio interface. With no arguments it starts
the terminal UI; press ? inside it for the keys.

Options:
  -h, --help     show this help
  -V, --version  show the version

Environment:
  NO_COLOR       set to turn colours off";

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.first().map(|a| a.to_str()) {
        None => {}
        Some(Some("-h" | "--help")) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some(Some("-V" | "--version")) => {
            println!("toppingctl {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some(_) => {
            eprintln!("toppingctl: unexpected argument {:?}\n\n{USAGE}", args[0]);
            return ExitCode::from(2);
        }
    }
    match toppingctl::tui::run::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("toppingctl: {e:#}");
            ExitCode::FAILURE
        }
    }
}
