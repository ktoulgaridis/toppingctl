//! Capture line format shared by `hidsniff` and test fixtures.
//!
//! One line per HID input report:
//!
//! ```text
//! <elapsed seconds, 6 decimals> rx <lowercase hex bytes, space-separated>
//! ```
//!
//! For example `12.034517 rx 01 0a ff`.

use std::fmt;
use std::time::Duration;

/// One input report together with the time it arrived, relative to capture start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub elapsed: Duration,
    pub data: Vec<u8>,
}

/// Why a line is not a valid capture line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub reason: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed capture line: {}", self.reason)
    }
}

impl std::error::Error for ParseError {}

/// Formats one input report as a capture line (no trailing newline).
///
/// The elapsed time is truncated to whole microseconds. `data` should be
/// non-empty: [`parse_line`] rejects a line without bytes.
pub fn format_line(elapsed: Duration, data: &[u8]) -> String {
    let mut line = format!("{}.{:06} rx", elapsed.as_secs(), elapsed.subsec_micros());
    for byte in data {
        line.push_str(&format!(" {byte:02x}"));
    }
    line
}

/// Parses a capture line produced by [`format_line`] (without its newline).
pub fn parse_line(line: &str) -> Result<Frame, ParseError> {
    let (timestamp, rest) = line.split_once(' ').ok_or(err("missing direction"))?;
    let elapsed = parse_elapsed(timestamp)?;
    let bytes = rest
        .strip_prefix("rx ")
        .ok_or(err("expected `rx` and data"))?;
    let data = bytes.split(' ').map(parse_byte).collect::<Result<_, _>>()?;
    Ok(Frame { elapsed, data })
}

fn err(reason: &'static str) -> ParseError {
    ParseError { reason }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn parse_elapsed(s: &str) -> Result<Duration, ParseError> {
    let (secs, micros) = s.split_once('.').ok_or(err("timestamp needs 6 decimals"))?;
    if !is_digits(secs) || !is_digits(micros) || micros.len() != 6 {
        return Err(err("timestamp must be <seconds>.<6 digits>"));
    }
    let secs: u64 = secs.parse().map_err(|_| err("timestamp out of range"))?;
    let micros: u32 = micros.parse().map_err(|_| err("bad microseconds"))?;
    Ok(Duration::new(secs, micros * 1_000))
}

fn parse_byte(s: &str) -> Result<u8, ParseError> {
    let lower_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    if s.len() != 2 || !s.bytes().all(lower_hex) {
        return Err(err("bytes must be two lowercase hex digits"));
    }
    u8::from_str_radix(s, 16).map_err(|_| err("bad hex byte"))
}
