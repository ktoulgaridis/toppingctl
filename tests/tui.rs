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
    s.readout.monitor_mix = Some(0x49);
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
        if m == app.mix() {
            // The active tab: its key and name on one chip of its colour.
            for x in r.left()..r.right() {
                assert_eq!(buf[(x, r.y)].bg, Theme::MIX[m], "active tab cell {x}");
            }
        } else {
            // The others: key and name in their colour, as in the mockup.
            assert_eq!(key_cell.fg, Theme::MIX[m], "tab {m} key colour");
            assert_eq!(buf[(r.x + 2, r.y)].fg, Theme::MIX[m], "tab {m} name colour");
        }
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
    // The first LINK is IN 1+2's (off): a pill like the others, its text
    // greyed out.
    let link = find_all(" LINK ");
    assert!(link.len() >= 4);
    let in12 = link.iter().min_by_key(|(x, y)| (*y, *x)).unwrap();
    assert_eq!(buf[(in12.0 + 1, in12.1)].fg, Theme::SCALE);
    assert_eq!(buf[(in12.0 + 1, in12.1)].bg, Theme::PILL_OFF_BG);
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

// ---------------------------------------------------------- pixel widgets

fn kitty_layer() -> toppingctl::tui::pixel::PixelLayer {
    use ratatui_image::picker::{Picker, ProtocolType};
    // A kitty picker with a fixed cell size (10×20), without asking a
    // terminal.
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(ProtocolType::Kitty);
    toppingctl::tui::pixel::PixelLayer::new(picker)
}

/// Draws with the pixel layer; returns the buffer, the hit map and the
/// kitty sequences the frame emits.
fn draw_pixels(
    app: &App,
    meters: &Meters,
    w: u16,
    h: u16,
    layer: &mut toppingctl::tui::pixel::PixelLayer,
) -> (Buffer, HitMap, String) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    terminal
        .draw(|f| hits = ui::draw_with(f, app, meters, Some(&mut *layer)))
        .unwrap();
    let out = layer.take_output();
    (terminal.backend().buffer().clone(), hits, out)
}

/// A placement in the emitted sequences: cell, size in cells, z.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placed {
    x: u16,
    y: u16,
    cols: u16,
    rows: u16,
    z: i32,
    image: u32,
}

/// The kitty commands in `out`: their control data (`a=…,i=…`), in order,
/// with the cursor position before each.
fn commands(out: &str) -> Vec<(Option<(u16, u16)>, String)> {
    let mut cmds = Vec::new();
    let mut pos = None;
    let mut rest = out;
    loop {
        let csi = rest.find("\x1b[");
        let apc = rest.find("\x1b_G");
        match (csi, apc) {
            (Some(c), a) if a.is_none_or(|a| c < a) => {
                let tail = &rest[c + 2..];
                let end = tail.find('H').expect("cursor move");
                let (y, x) = tail[..end].split_once(';').unwrap();
                pos = Some((x.parse::<u16>().unwrap() - 1, y.parse::<u16>().unwrap() - 1));
                rest = &tail[end + 1..];
            }
            (_, Some(a)) => {
                let tail = &rest[a + 3..];
                let end = tail.find("\x1b\\").expect("ST");
                let body = &tail[..end];
                let ctrl = body.split(';').next().unwrap().to_string();
                cmds.push((pos, ctrl));
                rest = &tail[end + 2..];
            }
            _ => return cmds,
        }
    }
}

fn field<'a>(ctrl: &'a str, k: &str) -> Option<&'a str> {
    ctrl.split(',')
        .find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
}

fn placements(out: &str) -> Vec<Placed> {
    commands(out)
        .into_iter()
        .filter(|(_, c)| field(c, "a") == Some("p"))
        .map(|(pos, c)| {
            let (x, y) = pos.expect("a placement is positioned");
            assert_eq!(field(&c, "C"), Some("1"), "the cursor stays: {c}");
            Placed {
                x,
                y,
                cols: field(&c, "c").unwrap().parse().unwrap(),
                rows: field(&c, "r").unwrap().parse().unwrap(),
                z: field(&c, "z").unwrap().parse().unwrap(),
                image: field(&c, "i").unwrap().parse().unwrap(),
            }
        })
        .collect()
}

fn count(out: &str, action: &str) -> usize {
    commands(out)
        .iter()
        .filter(|(_, c)| field(c, "a") == Some(action))
        .count()
}

fn kind(w: &toppingctl::tui::pixel::PixelWidget) -> &'static str {
    use toppingctl::tui::pixel::PixelWidget as W;
    match w {
        W::Knob { .. } => "knob",
        W::Fader { .. } => "fader",
        W::Meter { .. } => "meter",
        W::ClipLight { .. } => "clip",
        W::Chip { .. } => "chip",
    }
}

/// In kitty / Ghostty every fader, meter, clip light, pill, tab, chip and
/// knob is a kitty image placed directly in its cells, labels on top.
#[test]
fn pixel_widgets_in_kitty() {
    let app = synced_app(Theme::color());
    let meters = fake_meters();
    let mut layer = kitty_layer();
    let (buf, hits, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    let spots = hits.pixels();

    let n = |k: &str| spots.iter().filter(|s| kind(&s.widget) == k).count();
    // Two gain knobs, eight pans, Monitor Mix.
    assert_eq!(n("knob"), 11);
    // IN1, IN2 in IN 1+2, three PB strips, two loopbacks, the output.
    assert_eq!(n("fader"), 8);
    // Input 2 (2 wide), IN 1+2 2, PB 6, loopback 4, output 2; each has a
    // clip light.
    assert_eq!(n("meter"), 16);
    assert_eq!(n("clip"), 16);
    assert!(n("chip") >= 30, "pills, titles, the mix tab: {}", n("chip"));

    // Each widget is placed once, at its cells, with its size in cells.
    // Widgets with text on top (pills, tabs, chips, faders' scales) sit
    // under the text (z=-1).
    let placed = placements(&out);
    assert_eq!(placed.len(), spots.len());
    for s in spots {
        let p = placed
            .iter()
            .find(|p| (p.x, p.y) == (s.rect.x, s.rect.y))
            .unwrap_or_else(|| panic!("no placement for {s:?}"));
        assert_eq!((p.cols, p.rows), (s.rect.width, s.rect.height), "{s:?}");
        let z = match kind(&s.widget) {
            "chip" | "fader" => -1,
            _ => 0,
        };
        assert_eq!(p.z, z, "{s:?}");
    }
    // Each distinct image is sent once (zlib, RGBA), then placed by id.
    let sent: Vec<_> = commands(&out)
        .into_iter()
        .filter(|(_, c)| field(c, "a") == Some("t"))
        .collect();
    assert_eq!(sent.len(), layer.cached());
    assert_eq!(layer.built(), layer.cached());
    assert!(layer.cached() < spots.len(), "equal widgets share an image");
    for (_, c) in &sent {
        assert_eq!(field(c, "f"), Some("32"), "{c}");
        assert_eq!(field(c, "o"), Some("z"), "{c}");
        assert_eq!(field(c, "q"), Some("2"), "{c}");
    }
    // No Unicode placeholders: the images are not in the cells' text.
    assert!(buf
        .content()
        .iter()
        .all(|c| !c.symbol().contains('\u{10EEEE}')));

    // Labels stay readable on top: the pill's text is there, with no cell
    // background in the way of the image.
    let mon = hits
        .rect(Hit::Control(input(0, InputControl::Mon)))
        .unwrap();
    let chip = spots
        .iter()
        .find(|s| s.rect == mon)
        .expect("a chip under MON");
    assert_eq!(kind(&chip.widget), "chip");
    let label: String = (mon.x..mon.right())
        .map(|x| buf[(x, mon.y)].symbol())
        .collect();
    assert_eq!(label.trim(), "MON");
    assert!((mon.x..mon.right()).all(|x| buf[(x, mon.y)].bg == Color::Reset));
    // The active mix tab and every strip title have a chip.
    let tab = hits.rect(Hit::MixTab(0)).unwrap();
    assert!(spots
        .iter()
        .any(|s| kind(&s.widget) == "chip" && s.rect.y == tab.y && s.rect.x >= tab.x));
    for name in [
        "IN1", "IN2", "IN 1+2", "PB 1/2", "LOOP 1+2", "OUT 1+2", "PHONE",
    ] {
        assert!(
            spots.iter().any(|s| {
                let t: String = (s.rect.x..s.rect.right())
                    .map(|x| buf[(x, s.rect.y)].symbol())
                    .collect();
                kind(&s.widget) == "chip" && t.trim() == name
            }),
            "a chip under {name}"
        );
    }
    // Graphics without labels leave their cells blank (no Braille or
    // blocks under the image); a fader keeps its scale labels.
    for s in spots
        .iter()
        .filter(|s| matches!(kind(&s.widget), "knob" | "meter" | "clip"))
    {
        for y in s.rect.top()..s.rect.bottom() {
            for x in s.rect.left()..s.rect.right() {
                assert_eq!(buf[(x, y)].symbol(), " ", "{s:?} at {x},{y}");
            }
        }
    }
    let fader = spots
        .iter()
        .find(|s| {
            matches!(
                s.widget,
                toppingctl::tui::pixel::PixelWidget::Fader { labels: true, .. }
            )
        })
        .unwrap();
    let rows: Vec<String> = (fader.rect.top()..fader.rect.bottom())
        .map(|y| {
            (fader.rect.left()..fader.rect.right())
                .map(|x| buf[(x, y)].symbol())
                .collect()
        })
        .collect();
    assert!(rows.iter().any(|r| r.contains("−12")), "{rows:?}");
    assert!(
        rows.iter().all(|r| !r.contains('┃') && !r.contains('●')),
        "{rows:?}"
    );

    // Mouse hit-testing is the same as on the text path.
    let (_, text_hits) = render(&app, &meters, 120, 40);
    for y in 0..40 {
        for x in 0..120 {
            assert_eq!(hits.get(x, y), text_hits.get(x, y), "at {x},{y}");
        }
    }
}

/// Redraws only send what changed: nothing for an unchanged frame, a
/// swap of placements for a moving meter, cached images for values seen
/// before, and every placement goes on resize, too-small and a modal.
#[test]
fn pixel_widgets_redraw() {
    let mut app = synced_app(Theme::color());
    let mut meters = fake_meters();
    let mut layer = kitty_layer();
    let (_, hits, _) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    let all = hits.pixels().len();

    // Unchanged: nothing at all is written (no flicker).
    let (_, _, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    assert_eq!(out, "", "an unchanged frame writes nothing");

    // IN1 moves: its two meters (input panel, IN 1+2) are re-placed, new
    // image first, then the old placement is deleted.
    let built = layer.built();
    meters.inputs[0] = -6.0;
    let (_, hits, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    let placed = placements(&out);
    assert_eq!(placed.len(), 2, "{out:?}");
    assert_eq!(count(&out, "d"), 2, "{out:?}");
    for p in &placed {
        let s = hits
            .pixels()
            .iter()
            .find(|s| (s.rect.x, s.rect.y) == (p.x, p.y))
            .unwrap();
        assert_eq!(kind(&s.widget), "meter");
    }
    let first_delete = out.find("a=d").unwrap();
    let first_place = out.find("a=p").unwrap();
    assert!(first_place < first_delete, "place, then delete: {out:?}");
    assert!(layer.built() > built);

    // Back to a level seen before: the cached image, nothing sent.
    let built = layer.built();
    meters.inputs[0] = -18.4;
    let (_, _, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    assert_eq!(count(&out, "t"), 0, "cache hit");
    assert_eq!(placements(&out).len(), 2);
    assert_eq!(layer.built(), built);
    // A wiggle under the quantisation step is the same image.
    meters.inputs[0] = -18.45;
    let (_, _, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    assert_eq!(out, "");

    // A focus change re-places only the strip titles' chips (the
    // focused title is lit), with no blanket delete.
    let mut focus_app = synced_app(Theme::color());
    let mut focus_layer = kitty_layer();
    draw_pixels(&focus_app, &meters, 120, 40, &mut focus_layer);
    focus_app.handle_key(key(KeyCode::Tab));
    let (_, hits, out) = draw_pixels(&focus_app, &meters, 120, 40, &mut focus_layer);
    assert!(!out.contains("d=a"), "{out:?}");
    let placed = placements(&out);
    assert!(!placed.is_empty());
    assert_eq!(count(&out, "d"), placed.len());
    for p in &placed {
        let s = hits
            .pixels()
            .iter()
            .find(|s| (s.rect.x, s.rect.y) == (p.x, p.y))
            .unwrap();
        assert_eq!(kind(&s.widget), "chip", "{s:?}");
    }

    // A clip lights IN1's lamps and they go out after the hold.
    let t0 = Instant::now();
    meters.inputs[0] = 0.0;
    app.tick(&meters, t0);
    let (_, hits, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    let lit = hits
        .pixels()
        .iter()
        .filter(|s| {
            matches!(
                s.widget,
                toppingctl::tui::pixel::PixelWidget::ClipLight { lit: true }
            )
        })
        .count();
    assert_eq!(lit, 2, "IN1 in the input panel and in IN 1+2");
    assert!(placements(&out).len() >= 4, "2 meters, 2 lamps");
    meters.inputs[0] = -18.4;
    app.tick(&meters, t0 + Duration::from_millis(1100));
    let (_, hits, _) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    assert!(hits.pixels().iter().all(|s| !matches!(
        s.widget,
        toppingctl::tui::pixel::PixelWidget::ClipLight { lit: true }
    )));

    // The gain knob turns: one new image.
    let built = layer.built();
    app.handle_key(ch('1'));
    for _ in 0..6 {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Right));
    let (_, _, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    assert_eq!(layer.built(), built + 1);
    assert_eq!(count(&out, "t"), 1);

    // Resize: every placement goes first, then all are placed again in
    // the new cells.
    let (_, hits, out) = draw_pixels(&app, &meters, 100, 34, &mut layer);
    assert!(out.starts_with("\x1b_Ga=d,d=a,q=2\x1b\\"), "{out:?}");
    assert_eq!(placements(&out).len(), hits.pixels().len());
    assert!(placements(&out)
        .iter()
        .all(|p| p.x + p.cols <= 100 && p.y + p.rows <= 34));

    // Too small: nothing placed, the old placements gone.
    let (_, hits, out) = draw_pixels(&app, &meters, 90, 30, &mut layer);
    assert!(hits.pixels().is_empty());
    assert!(out.starts_with("\x1b_Ga=d,d=a,q=2\x1b\\"), "{out:?}");
    assert!(placements(&out).is_empty());

    // Back to full size, then a modal: every placement is deleted so its
    // text stays on top, and the text widgets are drawn under it.
    draw_pixels(&app, &meters, 120, 40, &mut layer);
    let built = layer.built();
    app.handle_key(ch('?'));
    let (_, _, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    assert!(placements(&out).is_empty());
    assert_eq!(count(&out, "d"), all);
    // Closed: placed again from the cache.
    app.handle_key(key(KeyCode::Esc));
    let (_, _, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    assert_eq!(placements(&out).len(), all);
    assert_eq!(count(&out, "t"), 0);
    assert_eq!(layer.built(), built);

    // The text path (and the golden snapshots) never place images.
    let (buf, hits) = render(&app, &meters, 120, 40);
    assert!(buf.content().iter().all(|c| !c.symbol().contains("\x1b_G")));
    assert!(!hits.pixels().is_empty(), "spots are recorded either way");
}

/// Quitting frees every image the TUI sent; TOPPINGCTL_PIXELS=0 keeps
/// the text widgets.
#[test]
fn pixel_cleanup_and_opt_out() {
    use std::ffi::OsStr;
    use toppingctl::tui::pixel::{pixels_wanted, PixelLayer};
    let mut out = Vec::new();
    toppingctl::tui::run::free_images(&mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "\x1b_Ga=d,d=A,q=2\x1b\\");
    assert_eq!(PixelLayer::FREE_ALL, "\x1b_Ga=d,d=A,q=2\x1b\\");

    assert!(pixels_wanted(None));
    assert!(pixels_wanted(Some(OsStr::new("1"))));
    assert!(!pixels_wanted(Some(OsStr::new("0"))));
}

/// Every kitty escape carries at most 4096 bytes of base64; a long image
/// goes in `m=1` chunks ending with `m=0`.
#[test]
fn pixel_images_are_chunked() {
    let app = synced_app(Theme::color());
    let mut layer = kitty_layer();
    let (_, _, out) = draw_pixels(&app, &fake_meters(), 120, 40, &mut layer);
    let mut open = false;
    let mut chunked = 0;
    for apc in out.split("\x1b_G").skip(1) {
        let body = &apc[..apc.find("\x1b\\").expect("every escape is closed")];
        let (ctrl, payload) = body.split_once(';').unwrap_or((body, ""));
        assert!(
            payload.len() <= 4096,
            "{} bytes in one escape",
            payload.len()
        );
        let m = field(ctrl, "m");
        if open {
            assert_eq!(
                ctrl.split(',').count(),
                1,
                "a continuation has only m: {ctrl}"
            );
        }
        if m == Some("1") && field(ctrl, "a") == Some("t") {
            chunked += 1;
        }
        open = m == Some("1");
    }
    assert!(!open, "the last chunk is m=0");
    // Nothing but well-formed escapes: kitty commands and cursor moves.
    let mut rest = out.as_str();
    while !rest.is_empty() {
        if let Some(t) = rest.strip_prefix("\x1b_G") {
            rest = &t[t.find("\x1b\\").unwrap() + 2..];
        } else if let Some(t) = rest.strip_prefix("\x1b[") {
            let end = t.find('H').unwrap();
            assert!(t[..end].chars().all(|c| c.is_ascii_digit() || c == ';'));
            rest = &t[end + 1..];
        } else {
            panic!("stray bytes: {:?}", &rest[..rest.len().min(40)]);
        }
    }
    assert!(chunked > 0, "some image needed chunks");
}

/// Regression (hardware run on 0e04129: the frame stopped where the first
/// pixel knob began, and the meters froze): a frame through a real
/// crossterm backend writes the whole text grid, with no image bytes in
/// it; the images go out once, after the text, and the next frames keep
/// following the meters.
#[test]
fn pixel_frame_writes_the_whole_grid() {
    use ratatui::backend::CrosstermBackend;
    use ratatui::{TerminalOptions, Viewport};
    use toppingctl::tui::run::draw_frame;

    /// What the backend writes, readable from the test.
    #[derive(Clone, Default)]
    struct Shared(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
    impl std::io::Write for Shared {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let app = synced_app(Theme::color());
    let mut meters = fake_meters();
    let mut layer = kitty_layer();
    let written = Shared::default();
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(written.clone()),
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 120, 40)),
        },
    )
    .unwrap();
    let mut images = Vec::new();
    let hits = draw_frame(&mut terminal, &app, &meters, Some(&mut layer), &mut images).unwrap();
    let grid = String::from_utf8(written.0.take()).unwrap();
    assert!(!grid.contains("\x1b_G"), "no image bytes in the text grid");
    // The last row (the key hints) and the last panel made it out.
    assert!(grid.contains("quit"), "the footer was written");
    assert!(grid.contains("PHONE"), "the output panel was written");
    let images = String::from_utf8(images).unwrap();
    assert_eq!(placements(&images).len(), hits.pixels().len());

    // The meters move: the next frame places the new meter images.
    meters.inputs[1] = -30.0;
    let mut images = Vec::new();
    draw_frame(&mut terminal, &app, &meters, Some(&mut layer), &mut images).unwrap();
    let images = String::from_utf8(images).unwrap();
    // IN2's meter in the input panel and in IN 1+2.
    assert_eq!(placements(&images).len(), 2, "{images:?}");
}

/// A first frame with more images than one frame's budget sends them over
/// a few frames; until a widget's image is sent it keeps its text form.
#[test]
fn pixel_transmit_budget() {
    use toppingctl::tui::pixel::{PixelLayer, TRANSMIT_BUDGET};
    let app = synced_app(Theme::color());
    let meters = fake_meters();
    // Big cells: the first frame is over budget.
    let mut layer = PixelLayer::with_cell_size((40, 80));
    let (buf, hits, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    let first = placements(&out).len();
    assert!(
        first < hits.pixels().len(),
        "not everything fits in one frame"
    );
    assert!(out.len() < TRANSMIT_BUDGET * 2, "{} bytes", out.len());
    // A widget not placed yet is still drawn as text (the Braille knob's
    // dots, block meters).
    let t = text(&buf);
    assert!(t.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)) || t.contains('▕'));
    let mut frames = 1;
    let mut total = first;
    while total < hits.pixels().len() {
        let (_, _, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
        total += placements(&out).len();
        frames += 1;
        assert!(frames < 30, "everything is sent in a few frames");
    }
    let (_, _, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    assert_eq!(out, "", "then nothing more");
}

// ------------------------------------------------- Monitor Mix and PHONE

/// MONITOR MIX mirrors the balance the device reports (`11 01`, 0 ..= 100,
/// 50 = centre) on a dimmed knob nobody can turn; PHONE shows the level
/// switch (`11 02`) as +17 dBu / 0 dBu, with no volume knob.
#[test]
fn monitor_mix_and_phone_level() {
    use toppingctl::tui::pixel::PixelWidget;
    let mut app = synced_app(Theme::color());
    app.on_report(&frame(0x11, 0x01, 30));
    let (buf, hits) = render(&app, &fake_meters(), 120, 40);
    let t = text(&buf);
    assert!(!t.contains("on device only"), "{t}");
    assert!(!t.contains("volume knob"), "{t}");
    let monitor = hits
        .pixels()
        .iter()
        .find(|s| matches!(s.widget, PixelWidget::Knob { dimmed: true, .. }))
        .expect("the Monitor Mix knob");
    assert_eq!(
        monitor.widget,
        PixelWidget::Knob {
            value: Some(0.3),
            ticks: 0,
            dimmed: true
        }
    );
    // Its value in the strip, as the app shows it (v − 50).
    let strip: String = (monitor.rect.y..40)
        .map(|y| {
            (monitor.rect.x.saturating_sub(4)..monitor.rect.right() + 4)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                + "\n"
        })
        .collect();
    assert!(strip.contains("−20"), "{strip}");
    // Not interactive: a click on it sends nothing.
    let mut actions = Vec::new();
    let c = monitor.rect.as_position();
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        actions.extend(app.handle_mouse(mouse(kind, c.x, c.y, KeyModifiers::NONE), &hits));
    }
    actions.extend(app.handle_mouse(
        mouse(MouseEventKind::ScrollUp, c.x, c.y, KeyModifiers::NONE),
        &hits,
    ));
    assert!(sends(&actions).is_empty());

    // Centre reads 0; unknown has no pointer and a dash.
    app.on_report(&frame(0x11, 0x01, 50));
    let (buf, _) = render(&app, &fake_meters(), 120, 40);
    let t = text(&buf);
    assert!(t.contains("input") && t.contains("playback"), "{t}");

    // PHONE: the level switch, lit on the active side.
    let lit = |buf: &Buffer, label: &str| -> Option<bool> {
        let t = text(buf);
        let (y, line) = t.lines().enumerate().find(|(_, l)| l.contains(label))?;
        let col = line[..line.find(label)?].chars().count() as u16;
        Some(buf[(col, y as u16)].bg == Theme::ACCENT)
    };
    app.on_report(&frame(0x11, 0x02, 1));
    let (buf, _) = render(&app, &fake_meters(), 120, 40);
    assert_eq!(lit(&buf, "+17 dBu"), Some(true));
    assert_eq!(lit(&buf, " 0 dBu"), Some(false));
    app.on_report(&frame(0x11, 0x02, 0));
    let (buf, _) = render(&app, &fake_meters(), 120, 40);
    assert_eq!(lit(&buf, "+17 dBu"), Some(false));
    assert_eq!(lit(&buf, " 0 dBu"), Some(true));
}

// ------------------------------------------------------ hardware fix round

/// The device's idle frames keep the header at "synced" while every meter
/// sits at the floor; "no meters" is kept for a real silence.
#[test]
fn idle_frames_keep_synced() {
    use toppingctl::tui::run::mark_alive;
    let header = |app: &App| {
        let (buf, _) = render(app, &Meters::default(), 120, 40);
        text(&buf).lines().take(4).collect::<Vec<_>>().join("\n")
    };
    let long_ago = Instant::now() - Duration::from_secs(3);

    // Silence: nothing at all for 3 s.
    let mut app = synced_app(Theme::color());
    app.tick(&Meters::default(), long_ago);
    app.tick(&Meters::default(), Instant::now());
    assert!(header(&app).contains("no meters"), "{}", header(&app));

    // Idle frames only (no signal, no meter frames): alive.
    let (transport, fake) = FakeTransport::new();
    let (conn, _events) = Connection::spawn(transport);
    let mut app = synced_app(Theme::color());
    app.tick(&Meters::default(), long_ago);
    fake.push_report(&frame(0x00, 0x00, 0).encode(toppingctl::frame::Crc::Fill));
    let deadline = Instant::now() + Duration::from_secs(2);
    while conn.last_seen().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    mark_alive(&conn, &mut app);
    app.tick(&Meters::default(), Instant::now());
    let h = header(&app);
    assert!(h.contains("synced") && !h.contains("no meters"), "{h}");
}

/// The PHONE level switch (`11 02`): space / Enter on PHONE toggles it and
/// writes it; a click on either pill picks that level; the device's own
/// button (its echo) moves it too.
#[test]
fn phone_level_switch_is_programmable() {
    let mut app = synced_app(Theme::color());
    assert!(app.state().phone_gain, "+17 dBu to start");
    app.handle_key(ch('4'));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.focus(), Control::Output(2));
    let a = app.handle_key(ch(' '));
    assert_eq!(sends(&a), vec![frame(0x11, 0x02, 0)]);
    assert!(a.contains(&Action::Save));
    assert!(!app.state().phone_gain);
    let a = app.handle_key(key(KeyCode::Enter));
    assert_eq!(sends(&a), vec![frame(0x11, 0x02, 1)]);

    // Clicks: "0 dBu" picks 0 dBu, again does nothing, "+17 dBu" goes back.
    let (_, hits) = render(&app, &fake_meters(), 120, 40);
    let low = hits.rect(Hit::Phone(false)).expect("the 0 dBu pill");
    let high = hits.rect(Hit::Phone(true)).expect("the +17 dBu pill");
    let a = click_at(&mut app, &hits, low, Instant::now());
    assert_eq!(sends(&a), vec![frame(0x11, 0x02, 0)]);
    let a = click_at(
        &mut app,
        &hits,
        low,
        Instant::now() + Duration::from_secs(1),
    );
    assert!(sends(&a).is_empty());
    let a = click_at(
        &mut app,
        &hits,
        high,
        Instant::now() + Duration::from_secs(2),
    );
    assert_eq!(sends(&a), vec![frame(0x11, 0x02, 1)]);

    // The device's button: its report moves the switch.
    app.on_report(&frame(0x11, 0x02, 0));
    assert!(!app.state().phone_gain);
    let (buf, hits) = render(&app, &fake_meters(), 120, 40);
    let low = hits.rect(Hit::Phone(false)).unwrap();
    assert_eq!(buf[(low.x, low.y)].bg, Theme::ACCENT, "0 dBu lit");
}

/// The header shows the interface's real sample rate (`48 kHz ▾`); the
/// ▾ (or `r`) opens a list of the E2x2's rates, and picking one asks the
/// event loop to set it.
#[test]
fn sample_rate_header_and_list() {
    let mut app = synced_app(Theme::color());
    let head = |app: &App, w: u16, h: u16| {
        let (buf, hits) = render(app, &fake_meters(), w, h);
        (
            text(&buf).lines().take(4).collect::<Vec<_>>().join("\n"),
            hits,
        )
    };
    assert!(head(&app, 120, 40).0.contains("— kHz ▾"));
    app.set_sample_rate(Some(48000));
    assert!(
        head(&app, 120, 40).0.contains("48 kHz ▾"),
        "{}",
        head(&app, 120, 40).0
    );
    app.set_sample_rate(Some(44100));
    assert!(head(&app, 140, 40).0.contains("44.1 kHz ▾"));

    // Click the rate: the list opens; click 96 kHz: set it.
    let (_, hits) = head(&app, 120, 40);
    let r = hits.rect(Hit::Rate).expect("the rate is clickable");
    assert!(click_at(&mut app, &hits, r, Instant::now()).is_empty());
    assert!(matches!(app.modal(), Some(Modal::Rate { .. })));
    let (buf, hits) = render(&app, &fake_meters(), 120, 40);
    let t = text(&buf);
    for k in [
        "44.1 kHz",
        "48 kHz",
        "88.2 kHz",
        "96 kHz",
        "176.4 kHz",
        "192 kHz",
    ] {
        assert!(t.contains(k), "{k} in the list:\n{t}");
    }
    let r96 = hits.rect(Hit::RateOption(3)).unwrap();
    let a = click_at(
        &mut app,
        &hits,
        r96,
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(a, vec![Action::SetRate(96000)]);
    assert_eq!(app.modal(), None);

    // Keys: r opens at the current rate, ↓ and Enter pick the next one,
    // Esc closes without a change.
    app.handle_key(ch('r'));
    assert_eq!(app.modal(), Some(Modal::Rate { sel: 0 }), "44.1 kHz now");
    app.handle_key(key(KeyCode::Down));
    assert_eq!(
        app.handle_key(key(KeyCode::Enter)),
        vec![Action::SetRate(48000)]
    );
    app.handle_key(ch('r'));
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert_eq!(app.modal(), None);
}

/// The exact meter frames hidsniff saw (IN1 at −34.3 dB, output L at
/// −34.2 dB), and the PB 1/2 and output R meters the device sends while
/// music plays (`41 01/02`, `31 04`), amid idle frames, go through a
/// `Connection` to the pixel meters.
#[test]
fn hidsniff_meters_reach_the_pixel_meters() {
    use toppingctl::frame::Crc;
    use toppingctl::tui::pixel::PixelWidget;
    use toppingctl::tui::run::drain_device;
    let input = [
        0x22, 0x33, 0x20, 0x01, 0x01, 0x21, 0x05, 0xff, 0xff, 0xfe, 0xa9, 0xb6, 0x08, 0x66, 0x77,
        0x00,
    ];
    let enc = |g, p, v: i32| frame(g, p, v).encode(Crc::Fill);
    let idle = enc(0x00, 0x00, 0);
    let (transport, fake) = FakeTransport::new();
    for report in [
        idle,
        input,
        idle,
        enc(0x31, 0x03, 0xffff_feaa_u32 as i32), // −34.2 dB
        enc(0x31, 0x04, -150),                   // −15.0 dB
        enc(0x41, 0x01, -120),                   // −12.0 dB
        idle,
        enc(0x41, 0x02, -95), // −9.5 dB
    ] {
        fake.push_report(&report);
    }
    let (_conn, events) = Connection::spawn(transport);
    let mut app = synced_app(Theme::color());
    let mut meters = Meters::default();
    let deadline = Instant::now() + Duration::from_secs(2);
    while meters.playback[0][1] < -90.0 && Instant::now() < deadline {
        drain_device(&events, &mut app, &mut meters).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    let near = |a: f64, b: f64| (a - b).abs() < 0.05;
    assert!(near(meters.inputs[0], -34.3), "{:?}", meters.inputs);
    assert!(near(meters.output[0], -34.2), "{:?}", meters.output);
    assert!(near(meters.output[1], -15.0), "{:?}", meters.output);
    assert!(near(meters.playback[0][0], -12.0), "{:?}", meters.playback);
    assert!(near(meters.playback[0][1], -9.5), "{:?}", meters.playback);

    let mut layer = kitty_layer();
    let (_, hits, out) = draw_pixels(&app, &meters, 120, 40, &mut layer);
    let placed = placements(&out);
    let at = |db: f64| {
        hits.pixels()
            .iter()
            .filter(|s| matches!(s.widget, PixelWidget::Meter { db: d } if (d - db).abs() < 1e-9))
            .map(|s| {
                assert!(
                    placed.iter().any(|p| (p.x, p.y) == (s.rect.x, s.rect.y)),
                    "{s:?}"
                );
                s.rect
            })
            .count()
    };
    assert_eq!(at(meters.inputs[0]), 2, "input panel and IN 1+2");
    assert_eq!(at(meters.output[0]), 1, "OUT 1+2 L");
    assert_eq!(at(meters.output[1]), 1, "OUT 1+2 R");
    assert_eq!(at(meters.playback[0][0]), 1, "PB 1/2 L");
    assert_eq!(at(meters.playback[0][1]), 1, "PB 1/2 R");
}
