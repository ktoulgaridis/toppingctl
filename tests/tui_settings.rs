//! The Settings dialog (#38): the official app's settings button
//! (`PButton_Set`) opens SYSTEMSETTING. `e` opens the TUI's equivalent:
//! display brightness (`10 0a`, 0/1/2), automatic standby (`10 0b`) and
//! mobile applications (`10 0c`, which takes effect only after the device
//! restarts, so changing it shows a notice), then the versions. No
//! terminal, no hardware: frames go through a real `Connection` over
//! `FakeTransport`.
//!
//! Golden snapshots live in `tests/snapshots/`; run with
//! `UPDATE_SNAPSHOTS=1` to rewrite them after an intended change.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use toppingctl::conn::fake::{FakeHandle, FakeTransport};
use toppingctl::conn::Connection;
use toppingctl::frame::{Dir, Frame};
use toppingctl::state::{Meters, State, Version};
use toppingctl::tui::app::{Action, App, Modal, Panel};
use toppingctl::tui::run::send_actions;
use toppingctl::tui::theme::Theme;
use toppingctl::tui::ui::{self, Hit, HitMap};

// ---------------------------------------------------------------- helpers

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn synced() -> App {
    let mut app = App::new(Theme::color());
    app.synced(State::default());
    app
}

/// A synced app with the Settings dialog open.
fn open() -> App {
    let mut app = synced();
    assert!(app.handle_key(ch('e')).is_empty());
    assert_eq!(app.modal(), Some(Modal::Settings { sel: 0 }));
    app
}

fn render(app: &App, w: u16, h: u16) -> (Buffer, HitMap) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    terminal
        .draw(|f| hits = ui::draw(f, app, &Meters::default()))
        .unwrap();
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

/// The dialog's text, with the app version (it changes on every release)
/// as `X.Y.Z`.
fn dialog_text(buf: &Buffer) -> String {
    text(buf).replace(env!("CARGO_PKG_VERSION"), "X.Y.Z")
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

fn frame(param: u8, value: i32) -> Frame {
    Frame {
        group: 0x10,
        param,
        value,
    }
}

/// The frame sent, then a save: the one register that changed.
fn sent(param: u8, value: i32) -> Vec<Action> {
    vec![Action::Send(frame(param, value)), Action::Save]
}

fn click(app: &mut App, hits: &HitMap, hit: Hit) -> Vec<Action> {
    let r = hits
        .rect(hit)
        .unwrap_or_else(|| panic!("{hit:?} not on screen"));
    let ev = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: r.x,
        row: r.y,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(ev, hits)
}

struct Device {
    conn: Option<Connection>,
    fake: FakeHandle,
}

impl Device {
    fn new() -> Self {
        let (transport, fake) = FakeTransport::new();
        let (conn, _events) = Connection::spawn(transport);
        Device {
            conn: Some(conn),
            fake,
        }
    }

    fn send(&self, actions: &[Action]) {
        send_actions(self.conn.as_ref().unwrap(), actions).unwrap();
    }

    fn written(mut self) -> Vec<Frame> {
        drop(self.conn.take());
        self.fake
            .written()
            .iter()
            .map(|r| Frame::decode(r, Dir::ToDevice).unwrap())
            .collect()
    }
}

// ---------------------------------------------------------- open / close

/// `e` opens the dialog and `e` or Esc closes it; neither sends anything
/// nor moves the panel focus.
#[test]
fn e_opens_and_closes() {
    let mut app = open();
    let focus = app.focus();
    assert!(app.handle_key(ch('e')).is_empty());
    assert_eq!(app.modal(), None);
    assert!(app.handle_key(ch('e')).is_empty());
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert_eq!(app.modal(), None);
    assert_eq!(app.focus(), focus);
    assert_eq!(app.state(), &State::default());
}

/// Not before the device is synced: the splash takes no keys.
#[test]
fn closed_while_connecting() {
    let mut app = App::new(Theme::color());
    assert!(app.handle_key(ch('e')).is_empty());
    assert_eq!(app.modal(), None);
}

/// The close button closes it.
#[test]
fn click_close() {
    let mut app = open();
    let (_, hits) = render(&app, 120, 40);
    assert!(click(&mut app, &hits, Hit::No).is_empty());
    assert_eq!(app.modal(), None);
}

/// Inside the dialog ↑↓ and Tab / Shift-Tab move between the three
/// settings and never send; the panels' focus stays put.
#[test]
fn arrows_and_tab_move_between_settings() {
    let mut app = open();
    let focus = app.focus();
    let mut walk = |k: KeyCode, want: usize| {
        assert!(app.handle_key(key(k)).is_empty(), "{k:?}");
        assert_eq!(app.modal(), Some(Modal::Settings { sel: want }), "{k:?}");
    };
    walk(KeyCode::Up, 0);
    walk(KeyCode::Down, 1);
    walk(KeyCode::Down, 2);
    walk(KeyCode::Down, 2);
    walk(KeyCode::Up, 1);
    walk(KeyCode::Tab, 2);
    walk(KeyCode::Tab, 0);
    walk(KeyCode::BackTab, 2);
    assert_eq!(app.focus(), focus);
    assert_eq!(app.focus().panel(), Panel::Input);
    assert_eq!(app.state(), &State::default());
}

// ------------------------------------------------------------ brightness

/// Brightness (`10 0a`): → and + brighten, ← and - dim, one step each, at
/// the ends nothing; Enter and space cycle Dim → Normal → Bright → Dim.
#[test]
fn brightness_keys_send_10_0a() {
    let mut app = open();
    assert_eq!(app.state().system.brightness, 1);
    assert_eq!(app.handle_key(key(KeyCode::Right)), sent(0x0a, 2));
    assert_eq!(app.state().system.brightness, 2);
    assert!(app.handle_key(ch('+')).is_empty(), "already Bright");
    assert_eq!(app.handle_key(key(KeyCode::Left)), sent(0x0a, 1));
    assert_eq!(app.handle_key(ch('-')), sent(0x0a, 0));
    assert!(app.handle_key(key(KeyCode::Left)).is_empty(), "already Dim");
    assert_eq!(app.state().system.brightness, 0);
    assert_eq!(app.handle_key(ch('=')), sent(0x0a, 1));
    assert_eq!(app.handle_key(key(KeyCode::Enter)), sent(0x0a, 2));
    assert_eq!(app.handle_key(ch(' ')), sent(0x0a, 0));
    assert_eq!(app.state().system.brightness, 0);
    assert_eq!(app.modal(), Some(Modal::Settings { sel: 0 }), "stays open");
}

/// A click on Dim / Normal / Bright sets it.
#[test]
fn brightness_clicks_send_10_0a() {
    let mut app = open();
    for (value, want) in [(0, sent(0x0a, 0)), (2, sent(0x0a, 2)), (1, sent(0x0a, 1))] {
        let (_, hits) = render(&app, 120, 40);
        let hit = Hit::SettingOption { row: 0, value };
        assert_eq!(click(&mut app, &hits, hit), want, "{value}");
        assert_eq!(app.state().system.brightness, value);
    }
    let (_, hits) = render(&app, 120, 40);
    let hit = Hit::SettingOption { row: 0, value: 1 };
    assert!(click(&mut app, &hits, hit).is_empty(), "already Normal");
}

// ------------------------------------------------------- the two switches

/// Automatic standby (`10 0b`): → / + turn it on, ← / - off, Enter and
/// space flip it.
#[test]
fn auto_standby_keys_send_10_0b() {
    let mut app = open();
    app.handle_key(key(KeyCode::Down));
    assert!(!app.state().system.auto_standby);
    assert_eq!(app.handle_key(key(KeyCode::Right)), sent(0x0b, 1));
    assert!(app.state().system.auto_standby);
    assert!(app.handle_key(ch('+')).is_empty(), "already on");
    assert_eq!(app.handle_key(ch('-')), sent(0x0b, 0));
    assert!(app.handle_key(key(KeyCode::Left)).is_empty(), "already off");
    assert_eq!(app.handle_key(key(KeyCode::Enter)), sent(0x0b, 1));
    assert_eq!(app.handle_key(ch(' ')), sent(0x0b, 0));
    assert!(!app.state().system.auto_standby);
    assert!(!app.restart_required(), "standby needs no restart");
}

/// Mobile applications (`10 0c`): the same keys.
#[test]
fn mobile_apps_keys_send_10_0c() {
    let mut app = open();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert!(!app.restart_required());
    assert_eq!(app.handle_key(key(KeyCode::Enter)), sent(0x0c, 1));
    assert!(app.state().system.mobile_apps);
    assert!(app.handle_key(key(KeyCode::Right)).is_empty(), "already on");
    assert_eq!(app.handle_key(key(KeyCode::Left)), sent(0x0c, 0));
    assert_eq!(app.handle_key(ch('+')), sent(0x0c, 1));
    assert_eq!(app.handle_key(ch(' ')), sent(0x0c, 0));
    assert!(!app.state().system.mobile_apps);
}

/// A click on Off / On sets each switch.
#[test]
fn switch_clicks_send_10_0b_and_10_0c() {
    let mut app = open();
    for (row, param) in [(1, 0x0b), (2, 0x0c)] {
        let (_, hits) = render(&app, 120, 40);
        let on = Hit::SettingOption { row, value: 1 };
        assert_eq!(click(&mut app, &hits, on), sent(param, 1), "row {row}");
        assert_eq!(app.modal(), Some(Modal::Settings { sel: row }));
        let (_, hits) = render(&app, 120, 40);
        assert!(
            click(&mut app, &hits, on).is_empty(),
            "row {row} already on"
        );
        let off = Hit::SettingOption { row, value: 0 };
        assert_eq!(click(&mut app, &hits, off), sent(param, 0), "row {row}");
    }
    assert_eq!(app.state().system, State::default().system);
}

/// What the keys send reaches the device as those frames, and nothing else.
#[test]
fn frames_reach_the_device() {
    let dev = Device::new();
    let mut app = open();
    dev.send(&app.handle_key(key(KeyCode::Left)));
    app.handle_key(key(KeyCode::Down));
    dev.send(&app.handle_key(key(KeyCode::Enter)));
    app.handle_key(key(KeyCode::Down));
    dev.send(&app.handle_key(key(KeyCode::Enter)));
    assert_eq!(
        dev.written(),
        vec![frame(0x0a, 0), frame(0x0b, 1), frame(0x0c, 1)]
    );
}

/// A device report of a setting updates the dialog but asks for no
/// restart: only the user's change does.
#[test]
fn report_updates_without_notice() {
    let mut app = open();
    app.on_report(&frame(0x0c, 1));
    assert!(app.state().system.mobile_apps);
    assert!(!app.restart_required());
}

// ------------------------------------------------------------ the notice

/// Changing mobile apps says it takes effect after the device restarts;
/// there is no notice before the change.
#[test]
fn mobile_apps_change_shows_restart_notice() {
    let mut app = open();
    let (buf, _) = render(&app, 120, 40);
    assert!(
        !text(&buf).to_lowercase().contains("restart"),
        "{}",
        text(&buf)
    );
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Enter));
    assert!(app.restart_required());
    let (buf, _) = render(&app, 120, 40);
    assert_snapshot("settings_notice_120x40", &dialog_text(&buf));
    // Closed and opened again, it still says so: the device has not
    // restarted.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(ch('e'));
    let (buf, _) = render(&app, 120, 40);
    assert!(text(&buf).contains("Restart the device"), "{}", text(&buf));
}

// ------------------------------------------------------------ the render

/// The dialog with the versions known: model, hardware, firmware, app.
#[test]
fn settings_snapshot() {
    let mut state = State::default();
    state.system.auto_standby = true;
    state.readout.hardware_version = Some(Version { major: 1, minor: 0 });
    state.readout.firmware_version = Some(Version { major: 1, minor: 7 });
    let mut app = App::new(Theme::color());
    app.synced(state);
    app.handle_key(ch('e'));
    let (buf, _) = render(&app, 120, 40);
    let t = text(&buf);
    assert!(t.contains(env!("CARGO_PKG_VERSION")), "{t}");
    assert_snapshot("settings_120x40", &dialog_text(&buf));
    // It fits the smallest screen too.
    let (buf, _) = render(&app, 100, 34);
    assert_snapshot("settings_100x34", &dialog_text(&buf));
}

/// Versions the device has not reported show "—".
#[test]
fn unknown_versions_show_dash() {
    let app = open();
    let (buf, _) = render(&app, 120, 40);
    let t = text(&buf);
    for label in ["Hardware", "Firmware"] {
        let line = t
            .lines()
            .find(|l| l.contains(label))
            .unwrap_or_else(|| panic!("{label} missing:\n{t}"));
        assert!(line.contains(&format!("{label}  ")), "{line}");
        let after = &line[line.find(label).unwrap() + label.len()..];
        assert_eq!(after.split_whitespace().next(), Some("—"), "{line}");
    }
    assert!(t.contains("E2x2"), "{t}");
}

/// The focused setting is marked, in the controls' accent, not in a
/// meter colour.
#[test]
fn focused_setting_uses_accent() {
    let app = open();
    let (buf, hits) = render(&app, 120, 40);
    let r = hits.rect(Hit::SettingOption { row: 0, value: 1 }).unwrap();
    let cell = &buf[(r.x + 1, r.y)];
    let meters = [
        Theme::METER_RED,
        Theme::METER_AMBER,
        Theme::METER_GREEN,
        Theme::WARN,
    ];
    for c in [cell.fg, cell.bg] {
        assert!(!meters.contains(&c), "{c:?}");
    }
    assert!(
        cell.fg == Theme::ACCENT || cell.bg == Theme::ACCENT,
        "{cell:?}"
    );
}
