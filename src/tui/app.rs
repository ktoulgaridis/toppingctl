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
use crate::rate::RATES;
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
    /// Set the interface's sample rate (Hz), through the OS.
    SetRate(u32),
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
    /// A mixer strip: IN 1+2 (0), PB 1/2 (1), PB 3/4 (2), PB 5/6 (3).
    /// Highlighted only: its controls come with #14.
    Mixer(usize),
    /// LOOP 1+2 (0), LOOP 3+4 (1). Highlighted only until #15.
    Loopback(usize),
    /// OUT 1+2 (0), MONITOR MIX (1), PHONE (2). Highlighted only until #15.
    Output(usize),
}

/// The four panels, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Panel {
    Input,
    Mixer,
    Loopback,
    Output,
}

impl Panel {
    pub const ALL: [Panel; 4] = [Panel::Input, Panel::Mixer, Panel::Loopback, Panel::Output];

    /// Where focus lands when the panel is entered.
    pub fn first(self) -> Control {
        match self {
            Panel::Input => Control::Input {
                ch: 0,
                kind: InputControl::ORDER[0],
            },
            Panel::Mixer => Control::Mixer(0),
            Panel::Loopback => Control::Loopback(0),
            Panel::Output => Control::Output(0),
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&p| p == self).unwrap_or(0)
    }
}

impl Control {
    /// The panel this control is in.
    pub fn panel(self) -> Panel {
        match self {
            Control::Input { .. } => Panel::Input,
            Control::Mixer(_) => Panel::Mixer,
            Control::Loopback(_) => Panel::Loopback,
            Control::Output(_) => Panel::Output,
        }
    }

    /// The strip to the left (`step` −1) or right (+1) in the same panel,
    /// clamped at the ends.
    fn beside(self, step: isize) -> Control {
        let mv = |i: usize, n: usize| (i as isize + step).clamp(0, n as isize - 1) as usize;
        match self {
            Control::Input { ch, kind } => Control::Input {
                ch: mv(ch, 2),
                kind,
            },
            Control::Mixer(i) => Control::Mixer(mv(i, 4)),
            Control::Loopback(i) => Control::Loopback(mv(i, 2)),
            Control::Output(i) => Control::Output(mv(i, 3)),
        }
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
    /// The sample-rate list ([`crate::rate::RATES`]), `sel` highlighted.
    Rate { sel: usize },
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
    /// When the device last sent anything (meters included).
    last_rx: Option<Instant>,
    /// When [`App::tick`] first saw the session synced.
    synced_at: Option<Instant>,
    /// Nothing from the device for [`RX_STALE`] while synced.
    rx_stale: bool,
    theme: Theme,
    notice: Option<String>,
    /// The interface's sample rate (Hz), when the OS tells.
    sample_rate: Option<u32>,
}

/// The PHONE strip's index among the output strips.
pub const PHONE: usize = 2;

/// With no report from the device for this long, the header says so (the
/// device streams meter reports several times a second).
pub const RX_STALE: Duration = Duration::from_secs(2);

impl App {
    /// A new session, connecting.
    pub fn new(theme: Theme) -> Self {
        App {
            state: State::default(),
            phase: Phase::Connecting,
            focus: Panel::Input.first(),
            modal: None,
            drag: None,
            last_click: None,
            mix: 0,
            clips: Clips::default(),
            last_rx: None,
            synced_at: None,
            rx_stale: false,
            theme,
            notice: None,
            sample_rate: None,
        }
    }

    /// The interface's sample rate (Hz), as last read from the OS.
    pub fn sample_rate(&self) -> Option<u32> {
        self.sample_rate
    }

    /// A new reading of the sample rate.
    pub fn set_sample_rate(&mut self, rate: Option<u32>) {
        self.sample_rate = rate;
    }

    /// The mix the mixer shows: A (0), B (1), C (2).
    pub fn mix(&self) -> usize {
        self.mix
    }

    pub fn clips(&self) -> &Clips {
        &self.clips
    }

    /// The meters as of `now`: updates the clip lights, and whether the
    /// device has gone quiet.
    pub fn tick(&mut self, meters: &Meters, now: Instant) {
        self.clips.update(meters, now);
        if self.phase == Phase::Synced {
            let since = *self.synced_at.get_or_insert(now);
            let last = self.last_rx.map_or(since, |l| l.max(since));
            self.rx_stale = now.saturating_duration_since(last) > RX_STALE;
        }
    }

    /// The device sent something (a report or a meter) at `now`.
    pub fn on_rx(&mut self, now: Instant) {
        self.last_rx = Some(self.last_rx.map_or(now, |l| l.max(now)));
    }

    /// Whether the device has sent nothing for [`RX_STALE`] (as of the last
    /// [`App::tick`]).
    pub fn rx_stale(&self) -> bool {
        self.rx_stale
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
            Some(Modal::Rate { sel }) => {
                match key.code {
                    KeyCode::Up => {
                        self.modal = Some(Modal::Rate {
                            sel: sel.saturating_sub(1),
                        })
                    }
                    KeyCode::Down => {
                        self.modal = Some(Modal::Rate {
                            sel: (sel + 1).min(RATES.len() - 1),
                        })
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => return self.pick_rate(sel),
                    KeyCode::Esc | KeyCode::Char('r') => self.modal = None,
                    KeyCode::Char('q') => return vec![Action::Quit],
                    _ => {}
                }
                return Vec::new();
            }
            Some(_) => return Vec::new(),
            None => {}
        }
        if key.code == KeyCode::Char('q') {
            return vec![Action::Quit];
        }
        if self.phase != Phase::Synced {
            return Vec::new();
        }
        let step = if key.modifiers.contains(KeyModifiers::SHIFT) {
            10
        } else {
            1
        };
        let gain_focused = matches!(
            self.focus,
            Control::Input {
                kind: InputControl::Gain,
                ..
            }
        );
        match key.code {
            KeyCode::Tab => self.cycle_panel(1),
            KeyCode::BackTab => self.cycle_panel(-1),
            KeyCode::Char(c @ '1'..='4') => {
                self.focus = Panel::ALL[usize::from(c as u8 - b'1')].first();
            }
            KeyCode::Up => self.move_vertical(-1),
            KeyCode::Down => self.move_vertical(1),
            KeyCode::Left | KeyCode::Right => {
                let delta = if key.code == KeyCode::Left {
                    -step
                } else {
                    step
                };
                if gain_focused {
                    return self.adjust(self.focus, delta);
                }
                self.focus = self.focus.beside(delta.signum() as isize);
            }
            KeyCode::Char('?') => {
                self.modal = Some(Modal::Help);
                self.drag = None;
            }
            KeyCode::Char('0') => return self.reset(self.focus),
            KeyCode::Char('r') => self.open_rates(),
            KeyCode::Char(c @ ('a' | 'b' | 'c')) => self.mix = usize::from(c as u8 - b'a'),
            KeyCode::Char(' ') | KeyCode::Enter => match self.focus {
                Control::Input { ch, kind } => return self.toggle(ch, kind),
                Control::Output(PHONE) => return self.set_phone_level(!self.state.phone_gain),
                _ => {}
            },
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
                (Modal::Help | Modal::Rate { .. }, Some(Hit::No)) => {
                    self.modal = None;
                    Vec::new()
                }
                (Modal::Rate { .. }, Some(Hit::RateOption(i))) => self.pick_rate(i),
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
                    Some(Hit::Focus(c)) => self.focus = c,
                    Some(Hit::Control(c)) => {
                        self.focus = c;
                        let Control::Input { ch, kind } = c else {
                            return Vec::new();
                        };
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
                    Some(Hit::Phone(high)) => {
                        self.focus = Control::Output(PHONE);
                        return self.set_phone_level(high);
                    }
                    Some(Hit::Rate) => self.open_rates(),
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
        let Control::Input {
            ch,
            kind: InputControl::Gain,
        } = c
        else {
            return Vec::new();
        };
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

    /// The PHONE level switch (`11 02`): +17 dBu (`true`) or 0 dBu.
    fn set_phone_level(&mut self, high: bool) -> Vec<Action> {
        let mut new = self.state.clone();
        new.phone_gain = high;
        self.commit(new)
    }

    /// Opens the sample-rate list at the current rate (48 kHz if unknown).
    fn open_rates(&mut self) {
        let sel = self
            .sample_rate
            .and_then(|r| RATES.iter().position(|&x| x == r))
            .unwrap_or(1);
        self.modal = Some(Modal::Rate { sel });
        self.drag = None;
    }

    /// Picks rate `i` from the list: the event loop sets it.
    fn pick_rate(&mut self, i: usize) -> Vec<Action> {
        self.modal = None;
        RATES
            .get(i)
            .map(|&r| vec![Action::SetRate(r)])
            .unwrap_or_default()
    }

    /// Tab / Shift-Tab: the next / previous panel, wrapping.
    fn cycle_panel(&mut self, step: isize) {
        let i = self.focus.panel().index() as isize;
        let n = Panel::ALL.len() as isize;
        self.focus = Panel::ALL[(i + step).rem_euclid(n) as usize].first();
    }

    /// Up/Down: along an input's column, clamped at the ends.
    fn move_vertical(&mut self, step: isize) {
        let Control::Input { ch, kind } = self.focus else {
            return;
        };
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
