//! Unit tests for `toppingctl::frame`, checked against recorded Mac frames.

use toppingctl::frame::*;

fn hex(s: &str) -> Vec<u8> {
    s.split(' ')
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

// Device→host frames from captures/mac-2/txrx.txt.
const RX_MON_ON: &str = "22 33 20 01 01 21 01 00 00 00 01 8c b8 66 77 00";
const RX_MON_OFF: &str = "22 33 20 01 01 21 01 00 00 00 00 4c 79 66 77 00";
const RX_IDLE: &str = "22 33 20 01 01 00 00 00 00 00 00 3d 42 66 77 00";
const RX_METER: &str = "22 33 20 01 01 21 05 ff ff fd d7 66 88 66 77 00";
// Host→device frame the official app sent (captures/mac-1/tx.txt), padded to 16.
const TX_MON_OFF: &str = "22 33 20 01 01 21 01 00 00 00 00 00 00 66 77 00";

#[test]
fn crc16_matches_recorded_device_frames() {
    for frame in [RX_MON_ON, RX_MON_OFF, RX_IDLE, RX_METER] {
        assert!(crc_ok(&hex(frame)), "{frame}");
    }
}

#[test]
fn crc_ok_rejects_a_corrupted_frame() {
    let mut frame = hex(RX_MON_ON);
    frame[10] ^= 1;
    assert!(!crc_ok(&frame));
}

#[test]
fn encode_without_crc_matches_the_app() {
    assert_eq!(encode(0x21, 0x01, 0, false).to_vec(), hex(TX_MON_OFF));
}

#[test]
fn encode_with_crc_matches_the_device() {
    assert_eq!(encode(0x21, 0x01, 1, true).to_vec(), hex(RX_MON_ON));
    assert_eq!(encode(0x21, 0x01, 0, true).to_vec(), hex(RX_MON_OFF));
}

#[test]
fn encode_writes_negative_values_big_endian() {
    // IN2 at 0 dB with Ø on, as the app sent it.
    let report = encode(0x22, 0x04, -0x0200_0000, false);
    assert_eq!(report[7..11], [0xfe, 0x00, 0x00, 0x00]);
}

#[test]
fn decode_round_trips_encode() {
    for (g, p, v) in [
        (0x21, 0x04, 0x0284_91d0),
        (0x51, 0x11, -0x00ff_fff0),
        (0x1a, 0x03, 0),
    ] {
        assert_eq!(decode(&encode(g, p, v, true)), Some((g, p, v)));
    }
}

#[test]
fn decode_rejects_frames_without_header_or_trailer() {
    let mut frame = hex(RX_MON_ON);
    frame[0] = 0x23;
    assert_eq!(decode(&frame), None);
    let mut frame = hex(RX_MON_ON);
    frame[14] = 0x00;
    assert_eq!(decode(&frame), None);
    assert_eq!(decode(&hex("22 33 20 01 01")), None);
}

#[test]
fn only_48v_on_counts_as_phantom_power() {
    assert!(turns_on_phantom_power(0x21, 0x02, 1));
    assert!(turns_on_phantom_power(0x22, 0x02, 1));
    assert!(!turns_on_phantom_power(0x21, 0x02, 0));
    assert!(!turns_on_phantom_power(0x21, 0x01, 1));
    assert!(!turns_on_phantom_power(0x51, 0x02, 1));
}

#[test]
fn meter_reports_are_recognised() {
    assert!(is_meter(0x00, 0x00));
    assert!(is_meter(0x21, 0x05));
    assert!(is_meter(0x32, 0x04));
    assert!(is_meter(0x43, 0x02));
    assert!(!is_meter(0x21, 0x01));
    assert!(!is_meter(0x11, 0x01));
}

/// Every frame in the capture fixtures (tests/fixtures/frames.tsv, built by
/// tools/fixtures.py): rx frames carry a valid CRC16, tx frames carry either
/// `00 00` or the CRC16, and re-encoding the decoded fields gives the same bytes.
#[test]
fn every_capture_fixture_round_trips() {
    let fixtures = include_str!("fixtures/frames.tsv");
    let mut count = 0;
    for line in fixtures.lines().filter(|l| !l.starts_with('#')) {
        let cols: Vec<&str> = line.split('\t').collect();
        let (dir, frame) = (cols[0], hex(cols[1]));
        let (g, p, v) = decode(&frame).unwrap_or_else(|| panic!("decode: {line}"));
        let with_crc = frame[11..13] != [0, 0];
        if dir == "rx" {
            assert!(crc_ok(&frame), "rx crc: {line}");
        } else if with_crc {
            assert!(crc_ok(&frame), "tx crc: {line}");
        }
        assert_eq!(encode(g, p, v, with_crc).to_vec(), frame, "{line}");
        count += 1;
    }
    assert!(count > 200, "only {count} fixtures");
}
