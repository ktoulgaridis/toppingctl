//! The TUI's state and input handling, with no I/O.
//!
//! [`App`] turns keys, mouse events and device reports into [`Action`]s for
//! the event loop to carry out. A change to the [`State`] becomes the frames
//! of [`State::diff`] and a [`Action::Save`]. 48V is never in a diff: turning
//! it on needs the confirm modal and comes out as
//! [`Action::SendConfirmedPhantom`].
//!
//! Only the input panel takes input in this version (#34); the mixer,
//! loopback and output are drawn from the state, and the mix tabs pick
//! which mix the mixer shows. Their controls come with #14 and #15.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton};
use crossterm::event::{MouseEvent, MouseEventKind};

use crate::frame::Frame;
use crate::state::{Meters, State};
use crate::tui::theme::Theme;
use crate::tui::ui::{Hit, HitMap};
use crate::tui::widgets::meter::ClipHold;

/// The highest input gain, in dB.
pub const MAX_GAIN_DB: u8 = 20;
/// The input gain a reset (double-click, `0`) goes back to.
pub const DEFAULT_GAIN_DB: u8 = 0;
/// Two clicks on the same knob within this are a double-click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// What the event loop should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Send a frame with `Connection::send`.
    Send(Frame),
    /// Send a frame with `Connection::send_confirmed_phantom`; the user has
    /// just confirmed it.
    SendConfirmedPhantom(Frame),
    /// Save [`App::state`] to the store.
    Save,
    /// Restore the terminal and exit.
    Quit,
}

/// A control on one input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputControl {
    Gain,
    Mon,
    Phantom,
    Inst,
    Mute,
    Solo,
    Phase,
}

impl InputControl {
    /// The order on one input, top to bottom as drawn: the switches, then
    /// the gain knob. Tab and ↑↓ follow it.
    pub const ORDER: [InputControl; 7] = [
        InputControl::Mon,
        InputControl::Phantom,
        InputControl::Inst,
        InputControl::Solo,
        InputControl::Mute,
        InputControl::Phase,
        InputControl::Gain,
    ];

    fn index(self) -> usize {
        Self::ORDER.iter().position(|&k| k == self).unwrap_or(0)
    }
}

/// Something that can have focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Control {
    /// A control on IN1 (`ch` 0) or IN2 (`ch` 1).
    Input { ch: usize, kind: InputControl },
}

impl Control {
    /// All focusable controls, in Tab order.
    pub fn all() -> Vec<Control> {
        (0..2)
            .flat_map(|ch| {
                InputControl::ORDER
                    .iter()
                    .map(move |&kind| Control::Input { ch, kind })
            })
            .collect()
    }
}

/// A dialog on top of the screen. While one is open, keys and clicks go
/// only to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modal {
    /// First run: push the defaults to the device?
    PushDefaults,
    /// Turn on 48V on IN1 (`ch` 0) or IN2 (`ch` 1)?
    Phantom { ch: usize },
    /// The key and mouse reference.
    Help,
}

/// Where the session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the device: only the splash is shown.
    Connecting,
    /// The device has a state toppingctl knows (or the user is asked to
    /// push one).
    Synced,
}

/// A gain knob being dragged: which input, where the press was, and the
/// gain then.
#[derive(Clone, Copy, Debug)]
struct Drag {
    ch: usize,
    y: u16,
    gain: u8,
}

/// The meters' clip lights, in the shape of [`Meters`].
#[derive(Clone, Debug, Default)]
pub struct Clips {
    holds: [ClipHold; 14],
    lit: [bool; 14],
}

/// Which meter a clip light belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterId {
    /// IN1 (0), IN2 (1).
    Input(usize),
    /// PB 1/2 (0) … 5/6 (2), L (0) / R (1).
    Playback(usize, usize),
    /// Output 1+2, L / R.
    Output(usize),
    /// Loopback 1+2 (0), 3+4 (1), L / R.
    Loopback(usize, usize),
}

impl MeterId {
    fn index(self) -> usize {
        match self {
            MeterId::Input(i) => i.min(1),
            MeterId::Playback(p, c) => 2 + p.min(2) * 2 + c.min(1),
            MeterId::Output(c) => 8 + c.min(1),
            MeterId::Loopback(l, c) => 10 + l.min(1) * 2 + c.min(1),
        }
    }
}

impl Clips {
    /// Takes the latest readings at `now`.
    pub fn update(&mut self, m: &Meters, now: Instant) {
        let values = [
            m.inputs[0],
            m.inputs[1],
            m.playback[0][0],
            m.playback[0][1],
            m.playback[1][0],
            m.playback[1][1],
            m.playback[2][0],
            m.playback[2][1],
            m.output[0],
            m.output[1],
            m.loopbacks[0][0],
            m.loopbacks[0][1],
            m.loopbacks[1][0],
            m.loopbacks[1][1],
        ];
        for (i, db) in values.into_iter().enumerate() {
            self.holds[i].update(db, now);
            self.lit[i] = self.holds[i].lit(now);
        }
    }

    /// Whether `id`'s clip light is on (as of the last update).
    pub fn lit(&self, id: MeterId) -> bool {
        self.lit[id.index()]
    }
}

/// The TUI's state.
#[derive(Debug)]
pub struct App {
    state: State,
    phase: Phase,
    focus: Control,
    modal: Option<Modal>,
    drag: Option<Drag>,
    /// The last left click: what it hit and when (for double-clicks).
    last_click: Option<(Hit, Instant)>,
    /// The mix the mixer shows: A (0), B (1), C (2).
    mix: usize,
    clips: Clips,
    theme: Theme,
    notice: Option<String>,
}

impl App {
    /// A new session, connecting.
    pub fn new(theme: Theme) -> Self {
        App {
            state: State::default(),
            phase: Phase::Connecting,
            focus: Control::Input {
                ch: 0,
                kind: InputControl::ORDER[0],
            },
            modal: None,
            drag: None,
            last_click: None,
            mix: 0,
            clips: Clips::default(),
            theme,
            notice: None,
        }
    }

    /// The mix the mixer shows: A (0), B (1), C (2).
    pub fn mix(&self) -> usize {
        self.mix
    }

    pub fn clips(&self) -> &Clips {
        &self.clips
    }

    /// The meters as of `now`: updates the clip lights.
    pub fn tick(&mut self, meters: &Meters, now: Instant) {
        self.clips.update(meters, now);
    }

    /// The device is at `state` (from `sync::connect`).
    pub fn synced(&mut self, state: State) {
        self.state = state;
        self.phase = Phase::Synced;
        self.modal = None;
    }

    /// First run: nothing saved. Asks before pushing `state` (the defaults
    /// with the device's reply applied).
    pub fn needs_initial_state(&mut self, state: State) {
        self.state = state;
        self.phase = Phase::Synced;
        self.modal = Some(Modal::PushDefaults);
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn focus(&self) -> Control {
        self.focus
    }

    pub fn modal(&self) -> Option<Modal> {
        self.modal
    }

    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// A one-line message for the footer, such as a failed save.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn set_notice(&mut self, notice: Option<String>) {
        self.notice = notice;
    }

    /// Whether the controls are on screen and take input.
    fn controls_live(&self) -> bool {
        self.phase == Phase::Synced && self.modal != Some(Modal::PushDefaults)
    }

    /// Handles a key press.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('c' | 'C')) {
            return vec![Action::Quit];
        }
        let yes = matches!(key.code, KeyCode::Char('y' | 'Y'));
        let no = matches!(key.code, KeyCode::Char('n' | 'N') | KeyCode::Esc);
        match self.modal {
            Some(Modal::PushDefaults) if yes => return self.push_defaults(),
            Some(Modal::PushDefaults) if no || key.code == KeyCode::Char('q') => {
                return vec![Action::Quit]
            }
            Some(Modal::Phantom { ch }) if yes => return self.confirm_phantom(ch),
            Some(Modal::Phantom { .. }) if no => {
                self.modal = None;
                return Vec::new();
            }
            Some(Modal::Help) if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) => {
                self.modal = None;
                return Vec::new();
            }
            Some(Modal::Help) if key.code == KeyCode::Char('q') => return vec![Action::Quit],
            Some(_) => return Vec::new(),
            None => {}
        }
        if key.code == KeyCode::Char('q') {
            return vec![Action::Quit];
        }
        if self.phase != Phase::Synced {
            return Vec::new();
        }
        let Control::Input { ch, kind } = self.focus;
        let step = if key.modifiers.contains(KeyModifiers::SHIFT) {
            10
        } else {
            1
        };
        match key.code {
            KeyCode::Tab => self.cycle(1),
            KeyCode::BackTab => self.cycle(-1),
            KeyCode::Up => self.move_vertical(-1),
            KeyCode::Down => self.move_vertical(1),
            KeyCode::Left | KeyCode::Right => {
                let delta = if key.code == KeyCode::Left {
                    -step
                } else {
                    step
                };
                if kind == InputControl::Gain {
                    return self.adjust(self.focus, delta);
                }
                self.focus = Control::Input {
                    ch: if delta < 0 { 0 } else { 1 },
                    kind,
                };
            }
            KeyCode::Char('?') => {
                self.modal = Some(Modal::Help);
                self.drag = None;
            }
            KeyCode::Char('0') => return self.reset(self.focus),
            KeyCode::Char(c @ ('a' | 'b' | 'c')) => self.mix = usize::from(c as u8 - b'a'),
            KeyCode::Char(' ') | KeyCode::Enter => return self.toggle(ch, kind),
            _ => {}
        }
        Vec::new()
    }

    /// Handles a mouse event, using the hit map of the last render.
    pub fn handle_mouse(&mut self, ev: MouseEvent, hits: &HitMap) -> Vec<Action> {
        self.handle_mouse_at(ev, hits, Instant::now())
    }

    /// [`App::handle_mouse`] at time `now` (double-clicks are timed).
    pub fn handle_mouse_at(&mut self, ev: MouseEvent, hits: &HitMap, now: Instant) -> Vec<Action> {
        let hit = hits.get(ev.column, ev.row);
        if let Some(modal) = self.modal {
            if ev.kind != MouseEventKind::Down(MouseButton::Left) {
                return Vec::new();
            }
            return match (modal, hit) {
                (Modal::PushDefaults, Some(Hit::Yes)) => self.push_defaults(),
                (Modal::PushDefaults, Some(Hit::No)) => vec![Action::Quit],
                (Modal::Phantom { ch }, Some(Hit::Yes)) => self.confirm_phantom(ch),
                (Modal::Phantom { .. }, Some(Hit::No)) => {
                    self.modal = None;
                    Vec::new()
                }
                (Modal::Help, Some(Hit::No)) => {
                    self.modal = None;
                    Vec::new()
                }
                _ => Vec::new(),
            };
        }
        if !self.controls_live() {
            return Vec::new();
        }
        let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
        let step = if shift { 10 } else { 1 };
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let double = matches!(
                    (hit, self.last_click),
                    (Some(h), Some((last, at))) if h == last && now.saturating_duration_since(at) <= DOUBLE_CLICK
                );
                // A double-click is used up: a third click starts afresh.
                self.last_click = if double { None } else { hit.map(|h| (h, now)) };
                match hit {
                    Some(Hit::Control(c)) => {
                        self.focus = c;
                        let Control::Input { ch, kind } = c;
                        if kind != InputControl::Gain {
                            return self.toggle(ch, kind);
                        }
                        if double {
                            self.drag = None;
                            return self.reset(c);
                        }
                        self.drag = Some(Drag {
                            ch,
                            y: ev.row,
                            gain: self.state.inputs[ch].gain_db.min(MAX_GAIN_DB),
                        });
                    }
                    Some(Hit::MixTab(m)) => self.mix = m,
                    _ => {}
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(d) = self.drag {
                    let rows = i32::from(d.y) - i32::from(ev.row);
                    let gain = (i32::from(d.gain) + rows).clamp(0, i32::from(MAX_GAIN_DB));
                    return self.set_gain(d.ch, gain as u8);
                }
            }
            MouseEventKind::Up(_) => self.drag = None,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if ev.kind == MouseEventKind::ScrollUp {
                    step
                } else {
                    -step
                };
                if let Some(Hit::Control(c)) = hit {
                    return self.adjust(c, delta);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// Applies a device report (a button press or an echo). Saves when it
    /// changes what is persisted, but not while the first-run question is
    /// open: a save then would skip the question on the next launch.
    pub fn on_report(&mut self, f: &Frame) -> Vec<Action> {
        let before = persisted(&self.state);
        self.state.apply(f);
        if self.controls_live() && persisted(&self.state) != before {
            vec![Action::Save]
        } else {
            Vec::new()
        }
    }

    fn push_defaults(&mut self) -> Vec<Action> {
        self.modal = None;
        let mut out: Vec<Action> = self
            .state
            .frames()
            .into_iter()
            .filter(|f| f.group != 0x10)
            .map(Action::Send)
            .collect();
        out.push(Action::Save);
        out
    }

    fn confirm_phantom(&mut self, ch: usize) -> Vec<Action> {
        self.modal = None;
        self.state.inputs[ch].phantom = Some(true);
        vec![Action::SendConfirmedPhantom(phantom_frame(ch, true))]
    }

    /// Moves `new` into place and returns the frames that get the device
    /// there, then a save. Nothing when nothing changed.
    fn commit(&mut self, new: State) -> Vec<Action> {
        if new == self.state {
            return Vec::new();
        }
        let mut out: Vec<Action> = self
            .state
            .diff(&new)
            .into_iter()
            .map(Action::Send)
            .collect();
        self.state = new;
        out.push(Action::Save);
        out
    }

    fn adjust(&mut self, c: Control, delta: i32) -> Vec<Action> {
        let Control::Input { ch, kind } = c;
        if kind != InputControl::Gain {
            return Vec::new();
        }
        let gain = i32::from(self.state.inputs[ch].gain_db.min(MAX_GAIN_DB));
        let gain = (gain + delta).clamp(0, i32::from(MAX_GAIN_DB)) as u8;
        self.set_gain(ch, gain)
    }

    /// Puts a knob back to its default.
    fn reset(&mut self, c: Control) -> Vec<Action> {
        match c {
            Control::Input {
                ch,
                kind: InputControl::Gain,
            } => self.set_gain(ch, DEFAULT_GAIN_DB),
            _ => Vec::new(),
        }
    }

    fn set_gain(&mut self, ch: usize, gain: u8) -> Vec<Action> {
        let mut new = self.state.clone();
        new.inputs[ch].gain_db = gain;
        self.commit(new)
    }

    fn toggle(&mut self, ch: usize, kind: InputControl) -> Vec<Action> {
        let mut new = self.state.clone();
        let input = &mut new.inputs[ch];
        match kind {
            InputControl::Gain => return Vec::new(),
            InputControl::Phantom => {
                return if self.state.inputs[ch].phantom == Some(true) {
                    self.state.inputs[ch].phantom = Some(false);
                    vec![Action::Send(phantom_frame(ch, false))]
                } else {
                    self.modal = Some(Modal::Phantom { ch });
                    self.drag = None;
                    Vec::new()
                };
            }
            InputControl::Mon => input.mon = !input.mon,
            InputControl::Inst => input.inst = !input.inst,
            InputControl::Mute => input.mute = !input.mute,
            InputControl::Solo => input.solo = !input.solo,
            InputControl::Phase => input.phase = !input.phase,
        }
        self.commit(new)
    }

    fn cycle(&mut self, step: isize) {
        let all = Control::all();
        let i = all.iter().position(|&c| c == self.focus).unwrap_or(0);
        let n = all.len() as isize;
        self.focus = all[(i as isize + step).rem_euclid(n) as usize];
    }

    /// Up/Down: along the input's column, clamped at the ends.
    fn move_vertical(&mut self, step: isize) {
        let Control::Input { ch, kind } = self.focus;
        let n = InputControl::ORDER.len() as isize;
        let i = (kind.index() as isize + step).clamp(0, n - 1) as usize;
        self.focus = Control::Input {
            ch,
            kind: InputControl::ORDER[i],
        };
    }
}

/// `state` without what the store leaves out (48V and the readout).
fn persisted(state: &State) -> State {
    let mut s = state.clone();
    s.readout = Default::default();
    for input in &mut s.inputs {
        input.phantom = None;
    }
    s
}

/// The 48V register of input `ch`.
fn phantom_frame(ch: usize, on: bool) -> Frame {
    Frame {
        group: 0x21 + ch as u8,
        param: 0x02,
        value: i32::from(on),
    }
}
