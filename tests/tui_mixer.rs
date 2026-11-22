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
    a.pb34.set_fader(-12.0);
    a.pb34.pan_l = 0.2;
    a.pb34.pan_r = 0.8;
    (a.pb34.solo_l, a.pb34.solo_r) = (true, true);
    a.pb56.set_fader(f64::NEG_INFINITY);
    a.pb56.link = false;
    let b = &mut s.mixes[1];
    b.link = true;
    b.in1.fader_db = -6.0;
    b.pb12.set_fader(4.0);
    (b.pb12.phase_l, b.pb12.phase_r) = (true, true);
    let c = &mut s.mixes[2];
    c.in2.mute = true;
    c.pb56.set_fader(-24.0);
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

    // The same with LINK on (#15): the app moves each pan on its own
    // (`50−v`), so the right pan stays.
    s.mixes[0].pb34.link = true;
    let mut app = app_with(s);
    let dev = Device::new();
    focus(&mut app, mix(2, Pan(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(shift(KeyCode::Left)));
    let pb = &app.state().mixes[0].pb34;
    assert_eq!((pb.pan_l, pb.pan_r), (0.0, 0.5));
    assert_eq!(frames, [frame(0x51, 0x15, zero), frame(0x51, 0x25, 0)]);
    // And the right dial moves only the right pan.
    let frames = step(&mut app, &dev, |a| {
        a.handle_key(key(KeyCode::Up));
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down));
        assert_eq!(a.focus(), mix(2, Pan(1)));
        a.handle_key(key(KeyCode::Left))
    });
    let pb = &app.state().mixes[0].pb34;
    assert!((pb.pan_r - 0.45).abs() < 1e-9 && pb.pan_l == 0.0, "{pb:?}");
    assert_eq!(frames.len(), 2, "{frames:?}");
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
    assert!(app.state().mixes[0].pb12.solo_l);
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

    // IN 1+2 LINK (#15): no frames of its own; the pans stay their own.
    let mut s = State::default();
    s.mixes[0].in1.pan = 0.5;
    s.mixes[0].in2.pan = 0.5;
    let mut app = app_with(s);
    let dev = Device::new();
    focus(&mut app, mix(0, Link));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert!(app.state().mixes[0].link);
    assert!(frames.is_empty(), "{frames:?}");
    assert!(dev.written().is_empty());

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
    assert_eq!(app.state().mixes[0].pb12.fader_l_db, 6.0, "clamped at +6");
    for _ in 0..7 {
        step(&mut app, &dev, |a| a.handle_key(shift(KeyCode::Left)));
    }
    assert_eq!(app.state().mixes[0].pb12.fader_l_db, f64::NEG_INFINITY);
    let up = step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Right)));
    assert_eq!(app.state().mixes[0].pb12.fader_l_db, -59.0, "−∞ +1 dB");
    assert_eq!(up[0], frame(0x51, 0x13, -xp(-59.0)));
    step(&mut app, &dev, |a| a.handle_key(ch('0')));
    assert_eq!(app.state().mixes[0].pb12.fader_l_db, 0.0);
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
        app.state().mixes[0].pb12.fader_l_db,
        0.0,
        "a press moves nothing"
    );
    let frames = step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DRAG, f.x + 1, f.y, NONE), &hits, t0)
    });
    assert_eq!(app.state().mixes[0].pb12.fader_l_db, 6.0);
    assert!(!frames.is_empty());
    let frames = step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DRAG, f.x + 1, f.y.saturating_sub(3), NONE), &hits, t0)
    });
    assert!(frames.is_empty(), "clamped at +6 dB");
    // To the bottom: −∞.
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(DRAG, f.x + 1, f.bottom() + 2, NONE), &hits, t0)
    });
    assert_eq!(app.state().mixes[0].pb12.fader_l_db, f64::NEG_INFINITY);
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
    assert_eq!(app.state().mixes[0].pb12.fader_l_db, 0.0);

    // The wheel on the PB 3/4 left dial: 5 %, ⇧ ×10. LINK is on by
    // default; the right dial stays (#15: no mirror).
    let d = hits.rect(Hit::Control(mix(2, Pan(0)))).unwrap();
    let up = MouseEventKind::ScrollUp;
    step(&mut app, &dev, |a| {
        a.handle_mouse_at(mouse(up, d.x + 1, d.y + 1, NONE), &hits, t1)
    });
    let pb = app.state().mixes[0].pb34.clone();
    assert!((pb.pan_l - 0.05).abs() < 1e-9, "{pb:?}");
    assert_eq!(pb.pan_r, 1.0, "{pb:?}");
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
    // Double-click resets the dial (its partner was never moved).
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

    // Clicks on the pills: Ø, SOLO, MUTE of every strip, then LINK (last:
    // it swaps the strip's page, #15).
    let t4 = t3 + Duration::from_secs(5);
    let mut i = 0u64;
    for strip in 0..4 {
        let chans = if strip == 0 { 2 } else { 1 };
        let mut ctls = vec![];
        for c in 0..chans {
            ctls.extend([Phase(c), Solo(c), Mute(c)]);
        }
        ctls.push(Link);
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
        PixelWidget::Inset => "inset",
    }
}

/// The layout scales with the window (#14, #37 review): at every size the
/// console fills the terminal; panels, strips, faders, meters and knobs
/// grow and shrink with it in the app's proportions; each strip's content
/// stays one compact group with spare rows spread evenly; knobs are about
/// 75–80 % of #34's relative size; pixel widgets sit in the new rects and
/// the hit map stays exact.
#[test]
fn layout_scales_with_the_window() {
    use toppingctl::tui::app::InputControl;
    use MixControl::*;
    let app = app_with(fake_state());
    let meters = fake_meters();
    let sizes = [(100u16, 34u16), (120, 40), (160, 50), (250, 60)];
    let mut fader_h = Vec::new();
    let mut pan_w = Vec::new();
    let mut gain_w = Vec::new();
    for (w, h) in sizes {
        let (buf, hits) = render(&app, &meters, w, h);
        let t = text(&buf);
        if (w, h) == (160, 50) || (w, h) == (250, 60) {
            assert_snapshot(&format!("layout_{w}x{h}"), &t);
        }
        let ctx = format!("{w}x{h}:\n{t}");

        // The console fills the window.
        assert_eq!(buf[(0, 0)].symbol(), "╭", "{ctx}");
        assert_eq!(buf[(w - 1, 0)].symbol(), "╮", "{ctx}");
        assert_eq!(buf[(w - 1, h - 2)].symbol(), "╯", "{ctx}");

        let knobs: Vec<Rect> = hits
            .pixels()
            .iter()
            .filter(|s| kind(&s.widget) == "knob")
            .map(|s| s.rect)
            .collect();
        assert_eq!(knobs.len(), 2 + 8 + 1, "{ctx}");
        let knob_in = |r: Rect| {
            *knobs
                .iter()
                .find(|k| r.contains(k.as_position()))
                .unwrap_or_else(|| panic!("no knob in {r:?}: {ctx}"))
        };

        for strip in 0..4 {
            let strip_box = hits.rect(Hit::Focus(Control::Mixer(strip))).unwrap();
            // A pan knob: round (two columns a row), at most ~80 % of its
            // half of the strip, and not tiny.
            for c in 0..2 {
                let d = hits.rect(Hit::Control(mix(strip, Pan(c)))).unwrap();
                let k = knob_in(d);
                let half = (strip_box.width - 2) / 2;
                assert!(k.width <= half * 4 / 5 + 1, "pan {k:?} in {half}: {ctx}");
                assert!(k.width >= half / 2, "pan {k:?} in {half}: {ctx}");
                assert_eq!(k.width, k.height * 2, "round: {k:?}");
                if strip == 1 && c == 0 {
                    pan_w.push(k.width);
                }
            }
            // One compact group: dials, LINK, value, fader, switches with
            // no gap between them.
            let dial = hits.rect(Hit::Control(mix(strip, Pan(0)))).unwrap();
            let link = hits.rect(Hit::Control(mix(strip, Link))).unwrap();
            let fader = hits.rect(Hit::Control(mix(strip, Fader(0)))).unwrap();
            let phase = hits.rect(Hit::Control(mix(strip, Phase(0)))).unwrap();
            assert_eq!(link.y, dial.bottom(), "strip {strip}: {ctx}");
            assert_eq!(fader.y, link.y + 2, "strip {strip}: {ctx}");
            assert!(phase.y <= fader.bottom() + 1, "strip {strip}: {ctx}");
            if strip == 1 {
                fader_h.push(fader.height);
            }
        }

        // An input strip: the switches and the gain knob spread evenly
        // down the strip, the knob ending where the meter does.
        for c in 0..2 {
            let rows: Vec<u16> = InputControl::ORDER[..6]
                .iter()
                .map(|&kind| {
                    hits.rect(Hit::Control(Control::Input { ch: c, kind }))
                        .unwrap()
                        .y
                })
                .collect();
            let gaps: Vec<u16> = rows.windows(2).map(|p| p[1] - p[0]).collect();
            assert!(
                gaps.iter().max().unwrap() - gaps.iter().min().unwrap() <= 1,
                "even: {rows:?}: {ctx}"
            );
            let knob = hits
                .rect(Hit::Control(Control::Input {
                    ch: c,
                    kind: InputControl::Gain,
                }))
                .unwrap();
            let after = knob.y - rows[5];
            assert!(
                after >= *gaps.iter().min().unwrap() && after <= gaps.iter().max().unwrap() + 1,
                "knob follows at the same spacing: {rows:?} {knob:?}: {ctx}"
            );
            let strip = hits
                .rect(Hit::Focus(Control::Input {
                    ch: c,
                    kind: InputControl::ORDER[0],
                }))
                .unwrap();
            assert!(
                knob.bottom() + 2 >= strip.bottom(),
                "{knob:?} {strip:?}: {ctx}"
            );
            let k = knob_in(knob);
            assert_eq!(k.width, k.height * 2, "round: {k:?}");
            // About 75–80 % of #34's knob, which was ~0.63 of the strip.
            let ratio = f64::from(k.width) / f64::from(strip.width - 2);
            assert!(
                (0.35..=0.6).contains(&ratio),
                "gain {k:?} ratio {ratio}: {ctx}"
            );
            if c == 0 {
                gain_w.push(k.width);
            }
        }

        // Every control's hit rect answers for its own cells.
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
            // The fader's pixels are over its hit rect's track.
            let f = hits.rect(Hit::Control(mix(strip, Fader(0)))).unwrap();
            assert!(hits.pixels().iter().any(|s| kind(&s.widget) == "fader"
                && s.rect.y == f.y
                && s.rect.height == f.height));
        }
    }
    // Faders and knobs grow with the window, and shrink back.
    assert!(fader_h.windows(2).all(|p| p[0] <= p[1]), "{fader_h:?}");
    assert!(fader_h[3] > fader_h[0], "{fader_h:?}");
    assert!(
        pan_w.windows(2).all(|p| p[0] <= p[1]) && pan_w[3] > pan_w[0],
        "{pan_w:?}"
    );
    assert!(
        gain_w.windows(2).all(|p| p[0] <= p[1]) && gain_w[3] > gain_w[0],
        "{gain_w:?}"
    );
    // Only absurd sizes stop growing, centred.
    let (buf, _) = render(&app, &meters, 400, 120);
    assert_eq!(buf[(0, 0)].symbol(), " ");
    let (mw, mh) = (ui::MAX_WIDTH, ui::MAX_HEIGHT);
    assert!(mw > 300 && mh >= 90);
    assert_eq!(buf[((400 - mw) / 2, (120 - mh) / 2)].symbol(), "╭");
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
        assert_eq!(app.state().mixes[0].pb12.fader_l_db, 1.0, "{w}x{h}");
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
        assert!(app.state().mixes[0].pb34.mute_l, "{w}x{h}");
        assert!(app.state().mixes[0].pb56.solo_l, "{w}x{h}");
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

// ------------------------------------------- buttons and meter bridges

/// Every meter sits in its own sunken box (#37 review): the input's clip
/// light, peak readout, bar and scale; the mixer, loopback and output
/// meters beside their faders. In the text form the box is a darker
/// background; in kitty an inset image under the text.
#[test]
fn meters_sit_in_inset_boxes() {
    let app = app_with(fake_state());
    let meters = fake_meters();
    for (w, h) in [(100u16, 34u16), (120, 40), (250, 60)] {
        let (buf, hits) = render(&app, &meters, w, h);
        let insets: Vec<Rect> = hits
            .pixels()
            .iter()
            .filter(|s| kind(&s.widget) == "inset")
            .map(|s| s.rect)
            .collect();
        // 2 inputs, IN 1+2's two, three PB pairs, two loopback pairs, OUT.
        assert_eq!(insets.len(), 2 + 2 + 6 + 4 + 2, "{w}x{h}");
        for spot in hits.pixels() {
            if matches!(kind(&spot.widget), "meter" | "clip") {
                assert!(
                    insets.iter().any(|b| b.union(spot.rect) == *b),
                    "{w}x{h}: {spot:?} outside every box"
                );
            }
        }
        // The input boxes hold the peak readout and the scale too.
        let t = text(&buf);
        for (y, line) in t.lines().enumerate() {
            let mut from = 0;
            while let Some(i) = line[from..].find(" pk") {
                let x = line[..from + i].chars().count() as u16 + 2;
                let pos = ratatui::layout::Position { x, y: y as u16 };
                assert!(
                    insets.iter().any(|b| b.contains(pos)),
                    "{w}x{h}: peak at {x},{y} outside a box"
                );
                from += i + 3;
            }
        }
        // The text form: a darker background over each box.
        for b in &insets {
            for y in b.top()..b.bottom() {
                for x in b.left()..b.right() {
                    assert_eq!(buf[(x, y)].bg, Theme::INSET_BG, "{w}x{h} at {x},{y}");
                }
            }
        }
    }
}

/// Buttons are square, Xone-style lit caps: on, a square face in the
/// button's colour with a soft glow around it and a top highlight; off, a
/// dark recessed cap with a faint outline. No rounded pill ends.
#[test]
fn buttons_are_square_lit_caps() {
    use toppingctl::tui::pixel::{chip_image, PX_ACCENT, PX_CHIP};
    let cell = (10, 20);
    let on = chip_image(6, 1, cell, PX_ACCENT);
    let off = chip_image(6, 1, cell, PX_CHIP);
    let (w, h) = on.dimensions();
    let px = |img: &image::RgbaImage, x: u32, y: u32| img.get_pixel(x, y).0;
    // The face: find its top-left corner from the middle row going left.
    let mid = h / 2;
    let left = (0..w).find(|&x| px(&on, x, mid)[3] > 200).unwrap();
    let top = (0..h).find(|&y| px(&on, w / 2, y)[3] > 200).unwrap();
    // Square: the corner pixel of the face is as solid as its edges.
    let corner = px(&on, left, top);
    assert!(corner[3] > 200, "square corner: {corner:?}");
    // A glow outside the face: faint, not empty, not solid.
    let glow = px(&on, left.saturating_sub(2), mid);
    assert!(glow[3] > 0 && glow[3] < 160, "glow: {glow:?}");
    // A top highlight: the face's top rows are lighter than its middle.
    let (hi, body) = (px(&on, w / 2, top + 1), px(&on, w / 2, mid));
    assert!(
        hi[0] as u32 + hi[1] as u32 + hi[2] as u32
            > body[0] as u32 + body[1] as u32 + body[2] as u32,
        "highlight {hi:?} over {body:?}"
    );
    // Off: a dark cap, darker than the outline around it, no glow.
    let oleft = (0..w).find(|&x| px(&off, x, mid)[3] > 200).unwrap();
    let edge = px(&off, oleft, mid);
    let face = px(&off, w / 2, mid);
    assert!(
        face[0] < 60 && face[1] < 60 && face[2] < 70,
        "dark cap {face:?}"
    );
    assert!(
        edge[0] as u32 + edge[1] as u32 > face[0] as u32 + face[1] as u32,
        "outline"
    );
    assert_eq!(
        px(&off, oleft.saturating_sub(2), mid)[3],
        0,
        "no glow when off"
    );
    let otop = (0..h).find(|&y| px(&off, w / 2, y)[3] > 200).unwrap();
    assert!(px(&off, oleft, otop)[3] > 200, "square corner when off");
}

// ------------------------------------------------------ modals (#37)

/// A modal (rate list, help, 48V confirm) keeps the pixel widgets around
/// it: only those under the modal's box go, nothing is sent again, and
/// closing it puts them back from the cache without re-sending an image.
#[test]
fn modals_keep_the_pixels_around_them() {
    use toppingctl::tui::app::InputControl;
    let meters = fake_meters();
    let mut state = State::default();
    state.inputs[0].phantom = Some(false);
    let mut app = app_with(state);
    let mut layer = kitty_layer();
    layer.set_resize_settle(Duration::ZERO);
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let mut frame_out = |app: &App, layer: &mut toppingctl::tui::pixel::PixelLayer| {
        let mut hits = HitMap::default();
        terminal
            .draw(|f| hits = ui::draw_with(f, app, &meters, Some(&mut *layer)))
            .unwrap();
        (hits, layer.take_output())
    };
    for _ in 0..40 {
        if frame_out(&app, &mut layer).1.is_empty() {
            break;
        }
    }
    let (base, out) = frame_out(&app, &mut layer);
    assert!(out.is_empty(), "idle before");
    type Open = Box<dyn Fn(&mut App)>;
    let opens: [(&str, Open); 3] = [
        ("rate", Box::new(|a: &mut App| drop(a.handle_key(ch('r'))))),
        ("help", Box::new(|a: &mut App| drop(a.handle_key(ch('?'))))),
        (
            "48V",
            Box::new(|a: &mut App| {
                a.handle_key(ch('1'));
                a.handle_key(key(KeyCode::Down));
                assert_eq!(
                    a.focus(),
                    Control::Input {
                        ch: 0,
                        kind: InputControl::Phantom
                    }
                );
                drop(a.handle_key(ch(' ')));
            }),
        ),
    ];
    for (name, open) in opens {
        open(&mut app);
        assert!(app.modal().is_some(), "{name}");
        let (hits, out) = frame_out(&app, &mut layer);
        let modal = hits.modal_rect().expect("the modal's box");
        assert!(!out.contains("a=t"), "{name}: no image sent on open");
        for s in base.pixels() {
            let under = s.rect.intersects(modal);
            assert_eq!(
                layer.is_placed(s.rect),
                !under,
                "{name}: {s:?} vs {modal:?}"
            );
        }
        // Idle while open.
        assert!(
            frame_out(&app, &mut layer).1.is_empty(),
            "{name}: idle open"
        );
        app.handle_key(key(KeyCode::Esc));
        assert!(app.modal().is_none(), "{name}");
        let (_, out) = frame_out(&app, &mut layer);
        assert!(!out.contains("a=t"), "{name}: put back from the cache");
        for s in base.pixels() {
            assert!(layer.is_placed(s.rect), "{name}: {s:?} back");
        }
        assert!(
            frame_out(&app, &mut layer).1.is_empty(),
            "{name}: idle after"
        );
    }
}

// ------------------------------------------ the LINK page swap (#15)

/// The fader hit rects of mixer strip `strip` on the last render.
fn faders_of(hits: &HitMap, strip: usize) -> Vec<MixControl> {
    [MixControl::Fader(0), MixControl::Fader(1)]
        .into_iter()
        .filter(|&c| hits.rect(Hit::Control(mix(strip, c))).is_some())
        .collect()
}

/// Every mixer strip swaps pages on LINK, as in the app (#15 audit):
/// unlinked, L/R faders and SOLO/MUTE/Ø per channel; linked, one of each
/// writing both channels. Linking with L ≠ R averages the faders. LINK
/// itself sends nothing.
#[test]
fn link_swaps_every_mixer_strip() {
    use MixControl::*;
    let meters = fake_meters();
    let mut app = app_with(State::default());
    let dev = Device::new();

    // The defaults: IN 1+2 unlinked (two faders), every PB linked (one).
    let (_, hits) = render(&app, &meters, 120, 40);
    assert_eq!(faders_of(&hits, 0), [Fader(0), Fader(1)]);
    for strip in 1..4 {
        assert_eq!(faders_of(&hits, strip), [Fader(0)], "PB {strip}");
        assert!(hits.rect(Hit::Control(mix(strip, Mute(1)))).is_none());
    }

    // Unlink PB 1/2: only a save; now an L and an R of everything.
    focus(&mut app, mix(1, Link));
    assert_eq!(app.handle_key(ch(' ')), [Action::Save]);
    assert!(!app.state().mixes[0].pb12.link);
    let (buf, hits) = render(&app, &meters, 120, 40);
    assert_eq!(faders_of(&hits, 1), [Fader(0), Fader(1)], "{}", text(&buf));
    for ctl in [Phase(1), Solo(1), Mute(1)] {
        let r = hits.rect(Hit::Control(mix(1, ctl))).unwrap();
        assert_eq!(hits.get(r.x, r.y), Some(Hit::Control(mix(1, ctl))));
    }
    // Its focus order is IN 1+2's.
    let mut seen = vec![];
    for _ in 0..12 {
        app.handle_key(key(KeyCode::Up));
    }
    assert_eq!(app.focus(), Control::Mixer(1));
    for _ in 0..11 {
        app.handle_key(key(KeyCode::Down));
        seen.push(app.focus());
    }
    assert_eq!(seen, MixControl::UNLINKED.map(|c| mix(1, c)));

    // The even channel alone: PB2 is hard R, so only 51 24 moves.
    focus(&mut app, mix(1, Fader(1)));
    let frames = step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Left)));
    assert_eq!(frames, [frame(0x51, 0x24, xp(-1.0))]);
    assert_eq!(app.state().mixes[0].pb12.fader_l_db, 0.0);
    focus(&mut app, mix(1, Mute(1)));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert_eq!(frames, [frame(0x51, 0x24, 0)]);
    assert!(!app.state().mixes[0].pb12.mute_l);
    step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    focus(&mut app, mix(1, Phase(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert_eq!(frames, [frame(0x51, 0x13, -xp(0.0))]);
    step(&mut app, &dev, |a| a.handle_key(ch(' ')));

    // L −6, R −1; link: both to the average, −3.5 dB.
    focus(&mut app, mix(1, Fader(0)));
    for _ in 0..6 {
        step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Left)));
    }
    focus(&mut app, mix(1, Link));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    let pb = &app.state().mixes[0].pb12;
    assert!(pb.link);
    assert_eq!((pb.fader_l_db, pb.fader_r_db), (-3.5, -3.5));
    assert_eq!(
        frames,
        [frame(0x51, 0x13, xp(-3.5)), frame(0x51, 0x24, xp(-3.5))]
    );
    // Linked: one fader and one MUTE for both channels.
    focus(&mut app, mix(1, Fader(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Right)));
    assert_eq!(
        frames,
        [frame(0x51, 0x13, xp(-2.5)), frame(0x51, 0x24, xp(-2.5))]
    );
    focus(&mut app, mix(1, Mute(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert_eq!(frames, [frame(0x51, 0x13, 0), frame(0x51, 0x24, 0)]);
    let pb = &app.state().mixes[0].pb12;
    assert!(pb.mute_l && pb.mute_r);

    // IN 1+2 linked: one fader; focus on IN2's fader falls back to it.
    focus(&mut app, mix(0, Fader(1)));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.focus(), mix(0, Fader(0)));
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(app.focus(), mix(0, Phase(0)));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.focus(), mix(0, Link));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch(' ')));
    assert!(frames.is_empty() && app.state().mixes[0].link);
    let (buf, hits) = render(&app, &meters, 120, 40);
    assert_eq!(faders_of(&hits, 0), [Fader(0)], "{}", text(&buf));
    // IN1 hard L, IN2 hard R: the fader writes 51 11 and 51 22.
    focus(&mut app, mix(0, Fader(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(key(KeyCode::Left)));
    assert_eq!(
        frames,
        [frame(0x51, 0x11, xp(-1.0)), frame(0x51, 0x22, xp(-1.0))]
    );
    // A mix where IN 1+2 is unlinked: focus on IN2's fader stays valid
    // there, and comes back to the single fader in Mix A.
    app.handle_key(ch('b'));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.focus(), mix(0, Fader(1)));
    app.handle_key(ch('a'));
    assert_eq!(app.focus(), mix(0, Fader(0)));
    dev.written();
}

/// Linked pans move independently (the app's `50−v`): no mirror on a PB
/// strip, and IN 1+2's dials still turn while linked.
#[test]
fn linked_pans_move_independently() {
    use MixControl::*;
    let mut s = State::default();
    s.mixes[0].link = true;
    let mut app = app_with(s);
    let dev = Device::new();
    let zero = xp(0.0);

    // PB 3/4 linked: the left dial alone.
    focus(&mut app, mix(2, Pan(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(shift(KeyCode::Right)));
    let pb = &app.state().mixes[0].pb34;
    assert!((pb.pan_l - 0.5).abs() < 1e-9 && pb.pan_r == 1.0, "{pb:?}");
    assert_eq!(
        frames,
        [
            frame(0x51, 0x15, zero - zero / 2),
            frame(0x51, 0x25, zero / 2)
        ]
    );

    // IN 1+2 linked: IN1's dial turns, IN2's stays.
    focus(&mut app, mix(0, Pan(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(shift(KeyCode::Right)));
    let m = &app.state().mixes[0];
    assert!((m.in1.pan - 0.5).abs() < 1e-9 && m.in2.pan == 1.0);
    assert_eq!(
        frames,
        [
            frame(0x51, 0x11, zero - zero / 2),
            frame(0x51, 0x21, zero / 2)
        ]
    );
    // Shown as it is (no override to hard L / R).
    let (_, hits) = render(&app, &fake_meters(), 120, 40);
    let dial = hits.rect(Hit::Control(mix(0, Pan(0)))).unwrap();
    let value = hits.pixels().iter().find_map(|s| match s.widget {
        PixelWidget::Knob { value, .. } if dial.contains(s.rect.as_position()) => Some(value),
        _ => None,
    });
    assert_eq!(value, Some(Some(0.5)), "IN1's dial shows its own pan");
    dev.written();
}
