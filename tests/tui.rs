//! The TUI shell and the INPUTS panel (#13), tested on the pure `App` and on
//! ratatui's `TestBackend`. No terminal, no hardware.
//!
//! Golden snapshots live in `tests/snapshots/` as plain text. Run with
//! `UPDATE_SNAPSHOTS=1` to rewrite them after an intended change.

use std::fs;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Terminal;

use toppingctl::frame::Frame;
use toppingctl::state::{Meters, State, Version};
use toppingctl::tui::app::{Action, App, Control, InputControl, Modal};
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

/// A state with something to show in every input control.
fn fake_state() -> State {
    let mut s = State::default();
    s.inputs[0].gain_db = 12;
    s.inputs[0].mon = true;
    s.inputs[0].phantom = Some(false);
    s.inputs[1].gain_db = 4;
    s.inputs[1].inst = true;
    s.inputs[1].mute = true;
    s.inputs[1].phantom = Some(true);
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

fn fake_meters() -> Meters {
    Meters {
        inputs: [-18.4, -4.0],
        ..Meters::default()
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

fn confirmed(actions: &[Action]) -> Vec<Frame> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::SendConfirmedPhantom(f) => Some(*f),
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

#[test]
fn splash_until_synced() {
    let meters = fake_meters();
    let mut app = App::new(Theme::color());
    let (buf, hits) = render(&app, &meters, 100, 30);
    let splash = text(&buf);
    assert!(splash.contains("Connecting"), "splash:\n{splash}");
    for panel in ["INPUTS", "MIX", "OUT", "LOOPBACK", "MON", "48V"] {
        assert!(!splash.contains(panel), "{panel} on the splash:\n{splash}");
    }
    assert!(hits.is_empty(), "no interactive widgets while connecting");
    // Keys do nothing to the device while connecting.
    for k in [ch('l'), ch(' '), ch('m'), key(KeyCode::Tab)] {
        assert!(app.handle_key(k).is_empty());
    }

    app.synced(fake_state());
    let (buf, hits) = render(&app, &meters, 100, 30);
    let full = text(&buf);
    assert!(!full.contains("Connecting"), "full frame:\n{full}");
    for panel in [
        "INPUTS", "MIX", "OUT", "LOOPBACK", "IN1", "IN2", "MON", "48V", "INST",
    ] {
        assert!(full.contains(panel), "{panel} missing:\n{full}");
    }
    assert!(!hits.is_empty());
}

#[test]
fn keys_drive_app() {
    let mut app = synced_app(Theme::color());
    use InputControl::*;

    // Tab cycles over every input control, IN1 then IN2, and wraps.
    let order = [Gain, Mon, Phantom, Inst, Mute, Solo, Phase];
    let mut seen = vec![app.focus()];
    for _ in 0..14 {
        app.handle_key(key(KeyCode::Tab));
        seen.push(app.focus());
    }
    let expected: Vec<Control> = (0..2)
        .flat_map(|ch| order.iter().map(move |&k| input(ch, k)))
        .chain([input(0, Gain)])
        .collect();
    assert_eq!(seen, expected);
    app.handle_key(key(KeyCode::BackTab));
    assert_eq!(
        app.focus(),
        input(1, Phase),
        "Shift-Tab goes back and wraps"
    );
    app.handle_key(key(KeyCode::Tab));

    // Arrows move focus too.
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.focus(), input(0, Mon));
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.focus(), input(1, Mon));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.focus(), input(1, Gain));
    app.handle_key(key(KeyCode::Left));
    assert_eq!(app.focus(), input(0, Gain));

    // h/l ±1 dB, H/L ±10 dB, clamped to 0 ..= +20; each change is a diff + save.
    assert_eq!(app.state().inputs[0].gain_db, 12);
    let actions = app.handle_key(ch('l'));
    assert_eq!(app.state().inputs[0].gain_db, 13);
    let mut expected = fake_state();
    expected.inputs[0].gain_db = 13;
    assert_eq!(sends(&actions), fake_state().diff(&expected));
    assert_eq!(sends(&actions).len(), 1);
    assert_eq!(actions.last(), Some(&Action::Save));
    app.handle_key(ch('L'));
    assert_eq!(app.state().inputs[0].gain_db, 20, "clamped at +20");
    assert!(app.handle_key(ch('l')).is_empty(), "no change at the top");
    app.handle_key(ch('h'));
    assert_eq!(app.state().inputs[0].gain_db, 19);
    app.handle_key(ch('H'));
    app.handle_key(ch('H'));
    assert_eq!(app.state().inputs[0].gain_db, 0, "clamped at 0");
    assert!(
        app.handle_key(ch('h')).is_empty(),
        "no change at the bottom"
    );

    // Space toggles MON and INST.
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.focus(), input(0, Mon));
    let actions = app.handle_key(ch(' '));
    assert!(!app.state().inputs[0].mon);
    assert_eq!(sends(&actions), vec![frame(0x21, 0x01, 0)]);
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.focus(), input(0, Inst));
    let actions = app.handle_key(ch(' '));
    assert!(app.state().inputs[0].inst);
    assert_eq!(sends(&actions), vec![frame(0x21, 0x03, 1)]);

    // m/s/p act on the focused input: IN2 here.
    app.handle_key(key(KeyCode::Right));
    assert_eq!(app.focus(), input(1, Inst));
    app.handle_key(ch('m'));
    assert!(!app.state().inputs[1].mute, "IN2 was muted; m unmutes");
    let actions = app.handle_key(ch('s'));
    assert!(app.state().inputs[1].solo);
    assert!(!sends(&actions).is_empty(), "solo IN2 silences IN1");
    app.handle_key(ch('p'));
    assert!(app.state().inputs[1].phase);
    assert!(!app.state().inputs[0].mute && !app.state().inputs[0].solo);
    assert!(!app.state().inputs[0].phase);

    // q and Ctrl-C quit.
    assert_eq!(app.handle_key(ch('q')), vec![Action::Quit]);
    assert_eq!(app.handle_key(ctrl('c')), vec![Action::Quit]);
}

#[test]
fn phantom_needs_confirmation() {
    let mut app = synced_app(Theme::color());
    // IN1 48V is off. Focus it and press Space: a modal, nothing sent.
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(app.focus(), input(0, InputControl::Phantom));
    assert!(app.handle_key(ch(' ')).is_empty());
    assert_eq!(app.modal(), Some(Modal::Phantom { ch: 0 }));
    let (buf, _) = render(&app, &fake_meters(), 100, 30);
    let shown = text(&buf);
    assert!(
        shown.contains("Turn on 48V phantom power on IN1?") && shown.contains("ribbon"),
        "{shown}"
    );

    // While the modal is open, other keys do nothing.
    for k in [ch('l'), ch('m'), key(KeyCode::Tab), ch(' ')] {
        assert!(app.handle_key(k).is_empty());
    }
    assert_eq!(app.modal(), Some(Modal::Phantom { ch: 0 }));

    // n and Esc cancel.
    assert!(app.handle_key(ch('n')).is_empty());
    assert_eq!(app.modal(), None);
    assert_eq!(app.state().inputs[0].phantom, Some(false));
    app.handle_key(ch(' '));
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert_eq!(app.modal(), None);

    // y sends exactly one confirmed-phantom action.
    app.handle_key(ch(' '));
    let actions = app.handle_key(ch('y'));
    assert_eq!(
        actions,
        vec![Action::SendConfirmedPhantom(frame(0x21, 0x02, 1))]
    );
    assert_eq!(app.modal(), None);
    assert_eq!(app.state().inputs[0].phantom, Some(true));

    // Off is sent directly.
    let actions = app.handle_key(ch(' '));
    assert_eq!(actions, vec![Action::Send(frame(0x21, 0x02, 0))]);
    assert_eq!(app.modal(), None);
    assert_eq!(app.state().inputs[0].phantom, Some(false));

    // Unknown 48V (no device report yet) counts as off: it asks first.
    let mut app = App::new(Theme::color());
    app.synced(State::default());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert!(app.handle_key(ch(' ')).is_empty());
    assert_eq!(app.modal(), Some(Modal::Phantom { ch: 0 }));

    // A device report updates 48V.
    let mut app = synced_app(Theme::color());
    app.on_report(&frame(0x22, 0x02, 0));
    assert_eq!(app.state().inputs[1].phantom, Some(false));
}

#[test]
fn first_run_asks_before_pushing() {
    let mut device = State::default();
    device.system.brightness = 0;
    let mut app = App::new(Theme::color());
    app.needs_initial_state(device.clone());
    assert_eq!(app.modal(), Some(Modal::PushDefaults));
    let (buf, hits) = render(&app, &fake_meters(), 100, 30);
    let shown = text(&buf);
    assert!(
        shown.contains("No saved state") && shown.contains("Push defaults to the device?"),
        "{shown}"
    );
    assert!(
        !shown.contains("INPUTS"),
        "no controls behind the modal:\n{shown}"
    );
    assert!(hits.rect(Hit::Yes).is_some() && hits.rect(Hit::No).is_some());

    // Nothing is pushed by other keys.
    for k in [
        ch('l'),
        ch(' '),
        ch('m'),
        key(KeyCode::Tab),
        key(KeyCode::Enter),
    ] {
        assert!(app.handle_key(k).is_empty());
    }
    assert_eq!(app.modal(), Some(Modal::PushDefaults));

    // y pushes the state (the device's own settings excepted) and saves it.
    let actions = app.handle_key(ch('y'));
    let expected: Vec<Frame> = device
        .frames()
        .into_iter()
        .filter(|f| f.group != 0x10)
        .collect();
    assert_eq!(sends(&actions), expected);
    assert_eq!(actions.last(), Some(&Action::Save));
    assert!(confirmed(&actions).is_empty(), "48V is never pushed");
    assert_eq!(app.modal(), None);
    assert_eq!(app.state(), &device);
    let (buf, _) = render(&app, &fake_meters(), 100, 30);
    assert!(text(&buf).contains("INPUTS"));

    // n quits without writing anything.
    let mut app = App::new(Theme::color());
    app.needs_initial_state(device);
    assert_eq!(app.handle_key(ch('n')), vec![Action::Quit]);
}

#[test]
fn snapshots() {
    let app = synced_app(Theme::color());
    let meters = fake_meters();
    let (buf, _) = render(&app, &meters, 100, 30);
    assert_snapshot("full_100x30", &text(&buf));
    let (buf, _) = render(&app, &meters, 80, 24);
    assert_snapshot("full_80x24", &text(&buf));
    let (buf, hits) = render(&app, &meters, 79, 24);
    assert_snapshot("too_small", &text(&buf));
    assert!(hits.is_empty(), "nothing to click when too small");
    let (buf, _) = render(&app, &meters, 80, 23);
    assert!(text(&buf).contains("too small"));

    let mut splash = App::new(Theme::color());
    let (buf, _) = render(&splash, &meters, 80, 24);
    assert_snapshot("splash_80x24", &text(&buf));
    splash.needs_initial_state(State::default());
    let (buf, _) = render(&splash, &meters, 80, 24);
    assert_snapshot("push_defaults_80x24", &text(&buf));

    let mut modal = synced_app(Theme::color());
    modal.handle_key(key(KeyCode::Tab));
    modal.handle_key(key(KeyCode::Tab));
    modal.handle_key(ch(' '));
    let (buf, _) = render(&modal, &meters, 80, 24);
    assert_snapshot("phantom_80x24", &text(&buf));

    let mut help = synced_app(Theme::color());
    help.handle_key(ch('?'));
    assert_eq!(help.modal(), Some(Modal::Help));
    let (buf, _) = render(&help, &meters, 80, 24);
    assert_snapshot("help_80x24", &text(&buf));
    help.handle_key(key(KeyCode::Esc));
    assert_eq!(help.modal(), None);
}

/// A focused control is drawn in the accent colour and bold.
#[test]
fn focus_is_accent_and_bold() {
    let app = synced_app(Theme::color());
    let (buf, hits) = render(&app, &fake_meters(), 100, 30);
    let r = hits
        .rect(Hit::Control(input(0, InputControl::Gain)))
        .unwrap();
    let cells: Vec<_> = (r.x..r.right())
        .map(|x| &buf[(x, r.y)])
        .filter(|c| c.symbol() != " ")
        .collect();
    assert!(cells
        .iter()
        .any(|c| c.fg == Theme::ACCENT && c.modifier.contains(ratatui::style::Modifier::BOLD)));
}

fn mouse(kind: MouseEventKind, x: u16, y: u16, modifiers: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers,
    }
}

fn click_at(app: &mut App, hits: &HitMap, r: Rect) -> Vec<Action> {
    let none = KeyModifiers::NONE;
    let down = MouseEventKind::Down(MouseButton::Left);
    let up = MouseEventKind::Up(MouseButton::Left);
    let mut actions = app.handle_mouse(mouse(down, r.x, r.y, none), hits);
    actions.extend(app.handle_mouse(mouse(up, r.x, r.y, none), hits));
    actions
}

#[test]
fn mouse_drives_app() {
    use InputControl::*;
    let meters = fake_meters();
    let mut app = synced_app(Theme::color());
    let (_, hits) = render(&app, &meters, 100, 30);
    let rect = |c: Control| hits.rect(Hit::Control(c)).unwrap();
    let none = KeyModifiers::NONE;

    // Click focuses a control and flips a toggle.
    let actions = click_at(&mut app, &hits, rect(input(1, Inst)));
    assert_eq!(app.focus(), input(1, Inst));
    assert!(!app.state().inputs[1].inst);
    assert_eq!(sends(&actions), vec![frame(0x22, 0x03, 0)]);
    click_at(&mut app, &hits, rect(input(0, Solo)));
    assert_eq!(app.focus(), input(0, Solo));
    assert!(app.state().inputs[0].solo);
    let r = rect(input(0, Gain));
    click_at(
        &mut app,
        &hits,
        Rect {
            x: r.right() - 1,
            ..r
        },
    );
    assert_eq!(app.focus(), input(0, Gain));

    // The wheel adjusts the control under the pointer: ±1, Shift ×10, clamped.
    assert_eq!(app.state().inputs[1].gain_db, 4);
    let g2 = rect(input(1, Gain));
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, g2.x, g2.y, none), &hits);
    assert_eq!(app.state().inputs[1].gain_db, 5);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, g2.x, g2.y, none), &hits);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, g2.x, g2.y, none), &hits);
    assert_eq!(app.state().inputs[1].gain_db, 3);
    let shift = KeyModifiers::SHIFT;
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, g2.x, g2.y, shift), &hits);
    assert_eq!(app.state().inputs[1].gain_db, 13);
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, g2.x, g2.y, shift), &hits);
    assert_eq!(app.state().inputs[1].gain_db, 20, "clamped");
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, g2.x, g2.y, shift), &hits);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, g2.x, g2.y, shift), &hits);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, g2.x, g2.y, shift), &hits);
    assert_eq!(app.state().inputs[1].gain_db, 0, "clamped");

    // A drag on the IN1 gain bar sets gain from x: left end 0, right end +20.
    let bar = hits.rect(Hit::GainBar(0)).unwrap();
    let left = MouseEventKind::Down(MouseButton::Left);
    let drag = MouseEventKind::Drag(MouseButton::Left);
    app.handle_mouse(mouse(left, bar.x, bar.y, none), &hits);
    assert_eq!(app.state().inputs[0].gain_db, 0);
    let actions = app.handle_mouse(mouse(drag, bar.right() - 1, bar.y, none), &hits);
    assert_eq!(app.state().inputs[0].gain_db, 20);
    assert!(actions.contains(&Action::Save));
    // In between it is linear: the bar's first cell is 0, its last +20.
    app.handle_mouse(mouse(drag, bar.x + 4, bar.y, none), &hits);
    let expected = (4.0 * 20.0 / f64::from(bar.width - 1)).round() as u8;
    assert_eq!(app.state().inputs[0].gain_db, expected);
    // Past the ends it clamps, even off the bar's row.
    app.handle_mouse(mouse(drag, bar.right() + 5, bar.y + 3, none), &hits);
    assert_eq!(app.state().inputs[0].gain_db, 20);
    app.handle_mouse(mouse(drag, 0, bar.y, none), &hits);
    assert_eq!(app.state().inputs[0].gain_db, 0);
    let up = MouseEventKind::Up(MouseButton::Left);
    app.handle_mouse(mouse(up, 0, bar.y, none), &hits);
    app.handle_mouse(mouse(drag, bar.right() - 1, bar.y, none), &hits);
    assert_eq!(app.state().inputs[0].gain_db, 0, "the drag ended");

    // A click on 48V opens the confirm modal and sends nothing.
    let actions = click_at(&mut app, &hits, rect(input(0, Phantom)));
    assert!(actions.is_empty(), "{actions:?}");
    assert_eq!(app.modal(), Some(Modal::Phantom { ch: 0 }));
    assert_eq!(app.state().inputs[0].phantom, Some(false));

    // With the modal open, clicks outside it do nothing; its buttons work.
    let (_, hits) = render(&app, &meters, 100, 30);
    for c in [input(0, Phantom), input(0, Mon), input(1, Gain)] {
        if let Some(r) = hits.rect(Hit::Control(c)) {
            assert!(click_at(&mut app, &hits, r).is_empty());
        }
    }
    assert!(click_at(&mut app, &hits, Rect::new(0, 0, 1, 1)).is_empty());
    assert_eq!(app.modal(), Some(Modal::Phantom { ch: 0 }));
    let no = hits.rect(Hit::No).unwrap();
    assert!(click_at(&mut app, &hits, no).is_empty());
    assert_eq!(app.modal(), None);

    let (_, hits) = render(&app, &meters, 100, 30);
    click_at(&mut app, &hits, rect(input(0, Phantom)));
    let (_, hits) = render(&app, &meters, 100, 30);
    let yes = hits.rect(Hit::Yes).unwrap();
    let actions = click_at(&mut app, &hits, yes);
    assert_eq!(
        actions,
        vec![Action::SendConfirmedPhantom(frame(0x21, 0x02, 1))]
    );
    assert_eq!(app.modal(), None);
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
        for (w, h) in [(100, 30), (80, 24), (60, 20)] {
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
    let (coloured, _) = render(&synced_app(Theme::color()), &meters, 100, 30);
    let (uncoloured, _) = render(&synced_app(plain), &meters, 100, 30);
    assert_eq!(text(&coloured), text(&uncoloured));

    // Focus is still visible without colour (bold).
    let (_, hits) = render(&synced_app(plain), &meters, 100, 30);
    let r = hits
        .rect(Hit::Control(input(0, InputControl::Gain)))
        .unwrap();
    assert!((r.x..r.right()).any(|x| uncoloured[(x, r.y)]
        .modifier
        .contains(ratatui::style::Modifier::BOLD)));
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
