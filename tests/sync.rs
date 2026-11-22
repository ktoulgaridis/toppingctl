use std::fs;
use std::path::{Path, PathBuf};

use toppingctl::conn::fake::{FakeHandle, FakeTransport};
use toppingctl::conn::Connection;
use toppingctl::frame::{Crc, Dir, Frame};
use toppingctl::state::{Source, State, Version};
use toppingctl::sync::{connect, state_path_from, Store, StoreError, SyncError};

fn frame(group: u8, param: u8, value: i32) -> Frame {
    Frame {
        group,
        param,
        value,
    }
}

/// A fresh directory under the system temp dir, unique to this test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("toppingctl-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Queues the device's reply to the `10 06` query, as in
/// `captures/mac-3/sweep-same-value.txt`, with brightness 0.
fn push_query_reply(fake: &FakeHandle) {
    for f in [
        frame(0x10, 0x06, 1),
        frame(0x10, 0x0b, 0),
        frame(0x10, 0x04, 0x0001_000d),
        frame(0x11, 0x01, 0x49),
        frame(0x10, 0x0a, 0),
        frame(0x10, 0x03, 0x0001_000e),
        frame(0x10, 0x0c, 0),
    ] {
        fake.push_report(&f.encode(Crc::Fill));
    }
}

fn decode_all(written: &[[u8; 16]]) -> Vec<Frame> {
    written
        .iter()
        .map(|r| Frame::decode(r, Dir::ToDevice).unwrap())
        .collect()
}

fn saved_state() -> State {
    let mut s = State::default();
    s.inputs[0].gain_db = 7;
    s.inputs[0].mon = true;
    s.inputs[1].phantom = Some(true);
    s.mixes[0].in1.fader_db = -1.0;
    s.mixes[1].pb34.pan = 0.25;
    s.loopbacks[0].source = Source::MixB;
    s.output.level_db = -6.0;
    s.system.brightness = 2;
    s.system.mobile_apps = true;
    s
}

#[test]
fn connect_pushes_saved_state() {
    let dir = temp_dir("connect");
    let store = Store::at(dir.join("state.json"));
    let saved = saved_state();
    store.save(&saved).unwrap();

    let (transport, fake) = FakeTransport::new();
    push_query_reply(&fake);
    let (conn, events) = Connection::spawn(transport);
    let state = connect(&conn, &events, &store).unwrap();
    drop(conn); // flushes every queued write
    let written = decode_all(&fake.written());

    assert_eq!(written[0], frame(0x10, 0x06, 1), "query first");
    let pushed = &written[1..];
    let expected: Vec<Frame> = saved
        .frames()
        .into_iter()
        .filter(|f| f.group != 0x10)
        .collect();
    assert_eq!(pushed, &expected[..]);
    assert!(
        written
            .iter()
            .all(|f| !(matches!(f.group, 0x21 | 0x22) && f.param == 0x02)),
        "no 48V frame"
    );

    // The device's settings win over the saved ones; 48V is unknown.
    assert_eq!(state.system.brightness, 0);
    assert!(!state.system.mobile_apps);
    assert_eq!(state.inputs[1].phantom, None);
    assert_eq!(state.inputs[0].gain_db, 7);
    assert_eq!(state.mixes[0].in1.fader_db, -1.0);
    assert_eq!(state.readout.volume_knob, Some(0x49));
    assert_eq!(
        state.readout.hardware_version,
        Some(Version {
            major: 1,
            minor: 14
        })
    );

    // First run: no saved state, nothing written after the query.
    let empty = Store::at(dir.join("missing").join("state.json"));
    let (transport, fake) = FakeTransport::new();
    push_query_reply(&fake);
    let (conn, events) = Connection::spawn(transport);
    match connect(&conn, &events, &empty) {
        Err(SyncError::NeedsInitialState(device)) => {
            assert_eq!(device.readout.volume_knob, Some(0x49));
            assert_eq!(device.system.brightness, 0);
        }
        other => panic!("expected NeedsInitialState, got {other:?}"),
    }
    drop(conn);
    assert_eq!(decode_all(&fake.written()), vec![frame(0x10, 0x06, 1)]);
    assert!(!dir.join("missing").exists(), "first run writes no file");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn store_round_trip() {
    let dir = temp_dir("store");
    let path = dir.join("nested").join("state.json");
    let store = Store::at(path.clone());
    assert_eq!(store.path(), path.as_path());
    assert!(store.load().unwrap().is_none());

    let mut state = saved_state();
    state.mixes[2].in2.fader_db = f64::NEG_INFINITY;
    store.save(&state).unwrap();
    let loaded = store.load().unwrap().expect("a saved state");
    // 48V and the readout are not persisted.
    let mut expected = state.clone();
    expected.inputs[1].phantom = None;
    assert_eq!(loaded, expected);
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("phantom"), "48V is not persisted: {text}");
    // No temp file is left behind by the atomic write.
    let names: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("state.json")]);

    // A corrupt file is an error and is left untouched.
    fs::write(&path, b"{ not json").unwrap();
    assert!(matches!(store.load(), Err(StoreError::Corrupt { .. })));
    assert!(matches!(store.save(&state), Err(StoreError::Corrupt { .. })));
    assert_eq!(fs::read(&path).unwrap(), b"{ not json");

    // The path honours XDG_CONFIG_HOME, and falls back to the home dir.
    let xdg = Path::new("/xdg/config");
    let home = Path::new("/home/u");
    assert_eq!(
        state_path_from(Some(xdg), Some(home)),
        Some(PathBuf::from("/xdg/config/toppingctl/state.json"))
    );
    let fallback = if cfg!(target_os = "macos") {
        "/home/u/Library/Application Support/toppingctl/state.json"
    } else {
        "/home/u/.config/toppingctl/state.json"
    };
    assert_eq!(
        state_path_from(None, Some(home)),
        Some(PathBuf::from(fallback))
    );
    // A relative or empty XDG_CONFIG_HOME is ignored, as the XDG spec says.
    assert_eq!(
        state_path_from(Some(Path::new("rel")), Some(home)),
        Some(PathBuf::from(fallback))
    );
    assert_eq!(state_path_from(None, None), None);

    let _ = fs::remove_dir_all(&dir);
}
