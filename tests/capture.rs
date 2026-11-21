//! Unit tests for `toppingctl::capture`.

use std::time::Duration;
use toppingctl::capture::*;

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
