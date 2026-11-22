//! The TUI's state and input handling, with no I/O.
//!
//! [`App`] turns keys, mouse events and device reports into [`Action`]s for
//! the event loop to carry out. A change to the [`State`] becomes the frames
//! of [`State::diff`] and a [`Action::Save`]. 48V is never in a diff: turning
//! it on needs the confirm modal and comes out as
//! [`Action::SendConfirmedPhantom`].

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton};
use crossterm::event::{MouseEvent, MouseEventKind};

use crate::frame::Frame;
use crate::state::State;
use crate::tui::theme::Theme;
use crate::tui::ui::{Hit, HitMap};

/// The highest input gain, in dB.
pub const MAX_GAIN_DB: u8 = 20;

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
    /// The Tab order on one input.
    pub const ORDER: [InputControl; 7] = [
        InputControl::Gain,
        InputControl::Mon,
        InputControl::Phantom,
        InputControl::Inst,
        InputControl::Mute,
        InputControl::Solo,
        InputControl::Phase,
    ];

    /// The row in the panel (M, S and Ø share the last one).
    fn row(self) -> usize {
        match self {
            InputControl::Gain => 0,
            InputControl::Mon => 1,
            InputControl::Phantom => 2,
            InputControl::Inst => 3,
            InputControl::Mute | InputControl::Solo | InputControl::Phase => 4,
        }
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

/// The TUI's state.
#[derive(Debug)]
pub struct App {
    state: State,
    phase: Phase,
    focus: Control,
    modal: Option<Modal>,
    /// The input whose gain bar is being dragged.
    drag: Option<usize>,
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
                kind: InputControl::Gain,
            },
            modal: None,
            drag: None,
            theme,
            notice: None,
        }
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
        match key.code {
            KeyCode::Tab => self.cycle(1),
            KeyCode::BackTab => self.cycle(-1),
            KeyCode::Up => self.move_vertical(-1),
            KeyCode::Down => self.move_vertical(1),
            KeyCode::Left => self.move_horizontal(-1),
            KeyCode::Right => self.move_horizontal(1),
            KeyCode::Char('?') => {
                self.modal = Some(Modal::Help);
                self.drag = None;
            }
            KeyCode::Char('h') => return self.adjust(self.focus, -1),
            KeyCode::Char('l') => return self.adjust(self.focus, 1),
            KeyCode::Char('H') => return self.adjust(self.focus, -10),
            KeyCode::Char('L') => return self.adjust(self.focus, 10),
            KeyCode::Char(' ') | KeyCode::Enter => return self.toggle(ch, kind),
            KeyCode::Char('m') => return self.toggle(ch, InputControl::Mute),
            KeyCode::Char('s') => return self.toggle(ch, InputControl::Solo),
            KeyCode::Char('p') => return self.toggle(ch, InputControl::Phase),
            _ => {}
        }
        Vec::new()
    }

    /// Handles a mouse event, using the hit map of the last render.
    pub fn handle_mouse(&mut self, ev: MouseEvent, hits: &HitMap) -> Vec<Action> {
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
            MouseEventKind::Down(MouseButton::Left) => match hit {
                Some(Hit::Control(c)) => {
                    self.focus = c;
                    let Control::Input { ch, kind } = c;
                    if kind != InputControl::Gain {
                        return self.toggle(ch, kind);
                    }
                }
                Some(Hit::GainBar(ch)) => {
                    self.focus = Control::Input {
                        ch,
                        kind: InputControl::Gain,
                    };
                    self.drag = Some(ch);
                    return self.drag_to(ch, ev.column, hits);
                }
                _ => {}
            },
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(ch) = self.drag {
                    return self.drag_to(ch, ev.column, hits);
                }
            }
            MouseEventKind::Up(_) => self.drag = None,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if ev.kind == MouseEventKind::ScrollUp {
                    step
                } else {
                    -step
                };
                match hit {
                    Some(Hit::Control(c)) => return self.adjust(c, delta),
                    Some(Hit::GainBar(ch)) => {
                        let c = Control::Input {
                            ch,
                            kind: InputControl::Gain,
                        };
                        return self.adjust(c, delta);
                    }
                    _ => {}
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

    fn set_gain(&mut self, ch: usize, gain: u8) -> Vec<Action> {
        let mut new = self.state.clone();
        new.inputs[ch].gain_db = gain;
        self.commit(new)
    }

    fn drag_to(&mut self, ch: usize, x: u16, hits: &HitMap) -> Vec<Action> {
        let Some(bar) = hits.rect(Hit::GainBar(ch)) else {
            return Vec::new();
        };
        let span = f64::from(bar.width.saturating_sub(1).max(1));
        let offset = f64::from(x.saturating_sub(bar.x)).min(span);
        let gain = (offset / span * f64::from(MAX_GAIN_DB)).round() as u8;
        self.set_gain(ch, gain)
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

    /// Up/Down: the previous/next row of the same input. From the M/S/Ø
    /// row up goes to INST; down from INST goes to M.
    fn move_vertical(&mut self, step: isize) {
        let Control::Input { ch, kind } = self.focus;
        let rows = [
            InputControl::Gain,
            InputControl::Mon,
            InputControl::Phantom,
            InputControl::Inst,
            InputControl::Mute,
        ];
        let row = (kind.row() as isize + step).clamp(0, rows.len() as isize - 1) as usize;
        if row != kind.row() {
            self.focus = Control::Input {
                ch,
                kind: rows[row],
            };
        }
    }

    /// Left/Right: across to the other input; along the M/S/Ø row, through
    /// IN1's switches and then IN2's.
    fn move_horizontal(&mut self, step: isize) {
        let Control::Input { ch, kind } = self.focus;
        if kind.row() == 4 {
            let row: Vec<Control> = Control::all()
                .into_iter()
                .filter(|c| matches!(c, Control::Input { kind, .. } if kind.row() == 4))
                .collect();
            let i = row.iter().position(|&c| c == self.focus).unwrap_or(0) as isize;
            let j = (i + step).clamp(0, row.len() as isize - 1) as usize;
            self.focus = row[j];
        } else {
            let ch = (ch as isize + step).clamp(0, 1) as usize;
            self.focus = Control::Input { ch, kind };
        }
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
