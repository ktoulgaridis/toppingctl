//! Minimal E2x2 frame encoding for the protocol spike (#8).
//!
//! A frame is 15 bytes, padded to the device's 16-byte HID report:
//!
//! ```text
//! 22 33 20 01 01 <group> <param> <value: i32 big-endian> <crc: u16 big-endian> 66 77 [00]
//! ```
//!
//! The device→host frames always carry the CRC16 below. The official app sends
//! `00 00` in the CRC bytes instead. The full codec belongs to #9.

/// Bytes 0–4 of every frame.
pub const HEADER: [u8; 5] = [0x22, 0x33, 0x20, 0x01, 0x01];
/// Bytes 13–14 of every frame.
pub const TRAILER: [u8; 2] = [0x66, 0x77];
/// Size of the device's HID input and output reports.
pub const REPORT_LEN: usize = 16;

/// CRC16 over `data`: poly 0xA001 (reflected), init 0, final XOR 0x785A.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0x785A
}

/// Builds the 16-byte report that sets `group`/`param` to `value`.
///
/// With `with_crc` the CRC16 of bytes 0–10 goes in bytes 11–12; without it
/// they are `00 00`, as the official app sends them.
pub fn encode(group: u8, param: u8, value: i32, with_crc: bool) -> [u8; REPORT_LEN] {
    let mut report = [0u8; REPORT_LEN];
    report[..5].copy_from_slice(&HEADER);
    report[5] = group;
    report[6] = param;
    report[7..11].copy_from_slice(&value.to_be_bytes());
    if with_crc {
        let crc = crc16(&report[..11]);
        report[11..13].copy_from_slice(&crc.to_be_bytes());
    }
    report[13..15].copy_from_slice(&TRAILER);
    report
}

/// The group, param and value of a frame, if it has the frame header and trailer.
pub fn decode(data: &[u8]) -> Option<(u8, u8, i32)> {
    if data.len() < 15 || data[..5] != HEADER || data[13..15] != TRAILER {
        return None;
    }
    let value = i32::from_be_bytes([data[7], data[8], data[9], data[10]]);
    Some((data[5], data[6], value))
}

/// Whether bytes 11–12 hold the CRC16 of bytes 0–10.
pub fn crc_ok(data: &[u8]) -> bool {
    data.len() >= 13 && crc16(&data[..11]) == u16::from_be_bytes([data[11], data[12]])
}

/// Whether setting `group`/`param` to `value` turns on 48V phantom power
/// (`21 02` for IN1, `22 02` for IN2).
pub fn turns_on_phantom_power(group: u8, param: u8, value: i32) -> bool {
    matches!(group, 0x21 | 0x22) && param == 0x02 && value != 0
}

/// Whether a device→host frame is a periodic meter or idle report rather than
/// a state change.
pub fn is_meter(group: u8, param: u8) -> bool {
    matches!(
        (group, param),
        (0x00, 0x00)
            | (0x21 | 0x22, 0x05)
            | (0x31..=0x33, 0x03 | 0x04)
            | (0x41..=0x43, 0x01 | 0x02)
    )
}
