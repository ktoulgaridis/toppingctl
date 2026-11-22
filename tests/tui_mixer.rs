//! The mixer panel's controls (#14): every Mix A/B/C control, from a key or
//! the mouse, through `State::diff` and a real `Connection` over
//! `FakeTransport`; strip names; the mixer snapshots; and the content-sized
//! layout of every panel (AC7). No terminal, no hardware.
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
use toppingctl::state::{Meters, State, CROSSPOINT_ZERO_DB};
use toppingctl::sync::Store;
use toppingctl::tui::app::{Action, App, Control, MixControl};
use toppingctl::tui::pixel::PixelWidget;
use toppingctl::tui::run::send_actions;
use toppingctl::tui::theme::Theme;
use toppingctl::tui::ui::{self, Hit, HitMap};

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

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn mix(strip: usize, ctl: MixControl) -> Control {
    Control::Mix { strip, ctl }
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

/// A state with something different in each mix, for the snapshots.
fn fake_state() -> State {
    let mut s = State::default();
    s.inputs[0].gain_db = 12;
    s.inputs[1].gain_db = 4;
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
    let b = &mut s.mixes[1];
    b.link = true;
    b.in1.fader_db = -6.0;
    b.pb12.fader_db = 4.0;
    b.pb12.phase = true;
    let c = &mut s.mixes[2];
    c.in2.mute = true;
    c.pb56.fader_db = -24.0;
    c.pb56.pan_l = 0.5;
    c.pb56.pan_r = 0.5;
    s
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
/// `State::diff` from the state before to the state after, followed by a
/// save when anything changed. Forwards the actions to `dev`.
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

/// Moves focus to `target` with keys only: `2` (the mixer), → to the
/// strip, ↓ into the strip's controls.
fn focus(app: &mut App, target: Control) {
    let Control::Mix { strip, .. } = target else {
        panic!("not a mixer control: {target:?}");
    };
    assert!(app.handle_key(ch('2')).is_empty());
    assert_eq!(app.focus(), Control::Mixer(0));
    for _ in 0..strip {
        assert!(app.handle_key(key(KeyCode::Right)).is_empty());
    }
    assert_eq!(app.focus(), Control::Mixer(strip));
    for _ in 0..12 {
        if app.focus() == target {
            return;
        }
        assert!(app.handle_key(key(KeyCode::Down)).is_empty());
    }
    assert_eq!(app.focus(), target, "↓ never reached it");
}

/// The app's crosspoint level for a fader at `db`, from PROTOCOL.md's
/// rule: `round(335544 · 10^(dB/20)) · 100`.
fn xp(db: f64) -> i32 {
    ((335_544.0 * 10f64.powf(db / 20.0)).round() as i32) * 100
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

// ------------------------------------------------------------------ AC1

/// `a`/`b`/`c` and a click on a tab switch the mix shown; a control then
/// writes that mix's crosspoints (51 / 52 / 53).
#[test]
fn mix_selection() {
    let mut app = app_with(State::default());
    let dev = Device::new();
    focus(&mut app, mix(0, MixControl::Fader(0)));
    for (k, m, group) in [('b', 1, 0x52), ('c', 2, 0x53), ('a', 0, 0x51)] {
        assert!(app.handle_key(ch(k)).is_empty(), "switching sends nothing");
        assert_eq!(app.mix(), m);
        assert_eq!(app.focus(), mix(0, MixControl::Fader(0)), "focus stays");
        let frames = step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Left)));
        assert!(!frames.is_empty());
        assert!(frames.iter().all(|f| f.group == group), "{frames:?}");
        assert_eq!(app.state().mixes[m].in1.fader_db, -1.0);
    }
    // The tabs in the border.
    let meters = fake_meters();
    for m in [1, 2, 0] {
        let (_, hits) = render(&app, &meters, 120, 40);
        let r = hits.rect(Hit::MixTab(m)).expect("a tab");
        let now = Instant::now();
        assert!(app
            .handle_mouse_at(mouse(DOWN, r.x, r.y, NONE), &hits, now)
            .is_empty());
        app.handle_mouse_at(mouse(UP, r.x, r.y, NONE), &hits, now);
        assert_eq!(app.mix(), m);
        let (buf, hits) = render(&app, &meters, 120, 40);
        let f = hits
            .rect(Hit::Control(mix(1, MixControl::Fader(0))))
            .expect("PB 1/2 fader");
        let frames = step(&mut app, &dev, |a| {
            a.handle_mouse_at(
                mouse(MouseEventKind::ScrollDown, f.x + 1, f.y + 1, NONE),
                &hits,
                Instant::now(),
            )
        });
        assert!(frames.iter().all(|fr| fr.group == 0x51 + m as u8));
        assert!(!frames.is_empty());
        assert!(text(&buf).contains(["┤a mix A├", "┤b mix B├", "┤c mix C├"][m]));
    }
    assert!(!dev.written().is_empty());
}

// ------------------------------------------------------------------ AC2

/// Every case of the validation table, plus the rest of the controls,
/// end to end: the frames sent equal `State::diff`, and the crosspoint
/// values follow PROTOCOL.md (fader level, linear pan split, solo, mute,
/// Ø, link).
#[test]
fn controls_send_expected_frames() {
    use MixControl::*;
    let zero = CROSSPOINT_ZERO_DB;
    assert_eq!(xp(0.0), zero);

    // Mix B IN1 fader to −6 dB: six steps of 1 dB. IN1 is hard L by
    // default, so only `52 11` moves.
    let mut app = app_with(State::default());
    let dev = Device::new();
    app.handle_key(ch('b'));
    focus(&mut app, mix(0, Fader(0)));
    let mut all = Vec::new();
    for _ in 0..6 {
        all.extend(step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Left))));
    }
    assert_eq!(app.state().mixes[1].in1.fader_db, -6.0);
    assert_eq!(all.last(), Some(&frame(0x52, 0x11, xp(-6.0))));
    assert!(all.iter().all(|f| (f.group, f.param) == (0x52, 0x11)));
    assert_eq!(dev.written(), all, "the connection wrote them, in order");

    // Mix A PB3/4 left pan to full L with LINK off: the right pan stays.
    let mut s = State::default();
    s.mixes[0].pb34.pan_l = 0.5;
    s.mixes[0].pb34.pan_r = 0.5;
    s.mixes[0].pb34.link = false;
    let mut app = app_with(s.clone());
    let dev = Device::new();
    focus(&mut app, mix(2, Pan(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(shift(KeyCode::Left)));
    let pb = &app.state().mixes[0].pb34;
    assert_eq!((pb.pan_l, pb.pan_r), (0.0, 0.5));
    let half = zero - (f64::from(zero) * 0.5).round() as i32;
    assert_eq!(half, 0x00ff_fff0, "centre is a linear split");
    assert_eq!(frames, [frame(0x51, 0x15, zero), frame(0x51, 0x25, 0)]);
    assert_eq!(dev.written(), frames);

    // The same with LINK on: both pans move, mirrored.
    s.mixes[0].pb34.link = true;
    let mut app = app_with(s);
    let dev = Device::new();
    focus(&mut app, mix(2, Pan(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(shift(KeyCode::Left)));
    let pb = &app.state().mixes[0].pb34;
    assert_eq!((pb.pan_l, pb.pan_r), (0.0, 1.0));
    assert_eq!(
        frames,
        [
            frame(0x51, 0x15, zero),
            frame(0x51, 0x25, 0),
            frame(0x51, 0x16, 0),
            frame(0x51, 0x26, zero),
        ]
    );
    // And the right dial drives the left one.
    let frames = step(&mut app, &dev, |a| {
        a.handle_key(key(KeyCode::Up));
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down));
        assert_eq!(a.focus(), mix(2, Pan(1)));
        a.handle_key(key(KeyCode::Left))
    });
    let pb = &app.state().mixes[0].pb34;
    assert!((pb.pan_r - 0.95).abs() < 1e-9 && (pb.pan_l - 0.05).abs() < 1e-9);
    assert_eq!(frames.len(), 4, "{frames:?}");
    dev.written();

    // Mix C IN2 mute: IN2 is hard R, so only `53 22` drops to 0.
    let mut app = app_with(State::default());
    let dev = Device::new();
    app.handle_key(ch('c'));
    focus(&mut app, mix(0, Mute(1)));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert!(app.state().mixes[2].in2.mute);
    assert_eq!(frames, [frame(0x53, 0x22, 0)]);
    assert_eq!(dev.written(), frames);

    // Solo PB1/2 in Mix A: every other strip of mix A goes to 0.
    let mut app = app_with(State::default());
    let dev = Device::new();
    focus(&mut app, mix(1, Solo(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert!(app.state().mixes[0].pb12.solo);
    assert_eq!(
        frames,
        [
            frame(0x51, 0x11, 0),
            frame(0x51, 0x22, 0),
            frame(0x51, 0x15, 0),
            frame(0x51, 0x26, 0),
            frame(0x51, 0x17, 0),
            frame(0x51, 0x28, 0),
        ]
    );
    // Solo off again restores them.
    let back = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert_eq!(back.len(), 6);
    assert!(back.iter().all(|f| f.value == zero));
    dev.written();

    // IN 1+2 LINK on: IN1 hard L, IN2 hard R, whatever their pans.
    let mut s = State::default();
    s.mixes[0].in1.pan = 0.5;
    s.mixes[0].in2.pan = 0.5;
    let mut app = app_with(s);
    let dev = Device::new();
    focus(&mut app, mix(0, Link));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert!(app.state().mixes[0].link);
    assert_eq!(
        frames,
        [
            frame(0x51, 0x11, zero),
            frame(0x51, 0x21, 0),
            frame(0x51, 0x12, 0),
            frame(0x51, 0x22, zero),
        ]
    );
    // While linked, the IN dials don't move (the link overrides them).
    focus(&mut app, mix(0, Pan(0)));
    assert!(step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Right))).is_empty());
    assert_eq!(dev.written(), frames);

    // PB LINK has no frames of its own: only a save.
    let mut app = app_with(State::default());
    focus(&mut app, mix(3, Link));
    let actions = app.handle_key(ch(' '));
    assert_eq!(actions, [Action::Save]);
    assert!(!app.state().mixes[0].pb56.link);

    // Ø negates the strip's levels; fader +6 dB caps at the app's top;
    // below −60 dB snaps to −∞; `0` resets to 0 dB.
    let mut app = app_with(State::default());
    let dev = Device::new();
    focus(&mut app, mix(1, Phase(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Enter)));
    assert_eq!(frames, [frame(0x51, 0x13, -zero), frame(0x51, 0x24, -zero)]);
    focus(&mut app, mix(1, Fader(0)));
    for _ in 0..8 {
        step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Right)));
    }
    assert_eq!(app.state().mixes[0].pb12.fader_db, 6.0, "clamped at +6");
    for _ in 0..7 {
        step(&mut app, &dev, |a| a.handle_key(shift(KeyCode::Left)));
    }
    assert_eq!(app.state().mixes[0].pb12.fader_db, f64::NEG_INFINITY);
    let up = step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Right)));
    assert_eq!(app.state().mixes[0].pb12.fader_db, -59.0, "−∞ +1 dB");
    assert_eq!(up[0], frame(0x51, 0x13, -xp(-59.0)));
    step(&mut app, &dev, |a| a.handle_key(ch('0')));
    assert_eq!(app.state().mixes[0].pb12.fader_db, 0.0);
    // `0` on a dial: its default pan (PB odd channel hard L).
    focus(&mut app, mix(1, Pan(0)));
    step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Right)));
    assert!(app.state().mixes[0].pb12.pan_l > 0.0);
    step(&mut app, &dev, |a| a.handle_key(ch('0')));
    assert_eq!(app.state().mixes[0].pb12.pan_l, 0.0);
    assert_eq!(app.state().mixes[0].pb12.pan_r, 1.0);
    let written = dev.written();
    assert!(written.iter().all(|f| f.group == 0x51), "{written:?}");
    // The device ends where the state says.
    let mut regs = std::collections::HashMap::new();
    for f in State::default().frames().iter().chain(&written) {
        regs.insert((f.group, f.param), f.value);
    }
    for f in app.state().frames() {
        assert_eq!(regs[&(f.group, f.param)], f.value, "{f:?}");
    }
}

/// Focus walks the strips, then the controls inside a strip, in the
/// spec's order; ←→ beside a toggle moves to the next strip.
#[test]
fn focus_walks_strips_then_controls() {
    use MixControl::*;
    let mut app = app_with(State::default());
    app.handle_key(ch('2'));
    app.handle_key(key(KeyCode::Right));
    let mut seen = vec![app.focus()];
    for _ in 0..8 {
        app.handle_key(key(KeyCode::Down));
        seen.push(app.focus());
    }
    assert_eq!(
        seen,
        [
            Control::Mixer(1),
            mix(1, Pan(0)),
            mix(1, Pan(1)),
            mix(1, Fader(0)),
            mix(1, Link),
            mix(1, Phase(0)),
            mix(1, Solo(0)),
            mix(1, Mute(0)),
            mix(1, Mute(0)),
        ]
    );
    // ←→ on a toggle: the same control on the strip beside.
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.focus(), mix(2, Mute(0)));
    app.handle_key(key(KeyCode::Left));
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.focus(), mix(0, Mute(0)));
    // ↑ all the way leaves the controls for the strip name.
    for _ in 0..12 {
        app.handle_key(key(KeyCode::Up));
    }
    assert_eq!(app.focus(), Control::Mixer(0));
    // IN 1+2 has two of each channel control.
    let mut seen = vec![];
    for _ in 0..11 {
        app.handle_key(key(KeyCode::Down));
        seen.push(app.focus());
    }
    assert_eq!(
        seen,
        [
            mix(0, Pan(0)),
            mix(0, Pan(1)),
            mix(0, Fader(0)),
            mix(0, Fader(1)),
            mix(0, Link),
            mix(0, Phase(0)),
            mix(0, Phase(1)),
            mix(0, Solo(0)),
            mix(0, Solo(1)),
            mix(0, Mute(0)),
            mix(0, Mute(1)),
        ]
    );
    // From IN2's mute to PB 1/2: its only mute.
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.focus(), mix(1, Mute(0)));
}

// ------------------------------------------------------------------ AC3

/// Mouse on the strips: a drag to the top of a fader gives +6 dB
/// (clamped), the wheel on a dial steps 5 % (⇧ ×10), a double-click resets
/// a fader to 0 dB and a dial to its default; clicks flip the pills.
#[test]
fn mouse_on_strips() {
    use MixControl::*;
    let meters = fake_meters();
    let mut app = app_with(State::default());
    let dev = Device::new();
    let t0 = Instant::now();
    let (_, hits) = render(&app, &meters, 120, 40);

    // Drag the PB 1/2 fader from its cap to the top, then past it.
    let f = hits.rect(Hit::Control(mix(1, Fader(0)))).unwrap();
    let cap = f.y + (f.height - 1) / 2;
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DOWN, f.x + 1, cap, NONE), &hits, t0)
    });
    assert_eq!(app.focus(), mix(1, Fader(0)));
    assert_eq!(
        app.state().mixes[0].pb12.fader_db,
        0.0,
        "a press moves nothing"
    );
    let frames = step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DRAG, f.x + 1, f.y, NONE), &hits, t0)
    });
    assert_eq!(app.state().mixes[0].pb12.fader_db, 6.0);
    assert!(!frames.is_empty());
    let frames = step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DRAG, f.x + 1, f.y.saturating_sub(3), NONE), &hits, t0)
    });
    assert!(frames.is_empty(), "clamped at +6 dB");
    // To the bottom: −∞.
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DRAG, f.x + 1, f.bottom() + 2, NONE), &hits, t0)
    });
    assert_eq!(app.state().mixes[0].pb12.fader_db, f64::NEG_INFINITY);
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(UP, f.x + 1, f.bottom(), NONE), &hits, t0)
    });
    // A drag after the release does nothing.
    assert!(step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DRAG, f.x + 1, f.y, NONE), &hits, t0)
    })
    .is_empty());

    // Double-click on the fader: 0 dB.
    let t1 = t0 + Duration::from_secs(5);
    for dt in [0, 100] {
        step(&mut app, &dev, |a| {
            let at = t1 + Duration::from_millis(dt);
            let mut out = a.handle_mouse_at(mouse(DOWN, f.x + 1, f.y + 2, NONE), &hits, at);
            out.extend(a.handle_mouse_at(mouse(UP, f.x + 1, f.y + 2, NONE), &hits, at));
            out
        });
    }
    assert_eq!(app.state().mixes[0].pb12.fader_db, 0.0);

    // The wheel on the PB 3/4 left dial: 5 %, ⇧ ×10. LINK is on by
    // default, so the right dial mirrors it.
    let d = hits.rect(Hit::Control(mix(2, Pan(0)))).unwrap();
    let up = MouseEventKind::ScrollUp;
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(up, d.x + 1, d.y + 1, NONE), &hits, t1)
    });
    let pb = app.state().mixes[0].pb34.clone();
    assert!((pb.pan_l - 0.05).abs() < 1e-9, "{pb:?}");
    assert!((pb.pan_r - 0.95).abs() < 1e-9, "{pb:?}");
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(up, d.x + 1, d.y + 1, KeyModifiers::SHIFT), &hits, t1)
    });
    assert!((app.state().mixes[0].pb34.pan_l - 0.55).abs() < 1e-9);
    // Drag a dial up: one step a row.
    let t2 = t1 + Duration::from_secs(5);
    step(&mut app, &dev, |a| {
        let mut out = a.handle_mouse_at(mouse(DOWN, d.x + 1, d.y + 2, NONE), &hits, t2);
        out.extend(a.handle_mouse_at(mouse(DRAG, d.x + 1, d.y, NONE), &hits, t2));
        out.extend(a.handle_mouse_at(mouse(UP, d.x + 1, d.y, NONE), &hits, t2));
        out
    });
    assert!((app.state().mixes[0].pb34.pan_l - 0.65).abs() < 1e-9);
    // Double-click resets the dial (and, linked, its partner).
    let t3 = t2 + Duration::from_secs(5);
    for dt in [0, 150] {
        step(&mut app, &dev, |a| {
            let at = t3 + Duration::from_millis(dt);
            let mut out = a.handle_mouse_at(mouse(DOWN, d.x + 1, d.y + 1, NONE), &hits, at);
            out.extend(a.handle_mouse_at(mouse(UP, d.x + 1, d.y + 1, NONE), &hits, at));
            out
        });
    }
    let pb = &app.state().mixes[0].pb34;
    assert_eq!((pb.pan_l, pb.pan_r), (0.0, 1.0));

    // The wheel on a fader: 1 dB, ⇧ 10.
    let f2 = hits.rect(Hit::Control(mix(0, Fader(1)))).unwrap();
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(
            mouse(MouseEventKind::ScrollDown, f2.x, f2.y, KeyModifiers::SHIFT),
            &hits,
            t3,
        )
    });
    assert_eq!(app.state().mixes[0].in2.fader_db, -10.0);

    // Clicks on the pills: LINK, Ø, SOLO, MUTE of every strip.
    let t4 = t3 + Duration::from_secs(5);
    let mut i = 0u64;
    for strip in 0..4 {
        let chans = if strip == 0 { 2 } else { 1 };
        let mut ctls = vec![Link];
        for c in 0..chans {
            ctls.extend([Phase(c), Solo(c), Mute(c)]);
        }
        for ctl in ctls {
            let r = hits
                .rect(Hit::Control(mix(strip, ctl)))
                .unwrap_or_else(|| panic!("{strip} {ctl:?}"));
            assert_eq!(hits.get(r.x, r.y), Some(Hit::Control(mix(strip, ctl))));
            i += 1;
            let at = t4 + Duration::from_secs(i);
            let before = app.state().clone();
            step(&mut app, &dev, |a| {
                a.handle_mouse_at(mouse(DOWN, r.x, r.y, NONE), &hits, at)
            });
            assert_ne!(&before, app.state(), "{strip} {ctl:?} flips");
            assert_eq!(app.focus(), mix(strip, ctl));
            app.handle_mouse_at(mouse(UP, r.x, r.y, NONE), &hits, at);
        }
    }
    let written = dev.written();
    assert!(!written.is_empty());
    assert!(written.iter().all(|f| f.group == 0x51));
}

// ------------------------------------------------------------------ AC4

/// Enter on a strip name edits it: "DAW" shows in the strip's header,
/// survives a save and load, and sends no frame.
#[test]
fn rename_strip() {
    let meters = fake_meters();
    let mut app = app_with(State::default());
    app.handle_key(ch('2'));
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.focus(), Control::Mixer(1));
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert!(app.editing().is_some(), "editing the name");
    let mut actions = Vec::new();
    // Keys go to the name, not to the app: `q`, `a`, `2` are letters here.
    for c in "Xq".chars() {
        actions.extend(app.handle_key(ch(c)));
    }
    actions.extend(app.handle_key(key(KeyCode::Backspace)));
    actions.extend(app.handle_key(key(KeyCode::Backspace)));
    for c in "DAW".chars() {
        actions.extend(app.handle_key(ch(c)));
    }
    assert!(actions.is_empty(), "nothing until Enter: {actions:?}");
    assert_eq!(app.mix(), 0);
    let (buf, _) = render(&app, &meters, 120, 40);
    assert!(text(&buf).contains("DAW"), "shown while editing");
    let actions = app.handle_key(key(KeyCode::Enter));
    assert_eq!(actions, [Action::Save], "a save, no frame");
    assert!(app.editing().is_none());
    assert_eq!(app.state().strip_names[1].as_deref(), Some("DAW"));
    let (buf, _) = render(&app, &meters, 120, 40);
    let t = text(&buf);
    assert!(t.contains(" DAW ") && !t.contains("PB 1/2"), "{t}");
    // The same name in every mix.
    app.handle_key(ch('c'));
    let (buf, _) = render(&app, &meters, 120, 40);
    assert!(text(&buf).contains(" DAW "));
    // No frame for a name.
    assert!(State::default().diff(app.state()).is_empty());

    // Save and load.
    let dir = std::env::temp_dir().join(format!("toppingctl-mixer-{}", std::process::id()));
    let store = Store::at(dir.join("state.json"));
    store.save(app.state()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.strip_names[1].as_deref(), Some("DAW"));
    let _ = fs::remove_dir_all(&dir);
    let back = app_with(loaded);
    let (buf, _) = render(&back, &meters, 120, 40);
    assert!(text(&buf).contains(" DAW "));

    // Esc cancels; an empty name goes back to the default.
    let mut app = back;
    app.handle_key(ch('2'));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(ch('Z'));
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert_eq!(app.state().strip_names[1].as_deref(), Some("DAW"));
    app.handle_key(key(KeyCode::Enter));
    for _ in 0..5 {
        app.handle_key(key(KeyCode::Backspace));
    }
    assert_eq!(app.handle_key(key(KeyCode::Enter)), [Action::Save]);
    assert_eq!(app.state().strip_names[1], None);
    let (buf, _) = render(&app, &meters, 120, 40);
    assert!(text(&buf).contains("PB 1/2"));

    // Names are one cell a character (the strip title is laid out by
    // cells): wide characters are refused, and the length is capped.
    app.handle_key(key(KeyCode::Enter));
    for c in "漢😀ABCDEFGHIJKL".chars() {
        app.handle_key(ch(c));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.state().strip_names[1].as_deref(), Some("ABCDEFGHIJ"));

    // An old saved state (no names) loads with the defaults.
    let old: State = serde_json::from_str("{}").unwrap();
    assert_eq!(old.strip_names, [None, None, None, None]);
}

// ------------------------------------------------------------------ AC5

/// Mix A, B and C at 120×40, and one with no meter data (no bars).
#[test]
fn snapshots() {
    let meters = fake_meters();
    let mut app = app_with(fake_state());
    for (k, name) in [('a', "mixer_a"), ('b', "mixer_b"), ('c', "mixer_c")] {
        app.handle_key(ch(k));
        let (buf, _) = render(&app, &meters, 120, 40);
        assert_snapshot(&format!("{name}_120x40"), &text(&buf));
    }
    app.handle_key(ch('a'));
    let (buf, _) = render(&app, &Meters::default(), 120, 40);
    let t = text(&buf);
    assert_snapshot("mixer_no_meters_120x40", &t);
    for bar in ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"] {
        let rows: Vec<&str> = t.lines().skip(4).collect();
        assert!(
            rows.iter().all(|l| !l.contains(bar)),
            "{bar} drawn with no meter data:\n{t}"
        );
    }
}

// ------------------------------------------------------------------ AC7

fn kind(w: &PixelWidget) -> &'static str {
    match w {
        PixelWidget::Knob { .. } => "knob",
        PixelWidget::Fader { .. } => "fader",
        PixelWidget::Meter { .. } => "meter",
        PixelWidget::ClipLight { .. } => "clip",
        PixelWidget::Chip { .. } => "chip",
    }
}

/// A large terminal does not stretch the strips: the console keeps the
/// app's proportions, capped and centred; faders and meters stop growing;
/// each strip's knob, pills, value and fader stay one compact group; the
/// pixel widgets sit in the new rects and the hit map stays exact.
#[test]
fn layout_is_content_sized() {
    use MixControl::*;
    let app = app_with(fake_state());
    let meters = fake_meters();
    let (buf, hits) = render(&app, &meters, 250, 60);
    let t = text(&buf);
    assert_snapshot("layout_250x60", &t);

    // The console is capped and centred.
    let (mw, mh) = (ui::MAX_WIDTH, ui::MAX_HEIGHT);
    assert!(mw < 250 && mh <= 60);
    let cols: Vec<u16> = (0..250u16)
        .filter(|&x| (0..60u16).any(|y| buf[(x, y)].symbol() != " "))
        .collect();
    let (x0, x1) = (cols[0], *cols.last().unwrap());
    assert!(x1 - x0 < mw, "{x0}..{x1}");
    assert!(x0.abs_diff(249 - x1) <= 1, "centred: {x0}..{x1}");

    // Faders and meters are capped.
    for spot in hits.pixels() {
        match kind(&spot.widget) {
            "fader" => assert!(spot.rect.height <= ui::FADER_MAX_ROWS, "{spot:?}"),
            "meter" => assert!(spot.rect.height <= ui::METER_MAX_ROWS, "{spot:?}"),
            _ => {}
        }
    }

    // A mix strip's dials, LINK, value and fader stay together: the fader
    // starts within 3 rows of the LINK pill, and LINK within 2 of the dials.
    for strip in 0..4 {
        let dial = hits.rect(Hit::Control(mix(strip, Pan(0)))).unwrap();
        let link = hits.rect(Hit::Control(mix(strip, Link))).unwrap();
        let fader = hits.rect(Hit::Control(mix(strip, Fader(0)))).unwrap();
        let mute = hits.rect(Hit::Control(mix(strip, Mute(0)))).unwrap();
        assert!(
            link.y <= dial.bottom() + 1,
            "strip {strip}: {dial:?} {link:?}"
        );
        assert!(fader.y <= link.y + 3, "strip {strip}: {link:?} {fader:?}");
        assert!(
            mute.y <= fader.bottom() + 3,
            "strip {strip}: {fader:?} {mute:?}"
        );
    }
    // An input's gain knob sits right under its switches.
    for c in 0..2 {
        use toppingctl::tui::app::InputControl;
        let phase = hits
            .rect(Hit::Control(Control::Input {
                ch: c,
                kind: InputControl::Phase,
            }))
            .unwrap();
        let knob = hits
            .rect(Hit::Control(Control::Input {
                ch: c,
                kind: InputControl::Gain,
            }))
            .unwrap();
        assert!(knob.y <= phase.y + 2, "IN{}: {phase:?} {knob:?}", c + 1);
        let gain = hits
            .pixels()
            .iter()
            .find(|s| kind(&s.widget) == "knob" && knob.contains(s.rect.as_position()))
            .expect("gain knob pixels in its hit rect");
        assert!(gain.rect.y <= phase.y + 3);
    }
    // No box is mostly empty: PHONE is no wider than its content needs.
    let phone = hits.rect(Hit::Focus(Control::Output(2))).unwrap();
    assert!(phone.width <= 32, "{phone:?}");

    // Every pixel widget is inside the console; every control's hit rect
    // answers for its own cells (the hit map stays exact).
    let console = Rect::new(x0, 0, x1 + 1 - x0, 60);
    for spot in hits.pixels() {
        assert!(console.contains(spot.rect.as_position()), "{spot:?}");
    }
    for strip in 0..4 {
        let chans = if strip == 0 { 2 } else { 1 };
        let mut ctls = vec![Pan(0), Pan(1), Link];
        for c in 0..chans {
            ctls.extend([Fader(c), Phase(c), Solo(c), Mute(c)]);
        }
        for ctl in ctls {
            let hit = Hit::Control(mix(strip, ctl));
            let r = hits.rect(hit).unwrap_or_else(|| panic!("{strip} {ctl:?}"));
            for (x, y) in [(r.x, r.y), (r.right() - 1, r.bottom() - 1)] {
                assert_eq!(hits.get(x, y), Some(hit), "{strip} {ctl:?} at {x},{y}");
            }
        }
        // The fader's pixels are exactly over its hit rect's track.
        let f = hits.rect(Hit::Control(mix(strip, Fader(0)))).unwrap();
        assert!(hits
            .pixels()
            .iter()
            .any(|s| kind(&s.widget) == "fader" && s.rect.y == f.y && s.rect.height == f.height));
    }

    // At 120×40 nothing is capped: the console fills the terminal.
    let (buf, _) = render(&app, &meters, 120, 40);
    assert!(buf[(0, 0)].symbol() == "╭" && buf[(119, 39 - 1)].symbol() != " ");
}

// ------------------------------------------------ responsiveness (#37)

fn kitty_layer() -> toppingctl::tui::pixel::PixelLayer {
    use ratatui_image::picker::{Picker, ProtocolType};
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(ProtocolType::Kitty);
    toppingctl::tui::pixel::PixelLayer::new(picker)
}

/// The event loop's own steps, in a kitty terminal, after startup: draw a
/// frame (text plus images), take the queued input, send the frames. Keys
/// and clicks on the mixer change the state and reach the device within a
/// tick or two; an idle frame writes no image bytes, in the centred
/// layout of a large terminal too; a press and release never leave a drag
/// behind that eats the next click.
#[test]
fn event_loop_stays_responsive() {
    use crossterm::event::Event;
    use toppingctl::tui::run::{draw_frame, process_input};
    use MixControl::*;
    for (w, h) in [(250u16, 60u16), (120, 40)] {
        let mut app = app_with(State::default());
        let meters = fake_meters();
        let dev = Device::new();
        let mut layer = kitty_layer();
        layer.set_resize_settle(Duration::ZERO);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut tick = |app: &App| -> (HitMap, usize) {
            let mut images = Vec::new();
            let hits =
                draw_frame(&mut terminal, app, &meters, Some(&mut layer), &mut images).unwrap();
            (hits, images.len())
        };
        // Startup: images go out until everything is placed, then idle
        // frames write nothing.
        let mut idle = usize::MAX;
        for _ in 0..40 {
            idle = tick(&app).1;
            if idle == 0 {
                break;
            }
        }
        assert_eq!(idle, 0, "{w}x{h}: an unchanged frame writes no image bytes");
        let (hits, n) = tick(&app);
        assert_eq!(n, 0, "{w}x{h}: still idle");

        let feed = |app: &mut App, hits: &HitMap, evs: Vec<Event>| {
            let mut it = evs.into_iter();
            let mut actions = Vec::new();
            // One event per tick, as the loop does.
            for _ in 0..8 {
                actions.extend(process_input(app, &mut it, hits));
            }
            dev.send(&actions);
            sends(&actions)
        };

        // Keys: to the PB 1/2 fader and up 1 dB.
        let keys = vec![
            Event::Key(ch('2')),
            Event::Key(key(KeyCode::Right)),
            Event::Key(key(KeyCode::Down)),
            Event::Key(key(KeyCode::Down)),
            Event::Key(key(KeyCode::Down)),
            Event::Key(key(KeyCode::Right)),
        ];
        let sent = feed(&mut app, &hits, keys);
        assert_eq!(app.state().mixes[0].pb12.fader_db, 1.0, "{w}x{h}");
        assert!(!sent.is_empty());
        let (hits, _) = tick(&app);

        // Mouse: click MUTE on PB 3/4, then SOLO on PB 5/6 right after.
        let at = |r: Rect, kind| Event::Mouse(mouse(kind, r.x + 1, r.y, NONE));
        let mute = hits.rect(Hit::Control(mix(2, Mute(0)))).unwrap();
        let solo = hits.rect(Hit::Control(mix(3, Solo(0)))).unwrap();
        let fader = hits.rect(Hit::Control(mix(1, Fader(0)))).unwrap();
        let sent = feed(
            &mut app,
            &hits,
            vec![
                at(fader, DOWN),
                at(fader, UP),
                at(mute, DOWN),
                at(mute, UP),
                at(solo, DOWN),
                at(solo, UP),
            ],
        );
        assert!(app.state().mixes[0].pb34.mute, "{w}x{h}");
        assert!(app.state().mixes[0].pb56.solo, "{w}x{h}");
        assert!(!sent.is_empty());
        // A drag after the releases moves nothing.
        let none = feed(&mut app, &hits, vec![at(fader, DRAG)]);
        assert!(none.is_empty(), "{w}x{h}: no drag left behind");

        // The changes redraw, then the loop is idle again.
        let mut idle = usize::MAX;
        for _ in 0..10 {
            idle = tick(&app).1;
            if idle == 0 {
                break;
            }
        }
        assert_eq!(idle, 0, "{w}x{h}: idle after the changes");
        assert!(!dev.written().is_empty());
    }
}

/// Renaming is obvious and never traps the keyboard: the title is marked
/// while editing and the footer says how to leave; Esc leaves; Tab, ↑ and
/// ↓ leave too (keeping the text) and then act as usual.
#[test]
fn rename_never_traps_keys() {
    let meters = fake_meters();
    let mut app = app_with(State::default());
    app.handle_key(ch('2'));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Enter));
    assert!(app.editing().is_some());
    let (buf, _) = render(&app, &meters, 120, 40);
    let t = text(&buf);
    assert!(t.contains("▏"), "a cursor in the title:\n{t}");
    let footer = t.lines().nth(39).unwrap();
    assert!(
        footer.contains("rename") && footer.contains("esc"),
        "the footer says how to leave: {footer}"
    );
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert!(app.editing().is_none());

    for (k, want) in [
        (key(KeyCode::Tab), Control::Loopback(0)),
        (key(KeyCode::Down), mix(1, MixControl::Pan(0))),
        (key(KeyCode::Up), Control::Mixer(1)),
    ] {
        app.handle_key(ch('2'));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Backspace));
        app.handle_key(ch('X'));
        app.handle_key(k);
        assert!(app.editing().is_none(), "{k:?} leaves the edit");
        assert_eq!(app.focus(), want, "{k:?} then acts");
        assert_eq!(app.state().strip_names[1].as_deref(), Some("X"));
    }
    // On a strip name the footer says how to reach the controls.
    app.handle_key(ch('2'));
    let (buf, _) = render(&app, &meters, 120, 40);
    let footer = text(&buf).lines().nth(39).unwrap().to_string();
    assert!(
        footer.contains("↓") && footer.contains("controls") && footer.contains("rename"),
        "{footer}"
    );
    // Enter on a switch or a fader never starts an edit.
    app.handle_key(ch('2'));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert!(app.editing().is_none());
}
