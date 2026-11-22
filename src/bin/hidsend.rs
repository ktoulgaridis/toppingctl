//! Protocol-spike tool (#8): sends E2x2 frames and prints what the device
//! reports back.
//!
//! Each `GG:PP=VALUE` argument becomes one frame (group `GG`, param `PP`, both
//! hex; `VALUE` decimal or `0x`-hex, optionally negative). After each frame it
//! listens for `--listen` milliseconds and prints the device's non-meter
//! reports, in the capture line format with a `tx`/`rx` direction and the CRC
//! check result. 48V phantom power is never turned on without `--allow-48v`.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use hidapi::HidDevice;
use toppingctl::error::{is_interrupted, message, Platform};
use toppingctl::frame::{crc_ok, decode, encode, is_meter, turns_on_phantom_power};
use toppingctl::open::open_e2x2;

const USAGE: &str = "\
Usage: hidsend [--crc] [--listen <ms>] [--all] [--allow-48v] <GG:PP=VALUE>...

Sends one frame per GG:PP=VALUE to the TOPPING E2x2 (GG, PP hex; VALUE
decimal or 0x-hex, may be negative) and prints the device's replies.

Options:
  --crc          fill bytes 11-12 with the CRC16 (default: 00 00, like the app)
  --listen <ms>  how long to listen after each frame (default 500)
  --all          also print meter and idle reports
  --allow-48v    allow frames that turn 48V phantom power on
  -h, --help     show this help

Example: hidsend 21:01=1 21:01=0     (IN1 MON on, then off)";

const DEFAULT_LISTEN_MS: u64 = 500;
const READ_TIMEOUT_MS: i32 = 50;
const REPORT_BUFFER_LEN: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Set {
    group: u8,
    param: u8,
    value: i32,
}

#[derive(Debug, PartialEq, Eq)]
struct Options {
    crc: bool,
    listen: Duration,
    all: bool,
    sets: Vec<Set>,
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Send(Options),
    Help,
}

fn parse_set(s: &str) -> Result<Set, String> {
    let bad = || format!("expected GG:PP=VALUE, got {s:?}");
    let (addr, value) = s.split_once('=').ok_or_else(bad)?;
    let (group, param) = addr.split_once(':').ok_or_else(bad)?;
    let hex_byte = |h: &str| {
        (h.len() == 2)
            .then(|| u8::from_str_radix(h, 16).ok())
            .flatten()
            .ok_or_else(bad)
    };
    let (negative, digits) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    let magnitude = match digits.strip_prefix("0x") {
        Some(h) => i64::from_str_radix(h, 16),
        None => digits.parse::<i64>(),
    }
    .map_err(|_| bad())?;
    let value = if negative { -magnitude } else { magnitude };
    // Accept the full u32 range too, so raw register words can be pasted.
    let value = i32::try_from(value)
        .or_else(|_| u32::try_from(value).map(|v| v as i32))
        .map_err(|_| format!("value out of range in {s:?}"))?;
    Ok(Set {
        group: hex_byte(group)?,
        param: hex_byte(param)?,
        value,
    })
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Command, String> {
    let mut opts = Options {
        crc: false,
        listen: Duration::from_millis(DEFAULT_LISTEN_MS),
        all: false,
        sets: Vec::new(),
    };
    let mut allow_48v = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|a| format!("unexpected argument {a:?}"))?;
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--crc" => opts.crc = true,
            "--all" => opts.all = true,
            "--allow-48v" => allow_48v = true,
            "--listen" => {
                let ms = args
                    .next()
                    .and_then(|v| v.into_string().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .ok_or("--listen needs milliseconds")?;
                opts.listen = Duration::from_millis(ms);
            }
            a if a.starts_with('-') && !a.contains('=') => {
                return Err(format!("unexpected argument {a:?}"))
            }
            a => opts.sets.push(parse_set(a)?),
        }
    }
    if opts.sets.is_empty() {
        return Err("give at least one GG:PP=VALUE".into());
    }
    if !allow_48v {
        if let Some(set) = opts
            .sets
            .iter()
            .find(|s| turns_on_phantom_power(s.group, s.param, s.value))
        {
            return Err(format!(
                "{:02x}:{:02x}={} turns 48V phantom power on: unplug mics and pass --allow-48v",
                set.group, set.param, set.value
            ));
        }
    }
    Ok(Command::Send(opts))
}

fn main() -> ExitCode {
    let command = match parse_args(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("hidsend: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Command::Send(opts) => run(&opts),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("hidsend: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn line(start: Instant, dir: &str, data: &[u8]) -> String {
    let t = start.elapsed();
    let mut s = format!("{}.{:06} {dir}", t.as_secs(), t.subsec_micros());
    for byte in data {
        s.push_str(&format!(" {byte:02x}"));
    }
    s
}

fn run(opts: &Options) -> Result<()> {
    let device = open_e2x2().map_err(|failure| anyhow!(message(&failure, Platform::current())))?;
    let start = Instant::now();
    for set in &opts.sets {
        let report = encode(set.group, set.param, set.value, opts.crc);
        // No report IDs on this device: hidapi wants a leading 0 byte.
        let mut out = Vec::with_capacity(report.len() + 1);
        out.push(0);
        out.extend_from_slice(&report);
        device
            .write(&out)
            .map_err(|e| anyhow!("writing to the TOPPING E2x2 failed: {e}"))?;
        println!("{}", line(start, "tx", &report));
        listen(&device, start, opts)?;
    }
    Ok(())
}

fn listen(device: &HidDevice, start: Instant, opts: &Options) -> Result<()> {
    let until = Instant::now() + opts.listen;
    let mut buf = [0u8; REPORT_BUFFER_LEN];
    while Instant::now() < until {
        let len = match device.read_timeout(&mut buf, READ_TIMEOUT_MS) {
            Ok(0) => continue,
            Ok(len) => len,
            Err(e) if is_interrupted(&e) => continue,
            Err(e) => return Err(anyhow!("reading from the TOPPING E2x2 failed: {e}")),
        };
        let data = &buf[..len];
        let meter = decode(data).is_some_and(|(g, p, _)| is_meter(g, p));
        if opts.all || !meter {
            let crc = if crc_ok(data) { "crc-ok" } else { "crc-BAD" };
            println!("{}  {crc}", line(start, "rx", data));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(OsString::from))
    }

    fn sets(cmd: Command) -> Vec<Set> {
        match cmd {
            Command::Send(o) => o.sets,
            Command::Help => panic!("expected Send"),
        }
    }

    #[test]
    fn parses_decimal_hex_and_negative_values() {
        assert_eq!(
            sets(parse(&["21:01=1", "21:04=0x028491d0", "22:04=-0x2000000"]).unwrap()),
            vec![
                Set {
                    group: 0x21,
                    param: 0x01,
                    value: 1
                },
                Set {
                    group: 0x21,
                    param: 0x04,
                    value: 0x0284_91d0
                },
                Set {
                    group: 0x22,
                    param: 0x04,
                    value: -0x0200_0000
                },
            ]
        );
    }

    #[test]
    fn accepts_raw_u32_words() {
        assert_eq!(
            sets(parse(&["22:04=0xfe000000"]).unwrap())[0].value,
            -0x0200_0000
        );
    }

    #[test]
    fn rejects_malformed_sets() {
        for bad in [
            "21-01=1",
            "21:01",
            "2:01=1",
            "21:zz=1",
            "21:01=x",
            "21:01=0x1ffffffff",
        ] {
            assert!(parse(&[bad]).is_err(), "{bad}");
        }
    }

    #[test]
    fn needs_at_least_one_set() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["--crc"]).is_err());
    }

    #[test]
    fn refuses_48v_on_without_the_flag() {
        assert!(parse(&["21:02=1"]).is_err());
        assert!(parse(&["22:01=0", "22:02=1"]).is_err());
        assert!(parse(&["21:02=0"]).is_ok());
        assert!(parse(&["--allow-48v", "21:02=1"]).is_ok());
    }

    #[test]
    fn parses_options() {
        match parse(&["--crc", "--all", "--listen", "200", "21:01=1"]).unwrap() {
            Command::Send(o) => {
                assert!(o.crc && o.all);
                assert_eq!(o.listen, Duration::from_millis(200));
            }
            Command::Help => panic!("expected Send"),
        }
        assert!(parse(&["--listen"]).is_err());
        assert!(parse(&["--listen", "x", "21:01=1"]).is_err());
        assert_eq!(parse(&["-h"]), Ok(Command::Help));
    }
}
