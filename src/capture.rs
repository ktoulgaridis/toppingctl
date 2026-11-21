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
    let bytes = rest.strip_prefix("rx ").ok_or(err("expected `rx` and data"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn us(micros: u64) -> Duration {
        Duration::from_micros(micros)
    }

    #[test]
    fn formats_elapsed_with_six_decimals_and_lowercase_hex() {
        assert_eq!(
            format_line(us(12_034_517), &[0x01, 0x0a, 0xff]),
            "12.034517 rx 01 0a ff"
        );
        assert_eq!(format_line(Duration::ZERO, &[0xAB]), "0.000000 rx ab");
    }

    #[test]
    fn truncates_sub_microsecond_precision() {
        let t = Duration::new(3, 123_456_999);
        assert_eq!(format_line(t, &[0x00]), "3.123456 rx 00");
    }

    #[test]
    fn round_trips_a_sixteen_byte_frame() {
        let data: Vec<u8> = (0u8..16).map(|i| i.wrapping_mul(17)).collect();
        let line = format_line(us(1_500_000), &data);
        assert_eq!(
            line,
            "1.500000 rx 00 11 22 33 44 55 66 77 88 99 aa bb cc dd ee ff"
        );
        assert_eq!(
            parse_line(&line),
            Ok(Frame {
                elapsed: us(1_500_000),
                data
            })
        );
    }

    #[test]
    fn round_trips_sample_lines() {
        for line in [
            "0.000000 rx 00",
            "0.000001 rx 7f 80",
            "12.034517 rx 01 0a ff",
            "3600.999999 rx de ad be ef",
        ] {
            let frame = parse_line(line).unwrap_or_else(|e| panic!("{line:?}: {e}"));
            assert_eq!(format_line(frame.elapsed, &frame.data), line);
        }
    }

    #[test]
    fn parses_fields() {
        assert_eq!(
            parse_line("12.034517 rx 01 0a ff"),
            Ok(Frame {
                elapsed: us(12_034_517),
                data: vec![0x01, 0x0a, 0xff]
            })
        );
    }

    #[test]
    fn rejects_malformed_lines() {
        for line in [
            "",
            "rx 01",
            "12.034517",
            "12.034517 rx",
            "12.034517 rx ",
            "12.034517 tx 01",
            "12.034517 RX 01",
            "12.03451 rx 01",
            "12.0345170 rx 01",
            "12 rx 01",
            ".034517 rx 01",
            "-1.000000 rx 01",
            "+1.000000 rx 01",
            "1.00000a rx 01",
            "12.034517 rx 0A",
            "12.034517 rx 1",
            "12.034517 rx 001",
            "12.034517 rx zz",
            "12.034517 rx +1",
            "12.034517 rx 01  02",
            "12.034517  rx 01",
            " 12.034517 rx 01",
            "12.034517 rx 01 ",
            "12.034517 rx 01\n",
            "99999999999999999999.000000 rx 01",
        ] {
            assert!(parse_line(line).is_err(), "accepted {line:?}");
        }
    }
}
