//! Keyboard control that works (#47): an obvious focus marker on every
//! focusable widget, arrows that only move (alsamixer-style), `+`/`-` that
//! adjust, and button faces that line up in their columns. No terminal, no
//! hardware: frames go through a real `Connection` over `FakeTransport`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::Terminal;

use crossterm::event::Event as Input;
use toppingctl::conn::fake::{FakeHandle, FakeTransport};
use toppingctl::conn::Connection;
use toppingctl::frame::{Dir, Frame};
use toppingctl::state::{Meters, State, CROSSPOINT_ZERO_DB};
use toppingctl::tui::app::{Action, App, BusControl, Control, InputControl, MixControl, OUT_BUS};
use toppingctl::tui::pixel::{
    button_image, fader_image_focused, knob_image, knob_image_focused, FaceLabel, PixelWidget,
    PX_ACCENT, PX_ACCENT_DIM, PX_ARC_OFF, PX_CHIP, PX_FADER_CAP, PX_LABEL_ON,
};
use toppingctl::tui::run::{process_input, send_actions};
use toppingctl::tui::theme::Theme;
use toppingctl::tui::ui::{self, Hit, HitMap};
use toppingctl::tui::widgets::fader::{fader_frac, fader_row};

// ---------------------------------------------------------------- helpers

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ch(c: char) -> KeyEvent {
    key(KeyCode::Char(c))
}

fn mix(strip: usize, ctl: MixControl) -> Control {
    Control::Mix { strip, ctl }
}

fn bus(bus: usize, ctl: BusControl) -> Control {
    Control::Bus { bus, ctl }
}

fn input(ch: usize, kind: InputControl) -> Control {
    Control::Input { ch, kind }
}

fn app_with(state: State) -> App {
    let mut app = App::new(Theme::color());
    app.synced(state);
    app
}

/// A state with an unlinked strip, loopback and output, so both pages are
/// on screen.
fn mixed_state() -> State {
    let mut s = State::default();
    s.inputs[0].gain_db = 12;
    let a = &mut s.mixes[0];
    a.pb34.pan_l = 0.2;
    a.pb56.link = false;
    s.loopbacks[0].link = true;
    s.loopbacks[1].link = false;
    s.output.link = false;
    s
}

fn render(app: &App, w: u16, h: u16) -> (Buffer, HitMap) {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    terminal
        .draw(|f| hits = ui::draw(f, app, &Meters::default()))
        .unwrap();
    (terminal.backend().buffer().clone(), hits)
}

fn cells(buf: &Buffer, y: u16, x0: u16, x1: u16) -> String {
    (x0..x1).map(|x| buf[(x, y)].symbol()).collect()
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

/// Runs `f` and checks the frames sent are `State::diff`, then a save;
/// forwards the actions to `dev`.
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

/// Presses `k`, which must send nothing, and checks focus lands on `want`.
fn press(app: &mut App, k: KeyEvent, want: Control) {
    let before = app.state().clone();
    assert!(app.handle_key(k).is_empty(), "{k:?} from {:?}", app.focus());
    assert_eq!(app.state(), &before);
    assert_eq!(app.focus(), want, "after {:?}", k.code);
}

/// The app's crosspoint level for a fader at `db` (PROTOCOL.md).
fn xp(db: f64) -> i32 {
    ((335_544.0 * 10f64.powf(db / 20.0)).round() as i32) * 100
}

const UP: KeyCode = KeyCode::Up;
const DOWN: KeyCode = KeyCode::Down;
const LEFT: KeyCode = KeyCode::Left;
const RIGHT: KeyCode = KeyCode::Right;

// ------------------------------------------------------------ navigation

/// A long walk of arrows (and Tab) over every panel, linked and unlinked
/// pages, switches in a row and stacked: no arrow ever sends a frame or
/// changes the state, and the walk reaches every kind of control.
#[test]
fn arrows_never_change_state() {
    for stacked in [false, true] {
        let mut app = app_with(mixed_state());
        app.set_switches_stacked(stacked);
        let state = app.state().clone();
        let mut seen = std::collections::HashSet::new();
        let mut rng: u32 = 0x2545_f491;
        for i in 0..20_000 {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let k = match rng % 41 {
                0..10 => UP,
                10..20 => DOWN,
                20..30 => LEFT,
                30..40 => RIGHT,
                _ if i % 2 == 0 => KeyCode::Tab,
                _ => KeyCode::BackTab,
            };
            let out = app.handle_key(key(k));
            assert!(out.is_empty(), "{k:?} sent {out:?}");
            assert_eq!(app.state(), &state, "{k:?} changed the state");
            seen.insert(app.focus());
        }
        for want in [
            input(1, InputControl::Gain),
            input(1, InputControl::Mon),
            Control::Mixer(3),
            mix(2, MixControl::Pan(1)),
            mix(1, MixControl::Link),
            mix(3, MixControl::Fader(1)),
            mix(3, MixControl::Phase(1)),
            mix(3, MixControl::Mute(1)),
            mix(2, MixControl::Solo(0)),
            Control::Loopback(1),
            bus(1, BusControl::Mute(1)),
            bus(1, BusControl::Fader(1)),
            bus(0, BusControl::Source),
            bus(OUT_BUS, BusControl::Line),
            bus(OUT_BUS, BusControl::Fader(1)),
            Control::Output(2),
        ] {
            assert!(
                seen.contains(&want),
                "stacked {stacked}: never reached {want:?}"
            );
        }
    }
}

/// ← → move between strips and between side-by-side controls; ↑ ↓ go down
/// a strip in drawing order: name → pans → LINK → fader → Ø / SOLO / MUTE.
#[test]
fn arrows_navigate_like_alsamixer() {
    use MixControl::*;
    let mut app = app_with(mixed_state());

    // The mixer, switches in one row (the 120×40 layout).
    press(&mut app, ch('2'), Control::Mixer(0));
    press(&mut app, key(RIGHT), Control::Mixer(1));
    press(&mut app, key(DOWN), mix(1, Pan(0)));
    press(&mut app, key(RIGHT), mix(1, Pan(1)));
    press(&mut app, key(RIGHT), mix(2, Pan(0)));
    press(&mut app, key(LEFT), mix(1, Pan(1)));
    press(&mut app, key(DOWN), mix(1, Link));
    press(&mut app, key(DOWN), mix(1, Fader(0)));
    press(&mut app, key(DOWN), mix(1, Phase(0)));
    press(&mut app, key(RIGHT), mix(1, Solo(0)));
    press(&mut app, key(RIGHT), mix(1, Mute(0)));
    press(&mut app, key(RIGHT), mix(2, Phase(0)));
    press(&mut app, key(UP), mix(2, Fader(0)));
    // Into the unlinked PB 5/6: its L fader, then its R fader.
    press(&mut app, key(RIGHT), mix(3, Fader(0)));
    press(&mut app, key(RIGHT), mix(3, Fader(1)));
    press(&mut app, key(RIGHT), mix(3, Fader(1)));
    press(&mut app, key(DOWN), mix(3, Phase(1)));
    press(&mut app, key(LEFT), mix(3, Mute(0)));
    press(&mut app, key(UP), mix(3, Fader(0)));
    press(&mut app, key(UP), mix(3, Link));
    press(&mut app, key(UP), mix(3, Pan(0)));
    press(&mut app, key(UP), Control::Mixer(3));
    press(&mut app, key(UP), Control::Mixer(3));
    // Moving left into a strip lands on its rightmost control.
    press(&mut app, key(DOWN), mix(3, Pan(0)));
    press(&mut app, key(DOWN), mix(3, Link));
    press(&mut app, key(DOWN), mix(3, Fader(0)));
    press(&mut app, key(LEFT), mix(2, Fader(0)));

    // Switches stacked (a tall window): Ø, SOLO, MUTE down the strip,
    // channels side by side.
    app.set_switches_stacked(true);
    press(&mut app, key(DOWN), mix(2, Phase(0)));
    press(&mut app, key(DOWN), mix(2, Solo(0)));
    press(&mut app, key(DOWN), mix(2, Mute(0)));
    press(&mut app, key(DOWN), mix(2, Mute(0)));
    press(&mut app, key(RIGHT), mix(3, Mute(0)));
    press(&mut app, key(RIGHT), mix(3, Mute(1)));
    press(&mut app, key(UP), mix(3, Solo(1)));
    press(&mut app, key(UP), mix(3, Phase(1)));
    press(&mut app, key(UP), mix(3, Fader(1)));

    // Inputs: ↑ ↓ along the column, ← → to the other input.
    press(&mut app, ch('1'), input(0, InputControl::Mon));
    press(&mut app, key(DOWN), input(0, InputControl::Phantom));
    press(&mut app, key(RIGHT), input(1, InputControl::Phantom));
    press(&mut app, key(LEFT), input(0, InputControl::Phantom));

    // Loopback: source, LINK, fader(s), MUTE(s), as drawn.
    press(&mut app, ch('3'), Control::Loopback(0));
    press(&mut app, key(DOWN), bus(0, BusControl::Source));
    press(&mut app, key(DOWN), bus(0, BusControl::Link));
    press(&mut app, key(DOWN), bus(0, BusControl::Fader(0)));
    press(&mut app, key(RIGHT), bus(1, BusControl::Fader(0)));
    press(&mut app, key(RIGHT), bus(1, BusControl::Fader(1)));
    press(&mut app, key(DOWN), bus(1, BusControl::Mute(1)));
    press(&mut app, key(LEFT), bus(1, BusControl::Mute(0)));
    press(&mut app, key(LEFT), bus(0, BusControl::Mute(0)));

    // Output: HP and LINE side by side, then MONITOR MIX and PHONE.
    press(&mut app, ch('4'), Control::Output(0));
    press(&mut app, key(DOWN), bus(OUT_BUS, BusControl::Source));
    press(&mut app, key(DOWN), bus(OUT_BUS, BusControl::Link));
    press(&mut app, key(DOWN), bus(OUT_BUS, BusControl::Fader(0)));
    press(&mut app, key(RIGHT), bus(OUT_BUS, BusControl::Fader(1)));
    press(&mut app, key(DOWN), bus(OUT_BUS, BusControl::Line));
    press(&mut app, key(LEFT), bus(OUT_BUS, BusControl::Hp));
    press(&mut app, key(RIGHT), bus(OUT_BUS, BusControl::Line));
    press(&mut app, key(RIGHT), Control::Output(1));
    press(&mut app, key(RIGHT), Control::Output(2));
    press(&mut app, key(LEFT), Control::Output(1));
    press(&mut app, key(LEFT), Control::Output(0));
}

/// The renderer reports whether the mix strips stack their switches.
#[test]
fn hit_map_reports_stacked_switches() {
    let app = app_with(mixed_state());
    assert!(!render(&app, 120, 40).1.switches_stacked());
    assert!(render(&app, 250, 60).1.switches_stacked());

    // The event loop hands it to the app: ↓ from Ø goes to SOLO when
    // stacked, and on to the next strip's Ø from MUTE in a row.
    for (size, want) in [
        ((250, 60), MixControl::Solo(0)),
        ((120, 40), MixControl::Phase(0)),
    ] {
        let mut app = app_with(mixed_state());
        for k in [
            ch('2'),
            key(RIGHT),
            key(DOWN),
            key(DOWN),
            key(DOWN),
            key(DOWN),
        ] {
            app.handle_key(k);
        }
        assert_eq!(app.focus(), mix(1, MixControl::Phase(0)));
        let (_, hits) = render(&app, size.0, size.1);
        let mut events = vec![Input::Key(key(DOWN))].into_iter();
        assert!(process_input(&mut app, &mut events, &hits).is_empty());
        assert_eq!(app.focus(), mix(1, want), "{size:?}");
    }
}

// -------------------------------------------------------------- adjusting

/// `+`/`=` and `-`/`_` adjust the focused knob or fader by one step (fader
/// 1 dB, dial 5 %, gain 1 dB, loop/out 1 dB); PgUp / PgDn ten. `+` and `_`
/// come with Shift on most keyboards, and still step once. The frames are
/// `State::diff`, written by the connection.
#[test]
fn plus_minus_send_expected_frames() {
    use MixControl::*;
    let dev = Device::new();
    let mut all = Vec::new();
    let mut app = app_with(State::default());
    let shifted = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT);

    // Gain: +1, +1, −1, −1, +10, −10.
    press(&mut app, ch('1'), input(0, InputControl::Mon));
    for _ in 0..6 {
        app.handle_key(key(DOWN));
    }
    assert_eq!(app.focus(), input(0, InputControl::Gain));
    for (k, want) in [
        (shifted('+'), 1),
        (ch('='), 2),
        (ch('-'), 1),
        (shifted('_'), 0),
        (key(KeyCode::PageUp), 10),
        (key(KeyCode::PageDown), 0),
    ] {
        let frames = step(&mut app, &dev, |a| a.handle_key(k));
        assert!(!frames.is_empty(), "{k:?}");
        assert_eq!(app.state().inputs[0].gain_db, want, "{k:?}");
        all.extend(frames);
    }

    // Mix A IN1 fader −1 dB: `51 11` (IN1 is hard L).
    press(&mut app, ch('2'), Control::Mixer(0));
    for _ in 0..3 {
        app.handle_key(key(DOWN));
    }
    assert_eq!(app.focus(), mix(0, Fader(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch('-')));
    assert_eq!(
        frames,
        [Frame {
            group: 0x51,
            param: 0x11,
            value: xp(-1.0)
        }]
    );
    all.extend(frames);

    // PB 1/2's left pan from hard L: 5 % to the right.
    app.handle_key(key(RIGHT));
    app.handle_key(key(RIGHT));
    app.handle_key(key(UP));
    app.handle_key(key(UP));
    assert_eq!(app.focus(), mix(1, Pan(0)));
    let frames = step(&mut app, &dev, |a| a.handle_key(ch('+')));
    assert!(!frames.is_empty());
    assert_eq!(app.state().mixes[0].pb12.pan_l, 0.05);
    assert!(frames.iter().all(|f| f.group == 0x51), "{frames:?}");
    all.extend(frames);
    // `0` puts it back.
    all.extend(step(&mut app, &dev, |a| a.handle_key(ch('0'))));
    assert_eq!(app.state().mixes[0].pb12.pan_l, 0.0);

    // Loopback 1+2's level: −1 dB, then PgDn its page step (5 dB).
    press(&mut app, ch('3'), Control::Loopback(0));
    for _ in 0..3 {
        app.handle_key(key(DOWN));
    }
    assert_eq!(app.focus(), bus(0, BusControl::Fader(0)));
    all.extend(step(&mut app, &dev, |a| a.handle_key(ch('-'))));
    assert_eq!(app.state().loopbacks[0].level_l_db, -1.0);
    all.extend(step(&mut app, &dev, |a| {
        a.handle_key(key(KeyCode::PageDown))
    }));
    assert_eq!(app.state().loopbacks[0].level_l_db, -6.0);

    // On a button, a name or a source, `+`/`-` do nothing.
    for focus_key in [ch('2'), ch('4')] {
        app.handle_key(focus_key);
        for k in [ch('+'), ch('-'), key(KeyCode::PageUp)] {
            assert!(app.handle_key(k).is_empty(), "{k:?} on {:?}", app.focus());
        }
    }
    app.handle_key(key(DOWN));
    assert_eq!(app.focus(), bus(OUT_BUS, BusControl::Source));
    assert!(app.handle_key(ch('+')).is_empty());

    assert_eq!(dev.written(), all, "the connection wrote every frame");
    assert_ne!(CROSSPOINT_ZERO_DB, 0);
}

// ----------------------------------------------------------- focus marker

/// The focused widget, whatever its kind, gets the flat accent marker: in
/// pixels its widget is drawn focused (knob, fader, button) or an outline
/// spot sits on it (source menu, name); in text, its cells are reversed.
/// Exactly one marker is on screen.
#[test]
fn focus_marker_for_each_widget_kind() {
    use MixControl::*;
    let cases: Vec<(Vec<KeyEvent>, Control, &str)> = vec![
        (vec![ch('1'); 1], input(0, InputControl::Mon), "button"),
        (
            [vec![ch('1')], vec![key(DOWN); 6]].concat(),
            input(0, InputControl::Gain),
            "knob",
        ),
        (vec![ch('2'), key(DOWN)], mix(0, Pan(0)), "knob"),
        (vec![ch('2'), key(DOWN), key(DOWN)], mix(0, Link), "button"),
        (
            vec![ch('2'), key(DOWN), key(DOWN), key(DOWN)],
            mix(0, Fader(0)),
            "fader",
        ),
        (
            vec![ch('3'), key(DOWN)],
            bus(0, BusControl::Source),
            "source",
        ),
        (vec![ch('2')], Control::Mixer(0), "name"),
        (vec![ch('4')], Control::Output(0), "name"),
        (
            vec![ch('4'), key(RIGHT), key(RIGHT)],
            Control::Output(2),
            "name",
        ),
    ];
    for (keys, want, kind) in cases {
        let mut app = app_with(mixed_state());
        for k in keys {
            app.handle_key(k);
        }
        assert_eq!(app.focus(), want);
        let (buf, hits) = render(&app, 120, 40);
        let marked: Vec<_> = hits
            .pixels()
            .iter()
            .filter(|s| match s.widget {
                PixelWidget::Knob { focused, .. }
                | PixelWidget::Fader { focused, .. }
                | PixelWidget::Chip { focused, .. } => focused,
                PixelWidget::Focus => true,
                _ => false,
            })
            .collect();
        assert_eq!(marked.len(), 1, "{want:?}: one marker: {marked:?}");
        let spot = marked[0];
        let ok = matches!(
            (kind, &spot.widget),
            ("knob", PixelWidget::Knob { .. })
                | ("fader", PixelWidget::Fader { .. })
                | ("button", PixelWidget::Chip { .. })
                | ("source" | "name", PixelWidget::Focus)
        );
        assert!(ok, "{want:?}: {kind} marker, got {spot:?}");
        if let Some(hit) = hits.rect(Hit::Control(want)) {
            assert!(spot.rect.intersects(hit), "{want:?}: {spot:?} on {hit:?}");
        }
        // The text form: some of the marker's cells are reversed.
        let r = spot.rect;
        let reversed = (r.top()..r.bottom())
            .flat_map(|y| (r.left()..r.right()).map(move |x| (x, y)))
            .chain((r.left()..r.right()).map(|x| (x, r.bottom())))
            .filter(|&(_, y)| y < buf.area.height)
            .any(|p| buf[p].modifier.contains(Modifier::REVERSED));
        assert!(reversed, "{want:?}: reversed cells in the text form");
    }
}

/// The pixel images of a focused knob, fader and button: a 2-px accent
/// outline around the cell box; the knob's track arc in accent-dim; the
/// fader's cap filled with the accent. Flat: nothing outside the box.
#[test]
fn focused_images() {
    let near = |a: [u8; 4], c: (u8, u8, u8), tol: i32| {
        a[3] > 200
            && (i32::from(a[0]) - i32::from(c.0)).abs() <= tol
            && (i32::from(a[1]) - i32::from(c.1)).abs() <= tol
            && (i32::from(a[2]) - i32::from(c.2)).abs() <= tol
    };
    let cell = (12, 24);
    let k = knob_image_focused(120, 120, Some(0.5), true);
    for (x, y) in [(0, 0), (1, 60), (60, 119), (119, 119)] {
        assert!(
            near(k.get_pixel(x, y).0, PX_ACCENT, 0),
            "outline at {x},{y}"
        );
    }
    assert_eq!(k.get_pixel(3, 60).0[3], 0, "2 px only");
    // The unlit track: accent-dim, where the plain knob has it off.
    let r = 60.0 * 0.80;
    let ra = r - 0.13 * r / 2.0;
    let a = 315f32.to_radians();
    let (tx, ty) = ((60.0 + ra * a.cos()) as u32, (60.0 - ra * a.sin()) as u32);
    assert!(near(
        knob_image(120, 120, Some(0.5), false, true)
            .get_pixel(tx, ty)
            .0,
        PX_ARC_OFF,
        4
    ));
    assert!(
        near(k.get_pixel(tx, ty).0, PX_ACCENT_DIM, 4),
        "{:?}",
        k.get_pixel(tx, ty)
    );

    let f = fader_image_focused(11, 10, cell, -12.0, 6.0, true);
    let cap = (f64::from(fader_row(fader_frac(-12.0, 6.0), 10)) + 0.5) * 24.0;
    let p = f
        .get_pixel((5.5 * 12.0 - 10.0) as u32, (cap - 5.0) as u32)
        .0;
    assert!(near(p, PX_ACCENT, 0), "accent cap {p:?}");
    assert!(!near(p, PX_FADER_CAP, 10));
    assert!(near(f.get_pixel(0, 0).0, PX_ACCENT, 0));

    let b = button_image(6, 1, cell, PX_CHIP, None, true);
    assert!(near(b.get_pixel(0, 12).0, PX_ACCENT, 0));
    assert!(near(b.get_pixel(71, 23).0, PX_ACCENT, 0));
    let plain = button_image(6, 1, cell, PX_CHIP, None, false);
    assert_eq!(plain.get_pixel(0, 12).0[3], 0, "no outline unfocused");
}

/// The footer names what is focused and what the keys will do there.
#[test]
fn footer_names_the_focus() {
    let mut app = app_with(mixed_state());
    for k in [ch('2'), key(RIGHT), key(RIGHT), key(DOWN)] {
        app.handle_key(k);
    }
    assert_eq!(app.focus(), mix(2, MixControl::Pan(0)));
    let (buf, _) = render(&app, 120, 40);
    let footer = cells(&buf, 39, 0, 120);
    assert!(footer.contains("PB 3/4 · left pan L60"), "{footer:?}");
    assert!(footer.contains("+/- adjust"), "{footer:?}");
    assert!(footer.contains("0 reset"), "{footer:?}");

    for (keys, want) in [
        (vec![ch('1')], "IN1 · MON off"),
        (
            vec![
                ch('1'),
                key(DOWN),
                key(DOWN),
                key(DOWN),
                key(DOWN),
                key(DOWN),
                key(DOWN),
            ],
            "IN1 · gain +12 dB",
        ),
        (vec![ch('3'), key(DOWN)], "LOOP 1+2 · source PB 3/4"),
        (
            vec![ch('4'), key(RIGHT), key(RIGHT)],
            "PHONE · level +17 dBu",
        ),
    ] {
        for k in keys {
            app.handle_key(k);
        }
        let (buf, _) = render(&app, 120, 40);
        let footer = cells(&buf, 39, 0, 120);
        assert!(footer.contains(want), "{want:?} in {footer:?}");
    }
    // A switch says space toggles.
    app.handle_key(ch('1'));
    let (buf, _) = render(&app, 120, 40);
    assert!(cells(&buf, 39, 0, 120).contains("space toggle"));
}

// ------------------------------------------------------------ button faces

/// Every face in a column has the same x and width: the six input buttons,
/// a mix strip's LINK and its stacked Ø / SOLO / MUTE, at 120×40, 160×50
/// and a tall window. A label that can't be centred in whole cells is drawn
/// as pixel glyphs in the face, so no label sits half a cell off.
#[test]
fn faces_share_x_and_width() {
    for (w, h) in [(120, 40), (160, 50), (250, 60)] {
        let app = app_with(mixed_state());
        let (buf, hits) = render(&app, w, h);
        let chip = |c: Control| {
            let r = hits.rect(Hit::Control(c)).unwrap();
            hits.pixels()
                .iter()
                .find(|s| s.rect == r && matches!(s.widget, PixelWidget::Chip { .. }))
                .unwrap_or_else(|| panic!("{c:?} has a face at {w}x{h}"))
                .clone()
        };
        for c in 0..2 {
            let faces: Vec<Rect> = InputControl::ORDER[..6]
                .iter()
                .map(|&k| chip(input(c, k)).rect)
                .collect();
            for f in &faces {
                assert_eq!(
                    (f.x, f.width),
                    (faces[0].x, faces[0].width),
                    "IN{} at {w}x{h}: {faces:?}",
                    c + 1
                );
            }
        }
        if hits.switches_stacked() {
            for strip in 1..3 {
                let link = chip(mix(strip, MixControl::Link)).rect;
                for k in [
                    MixControl::Phase(0),
                    MixControl::Solo(0),
                    MixControl::Mute(0),
                ] {
                    let f = chip(mix(strip, k)).rect;
                    assert_eq!(
                        (f.x, f.width),
                        (link.x, link.width),
                        "strip {strip} {k:?} at {w}x{h}"
                    );
                }
            }
        } else {
            for strip in 1..3 {
                let ws: Vec<u16> = [
                    MixControl::Phase(0),
                    MixControl::Solo(0),
                    MixControl::Mute(0),
                ]
                .map(|k| chip(mix(strip, k)).rect.width)
                .to_vec();
                assert!(ws.iter().all(|&x| x == ws[0]), "strip {strip}: {ws:?}");
            }
        }
        // No label half a cell off: odd parity means pixel glyphs.
        for s in hits.pixels() {
            if let PixelWidget::Chip { label, .. } = &s.widget {
                let y = s.rect.y + s.rect.height / 2;
                let text = cells(&buf, y, s.rect.x, s.rect.right());
                let t = text.trim();
                if t.is_empty() || !t.chars().all(|c| !c.is_whitespace()) {
                    continue;
                }
                let odd = (s.rect.width as usize - t.chars().count()) % 2 == 1;
                if odd {
                    let l = label
                        .as_ref()
                        .unwrap_or_else(|| panic!("{t:?} at {s:?}: glyphs ({w}x{h})"));
                    assert_eq!(l.text(), t);
                } else {
                    assert!(label.is_none(), "{t:?} stays terminal text");
                }
            }
        }
    }
}

/// Ø, MON and 48V drawn in a face sit exactly in its middle: the ink's
/// bounding box is centred on the face, to a pixel.
#[test]
fn glyph_labels_centred_in_face() {
    let cell = (12, 24);
    for text in ["Ø", "MON", "48V"] {
        let label = FaceLabel::new(text, PX_LABEL_ON).expect("drawable");
        let img = button_image(6, 1, cell, PX_ACCENT, Some(label), false);
        let ink: Vec<(u32, u32)> = img
            .enumerate_pixels()
            .filter(|(_, _, p)| {
                let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs();
                p.0[3] > 0
                    && d(p.0[0], PX_ACCENT.0) + d(p.0[1], PX_ACCENT.1) + d(p.0[2], PX_ACCENT.2) > 60
            })
            .map(|(x, y, _)| (x, y))
            .collect();
        assert!(!ink.is_empty(), "{text}: ink");
        let x0 = ink.iter().map(|p| p.0).min().unwrap();
        let x1 = ink.iter().map(|p| p.0).max().unwrap();
        let y0 = ink.iter().map(|p| p.1).min().unwrap();
        let y1 = ink.iter().map(|p| p.1).max().unwrap();
        let (cx, cy) = ((x0 + x1 + 1) as f32 / 2.0, (y0 + y1 + 1) as f32 / 2.0);
        assert!(
            (cx - 36.0).abs() <= 1.0,
            "{text}: centre x {cx} ({x0}..{x1})"
        );
        assert!(
            (cy - 12.0).abs() <= 1.0,
            "{text}: centre y {cy} ({y0}..{y1})"
        );
        assert!(x1 - x0 >= 5, "{text}: visible width");
    }
    assert!(FaceLabel::new("Ø", PX_LABEL_ON).is_some());
    assert!(FaceLabel::new("toolong", PX_LABEL_ON).is_none());
}
