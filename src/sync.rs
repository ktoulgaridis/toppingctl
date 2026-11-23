//! Connecting to the device and keeping the last state toppingctl set.
//!
//! The device has no full-state read (PROTOCOL.md, "Open questions"), so
//! toppingctl does what the official app does: it saves the state it last set
//! and pushes it on connect. On first run there is nothing to push, and
//! [`connect`] returns [`SyncError::NeedsInitialState`] without writing
//! anything after the query, so the caller can ask the user first. 48V is
//! never pushed and never saved.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::conn::{Connection, Event, SendError};
use crate::frame::Frame;
use crate::state::State;

/// How long [`connect`] waits for the reply to the `10 06` query.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(1);

/// The registers the device sends in reply to the `10 06` query.
const QUERY_REPLY: [(u8, u8); 6] = [
    (0x10, 0x03),
    (0x10, 0x04),
    (0x10, 0x0a),
    (0x10, 0x0b),
    (0x10, 0x0c),
    (0x11, 0x01),
];

/// Why [`connect`] did not return a state.
#[derive(Debug)]
pub enum SyncError {
    /// No saved state (first run). Nothing was written after the query.
    /// Holds the defaults with the device's reply applied, for the caller
    /// to confirm and push.
    NeedsInitialState(Box<State>),
    /// The saved state could not be read.
    Store(StoreError),
    /// A frame could not be queued.
    Send(SendError),
    /// The device went away while connecting.
    Disconnected(io::Error),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedsInitialState(_) => write!(f, "no saved state to push to the device"),
            Self::Store(e) => write!(f, "{e}"),
            Self::Send(e) => write!(f, "{e}"),
            Self::Disconnected(e) => write!(f, "the TOPPING E2x2 disconnected: {e}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            Self::Send(e) => Some(e),
            Self::Disconnected(e) => Some(e),
            Self::NeedsInitialState(_) => None,
        }
    }
}

impl From<StoreError> for SyncError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

impl From<SendError> for SyncError {
    fn from(e: SendError) -> Self {
        Self::Send(e)
    }
}

/// Brings the device to the saved state.
///
/// Sends the `10 06` query and applies its reply (waiting at most
/// [`REPLY_TIMEOUT`]; a missing reply leaves those fields as they are), then
/// loads the saved state, keeps the device's settings and readout, and pushes
/// everything else. 48V is not pushed and is unknown (`None`) afterwards.
/// Other events that arrive while waiting are consumed.
pub fn connect(
    conn: &Connection,
    events: &Receiver<Event>,
    store: &Store,
) -> Result<State, SyncError> {
    conn.send(Frame {
        group: 0x10,
        param: 0x06,
        value: 1,
    })?;
    let device = read_query_reply(events)?;

    let Some(mut state) = store.load()? else {
        return Err(SyncError::NeedsInitialState(Box::new(device)));
    };
    state.system = device.system;
    state.readout = device.readout;
    for input in &mut state.inputs {
        input.phantom = None;
    }
    for f in state.frames().into_iter().filter(|f| f.group != 0x10) {
        conn.send(f)?;
    }
    Ok(state)
}

/// Defaults with the reply to the `10 06` query applied.
fn read_query_reply(events: &Receiver<Event>) -> Result<State, SyncError> {
    let mut device = State::default();
    let mut missing = QUERY_REPLY.to_vec();
    let deadline = Instant::now() + REPLY_TIMEOUT;
    while !missing.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(Event::Report(f)) => {
                missing.retain(|&reg| reg != (f.group, f.param));
                device.apply(&f);
            }
            Ok(Event::Disconnected(e)) => return Err(SyncError::Disconnected(e)),
            Ok(Event::Meter(_) | Event::Malformed { .. }) => {}
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(SyncError::Disconnected(io::Error::other(
                    "event channel closed",
                )))
            }
        }
    }
    Ok(device)
}

/// Why the saved state could not be read or written.
#[derive(Debug)]
pub enum StoreError {
    /// No config directory: neither `XDG_CONFIG_HOME` nor `HOME` is set.
    NoConfigDir,
    /// Reading or writing `path` failed.
    Io { path: PathBuf, source: io::Error },
    /// `path` is not a valid saved state. It is left as it is; move or
    /// delete it to start over.
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoConfigDir => write!(f, "no config directory (set XDG_CONFIG_HOME or HOME)"),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Corrupt { path, source } => write!(
                f,
                "{} is not a valid saved state ({source}); move or delete it to start over",
                path.display()
            ),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoConfigDir => None,
            Self::Io { source, .. } => Some(source),
            Self::Corrupt { source, .. } => Some(source),
        }
    }
}

/// The saved state's path for these values of `XDG_CONFIG_HOME` and `HOME`.
///
/// An absolute `XDG_CONFIG_HOME` wins on every platform (a relative one is
/// ignored, as the XDG spec says). Otherwise it is `~/.config`, or
/// `~/Library/Application Support` on macOS.
pub fn state_path_from(xdg_config_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    Some(config_dir_from(xdg_config_home, home, None)?.join("state.json"))
}

/// toppingctl's config directory for these values of `XDG_CONFIG_HOME`,
/// `HOME` and (on Windows) `APPDATA`: as [`state_path_from`], and on
/// Windows `%APPDATA%\toppingctl` when `XDG_CONFIG_HOME` is not set.
pub fn config_dir_from(
    xdg_config_home: Option<&Path>,
    home: Option<&Path>,
    appdata: Option<&Path>,
) -> Option<PathBuf> {
    let appdata = appdata.filter(|_| cfg!(windows));
    let base = match (xdg_config_home.filter(|p| p.is_absolute()), appdata) {
        (Some(xdg), _) => xdg.to_path_buf(),
        (None, Some(appdata)) => appdata.to_path_buf(),
        (None, None) if cfg!(target_os = "macos") => {
            home?.join("Library").join("Application Support")
        }
        (None, None) => home?.join(".config"),
    };
    Some(base.join("toppingctl"))
}

/// Writes `bytes` to `path` through a temp file next to it, renamed over,
/// so a crash never leaves a half-written file. Creates the directory.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".tmp{}", std::process::id()));
    let tmp = dir.join(tmp_name);
    let result = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// The saved state: a JSON file, written atomically.
#[derive(Clone, Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// A store at `path`.
    pub fn at(path: PathBuf) -> Self {
        Store { path }
    }

    /// The store at the platform's config path (see [`state_path_from`]),
    /// read from the environment.
    pub fn from_env() -> Result<Self, StoreError> {
        let var = |name| std::env::var_os(name).filter(|v: &OsString| !v.is_empty());
        let xdg = var("XDG_CONFIG_HOME").map(PathBuf::from);
        let home = var("HOME").map(PathBuf::from);
        let appdata = var("APPDATA").map(PathBuf::from);
        config_dir_from(xdg.as_deref(), home.as_deref(), appdata.as_deref())
            .map(|dir| Store::at(dir.join("state.json")))
            .ok_or(StoreError::NoConfigDir)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The saved state, or `None` when there is no file.
    pub fn load(&self) -> Result<Option<State>, StoreError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(self.io_error(source)),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| StoreError::Corrupt {
                path: self.path.clone(),
                source,
            })
    }

    /// Saves `state` (48V and the readout are left out). Writes a temp file
    /// next to the target and renames it over, so a crash never leaves a
    /// half-written file. Refuses to replace a corrupt file.
    pub fn save(&self, state: &State) -> Result<(), StoreError> {
        self.load()?;
        let json = serde_json::to_vec_pretty(state).expect("State serializes to JSON");
        write_atomic(&self.path, &json).map_err(|e| self.io_error(e))
    }

    fn io_error(&self, source: io::Error) -> StoreError {
        StoreError::Io {
            path: self.path.clone(),
            source,
        }
    }
}
