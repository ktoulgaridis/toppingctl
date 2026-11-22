//! The redesigned TUI (#34, replacing the #13 layout), tested on the pure
//! `App`, on ratatui's `TestBackend`, and through a `Connection` over
//! `FakeTransport` for what reaches the device. No terminal, no hardware.
//!
//! Golden snapshots live in `tests/snapshots/` as plain text. Run with
//! `UPDATE_SNAPSHOTS=1` to rewrite them after an intended change.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use toppingctl::conn::fake::{FakeHandle, FakeTransport};
use toppingctl::conn::Connection;
use toppingctl::frame::{Dir, Frame};
use toppingctl::state::{Meters, Source, State, Version};
use toppingctl::tui::app::{Action, App, Control, InputControl, Modal, Panel};
use toppingctl::tui::run::{process_input, send_actions};
use toppingctl::tui::theme::Theme;
use toppingctl::tui::ui::{self, Hit, HitMap};

fn frame(group: u8, param: u8, value: i32) -> Frame {
    Frame {
        group,
        param,
        value,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn input(ch: usize, kind: InputControl) -> Control {
    Control::Input { ch, kind }
}

/// Renders `app` at `w`×`h` and returns the buffer and the hit map.
fn render(app: &App, meters: &Meters, w: u16, h: u16) -> (Buffer, HitMap) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    terminal.draw(|f| hits = ui::draw(f, app, meters)).unwrap();
    (terminal.backend().buffer().clone(), hits)
}

/// The buffer as text, one line per row, trailing spaces trimmed. Every
/// glyph the TUI draws is one cell wide.
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

/// A state with something to show in every panel.
fn fake_state() -> State {
    let mut s = State::default();
    s.inputs[0].gain_db = 12;
    s.inputs[0].mon = true;
    s.inputs[0].phantom = Some(false);
    s.inputs[1].gain_db = 4;
    s.inputs[1].inst = true;
    s.inputs[1].mute = true;
    s.inputs[1].phantom = Some(true);
    let a = &mut s.mixes[0];
    a.in1.fader_db = -3.0;
    a.in1.pan = 0.3;
    a.in2.pan = 0.7;
    a.in2.mute = true;
    a.pb34.fader_db = -12.0;
    a.pb34.pan_l = 0.2;
    a.pb34.pan_r = 0.8;
    a.pb34.solo = true;
    a.pb56.fader_db = f64::NEG_INFINITY;
    a.pb56.link = false;
    s.loopbacks[1].set_level(-6.0);
    s.output.source = Source::MixA;
    s.readout.volume_knob = Some(0x49);
    s.readout.hardware_version = Some(Version {
        major: 1,
        minor: 14,
    });
    s.readout.firmware_version = Some(Version {
        major: 1,
        minor: 13,
    });
    s
}

/// Frozen meters.
fn fake_meters() -> Meters {
    Meters {
        inputs: [-18.4, -4.0],
        playback: [[-8.0, -9.5], [-22.0, -23.5], [-96.0, -96.0]],
        output: [-9.0, -10.5],
        loopbacks: [[-22.0, -23.0], [-96.0, -96.0]],
    }
}

fn synced_app(theme: Theme) -> App {
    let mut app = App::new(theme);
    app.synced(fake_state());
    app
}

fn sends(actions: &[Action]) -> Vec<Frame> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Send(f) => Some(*f),
            _ => None,
        })
        .collect()
}

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.txt"))
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = snapshot_path(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
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

const PANELS: [&str; 4] = ["input", "mixer", "loopback", "output"];

#[test]
fn splash_until_synced() {
    let meters = fake_meters();
    let mut app = App::new(Theme::color());
    let (buf, hits) = render(&app, &meters, 120, 40);
    let splash = text(&buf);
    assert!(splash.contains("Connecting"), "splash:\n{splash}");
    for panel in PANELS.iter().chain(&["MON", "48V"]) {
        assert!(!splash.contains(panel), "{panel} on the splash:\n{splash}");
    }
    assert!(hits.is_empty(), "no interactive widgets while connecting");
    for k in [
        key(KeyCode::Right),
        ch(' '),
        ch('0'),
        key(KeyCode::Tab),
        ch('b'),
    ] {
        assert!(app.handle_key(k).is_empty());
    }

    app.synced(fake_state());
    let (buf, hits) = render(&app, &meters, 120, 40);
    let full = text(&buf);
    assert!(!full.contains("Connecting"), "full frame:\n{full}");
    for label in PANELS.iter().chain(&[
        "IN1",
        "IN2",
        "IN 1+2",
        "PB 1/2",
        "PB 3/4",
        "PB 5/6",
        "LOOP 1+2",
        "LOOP 3+4",
        "OUT 1+2",
        "MONITOR MIX",
        "PHONE",
        "MON",
        "48V",
        "INST",
        "SOLO",
        "MUTE",
        "LINK",
        "HP",
        "LINE",
    ]) {
        assert!(full.contains(label), "{label} missing:\n{full}");
    }
    assert!(!hits.is_empty());
}

/// The golden renders of the redesigned screen at the design's two sizes
/// (docs/design/render-*.txt), the "too small" screen, and the modals.
#[test]
fn snapshots() {
    let app = synced_app(Theme::color());
    let meters = fake_meters();
    let (buf, _) = render(&app, &meters, 120, 40);
    assert_snapshot("redesign_120x40", &text(&buf));
    let (buf, _) = render(&app, &meters, 100, 34);
    assert_snapshot("redesign_100x34", &text(&buf));

    // Below 100×34: "too small", nothing to click.
    let (buf, hits) = render(&app, &meters, 99, 34);
    assert_snapshot("too_small", &text(&buf));
    assert!(hits.is_empty(), "nothing to click when too small");
    let (buf, hits) = render(&app, &meters, 100, 33);
    assert!(text(&buf).contains("too small"));
    assert!(hits.is_empty());

    let mut splash = App::new(Theme::color());
    let (buf, _) = render(&splash, &meters, 100, 34);
    assert_snapshot("splash_100x34", &text(&buf));
    splash.needs_initial_state(State::default());
    let (buf, _) = render(&splash, &meters, 100, 34);
    assert_snapshot("push_defaults_100x34", &text(&buf));

    let mut modal = synced_app(Theme::color());
    modal.handle_key(key(KeyCode::Down));
    assert_eq!(modal.focus(), input(0, InputControl::Phantom));
    modal.handle_key(ch(' '));
    let (buf, _) = render(&modal, &meters, 100, 34);
    assert_snapshot("phantom_100x34", &text(&buf));

    let mut help = synced_app(Theme::color());
    help.handle_key(ch('?'));
    assert_eq!(help.modal(), Some(Modal::Help));
    let (buf, _) = render(&help, &meters, 100, 34);
    assert_snapshot("help_100x34", &text(&buf));
    help.handle_key(key(KeyCode::Esc));
    assert_eq!(help.modal(), None);
}

/// The four panels sit 1:1 like the app: input and mixer on top, loopback
/// and output below; the strips in the app's order.
#[test]
fn layout_matches_app() {
    let app = synced_app(Theme::color());
    for (w, h) in [(120, 40), (100, 34)] {
        let (buf, _) = render(&app, &fake_meters(), w, h);
        let t = text(&buf);
        let lines: Vec<&str> = t.lines().collect();
        let find = |s: &str| {
            lines
                .iter()
                .enumerate()
                .find_map(|(y, l)| l.find(s).map(|x| (l[..x].chars().count(), y)))
                .unwrap_or_else(|| panic!("{s} missing at {w}x{h}:\n{t}"))
        };
        let (ix, iy) = find("input");
        let (mx, my) = find("mixer");
        let (lx, ly) = find("loopback");
        let (ox, oy) = find("output");
        assert!(iy == my && ly == oy && iy < ly, "{w}x{h}:\n{t}");
        assert!(ix < mx && lx < ox && ix == lx, "{w}x{h}:\n{t}");
        let order = |names: &[&str]| {
            let pos: Vec<(usize, usize)> = names.iter().map(|n| find(n)).collect();
            assert!(
                pos.windows(2).all(|p| p[0].1 == p[1].1 && p[0].0 < p[1].0),
                "{names:?} out of order at {w}x{h}:\n{t}"
            );
        };
        order(&["IN1", "IN2"]);
        order(&["IN 1+2", "PB 1/2", "PB 3/4", "PB 5/6"]);
        order(&["LOOP 1+2", "LOOP 3+4", "OUT 1+2", "MONITOR MIX", "PHONE"]);
        // The mix tabs, A boxed (active).
        assert!(t.contains("┤a mix A├"), "{t}");
    }
}

/// The header: compact below 128 columns, full above; s / l / w and the
/// sync dot in both; the sample rate is shown, not set.
#[test]
fn header_compact_and_full() {
    let app = synced_app(Theme::color());
    let (buf, _) = render(&app, &fake_meters(), 120, 40);
    let head: String = text(&buf).lines().take(4).collect::<Vec<_>>().join("\n");
    for s in [
        "E2x2",
        "fw 1.13",
        "▾",
        "s save",
        "l load",
        "w store",
        "● synced",
    ] {
        assert!(head.contains(s), "{s} missing:\n{head}");
    }
    assert!(!head.contains("store to device"), "compact:\n{head}");

    let (buf, _) = render(&app, &fake_meters(), 140, 40);
    let head: String = text(&buf).lines().take(4).collect::<Vec<_>>().join("\n");
    for s in [
        "workspace",
        "sample rate",
        "s save workspace",
        "l load",
        "w store to device",
        "● synced",
    ] {
        assert!(head.contains(s), "{s} missing:\n{head}");
    }
    // The sync dot is green; the shortcut keys are the accent.
    let dot = (0..140u16)
        .flat_map(|x| (0..4u16).map(move |y| (x, y)))
        .find(|&p| buf[p].symbol() == "●")
        .unwrap();
    assert_eq!(buf[dot].fg, Theme::OK);
}

/// Mix tabs: A green, B blue, C amber; `a b c` and a click switch the
/// boxed (active) one, and the mixer shows that mix.
#[test]
fn mix_tabs() {
    let mut app = synced_app(Theme::color());
    let meters = fake_meters();
    let (buf, hits) = render(&app, &meters, 120, 40);
    for m in 0..3 {
        let r = hits.rect(Hit::MixTab(m)).expect("a tab");
        let key_cell = &buf[(r.x, r.y)];
        assert_eq!(key_cell.fg, Theme::MIX[m], "tab {m} colour");
    }
    assert_eq!(app.mix(), 0);
    assert!(text(&buf).contains("−12.0 dB"), "mix A's PB 3/4 fader");

    assert!(app.handle_key(ch('b')).is_empty(), "no frames");
    assert_eq!(app.mix(), 1);
    let (buf, hits) = render(&app, &meters, 120, 40);
    let t = text(&buf);
    assert!(t.contains("┤b mix B├") && !t.contains("┤a mix A├"), "{t}");
    assert!(!t.contains("−12.0 dB"), "mix B is at defaults:\n{t}");

    let c = hits.rect(Hit::MixTab(2)).unwrap();
    assert!(click_at(&mut app, &hits, c, Instant::now()).is_empty());
    assert_eq!(app.mix(), 2);
}

/// Pills: 48V and MUTE light red, HP / LINE the accent, IN 1+2 LINK is
/// greyed out while off.
#[test]
fn pill_colours() {
    let app = synced_app(Theme::color());
    let (buf, hits) = render(&app, &fake_meters(), 120, 40);
    let bg_at = |r: Rect| buf[(r.x, r.y)].bg;
    let phantom2 = hits
        .rect(Hit::Control(input(1, InputControl::Phantom)))
        .unwrap();
    assert_eq!(bg_at(phantom2), Theme::WARN, "48V on");
    let mute2 = hits
        .rect(Hit::Control(input(1, InputControl::Mute)))
        .unwrap();
    assert_eq!(bg_at(mute2), Theme::WARN, "MUTE on");
    let mon1 = hits
        .rect(Hit::Control(input(0, InputControl::Mon)))
        .unwrap();
    assert_eq!(bg_at(mon1), Theme::ACCENT, "MON on");
    let phantom1 = hits
        .rect(Hit::Control(input(0, InputControl::Phantom)))
        .unwrap();
    assert_eq!(bg_at(phantom1), Theme::PILL_OFF_BG, "48V off");

    let find_all = |s: &str| -> Vec<(u16, u16)> {
        let mut out = Vec::new();
        for y in 0..40u16 {
            let line: String = (0..120u16).map(|x| buf[(x, y)].symbol()).collect();
            let mut from = 0;
            while let Some(i) = line[from..].find(s) {
                let x = line[..from + i].chars().count() as u16;
                out.push((x, y));
                from += i + s.len();
            }
        }
        out
    };
    let hp = find_all(" HP ");
    assert_eq!(hp.len(), 1);
    assert_eq!(buf[hp[0]].bg, Theme::ACCENT);
    let line = find_all(" LINE ");
    assert_eq!(buf[line[0]].bg, Theme::ACCENT);
    // The first LINK is IN 1+2's (off): grey text, no pill background.
    let link = find_all(" LINK ");
    assert!(link.len() >= 4);
    let in12 = link.iter().min_by_key(|(x, y)| (*y, *x)).unwrap();
    assert_eq!(buf[(in12.0 + 1, in12.1)].fg, Theme::TRACK);
    assert_eq!(buf[(in12.0 + 1, in12.1)].bg, Color::Reset);
}

/// The clip lights hold about a second after a clip (fake clock).
#[test]
fn clip_lights_hold() {
    let mut app = synced_app(Theme::color());
    let quiet = fake_meters();
    let mut hot = fake_meters();
    hot.inputs[0] = 0.0;
    hot.playback[0][1] = 0.0;
    let t0 = Instant::now();
    let lamps = |app: &App, m: &Meters| text(&render(app, m, 120, 40).0).matches('▀').count();
    app.tick(&quiet, t0);
    let none = lamps(&app, &quiet);
    app.tick(&hot, t0);
    let lit = lamps(&app, &hot);
    assert!(lit >= none + 2, "IN1 and PB2 lamps light: {none} → {lit}");
    app.tick(&quiet, t0 + Duration::from_millis(900));
    assert_eq!(lamps(&app, &quiet), lit, "held");
    app.tick(&quiet, t0 + Duration::from_millis(1100));
    assert_eq!(lamps(&app, &quiet), none, "released");
}

// ------------------------------------------------- input panel, end to end

/// A connection over a fake transport; feed it the app's actions.
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

    /// Everything written, after flushing.
    fn written(mut self) -> Vec<Frame> {
        drop(self.conn.take());
        self.fake
            .written()
            .iter()
            .map(|r| Frame::decode(r, Dir::ToDevice).unwrap())
            .collect()
    }
}

/// Every key-driven input control writes its frame through the real
/// connection; 48V asks first and goes out on the confirmed path.
#[test]
fn input_keys_send_frames() {
    use InputControl::*;
    let mut app = synced_app(Theme::color());
    let dev = Device::new();
    let mut expect = Vec::new();
    let press = |app: &mut App, k: KeyEvent| {
        let a = app.handle_key(k);
        dev.send(&a);
        a
    };
    // By keys only: `1` for the input panel, → for IN2, ↓ down the column.
    let goto = |app: &mut App, c: usize, kind: InputControl| {
        press(app, ch('1'));
        if c == 1 {
            press(app, key(KeyCode::Right));
        }
        while app.focus() != input(c, kind) {
            assert!(press(app, key(KeyCode::Down)).is_empty());
        }
    };

    // ↓ walks IN1's column top to bottom and stops at the end.
    let order = [Mon, Phantom, Inst, Solo, Mute, Phase, Gain];
    assert_eq!(app.focus(), input(0, Mon));
    let mut seen = vec![app.focus()];
    for _ in 0..7 {
        press(&mut app, key(KeyCode::Down));
        seen.push(app.focus());
    }
    let want: Vec<Control> = order
        .iter()
        .map(|&k| input(0, k))
        .chain([input(0, Gain)])
        .collect();
    assert_eq!(seen, want);
    press(&mut app, ch('1'));

    // ↑↓ move along the column; ←→ on a switch cross to the other input.
    press(&mut app, key(KeyCode::Down));
    assert_eq!(app.focus(), input(0, Phantom));
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.focus(), input(1, Phantom));
    press(&mut app, key(KeyCode::Left));
    press(&mut app, key(KeyCode::Up));
    assert_eq!(app.focus(), input(0, Mon));
    press(&mut app, key(KeyCode::Up));
    assert_eq!(app.focus(), input(0, Mon), "clamped at the top");

    // Space flips MON and INST.
    let a = press(&mut app, ch(' '));
    assert!(!app.state().inputs[0].mon);
    assert_eq!(sends(&a), [frame(0x21, 0x01, 0)]);
    assert_eq!(a.last(), Some(&Action::Save));
    expect.push(frame(0x21, 0x01, 0));
    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Down));
    assert_eq!(app.focus(), input(0, Inst));
    press(&mut app, key(KeyCode::Enter));
    assert!(app.state().inputs[0].inst);
    expect.push(frame(0x21, 0x03, 1));

    // Unmute IN2 (it starts muted), so IN1's SOLO has something to silence.
    goto(&mut app, 1, Mute);
    press(&mut app, ch(' '));
    assert!(!app.state().inputs[1].mute);
    expect.push(frame(0x22, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[4]));
    // Back on IN1: SOLO.
    goto(&mut app, 0, Solo);
    press(&mut app, ch(' '));
    assert!(app.state().inputs[0].solo);
    expect.push(frame(0x22, 0x04, 0));
    // MUTE IN1.
    press(&mut app, key(KeyCode::Down));
    assert_eq!(app.focus(), input(0, Mute));
    press(&mut app, ch(' '));
    expect.push(frame(0x21, 0x04, 0));
    press(&mut app, ch(' '));
    expect.push(frame(0x21, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[12]));
    // Ø IN1: negated level.
    press(&mut app, key(KeyCode::Down));
    assert_eq!(app.focus(), input(0, Phase));
    press(&mut app, ch(' '));
    assert!(app.state().inputs[0].phase);
    expect.push(frame(0x21, 0x04, -toppingctl::state::INPUT_GAIN_LEVELS[12]));

    // Gain: ←→ ±1 dB, Shift ×10, clamped to 0 ..= +20; 0 resets.
    press(&mut app, key(KeyCode::Down));
    assert_eq!(app.focus(), input(0, Gain));
    let gain_frame = |g: usize| frame(0x21, 0x04, -toppingctl::state::INPUT_GAIN_LEVELS[g]);
    press(&mut app, key(KeyCode::Right));
    assert_eq!(app.state().inputs[0].gain_db, 13);
    expect.push(gain_frame(13));
    press(&mut app, shift(KeyCode::Right));
    assert_eq!(app.state().inputs[0].gain_db, 20, "clamped");
    expect.push(gain_frame(20));
    assert!(
        press(&mut app, key(KeyCode::Right)).is_empty(),
        "no change at the top"
    );
    press(&mut app, key(KeyCode::Left));
    expect.push(gain_frame(19));
    press(&mut app, ch('0'));
    assert_eq!(app.state().inputs[0].gain_db, 0, "0 resets to the default");
    expect.push(gain_frame(0));
    assert!(
        press(&mut app, ch('0')).is_empty(),
        "already at the default"
    );
    assert!(
        press(&mut app, shift(KeyCode::Left)).is_empty(),
        "clamped at 0"
    );

    // 48V: asks first and sends nothing; n cancels; y sends on the
    // confirmed path; off goes straight out.
    goto(&mut app, 0, Phantom);
    assert!(press(&mut app, ch(' ')).is_empty());
    assert_eq!(app.modal(), Some(Modal::Phantom { ch: 0 }));
    for k in [key(KeyCode::Right), ch('0'), key(KeyCode::Tab), ch(' ')] {
        assert!(press(&mut app, k).is_empty(), "keys go to the modal");
    }
    assert!(press(&mut app, ch('n')).is_empty());
    assert_eq!(app.modal(), None);
    assert_eq!(app.state().inputs[0].phantom, Some(false));
    press(&mut app, ch(' '));
    let a = press(&mut app, ch('y'));
    assert_eq!(a, [Action::SendConfirmedPhantom(frame(0x21, 0x02, 1))]);
    assert_eq!(app.state().inputs[0].phantom, Some(true));
    expect.push(frame(0x21, 0x02, 1));
    let a = press(&mut app, ch(' '));
    assert_eq!(a, [Action::Send(frame(0x21, 0x02, 0))]);
    expect.push(frame(0x21, 0x02, 0));

    // Unknown 48V counts as off: it asks.
    let mut fresh = App::new(Theme::color());
    fresh.synced(State::default());
    fresh.handle_key(key(KeyCode::Down));
    assert!(fresh.handle_key(ch(' ')).is_empty());
    assert_eq!(fresh.modal(), Some(Modal::Phantom { ch: 0 }));

    // q and Ctrl-C quit.
    assert_eq!(app.handle_key(ch('q')), [Action::Quit]);
    assert_eq!(app.handle_key(ctrl('c')), [Action::Quit]);

    assert_eq!(dev.written(), expect);
}

/// A device report updates 48V; 48V and the volume knob are not persisted,
/// so they cause no save. A MON press does.
#[test]
fn reports_update_app() {
    let mut app = synced_app(Theme::color());
    assert!(app.on_report(&frame(0x22, 0x02, 0)).is_empty());
    assert_eq!(app.state().inputs[1].phantom, Some(false));
    assert!(app.on_report(&frame(0x11, 0x01, 0x30)).is_empty());
    assert_eq!(app.on_report(&frame(0x22, 0x01, 1)), [Action::Save]);
}

fn mouse(kind: MouseEventKind, x: u16, y: u16, modifiers: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers,
    }
}

fn click_at(app: &mut App, hits: &HitMap, r: Rect, now: Instant) -> Vec<Action> {
    let none = KeyModifiers::NONE;
    let down = MouseEventKind::Down(MouseButton::Left);
    let up = MouseEventKind::Up(MouseButton::Left);
    let mut actions = app.handle_mouse_at(mouse(down, r.x, r.y, none), hits, now);
    actions.extend(app.handle_mouse_at(mouse(up, r.x, r.y, none), hits, now));
    actions
}

/// Click, scroll, drag and double-click on the input panel, through the
/// real connection.
#[test]
fn input_mouse_sends_frames() {
    use InputControl::*;
    let meters = fake_meters();
    let mut app = synced_app(Theme::color());
    let dev = Device::new();
    let (_, hits) = render(&app, &meters, 100, 34);
    let rect = |c: Control| {
        hits.rect(Hit::Control(c))
            .unwrap_or_else(|| panic!("{c:?} not on screen"))
    };
    let none = KeyModifiers::NONE;
    let t0 = Instant::now();
    let ms = |n| t0 + Duration::from_millis(n);
    let mut expect = Vec::new();

    // Every pill is clickable: it focuses and flips.
    let a = click_at(&mut app, &hits, rect(input(1, Inst)), ms(0));
    dev.send(&a);
    assert_eq!(app.focus(), input(1, Inst));
    assert!(!app.state().inputs[1].inst);
    expect.push(frame(0x22, 0x03, 0));
    let a = click_at(&mut app, &hits, rect(input(0, Mon)), ms(1000));
    dev.send(&a);
    expect.push(frame(0x21, 0x01, 0));
    let a = click_at(&mut app, &hits, rect(input(1, Mute)), ms(2000));
    dev.send(&a);
    expect.push(frame(0x22, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[4]));
    let a = click_at(&mut app, &hits, rect(input(1, Phase)), ms(3000));
    dev.send(&a);
    expect.push(frame(0x22, 0x04, -toppingctl::state::INPUT_GAIN_LEVELS[4]));
    let a = click_at(&mut app, &hits, rect(input(1, Phase)), ms(4000));
    dev.send(&a);
    expect.push(frame(0x22, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[4]));
    let a = click_at(&mut app, &hits, rect(input(0, Solo)), ms(5000));
    dev.send(&a);
    assert!(app.state().inputs[0].solo);
    expect.push(frame(0x22, 0x04, 0));
    let a = click_at(&mut app, &hits, rect(input(0, Solo)), ms(6000));
    dev.send(&a);
    expect.push(frame(0x22, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[4]));

    // The wheel on a gain knob: ±1, Shift ×10, clamped.
    let g2 = rect(input(1, Gain));
    let wheel = |app: &mut App, up: bool, m: KeyModifiers| {
        let kind = if up {
            MouseEventKind::ScrollUp
        } else {
            MouseEventKind::ScrollDown
        };
        let a = app.handle_mouse_at(mouse(kind, g2.x, g2.y, m), &hits, ms(7000));
        dev.send(&a);
    };
    wheel(&mut app, true, none);
    assert_eq!(app.state().inputs[1].gain_db, 5);
    expect.push(frame(0x22, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[5]));
    wheel(&mut app, false, none);
    wheel(&mut app, false, none);
    assert_eq!(app.state().inputs[1].gain_db, 3);
    expect.push(frame(0x22, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[4]));
    expect.push(frame(0x22, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[3]));
    wheel(&mut app, true, KeyModifiers::SHIFT);
    wheel(&mut app, true, KeyModifiers::SHIFT);
    assert_eq!(app.state().inputs[1].gain_db, 20, "clamped");
    expect.push(frame(0x22, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[13]));
    expect.push(frame(0x22, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[20]));
    // The wheel on a switch does nothing.
    let mon = rect(input(0, Mon));
    assert!(app
        .handle_mouse_at(
            mouse(MouseEventKind::ScrollUp, mon.x, mon.y, none),
            &hits,
            ms(7000)
        )
        .is_empty());

    // Drag a knob vertically: up raises the gain, 1 dB per row, clamped;
    // it keeps working off the knob until the button comes up.
    let g1 = rect(input(0, Gain));
    let (x, y) = (g1.x + 1, g1.y + 1);
    let down = MouseEventKind::Down(MouseButton::Left);
    let drag = MouseEventKind::Drag(MouseButton::Left);
    let up = MouseEventKind::Up(MouseButton::Left);
    assert_eq!(app.state().inputs[0].gain_db, 12);
    let a = app.handle_mouse_at(mouse(down, x, y, none), &hits, ms(8000));
    assert!(sends(&a).is_empty(), "a press alone changes nothing");
    assert_eq!(app.focus(), input(0, Gain));
    let a = app.handle_mouse_at(mouse(drag, x, y - 3, none), &hits, ms(8100));
    dev.send(&a);
    assert_eq!(app.state().inputs[0].gain_db, 15);
    expect.push(frame(0x21, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[15]));
    let a = app.handle_mouse_at(mouse(drag, 0, 0, none), &hits, ms(8200));
    dev.send(&a);
    assert_eq!(app.state().inputs[0].gain_db, 20, "clamped, off the knob");
    expect.push(frame(0x21, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[20]));
    let a = app.handle_mouse_at(mouse(drag, x, y + 2, none), &hits, ms(8300));
    dev.send(&a);
    assert_eq!(app.state().inputs[0].gain_db, 10);
    expect.push(frame(0x21, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[10]));
    app.handle_mouse_at(mouse(up, x, y + 2, none), &hits, ms(8400));
    assert!(app
        .handle_mouse_at(mouse(drag, x, y - 5, none), &hits, ms(8500))
        .is_empty());
    assert_eq!(app.state().inputs[0].gain_db, 10, "the drag ended");

    // Double-click a knob: back to its default (+0 dB). Two slow clicks
    // are not a double-click.
    let a = click_at(&mut app, &hits, g1, ms(10_000));
    assert!(sends(&a).is_empty());
    let a = click_at(&mut app, &hits, g1, ms(11_000));
    assert!(sends(&a).is_empty(), "too slow");
    let a = click_at(&mut app, &hits, g1, ms(11_200));
    dev.send(&a);
    assert_eq!(app.state().inputs[0].gain_db, 0, "double-click resets");
    expect.push(frame(0x21, 0x04, toppingctl::state::INPUT_GAIN_LEVELS[0]));

    // A click on 48V opens the confirm modal and sends nothing; clicks
    // outside it do nothing; its buttons work.
    let a = click_at(&mut app, &hits, rect(input(0, Phantom)), ms(12_000));
    assert!(a.is_empty(), "{a:?}");
    assert_eq!(app.modal(), Some(Modal::Phantom { ch: 0 }));
    let (_, mhits) = render(&app, &meters, 100, 34);
    for c in [input(0, Phantom), input(0, Mon), input(1, Gain)] {
        if let Some(r) = mhits.rect(Hit::Control(c)) {
            assert!(click_at(&mut app, &mhits, r, ms(12_500)).is_empty());
        }
    }
    assert_eq!(app.modal(), Some(Modal::Phantom { ch: 0 }));
    let no = mhits.rect(Hit::No).unwrap();
    assert!(click_at(&mut app, &mhits, no, ms(13_000)).is_empty());
    assert_eq!(app.modal(), None);
    click_at(&mut app, &hits, rect(input(0, Phantom)), ms(14_000));
    let (_, mhits) = render(&app, &meters, 100, 34);
    let yes = mhits.rect(Hit::Yes).unwrap();
    let a = click_at(&mut app, &mhits, yes, ms(15_000));
    assert_eq!(a, [Action::SendConfirmedPhantom(frame(0x21, 0x02, 1))]);
    dev.send(&a);
    expect.push(frame(0x21, 0x02, 1));

    assert_eq!(dev.written(), expect);
}

/// The focused control is drawn in the accent colour and bold.
#[test]
fn focus_is_accent_and_bold() {
    let mut app = synced_app(Theme::color());
    for _ in 0..6 {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(app.focus(), input(0, InputControl::Gain));
    let (buf, hits) = render(&app, &fake_meters(), 100, 34);
    let r = hits
        .rect(Hit::Control(input(0, InputControl::Gain)))
        .unwrap();
    let cells: Vec<_> = (r.x..r.right())
        .flat_map(|x| (r.y..r.bottom()).map(move |y| (x, y)))
        .map(|p| &buf[p])
        .filter(|c| c.symbol() != " ")
        .collect();
    assert!(cells
        .iter()
        .any(|c| c.fg == Theme::ACCENT && c.modifier.contains(Modifier::BOLD)));
}

#[test]
fn no_color_respected() {
    use std::ffi::OsStr;
    assert!(Theme::detect(None).is_color());
    assert!(
        Theme::detect(Some(OsStr::new(""))).is_color(),
        "empty NO_COLOR is unset"
    );
    assert!(!Theme::detect(Some(OsStr::new("1"))).is_color());

    let plain = Theme::detect(Some(OsStr::new("1")));
    let meters = fake_meters();
    let mut apps = vec![synced_app(plain), App::new(plain)];
    let mut modal = synced_app(plain);
    modal.handle_key(ch('?'));
    apps.push(modal);
    let mut first = App::new(plain);
    first.needs_initial_state(State::default());
    apps.push(first);
    for app in &apps {
        for (w, h) in [(140, 40), (120, 40), (100, 34), (60, 20)] {
            let (buf, _) = render(app, &meters, w, h);
            for y in 0..h {
                for x in 0..w {
                    let c = &buf[(x, y)];
                    assert_eq!(
                        (c.fg, c.bg),
                        (Color::Reset, Color::Reset),
                        "colour at {x},{y} ({w}x{h}) under NO_COLOR"
                    );
                }
            }
        }
    }

    // Same text as the colour render, so the snapshots hold for both.
    let (coloured, _) = render(&synced_app(Theme::color()), &meters, 120, 40);
    let (uncoloured, _) = render(&synced_app(plain), &meters, 120, 40);
    assert_eq!(text(&coloured), text(&uncoloured));

    // Focus is still visible without colour (bold), and so is a lit pill.
    let (_, hits) = render(&synced_app(plain), &meters, 120, 40);
    let r = hits
        .rect(Hit::Control(input(0, InputControl::Mon)))
        .unwrap();
    assert!((r.x..r.right()).any(|x| uncoloured[(x, r.y)].modifier.contains(Modifier::BOLD)));
    assert!((r.x..r.right()).any(|x| uncoloured[(x, r.y)].modifier.contains(Modifier::REVERSED)));
}

/// Leaving the TUI turns mouse capture off, leaves the alternate screen and
/// shows the cursor (the panic hook and the normal exit both use it).
#[test]
fn terminal_restore_sequence() {
    let mut out = Vec::new();
    toppingctl::tui::run::write_leave(&mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    for (what, seq) in [
        ("mouse capture off", "\x1b[?1000l"),
        ("SGR mouse off", "\x1b[?1006l"),
        ("alternate screen off", "\x1b[?1049l"),
        ("cursor shown", "\x1b[?25h"),
    ] {
        assert!(out.contains(seq), "{what}: {out:?}");
    }

    let mut out = Vec::new();
    toppingctl::tui::run::write_enter(&mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("\x1b[?1049h") && out.contains("\x1b[?1000h"),
        "{out:?}"
    );
}

/// The first run asks before pushing anything.
#[test]
fn first_run_asks_before_pushing() {
    let mut device = State::default();
    device.system.brightness = 0;
    let mut app = App::new(Theme::color());
    app.needs_initial_state(device.clone());
    assert_eq!(app.modal(), Some(Modal::PushDefaults));
    let (buf, hits) = render(&app, &fake_meters(), 100, 34);
    let shown = text(&buf);
    assert!(
        shown.contains("No saved state") && shown.contains("Push defaults to the device?"),
        "{shown}"
    );
    assert!(
        !shown.contains("mixer"),
        "no controls behind the modal:\n{shown}"
    );
    assert!(hits.rect(Hit::Yes).is_some() && hits.rect(Hit::No).is_some());
    for k in [
        ch('0'),
        ch(' '),
        key(KeyCode::Right),
        key(KeyCode::Tab),
        key(KeyCode::Enter),
    ] {
        assert!(app.handle_key(k).is_empty());
    }
    assert!(app.on_report(&frame(0x21, 0x01, 1)).is_empty());
    app.on_report(&frame(0x21, 0x01, 0));

    let actions = app.handle_key(ch('y'));
    let expected: Vec<Frame> = device
        .frames()
        .into_iter()
        .filter(|f| f.group != 0x10)
        .collect();
    assert_eq!(sends(&actions), expected);
    assert_eq!(actions.last(), Some(&Action::Save));
    assert_eq!(app.modal(), None);
    let (buf, _) = render(&app, &fake_meters(), 100, 34);
    assert!(text(&buf).contains("mixer"));

    let mut app = App::new(Theme::color());
    app.needs_initial_state(device);
    assert_eq!(app.handle_key(ch('n')), [Action::Quit]);
}

// ------------------------------------------------------- meters, end to end

/// The device→host reports of a capture, as raw 16-byte reports.
fn captured_rx(path: &str) -> Vec<Vec<u8>> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| l.split_whitespace().nth(1) == Some("rx"))
        .map(|l| {
            l.split_whitespace()
                .skip(2)
                .take(16)
                .map(|b| u8::from_str_radix(b, 16).unwrap())
                .collect()
        })
        .collect()
}

/// Real captured meter reports (`captures/mac-3/meters-tone.txt`: IN1,
/// playback 1/2, output and loopback meters) go through the connection,
/// `sync::connect` and the TUI's drain into the meters, and the input strip
/// shows them.
#[test]
fn captured_meters_reach_the_screen() {
    use toppingctl::frame::Crc;
    use toppingctl::sync::{connect, Store};
    use toppingctl::tui::run::drain_device;

    let dir = std::env::temp_dir().join(format!("toppingctl-{}-meters", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let store = Store::at(dir.join("state.json"));
    store.save(&fake_state()).unwrap();

    let (transport, fake) = FakeTransport::new();
    for f in [
        frame(0x10, 0x06, 1),
        frame(0x10, 0x0b, 0),
        frame(0x10, 0x04, 0x0001_000d),
        frame(0x11, 0x01, 0x49),
        frame(0x10, 0x0a, 1),
        frame(0x10, 0x03, 0x0001_000e),
        frame(0x10, 0x0c, 0),
    ] {
        fake.push_report(&f.encode(Crc::Fill));
    }
    let rx = captured_rx("captures/mac-3/meters-tone.txt");
    assert!(rx.len() > 1000);
    for r in &rx {
        fake.push_report(r);
    }
    // A louder IN1 reading after the capture: −12.3 dBFS.
    fake.push_report(&frame(0x21, 0x05, -123).encode(Crc::Fill));

    let (conn, events) = Connection::spawn(transport);
    let mut app = App::new(Theme::color());
    app.synced(connect(&conn, &events, &store).unwrap());
    let mut meters = Meters::default();
    let deadline = Instant::now() + Duration::from_secs(20);
    while meters.inputs[0] != -12.3 && Instant::now() < deadline {
        drain_device(&events, &mut app, &mut meters).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(meters.inputs[0], -12.3, "IN1 meter frames reach the meters");
    assert!(meters.playback[0][0] > -60.0, "{:?}", meters.playback);
    assert!(meters.output[0] > -60.0, "{:?}", meters.output);

    app.tick(&meters, Instant::now());
    let (buf, _) = render(&app, &meters, 120, 40);
    let t = text(&buf);
    assert!(t.contains("−12 pk"), "the IN1 peak readout moves:\n{t}");
    // The IN1 meter is lit well up its height.
    let lines: Vec<&str> = t.lines().collect();
    let lit = lines[6..23]
        .iter()
        .filter(|l| l.chars().take(12).collect::<String>().contains("██"))
        .count();
    assert!(lit >= 8, "IN1 meter lit rows: {lit}\n{t}");
}

// ---------------------------------------------------------- panel focus

/// Tab and 1–4 move between the panels, ←→ between strips, and a click on a
/// strip focuses it; the focused strip and panel are drawn in the accent.
#[test]
fn panel_focus_reaches_every_strip() {
    let mut app = synced_app(Theme::color());
    let meters = fake_meters();
    assert_eq!(app.focus().panel(), Panel::Input);

    // Tab cycles input → mixer → loopback → output → input; Shift-Tab back.
    let mut seen = vec![app.focus()];
    for _ in 0..4 {
        assert!(app.handle_key(key(KeyCode::Tab)).is_empty());
        seen.push(app.focus());
    }
    assert_eq!(
        seen,
        [
            input(0, InputControl::Mon),
            Control::Mixer(0),
            Control::Loopback(0),
            Control::Output(0),
            input(0, InputControl::Mon),
        ]
    );
    app.handle_key(key(KeyCode::BackTab));
    assert_eq!(app.focus(), Control::Output(0));

    // 1–4 jump to a panel.
    for (c, want) in [
        ('2', Control::Mixer(0)),
        ('3', Control::Loopback(0)),
        ('4', Control::Output(0)),
        ('1', input(0, InputControl::Mon)),
    ] {
        assert!(app.handle_key(ch(c)).is_empty());
        assert_eq!(app.focus(), want);
    }

    // ←→ walk the strips of a panel, clamped; nothing reaches the device.
    app.handle_key(ch('2'));
    for want in [1, 2, 3, 3] {
        assert!(app.handle_key(key(KeyCode::Right)).is_empty());
        assert_eq!(app.focus(), Control::Mixer(want));
    }
    for k in [ch(' '), ch('0'), key(KeyCode::Up), key(KeyCode::Enter)] {
        assert!(app.handle_key(k).is_empty(), "the mixer is not wired yet");
    }
    app.handle_key(ch('4'));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.focus(), Control::Output(2));

    // The focused strip's border and its panel's border are the accent.
    app.handle_key(ch('2'));
    app.handle_key(key(KeyCode::Right));
    let (buf, hits) = render(&app, &meters, 120, 40);
    let pb12 = hits.rect(Hit::Focus(Control::Mixer(1))).unwrap();
    assert_eq!(buf[(pb12.x, pb12.y)].fg, Theme::ACCENT, "PB 1/2 border");
    let pb34 = hits.rect(Hit::Focus(Control::Mixer(2))).unwrap();
    assert_ne!(buf[(pb34.x, pb34.y)].fg, Theme::ACCENT);
    let top_left_of_mixer = (0..120u16)
        .find(|&x| buf[(x, 4)].symbol() == "╭" && x > 0)
        .unwrap();
    assert_eq!(
        buf[(top_left_of_mixer, 4)].fg,
        Theme::ACCENT,
        "mixer panel border"
    );

    // Every strip has a click target that focuses it.
    let none = KeyModifiers::NONE;
    let down = MouseEventKind::Down(MouseButton::Left);
    let mut targets = vec![];
    for i in 0..4 {
        targets.push(Control::Mixer(i));
    }
    for i in 0..2 {
        targets.push(Control::Loopback(i));
    }
    for i in 0..3 {
        targets.push(Control::Output(i));
    }
    for c in targets {
        let r = hits.rect(Hit::Focus(c)).unwrap_or_else(|| panic!("{c:?}"));
        let a = app.handle_mouse(mouse(down, r.x + 2, r.bottom() - 2, none), &hits);
        assert!(a.is_empty());
        assert_eq!(app.focus(), c);
    }
    // A click on an empty spot of an input strip focuses that input, and
    // flips nothing.
    let in2 = hits.rect(Hit::Focus(input(1, InputControl::Mon))).unwrap();
    let a = app.handle_mouse(mouse(down, in2.x + 1, in2.y, none), &hits);
    assert!(a.is_empty());
    assert_eq!(app.focus(), input(1, InputControl::Mon));

    // The footer and help say how.
    let t = text(&buf);
    assert!(t.contains("tab 1-4 panel"), "{t}");
    app.handle_key(ch('?'));
    let (buf, _) = render(&app, &meters, 120, 40);
    let t = text(&buf);
    assert!(t.contains("next / previous panel") && t.contains("input, mixer, loopback, output"));
}

/// Real crossterm events, through the same function the event loop uses:
/// a flood of pointer motion (crossterm's capture reports every move) does
/// not hold up the click behind it, and wheel, drag and tab clicks work.
#[test]
fn mouse_events_through_the_event_loop() {
    use crossterm::event::Event;
    let meters = fake_meters();
    let mut app = synced_app(Theme::color());
    let (_, hits) = render(&app, &meters, 120, 40);
    let none = KeyModifiers::NONE;
    let ev = |kind, r: Rect| Event::Mouse(mouse(kind, r.x, r.y, none));
    let inst2 = hits
        .rect(Hit::Control(input(1, InputControl::Inst)))
        .unwrap();

    let mut queue: Vec<Event> = (0..2000)
        .map(|i| Event::Mouse(mouse(MouseEventKind::Moved, (i % 100) as u16, 10, none)))
        .collect();
    queue.push(ev(MouseEventKind::Down(MouseButton::Left), inst2));
    queue.push(ev(MouseEventKind::Up(MouseButton::Left), inst2));
    let mut it = queue.into_iter();
    let a = process_input(&mut app, &mut it, &hits);
    assert_eq!(sends(&a), [frame(0x22, 0x03, 0)], "the click, in one pass");
    assert!(!app.state().inputs[1].inst);
    // The release is next; then the queue is empty.
    assert!(process_input(&mut app, &mut it, &hits).is_empty());
    assert!(it.next().is_none());

    // Wheel and drag on a knob.
    let g1 = hits
        .rect(Hit::Control(input(0, InputControl::Gain)))
        .unwrap();
    let a = process_input(
        &mut app,
        &mut [ev(MouseEventKind::ScrollUp, g1)].into_iter(),
        &hits,
    );
    assert_eq!(app.state().inputs[0].gain_db, 13);
    assert_eq!(sends(&a).len(), 1);
    let at = |x: u16, y: u16, kind| Event::Mouse(mouse(kind, x, y, none));
    let (x, y) = (g1.x + 2, g1.y + 2);
    let drag = [
        at(x, y, MouseEventKind::Down(MouseButton::Left)),
        at(x, y - 1, MouseEventKind::Moved),
        at(x, y - 4, MouseEventKind::Drag(MouseButton::Left)),
        at(x, y - 4, MouseEventKind::Up(MouseButton::Left)),
    ]
    .into_iter();
    for e in drag {
        process_input(&mut app, &mut std::iter::once(e), &hits);
    }
    assert_eq!(app.state().inputs[0].gain_db, 17);

    // A click on a mix tab and on a strip.
    let tab = hits.rect(Hit::MixTab(1)).unwrap();
    process_input(
        &mut app,
        &mut [ev(MouseEventKind::Down(MouseButton::Left), tab)].into_iter(),
        &hits,
    );
    assert_eq!(app.mix(), 1);
    let lb = hits.rect(Hit::Focus(Control::Loopback(1))).unwrap();
    process_input(
        &mut app,
        &mut [ev(MouseEventKind::Down(MouseButton::Left), lb)].into_iter(),
        &hits,
    );
    assert_eq!(app.focus(), Control::Loopback(1));

    // Keys go through the same path.
    let a = process_input(&mut app, &mut [Event::Key(ch('1'))].into_iter(), &hits);
    assert!(a.is_empty());
    assert_eq!(app.focus(), input(0, InputControl::Mon));
}

/// 0 dB reads −0, as in the official app: fader values and the gain.
#[test]
fn zero_db_reads_minus_zero() {
    assert_eq!(ui::gain_text(0), "−0 dB");
    assert_eq!(ui::gain_text(12), "+12 dB");
    let mut app = synced_app(Theme::color());
    let mut s = fake_state();
    s.inputs[1].gain_db = 0;
    app.synced(s);
    let (buf, _) = render(&app, &fake_meters(), 120, 40);
    let t = text(&buf);
    assert!(t.contains("−0 dB"), "IN2 gain:\n{t}");
    assert!(t.contains("−0.0 dB"), "a 0 dB fader:\n{t}");
    assert!(!t.contains(" 0.0 dB"), "{t}");
}
