//! The loopback and output panels (#15), wired 1:1 with the official app:
//! source menus, faders on the app's −90…0 dB range, the LINK page swap,
//! per-channel loopback MUTE, HP / LINE, names, live reports, pre- and
//! post-fader meters with IN / OUT clip lights. Every frame goes through
//! `State::diff` and a real `Connection` over `FakeTransport`. No terminal,
//! no hardware.
//!
//! Golden snapshots live in `tests/snapshots/`. Run with
//! `UPDATE_SNAPSHOTS=1` to rewrite them after an intended change.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

use toppingctl::conn::fake::{FakeHandle, FakeTransport};
use toppingctl::conn::Connection;
use toppingctl::frame::{Dir, Frame};
use toppingctl::state::{Meters, Source, State};
use toppingctl::sync::Store;
use toppingctl::tui::app::{
    Action, App, BusControl, Control, InputControl, MeterId, Modal, Name, OUT_BUS, SOURCE_MENU,
};
use toppingctl::tui::pixel::PixelWidget;
use toppingctl::tui::run::send_actions;
use toppingctl::tui::theme::Theme;
use toppingctl::tui::ui::{self, Hit, HitMap};
use toppingctl::tui::widgets::meter::CLIP_HOLD;
use toppingctl::value::level_from_db;

// ---------------------------------------------------------------- helpers

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

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn bus(bus: usize, ctl: BusControl) -> Control {
    Control::Bus { bus, ctl }
}

fn lvl(db: f64) -> i32 {
    level_from_db(db)
}

fn render(app: &App, meters: &Meters, w: u16, h: u16) -> (Buffer, HitMap) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    terminal.draw(|f| hits = ui::draw(f, app, meters)).unwrap();
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

/// The text of `r` on the last render.
fn text_in(buf: &Buffer, r: Rect) -> String {
    let mut out = String::new();
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
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

fn app_with(state: State) -> App {
    let mut app = App::new(Theme::color());
    app.synced(state);
    app
}

fn fake_meters() -> Meters {
    Meters {
        inputs: [-18.4, -4.0],
        playback: [[-8.0, -9.5], [-22.0, -23.5], [-40.0, -41.0]],
        output: [-9.0, -10.5],
        loopbacks: [[-22.0, -23.0], [-96.0, -96.0]],
    }
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

    fn written(mut self) -> Vec<Frame> {
        drop(self.conn.take());
        self.fake
            .written()
            .iter()
            .map(|r| Frame::decode(r, Dir::ToDevice).unwrap())
            .collect()
    }
}

/// Runs `f` on the app and checks that the frames it sends are exactly
/// `State::diff` from the state before to the state after, then a save
/// when anything changed. Forwards the actions to `dev`.
fn step(app: &mut App, dev: &Device, f: impl FnOnce(&mut App) -> Vec<Action>) -> Vec<Frame> {
    let before = app.state().clone();
    let actions = f(app);
    let after = app.state().clone();
    let frames = sends(&actions);
    assert_eq!(frames, before.diff(&after), "frames are State::diff");
    if before != after {
        assert_eq!(actions.last(), Some(&Action::Save), "{actions:?}");
    } else {
        assert!(actions.is_empty(), "{actions:?}");
    }
    dev.send(&actions);
    frames
}

/// Moves focus to `target` with keys only: `3` / `4` (the panel), → to
/// the strip, ↓ into its controls, → along a row of two.
fn focus(app: &mut App, target: Control) {
    let Control::Bus { bus: b, .. } = target else {
        panic!("not a loopback or output control: {target:?}");
    };
    if b == OUT_BUS {
        app.handle_key(ch('4'));
        assert_eq!(app.focus(), Control::Output(0));
    } else {
        app.handle_key(ch('3'));
        for _ in 0..b {
            app.handle_key(key(KeyCode::Right));
        }
        assert_eq!(app.focus(), Control::Loopback(b));
    }
    // The second of a pair (R fader, R MUTE, LINE) is right of the first.
    let Control::Bus { ctl, .. } = target else {
        unreachable!()
    };
    let (down_to, right) = match ctl {
        BusControl::Fader(1) => (BusControl::Fader(0), true),
        BusControl::Mute(1) => (BusControl::Mute(0), true),
        BusControl::Line => (BusControl::Hp, true),
        c => (c, false),
    };
    for _ in 0..10 {
        if app.focus() == bus(b, down_to) {
            break;
        }
        assert!(app.handle_key(key(KeyCode::Down)).is_empty());
    }
    if right {
        assert!(app.handle_key(key(KeyCode::Right)).is_empty());
    }
    assert_eq!(app.focus(), target, "the arrows never reached it");
}

fn mouse(kind: MouseEventKind, x: u16, y: u16, modifiers: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers,
    }
}

const DOWN: MouseEventKind = MouseEventKind::Down(MouseButton::Left);
const UP: MouseEventKind = MouseEventKind::Up(MouseButton::Left);
const DRAG: MouseEventKind = MouseEventKind::Drag(MouseButton::Left);
const NONE: KeyModifiers = KeyModifiers::NONE;

/// Picks `src` in the open source menu with ↑↓ and Enter.
fn pick(app: &mut App, dev: &Device, src: Source) -> Vec<Frame> {
    let Some(Modal::Source { .. }) = app.modal() else {
        panic!("no source menu: {:?}", app.modal());
    };
    let want = SOURCE_MENU.iter().position(|&s| s == src).unwrap();
    for _ in 0..SOURCE_MENU.len() {
        app.handle_key(key(KeyCode::Up));
    }
    for _ in 0..want {
        assert!(app.handle_key(key(KeyCode::Down)).is_empty());
    }
    step(app, dev, |a| a.handle_key(key(KeyCode::Enter)))
}

// ------------------------------------------------------------------ AC1

/// The validation table's cases, end to end, plus the rest of the
/// controls: the frames sent equal `State::diff`, with PROTOCOL.md's
/// values.
#[test]
fn controls_send_expected_frames() {
    use BusControl::*;

    // Output source Mix B, from the menu (all nine sources offered).
    let mut app = app_with(State::default());
    let dev = Device::new();
    assert_eq!(SOURCE_MENU.len(), 9);
    focus(&mut app, bus(OUT_BUS, Source));
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert!(matches!(
        app.modal(),
        Some(Modal::Source { bus: OUT_BUS, .. })
    ));
    let frames = pick(&mut app, &dev, toppingctl::state::Source::MixB);
    assert_eq!(frames, [frame(0x31, 0x05, 0x20)]);
    assert!(app.modal().is_none());
    // And a source only the app's full menu has: PB 3/4.
    app.handle_key(ch(' '));
    let frames = pick(&mut app, &dev, toppingctl::state::Source::Pb34);
    assert_eq!(frames, [frame(0x31, 0x05, 0x200)]);
    // Esc closes the menu with nothing sent.
    app.handle_key(ch(' '));
    assert!(app.handle_key(key(KeyCode::Down)).is_empty());
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.modal().is_none());
    let written = dev.written();
    assert_eq!(written, [frame(0x31, 0x05, 0x20), frame(0x31, 0x05, 0x200)]);

    // Output level −6 dB with LINK on: 1 dB a step, both 31 01 and 31 02.
    let mut app = app_with(State::default());
    let dev = Device::new();
    focus(&mut app, bus(OUT_BUS, Fader(0)));
    let mut all = Vec::new();
    for _ in 0..6 {
        all.extend(step(&mut app, &dev, |a| a.handle_key(ch('-'))));
    }
    assert_eq!(
        all[all.len() - 2..],
        [frame(0x31, 0x01, lvl(-6.0)), frame(0x31, 0x02, lvl(-6.0))]
    );
    assert_eq!(all.len(), 12);
    assert_eq!(dev.written(), all);

    // Output R only −12 dB with LINK off: L stays.
    let mut app = app_with(State::default());
    let dev = Device::new();
    focus(&mut app, bus(OUT_BUS, Link));
    assert_eq!(
        app.handle_key(ch(' ')),
        [Action::Save],
        "LINK sends nothing"
    );
    assert!(!app.state().output.link);
    focus(&mut app, bus(OUT_BUS, Fader(1)));
    let mut all = Vec::new();
    // PgDn (×10) is the app's page step, 5 dB.
    for k in [key(KeyCode::PageDown), key(KeyCode::PageDown)] {
        all.extend(step(&mut app, &dev, |a| a.handle_key(k)));
    }
    assert_eq!(app.state().output.level_r_db, -10.0);
    for _ in 0..2 {
        all.extend(step(&mut app, &dev, |a| a.handle_key(ch('-'))));
    }
    assert_eq!(all.last(), Some(&frame(0x31, 0x02, lvl(-12.0))));
    assert!(all.iter().all(|f| (f.group, f.param) == (0x31, 0x02)));
    assert_eq!(app.state().output.level_l_db, 0.0);
    // Linking with L ≠ R: both to the average, −6 dB.
    focus(&mut app, bus(OUT_BUS, Link));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert_eq!(
        frames,
        [frame(0x31, 0x01, lvl(-6.0)), frame(0x31, 0x02, lvl(-6.0))]
    );
    dev.written();

    // HP off: both switches, then the output level (as the app does).
    let mut app = app_with(State::default());
    let dev = Device::new();
    focus(&mut app, bus(OUT_BUS, Hp));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert_eq!(
        frames,
        [
            frame(0x34, 0x01, 0),
            frame(0x34, 0x02, 1),
            frame(0x31, 0x01, lvl(0.0)),
            frame(0x31, 0x02, lvl(0.0)),
        ]
    );
    // LINE off too: allowed (no rule forbids it).
    focus(&mut app, bus(OUT_BUS, Line));
    let frames = step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Enter)));
    assert_eq!(frames[..2], [frame(0x34, 0x01, 0), frame(0x34, 0x02, 0)]);
    assert!(!app.state().output.headphones && !app.state().output.line);
    dev.written();

    // Loopback 3+4 source IN1+2.
    let mut app = app_with(State::default());
    let dev = Device::new();
    focus(&mut app, bus(1, Source));
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.modal(), Some(Modal::Source { bus: 1, .. })));
    let frames = pick(&mut app, &dev, toppingctl::state::Source::In12);
    assert_eq!(frames, [frame(0x33, 0x05, 0x3000)]);
    assert_eq!(dev.written(), frames);

    // Loopback 1+2 mute (linked): both channels to 0.
    let mut app = app_with(State::default());
    let dev = Device::new();
    focus(&mut app, bus(0, Mute(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert_eq!(frames, [frame(0x32, 0x01, 0), frame(0x32, 0x02, 0)]);
    let lb = &app.state().loopbacks[0];
    assert!(lb.mute_l && lb.mute_r);
    assert_eq!(dev.written(), frames);

    // Loopback 3+4 unlinked: MUTE R alone, then the L fader alone.
    let mut app = app_with(State::default());
    let dev = Device::new();
    focus(&mut app, bus(1, Link));
    assert_eq!(app.handle_key(ch(' ')), [Action::Save]);
    focus(&mut app, bus(1, Mute(1)));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert_eq!(frames, [frame(0x33, 0x02, 0)]);
    focus(&mut app, bus(1, Fader(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch('-')));
    assert_eq!(frames, [frame(0x33, 0x01, lvl(-1.0))]);
    // Linking ORs the mutes: never unmutes by surprise.
    focus(&mut app, bus(1, Link));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    let lb = &app.state().loopbacks[1];
    assert!(lb.link && lb.mute_l && lb.mute_r);
    assert_eq!(frames, [frame(0x33, 0x01, 0)]);
    dev.written();

    // The range: −90…0 dB, 1 dB a step; −90 is −∞; 0 dB is the top;
    // `0` resets to 0 dB.
    let mut app = app_with(State::default());
    let dev = Device::new();
    focus(&mut app, bus(0, Fader(0)));
    assert!(step(&mut app, &dev, |a| a.handle_key(ch('+'))).is_empty());
    for _ in 0..17 {
        step(&mut app, &dev, |a| a.handle_key(key(KeyCode::PageDown)));
    }
    assert_eq!(app.state().loopbacks[0].level_l_db, -85.0);
    for _ in 0..4 {
        step(&mut app, &dev, |a| a.handle_key(ch('-')));
    }
    assert_eq!(app.state().loopbacks[0].level_l_db, -89.0);
    let frames = step(&mut app, &dev, |a| a.handle_key(ch('-')));
    assert_eq!(app.state().loopbacks[0].level_l_db, f64::NEG_INFINITY);
    assert_eq!(frames, [frame(0x32, 0x01, 0), frame(0x32, 0x02, 0)]);
    assert!(step(&mut app, &dev, |a| a.handle_key(ch('-'))).is_empty());
    step(&mut app, &dev, |a| a.handle_key(ch('+')));
    assert_eq!(app.state().loopbacks[0].level_l_db, -89.0);
    step(&mut app, &dev, |a| a.handle_key(ch('0')));
    assert_eq!(app.state().loopbacks[0].level_r_db, 0.0);
    let written = dev.written();
    assert!(written.iter().all(|f| f.group == 0x32), "{written:?}");
}

/// Focus walks a strip's controls in the drawn order (#47): source, LINK,
/// the fader(s), then MUTE (or HP and LINE) at the bottom; ←→ across a
/// pair, then to the strip beside; the page swap changes them.
#[test]
fn focus_walks_loopback_and_output() {
    use BusControl::*;
    let mut app = app_with(State::default());
    app.handle_key(ch('3'));
    let mut seen = vec![app.focus()];
    for _ in 0..5 {
        app.handle_key(key(KeyCode::Down));
        seen.push(app.focus());
    }
    assert_eq!(
        seen,
        [
            Control::Loopback(0),
            bus(0, Source),
            bus(0, Link),
            bus(0, Fader(0)),
            bus(0, Mute(0)),
            bus(0, Mute(0)),
        ]
    );
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.focus(), bus(1, Mute(0)));
    for _ in 0..5 {
        app.handle_key(key(KeyCode::Up));
    }
    assert_eq!(app.focus(), Control::Loopback(1));

    let mut s = State::default();
    s.output.link = false;
    let mut app = app_with(s);
    app.handle_key(ch('4'));
    let mut seen = vec![app.focus()];
    for k in [
        KeyCode::Down,
        KeyCode::Down,
        KeyCode::Down,
        KeyCode::Right,
        KeyCode::Down,
        KeyCode::Left,
    ] {
        app.handle_key(key(k));
        seen.push(app.focus());
    }
    assert_eq!(
        seen,
        [
            Control::Output(0),
            bus(OUT_BUS, Source),
            bus(OUT_BUS, Link),
            bus(OUT_BUS, Fader(0)),
            bus(OUT_BUS, Fader(1)),
            bus(OUT_BUS, Line),
            bus(OUT_BUS, Hp),
        ]
    );
    // Linked: one fader, so → from it goes on to MONITOR MIX.
    for _ in 0..2 {
        app.handle_key(key(KeyCode::Up));
    }
    assert_eq!(app.focus(), bus(OUT_BUS, Link));
    app.handle_key(ch(' '));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.focus(), bus(OUT_BUS, Fader(0)));
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.focus(), Control::Output(1));
}

/// Loopback and output names are editable like the mixer's: Enter on the
/// strip, saved, never sent.
#[test]
fn rename_loopback_and_output() {
    let meters = fake_meters();
    let mut app = app_with(State::default());
    app.handle_key(ch('3'));
    app.handle_key(key(KeyCode::Right));
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert_eq!(app.editing().map(|(n, _)| n), Some(Name::Loopback(1)));
    for c in "OBS".chars() {
        assert!(app.handle_key(ch(c)).is_empty());
    }
    assert_eq!(app.handle_key(key(KeyCode::Enter)), [Action::Save]);
    assert_eq!(app.state().loopback_names[1].as_deref(), Some("OBS"));

    app.handle_key(ch('4'));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.editing().map(|(n, _)| n), Some(Name::Output));
    for c in "Monitors".chars() {
        app.handle_key(ch(c));
    }
    assert_eq!(app.handle_key(key(KeyCode::Enter)), [Action::Save]);
    assert_eq!(app.state().output_name.as_deref(), Some("Monitors"));
    assert!(State::default().diff(app.state()).is_empty(), "no frame");

    let (buf, _) = render(&app, &meters, 120, 40);
    let t = text(&buf);
    assert!(t.contains(" OBS ") && t.contains(" Monitors "), "{t}");
    assert!(!t.contains("LOOP 3+4") && !t.contains("OUT 1+2"), "{t}");

    // Saved and loaded; an old state has the default names.
    let dir = std::env::temp_dir().join(format!("toppingctl-outputs-{}", std::process::id()));
    let store = Store::at(dir.join("state.json"));
    store.save(app.state()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(loaded.loopback_names[1].as_deref(), Some("OBS"));
    assert_eq!(loaded.output_name.as_deref(), Some("Monitors"));
    let old: State = serde_json::from_str("{}").unwrap();
    assert_eq!(old.loopback_names, [None, None]);
    assert_eq!(old.output_name, None);
}

// ------------------------------------------------------------------ AC3

/// Device reports update the widgets with no key input: the Monitor Mix
/// knob readout (`11 01`) and IN1's MON pill (`21 01`); meter frames move
/// the output and loopback meters.
#[test]
fn live_reports_update_widgets() {
    let mut app = app_with(State::default());
    let mut meters = Meters::default();
    let (before, _) = render(&app, &meters, 120, 40);
    assert!(
        app.on_report(&frame(0x11, 0x01, 70)).is_empty(),
        "readout only"
    );
    assert_eq!(app.on_report(&frame(0x21, 0x01, 1)), [Action::Save]);
    assert!(app.state().inputs[0].mon);
    for f in [
        frame(0x31, 0x03, -60),
        frame(0x31, 0x04, -120),
        frame(0x32, 0x03, -200),
        frame(0x33, 0x04, -30),
    ] {
        assert!(meters.apply(&f));
    }
    let (after, hits) = render(&app, &meters, 120, 40);
    let t = text(&after);
    assert!(t.contains("+20"), "Monitor Mix 70 reads +20: {t}");
    let mon = hits
        .rect(Hit::Control(Control::Input {
            ch: 0,
            kind: InputControl::Mon,
        }))
        .unwrap();
    assert_ne!(
        before[(mon.x + 1, mon.y)].bg,
        after[(mon.x + 1, mon.y)].bg,
        "MON lit"
    );
    // The pre-fader meter of each moves to its reading.
    let meter_dbs = |r: Rect| -> Vec<f64> {
        hits.pixels()
            .iter()
            .filter_map(|s| match s.widget {
                PixelWidget::Meter { db } if r.contains(s.rect.as_position()) => Some(db),
                _ => None,
            })
            .collect()
    };
    let out = hits.rect(Hit::Focus(Control::Output(0))).unwrap();
    let lb0 = hits.rect(Hit::Focus(Control::Loopback(0))).unwrap();
    let lb1 = hits.rect(Hit::Focus(Control::Loopback(1))).unwrap();
    for (r, want) in [(out, -6.0), (out, -12.0), (lb0, -20.0), (lb1, -3.0)] {
        assert!(meter_dbs(r).contains(&want), "{want} in {:?}", meter_dbs(r));
    }
}

// ------------------------------------------------- meters and clip LEDs

/// Pre- and post-fader meters everywhere the app has them (each mixer
/// channel, loopback and output), each with its clip light: IN on the
/// reading, OUT after the fader; both held 5 s (`overload_IN/OUT`).
#[test]
fn pre_and_post_meters_with_clip_lights() {
    assert_eq!(CLIP_HOLD, Duration::from_secs(5));
    let mut s = State::default();
    s.output.set_level(-12.0);
    s.loopbacks[0].set_mute(true);
    let mut app = app_with(s);
    let meters = Meters {
        output: [0.0, -30.0],
        loopbacks: [[0.0, 0.0], [-96.0, -96.0]],
        ..fake_meters()
    };
    let t0 = Instant::now();
    app.tick(&meters, t0);
    let clips = app.clips();
    assert!(clips.lit(MeterId::Output(0)), "IN: the reading clips");
    assert!(
        !clips.lit_out(MeterId::Output(0)),
        "OUT: −12 dB after the fader"
    );
    assert!(!clips.lit(MeterId::Output(1)));
    assert!(clips.lit(MeterId::Loopback(0, 0)));
    assert!(!clips.lit_out(MeterId::Loopback(0, 0)), "muted");
    // Held 5 s after the last clip.
    let quiet = Meters::default();
    app.tick(&quiet, t0 + Duration::from_millis(4900));
    assert!(app.clips().lit(MeterId::Output(0)));
    app.tick(&quiet, t0 + Duration::from_millis(5100));
    assert!(!app.clips().lit(MeterId::Output(0)));

    // An OUT clip: a mixer channel pushed +6 dB from −3.
    let mut s = State::default();
    s.mixes[0].pb12.set_fader(6.0);
    let mut app = app_with(s);
    let hot = Meters {
        playback: [[-3.0, -20.0], [-96.0; 2], [-96.0; 2]],
        ..Meters::default()
    };
    app.tick(&hot, t0);
    assert!(!app.clips().lit(MeterId::Playback(0, 0)));
    assert!(app.clips().lit_out(MeterId::Playback(0, 0)));
    assert!(!app.clips().lit_out(MeterId::Playback(0, 1)));

    // Drawn: each strip's meters are pre/post pairs, each with a light,
    // in one sunken box per channel.
    let (_, hits) = render(&app, &hot, 120, 40);
    let n = |k: fn(&PixelWidget) -> bool, r: Rect| {
        hits.pixels()
            .iter()
            .filter(|s| k(&s.widget) && r.contains(s.rect.as_position()))
            .count()
    };
    let is_meter = |w: &PixelWidget| matches!(w, PixelWidget::Meter { .. });
    let is_clip = |w: &PixelWidget| matches!(w, PixelWidget::ClipLight { .. });
    let is_inset = |w: &PixelWidget| matches!(w, PixelWidget::Inset);
    for focus in [
        Control::Mixer(0),
        Control::Mixer(1),
        Control::Loopback(0),
        Control::Loopback(1),
        Control::Output(0),
    ] {
        let r = hits.rect(Hit::Focus(focus)).unwrap();
        assert_eq!(n(is_meter, r), 4, "{focus:?}");
        assert_eq!(n(is_clip, r), 4, "{focus:?}");
        assert_eq!(n(is_inset, r), 2, "{focus:?}");
    }
    // The post bar shows the fader's gain: PB1 at −3 + 6 = +3 → the top.
    let pb = hits.rect(Hit::Focus(Control::Mixer(1))).unwrap();
    let dbs: Vec<f64> = hits
        .pixels()
        .iter()
        .filter_map(|s| match s.widget {
            PixelWidget::Meter { db } if pb.contains(s.rect.as_position()) => Some(db),
            _ => None,
        })
        .collect();
    assert_eq!(dbs, [-3.0, 3.0, -20.0, -14.0], "pre, post per channel");
}

// ------------------------------------------------------------------ AC4

/// Mouse: a click on a source opens the menu and a click on an item picks
/// it; the wheel on a source cycles it; clicks flip LINK, MUTE, HP, LINE;
/// a drag moves a fader and a double-click resets it to 0 dB.
#[test]
fn mouse_on_out_loopback() {
    use BusControl::*;
    let meters = fake_meters();
    let mut app = app_with(State::default());
    let dev = Device::new();
    let t0 = Instant::now();
    let (_, hits) = render(&app, &meters, 120, 40);

    // Click LOOP 1+2's source: the menu; click IN 2.
    let src = hits.rect(Hit::Control(bus(0, Source))).unwrap();
    assert!(step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DOWN, src.x, src.y, NONE), &hits, t0)
    })
    .is_empty());
    assert!(matches!(app.modal(), Some(Modal::Source { bus: 0, .. })));
    let (buf, menu) = render(&app, &meters, 120, 40);
    let i = SOURCE_MENU
        .iter()
        .position(|&s| s == toppingctl::state::Source::In2)
        .unwrap();
    let item = menu.rect(Hit::SourceOption(i)).unwrap();
    assert!(text_in(&buf, item).contains("IN 2"), "{}", text(&buf));
    let frames = step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DOWN, item.x + 1, item.y, NONE), &menu, t0)
    });
    assert_eq!(frames, [frame(0x32, 0x05, 0x2000)]);
    assert!(app.modal().is_none());
    app.handle_mouse_at(mouse(UP, item.x + 1, item.y, NONE), &menu, t0);

    // The wheel on the output's source: the next one in the menu.
    let osrc = hits.rect(Hit::Control(bus(OUT_BUS, Source))).unwrap();
    let frames = step(&mut app, &dev, |a| {
        a.handle_mouse_at(
            mouse(MouseEventKind::ScrollDown, osrc.x, osrc.y, NONE),
            &hits,
            t0,
        )
    });
    assert_eq!(frames, [frame(0x31, 0x05, 0x20)]);
    assert!(app.modal().is_none(), "the wheel opens nothing");

    // Drag the output fader to the bottom: −∞; double-click: 0 dB.
    let f = hits.rect(Hit::Control(bus(OUT_BUS, Fader(0)))).unwrap();
    let t1 = t0 + Duration::from_secs(5);
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DOWN, f.x + 1, f.y, NONE), &hits, t1)
    });
    assert_eq!(app.focus(), bus(OUT_BUS, Fader(0)));
    let frames = step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DRAG, f.x + 1, f.bottom() + 1, NONE), &hits, t1)
    });
    assert_eq!(frames, [frame(0x31, 0x01, 0), frame(0x31, 0x02, 0)]);
    // Mid-track: a whole dB.
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DRAG, f.x + 1, f.y + f.height / 2, NONE), &hits, t1)
    });
    let db = app.state().output.level_l_db;
    assert!(db < 0.0 && db > -90.0 && db.fract() == 0.0, "{db}");
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(UP, f.x + 1, f.y, NONE), &hits, t1)
    });
    let t2 = t1 + Duration::from_secs(5);
    for dt in [0, 120] {
        step(&mut app, &dev, |a| {
            let at = t2 + Duration::from_millis(dt);
            let mut out = a.handle_mouse_at(mouse(DOWN, f.x + 1, f.y + 1, NONE), &hits, at);
            out.extend(a.handle_mouse_at(mouse(UP, f.x + 1, f.y + 1, NONE), &hits, at));
            out
        });
    }
    assert_eq!(app.state().output.level_l_db, 0.0);
    assert_eq!(app.state().output.level_r_db, 0.0);
    // The wheel on a fader: 1 dB.
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(MouseEventKind::ScrollDown, f.x, f.y, NONE), &hits, t2)
    });
    assert_eq!(app.state().output.level_l_db, -1.0);

    // Clicks on the pills.
    let t3 = t2 + Duration::from_secs(5);
    let mut i = 0;
    for c in [
        bus(0, Mute(0)),
        bus(1, Mute(0)),
        bus(OUT_BUS, Hp),
        bus(OUT_BUS, Line),
        bus(0, Link),
    ] {
        let r = hits
            .rect(Hit::Control(c))
            .unwrap_or_else(|| panic!("{c:?}"));
        assert_eq!(hits.get(r.x, r.y), Some(Hit::Control(c)));
        assert_eq!(hits.get(r.right() - 1, r.y), Some(Hit::Control(c)));
        i += 1;
        let at = t3 + Duration::from_secs(i);
        let before = app.state().clone();
        step(&mut app, &dev, |a| {
            a.handle_mouse_at(mouse(DOWN, r.x, r.y, NONE), &hits, at)
        });
        assert_ne!(&before, app.state(), "{c:?} flips");
        assert_eq!(app.focus(), c);
    }
    assert!(!app.state().loopbacks[0].link);
    // Unlinked: the new page's MUTE R and R fader are clickable.
    let (_, hits) = render(&app, &meters, 120, 40);
    let r = hits.rect(Hit::Control(bus(0, Mute(1)))).unwrap();
    let frames = step(&mut app, &dev, |a| {
        a.handle_mouse_at(
            mouse(DOWN, r.x, r.y, NONE),
            &hits,
            t3 + Duration::from_secs(20),
        )
    });
    // MUTE was on for both (clicked linked); R alone comes back.
    assert_eq!(frames, [frame(0x32, 0x02, lvl(0.0))]);
    assert!(hits.rect(Hit::Control(bus(0, Fader(1)))).is_some());
    dev.written();
}

// ------------------------------------------------------------------ AC5

/// A state with something in each loopback and output control.
fn fake_state() -> State {
    let mut s = State::default();
    s.loopbacks[0].set_level(-6.0);
    s.loopbacks[1].source = toppingctl::state::Source::In12;
    s.loopbacks[1].set_mute(true);
    s.output.source = toppingctl::state::Source::MixB;
    s.output.set_level(-12.0);
    s
}

/// The full screen, an open source menu, every strip unlinked (split
/// faders) and the "outputs off" hint, at 120×40.
#[test]
fn snapshots() {
    let meters = fake_meters();
    let mut app = app_with(fake_state());
    let (buf, _) = render(&app, &meters, 120, 40);
    let t = text(&buf);
    assert!(!t.contains("outputs off"));
    assert_snapshot("outputs_120x40", &t);

    focus(&mut app, bus(1, BusControl::Source));
    app.handle_key(key(KeyCode::Enter));
    let (buf, _) = render(&app, &meters, 120, 40);
    let t = text(&buf);
    for item in ["Mixer", "Input", "Playback", "Mix A", "IN1+2", "PB 5/6"] {
        assert!(t.contains(item), "{item}: {t}");
    }
    assert_snapshot("outputs_source_menu_120x40", &t);
    app.handle_key(key(KeyCode::Esc));

    let mut s = fake_state();
    s.loopbacks[0].link = false;
    s.loopbacks[0].level_r_db = -24.0;
    s.loopbacks[1].link = false;
    s.loopbacks[1].mute_l = false;
    s.output.link = false;
    s.output.level_l_db = f64::NEG_INFINITY;
    s.mixes[0].link = true;
    for m in &mut s.mixes[0..1] {
        m.pb12.link = false;
        m.pb12.fader_r_db = -6.0;
        m.pb34.link = false;
        m.pb56.link = false;
        m.pb56.mute_r = true;
    }
    let app = app_with(s.clone());
    let (buf, hits) = render(&app, &meters, 120, 40);
    assert_snapshot("outputs_unlinked_120x40", &text(&buf));
    for b in [0, 1, OUT_BUS] {
        for c in [BusControl::Fader(0), BusControl::Fader(1)] {
            assert!(hits.rect(Hit::Control(bus(b, c))).is_some(), "{b} {c:?}");
        }
    }

    let mut s = fake_state();
    s.output.headphones = false;
    s.output.line = false;
    let app = app_with(s);
    let (buf, _) = render(&app, &meters, 120, 40);
    let t = text(&buf);
    assert!(t.contains("outputs off"), "{t}");
    assert_snapshot("outputs_off_120x40", &t);
}

/// The swapped pages keep #14's layout rules at every size: every control
/// of the loopback and output strips is a hit rect that answers for its
/// own cells; faders have pixels over their track; nothing overlaps.
#[test]
fn swapped_pages_lay_out_at_every_size() {
    let meters = fake_meters();
    let mut s = fake_state();
    for lb in &mut s.loopbacks {
        lb.link = false;
    }
    s.output.link = false;
    s.mixes[0].link = true;
    s.mixes[0].pb12.link = false;
    for state in [fake_state(), s] {
        let app = app_with(state.clone());
        for (w, h) in [(100u16, 34u16), (120, 40), (160, 50), (250, 60)] {
            let (buf, hits) = render(&app, &meters, w, h);
            let ctx = format!("{w}x{h}:\n{}", text(&buf));
            for b in [0, 1, OUT_BUS] {
                let linked = if b == OUT_BUS {
                    state.output.link
                } else {
                    state.loopbacks[b].link
                };
                let mut ctls = vec![BusControl::Source, BusControl::Link, BusControl::Fader(0)];
                if b == OUT_BUS {
                    ctls.extend([BusControl::Hp, BusControl::Line]);
                } else {
                    ctls.push(BusControl::Mute(0));
                }
                if !linked {
                    ctls.push(BusControl::Fader(1));
                    if b != OUT_BUS {
                        ctls.push(BusControl::Mute(1));
                    }
                }
                let mut rects = Vec::new();
                for c in ctls {
                    let hit = Hit::Control(bus(b, c));
                    let r = hits.rect(hit).unwrap_or_else(|| panic!("{b} {c:?}: {ctx}"));
                    for (x, y) in [(r.x, r.y), (r.right() - 1, r.bottom() - 1)] {
                        assert_eq!(hits.get(x, y), Some(hit), "{b} {c:?} at {x},{y}: {ctx}");
                    }
                    if let BusControl::Fader(_) = c {
                        assert!(
                            hits.pixels().iter().any(|s| matches!(
                                s.widget,
                                PixelWidget::Fader { max_db, .. } if max_db == 0.0
                            ) && s.rect.y == r.y
                                && s.rect.height == r.height),
                            "{b} {c:?}: {ctx}"
                        );
                    }
                    rects.push(r);
                }
                for (i, a) in rects.iter().enumerate() {
                    for b2 in &rects[i + 1..] {
                        assert!(!a.intersects(*b2), "{a:?} {b2:?}: {ctx}");
                    }
                }
            }
        }
    }
}
