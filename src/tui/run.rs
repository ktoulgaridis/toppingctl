//! The terminal and the event loop: the only part of the TUI with I/O.
//!
//! The loop merges terminal input, the device's events and a ~30 fps tick.
//! The terminal is restored on every way out: normal exit, error, and panic
//! (a panic hook), with mouse capture turned off.

use std::io::{self, Write};
use std::panic;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event as Input};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::conn::{Connection, Event, HidTransport, SendError};
use crate::error::{message, Platform};
use crate::open::open_e2x2;
use crate::state::Meters;
use crate::sync::{connect, Store, SyncError};
use crate::tui::app::{Action, App};
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

/// Runs the TUI until the user quits. Errors (no device, no permission, a
/// disconnect) come back after the terminal is restored, for the caller to
/// print.
pub fn run() -> Result<()> {
    let store = Store::from_env()?;
    let mut app = App::new(Theme::from_env());
    let _guard = Guard::enter()?;
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

    let mut hits = HitMap::default();
    loop {
        app.tick(&meters, Instant::now());
        terminal.draw(|f| hits = ui::draw(f, &app, &meters))?;
        let mut actions = Vec::new();
        if event::poll(TICK)? {
            match event::read()? {
                Input::Key(key) => actions = app.handle_key(key),
                Input::Mouse(ev) => actions = app.handle_mouse(ev, &hits),
                _ => {}
            }
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
                Action::Quit => return Ok(()),
            }
        }
    }
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
            Action::Save | Action::Quit => {}
        }
    }
    Ok(())
}

/// Applies everything the device sent since the last call.
fn drain_device(
    events: &Receiver<Event>,
    app: &mut App,
    meters: &mut Meters,
) -> Result<Vec<Action>> {
    let mut actions = Vec::new();
    loop {
        match events.try_recv() {
            Ok(Event::Meter(f)) => {
                meters.apply(&f);
            }
            Ok(Event::Report(f)) => actions.extend(app.on_report(&f)),
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
