//! Read-only HID input-report sniffer for the TOPPING E2x2.
//!
//! Prints one capture line per input report (see `toppingctl::capture`) to
//! stdout and, with `--out <file>`, to a file as well. It never writes to the
//! device. Ctrl-C stops the capture and flushes the output.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, LineWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use hidapi::HidDevice;
use toppingctl::capture::format_line;
use toppingctl::error::{is_interrupted, message, Platform};
use toppingctl::open::open_e2x2;

const USAGE: &str = "\
Usage: hidsniff [--out <file>]

Prints every HID input report from the TOPPING E2x2 as
'<elapsed seconds> rx <hex bytes>', one per line. Read-only.

Options:
  --out <file>  also write the lines to <file> (overwritten)
  -h, --help    show this help";

/// How long one blocking read waits, so Ctrl-C is noticed promptly.
const READ_TIMEOUT_MS: i32 = 100;
/// Larger than any HID report the E2x2 can send over USB.
const REPORT_BUFFER_LEN: usize = 1024;

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Sniff { out: Option<PathBuf> },
    Help,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let mut out: Option<PathBuf> = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let value = match arg.to_str() {
            Some("-h" | "--help") => return Ok(Command::Help),
            Some("--out") => args.next().ok_or("--out needs a file")?,
            Some(a) if a.starts_with("--out=") => OsString::from(&a["--out=".len()..]),
            _ => return Err(format!("unexpected argument {arg:?}")),
        };
        if value.is_empty() {
            return Err("--out needs a file".into());
        }
        if out.replace(PathBuf::from(value)).is_some() {
            return Err("--out given more than once".into());
        }
    }
    Ok(Command::Sniff { out })
}

fn main() -> ExitCode {
    let command = match parse_args(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("hidsniff: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Command::Sniff { out } => run(out),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("hidsniff: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(out: Option<PathBuf>) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let handler_stop = Arc::clone(&stop);
    ctrlc::set_handler(move || handler_stop.store(true, Ordering::SeqCst))
        .context("installing the Ctrl-C handler")?;

    let device = open()?;
    // Create the file only once the device is open, so failures leave no empty capture.
    let out = match out {
        Some(path) => {
            let file =
                File::create(&path).with_context(|| format!("creating {}", path.display()))?;
            Some(LineWriter::new(file))
        }
        None => None,
    };
    eprintln!("hidsniff: capturing, Ctrl-C to stop");
    sniff(&device, out, &stop)
}

fn open() -> Result<HidDevice> {
    open_e2x2().map_err(|failure| anyhow!(message(&failure, Platform::current())))
}

/// Reads input reports until `stop` is set or a read or write fails, then
/// flushes every output.
fn sniff(device: &HidDevice, mut out: Option<LineWriter<File>>, stop: &AtomicBool) -> Result<()> {
    let mut stdout = io::stdout().lock();
    let start = Instant::now();
    let mut buf = [0u8; REPORT_BUFFER_LEN];
    let result = loop {
        if stop.load(Ordering::SeqCst) {
            break Ok(());
        }
        let len = match device.read_timeout(&mut buf, READ_TIMEOUT_MS) {
            Ok(0) => continue,
            Ok(len) => len,
            // Ctrl-C can interrupt the read before the handler sets `stop`; re-check it.
            Err(e) if is_interrupted(&e) => continue,
            Err(e) => break Err(anyhow!("reading from the TOPPING E2x2 failed: {e}")),
        };
        let line = format_line(start.elapsed(), &buf[..len]);
        if let Err(e) = write_line(&mut stdout, out.as_mut(), &line) {
            break Err(e);
        }
    };
    let flushed = flush(&mut stdout, out.as_mut());
    result.and(flushed)
}

fn write_line(
    stdout: &mut impl Write,
    out: Option<&mut LineWriter<File>>,
    line: &str,
) -> Result<()> {
    writeln!(stdout, "{line}").context("writing to stdout")?;
    if let Some(out) = out {
        writeln!(out, "{line}").context("writing the capture file")?;
    }
    Ok(())
}

fn flush(stdout: &mut impl Write, out: Option<&mut LineWriter<File>>) -> Result<()> {
    stdout.flush().context("flushing stdout")?;
    if let Some(out) = out {
        out.flush().context("flushing the capture file")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(OsString::from))
    }

    fn sniff(out: Option<&str>) -> Command {
        Command::Sniff {
            out: out.map(PathBuf::from),
        }
    }

    #[test]
    fn no_arguments_prints_to_stdout_only() {
        assert_eq!(parse(&[]), Ok(sniff(None)));
    }

    #[test]
    fn out_takes_a_file() {
        assert_eq!(parse(&["--out", "cap.txt"]), Ok(sniff(Some("cap.txt"))));
        assert_eq!(parse(&["--out=cap.txt"]), Ok(sniff(Some("cap.txt"))));
    }

    #[test]
    fn help_flags() {
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
    }

    #[test]
    fn rejects_bad_arguments() {
        for args in [
            &["--out"][..],
            &["--out="],
            &["--out", "a", "--out", "b"],
            &["--verbose"],
            &["cap.txt"],
        ] {
            assert!(parse(args).is_err(), "accepted {args:?}");
        }
    }
}
