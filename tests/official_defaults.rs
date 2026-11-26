//! `State::default()` is the official app's Default workspace (#59): every
//! field toppingctl models is pinned to `tests/fixtures/official/Default.config`,
//! a copy of the official app V1.09's own `<WS_DATA_PATH>/TPwork/Default.config`.

use serde_json::Value;
use toppingctl::state::{Source, State};

fn official() -> Value {
    let text = include_str!("fixtures/official/Default.config");
    serde_json::from_str(text).expect("Default.config is JSON")
}

fn get<'a>(v: &'a Value, section: &str, key: &str) -> &'a Value {
    v.get(section)
        .and_then(|s| s.get(key))
        .unwrap_or_else(|| panic!("Default.config has no {section}.{key}"))
}

fn flag(v: &Value, section: &str, key: &str) -> bool {
    get(v, section, key)
        .as_bool()
        .unwrap_or_else(|| panic!("{section}.{key} is not a bool"))
}

fn int(v: &Value, section: &str, key: &str) -> i64 {
    get(v, section, key)
        .as_i64()
        .unwrap_or_else(|| panic!("{section}.{key} is not an integer"))
}

/// The source register value (PROTOCOL.md) as a [`Source`].
fn source(wire: i64) -> Source {
    [
        Source::MixA,
        Source::MixB,
        Source::MixC,
        Source::Pb12,
        Source::Pb34,
        Source::Pb56,
        Source::In1,
        Source::In2,
        Source::In12,
    ]
    .into_iter()
    .find(|s| i64::from(s.wire()) == wire)
    .unwrap_or_else(|| panic!("unknown source {wire}"))
}

/// A linear level word (`0x02000000` = 0 dB) in dB.
fn level_db(word: i64) -> f64 {
    20.0 * (word as f64 / f64::from(0x0200_0000)).log10()
}

/// A mix fader's `Mix_volume_*num` (the app's crosspoint step, 335544 =
/// 0 dB) in dB.
fn fader_db(num: i64) -> f64 {
    20.0 * (num as f64 / 335_544.0).log10()
}

#[test]
fn inputs_match_the_official_default() {
    let o = official();
    let s = State::default();
    for (i, input) in s.inputs.iter().enumerate() {
        let sec = format!("IN{}", i + 1);
        assert_eq!(
            i64::from(input.gain_db),
            int(&o, &sec, "IN_volume"),
            "{sec} gain"
        );
        assert_eq!(input.mute, flag(&o, &sec, "MUTEEnable"), "{sec} MUTE");
        assert_eq!(input.solo, flag(&o, &sec, "SOLOEnable"), "{sec} SOLO");
        assert_eq!(input.phase, flag(&o, &sec, "ANTIEnable"), "{sec} Ø");
        assert_eq!(input.mon, flag(&o, &sec, "MONIEnable"), "{sec} MON");
        assert_eq!(input.inst, flag(&o, &sec, "INSTEnable"), "{sec} INST");
        // 48V is off in the app's default, and never part of the state.
        assert!(!flag(&o, &sec, "P48VEnable"));
        assert_eq!(input.phantom, None);
    }
}

/// `MixSaveA/B/C`, strips `MixParmeter_1` (IN 1+2) and `_2` … `_4` (PB 1/2,
/// 3/4, 5/6). `Mix_channel_*` is a pan, 0 … 100 towards its own side: 50 is
/// centre, 100 on the L channel is hard L (L100), on the R channel hard R.
#[test]
fn mixes_match_the_official_default() {
    let o = official();
    let s = State::default();
    for (m, mix) in s.mixes.iter().enumerate() {
        let sec = format!("MixSave{}", ["A", "B", "C"][m]);
        for strip in 0..4 {
            let key = |k: &str| format!("MixParmeter_{}.{k}", strip + 1);
            let at = |k: &str| int(&o, &sec, &key(k));
            let on = |k: &str| flag(&o, &sec, &key(k));
            let what = format!("{sec} strip {}", strip + 1);
            assert_eq!(mix.linked(strip), on("LinkEnable"), "{what} LINK");
            let pans = mix.pans(strip);
            assert_eq!(
                pans[0],
                1.0 - at("Mix_channel_L") as f64 / 100.0,
                "{what} pan L"
            );
            assert_eq!(pans[1], at("Mix_channel_R") as f64 / 100.0, "{what} pan R");
            for (ch, side) in ["L", "R"].into_iter().enumerate() {
                let c = mix.channel(strip, ch);
                let num = at(&format!("Mix_volume_{side}num"));
                assert_eq!(c.fader_db, fader_db(num), "{what} fader {side}");
                assert_eq!(
                    c.mute,
                    on(&format!("MUTEEnable_{side}")),
                    "{what} MUTE {side}"
                );
                assert_eq!(
                    c.solo,
                    on(&format!("SOLOEnable_{side}")),
                    "{what} SOLO {side}"
                );
                assert_eq!(
                    c.phase,
                    on(&format!("ANTIEnable_{side}")),
                    "{what} Ø {side}"
                );
            }
        }
    }
}

/// The two pans the issue names: IN 1+2 centred (not IN1 L100 / IN2 R100),
/// and PB 1/2 L100 / R100.
#[test]
fn in12_pans_are_centred_and_pb_pans_hard_left_and_right() {
    let s = State::default();
    for mix in &s.mixes {
        assert!(!mix.link);
        assert_eq!((mix.in1.pan, mix.in2.pan), (0.5, 0.5));
        for pb in [&mix.pb12, &mix.pb34, &mix.pb56] {
            assert!(pb.link);
            assert_eq!((pb.pan_l, pb.pan_r), (0.0, 1.0));
        }
    }
}

#[test]
fn output_matches_the_official_default() {
    let o = official();
    let out = State::default().output;
    assert_eq!(out.source, source(int(&o, "OUT12", "ChannelOUT_mode")));
    assert_eq!(out.source, Source::Pb12);
    assert_eq!(out.link, flag(&o, "OUT12", "LinkEnable"));
    assert_eq!(out.headphones, flag(&o, "OUT12", "HPEnable"));
    assert_eq!(out.line, flag(&o, "OUT12", "LINEOUTEnable"));
    assert_eq!(
        out.level_l_db,
        level_db(int(&o, "OUT12", "OUT_volume_Lnum"))
    );
    assert_eq!(
        out.level_r_db,
        level_db(int(&o, "OUT12", "OUT_volume_Rnum"))
    );
    // The output has no MUTE in toppingctl; the app's default is unmuted.
    assert!(!flag(&o, "OUT12", "MUTEEnable_L"));
    assert!(!flag(&o, "OUT12", "MUTEEnable_R"));
}

#[test]
fn loopbacks_match_the_official_default() {
    let o = official();
    let s = State::default();
    for (lb, sec) in s.loopbacks.iter().zip(["Loopback12", "Loopback34"]) {
        assert_eq!(lb.source, source(int(&o, sec, "ChannelOUT_mode")), "{sec}");
        assert_eq!(lb.link, flag(&o, sec, "LinkEnable"), "{sec}");
        assert_eq!(lb.level_l_db, level_db(int(&o, sec, "OUT_volume_Lnum")));
        assert_eq!(lb.level_r_db, level_db(int(&o, sec, "OUT_volume_Rnum")));
        assert_eq!(lb.mute_l, flag(&o, sec, "MUTEEnable_L"), "{sec}");
        assert_eq!(lb.mute_r, flag(&o, sec, "MUTEEnable_R"), "{sec}");
    }
    assert_eq!(s.loopbacks[0].source, Source::Pb34);
    assert_eq!(s.loopbacks[1].source, Source::Pb56);
}

/// `Gain_Enable` is the Phone Out level: true is +17 dBu (the user's own
/// workspace has it true with the app showing +17 dBu), false 0 dBu.
#[test]
fn phone_out_level_matches_the_official_default() {
    let o = official();
    let gain = o["Gain_Enable"].as_bool().expect("Gain_Enable");
    assert_eq!(State::default().phone_gain, gain);
    assert!(!State::default().phone_gain, "0 dBu");
}

/// A saved state keeps its own values: only the defaults changed.
#[test]
fn saved_values_are_kept() {
    let mut saved = State::default();
    saved.mixes[1].in1.pan = 0.0;
    saved.mixes[1].in2.pan = 1.0;
    saved.output.source = Source::MixA;
    saved.phone_gain = true;
    let json = serde_json::to_string(&saved).unwrap();
    let back: State = serde_json::from_str(&json).unwrap();
    assert_eq!(back, saved);
}
