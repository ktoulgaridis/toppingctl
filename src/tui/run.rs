//! The terminal and the event loop: the only part of the TUI with I/O.
//!
//! The loop merges terminal input, the device's events and a ~30 fps tick
//! (~60 fps while a meter moves).
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
use crate::frame::Frame;
use crate::official_app::{self, System};
use crate::open::open_e2x2;
use crate::rate;
use crate::state::Meters;
use crate::sync::{connect, Store, SyncError};
use crate::tpwork;
use crate::tui::app::{Action, App};
use crate::tui::ballistics::ANIMATE_TICK;
use crate::tui::kitty;
use crate::tui::pixel::PixelLayer;
use crate::tui::theme::Theme;
use crate::tui::ui::{self, HitMap};
use crate::workspace::{Settings, Workspaces};

/// How long the loop waits for input before it redraws (~30 fps), with
/// the meters still.
pub const TICK: Duration = Duration::from_millis(33);

/// How long the loop waits for input before the next frame at `now`:
/// [`ANIMATE_TICK`] (~60 fps) while a meter is moving (a bar or a released
/// peak marker falling, #71), else [`TICK`].
pub fn frame_wait(app: &App, now: Instant) -> Duration {
    if app.meters_moving(now) {
        ANIMATE_TICK
    } else {
        TICK
    }
}

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
    let workspaces = Workspaces::from_env()?;
    let mut app = App::new(Theme::from_env());
    app.set_workspaces(workspaces.list(), workspaces.settings());
    app.set_workspaces_folder(workspaces.folder_shown());
    app.set_official(
        tpwork::folder_from_env()
            .map(|f| tpwork::list(&f))
            .unwrap_or_default(),
    );
    app.set_notice(official_app::warning(&System).map(String::from));
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
        let now = Instant::now();
        app.tick(&meters, now);
        let hits = draw_frame(
            &mut terminal,
            &app,
            &meters,
            pixels.as_mut(),
            &mut io::stdout(),
        )?;
        let mut actions = Vec::new();
        if event::poll(frame_wait(&app, now))? {
            // Everything already queued, without waiting again.
            let mut resized = false;
            let mut queued = std::iter::from_fn(|| match event::poll(Duration::ZERO) {
                Ok(true) => event::read().ok(),
                _ => None,
            })
            .inspect(|ev| resized |= matches!(ev, Input::Resize(..)));
            actions = process_input(&mut app, &mut queued, &hits);
            drop(queued);
            if let (true, Some(layer)) = (resized, pixels.as_mut()) {
                // Free the images before the next text frame, and take the
                // new cell size from the terminal (TIOCGWINSZ).
                layer.begin_resize(Instant::now(), kitty::window_cell_size());
                let mut out = io::stdout();
                out.write_all(layer.take_output().as_bytes())?;
                out.flush()?;
            }
        }
        actions.extend(drain_device(&events, &mut app, &mut meters)?);
        for action in carry_out(&conn, &mut app, &store, &workspaces, actions)? {
            match action {
                Action::SetRate(rate) => {
                    // The OS call (pw-metadata, CoreAudio) off the loop; a failure
                    // comes back as a notice.
                    let tx = rate_errors_tx.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = rate::set(rate) {
                            let _ = tx.send(e);
                        }
                    });
                }
                Action::Quit => return Ok(()),
                _ => {}
            }
        }
    }
}

/// ParameterSave: store the current state in the device's memory.
pub const STORE_FRAME: Frame = Frame {
    group: 0x1a,
    param: 0x03,
    value: 0,
};

/// Carries out `actions` in order: frames to the device (as
/// [`send_actions`]), the saved state, the workspace files and their
/// settings, and ParameterSave `1a 03` ([`Action::StoreToDevice`], skipped
/// when a save before it failed). A file error becomes the app's notice; a
/// workspace that can't be read changes nothing. Returns what is left for
/// the event loop ([`Action::SetRate`], [`Action::Quit`]).
pub fn carry_out(
    conn: &Connection,
    app: &mut App,
    store: &Store,
    workspaces: &Workspaces,
    actions: Vec<Action>,
) -> Result<Vec<Action>, SendError> {
    let mut left = Vec::new();
    let mut save_failed = false;
    for action in actions {
        match action {
            Action::Send(f) => conn.send(f)?,
            Action::SendConfirmedPhantom(f) => conn.send_confirmed_phantom(f)?,
            Action::Save => {
                let mut notice = store
                    .save(app.state())
                    .err()
                    .map(|e| format!("could not save the state: {e}"));
                if let (true, Some(name)) = (app.auto_save(), app.workspace()) {
                    if let Err(e) = workspaces.save(name, app.state()) {
                        notice = Some(format!("could not auto-save workspace {name:?}: {e}"));
                    }
                }
                save_failed |= notice.is_some();
                app.set_notice(notice);
            }
            Action::SaveWorkspace(name) => {
                match workspaces.save(&name, app.state()) {
                    Ok(()) => app.set_notice(Some(format!("saved workspace {name:?}"))),
                    Err(e) => {
                        save_failed = true;
                        // Back to what is on disk.
                        app.set_workspaces(workspaces.list(), workspaces.settings());
                        app.set_notice(Some(format!("could not save workspace {name:?}: {e}")));
                        continue;
                    }
                }
                sync_workspaces(app, workspaces);
            }
            Action::LoadWorkspace(name) => match workspaces.load(&name) {
                Ok(state) => {
                    let notes = app.take_import_notes();
                    app.set_notice(None);
                    let more = app.load_workspace(name.clone(), state);
                    left.extend(carry_out(conn, app, store, workspaces, more)?);
                    sync_workspaces(app, workspaces);
                    if app.notice().is_none() {
                        let mut n = format!("loaded workspace {name:?}");
                        for note in &notes {
                            n.push_str("; ");
                            n.push_str(note);
                        }
                        app.set_notice(Some(n));
                    }
                }
                Err(e) => app.set_notice(Some(format!(
                    "could not load workspace {name:?} ({e}); nothing changed"
                ))),
            },
            Action::DeleteWorkspace(name) => {
                if let Err(e) = workspaces.delete(&name) {
                    app.set_notice(Some(format!("could not remove workspace {name:?}: {e}")));
                }
                sync_workspaces(app, workspaces);
            }
            // A saved import asks whether to load it ([`App::offer_load`]).
            Action::ImportWorkspace(path) => {
                let mut offer = None;
                let notice = match tpwork::import(workspaces, &path, None, false) {
                    Ok(saved) => {
                        let mut n = format!(
                            "imported {:?} from the TOPPING app; pick it with l to load it",
                            saved.name
                        );
                        for note in &saved.notices {
                            n.push_str("; ");
                            n.push_str(note);
                        }
                        offer = Some(saved);
                        n
                    }
                    Err(e @ tpwork::ImportError::Exists(_)) => format!(
                        "{e}; not imported (remove or rename it, or use toppingctl import --force)"
                    ),
                    Err(e) => format!("could not import: {e}"),
                };
                sync_workspaces(app, workspaces);
                app.set_notice(Some(notice));
                if let Some(saved) = offer {
                    app.offer_load(saved.name, saved.notices);
                }
            }
            Action::SaveWorkspaceSettings => sync_workspaces(app, workspaces),
            Action::SetWorkspaceFolder(typed) => set_folder(app, workspaces, &typed),
            Action::StoreToDevice if save_failed => {}
            Action::StoreToDevice => conn.send(STORE_FRAME)?,
            Action::SetRate(_) | Action::Quit => left.push(action),
        }
    }
    Ok(left)
}

/// Makes `typed` the workspaces folder (see `Workspaces::prepare_folder`)
/// and reads the list from it; nothing is moved. The open workspace, if
/// it isn't there, falls back to Default. A folder that can't be used
/// changes nothing and becomes the notice.
fn set_folder(app: &mut App, workspaces: &Workspaces, typed: &str) {
    let folder = match workspaces.prepare_folder(typed) {
        Ok(folder) => folder,
        Err(e) => {
            app.set_notice(Some(format!("could not use that workspaces folder: {e}")));
            return;
        }
    };
    let settings = Settings {
        folder,
        ..app.settings()
    };
    if let Err(e) = workspaces.save_settings(&settings) {
        app.set_notice(Some(format!("could not save the workspace settings: {e}")));
        return;
    }
    app.set_notice(None);
    app.set_workspaces(workspaces.list(), settings);
    sync_workspaces(app, workspaces);
    app.set_workspaces_folder(workspaces.folder_shown());
    if app.notice().is_none() {
        let shown = workspaces.folder_shown();
        app.set_notice(Some(format!("workspaces folder: {shown}")));
    }
}

/// Takes the workspace list from disk and saves the app's settings (the
/// open workspace, auto-save, the folder); a failure becomes the notice.
fn sync_workspaces(app: &mut App, workspaces: &Workspaces) {
    app.set_workspaces(workspaces.list(), app.settings());
    if let Err(e) = workspaces.save_settings(&app.settings()) {
        app.set_notice(Some(format!("could not save the workspace settings: {e}")));
    }
}

/// Takes terminal events from `events` until one that matters (a key, or
/// a mouse press, release, drag or wheel) has been handled, and returns
/// its actions. Pointer motion is dropped: crossterm's mouse capture
/// reports every move, and handling them one per frame used to queue clicks
/// behind seconds of motion. Stopping after one event keeps the hit map in
/// step with what was drawn: the next one is handled after a redraw. The
/// arrows follow the layout drawn ([`HitMap::switches_stacked`]).
pub fn process_input(
    app: &mut App,
    events: &mut dyn Iterator<Item = Input>,
    hits: &HitMap,
) -> Vec<Action> {
    app.set_switches_stacked(hits.switches_stacked());
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
            _ => {}
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
