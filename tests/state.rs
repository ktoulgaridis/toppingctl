use std::collections::BTreeMap;
use std::fs;

use toppingctl::frame::{Dir, Frame};
use toppingctl::state::{Meters, PbStrip, Source, State, Version};

fn frame(group: u8, param: u8, value: i32) -> Frame {
    Frame {
        group,
        param,
        value,
    }
}

/// Every writable register in PROTOCOL.md except `1a 03`, `10 06` and 48V.
fn writable_registers() -> Vec<(u8, u8)> {
    let mut regs = vec![(0x10, 0x0a), (0x10, 0x0b), (0x10, 0x0c), (0x11, 0x02)];
    for g in [0x21, 0x22] {
        regs.extend([(g, 0x01), (g, 0x03), (g, 0x04)]);
    }
    regs.extend([(0x31, 0x01), (0x31, 0x02), (0x31, 0x05)]);
    for g in [0x32, 0x33] {
        regs.extend([(g, 0x01), (g, 0x02), (g, 0x05)]);
    }
    regs.extend([(0x34, 0x01), (0x34, 0x02)]);
    for g in [0x51, 0x52, 0x53] {
        for o in 1..=2u8 {
            for s in 1..=8u8 {
                regs.push((g, o << 4 | s));
            }
        }
    }
    regs.sort();
    regs
}

#[test]
fn full_push_covers_every_register() {
    let frames = State::default().frames();
    let mut counts: BTreeMap<(u8, u8), usize> = BTreeMap::new();
    for f in &frames {
        *counts.entry((f.group, f.param)).or_default() += 1;
    }
    for (reg, n) in &counts {
        assert_eq!(
            *n, 1,
            "register {:02x} {:02x} written {n} times",
            reg.0, reg.1
        );
    }
    let written: Vec<(u8, u8)> = counts.keys().copied().collect();
    assert_eq!(written, writable_registers());
    assert!(frames
        .iter()
        .all(|f| f.param != 0x02 || !matches!(f.group, 0x21 | 0x22)));
}

/// The app's launch push in `captures/mac-1/tx.txt`, from the first
/// crosspoint (`51 11`, line 10) through `34 02` (line 69).
fn app_launch_frames() -> Vec<Frame> {
    let text = fs::read_to_string("captures/mac-1/tx.txt").unwrap();
    text.lines()
        .skip(9)
        .take(60)
        .map(|line| {
            let bytes: Vec<u8> = line
                .split_whitespace()
                .skip(2)
                .map(|b| u8::from_str_radix(b, 16).unwrap())
                .collect();
            Frame::decode(&bytes, Dir::ToDevice).unwrap()
        })
        .collect()
}

#[test]
fn matches_app_launch_frames() {
    let expected = app_launch_frames();
    assert_eq!((expected[0].group, expected[0].param), (0x51, 0x11));
    assert_eq!((expected[59].group, expected[59].param), (0x34, 0x02));

    let mut state = State::default();
    for mix in &mut state.mixes {
        for strip in [&mut mix.in1, &mut mix.in2] {
            strip.fader_db = 0.0;
            strip.pan = 0.5;
            strip.mute = false;
            strip.solo = false;
            strip.phase = false;
        }
        // The app's stereo default: the odd channel hard L, the even hard R.
        for strip in [&mut mix.pb12, &mut mix.pb34, &mut mix.pb56] {
            strip.fader_db = 0.0;
            strip.pan_l = 0.0;
            strip.pan_r = 1.0;
            strip.mute = false;
            strip.solo = false;
            strip.phase = false;
        }
        mix.link = false;
    }
    state.output.source = Source::MixA;
    state.output.level_l_db = 0.0;
    state.output.level_r_db = 0.0;
    state.output.headphones = true;
    state.output.line = true;
    state.loopbacks[0].source = Source::Pb34;
    state.loopbacks[1].source = Source::Pb56;
    for lb in &mut state.loopbacks {
        lb.level_l_db = 0.0;
        lb.level_r_db = 0.0;
        lb.mute = false;
    }
    state.phone_gain = true;

    let frames = state.frames();
    let start = frames
        .iter()
        .position(|f| (f.group, f.param) == (0x51, 0x11))
        .expect("a 51 11 frame");
    assert_eq!(&frames[start..start + 60], &expected[..]);
}

fn diff_regs(old: &State, new: &State) -> Vec<Frame> {
    old.diff(new)
}

#[test]
fn diff_is_minimal() {
    let base = State::default();
    assert!(base.diff(&base).is_empty());

    // IN1 mute: one 21 04 = 0.
    let mut s = base.clone();
    s.inputs[0].mute = true;
    assert_eq!(diff_regs(&base, &s), vec![frame(0x21, 0x04, 0)]);

    // Mix A IN1 pan: 51 11 + 51 21, split linearly.
    let mut s = base.clone();
    s.mixes[0].in1.pan = 0.49;
    assert_eq!(
        diff_regs(&base, &s),
        vec![
            frame(0x51, 0x11, 0x0105_1ea8),
            frame(0x51, 0x21, 0x00fa_e138)
        ]
    );

    // IN1 solo: the other input goes to 0.
    let mut s = base.clone();
    s.inputs[0].solo = true;
    assert_eq!(diff_regs(&base, &s), vec![frame(0x22, 0x04, 0)]);

    // IN1 phase: negated level.
    let mut s = base.clone();
    s.inputs[0].phase = true;
    assert_eq!(diff_regs(&base, &s), vec![frame(0x21, 0x04, -0x0200_0000)]);

    // Link: the 4 IN crosspoints, IN1 hard L, IN2 hard R (from centred
    // pans; the default pans are already hard L / R).
    let mut centred = base.clone();
    centred.mixes[0].in1.pan = 0.5;
    centred.mixes[0].in2.pan = 0.5;
    let mut s = centred.clone();
    s.mixes[0].link = true;
    assert_eq!(
        diff_regs(&centred, &s),
        vec![
            frame(0x51, 0x11, 0x01ff_ffe0),
            frame(0x51, 0x21, 0),
            frame(0x51, 0x12, 0),
            frame(0x51, 0x22, 0x01ff_ffe0),
        ]
    );

    // Input gain uses the app's values: +2 dB.
    let mut s = base.clone();
    s.inputs[0].gain_db = 2;
    assert_eq!(diff_regs(&base, &s), vec![frame(0x21, 0x04, 0x0284_91d0)]);

    // Mix strip fader at -1 dB, centred: the app's 0x00e428fc per side.
    let mut centred = base.clone();
    centred.mixes[1].in2.pan = 0.5;
    let mut s = centred.clone();
    s.mixes[1].in2.fader_db = -1.0;
    assert_eq!(
        diff_regs(&centred, &s),
        vec![
            frame(0x52, 0x12, 0x00e4_28fc),
            frame(0x52, 0x22, 0x00e4_28fc)
        ]
    );

    // Stereo PB strip at -1 dB: odd → L, even → R, full level each.
    let mut s = base.clone();
    s.mixes[0].pb12.fader_db = -1.0;
    assert_eq!(
        diff_regs(&base, &s),
        vec![
            frame(0x51, 0x13, 0x01c8_51f8),
            frame(0x51, 0x24, 0x01c8_51f8)
        ]
    );

    // Stereo PB: the even channel's own pan (the app's PB2 pan) at 0.98.
    let mut s = base.clone();
    s.mixes[0].pb12.pan_r = 0.98;
    assert_eq!(
        diff_regs(&base, &s),
        vec![
            frame(0x51, 0x14, 0x000a_3d70),
            frame(0x51, 0x24, 0x01f5_c270)
        ]
    );

    // Mix strip phase: both sides negated.
    let mut s = base.clone();
    s.mixes[0].pb12.phase = true;
    assert_eq!(
        diff_regs(&base, &s),
        vec![
            frame(0x51, 0x13, -0x01ff_ffe0),
            frame(0x51, 0x24, -0x01ff_ffe0)
        ]
    );

    // Mix strip solo: every other strip in that mix goes to 0 (IN2 → L is
    // already 0: IN2 is hard right by default).
    let mut s = base.clone();
    s.mixes[2].in1.solo = true;
    let regs: Vec<(u8, u8, i32)> = diff_regs(&base, &s)
        .iter()
        .map(|f| (f.group, f.param, f.value))
        .collect();
    assert_eq!(
        regs,
        vec![
            (0x53, 0x22, 0),
            (0x53, 0x13, 0),
            (0x53, 0x24, 0),
            (0x53, 0x15, 0),
            (0x53, 0x26, 0),
            (0x53, 0x17, 0),
            (0x53, 0x28, 0),
        ]
    );

    // Headphones: the app resends both switches and the output level.
    let mut s = base.clone();
    s.output.headphones = !s.output.headphones;
    let regs: Vec<(u8, u8)> = diff_regs(&base, &s)
        .iter()
        .map(|f| (f.group, f.param))
        .collect();
    assert_eq!(
        regs,
        vec![(0x34, 0x01), (0x34, 0x02), (0x31, 0x01), (0x31, 0x02)]
    );

    // Loopback mute: L and R to 0.
    let mut s = base.clone();
    s.loopbacks[1].mute = true;
    assert_eq!(
        diff_regs(&base, &s),
        vec![frame(0x33, 0x01, 0), frame(0x33, 0x02, 0)]
    );

    // A muted fader (-inf) writes 0 (IN1 → R is already 0: hard left).
    let mut s = base.clone();
    s.mixes[0].in1.fader_db = f64::NEG_INFINITY;
    assert_eq!(diff_regs(&base, &s), vec![frame(0x51, 0x11, 0)]);

    // 48V never appears in a diff.
    let mut s = base.clone();
    s.inputs[1].phantom = Some(true);
    assert!(base.diff(&s).is_empty());
}

#[test]
fn reports_update_state() {
    let mut state = State::default();
    let before = state.clone();

    // Meters, idle frames, levels and unknown registers are ignored.
    for f in [
        frame(0x21, 0x05, -299),
        frame(0x41, 0x01, -120),
        frame(0x00, 0x00, 0),
        frame(0x21, 0x04, 0),
        frame(0x51, 0x11, 0),
        frame(0x99, 0x01, 1),
    ] {
        state.apply(&f);
    }
    assert_eq!(state, before);

    for f in [
        frame(0x21, 0x01, 1),
        frame(0x22, 0x03, 1),
        frame(0x22, 0x02, 1),
        frame(0x21, 0x02, 0),
        frame(0x11, 0x02, 0),
        frame(0x10, 0x0a, 2),
        frame(0x10, 0x0b, 1),
        frame(0x10, 0x0c, 1),
        frame(0x11, 0x01, 0x49),
        frame(0x10, 0x03, 0x0001_000e),
        frame(0x10, 0x04, 0x0001_000d),
    ] {
        state.apply(&f);
    }
    assert!(state.inputs[0].mon);
    assert!(state.inputs[1].inst);
    assert_eq!(state.inputs[1].phantom, Some(true));
    assert_eq!(state.inputs[0].phantom, Some(false));
    assert!(!state.phone_gain);
    assert_eq!(state.system.brightness, 2);
    assert!(state.system.auto_standby);
    assert!(state.system.mobile_apps);
    assert_eq!(state.readout.monitor_mix, Some(0x49));
    assert_eq!(
        state.readout.hardware_version,
        Some(Version {
            major: 1,
            minor: 14
        })
    );
    assert_eq!(
        state.readout.firmware_version,
        Some(Version {
            major: 1,
            minor: 13
        })
    );
    assert_eq!(state.readout.firmware_version.unwrap().to_string(), "V1.13");

    let mut meters = Meters::default();
    assert!(meters.apply(&frame(0x21, 0x05, -299)));
    assert!(meters.apply(&frame(0x22, 0x05, -960)));
    assert!(meters.apply(&frame(0x31, 0x04, -60)));
    assert!(meters.apply(&frame(0x33, 0x03, -15)));
    assert!(meters.apply(&frame(0x42, 0x02, -301)));
    assert!(!meters.apply(&frame(0x21, 0x01, 1)));
    assert!(!meters.apply(&frame(0x00, 0x00, 0)));
    assert_eq!(meters.inputs, [-29.9, -96.0]);
    assert_eq!(meters.output[1], -6.0);
    assert_eq!(meters.loopbacks[1][0], -1.5);
    assert_eq!(meters.playback[1][1], -30.1);
}

/// Each PB channel has its own pan (the app's `setbalance_L` /
/// `setbalance_R`): PB1 → `x3` crosspoints, PB2 → `x4`, split linearly.
#[test]
fn pb_strips_pan_each_channel() {
    let base = State::default();
    for strip in [
        &base.mixes[0].pb12,
        &base.mixes[1].pb34,
        &base.mixes[2].pb56,
    ] {
        assert_eq!((strip.pan_l, strip.pan_r), (0.0, 1.0), "app default");
        assert!(strip.link, "PB strips start linked, as in the app");
    }
    assert!(!base.mixes[0].link, "IN 1+2 starts unlinked");

    // PB1 (odd) to centre: 51 13 and 51 23 split the level in half.
    let mut s = base.clone();
    s.mixes[0].pb12.pan_l = 0.5;
    assert_eq!(s.mixes[0].pb12.pan_r, 1.0, "the other channel is untouched");
    assert_eq!(
        base.diff(&s),
        vec![
            frame(0x51, 0x13, 0x00ff_fff0),
            frame(0x51, 0x23, 0x00ff_fff0)
        ]
    );

    // Both channels at once, in Mix C on PB 5/6: PB5 at 0.25, PB6 at 0.75.
    let mut s = base.clone();
    s.mixes[2].pb56.pan_l = 0.25;
    s.mixes[2].pb56.pan_r = 0.75;
    let lvl = 0x01ff_ffe0_i64;
    let quarter = ((lvl as f64) * 0.25).round() as i32;
    let three = ((lvl as f64) * 0.75).round() as i32;
    assert_eq!(
        base.diff(&s),
        vec![
            frame(0x53, 0x17, 0x01ff_ffe0 - quarter),
            frame(0x53, 0x27, quarter),
            frame(0x53, 0x18, 0x01ff_ffe0 - three),
            frame(0x53, 0x28, three),
        ]
    );

    // LINK on a PB strip is a UI coupling only: no frames of its own.
    let mut s = base.clone();
    s.mixes[1].pb34.link = false;
    assert!(base.diff(&s).is_empty());

    // The balance helpers the CLI uses: left of centre moves the even
    // channel, right of centre the odd one; `balance` is the inverse.
    let mut p = PbStrip::default();
    p.set_balance(0.25);
    assert_eq!((p.pan_l, p.pan_r), (0.0, 0.5));
    assert_eq!(p.balance(), 0.25);
    p.set_balance(0.75);
    assert_eq!((p.pan_l, p.pan_r), (0.5, 1.0));
    assert_eq!(p.balance(), 0.75);
    p.set_balance(0.5);
    assert_eq!((p.pan_l, p.pan_r), (0.0, 1.0));
}

/// Loopback and output have a level per channel (`x 01` L, `x 02` R) and a
/// LINK that couples them in the UI.
#[test]
fn loopback_and_output_levels_per_channel() {
    let base = State::default();
    assert!(base.loopbacks.iter().all(|lb| lb.link));
    assert!(base.output.link);

    let mut s = base.clone();
    s.loopbacks[0].level_l_db = -6.0;
    assert_eq!(
        base.diff(&s),
        vec![frame(0x32, 0x01, toppingctl::value::level_from_db(-6.0))]
    );

    let mut s = base.clone();
    s.loopbacks[1].level_r_db = f64::NEG_INFINITY;
    assert_eq!(base.diff(&s), vec![frame(0x33, 0x02, 0)]);

    let mut s = base.clone();
    s.output.level_r_db = -6.0;
    assert_eq!(
        base.diff(&s),
        vec![frame(0x31, 0x02, toppingctl::value::level_from_db(-6.0))]
    );

    // Mute still silences both channels.
    let mut s = base.clone();
    s.loopbacks[0].level_l_db = -3.0;
    s.loopbacks[0].mute = true;
    assert_eq!(
        base.diff(&s),
        vec![frame(0x32, 0x01, 0), frame(0x32, 0x02, 0)]
    );

    // LINK has no frames of its own; `set_level` moves both channels.
    let mut s = base.clone();
    s.output.link = false;
    s.loopbacks[1].link = false;
    assert!(base.diff(&s).is_empty());
    let mut s = base.clone();
    s.output.set_level(-3.0);
    s.loopbacks[1].set_level(-3.0);
    assert_eq!((s.output.level_l_db, s.output.level_r_db), (-3.0, -3.0));
    assert_eq!(
        (s.loopbacks[1].level_l_db, s.loopbacks[1].level_r_db),
        (-3.0, -3.0)
    );
}

fn issue11_frames() -> Vec<Frame> {
    fs::read_to_string("tests/fixtures/state-issue11.frames")
        .unwrap()
        .lines()
        .map(|l| {
            let mut it = l.split_whitespace();
            let mut next = || it.next().unwrap();
            let group = u8::from_str_radix(next(), 16).unwrap();
            let param = u8::from_str_radix(next(), 16).unwrap();
            let value = u32::from_str_radix(next(), 16).unwrap() as i32;
            frame(group, param, value)
        })
        .collect()
}

/// A saved state written by the #11 code (one balance per PB strip, one
/// level for loopback and output) still loads, and pushes exactly the
/// frames the #11 code pushed for it (`state-issue11.frames`).
#[test]
fn loads_issue11_saved_state() {
    let json = fs::read_to_string("tests/fixtures/state-issue11.json").unwrap();
    assert!(json.contains("\"pan\": 0.75") && json.contains("\"level_db\": -6.0"));
    let s: State = serde_json::from_str(&json).unwrap();

    // Balance 0.75 → PB1 at 0.5, PB2 hard R; 0.25 → PB3 hard L, PB4 at 0.5.
    assert_eq!((s.mixes[0].pb12.pan_l, s.mixes[0].pb12.pan_r), (0.5, 1.0));
    assert_eq!((s.mixes[1].pb34.pan_l, s.mixes[1].pb34.pan_r), (0.0, 0.5));
    assert_eq!((s.mixes[2].pb56.pan_l, s.mixes[2].pb56.pan_r), (0.0, 1.0));
    assert_eq!(s.mixes[1].pb34.fader_db, -12.0);
    assert_eq!(s.mixes[2].pb56.fader_db, f64::NEG_INFINITY);
    assert!(s.mixes[0].pb12.link, "missing link → the default");
    assert_eq!(s.mixes[0].in1.pan, 0.3);
    assert!(s.mixes[2].link);
    // One level → both channels; null → −∞.
    assert_eq!(
        (s.loopbacks[0].level_l_db, s.loopbacks[0].level_r_db),
        (-6.0, -6.0)
    );
    assert_eq!(s.loopbacks[1].level_l_db, f64::NEG_INFINITY);
    assert_eq!(s.loopbacks[1].level_r_db, f64::NEG_INFINITY);
    assert_eq!(s.loopbacks[1].source, Source::MixB);
    assert_eq!((s.output.level_l_db, s.output.level_r_db), (-3.0, -3.0));
    assert!(!s.output.line);
    assert!(s.loopbacks[0].link && s.output.link);

    assert_eq!(s.frames(), issue11_frames());

    // The new format round-trips and no longer writes the old fields.
    let out = serde_json::to_string_pretty(&s).unwrap();
    assert!(!out.contains("\"pan\": 0.75") && !out.contains("\"level_db\""));
    assert!(out.contains("\"pan_l\"") && out.contains("\"level_r_db\""));
    let back: State = serde_json::from_str(&out).unwrap();
    assert_eq!(back, s);

    // New fields win over a stale old one.
    let mixed = r#"{"pan": 0.25, "pan_l": 0.1, "pan_r": 0.9, "link": false}"#;
    let p: PbStrip = serde_json::from_str(mixed).unwrap();
    assert_eq!((p.pan_l, p.pan_r, p.link), (0.1, 0.9, false));
}

/// The mixer's IN 1+2 default, as in the official app on the Mac: IN1
/// hard left (L100), IN2 hard right (R100), in every mix. A saved state
/// keeps its own pans.
#[test]
fn in12_pans_default_hard_left_and_right() {
    let s = State::default();
    for mix in &s.mixes {
        assert_eq!(mix.in1.pan, 0.0, "IN1 L100");
        assert_eq!(mix.in2.pan, 1.0, "IN2 R100");
    }
    // Mix A IN1 at unity, hard left: all of it to L (51 11), none to R.
    let frames = s.frames();
    let get = |g: u8, p: u8| {
        frames
            .iter()
            .find(|f| (f.group, f.param) == (g, p))
            .unwrap()
            .value
    };
    assert!(get(0x51, 0x11) > 0);
    assert_eq!(get(0x51, 0x21), 0);
    assert_eq!(get(0x51, 0x12), 0);
    assert!(get(0x51, 0x22) > 0);

    let mut saved = State::default();
    saved.mixes[1].in1.pan = 0.3;
    saved.mixes[1].in2.pan = 0.5;
    let json = serde_json::to_string(&saved).unwrap();
    let back: State = serde_json::from_str(&json).unwrap();
    assert_eq!((back.mixes[1].in1.pan, back.mixes[1].in2.pan), (0.3, 0.5));
}
