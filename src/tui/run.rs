//! The terminal and the event loop: the only part of the TUI with I/O.
//!
//! The loop merges terminal input, the device's events and a ~30 fps tick.
//! The terminal is restored on every way out: normal exit, error, and panic
//! (a panic hook), with mouse capture turned off.

use std::io::{self, Write};
use std::panic;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as Input, MouseEventKind,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;

use crate::conn::{Connection, Event, HidTransport, SendError};
use crate::error::{message, Platform};
use crate::open::open_e2x2;
use crate::rate;
use crate::state::Meters;
use crate::sync::{connect, Store, SyncError};
use crate::tui::app::{Action, App};
use crate::tui::pixel::PixelLayer;
use crate::tui::theme::Theme;
use crate::tui::ui::{self, HitMap};

/// How long the loop waits for input before it redraws (~30 fps).
const TICK: Duration = Duration::from_millis(33);

/// Writes the sequence that takes the terminal into the TUI: the alternate
/// screen, mouse capture on, cursor hidden.
pub fn write_enter(w: &mut impl Write) -> io::Result<()> {
    crossterm::execute!(w, EnterAlternateScreen, EnableMouseCapture, Hide)
}

/// Writes the sequence that gives the terminal back: mouse capture off, the
/// main screen, cursor shown.
pub fn write_leave(w: &mut impl Write) -> io::Result<()> {
    crossterm::execute!(w, DisableMouseCapture, LeaveAlternateScreen, Show)
}

/// Gives the terminal back. Safe to call more than once.
fn restore() {
    let _ = terminal::disable_raw_mode();
    let _ = write_leave(&mut io::stdout());
}

/// Restores the terminal when dropped, so every return path and unwind
/// leaves a usable terminal.
struct Guard;

impl Guard {
    fn enter() -> Result<Self> {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
        terminal::enable_raw_mode().context("could not set up the terminal")?;
        let guard = Guard;
        write_enter(&mut io::stdout()).context("could not set up the terminal")?;
        Ok(guard)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

/// Writes the sequence that frees every kitty image the TUI sent.
pub fn free_images(w: &mut impl Write) -> io::Result<()> {
    w.write_all(PixelLayer::FREE_ALL.as_bytes())?;
    w.flush()
}

/// Frees every kitty image the TUI sent, when dropped (on every way out,
/// before the terminal leaves the alternate screen).
struct FreeImages(bool);

impl Drop for FreeImages {
    fn drop(&mut self) {
        if self.0 {
            let _ = free_images(&mut io::stdout());
        }
    }
}

/// Draws one frame: the text grid through `terminal`, then (with a pixel
/// layer) the frame's kitty commands to `images` in one write, flushed.
/// The images never go through the text grid, and a frame sends at most
/// [`TRANSMIT_BUDGET`](crate::tui::pixel::TRANSMIT_BUDGET) of image data,
/// so drawing never holds up the loop that drains the device.
pub fn draw_frame<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &App,
    meters: &Meters,
    mut pixels: Option<&mut PixelLayer>,
    images: &mut impl Write,
) -> Result<HitMap>
where
    B::Error: Send + Sync + 'static,
{
    let mut hits = HitMap::default();
    terminal.draw(|f| hits = ui::draw_with(f, app, meters, pixels.as_deref_mut()))?;
    if let Some(layer) = pixels {
        let out = layer.take_output();
        if !out.is_empty() {
            images.write_all(out.as_bytes())?;
            images.flush()?;
        }
    }
    Ok(hits)
}

/// Runs the TUI until the user quits. Errors (no device, no permission, a
/// disconnect) come back after the terminal is restored, for the caller to
/// print.
pub fn run() -> Result<()> {
    let store = Store::from_env()?;
    let mut app = App::new(Theme::from_env());
    let _guard = Guard::enter()?;
    // Before any event is read: does the terminal take kitty images?
    let mut pixels = PixelLayer::detect();
    let _free = FreeImages(pixels.is_some());
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut meters = Meters::default();
    terminal.draw(|f| {
        ui::draw(f, &app, &meters);
    })?;

    let device = open_e2x2().map_err(|e| anyhow!(message(&e, Platform::current())))?;
    let (conn, events) = Connection::spawn(HidTransport::new(device));
    match connect(&conn, &events, &store) {
        Ok(state) => app.synced(state),
        Err(SyncError::NeedsInitialState(state)) => app.needs_initial_state(*state),
        Err(e) => return Err(e.into()),
    }

    // The sample rate is read off the loop, every 2 s.
    let rates = rate::poll();
    let (rate_errors_tx, rate_errors) = mpsc::channel::<String>();
    loop {
        while let Ok(r) = rates.try_recv() {
            app.set_sample_rate(r);
        }
        while let Ok(e) = rate_errors.try_recv() {
            app.set_notice(Some(e));
        }
        mark_alive(&conn, &mut app);
        app.tick(&meters, Instant::now());
        let hits = draw_frame(
            &mut terminal,
            &app,
            &meters,
            pixels.as_mut(),
            &mut io::stdout(),
        )?;
        let mut actions = Vec::new();
        if event::poll(TICK)? {
            // Everything already queued, without waiting again.
            let mut queued = std::iter::from_fn(|| match event::poll(Duration::ZERO) {
                Ok(true) => event::read().ok(),
                _ => None,
            });
            actions = process_input(&mut app, &mut queued, &hits);
        }
        actions.extend(drain_device(&events, &mut app, &mut meters)?);
        send_actions(&conn, &actions)?;
        for action in actions {
            match action {
                Action::Send(_) | Action::SendConfirmedPhantom(_) => {}
                Action::Save => {
                    let notice = store
                        .save(app.state())
                        .err()
                        .map(|e| format!("could not save the state: {e}"));
                    app.set_notice(notice);
                }
                Action::SetRate(rate) => {
                    // pw-metadata off the loop; a failure comes back as a
                    // notice.
                    let tx = rate_errors_tx.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = rate::set(rate) {
                            let _ = tx.send(e);
                        }
                    });
                }
                Action::Quit => return Ok(()),
            }
        }
    }
}

/// Takes terminal events from `events` until one that matters (a key, or
/// a mouse press, release, drag or wheel) has been handled, and returns
/// its actions. Pointer motion is dropped: crossterm's mouse capture
/// reports every move, and handling them one per frame used to queue clicks
/// behind seconds of motion. Stopping after one event keeps the hit map in
/// step with what was drawn: the next one is handled after a redraw.
pub fn process_input(
    app: &mut App,
    events: &mut dyn Iterator<Item = Input>,
    hits: &HitMap,
) -> Vec<Action> {
    for ev in events {
        match ev {
            Input::Key(key) => return app.handle_key(key),
            Input::Mouse(m) if m.kind == MouseEventKind::Moved => {}
            Input::Mouse(m) => return app.handle_mouse(m, hits),
            _ => {}
        }
    }
    Vec::new()
}

/// Sends the frames in `actions` to the device, in order: [`Action::Send`]
/// with `Connection::send` (which refuses to turn 48V on) and
/// [`Action::SendConfirmedPhantom`] with `send_confirmed_phantom`. Other
/// actions are left to the caller.
pub fn send_actions(conn: &Connection, actions: &[Action]) -> Result<(), SendError> {
    for action in actions {
        match *action {
            Action::Send(f) => conn.send(f)?,
            Action::SendConfirmedPhantom(f) => conn.send_confirmed_phantom(f)?,
            Action::Save | Action::SetRate(_) | Action::Quit => {}
        }
    }
    Ok(())
}

/// Counts anything the device sent, its idle frames included, as a sign of
/// life (see [`Connection::last_seen`]), so a silent device with nothing to
/// meter still reads as synced.
pub fn mark_alive(conn: &Connection, app: &mut App) {
    if let Some(t) = conn.last_seen() {
        app.on_rx(t);
    }
}

/// Applies everything the device sent since the last call.
pub fn drain_device(
    events: &Receiver<Event>,
    app: &mut App,
    meters: &mut Meters,
) -> Result<Vec<Action>> {
    let mut actions = Vec::new();
    loop {
        match events.try_recv() {
            Ok(Event::Meter(f)) => {
                app.on_rx(Instant::now());
                meters.apply(&f);
            }
            Ok(Event::Report(f)) => {
                app.on_rx(Instant::now());
                actions.extend(app.on_report(&f));
            }
            Ok(Event::Malformed { .. }) => {}
            Ok(Event::Disconnected(e)) => {
                return Err(anyhow!("the TOPPING E2x2 disconnected: {e}"))
            }
            Err(TryRecvError::Empty) => return Ok(actions),
            Err(TryRecvError::Disconnected) => {
                return Err(anyhow!("the TOPPING E2x2 disconnected"))
            }
        }
    }
}
