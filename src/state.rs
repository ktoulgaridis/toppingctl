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
//! - **Stereo PB strips** ([`PbStrip`]): every channel has its own fader,
//!   pan, MUTE, SOLO and Ø, as on the app's unlinked `Mixer` page
//!   (`setbalance_L` / `setbalance_R`, app V1.09). The `_l` fields are the
//!   odd channel (PB1, PB3, PB5), the `_r` fields the even one; the pan
//!   default is the app's (odd hard L, even hard R). A saved state from
//!   before per-channel pan holds a single balance `b`; it loads as
//!   `pan_l = max(0, 2b − 1)`, `pan_r = min(1, 2b)`, which gives the same
//!   frames that balance gave. One from before per-channel faders and
//!   switches loads the single value into both channels.
//! - **Link** (IN 1+2 and every PB strip) swaps the app's page and couples
//!   the two channels in the UI; it has no frames of its own, and each
//!   pan stays its channel's own. Before #15, IN 1+2's link put IN1 hard L
//!   and IN2 hard R; a saved state from then (its link stored as `link`,
//!   not `in12_link`) loads with those pans, so it gives the same frames.
//! - **Solo** in a mix writes 0 for every channel of that mix that is not
//!   soloed.
//! - **Output and loopback levels** use [`value::level_from_db`] (0 dB =
//!   `0x02000000`, as the app writes). Away from 0 dB the app's own words
//!   differ from it by about one part in a million. Each channel has its own
//!   level (`x 01` L, `x 02` R), and each loopback channel its own MUTE;
//!   LINK couples them in the UI and has no frames of its own. A saved
//!   state from before holds one `level_db` (and one `mute`); it loads into
//!   both channels.
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
    /// The mixer strips' names (IN 1+2, PB 1/2, PB 3/4, PB 5/6) as the
    /// user set them, the same in every mix; `None` is the default name.
    /// Kept in the workspace, never sent to the device.
    pub strip_names: [Option<String>; 4],
    /// The loopback strips' names (LOOP 1+2, LOOP 3+4); `None` is the
    /// default. Kept in the workspace, never sent.
    pub loopback_names: [Option<String>; 2],
    /// The output strip's name (OUT 1+2); `None` is the default.
    pub output_name: Option<String>,
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

/// A mono channel strip of a mix (IN1, IN2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Strip {
    /// Fader, `-inf` ..= ≈ +6 dB. Stored as `null` in JSON when `-inf`.
    #[serde(with = "db_serde")]
    pub fader_db: f64,
    /// 0 = left, 0.5 = centre, 1 = right.
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
    pub phase: bool,
}

/// A stereo playback strip of a mix (PB 1/2, 3/4, 5/6): `_l` is the odd
/// channel (PB1, PB3, PB5), `_r` the even one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, from = "PbStripWire")]
pub struct PbStrip {
    /// Faders, `-inf` ..= ≈ +6 dB. Stored as `null` in JSON when `-inf`.
    #[serde(with = "db_serde")]
    pub fader_l_db: f64,
    #[serde(with = "db_serde")]
    pub fader_r_db: f64,
    /// Pan of the odd channel: 0 = left, 1 = right.
    pub pan_l: f64,
    /// Pan of the even channel: 0 = left, 1 = right.
    pub pan_r: f64,
    /// The app's linked page: one fader and one MUTE / SOLO / Ø for both
    /// channels. A UI coupling; no frames of its own.
    pub link: bool,
    pub mute_l: bool,
    pub mute_r: bool,
    pub solo_l: bool,
    pub solo_r: bool,
    pub phase_l: bool,
    pub phase_r: bool,
}

/// One of the three mixes. IN 1+2's link is stored as `in12_link`: a
/// `link` is from before #15 (see the module docs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, from = "MixWire", into = "MixWire")]
pub struct Mix {
    pub in1: Strip,
    pub in2: Strip,
    pub pb12: PbStrip,
    pub pb34: PbStrip,
    pub pb56: PbStrip,
    /// IN 1+2 on the app's linked page: one fader and one MUTE / SOLO / Ø
    /// for IN1 and IN2. A UI coupling; no frames of its own.
    pub link: bool,
}

/// One loopback pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, from = "LoopbackWire")]
pub struct Loopback {
    pub source: Source,
    /// Left level, `-inf` ..= 0 dB.
    #[serde(with = "db_serde")]
    pub level_l_db: f64,
    /// Right level, `-inf` ..= 0 dB.
    #[serde(with = "db_serde")]
    pub level_r_db: f64,
    /// The app's linked page: one fader and one MUTE for both channels.
    /// A UI coupling; no frames of its own.
    pub link: bool,
    pub mute_l: bool,
    pub mute_r: bool,
}

/// Output 1+2.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, from = "OutputWire")]
pub struct Output {
    /// Any of the nine sources, as the app's menu offers.
    pub source: Source,
    /// Left level, `-inf` ..= 0 dB.
    #[serde(with = "db_serde")]
    pub level_l_db: f64,
    /// Right level, `-inf` ..= 0 dB.
    #[serde(with = "db_serde")]
    pub level_r_db: f64,
    /// The app's linked page: one fader for both channels. A UI coupling;
    /// no frames of its own.
    pub link: bool,
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
    /// The Monitor Mix balance (`11 01`): 0 ..= 100, 50 = centre (input ←
    /// → playback). Reported when the device's knob turns; never written.
    pub monitor_mix: Option<i32>,
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
            strip_names: Default::default(),
            loopback_names: Default::default(),
            output_name: None,
            readout: Readout::default(),
        }
    }
}

impl Default for Mix {
    /// As in the official app: IN1 hard left (L100), IN2 hard right (R100).
    fn default() -> Self {
        Mix {
            in1: Strip {
                pan: 0.0,
                ..Strip::default()
            },
            in2: Strip {
                pan: 1.0,
                ..Strip::default()
            },
            pb12: PbStrip::default(),
            pb34: PbStrip::default(),
            pb56: PbStrip::default(),
            link: false,
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

/// The app's default: odd channel hard L, even hard R, linked.
impl Default for PbStrip {
    fn default() -> Self {
        PbStrip {
            fader_l_db: 0.0,
            fader_r_db: 0.0,
            pan_l: 0.0,
            pan_r: 1.0,
            link: true,
            mute_l: false,
            mute_r: false,
            solo_l: false,
            solo_r: false,
            phase_l: false,
            phase_r: false,
        }
    }
}

impl PbStrip {
    /// Sets both pans from a single balance `b` (0 = left, 1 = right): left
    /// of centre the even channel moves left, right of centre the odd
    /// channel moves right. Centre is the default.
    pub fn set_balance(&mut self, b: f64) {
        let b = b.clamp(0.0, 1.0);
        self.pan_l = (2.0 * b - 1.0).max(0.0);
        self.pan_r = (2.0 * b).min(1.0);
    }

    /// The balance that [`PbStrip::set_balance`] would take to reach these
    /// pans (their mean; exact for every pan pair a balance can make).
    pub fn balance(&self) -> f64 {
        (self.pan_l + self.pan_r) / 2.0
    }

    /// Sets both channels' faders to `db`.
    pub fn set_fader(&mut self, db: f64) {
        self.fader_l_db = db;
        self.fader_r_db = db;
    }
}

/// One channel of a mixer strip, by value: IN1 or IN2 of IN 1+2, or one
/// side of a PB strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Channel {
    pub fader_db: f64,
    pub mute: bool,
    pub solo: bool,
    pub phase: bool,
}

/// One channel of a mixer strip, to change it.
#[derive(Debug)]
pub struct ChannelMut<'a> {
    pub fader_db: &'a mut f64,
    pub mute: &'a mut bool,
    pub solo: &'a mut bool,
    pub phase: &'a mut bool,
}

impl Mix {
    /// Channel `ch` (0 = L / IN1 / odd, 1 = R / IN2 / even) of mixer strip
    /// `strip`: IN 1+2 (0), PB 1/2 (1), PB 3/4 (2), PB 5/6 (3).
    pub fn channel(&self, strip: usize, ch: usize) -> Channel {
        let (fader_db, mute, solo, phase) = match (strip, ch) {
            (0, 0) => (
                self.in1.fader_db,
                self.in1.mute,
                self.in1.solo,
                self.in1.phase,
            ),
            (0, _) => (
                self.in2.fader_db,
                self.in2.mute,
                self.in2.solo,
                self.in2.phase,
            ),
            (s, 0) => {
                let p = self.pb(s);
                (p.fader_l_db, p.mute_l, p.solo_l, p.phase_l)
            }
            (s, _) => {
                let p = self.pb(s);
                (p.fader_r_db, p.mute_r, p.solo_r, p.phase_r)
            }
        };
        Channel {
            fader_db,
            mute,
            solo,
            phase,
        }
    }

    /// [`Mix::channel`], to change it.
    pub fn channel_mut(&mut self, strip: usize, ch: usize) -> ChannelMut<'_> {
        match (strip, ch) {
            (0, c) => {
                let s = if c == 0 { &mut self.in1 } else { &mut self.in2 };
                ChannelMut {
                    fader_db: &mut s.fader_db,
                    mute: &mut s.mute,
                    solo: &mut s.solo,
                    phase: &mut s.phase,
                }
            }
            (s, 0) => {
                let p = self.pb_mut(s);
                ChannelMut {
                    fader_db: &mut p.fader_l_db,
                    mute: &mut p.mute_l,
                    solo: &mut p.solo_l,
                    phase: &mut p.phase_l,
                }
            }
            (s, _) => {
                let p = self.pb_mut(s);
                ChannelMut {
                    fader_db: &mut p.fader_r_db,
                    mute: &mut p.mute_r,
                    solo: &mut p.solo_r,
                    phase: &mut p.phase_r,
                }
            }
        }
    }

    /// Whether mixer strip `strip` is on its linked page.
    pub fn linked(&self, strip: usize) -> bool {
        match strip {
            0 => self.link,
            s => self.pb(s).link,
        }
    }

    /// Mixer strip `strip`'s LINK.
    pub fn link_mut(&mut self, strip: usize) -> &mut bool {
        match strip {
            0 => &mut self.link,
            s => &mut self.pb_mut(s).link,
        }
    }

    /// Mixer strip `strip`'s two pans: IN1 / IN2, or the odd / even
    /// channel's.
    pub fn pans(&self, strip: usize) -> [f64; 2] {
        match strip {
            0 => [self.in1.pan, self.in2.pan],
            s => [self.pb(s).pan_l, self.pb(s).pan_r],
        }
    }

    /// Mixer strip `strip`'s pan of channel `ch`, to change it.
    pub fn pan_mut(&mut self, strip: usize, ch: usize) -> &mut f64 {
        match (strip, ch) {
            (0, 0) => &mut self.in1.pan,
            (0, _) => &mut self.in2.pan,
            (s, 0) => &mut self.pb_mut(s).pan_l,
            (s, _) => &mut self.pb_mut(s).pan_r,
        }
    }

    /// PB strip `strip` (1 ..= 3).
    pub fn pb(&self, strip: usize) -> &PbStrip {
        match strip {
            1 => &self.pb12,
            2 => &self.pb34,
            _ => &self.pb56,
        }
    }

    /// PB strip `strip` (1 ..= 3), to change it.
    pub fn pb_mut(&mut self, strip: usize) -> &mut PbStrip {
        match strip {
            1 => &mut self.pb12,
            2 => &mut self.pb34,
            _ => &mut self.pb56,
        }
    }

    /// Whether anything in the mix is soloed.
    fn has_solo(&self) -> bool {
        (0..4).any(|s| (0..2).any(|c| self.channel(s, c).solo))
    }
}

impl Default for Loopback {
    fn default() -> Self {
        Loopback {
            source: Source::Pb12,
            level_l_db: 0.0,
            level_r_db: 0.0,
            link: true,
            mute_l: false,
            mute_r: false,
        }
    }
}

impl Loopback {
    /// Sets both channels to `db`.
    pub fn set_level(&mut self, db: f64) {
        self.level_l_db = db;
        self.level_r_db = db;
    }

    /// Mutes or unmutes both channels.
    pub fn set_mute(&mut self, mute: bool) {
        self.mute_l = mute;
        self.mute_r = mute;
    }
}

impl Default for Output {
    fn default() -> Self {
        Output {
            source: Source::MixA,
            level_l_db: 0.0,
            level_r_db: 0.0,
            link: true,
            headphones: true,
            line: true,
        }
    }
}

impl Output {
    /// Sets both channels to `db`.
    pub fn set_level(&mut self, db: f64) {
        self.level_l_db = db;
        self.level_r_db = db;
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

/// A strip's level before pan, with MUTE, SOLO and Ø folded in.
fn strip_level(fader_db: f64, mute: bool, solo: bool, phase: bool, mix_has_solo: bool) -> i32 {
    if mute || (mix_has_solo && !solo) {
        return 0;
    }
    value::phase(crosspoint_level(fader_db), phase)
}

impl Channel {
    fn level(&self, mix_has_solo: bool) -> i32 {
        strip_level(
            self.fader_db,
            self.mute,
            self.solo,
            self.phase,
            mix_has_solo,
        )
    }

    /// Whether this channel is heard in its mix (not muted, not soloed
    /// out, not at −∞).
    fn heard(&self, mix_has_solo: bool) -> bool {
        self.level(mix_has_solo) != 0
    }
}

impl Mix {
    /// The 16 crosspoint values as `(source 1..=8, left, right)`: source
    /// 1, 2 are IN1, IN2; 3 ..= 8 the PB channels.
    fn crosspoints(&self) -> [(u8, i32, i32); 8] {
        let soloed = self.has_solo();
        let mut out = [(0, 0, 0); 8];
        for strip in 0..4 {
            for (ch, pan) in self.pans(strip).into_iter().enumerate() {
                let i = 2 * strip + ch;
                let (l, r) = value::pan_split(self.channel(strip, ch).level(soloed), pan);
                out[i] = (i as u8 + 1, l, r);
            }
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
            out.push(frame(0x32 + i as u8, 0x01, level(lb.level_l_db, lb.mute_l)));
            out.push(frame(0x32 + i as u8, 0x02, level(lb.level_r_db, lb.mute_r)));
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
        let out = &self.output;
        [
            frame(0x31, 0x01, level(out.level_l_db.min(0.0), false)),
            frame(0x31, 0x02, level(out.level_r_db.min(0.0), false)),
        ]
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
            (0x11, 0x01) => self.readout.monitor_mix = Some(f.value),
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
    /// The post-fader readings, as the app computes them
    /// (`setBarMixervolume`): each reading plus its fader's gain, at the
    /// floor when the channel is muted, soloed out or at −∞. The mixer's
    /// are for mix `mix` (the one shown); the output's and the loopbacks'
    /// add their levels.
    pub fn post(&self, state: &State, mix: usize) -> Meters {
        let after = |pre: f64, gain: Option<f64>| match gain {
            Some(g) if g.is_finite() => (pre + g).max(METER_FLOOR_DB),
            _ => METER_FLOOR_DB,
        };
        let m = &state.mixes[mix.min(2)];
        let solo = m.has_solo();
        let mixer = |strip: usize, ch: usize, pre: f64| {
            let c = m.channel(strip, ch);
            after(pre, c.heard(solo).then_some(c.fader_db))
        };
        let out = &state.output;
        Meters {
            inputs: [0, 1].map(|c| mixer(0, c, self.inputs[c])),
            playback: [0, 1, 2].map(|p| [0, 1].map(|c| mixer(p + 1, c, self.playback[p][c]))),
            output: [
                after(self.output[0], Some(out.level_l_db.min(0.0))),
                after(self.output[1], Some(out.level_r_db.min(0.0))),
            ],
            loopbacks: [0, 1].map(|i| {
                let lb = &state.loopbacks[i];
                [
                    after(self.loopbacks[i][0], (!lb.mute_l).then_some(lb.level_l_db)),
                    after(self.loopbacks[i][1], (!lb.mute_r).then_some(lb.level_r_db)),
                ]
            }),
        }
    }

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

/// [`PbStrip`] as stored, including what saved states from before hold:
/// a single `pan` (a balance) from before per-channel pan, and a single
/// `fader_db`, `mute`, `solo`, `phase` from before per-channel faders and
/// switches (#15).
#[derive(Deserialize)]
#[serde(default)]
struct PbStripWire {
    #[serde(deserialize_with = "opt_db")]
    fader_db: Option<f64>,
    #[serde(deserialize_with = "opt_db")]
    fader_l_db: Option<f64>,
    #[serde(deserialize_with = "opt_db")]
    fader_r_db: Option<f64>,
    pan: Option<f64>,
    pan_l: Option<f64>,
    pan_r: Option<f64>,
    link: bool,
    mute: Option<bool>,
    mute_l: Option<bool>,
    mute_r: Option<bool>,
    solo: Option<bool>,
    solo_l: Option<bool>,
    solo_r: Option<bool>,
    phase: Option<bool>,
    phase_l: Option<bool>,
    phase_r: Option<bool>,
}

impl Default for PbStripWire {
    fn default() -> Self {
        PbStripWire {
            fader_db: None,
            fader_l_db: None,
            fader_r_db: None,
            pan: None,
            pan_l: None,
            pan_r: None,
            link: PbStrip::default().link,
            mute: None,
            mute_l: None,
            mute_r: None,
            solo: None,
            solo_l: None,
            solo_r: None,
            phase: None,
            phase_l: None,
            phase_r: None,
        }
    }
}

/// A channel's value: its own, else the old shared one, else `default`.
fn per_channel<T: Copy>(both: Option<T>, own: Option<T>, default: T) -> T {
    own.or(both).unwrap_or(default)
}

impl From<PbStripWire> for PbStrip {
    fn from(w: PbStripWire) -> Self {
        let d = PbStrip::default();
        let mut s = PbStrip {
            fader_l_db: per_channel(w.fader_db, w.fader_l_db, d.fader_l_db),
            fader_r_db: per_channel(w.fader_db, w.fader_r_db, d.fader_r_db),
            link: w.link,
            mute_l: per_channel(w.mute, w.mute_l, d.mute_l),
            mute_r: per_channel(w.mute, w.mute_r, d.mute_r),
            solo_l: per_channel(w.solo, w.solo_l, d.solo_l),
            solo_r: per_channel(w.solo, w.solo_r, d.solo_r),
            phase_l: per_channel(w.phase, w.phase_l, d.phase_l),
            phase_r: per_channel(w.phase, w.phase_r, d.phase_r),
            ..d
        };
        if let Some(b) = w.pan {
            s.set_balance(b);
        }
        s.pan_l = w.pan_l.unwrap_or(s.pan_l);
        s.pan_r = w.pan_r.unwrap_or(s.pan_r);
        s
    }
}

/// [`Mix`] as stored. IN 1+2's link is `in12_link`; a `link` is from
/// before #15, when the link put IN1 hard L and IN2 hard R, so a `link`
/// that is on loads as those pans (the frames stay the same).
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct MixWire {
    in1: Strip,
    in2: Strip,
    pb12: PbStrip,
    pb34: PbStrip,
    pb56: PbStrip,
    #[serde(skip_serializing)]
    link: Option<bool>,
    in12_link: Option<bool>,
}

// Not derivable: a missing IN strip takes the mix's default (IN1 hard L,
// IN2 hard R), not the strip's.
#[allow(clippy::derivable_impls)]
impl Default for MixWire {
    fn default() -> Self {
        MixWire {
            link: None,
            in12_link: None,
            ..Mix::default().into()
        }
    }
}

impl From<Mix> for MixWire {
    fn from(m: Mix) -> Self {
        MixWire {
            in1: m.in1,
            in2: m.in2,
            pb12: m.pb12,
            pb34: m.pb34,
            pb56: m.pb56,
            link: None,
            in12_link: Some(m.link),
        }
    }
}

impl From<MixWire> for Mix {
    fn from(w: MixWire) -> Self {
        let mut m = Mix {
            in1: w.in1,
            in2: w.in2,
            pb12: w.pb12,
            pb34: w.pb34,
            pb56: w.pb56,
            link: false,
        };
        match (w.in12_link, w.link) {
            (Some(link), _) => m.link = link,
            (None, Some(true)) => {
                m.link = true;
                m.in1.pan = 0.0;
                m.in2.pan = 1.0;
            }
            (None, _) => m.link = Mix::default().link,
        }
        m
    }
}

/// [`Loopback`] as stored, including the single `level_db` and `mute` of
/// older saved states.
#[derive(Deserialize)]
#[serde(default)]
struct LoopbackWire {
    source: Source,
    #[serde(deserialize_with = "opt_db")]
    level_db: Option<f64>,
    #[serde(deserialize_with = "opt_db")]
    level_l_db: Option<f64>,
    #[serde(deserialize_with = "opt_db")]
    level_r_db: Option<f64>,
    link: bool,
    mute: Option<bool>,
    mute_l: Option<bool>,
    mute_r: Option<bool>,
}

impl Default for LoopbackWire {
    fn default() -> Self {
        let d = Loopback::default();
        LoopbackWire {
            source: d.source,
            level_db: None,
            level_l_db: None,
            level_r_db: None,
            link: d.link,
            mute: None,
            mute_l: None,
            mute_r: None,
        }
    }
}

impl From<LoopbackWire> for Loopback {
    fn from(w: LoopbackWire) -> Self {
        let (l, r) = levels(w.level_db, w.level_l_db, w.level_r_db);
        Loopback {
            source: w.source,
            level_l_db: l,
            level_r_db: r,
            link: w.link,
            mute_l: per_channel(w.mute, w.mute_l, false),
            mute_r: per_channel(w.mute, w.mute_r, false),
        }
    }
}

/// [`Output`] as stored, including the single `level_db` of older saved
/// states.
#[derive(Deserialize)]
#[serde(default)]
struct OutputWire {
    source: Source,
    #[serde(deserialize_with = "opt_db")]
    level_db: Option<f64>,
    #[serde(deserialize_with = "opt_db")]
    level_l_db: Option<f64>,
    #[serde(deserialize_with = "opt_db")]
    level_r_db: Option<f64>,
    link: bool,
    headphones: bool,
    line: bool,
}

impl Default for OutputWire {
    fn default() -> Self {
        let d = Output::default();
        OutputWire {
            source: d.source,
            level_db: None,
            level_l_db: None,
            level_r_db: None,
            link: d.link,
            headphones: d.headphones,
            line: d.line,
        }
    }
}

impl From<OutputWire> for Output {
    fn from(w: OutputWire) -> Self {
        let (l, r) = levels(w.level_db, w.level_l_db, w.level_r_db);
        Output {
            source: w.source,
            level_l_db: l,
            level_r_db: r,
            link: w.link,
            headphones: w.headphones,
            line: w.line,
        }
    }
}

/// L and R levels from what was stored: each channel's own value, else the
/// old shared one, else 0 dB.
fn levels(both: Option<f64>, l: Option<f64>, r: Option<f64>) -> (f64, f64) {
    let both = both.unwrap_or(0.0);
    (l.unwrap_or(both), r.unwrap_or(both))
}

/// An optional dB value in JSON: absent is `None` (the field's default),
/// `null` is `-inf`.
fn opt_db<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    db_serde::deserialize(d).map(Some)
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
