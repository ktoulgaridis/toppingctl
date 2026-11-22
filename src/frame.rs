//! The E2x2 frame codec.
//!
//! A frame is 15 bytes, padded to the device's 16-byte HID report:
//!
//! ```text
//! 22 33 20 01 01 <group> <param> <value: i32 big-endian> <crc: u16 big-endian> 66 77 [00]
//! ```
//!
//! Device→host frames always carry the CRC16 of bytes 0–10. The official app
//! sends `00 00` in the CRC bytes instead; the device accepts both (PROTOCOL.md).

use std::fmt;

/// Bytes 0–4 of every frame.
pub const HEADER: [u8; 5] = [0x22, 0x33, 0x20, 0x01, 0x01];
/// Bytes 13–14 of every frame.
pub const TRAILER: [u8; 2] = [0x66, 0x77];
/// Size of the device's HID input and output reports.
pub const REPORT_LEN: usize = 16;
/// Size of a frame without the report's padding byte.
pub const FRAME_LEN: usize = 15;

/// One frame: set (or report) `group`/`param` to `value`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub group: u8,
    pub param: u8,
    pub value: i32,
}

/// What [`Frame::encode`] writes in the CRC bytes (11–12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Crc {
    /// The CRC16 of bytes 0–10. The device accepts it and echoes the frame
    /// byte for byte.
    #[default]
    Fill,
    /// `00 00`, as the official app sends it.
    Zero,
}

/// Which way a frame travelled, which decides the CRC rule in [`Frame::decode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Device→host: the CRC16 is required.
    FromDevice,
    /// Host→device: `00 00` or the CRC16.
    ToDevice,
}

/// Why bytes are not a valid frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Fewer than [`FRAME_LEN`] bytes.
    TooShort,
    /// Bytes 0–4 are not [`HEADER`].
    BadHeader,
    /// Bytes 13–14 are not [`TRAILER`].
    BadTrailer,
    /// Bytes 11–12 hold `found` where the direction requires `expected`.
    BadCrc { expected: u16, found: u16 },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(f, "frame shorter than {FRAME_LEN} bytes"),
            Self::BadHeader => write!(f, "frame header is not 22 33 20 01 01"),
            Self::BadTrailer => write!(f, "frame trailer is not 66 77"),
            Self::BadCrc { expected, found } => {
                write!(f, "frame CRC is {found:04x}, expected {expected:04x}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

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

impl Frame {
    /// The 16-byte report for this frame, with `crc` in bytes 11–12.
    pub fn encode(self, crc: Crc) -> [u8; REPORT_LEN] {
        let mut report = [0u8; REPORT_LEN];
        report[..5].copy_from_slice(&HEADER);
        report[5] = self.group;
        report[6] = self.param;
        report[7..11].copy_from_slice(&self.value.to_be_bytes());
        if crc == Crc::Fill {
            let crc = crc16(&report[..11]);
            report[11..13].copy_from_slice(&crc.to_be_bytes());
        }
        report[13..15].copy_from_slice(&TRAILER);
        report
    }

    /// Parses the first [`FRAME_LEN`] bytes of `data` as a frame sent in `dir`.
    /// Bytes after the frame (the report's padding) are ignored.
    pub fn decode(data: &[u8], dir: Dir) -> Result<Frame, DecodeError> {
        let data = data.get(..FRAME_LEN).ok_or(DecodeError::TooShort)?;
        if data[..5] != HEADER {
            return Err(DecodeError::BadHeader);
        }
        if data[13..15] != TRAILER {
            return Err(DecodeError::BadTrailer);
        }
        let expected = crc16(&data[..11]);
        let found = u16::from_be_bytes([data[11], data[12]]);
        let zero_allowed = dir == Dir::ToDevice && found == 0;
        if found != expected && !zero_allowed {
            return Err(DecodeError::BadCrc { expected, found });
        }
        Ok(Frame {
            group: data[5],
            param: data[6],
            value: i32::from_be_bytes([data[7], data[8], data[9], data[10]]),
        })
    }

    /// Whether this frame turns on 48V phantom power (`21 02` for IN1,
    /// `22 02` for IN2).
    pub fn turns_on_phantom_power(&self) -> bool {
        matches!(self.group, 0x21 | 0x22) && self.param == 0x02 && self.value != 0
    }

    /// Whether this device→host frame is a periodic meter or idle report rather
    /// than a state change.
    pub fn is_meter(&self) -> bool {
        matches!(
            (self.group, self.param),
            (0x00, 0x00)
                | (0x21 | 0x22, 0x05)
                | (0x31..=0x33, 0x03 | 0x04)
                | (0x41..=0x43, 0x01 | 0x02)
        )
    }
}
