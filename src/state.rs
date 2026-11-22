//! The typed model of the whole device, and its mapping to frames.
//!
//! [`State`] holds app-level values (dB, pan, mute, solo, Ø), not wire values.
//! [`State::frames`] folds them into register writes the way the official app
//! does (PROTOCOL.md, `captures/mac-1`): SOLO, MUTE, Ø, pan and Link have no
//! registers of their own and only change the levels that are written.
//!
//! Mapping rules taken from the captures, and the choices made where the
//! captures don't settle a detail:
//!
//! - **Input gain (`2x 04`)**: the app's own level word for each whole dB
//!   from +0 to +20 ([`INPUT_GAIN_LEVELS`], all 21 seen in `captures/mac-1`).
//!   Muted → 0; another input soloed (and this one not) → 0; Ø → negated.
//! - **Crosspoints (`5x <o><s>`)**: the app's 0 dB is `0x01ffffe0`, and its
//!   fader levels are `round(335544 · 10^(dB/20)) · 100` (every fader value
//!   in the capture fits this), capped at the app's fader top `0x03fb2784`.
//!   Pan is then a linear split ([`value::pan_split`]), so centre gives each
//!   side `0x00fffff0`.
//! - **Stereo PB strips**: the app has one pan per channel (odd channel hard
//!   L, even hard R by default). [`Strip::pan`] is a single balance `b`
//!   instead: the odd channel pans to `max(0, 2b − 1)` and the even channel
//!   to `min(1, 2b)`. Centre reproduces the app's default exactly, and moving
//!   the balance off centre gives the same frames as the app's PB2 pan
//!   (left of centre) or PB1 pan (right of centre). Per-channel pan settings
//!   that are not a balance can't be expressed.
//! - **Link** puts IN1 hard L and IN2 hard R in that mix, overriding their pan.
//! - **Solo** in a mix writes 0 for every strip of that mix that is not soloed.
//! - **Output and loopback levels** use [`value::level_from_db`] (0 dB =
//!   `0x02000000`, as the app writes). Away from 0 dB the app's own words
//!   differ from it by about one part in a million.
//! - **Output switches**: when headphones or line out change, [`State::diff`]
//!   sends both switches and then the output level, as the app does.
//! - **48V** is never in [`State::frames`] or [`State::diff`]; it is not
//!   persisted either. It goes through `Connection::send_confirmed_phantom`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::frame::Frame;
use crate::value;

/// The app's level word for input gain +0 … +20 dB, indexed by dB.
pub const INPUT_GAIN_LEVELS: [i32; 21] = [
    0x0200_0000,
    0x023e_7928,
    0x0284_91d0,
    0x02d3_382c,
    0x032b_771c,
    0x038e_7a98,
    0x03fd_92f8,
    0x047a_39a8,
    0x0506_15e8,
    0x05a3_0318,
    0x0653_1618,
    0x0718_a508,
    0x07f6_4f18,
    0x08ef_0520,
    0x0a06_1410,
    0x0b3f_3000,
    0x0c9e_8060,
    0x0e28_aec0,
    0x0fe2_f5e0,
    0x11d3_3460,
    0x1400_0000,
];

/// The app's crosspoint level for 0 dB.
pub const CROSSPOINT_ZERO_DB: i32 = 0x01ff_ffe0;
/// The top of the app's mix faders (≈ +6 dB).
pub const CROSSPOINT_MAX: i32 = 0x03fb_2784;

/// The app's crosspoint level for a fader at `db` (`-inf` gives 0).
pub fn crosspoint_level(db: f64) -> i32 {
    if db.is_nan() || db == f64::NEG_INFINITY {
        return 0;
    }
    let steps = (f64::from(CROSSPOINT_ZERO_DB / 100) * 10f64.powf(db / 20.0)).round();
    (steps * 100.0).min(f64::from(CROSSPOINT_MAX)) as i32
}

/// The whole device, as toppingctl sets it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// IN1, IN2.
    pub inputs: [Input; 2],
    /// Mix A, B, C.
    pub mixes: [Mix; 3],
    /// Loopback 1+2, 3+4.
    pub loopbacks: [Loopback; 2],
    pub output: Output,
    /// Phone Out Gain (the device's gain button).
    pub phone_gain: bool,
    pub system: System,
    /// What the device reports; not persisted.
    #[serde(skip)]
    pub readout: Readout,
}

/// One input channel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    /// Gain, 0 ..= 20 dB (higher values are treated as 20).
    pub gain_db: u8,
    pub mute: bool,
    pub solo: bool,
    /// Ø: phase inverted.
    pub phase: bool,
    pub mon: bool,
    /// INST (Hi-Z).
    pub inst: bool,
    /// 48V phantom power: `None` until the device reports it. Never
    /// persisted and never pushed.
    #[serde(skip)]
    pub phantom: Option<bool>,
}

/// One channel strip of a mix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Strip {
    /// Fader, `-inf` ..= ≈ +6 dB. Stored as `null` in JSON when `-inf`.
    #[serde(with = "db_serde")]
    pub fader_db: f64,
    /// 0 = left, 0.5 = centre, 1 = right. A balance on stereo PB strips.
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
    pub phase: bool,
}

/// One of the three mixes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mix {
    pub in1: Strip,
    pub in2: Strip,
    pub pb12: Strip,
    pub pb34: Strip,
    pub pb56: Strip,
    /// IN1+2 linked: IN1 hard left, IN2 hard right.
    pub link: bool,
}

/// One loopback pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Loopback {
    pub source: Source,
    /// Level, `-inf` ..= 0 dB.
    #[serde(with = "db_serde")]
    pub level_db: f64,
    pub mute: bool,
}

/// Output 1+2.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Output {
    /// The app offers the mixes here; the register accepts any source.
    pub source: Source,
    /// Level, `-inf` ..= 0 dB.
    #[serde(with = "db_serde")]
    pub level_db: f64,
    pub headphones: bool,
    pub line: bool,
}

/// Device settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct System {
    /// Display brightness 0 ..= 2 (1 = Normal).
    pub brightness: u8,
    pub auto_standby: bool,
    pub mobile_apps: bool,
}

/// Read-only values the device reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Readout {
    /// The volume knob's raw step count.
    pub volume_knob: Option<i32>,
    pub hardware_version: Option<Version>,
    pub firmware_version: Option<Version>,
}

/// A device version, `0x00MM00mm` on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u8,
    pub minor: u8,
}

impl Version {
    fn from_wire(value: i32) -> Self {
        Version {
            major: (value >> 16) as u8,
            minor: value as u8,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "V{}.{:02}", self.major, self.minor)
    }
}

/// A source for the output or a loopback (`3x 05`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    MixA,
    MixB,
    MixC,
    Pb12,
    Pb34,
    Pb56,
    In1,
    In2,
    In12,
}

impl Source {
    /// The register value.
    pub fn wire(self) -> i32 {
        match self {
            Source::MixA => 0x10,
            Source::MixB => 0x20,
            Source::MixC => 0x30,
            Source::Pb12 => 0x100,
            Source::Pb34 => 0x200,
            Source::Pb56 => 0x300,
            Source::In1 => 0x1000,
            Source::In2 => 0x2000,
            Source::In12 => 0x3000,
        }
    }
}

/// The official app's launch state (`captures/mac-1/tx.txt`), except the
/// inputs, which start at +0 dB and unmuted.
impl Default for State {
    fn default() -> Self {
        State {
            inputs: Default::default(),
            mixes: Default::default(),
            loopbacks: [
                Loopback {
                    source: Source::Pb34,
                    ..Loopback::default()
                },
                Loopback {
                    source: Source::Pb56,
                    ..Loopback::default()
                },
            ],
            output: Output::default(),
            phone_gain: true,
            system: System::default(),
            readout: Readout::default(),
        }
    }
}

impl Default for Strip {
    fn default() -> Self {
        Strip {
            fader_db: 0.0,
            pan: 0.5,
            mute: false,
            solo: false,
            phase: false,
        }
    }
}

impl Default for Loopback {
    fn default() -> Self {
        Loopback {
            source: Source::Pb12,
            level_db: 0.0,
            mute: false,
        }
    }
}

impl Default for Output {
    fn default() -> Self {
        Output {
            source: Source::MixA,
            level_db: 0.0,
            headphones: true,
            line: true,
        }
    }
}

impl Default for System {
    fn default() -> Self {
        System {
            brightness: 1,
            auto_standby: false,
            mobile_apps: false,
        }
    }
}

fn frame(group: u8, param: u8, value: i32) -> Frame {
    Frame {
        group,
        param,
        value,
    }
}

/// A level for `db` that is 0 when muted or at `-inf`.
fn level(db: f64, mute: bool) -> i32 {
    if mute || db.is_nan() || db == f64::NEG_INFINITY {
        0
    } else {
        value::level_from_db(db)
    }
}

impl Input {
    fn level(&self, other_soloed: bool) -> i32 {
        if self.mute || (other_soloed && !self.solo) {
            return 0;
        }
        let gain = INPUT_GAIN_LEVELS[usize::from(self.gain_db.min(20))];
        value::phase(gain, self.phase)
    }
}

impl Strip {
    /// The strip's level before pan, with MUTE, SOLO and Ø folded in.
    fn level(&self, mix_has_solo: bool) -> i32 {
        if self.mute || (mix_has_solo && !self.solo) {
            return 0;
        }
        value::phase(crosspoint_level(self.fader_db), self.phase)
    }
}

impl Mix {
    /// The 16 crosspoint values as `(source 1..=8, left, right)`.
    fn crosspoints(&self) -> [(u8, i32, i32); 8] {
        let soloed = [&self.in1, &self.in2, &self.pb12, &self.pb34, &self.pb56]
            .iter()
            .any(|s| s.solo);
        let (in1_pan, in2_pan) = if self.link {
            (0.0, 1.0)
        } else {
            (self.in1.pan, self.in2.pan)
        };
        let mut out = [(0, 0, 0); 8];
        let (l, r) = value::pan_split(self.in1.level(soloed), in1_pan);
        out[0] = (1, l, r);
        let (l, r) = value::pan_split(self.in2.level(soloed), in2_pan);
        out[1] = (2, l, r);
        for (i, strip) in [&self.pb12, &self.pb34, &self.pb56].into_iter().enumerate() {
            // A balance: the odd channel leaves hard left only right of
            // centre, the even channel leaves hard right only left of it.
            let b = strip.pan.clamp(0.0, 1.0);
            let lvl = strip.level(soloed);
            let odd = 3 + 2 * i as u8;
            let (l, r) = value::pan_split(lvl, (2.0 * b - 1.0).max(0.0));
            out[2 + 2 * i] = (odd, l, r);
            let (l, r) = value::pan_split(lvl, (2.0 * b).min(1.0));
            out[3 + 2 * i] = (odd + 1, l, r);
        }
        out
    }
}

impl State {
    /// Every writable register, in the app's launch order, then the
    /// settings. 48V, `10 06` and `1a 03` are not included.
    pub fn frames(&self) -> Vec<Frame> {
        let mut out = Vec::with_capacity(80);
        for (i, input) in self.inputs.iter().enumerate() {
            let g = 0x21 + i as u8;
            out.push(frame(g, 0x01, value::bool_value(input.mon)));
            out.push(frame(g, 0x03, value::bool_value(input.inst)));
        }
        for (i, input) in self.inputs.iter().enumerate() {
            let other_soloed = self.inputs[1 - i].solo;
            out.push(frame(0x21 + i as u8, 0x04, input.level(other_soloed)));
        }
        for (m, mix) in self.mixes.iter().enumerate() {
            let g = 0x51 + m as u8;
            for (s, l, r) in mix.crosspoints() {
                out.push(frame(g, 0x10 | s, l));
                out.push(frame(g, 0x20 | s, r));
            }
        }
        out.push(frame(0x31, 0x05, self.output.source.wire()));
        out.push(frame(0x32, 0x05, self.loopbacks[0].source.wire()));
        out.push(frame(0x33, 0x05, self.loopbacks[1].source.wire()));
        out.extend(self.output_level_frames());
        for (i, lb) in self.loopbacks.iter().enumerate() {
            let lvl = level(lb.level_db, lb.mute);
            out.push(frame(0x32 + i as u8, 0x01, lvl));
            out.push(frame(0x32 + i as u8, 0x02, lvl));
        }
        out.push(frame(0x11, 0x02, value::bool_value(self.phone_gain)));
        out.extend(self.output_switch_frames());
        out.push(frame(0x10, 0x0a, i32::from(self.system.brightness.min(2))));
        out.push(frame(
            0x10,
            0x0b,
            value::bool_value(self.system.auto_standby),
        ));
        out.push(frame(
            0x10,
            0x0c,
            value::bool_value(self.system.mobile_apps),
        ));
        out
    }

    fn output_level_frames(&self) -> [Frame; 2] {
        let lvl = level(self.output.level_db.min(0.0), false);
        [frame(0x31, 0x01, lvl), frame(0x31, 0x02, lvl)]
    }

    fn output_switch_frames(&self) -> [Frame; 2] {
        [
            frame(0x34, 0x01, value::bool_value(self.output.headphones)),
            frame(0x34, 0x02, value::bool_value(self.output.line)),
        ]
    }

    /// The frames that take the device from `self` to `new`: only the
    /// registers whose value changes, in [`State::frames`] order. A change
    /// to headphones or line out sends both switches and then the output
    /// level, as the app does.
    pub fn diff(&self, new: &State) -> Vec<Frame> {
        let switches_changed = self.output_switch_frames() != new.output_switch_frames();
        let mut out: Vec<Frame> = self
            .frames()
            .into_iter()
            .zip(new.frames())
            .filter(|(old, new)| old != new && new.group != 0x34)
            .map(|(_, new)| new)
            .collect();
        if switches_changed {
            out.retain(|f| f.group != 0x31 || !matches!(f.param, 0x01 | 0x02));
            out.extend(new.output_switch_frames());
            out.extend(new.output_level_frames());
        }
        out
    }

    /// Updates the state from a device report: MON, INST, 48V and Phone Gain
    /// echoes, the settings, the volume knob and the versions. Meters,
    /// levels and unknown registers are ignored.
    pub fn apply(&mut self, f: &Frame) {
        let on = value::value_bool(f.value);
        match (f.group, f.param) {
            (g @ (0x21 | 0x22), p) => {
                let input = &mut self.inputs[usize::from(g - 0x21)];
                match p {
                    0x01 => input.mon = on,
                    0x02 => input.phantom = Some(on),
                    0x03 => input.inst = on,
                    _ => {}
                }
            }
            (0x11, 0x02) => self.phone_gain = on,
            (0x11, 0x01) => self.readout.volume_knob = Some(f.value),
            (0x10, 0x03) => self.readout.hardware_version = Some(Version::from_wire(f.value)),
            (0x10, 0x04) => self.readout.firmware_version = Some(Version::from_wire(f.value)),
            (0x10, 0x0a) => self.system.brightness = f.value.clamp(0, 2) as u8,
            (0x10, 0x0b) => self.system.auto_standby = on,
            (0x10, 0x0c) => self.system.mobile_apps = on,
            _ => {}
        }
    }
}

/// The meter floor, in dB (the device reports about −96.0 at silence).
pub const METER_FLOOR_DB: f64 = -96.0;

/// The latest meter readings, in dBFS.
#[derive(Clone, Debug, PartialEq)]
pub struct Meters {
    /// IN1, IN2.
    pub inputs: [f64; 2],
    /// Playback 1/2, 3/4, 5/6, each L/R.
    pub playback: [[f64; 2]; 3],
    /// Output 1+2 L/R, before the output level.
    pub output: [f64; 2],
    /// Loopback 1+2, 3+4, each L/R, before the loopback level.
    pub loopbacks: [[f64; 2]; 2],
}

impl Default for Meters {
    fn default() -> Self {
        Meters {
            inputs: [METER_FLOOR_DB; 2],
            playback: [[METER_FLOOR_DB; 2]; 3],
            output: [METER_FLOOR_DB; 2],
            loopbacks: [[METER_FLOOR_DB; 2]; 2],
        }
    }
}

impl Meters {
    /// Applies a meter frame; returns whether `f` was one.
    pub fn apply(&mut self, f: &Frame) -> bool {
        let db = value::meter_db(f.value);
        let slot = match (f.group, f.param) {
            (g @ (0x21 | 0x22), 0x05) => &mut self.inputs[usize::from(g - 0x21)],
            (0x31, p @ (0x03 | 0x04)) => &mut self.output[usize::from(p - 0x03)],
            (g @ (0x32 | 0x33), p @ (0x03 | 0x04)) => {
                &mut self.loopbacks[usize::from(g - 0x32)][usize::from(p - 0x03)]
            }
            (g @ (0x41..=0x43), p @ (0x01 | 0x02)) => {
                &mut self.playback[usize::from(g - 0x41)][usize::from(p - 0x01)]
            }
            _ => return false,
        };
        *slot = db;
        true
    }
}

/// dB values in JSON: a number, or `null` for `-inf` (JSON has no infinity).
mod db_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(db: &f64, s: S) -> Result<S::Ok, S::Error> {
        if db.is_finite() {
            s.serialize_f64(*db)
        } else {
            s.serialize_none()
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NEG_INFINITY))
    }
}
