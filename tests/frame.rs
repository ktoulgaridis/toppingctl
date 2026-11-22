//! Unit tests for `toppingctl::frame`, checked against recorded Mac frames.

use toppingctl::frame::*;

fn hex(s: &str) -> Vec<u8> {
    s.split(' ')
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

fn frame(group: u8, param: u8, value: i32) -> Frame {
    Frame {
        group,
        param,
        value,
    }
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
    for f in [RX_MON_ON, RX_MON_OFF, RX_IDLE, RX_METER] {
        let bytes = hex(f);
        assert_eq!(
            crc16(&bytes[..11]),
            u16::from_be_bytes([bytes[11], bytes[12]]),
            "{f}"
        );
        assert!(Frame::decode(&bytes, Dir::FromDevice).is_ok(), "{f}");
    }
}

#[test]
fn decode_reads_the_fields() {
    assert_eq!(
        Frame::decode(&hex(RX_METER), Dir::FromDevice),
        Ok(frame(0x21, 0x05, -553))
    );
    assert_eq!(
        Frame::decode(&hex(TX_MON_OFF), Dir::ToDevice),
        Ok(frame(0x21, 0x01, 0))
    );
}

#[test]
fn encode_zero_crc_matches_the_app() {
    assert_eq!(
        frame(0x21, 0x01, 0).encode(Crc::Zero).to_vec(),
        hex(TX_MON_OFF)
    );
}

#[test]
fn encode_fill_crc_matches_the_device() {
    assert_eq!(
        frame(0x21, 0x01, 1).encode(Crc::Fill).to_vec(),
        hex(RX_MON_ON)
    );
    assert_eq!(
        frame(0x21, 0x01, 0).encode(Crc::Fill).to_vec(),
        hex(RX_MON_OFF)
    );
}

#[test]
fn fill_is_the_default_crc() {
    assert_eq!(Crc::default(), Crc::Fill);
}

#[test]
fn encode_writes_negative_values_big_endian() {
    // IN2 at 0 dB with Ø on, as the app sent it.
    let report = frame(0x22, 0x04, -0x0200_0000).encode(Crc::Zero);
    assert_eq!(report[7..11], [0xfe, 0x00, 0x00, 0x00]);
}

#[test]
fn decode_round_trips_encode() {
    for f in [
        frame(0x21, 0x04, 0x0284_91d0),
        frame(0x51, 0x11, -0x00ff_fff0),
        frame(0x1a, 0x03, 0),
    ] {
        assert_eq!(Frame::decode(&f.encode(Crc::Fill), Dir::FromDevice), Ok(f));
        assert_eq!(Frame::decode(&f.encode(Crc::Fill), Dir::ToDevice), Ok(f));
        assert_eq!(Frame::decode(&f.encode(Crc::Zero), Dir::ToDevice), Ok(f));
    }
}

#[test]
fn only_48v_on_counts_as_phantom_power() {
    assert!(frame(0x21, 0x02, 1).turns_on_phantom_power());
    assert!(frame(0x22, 0x02, 1).turns_on_phantom_power());
    assert!(!frame(0x21, 0x02, 0).turns_on_phantom_power());
    assert!(!frame(0x21, 0x01, 1).turns_on_phantom_power());
    assert!(!frame(0x51, 0x02, 1).turns_on_phantom_power());
}

#[test]
fn meter_reports_are_recognised() {
    assert!(frame(0x00, 0x00, 0).is_meter());
    assert!(frame(0x21, 0x05, -299).is_meter());
    assert!(frame(0x32, 0x04, 0).is_meter());
    assert!(frame(0x43, 0x02, 0).is_meter());
    assert!(!frame(0x21, 0x01, 1).is_meter());
    assert!(!frame(0x11, 0x01, 1).is_meter());
}

/// AC1: every frame in the capture fixtures (tests/fixtures/frames.tsv, built
/// by tools/fixtures.py) decodes with its direction, and re-encoding it with
/// the CRC it carried (`Fill` if bytes 11–12 are non-zero, else `Zero`) gives
/// the same 16 bytes.
#[test]
fn fixtures_round_trip_typed() {
    let fixtures = include_str!("fixtures/frames.tsv");
    let mut count = 0;
    for line in fixtures.lines().filter(|l| !l.starts_with('#')) {
        let cols: Vec<&str> = line.split('\t').collect();
        let bytes = hex(cols[1]);
        let dir = match cols[0] {
            "rx" => Dir::FromDevice,
            "tx" => Dir::ToDevice,
            other => panic!("unknown direction {other:?}: {line}"),
        };
        let decoded = Frame::decode(&bytes, dir).unwrap_or_else(|e| panic!("{e:?}: {line}"));
        let crc = if bytes[11..13] != [0, 0] {
            Crc::Fill
        } else {
            Crc::Zero
        };
        assert_eq!(decoded.encode(crc).to_vec(), bytes, "{line}");
        count += 1;
    }
    assert!(count > 200, "only {count} fixtures");
}

/// A small deterministic xorshift generator, so the fuzz input is reproducible.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// AC2: malformed frames return the matching error and never panic.
#[test]
fn malformed_frames_are_errors() {
    let rx = hex(RX_MON_ON);
    for dir in [Dir::FromDevice, Dir::ToDevice] {
        assert_eq!(Frame::decode(&[], dir), Err(DecodeError::TooShort));
        assert_eq!(Frame::decode(&rx[..14], dir), Err(DecodeError::TooShort));

        let mut bad_header = rx.clone();
        bad_header[0] = 0x23;
        assert_eq!(Frame::decode(&bad_header, dir), Err(DecodeError::BadHeader));

        let mut bad_trailer = rx.clone();
        bad_trailer[14] = 0x00;
        assert_eq!(
            Frame::decode(&bad_trailer, dir),
            Err(DecodeError::BadTrailer)
        );
    }

    // rx with a corrupted value: the stored CRC no longer matches.
    let mut corrupted = rx.clone();
    corrupted[10] ^= 1;
    let expected = crc16(&corrupted[..11]);
    assert_eq!(
        Frame::decode(&corrupted, Dir::FromDevice),
        Err(DecodeError::BadCrc {
            expected,
            found: 0x8cb8
        })
    );

    // rx without a CRC: the device always sends one.
    let tx = hex(TX_MON_OFF);
    assert_eq!(
        Frame::decode(&tx, Dir::FromDevice),
        Err(DecodeError::BadCrc {
            expected: 0x4c79,
            found: 0
        })
    );

    // tx with a wrong non-zero CRC.
    let mut wrong = tx.clone();
    wrong[11..13].copy_from_slice(&[0x12, 0x34]);
    assert_eq!(
        Frame::decode(&wrong, Dir::ToDevice),
        Err(DecodeError::BadCrc {
            expected: 0x4c79,
            found: 0x1234
        })
    );

    // Random input of 0–64 bytes never panics; random bytes after a valid
    // header and trailer exercise the CRC path too.
    let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
    for i in 0..20_000 {
        let len = (rng.next() % 65) as usize;
        let mut data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        if i % 2 == 0 && len >= 15 {
            data[..5].copy_from_slice(&HEADER);
            data[13..15].copy_from_slice(&TRAILER);
        }
        for dir in [Dir::FromDevice, Dir::ToDevice] {
            let result = Frame::decode(&data, dir);
            if len < 15 {
                assert_eq!(result, Err(DecodeError::TooShort));
            }
        }
    }
}
