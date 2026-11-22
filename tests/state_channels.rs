//! Per-channel state (#15, the 1:1 audit of the official app): every
//! mixer strip and both loopbacks have L/R values that LINK only couples
//! in the UI; IN 1+2's LINK no longer overrides the pans; saved states from
//! before load with the new fields taken from the old single ones; the
//! post-fader meters.

use std::fs;

use toppingctl::frame::Frame;
use toppingctl::state::{Loopback, Meters, PbStrip, State, METER_FLOOR_DB};
use toppingctl::value::level_from_db;

fn frame(group: u8, param: u8, value: i32) -> Frame {
    Frame {
        group,
        param,
        value,
    }
}

/// The app's crosspoint level for a fader at `db` (PROTOCOL.md).
fn xp(db: f64) -> i32 {
    ((335_544.0 * 10f64.powf(db / 20.0)).round() as i32) * 100
}

/// A PB strip's channels are separate: the even channel's fader, mute,
/// solo and Ø move only its own crosspoints (`x4` for PB2).
#[test]
fn pb_channels_are_separate() {
    let base = State::default();
    let zero = xp(0.0);

    // PB2 fader −6 dB: PB2 is hard R, so only 51 24 moves.
    let mut s = base.clone();
    s.mixes[0].pb12.fader_r_db = -6.0;
    assert_eq!(s.mixes[0].pb12.fader_l_db, 0.0);
    assert_eq!(base.diff(&s), [frame(0x51, 0x24, xp(-6.0))]);

    // PB3 mute (Mix B): only 52 15 (PB3 is hard L).
    let mut s = base.clone();
    s.mixes[1].pb34.mute_l = true;
    assert_eq!(base.diff(&s), [frame(0x52, 0x15, 0)]);

    // PB6 Ø (Mix C): 53 28 negated.
    let mut s = base.clone();
    s.mixes[2].pb56.phase_r = true;
    assert_eq!(base.diff(&s), [frame(0x53, 0x28, -zero)]);

    // PB2 solo: everything else in the mix goes, PB1 included.
    let mut s = base.clone();
    s.mixes[0].pb12.solo_r = true;
    assert_eq!(
        base.diff(&s),
        [
            frame(0x51, 0x11, 0),
            frame(0x51, 0x22, 0),
            frame(0x51, 0x13, 0),
            frame(0x51, 0x15, 0),
            frame(0x51, 0x26, 0),
            frame(0x51, 0x17, 0),
            frame(0x51, 0x28, 0),
        ]
    );
}

/// IN 1+2's LINK, like every LINK, has no frames of its own: the two pans
/// stay each channel's own (the app moves them independently, `50−v`).
#[test]
fn in12_link_keeps_the_pans() {
    let mut centred = State::default();
    centred.mixes[0].in1.pan = 0.5;
    centred.mixes[0].in2.pan = 0.5;
    let mut s = centred.clone();
    s.mixes[0].link = true;
    assert!(centred.diff(&s).is_empty());
    let half = xp(0.0) - (f64::from(xp(0.0)) * 0.5).round() as i32;
    let get = |g: u8, p: u8| {
        s.frames()
            .into_iter()
            .find(|f| (f.group, f.param) == (g, p))
            .unwrap()
            .value
    };
    assert_eq!(get(0x51, 0x11), half);
    assert_eq!(get(0x51, 0x21), half);
}

/// A loopback has a mute per channel; LINK couples them in the UI only.
#[test]
fn loopback_mute_per_channel() {
    let base = State::default();
    let mut s = base.clone();
    s.loopbacks[1].mute_r = true;
    assert_eq!(base.diff(&s), [frame(0x33, 0x02, 0)]);
    let mut muted = base.clone();
    muted.loopbacks[0].mute_l = true;
    assert_eq!(base.diff(&muted), [frame(0x32, 0x01, 0)]);
    let mut s = muted.clone();
    s.loopbacks[0].level_l_db = -6.0;
    assert!(muted.diff(&s).is_empty(), "muted: still 0");
    s.loopbacks[0].mute_l = false;
    assert_eq!(muted.diff(&s), [frame(0x32, 0x01, level_from_db(-6.0))]);
}

/// Saved states from before load: each new per-channel field takes the
/// old single one, and an old IN 1+2 link (which put IN1 hard L and IN2
/// hard R) loads as those pans, so the frames don't change.
#[test]
fn old_saved_states_load() {
    let old = r#"{"fader_db": -12.0, "mute": true, "solo": true, "phase": true, "pan": 0.5}"#;
    let p: PbStrip = serde_json::from_str(old).unwrap();
    assert_eq!((p.fader_l_db, p.fader_r_db), (-12.0, -12.0));
    assert!(p.mute_l && p.mute_r && p.solo_l && p.solo_r && p.phase_l && p.phase_r);
    let p: PbStrip = serde_json::from_str(r#"{"fader_db": null}"#).unwrap();
    assert_eq!(p.fader_r_db, f64::NEG_INFINITY);
    // New fields win.
    let p: PbStrip =
        serde_json::from_str(r#"{"fader_db": -3.0, "fader_r_db": -9.0, "mute_r": true}"#).unwrap();
    assert_eq!((p.fader_l_db, p.fader_r_db), (-3.0, -9.0));
    assert!(!p.mute_l && p.mute_r);

    let lb: Loopback = serde_json::from_str(r#"{"mute": true}"#).unwrap();
    assert!(lb.mute_l && lb.mute_r);

    // The #11 fixture: Mix C was linked with centred IN pans.
    let json = fs::read_to_string("tests/fixtures/state-issue11.json").unwrap();
    let s: State = serde_json::from_str(&json).unwrap();
    assert!(s.mixes[2].link);
    assert_eq!((s.mixes[2].in1.pan, s.mixes[2].in2.pan), (0.0, 1.0));
    assert_eq!((s.mixes[0].in1.pan, s.mixes[0].in2.pan), (0.3, 0.5));

    // The new format round-trips: a linked mix keeps its own pans.
    let mut s = State::default();
    s.mixes[1].link = true;
    s.mixes[1].in1.pan = 0.4;
    s.mixes[0].pb34.fader_r_db = -7.0;
    s.loopbacks[0].mute_r = true;
    let out = serde_json::to_string(&s).unwrap();
    let back: State = serde_json::from_str(&out).unwrap();
    assert_eq!(back, s);
}

/// The post-fader meters, as the app computes them (`setBarMixervolume`):
/// the reading plus the fader's gain, at the floor when muted, soloed out
/// or at −∞. The mixer's are for the mix shown.
#[test]
fn post_fader_meters() {
    let pre = Meters {
        inputs: [-10.0, -20.0],
        playback: [[-8.0, -9.0], [-30.0, -30.0], [-40.0, -41.0]],
        output: [-9.0, -10.0],
        loopbacks: [[-22.0, -23.0], [-96.0, -96.0]],
    };
    let mut s = State::default();
    s.mixes[1].in1.fader_db = -6.0;
    s.mixes[1].in2.mute = true;
    s.mixes[1].pb12.fader_r_db = 3.0;
    s.mixes[1].pb34.fader_l_db = f64::NEG_INFINITY;
    s.output.level_r_db = -12.0;
    s.loopbacks[0].mute_r = true;
    s.loopbacks[0].level_l_db = -2.0;
    let post = pre.post(&s, 1);
    assert_eq!(post.inputs, [-16.0, METER_FLOOR_DB]);
    assert_eq!(post.playback[0], [-8.0, -6.0]);
    assert_eq!(post.playback[1], [METER_FLOOR_DB, -30.0]);
    assert_eq!(post.output, [-9.0, -22.0]);
    assert_eq!(post.loopbacks[0], [-24.0, METER_FLOOR_DB]);
    assert_eq!(post.loopbacks[1], [METER_FLOOR_DB; 2]);
    // Mix A is untouched by Mix B's faders.
    assert_eq!(pre.post(&s, 0).inputs, [-10.0, -20.0]);
    // A solo elsewhere in the mix: the others fall to the floor.
    s.mixes[1].pb56.solo_l = true;
    let post = pre.post(&s, 1);
    assert_eq!(post.inputs, [METER_FLOOR_DB; 2]);
    assert_eq!(post.playback[2], [-40.0, METER_FLOOR_DB]);
}
