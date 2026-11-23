//! Store to device (#18): `w` (or a click on the header's `w`) opens the
//! official app's Download dialog. Each of its three choices does its saves
//! and then sends ParameterSave `1a 03`; Cancel does nothing. On the pure
//! `App`, carried out by `run::carry_out` against temp files and a
//! `Connection` over `FakeTransport`.
//!
//! Golden snapshots live in `tests/snapshots/`; run with
//! `UPDATE_SNAPSHOTS=1` to rewrite them after an intended change.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use toppingctl::conn::fake::{FakeHandle, FakeTransport};
use toppingctl::conn::Connection;
use toppingctl::frame::{Dir, Frame};
use toppingctl::state::{Meters, State};
use toppingctl::sync::Store;
use toppingctl::tui::app::{Action, App, Modal, NameThen, DOWNLOAD_OPTIONS};
use toppingctl::tui::run::carry_out;
use toppingctl::tui::theme::Theme;
use toppingctl::tui::ui::{self, Hit, HitMap};
use toppingctl::workspace::{Settings, Workspaces};

const STORE: Frame = Frame {
    group: 0x1a,
    param: 0x03,
    value: 0,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn render(app: &App, w: u16, h: u16) -> (Buffer, HitMap) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    let meters = Meters::default();
    terminal.draw(|f| hits = ui::draw(f, app, &meters)).unwrap();
    (terminal.backend().buffer().clone(), hits)
}

fn text(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::write(&path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_SNAPSHOTS=1)", path.display()));
    assert!(
        expected == actual,
        "snapshot {name} differs\n--- expected\n{expected}--- actual\n{actual}"
    );
}

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("toppingctl-tuistore-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn device_state() -> State {
    let mut s = State::default();
    s.inputs[0].phantom = Some(false);
    s.inputs[1].phantom = Some(false);
    s.inputs[0].gain_db = 9;
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
    fn new(name: &str, current: Option<&str>) -> Self {
        let dir = temp_dir(name);
        let ws = Workspaces::at(dir.clone());
        if let Some(c) = current {
            ws.save(c, &State::default()).unwrap();
        }
        ws.save_settings(&Settings {
            current: current.map(String::from),
            auto_save: false,
        })
        .unwrap();
        let (transport, fake) = FakeTransport::new();
        let (conn, _events) = Connection::spawn(transport);
        let mut app = App::new(Theme::color());
        app.synced(device_state());
        app.set_workspaces(ws.list(), ws.settings());
        Rig {
            store: Store::at(dir.join("state.json")),
            ws,
            dir,
            app,
            conn: Some(conn),
            fake,
        }
    }

    fn key(&mut self, k: KeyEvent) -> Vec<Action> {
        let actions = self.app.handle_key(k);
        let left = carry_out(
            self.conn.as_ref().unwrap(),
            &mut self.app,
            &self.store,
            &self.ws,
            actions.clone(),
        )
        .unwrap();
        assert!(left.is_empty(), "{left:?}");
        actions
    }

    fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            self.key(ch(c));
        }
    }

    fn click(&mut self, hit: Hit) -> Vec<Action> {
        let (_, hits) = render(&self.app, 120, 40);
        let r = hits
            .rect(hit)
            .unwrap_or_else(|| panic!("{hit:?} not on screen"));
        let at = |kind| MouseEvent {
            kind,
            column: r.x,
            row: r.y,
            modifiers: KeyModifiers::NONE,
        };
        let now = Instant::now();
        let mut actions =
            self.app
                .handle_mouse_at(at(MouseEventKind::Down(MouseButton::Left)), &hits, now);
        actions.extend(self.app.handle_mouse_at(
            at(MouseEventKind::Up(MouseButton::Left)),
            &hits,
            now,
        ));
        carry_out(
            self.conn.as_ref().unwrap(),
            &mut self.app,
            &self.store,
            &self.ws,
            actions.clone(),
        )
        .unwrap();
        actions
    }

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

/// `w` opens the Download dialog with the app's three choices.
#[test]
fn dialog_and_snapshot() {
    assert_eq!(
        DOWNLOAD_OPTIONS,
        [
            "Save the current workspace and download to the device",
            "Save as workspace… and download",
            "Download to device only",
        ]
    );
    let mut rig = Rig::new("dialog", Some("gig"));
    rig.key(ch('w'));
    assert_eq!(rig.app.modal(), Some(Modal::Download { sel: 0 }));
    let (buf, _) = render(&rig.app, 120, 40);
    let screen = text(&buf);
    for option in DOWNLOAD_OPTIONS {
        assert!(screen.contains(option), "{option}:\n{screen}");
    }
    assert_snapshot("download_120x40", &screen);
    rig.key(key(KeyCode::Up));
    assert_eq!(rig.app.modal(), Some(Modal::Download { sel: 0 }));
    rig.key(key(KeyCode::Down));
    rig.key(key(KeyCode::Down));
    rig.key(key(KeyCode::Down));
    assert_eq!(rig.app.modal(), Some(Modal::Download { sel: 2 }));
}

/// Choice 1 on a named workspace: save it, then `1a 03`.
#[test]
fn save_current_and_download() {
    let mut rig = Rig::new("current", Some("gig"));
    rig.key(ch('w'));
    let actions = rig.key(key(KeyCode::Enter));
    assert_eq!(
        actions,
        vec![Action::SaveWorkspace("gig".into()), Action::StoreToDevice]
    );
    assert_eq!(rig.app.modal(), None);
    assert_eq!(rig.ws.load("gig").unwrap().inputs[0].gain_db, 9);
    assert_eq!(rig.written(), vec![STORE]);
}

/// Choice 1 on Default asks for a name first, as `s` does.
#[test]
fn save_current_on_default_asks_a_name() {
    let mut rig = Rig::new("current-default", None);
    rig.key(ch('w'));
    rig.key(key(KeyCode::Enter));
    assert_eq!(
        rig.app.modal(),
        Some(Modal::WorkspaceName {
            then: NameThen::SaveAsAndStore
        })
    );
    rig.type_text("gig");
    rig.key(key(KeyCode::Enter));
    assert_eq!(rig.app.workspace(), Some("gig"));
    assert_eq!(rig.ws.load("gig").unwrap().inputs[0].gain_db, 9);
    assert_eq!(rig.written(), vec![STORE]);
}

/// Choice 2: a name (with "Overwrite?" for an existing one), the save,
/// then `1a 03`. Cancelling the name sends nothing.
#[test]
fn save_as_and_download() {
    let mut rig = Rig::new("save-as", Some("gig"));
    rig.key(ch('w'));
    rig.key(key(KeyCode::Down));
    rig.key(key(KeyCode::Enter));
    assert_eq!(
        rig.app.modal(),
        Some(Modal::WorkspaceName {
            then: NameThen::SaveAsAndStore
        })
    );
    rig.key(key(KeyCode::Esc));
    assert_eq!(rig.app.modal(), None);

    rig.key(ch('w'));
    rig.key(key(KeyCode::Down));
    rig.key(key(KeyCode::Enter));
    rig.type_text("gig");
    rig.key(key(KeyCode::Enter));
    assert_eq!(
        rig.app.modal(),
        Some(Modal::Overwrite {
            then: NameThen::SaveAsAndStore
        })
    );
    rig.key(ch('n'));

    rig.key(ch('w'));
    rig.key(key(KeyCode::Down));
    rig.key(key(KeyCode::Enter));
    rig.type_text("live");
    let actions = rig.key(key(KeyCode::Enter));
    assert_eq!(
        actions,
        vec![Action::SaveWorkspace("live".into()), Action::StoreToDevice]
    );
    assert_eq!(rig.app.workspace(), Some("live"));
    assert_eq!(rig.ws.load("live").unwrap().inputs[0].gain_db, 9);
    assert_eq!(rig.written(), vec![STORE], "once, after the save");
}

/// Choice 3: only `1a 03`.
#[test]
fn download_only() {
    let mut rig = Rig::new("only", None);
    rig.key(ch('w'));
    rig.key(key(KeyCode::Down));
    rig.key(key(KeyCode::Down));
    let actions = rig.key(key(KeyCode::Enter));
    assert_eq!(actions, vec![Action::StoreToDevice]);
    assert!(rig.ws.list().is_empty(), "nothing saved");
    assert_eq!(rig.written(), vec![STORE]);
}

/// Cancel (Esc, or the button) sends nothing and saves nothing.
#[test]
fn cancel_sends_nothing() {
    let mut rig = Rig::new("cancel", Some("gig"));
    let before = fs::read(rig.dir.join("workspaces").join("gig.json")).unwrap();
    rig.key(ch('w'));
    assert!(rig.key(key(KeyCode::Esc)).is_empty());
    assert_eq!(rig.app.modal(), None);
    rig.click(Hit::Download);
    assert!(rig.click(Hit::No).is_empty());
    assert_eq!(rig.app.modal(), None);
    assert_eq!(
        fs::read(rig.dir.join("workspaces").join("gig.json")).unwrap(),
        before
    );
    assert!(rig.written().is_empty());
}

/// By mouse: the header's `w`, a choice, OK.
#[test]
fn mouse() {
    let mut rig = Rig::new("mouse", Some("gig"));
    rig.click(Hit::Download);
    assert_eq!(rig.app.modal(), Some(Modal::Download { sel: 0 }));
    rig.click(Hit::DownloadOption(2));
    assert_eq!(rig.app.modal(), Some(Modal::Download { sel: 2 }));
    let actions = rig.click(Hit::Yes);
    assert_eq!(actions, vec![Action::StoreToDevice]);
    assert_eq!(rig.written(), vec![STORE]);
}

/// A save that fails stops the store: nothing is sent, and a notice says
/// why.
#[test]
fn failed_save_skips_the_store() {
    let mut rig = Rig::new("fail", Some("gig"));
    // The workspace file can't be replaced: a directory sits in its place.
    let file = rig.dir.join("workspaces").join("gig.json");
    fs::remove_file(&file).unwrap();
    fs::create_dir_all(file.join("x")).unwrap();
    rig.key(ch('w'));
    rig.key(key(KeyCode::Enter));
    let notice = rig.app.notice().unwrap_or_default().to_string();
    assert!(notice.contains("gig"), "{notice}");
    assert!(rig.written().is_empty());
}
