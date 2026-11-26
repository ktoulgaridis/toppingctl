//! Importing the official app's workspaces (#58). The fixtures in
//! `tests/fixtures/official/` are files written by our own install of the
//! official app V1.09 (user data, not the app).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use toppingctl::state::{Source, State};
use toppingctl::tpwork::{self, ImportError, Official};
use toppingctl::workspace::Workspaces;

const USER: &str = include_str!("fixtures/official/E2X2_New202608221427.TPwork");
const DEFAULT: &str = include_str!("fixtures/official/Default.config");

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("toppingctl-tpwork-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn parse(v: &Value) -> tpwork::Imported {
    tpwork::parse(v.to_string().as_bytes()).unwrap()
}

/// The user's workspace, as the official app shows it: IN1 +2 dB with MUTE
/// on, IN2 +0 dB, every other input switch off; Mix A IN 1+2 unlinked and
/// centred, PB 1/2, 3/4, 5/6 linked, faders 0 dB; Loopback 1+2 from PB 3/4,
/// 3+4 from PB 5/6, linked, 0 dB; Output 1+2 from Mix A, linked, 0 dB,
/// headphones and line on; Phone Out +17 dBu.
#[test]
fn user_workspace_matches_the_app_screen() {
    let got = tpwork::parse(USER.as_bytes()).unwrap();
    assert!(got.notices.is_empty(), "{:?}", got.notices);
    let s = got.state;

    let in1 = &s.inputs[0];
    assert_eq!((in1.gain_db, in1.mute), (2, true));
    let in2 = &s.inputs[1];
    assert_eq!((in2.gain_db, in2.mute), (0, false));
    for input in &s.inputs {
        assert!(!input.solo && !input.phase && !input.mon && !input.inst);
        assert_eq!(input.phantom, None, "48V is never in a workspace");
    }

    for mix in &s.mixes {
        assert!(!mix.link, "IN 1+2 link off");
        assert_eq!((mix.in1.pan, mix.in2.pan), (0.5, 0.5), "IN 1+2 centred");
        assert_eq!((mix.in1.fader_db, mix.in2.fader_db), (0.0, 0.0));
        for strip in 0..4 {
            for ch in 0..2 {
                let c = mix.channel(strip, ch);
                assert_eq!(c.fader_db, 0.0);
                assert!(!c.mute && !c.solo && !c.phase);
            }
        }
        for pb in [&mix.pb12, &mix.pb34, &mix.pb56] {
            assert!(pb.link);
            assert_eq!((pb.pan_l, pb.pan_r), (0.0, 1.0), "L100 / R100");
        }
    }

    assert_eq!(s.loopbacks[0].source, Source::Pb34);
    assert_eq!(s.loopbacks[1].source, Source::Pb56);
    for lb in &s.loopbacks {
        assert!(lb.link && !lb.mute_l && !lb.mute_r);
        assert_eq!((lb.level_l_db, lb.level_r_db), (0.0, 0.0));
    }

    let out = &s.output;
    assert_eq!(out.source, Source::MixA);
    assert!(out.link && out.headphones && out.line);
    assert_eq!((out.level_l_db, out.level_r_db), (0.0, 0.0));
    assert!(s.phone_gain, "+17 dBu");

    // Not in a workspace: the settings and the names stay the defaults.
    let d = State::default();
    assert_eq!(s.system, d.system);
    assert_eq!(s.strip_names, d.strip_names);
}

/// The app's own gain word agrees with `IN_volume` (the +2 dB word is
/// `INPUT_GAIN_LEVELS[2]`), and without `IN_volume` the word is used.
#[test]
fn input_gain_word_agrees() {
    let v: Value = serde_json::from_str(USER).unwrap();
    assert_eq!(
        v["IN1"]["IN_volumenum"].as_i64(),
        Some(i64::from(toppingctl::state::INPUT_GAIN_LEVELS[2]))
    );
    let mut v = v;
    v["IN1"].as_object_mut().unwrap().remove("IN_volume");
    v["IN1"]["IN_volumenum"] = json!(toppingctl::state::INPUT_GAIN_LEVELS[7]);
    assert_eq!(parse(&v).state.inputs[0].gain_db, 7);
}

/// The official Default workspace imports without notices; every mapped
/// field there is the app's default.
#[test]
fn official_default_imports() {
    let got = tpwork::parse(DEFAULT.as_bytes()).unwrap();
    assert!(got.notices.is_empty(), "{:?}", got.notices);
    let s = got.state;
    assert_eq!(s.output.source, Source::Pb12);
    assert!(!s.phone_gain, "0 dBu");
    for mix in &s.mixes {
        assert_eq!((mix.in1.pan, mix.in2.pan), (0.5, 0.5));
    }
}

/// 48V is never imported: `P48VEnable` is dropped, with a notice naming the
/// input, and the saved workspace has no 48V in it.
#[test]
fn phantom_is_never_imported() {
    let mut v: Value = serde_json::from_str(USER).unwrap();
    v["IN2"]["P48VEnable"] = json!(true);
    let got = parse(&v);
    assert_eq!(got.state.inputs[1].phantom, None);
    assert_eq!(got.notices.len(), 1, "{:?}", got.notices);
    assert!(got.notices[0].contains("48V") && got.notices[0].contains("IN2"));

    let dir = temp_dir("phantom");
    let file = dir.join("E2X2_Phantom.TPwork");
    fs::write(&file, v.to_string()).unwrap();
    let ws = Workspaces::at(dir.join("cfg"));
    let saved = tpwork::import(&ws, &file, None, false).unwrap();
    assert_eq!(saved.name, "Phantom");
    assert_eq!(saved.notices, got.notices);
    let text = fs::read_to_string(ws.path("Phantom").unwrap()).unwrap();
    assert!(!text.contains("phantom") && !text.contains("48"), "{text}");
    let _ = fs::remove_dir_all(&dir);
}

/// Every mapped field, away from its default.
#[test]
fn every_field_maps() {
    let mut v: Value = serde_json::from_str(DEFAULT).unwrap();
    let ins = json!({
        "IN_volume": 20, "MUTEEnable": false, "SOLOEnable": true, "ANTIEnable": true,
        "MONIEnable": true, "INSTEnable": true, "P48VEnable": false
    });
    v["IN2"] = ins;
    let b = v["MixSaveB"].as_object_mut().unwrap();
    for (k, x) in [
        ("MixParmeter_1.LinkEnable", json!(true)),
        ("MixParmeter_1.Mix_channel_L", json!(100)),
        ("MixParmeter_1.Mix_channel_R", json!(0)),
        ("MixParmeter_1.Mix_volume_Lnum", json!(0)),
        ("MixParmeter_1.MUTEEnable_R", json!(true)),
        ("MixParmeter_3.LinkEnable", json!(false)),
        ("MixParmeter_3.Mix_channel_L", json!(50)),
        ("MixParmeter_3.Mix_channel_R", json!(25)),
        // 335544 · 10^(−6/20) rounded: the app's −6 dB step.
        ("MixParmeter_3.Mix_volume_Rnum", json!(168170)),
        ("MixParmeter_3.SOLOEnable_L", json!(true)),
        ("MixParmeter_4.ANTIEnable_R", json!(true)),
        // Above the fader top (≈ +6 dB): capped.
        ("MixParmeter_2.Mix_volume_Lnum", json!(900_000)),
    ] {
        b.insert(k.into(), x);
    }
    v["Loopback34"]["ChannelOUT_mode"] = json!(0x3000);
    v["Loopback34"]["LinkEnable"] = json!(false);
    v["Loopback34"]["OUT_volume_Lnum"] = json!(0x0100_0000);
    v["Loopback34"]["OUT_volume_Rnum"] = json!(0);
    v["Loopback34"]["MUTEEnable_R"] = json!(true);
    v["OUT12"]["ChannelOUT_mode"] = json!(0x20);
    v["OUT12"]["HPEnable"] = json!(false);
    v["OUT12"]["LINEOUTEnable"] = json!(false);
    v["OUT12"]["LinkEnable"] = json!(false);
    v["OUT12"]["OUT_volume_Rnum"] = json!(0x0100_0000);
    v["Gain_Enable"] = json!(true);

    let got = parse(&v);
    assert!(got.notices.is_empty(), "{:?}", got.notices);
    let s = got.state;
    let in2 = &s.inputs[1];
    assert_eq!(in2.gain_db, 20);
    assert!(!in2.mute && in2.solo && in2.phase && in2.mon && in2.inst);

    let m = &s.mixes[1];
    assert!(m.link);
    assert_eq!((m.in1.pan, m.in2.pan), (0.0, 0.0), "IN1 L100, IN2 L100");
    assert_eq!(m.in1.fader_db, f64::NEG_INFINITY);
    assert!(m.in2.mute && !m.in1.mute);
    assert!(!m.pb34.link);
    assert_eq!((m.pb34.pan_l, m.pb34.pan_r), (0.5, 0.25));
    assert_eq!(m.pb34.fader_r_db, -6.0);
    assert_eq!(m.pb34.fader_l_db, 0.0);
    assert!(m.pb34.solo_l && !m.pb34.solo_r);
    assert!(m.pb56.phase_r && !m.pb56.phase_l);
    let top = m.pb12.fader_l_db;
    assert!(top > 5.9 && top < 6.1, "{top}");
    // Mix A and C as in the Default workspace.
    let base = tpwork::parse(DEFAULT.as_bytes()).unwrap().state;
    assert_eq!(s.mixes[0], base.mixes[0]);
    assert_eq!(s.mixes[2], base.mixes[2]);

    let lb = &s.loopbacks[1];
    assert_eq!(lb.source, Source::In12);
    assert!(!lb.link && lb.mute_r && !lb.mute_l);
    assert_eq!(lb.level_l_db, -6.02);
    assert_eq!(lb.level_r_db, f64::NEG_INFINITY);

    let out = &s.output;
    assert_eq!(out.source, Source::MixB);
    assert!(!out.headphones && !out.line && !out.link);
    assert_eq!((out.level_l_db, out.level_r_db), (0.0, -6.02));
    assert!(s.phone_gain);
}

/// What toppingctl can't hold comes with a notice: an output MUTE (the
/// level goes to −∞), Ø on a bus, an unknown source.
#[test]
fn unmodelled_fields_give_notices() {
    let mut v: Value = serde_json::from_str(USER).unwrap();
    v["OUT12"]["MUTEEnable_L"] = json!(true);
    v["Loopback12"]["ANTIEnable"] = json!(true);
    v["Loopback34"]["ChannelOUT_mode"] = json!(4);
    let got = parse(&v);
    assert_eq!(got.state.output.level_l_db, f64::NEG_INFINITY);
    assert_eq!(got.state.output.level_r_db, 0.0);
    assert_eq!(
        got.state.loopbacks[1].source,
        State::default().loopbacks[1].source
    );
    assert_eq!(got.notices.len(), 3, "{:?}", got.notices);
}

/// Missing sections and fields keep the defaults; unknown ones are
/// ignored; a file that isn't a workspace is refused.
#[test]
fn missing_and_unknown_fields() {
    let got = parse(&json!({"IN1": {"IN_volume": 5, "Future": 1}, "Other": []}));
    let mut want = State::default();
    want.inputs[0].gain_db = 5;
    assert_eq!(got.state, want);
    for bad in ["[]", "{}", "{\"CompanyName\": \"Topping\"}", "not json"] {
        assert!(tpwork::parse(bad.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn names_come_from_the_file() {
    let name = |p: &str| tpwork::default_name(Path::new(p));
    assert_eq!(
        name("/x/E2X2_New202608221427.TPwork").as_deref(),
        Some("New202608221427")
    );
    assert_eq!(name("/x/e2x2_Live.TPwork").as_deref(), Some("Live"));
    assert_eq!(name("/x/Studio.TPwork").as_deref(), Some("Studio"));
    assert_eq!(name("/x/E2X2_Default.TPwork"), None, "Default is reserved");
    assert_eq!(name("/x/E2X2_.TPwork"), None);
}

/// The folder comes from the app's `WS_DATA_PATH` (plus `TPwork`), else the
/// macOS default; the list holds only `E2X2_*.TPwork` files; a missing
/// folder (Linux) lists nothing.
#[test]
fn folder_and_list() {
    let home = temp_dir("home");
    let app = home.join(tpwork::APP_DIR);
    assert_eq!(tpwork::folder(&home), app.join("TPwork").join("TPwork"));
    assert!(tpwork::list(&tpwork::folder(&home)).is_empty());

    let data = home.join("elsewhere");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join(tpwork::SETTINGS_FILE),
        json!({"WS_DATA_PATH": data.to_str().unwrap(), "Language": "English"}).to_string(),
    )
    .unwrap();
    let folder = tpwork::folder(&home);
    assert_eq!(folder, data.join("TPwork"));

    fs::create_dir_all(&folder).unwrap();
    for f in [
        "E2X2_Live.TPwork",
        "E2X2_Band.TPwork",
        "Default.config",
        "E4X4_Other.TPwork",
        "notes.txt",
    ] {
        fs::write(folder.join(f), USER).unwrap();
    }
    fs::create_dir_all(folder.join("E2X2_Dir.TPwork")).unwrap();
    assert_eq!(
        tpwork::list(&folder),
        vec![
            Official {
                name: "Band".into(),
                path: folder.join("E2X2_Band.TPwork")
            },
            Official {
                name: "Live".into(),
                path: folder.join("E2X2_Live.TPwork")
            },
        ]
    );
    let _ = fs::remove_dir_all(&home);
}

/// `import` saves a workspace that loads back as the parsed state; an
/// existing name needs `force`; a bad name is refused before anything is
/// written.
#[test]
fn import_saves_a_workspace() {
    let dir = temp_dir("import");
    let file = dir.join("E2X2_New202608221427.TPwork");
    fs::write(&file, USER).unwrap();
    let ws = Workspaces::at(dir.join("cfg"));

    let saved = tpwork::import(&ws, &file, None, false).unwrap();
    assert_eq!(saved.name, "New202608221427");
    let want = tpwork::parse(USER.as_bytes()).unwrap().state;
    assert_eq!(ws.load("New202608221427").unwrap(), want);

    assert!(matches!(
        tpwork::import(&ws, &file, None, false),
        Err(ImportError::Exists(_))
    ));
    assert!(tpwork::import(&ws, &file, None, true).is_ok());
    let saved = tpwork::import(&ws, &file, Some("Mine"), false).unwrap();
    assert_eq!(saved.name, "Mine");
    assert!(matches!(
        tpwork::import(&ws, &file, Some("a/b"), false),
        Err(ImportError::Name(_))
    ));
    assert!(matches!(
        tpwork::import(&ws, &dir.join("missing.TPwork"), None, false),
        Err(ImportError::Read { .. })
    ));
    assert_eq!(ws.list(), vec!["Mine", "New202608221427"]);
    let _ = fs::remove_dir_all(&dir);
}
