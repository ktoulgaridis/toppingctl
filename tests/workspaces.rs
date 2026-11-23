//! Workspaces (#17): named JSON files in the config dir, loaded through
//! `State::diff`. 48V is never loaded, and a file that can't be read leaves
//! the device and the state as they are, with a notice. No hardware: what
//! reaches the device goes through a `Connection` over `FakeTransport`.

use std::fs;
use std::path::PathBuf;

use toppingctl::conn::fake::{FakeHandle, FakeTransport};
use toppingctl::conn::Connection;
use toppingctl::frame::{Dir, Frame};
use toppingctl::state::{Source, State, Version};
use toppingctl::sync::Store;
use toppingctl::tui::app::{Action, App};
use toppingctl::tui::run::carry_out;
use toppingctl::tui::theme::Theme;
use toppingctl::workspace::{valid_name, Settings, WorkspaceError, Workspaces};

/// A fresh config dir under the system temp dir, unique to this test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("toppingctl-ws-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A state with something set everywhere a workspace keeps.
fn busy_state() -> State {
    let mut s = State::default();
    s.inputs[0].gain_db = 12;
    s.inputs[0].mon = true;
    s.inputs[1].inst = true;
    s.inputs[1].mute = true;
    s.mixes[0].in1.fader_db = -3.0;
    s.mixes[1].pb34.set_fader(-12.0);
    s.mixes[2].pb56.link = false;
    s.loopbacks[1].set_level(-6.0);
    s.output.source = Source::MixB;
    s.output.set_level(-10.0);
    s.phone_gain = true;
    s.system.brightness = 2;
    s.strip_names[1] = Some("synth".into());
    s.output_name = Some("mains".into());
    s
}

/// The device as `connect` leaves it: 48V known, the readout filled in.
fn device_state() -> State {
    let mut s = State::default();
    s.inputs[0].phantom = Some(false);
    s.inputs[1].phantom = Some(true);
    s.readout.firmware_version = Some(Version {
        major: 1,
        minor: 13,
    });
    s.readout.monitor_mix = Some(0x49);
    s
}

struct Rig {
    dir: PathBuf,
    store: Store,
    ws: Workspaces,
    app: App,
    conn: Option<Connection>,
    fake: FakeHandle,
}

impl Rig {
    fn new(name: &str, device: State) -> Self {
        let dir = temp_dir(name);
        let (transport, fake) = FakeTransport::new();
        let (conn, _events) = Connection::spawn(transport);
        let mut app = App::new(Theme::color());
        app.synced(device);
        Rig {
            store: Store::at(dir.join("state.json")),
            ws: Workspaces::at(dir.clone()),
            dir,
            app,
            conn: Some(conn),
            fake,
        }
    }

    fn run(&mut self, actions: Vec<Action>) -> Vec<Action> {
        carry_out(
            self.conn.as_ref().unwrap(),
            &mut self.app,
            &self.store,
            &self.ws,
            actions,
        )
        .unwrap()
    }

    /// Everything written to the device, after flushing.
    fn written(&mut self) -> Vec<Frame> {
        drop(self.conn.take());
        self.fake
            .written()
            .iter()
            .map(|r| Frame::decode(r, Dir::ToDevice).unwrap())
            .collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// `state` as a workspace keeps it: no 48V, no readout.
fn kept(state: &State) -> State {
    let mut s = state.clone();
    s.readout = Default::default();
    for input in &mut s.inputs {
        input.phantom = None;
    }
    s
}

/// AC1: save → load gives the same State, 48V and the readout aside.
#[test]
fn round_trip() {
    let dir = temp_dir("round-trip");
    let ws = Workspaces::at(dir.clone());
    let mut state = busy_state();
    state.inputs[1].phantom = Some(true);
    state.readout = device_state().readout;
    ws.save("Live set 2", &state).unwrap();
    assert!(dir.join("workspaces").join("Live set 2.json").is_file());
    assert_eq!(ws.load("Live set 2").unwrap(), kept(&state));
    assert_eq!(ws.list(), vec!["Live set 2".to_string()]);

    // The file says which version it is, and holds no 48V.
    let json = fs::read_to_string(dir.join("workspaces").join("Live set 2.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["version"], 1);
    assert!(!json.contains("phantom"), "{json}");

    // The pointer and auto-save, off by default.
    assert_eq!(ws.settings(), Settings::default());
    assert!(!Settings::default().auto_save);
    let settings = Settings {
        current: Some("Live set 2".into()),
        auto_save: true,
    };
    ws.save_settings(&settings).unwrap();
    assert_eq!(ws.settings(), settings);

    ws.save("another", &State::default()).unwrap();
    assert_eq!(ws.list(), vec!["Live set 2".to_string(), "another".into()]);
    ws.delete("Live set 2").unwrap();
    assert_eq!(ws.list(), vec!["another".to_string()]);
    let _ = fs::remove_dir_all(&dir);
}

/// AC1: loading sends exactly `State::diff` from the device's state to the
/// workspace's, then saves it as the state toppingctl last set.
#[test]
fn load_sends_diff() {
    let device = device_state();
    let mut rig = Rig::new("diff", device.clone());
    rig.ws.save("gig", &busy_state()).unwrap();
    rig.app.set_workspaces(rig.ws.list(), rig.ws.settings());

    let left = rig.run(vec![Action::LoadWorkspace("gig".into())]);
    assert!(left.is_empty(), "{left:?}");

    let mut expected = busy_state();
    for (e, d) in expected.inputs.iter_mut().zip(&device.inputs) {
        e.phantom = d.phantom;
    }
    expected.readout = device.readout.clone();
    assert_eq!(rig.app.state(), &expected);
    assert_eq!(rig.app.workspace(), Some("gig"));
    assert_eq!(rig.ws.settings().current.as_deref(), Some("gig"));
    assert_eq!(rig.store.load().unwrap().unwrap(), kept(&expected));
    assert_eq!(rig.written(), device.diff(&expected));

    // Loading it again changes nothing, so sends nothing.
    let mut rig = Rig::new("diff-again", expected.clone());
    rig.ws.save("gig", &busy_state()).unwrap();
    rig.run(vec![Action::LoadWorkspace("gig".into())]);
    assert!(rig.written().is_empty());
}

/// AC3: a load never turns 48V on. Each input keeps the device's 48V, and
/// a workspace file that says otherwise is ignored on that point.
#[test]
fn no_48v() {
    let mut device = device_state();
    device.inputs[0].phantom = Some(false);
    device.inputs[1].phantom = Some(false);
    let mut rig = Rig::new("no-48v", device);
    let mut json: serde_json::Value = serde_json::to_value(busy_state()).unwrap();
    for input in json["inputs"].as_array_mut().unwrap() {
        input["phantom"] = serde_json::Value::Bool(true);
    }
    let file = serde_json::json!({ "version": 1, "state": json });
    fs::create_dir_all(rig.dir.join("workspaces")).unwrap();
    fs::write(
        rig.dir.join("workspaces").join("hot.json"),
        file.to_string(),
    )
    .unwrap();

    rig.run(vec![Action::LoadWorkspace("hot".into())]);
    assert_eq!(rig.app.state().inputs[0].phantom, Some(false));
    assert_eq!(rig.app.state().inputs[1].phantom, Some(false));
    let written = rig.written();
    assert!(!written.is_empty(), "the rest of the workspace loads");
    assert!(
        !written
            .iter()
            .any(|f| matches!(f.group, 0x21 | 0x22) && f.param == 0x02),
        "no 48V frame: {written:?}"
    );

    // 48V that is on stays on: a load leaves it as it is.
    let mut device = device_state();
    device.inputs[1].phantom = Some(true);
    let mut rig = Rig::new("48v-on", device);
    rig.ws.save("cold", &busy_state()).unwrap();
    rig.run(vec![Action::LoadWorkspace("cold".into())]);
    assert_eq!(rig.app.state().inputs[1].phantom, Some(true));
    assert!(!rig
        .written()
        .iter()
        .any(|f| f.param == 0x02 && f.group == 0x22));
}

/// AC3: a corrupt file, an unknown version or a missing one leaves the
/// device, the state, the saved state and the pointer as they are, and says
/// so in a notice.
#[test]
fn corrupt() {
    let bad: [(&str, &[u8]); 6] = [
        ("garbage", b"\x00\xffnot json"),
        ("truncated", b"{\"version\": 1, \"state\": {\"inputs\": ["),
        ("future", b"{\"version\": 99, \"state\": {}}"),
        ("zero", b"{\"version\": 0, \"state\": {}}"),
        (
            "wrong-type",
            b"{\"version\": 1, \"state\": {\"inputs\": \"x\"}}",
        ),
        ("array", b"[1, 2, 3]"),
    ];
    for (name, bytes) in bad {
        let device = device_state();
        let mut rig = Rig::new(&format!("corrupt-{name}"), device.clone());
        fs::create_dir_all(rig.dir.join("workspaces")).unwrap();
        fs::write(
            rig.dir.join("workspaces").join(format!("{name}.json")),
            bytes,
        )
        .unwrap();
        assert!(
            matches!(
                rig.ws.load(name),
                Err(WorkspaceError::Corrupt { .. } | WorkspaceError::Version { .. })
            ),
            "{name}"
        );
        rig.run(vec![Action::LoadWorkspace(name.into())]);
        assert_eq!(rig.app.state(), &device, "{name}: state unchanged");
        assert_eq!(rig.app.workspace(), None, "{name}");
        let notice = rig.app.notice().unwrap_or_default().to_string();
        assert!(
            notice.contains(name) && notice.contains("nothing changed"),
            "{name}: {notice}"
        );
        assert!(rig.store.load().unwrap().is_none(), "{name}: no save");
        assert_eq!(rig.ws.settings(), Settings::default(), "{name}");
        assert!(rig.written().is_empty(), "{name}: nothing sent");
    }

    // A workspace that is gone: a notice, nothing sent.
    let mut rig = Rig::new("corrupt-missing", device_state());
    rig.run(vec![Action::LoadWorkspace("nope".into())]);
    assert!(rig.app.notice().unwrap_or_default().contains("nope"));
    assert!(rig.written().is_empty());
}

/// A saved state from before workspaces (the bare State JSON, as in the
/// #11 fixture) loads through the back-compat path.
#[test]
fn old_state_files_load() {
    let fixture = fs::read_to_string("tests/fixtures/state-issue11.json").unwrap();
    let expected: State = serde_json::from_str(&fixture).unwrap();
    let dir = temp_dir("legacy");
    fs::create_dir_all(dir.join("workspaces")).unwrap();
    fs::write(dir.join("workspaces").join("old.json"), &fixture).unwrap();
    let ws = Workspaces::at(dir.clone());
    assert_eq!(ws.load("old").unwrap(), expected);
    let _ = fs::remove_dir_all(&dir);
}

/// Names are sanitised: no path separators, no dot names, nothing that
/// would land outside the workspaces directory.
#[test]
fn names() {
    for good in ["gig", "Live set 2", "mix-1_b", "v1.2", "Ζωντανά"] {
        assert!(valid_name(good), "{good}");
    }
    for bad in [
        "",
        " ",
        ".",
        "..",
        ".hidden",
        "a/b",
        "a\\b",
        "../x",
        "c:x",
        "tab\there",
        "nul",
        "CON",
        "default",
        "Default",
        " padded",
        "padded ",
        "a*b",
        "a?b",
        "a\"b",
        "a<b",
        "a|b",
        &"x".repeat(33),
    ] {
        assert!(!valid_name(bad), "{bad:?}");
    }

    let dir = temp_dir("names");
    let ws = Workspaces::at(dir.join("toppingctl"));
    for bad in ["../evil", "a/b", ".."] {
        assert!(
            matches!(
                ws.save(bad, &State::default()),
                Err(WorkspaceError::Name(_))
            ),
            "{bad}"
        );
        assert!(matches!(ws.load(bad), Err(WorkspaceError::Name(_))));
        assert!(matches!(ws.delete(bad), Err(WorkspaceError::Name(_))));
    }
    assert!(!dir.join("evil.json").exists());
    assert!(!dir.join("toppingctl").exists(), "nothing written");

    // Files that aren't workspaces aren't listed.
    let wsdir = dir.join("toppingctl").join("workspaces");
    fs::create_dir_all(&wsdir).unwrap();
    fs::write(wsdir.join("notes.txt"), "x").unwrap();
    fs::write(wsdir.join(".hidden.json"), "{}").unwrap();
    fs::write(wsdir.join("ok.json"), "{}").unwrap();
    assert_eq!(ws.list(), vec!["ok".to_string()]);
    let _ = fs::remove_dir_all(&dir);
}
