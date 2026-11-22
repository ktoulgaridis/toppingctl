//! The TUI's state and input handling, with no I/O.
//!
//! [`App`] turns keys, mouse events and device reports into [`Action`]s for
//! the event loop to carry out. A change to the [`State`] becomes the frames
//! of [`State::diff`] and a [`Action::Save`]. 48V is never in a diff: turning
//! it on needs the confirm modal and comes out as
//! [`Action::SendConfirmedPhantom`].
//!
//! The input panel and the mixer (#14) take input; the mix tabs pick which
//! mix the mixer shows and its controls act on. Loopback and output are
//! drawn from the state; their controls come with #15.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton};
use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Span;

use crate::frame::Frame;
use crate::rate::RATES;
use crate::state::{Meters, Mix, PbStrip, State};
use crate::tui::theme::Theme;
use crate::tui::ui::{Hit, HitMap};
use crate::tui::widgets::fader::db_at_frac;
use crate::tui::widgets::meter::ClipHold;

/// The highest input gain, in dB.
pub const MAX_GAIN_DB: u8 = 20;
/// The input gain a reset (double-click, `0`) goes back to.
pub const DEFAULT_GAIN_DB: u8 = 0;
/// A mix fader's top, in dB (the app's crosspoint top, ≈ +6 dB).
pub const MIX_FADER_MAX_DB: f64 = 6.0;
/// A mix fader at or below this is at −∞.
pub const MIX_FADER_FLOOR_DB: f64 = -60.0;
/// One step of a pan dial: 5 % of its travel.
pub const PAN_STEP: f64 = 0.05;
/// The longest strip name, in characters.
pub const NAME_MAX: usize = 10;
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

/// A control on a mixer strip. `usize` is the channel: on IN 1+2, IN1 (0)
/// or IN2 (1); on a PB strip, the odd (0) or even (1) channel's pan dial,
/// and 0 for its single fader and switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MixControl {
    Pan(usize),
    Fader(usize),
    Link,
    Phase(usize),
    Solo(usize),
    Mute(usize),
}

impl MixControl {
    /// A PB strip's controls in focus order (↑↓): left dial, right dial,
    /// fader, LINK, Ø, SOLO, MUTE.
    pub const PB: [MixControl; 7] = [
        MixControl::Pan(0),
        MixControl::Pan(1),
        MixControl::Fader(0),
        MixControl::Link,
        MixControl::Phase(0),
        MixControl::Solo(0),
        MixControl::Mute(0),
    ];
    /// IN 1+2's controls in focus order: the same, with IN1's and IN2's
    /// fader and switches side by side.
    pub const IN12: [MixControl; 11] = [
        MixControl::Pan(0),
        MixControl::Pan(1),
        MixControl::Fader(0),
        MixControl::Fader(1),
        MixControl::Link,
        MixControl::Phase(0),
        MixControl::Phase(1),
        MixControl::Solo(0),
        MixControl::Solo(1),
        MixControl::Mute(0),
        MixControl::Mute(1),
    ];

    /// The controls of mixer strip `strip`, in focus order.
    pub fn order(strip: usize) -> &'static [MixControl] {
        if strip == 0 {
            &Self::IN12
        } else {
            &Self::PB
        }
    }

    /// The same control on mixer strip `strip`: a PB strip has one fader
    /// and one set of switches, so IN2's become its.
    fn on_strip(self, strip: usize) -> MixControl {
        if Self::order(strip).contains(&self) {
            return self;
        }
        match self {
            MixControl::Fader(_) => MixControl::Fader(0),
            MixControl::Phase(_) => MixControl::Phase(0),
            MixControl::Solo(_) => MixControl::Solo(0),
            MixControl::Mute(_) => MixControl::Mute(0),
            other => other,
        }
    }
}

/// Something that can have focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Control {
    /// A control on IN1 (`ch` 0) or IN2 (`ch` 1).
    Input { ch: usize, kind: InputControl },
    /// A mixer strip's name: IN 1+2 (0), PB 1/2 (1), PB 3/4 (2), PB 5/6
    /// (3). Enter renames it; ↓ goes into its controls.
    Mixer(usize),
    /// A control on mixer strip `strip` (numbered as in [`Control::Mixer`]),
    /// acting on the mix shown.
    Mix { strip: usize, ctl: MixControl },
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
            Control::Mixer(_) | Control::Mix { .. } => Panel::Mixer,
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
            Control::Mix { strip, ctl } => {
                let strip = mv(strip, 4);
                Control::Mix {
                    strip,
                    ctl: ctl.on_strip(strip),
                }
            }
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

/// A knob or fader being dragged.
#[derive(Clone, Copy, Debug)]
enum Drag {
    /// An input's gain knob: where the press was, and the gain then.
    Gain { ch: usize, y: u16, gain: u8 },
    /// A mix fader: its cap follows the pointer along `track`.
    Fader {
        strip: usize,
        ch: usize,
        track: Rect,
    },
    /// A pan dial: one step a row from where the press was.
    Pan {
        strip: usize,
        ch: usize,
        y: u16,
        pan: f64,
    },
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
    /// A mixer strip's name being edited: the strip and the text so far.
    editing: Option<(usize, String)>,
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
            editing: None,
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

    /// The mixer strip whose name is being edited, and the text so far.
    pub fn editing(&self) -> Option<(usize, &str)> {
        self.editing.as_ref().map(|(i, s)| (*i, s.as_str()))
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
        if self.modal.is_none() && self.editing.is_some() {
            return self.edit_key(key.code);
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
        let adjustable = matches!(
            self.focus,
            Control::Input {
                kind: InputControl::Gain,
                ..
            } | Control::Mix {
                ctl: MixControl::Fader(_) | MixControl::Pan(_),
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
                if adjustable {
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
            KeyCode::Enter if matches!(self.focus, Control::Mixer(_)) => {
                if let Control::Mixer(strip) = self.focus {
                    let name = self.state.strip_names[strip].clone().unwrap_or_default();
                    self.editing = Some((strip, name));
                    self.drag = None;
                }
            }
            KeyCode::Char(' ') | KeyCode::Enter => match self.focus {
                Control::Input { ch, kind } => return self.toggle(ch, kind),
                Control::Output(PHONE) => return self.set_phone_level(!self.state.phone_gain),
                Control::Mix { strip, ctl } => return self.toggle_mix(strip, ctl),
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
        if matches!(ev.kind, MouseEventKind::Down(_)) {
            // A click elsewhere ends a name edit, without keeping it.
            self.editing = None;
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
                    Some(h @ Hit::Control(c)) => {
                        self.focus = c;
                        let adjustable = match c {
                            Control::Input { ch, kind } if kind != InputControl::Gain => {
                                return self.toggle(ch, kind)
                            }
                            Control::Mix { strip, ctl } => match ctl {
                                MixControl::Fader(_) | MixControl::Pan(_) => true,
                                _ => return self.toggle_mix(strip, ctl),
                            },
                            Control::Input { .. } => true,
                            _ => false,
                        };
                        if !adjustable {
                            return Vec::new();
                        }
                        if double {
                            self.drag = None;
                            return self.reset(c);
                        }
                        self.drag = self.start_drag(c, ev.row, hits.rect(h));
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
                    return self.drag_to(d, ev.row);
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

    /// Turns a knob or moves a fader by `delta` steps: 1 dB of gain or
    /// fader, [`PAN_STEP`] of pan.
    fn adjust(&mut self, c: Control, delta: i32) -> Vec<Action> {
        match c {
            Control::Input {
                ch,
                kind: InputControl::Gain,
            } => {
                let gain = i32::from(self.state.inputs[ch].gain_db.min(MAX_GAIN_DB));
                let gain = (gain + delta).clamp(0, i32::from(MAX_GAIN_DB)) as u8;
                self.set_gain(ch, gain)
            }
            Control::Mix {
                strip,
                ctl: MixControl::Fader(ch),
            } => {
                let db = step_fader(self.fader_db(strip, ch), f64::from(delta));
                self.set_fader(strip, ch, db)
            }
            Control::Mix {
                strip,
                ctl: MixControl::Pan(ch),
            } => {
                let pan = self.pan(strip, ch) + PAN_STEP * f64::from(delta);
                self.set_pan(strip, ch, pan)
            }
            _ => Vec::new(),
        }
    }

    /// Puts a knob or fader back to its default: gain −0 dB, fader 0 dB,
    /// a pan dial where the app starts it (odd channel and IN1 hard L, even
    /// channel and IN2 hard R).
    fn reset(&mut self, c: Control) -> Vec<Action> {
        match c {
            Control::Input {
                ch,
                kind: InputControl::Gain,
            } => self.set_gain(ch, DEFAULT_GAIN_DB),
            Control::Mix {
                strip,
                ctl: MixControl::Fader(ch),
            } => self.set_fader(strip, ch, 0.0),
            Control::Mix {
                strip,
                ctl: MixControl::Pan(ch),
            } => self.set_pan(strip, ch, ch.min(1) as f64),
            _ => Vec::new(),
        }
    }

    /// The drag a press on `c` (at `row`, on `rect`) starts.
    fn start_drag(&self, c: Control, row: u16, rect: Option<Rect>) -> Option<Drag> {
        match c {
            Control::Input { ch, .. } => Some(Drag::Gain {
                ch,
                y: row,
                gain: self.state.inputs[ch].gain_db.min(MAX_GAIN_DB),
            }),
            Control::Mix {
                strip,
                ctl: MixControl::Fader(ch),
            } => rect.map(|track| Drag::Fader { strip, ch, track }),
            Control::Mix {
                strip,
                ctl: MixControl::Pan(ch),
            } => Some(Drag::Pan {
                strip,
                ch,
                y: row,
                pan: self.pan(strip, ch),
            }),
            _ => None,
        }
    }

    /// The pointer of drag `d` is at `row`.
    fn drag_to(&mut self, d: Drag, row: u16) -> Vec<Action> {
        match d {
            Drag::Gain { ch, y, gain } => {
                let rows = i32::from(y) - i32::from(row);
                let gain = (i32::from(gain) + rows).clamp(0, i32::from(MAX_GAIN_DB));
                self.set_gain(ch, gain as u8)
            }
            Drag::Fader { strip, ch, track } => {
                let db = db_at_row(track, row);
                self.set_fader(strip, ch, db)
            }
            Drag::Pan { strip, ch, y, pan } => {
                let rows = f64::from(i32::from(y) - i32::from(row));
                self.set_pan(strip, ch, pan + PAN_STEP * rows)
            }
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

    fn fader_db(&self, strip: usize, ch: usize) -> f64 {
        let m = &self.state.mixes[self.mix];
        match strip {
            0 if ch == 0 => m.in1.fader_db,
            0 => m.in2.fader_db,
            _ => pb(m, strip).fader_db,
        }
    }

    fn pan(&self, strip: usize, ch: usize) -> f64 {
        let m = &self.state.mixes[self.mix];
        match (strip, ch) {
            (0, 0) => m.in1.pan,
            (0, _) => m.in2.pan,
            (_, 0) => pb(m, strip).pan_l,
            _ => pb(m, strip).pan_r,
        }
    }

    /// Sets a mix fader, clamped at [`MIX_FADER_MAX_DB`].
    fn set_fader(&mut self, strip: usize, ch: usize, db: f64) -> Vec<Action> {
        let db = db.min(MIX_FADER_MAX_DB);
        let mut new = self.state.clone();
        let m = &mut new.mixes[self.mix];
        match strip {
            0 if ch == 0 => m.in1.fader_db = db,
            0 => m.in2.fader_db = db,
            _ => pb_mut(m, strip).fader_db = db,
        }
        self.commit(new)
    }

    /// Sets a pan dial, on the app's 1 % grid (L100 … R100). A linked PB
    /// strip's other dial mirrors it. While IN 1+2 is linked its dials
    /// stay put: the link sets IN1 hard L and IN2 hard R.
    fn set_pan(&mut self, strip: usize, ch: usize, pan: f64) -> Vec<Action> {
        let pan = ((pan * 200.0).round() / 200.0).clamp(0.0, 1.0);
        let mut new = self.state.clone();
        let m = &mut new.mixes[self.mix];
        if strip == 0 {
            if m.link {
                return Vec::new();
            }
            if ch == 0 {
                m.in1.pan = pan;
            } else {
                m.in2.pan = pan;
            }
        } else {
            let s = pb_mut(m, strip);
            let (mine, other) = if ch == 0 {
                (&mut s.pan_l, &mut s.pan_r)
            } else {
                (&mut s.pan_r, &mut s.pan_l)
            };
            *mine = pan;
            if s.link {
                *other = 1.0 - pan;
            }
        }
        self.commit(new)
    }

    /// Flips a mixer switch: LINK, Ø, SOLO or MUTE.
    fn toggle_mix(&mut self, strip: usize, ctl: MixControl) -> Vec<Action> {
        let mut new = self.state.clone();
        let m = &mut new.mixes[self.mix];
        if ctl == MixControl::Link {
            if strip == 0 {
                m.link = !m.link;
            } else {
                let s = pb_mut(m, strip);
                s.link = !s.link;
            }
            return self.commit(new);
        }
        let (mute, solo, phase) = match strip {
            0 => {
                let s = if channel(ctl) == 0 {
                    &mut m.in1
                } else {
                    &mut m.in2
                };
                (&mut s.mute, &mut s.solo, &mut s.phase)
            }
            _ => {
                let s = pb_mut(m, strip);
                (&mut s.mute, &mut s.solo, &mut s.phase)
            }
        };
        match ctl {
            MixControl::Mute(_) => *mute = !*mute,
            MixControl::Solo(_) => *solo = !*solo,
            MixControl::Phase(_) => *phase = !*phase,
            _ => return Vec::new(),
        }
        self.commit(new)
    }

    /// A key while a strip name is being edited: text, Backspace, Enter
    /// keeps it (empty: back to the default name), Esc drops it.
    fn edit_key(&mut self, code: KeyCode) -> Vec<Action> {
        let Some((strip, text)) = self.editing.as_mut() else {
            return Vec::new();
        };
        match code {
            // One cell a character: the strip title is laid out by cells.
            KeyCode::Char(c) if !c.is_control() && Span::raw(c.to_string()).width() == 1 => {
                if text.chars().count() < NAME_MAX {
                    text.push(c);
                }
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Esc => self.editing = None,
            KeyCode::Enter => {
                let strip = *strip;
                let name = text.trim().to_string();
                self.editing = None;
                let mut new = self.state.clone();
                new.strip_names[strip] = (!name.is_empty()).then_some(name);
                return self.commit(new);
            }
            _ => {}
        }
        Vec::new()
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

    /// Up/Down: along an input's column, or from a mixer strip's name
    /// down through its controls; clamped at the ends.
    fn move_vertical(&mut self, step: isize) {
        self.focus = match self.focus {
            Control::Input { ch, kind } => {
                let n = InputControl::ORDER.len() as isize;
                let i = (kind.index() as isize + step).clamp(0, n - 1) as usize;
                Control::Input {
                    ch,
                    kind: InputControl::ORDER[i],
                }
            }
            Control::Mixer(strip) if step > 0 => Control::Mix {
                strip,
                ctl: MixControl::order(strip)[0],
            },
            Control::Mix { strip, ctl } => {
                let order = MixControl::order(strip);
                let i = order.iter().position(|&c| c == ctl).unwrap_or(0) as isize + step;
                if i < 0 {
                    Control::Mixer(strip)
                } else {
                    Control::Mix {
                        strip,
                        ctl: order[(i as usize).min(order.len() - 1)],
                    }
                }
            }
            other => other,
        };
    }
}

/// PB strip `strip` (1 ..= 3) of `m`.
fn pb(m: &Mix, strip: usize) -> &PbStrip {
    match strip {
        1 => &m.pb12,
        2 => &m.pb34,
        _ => &m.pb56,
    }
}

fn pb_mut(m: &mut Mix, strip: usize) -> &mut PbStrip {
    match strip {
        1 => &mut m.pb12,
        2 => &mut m.pb34,
        _ => &mut m.pb56,
    }
}

/// The channel a mixer switch acts on.
fn channel(ctl: MixControl) -> usize {
    match ctl {
        MixControl::Pan(c)
        | MixControl::Fader(c)
        | MixControl::Phase(c)
        | MixControl::Solo(c)
        | MixControl::Mute(c) => c,
        MixControl::Link => 0,
    }
}

/// A mix fader `delta` dB from `db`: −∞ steps up from −60 dB, and at or
/// below −60 dB it snaps to −∞.
pub fn step_fader(db: f64, delta: f64) -> f64 {
    let base = if db.is_nan() || db <= MIX_FADER_FLOOR_DB {
        MIX_FADER_FLOOR_DB
    } else {
        db
    };
    let new = base + delta;
    if new <= MIX_FADER_FLOOR_DB {
        f64::NEG_INFINITY
    } else {
        new.min(MIX_FADER_MAX_DB)
    }
}

/// The mix fader value at `row` on a fader drawn over `track`: the top row
/// is [`MIX_FADER_MAX_DB`], the bottom −∞, rows outside clamped; rounded
/// to 0.1 dB.
pub fn db_at_row(track: Rect, row: u16) -> f64 {
    let h = track.height.max(1);
    let row = row.clamp(track.y, track.y + h - 1) - track.y;
    let frac = if h == 1 {
        1.0
    } else {
        1.0 - f64::from(row) / f64::from(h - 1)
    };
    let db = db_at_frac(frac, MIX_FADER_MAX_DB);
    if db.is_finite() {
        (db * 10.0).round() / 10.0
    } else {
        db
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
