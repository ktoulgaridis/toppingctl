use std::collections::BTreeMap;
use std::fs;

use toppingctl::frame::{Dir, Frame};
use toppingctl::state::{Meters, Source, State, Version};

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
        assert_eq!(*n, 1, "register {:02x} {:02x} written {n} times", reg.0, reg.1);
    }
    let written: Vec<(u8, u8)> = counts.keys().copied().collect();
    assert_eq!(written, writable_registers());
    assert!(frames.iter().all(|f| f.param != 0x02 || !matches!(f.group, 0x21 | 0x22)));
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
        for strip in [
            &mut mix.in1,
            &mut mix.in2,
            &mut mix.pb12,
            &mut mix.pb34,
            &mut mix.pb56,
        ] {
            strip.fader_db = 0.0;
            strip.pan = 0.5;
            strip.mute = false;
            strip.solo = false;
            strip.phase = false;
        }
        mix.link = false;
    }
    state.output.source = Source::MixA;
    state.output.level_db = 0.0;
    state.output.headphones = true;
    state.output.line = true;
    state.loopbacks[0].source = Source::Pb34;
    state.loopbacks[1].source = Source::Pb56;
    for lb in &mut state.loopbacks {
        lb.level_db = 0.0;
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
        vec![frame(0x51, 0x11, 0x0105_1ea8), frame(0x51, 0x21, 0x00fa_e138)]
    );

    // IN1 solo: the other input goes to 0.
    let mut s = base.clone();
    s.inputs[0].solo = true;
    assert_eq!(diff_regs(&base, &s), vec![frame(0x22, 0x04, 0)]);

    // IN1 phase: negated level.
    let mut s = base.clone();
    s.inputs[0].phase = true;
    assert_eq!(diff_regs(&base, &s), vec![frame(0x21, 0x04, -0x0200_0000)]);

    // Link: the 4 IN crosspoints, IN1 hard L, IN2 hard R.
    let mut s = base.clone();
    s.mixes[0].link = true;
    assert_eq!(
        diff_regs(&base, &s),
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
    let mut s = base.clone();
    s.mixes[1].in2.fader_db = -1.0;
    assert_eq!(
        diff_regs(&base, &s),
        vec![frame(0x52, 0x12, 0x00e4_28fc), frame(0x52, 0x22, 0x00e4_28fc)]
    );

    // Stereo PB strip at -1 dB: odd → L, even → R, full level each.
    let mut s = base.clone();
    s.mixes[0].pb12.fader_db = -1.0;
    assert_eq!(
        diff_regs(&base, &s),
        vec![frame(0x51, 0x13, 0x01c8_51f8), frame(0x51, 0x24, 0x01c8_51f8)]
    );

    // Stereo PB balance left of centre pans the even channel left (as the
    // app's PB2 pan does): 0.49 → PB2 at 0.98.
    let mut s = base.clone();
    s.mixes[0].pb12.pan = 0.49;
    assert_eq!(
        diff_regs(&base, &s),
        vec![frame(0x51, 0x14, 0x000a_3d70), frame(0x51, 0x24, 0x01f5_c270)]
    );

    // Mix strip phase: both sides negated.
    let mut s = base.clone();
    s.mixes[0].pb12.phase = true;
    assert_eq!(
        diff_regs(&base, &s),
        vec![frame(0x51, 0x13, -0x01ff_ffe0), frame(0x51, 0x24, -0x01ff_ffe0)]
    );

    // Mix strip solo: every other strip in that mix goes to 0.
    let mut s = base.clone();
    s.mixes[2].in1.solo = true;
    let regs: Vec<(u8, u8, i32)> = diff_regs(&base, &s)
        .iter()
        .map(|f| (f.group, f.param, f.value))
        .collect();
    assert_eq!(
        regs,
        vec![
            (0x53, 0x12, 0),
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
    assert_eq!(regs, vec![(0x34, 0x01), (0x34, 0x02), (0x31, 0x01), (0x31, 0x02)]);

    // Loopback mute: L and R to 0.
    let mut s = base.clone();
    s.loopbacks[1].mute = true;
    assert_eq!(
        diff_regs(&base, &s),
        vec![frame(0x33, 0x01, 0), frame(0x33, 0x02, 0)]
    );

    // A muted fader (-inf) writes 0.
    let mut s = base.clone();
    s.mixes[0].in1.fader_db = f64::NEG_INFINITY;
    assert_eq!(
        diff_regs(&base, &s),
        vec![frame(0x51, 0x11, 0), frame(0x51, 0x21, 0)]
    );

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
    assert_eq!(state.readout.volume_knob, Some(0x49));
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
