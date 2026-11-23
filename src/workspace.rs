//! Workspaces (#17): named snapshots of the [`State`], as in the official
//! app's workspace combo.
//!
//! Each workspace is a JSON file, `<config>/toppingctl/workspaces/<name>.json`,
//! holding `{"version": 1, "state": {…}}`. The open workspace and the
//! auto-save toggle live in `<config>/toppingctl/workspaces.json`. Names are
//! checked by [`valid_name`] before any path is built, so nothing is ever
//! read or written outside the workspaces directory.
//!
//! A file that is not valid JSON, or has a version other than 1, does not
//! load. A bare State object with no version (the saved-state format from
//! before workspaces, as in the test fixtures) loads through the same
//! back-compat path as the saved state. 48V and the readout are never in a
//! workspace (the [`State`] leaves them out of its JSON), and a load keeps
//! the device's own (see `App::load_workspace`).

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::State;
use crate::sync::{config_dir_from, write_atomic};

/// The workspace file format's version.
pub const VERSION: u64 = 1;
/// The longest workspace name, in characters.
pub const NAME_MAX: usize = 32;
/// Characters a workspace name can't hold: path separators and what
/// Windows refuses in a file name.
const FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
/// Names Windows reserves for devices (with any extension).
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Whether `c` can be typed into a workspace name.
pub fn valid_char(c: char) -> bool {
    !c.is_control() && !FORBIDDEN.contains(&c)
}

/// Whether `name` is a safe workspace name: 1 to [`NAME_MAX`] characters,
/// no path separator or other character a file name can't hold, no
/// leading or trailing space, not starting or ending with a dot, not a
/// Windows device name, and not "Default" (the combo's unnamed entry).
pub fn valid_name(name: &str) -> bool {
    let len = name.chars().count();
    let stem = name.split('.').next().unwrap_or_default().to_lowercase();
    (1..=NAME_MAX).contains(&len)
        && name.trim() == name
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name.chars().all(valid_char)
        && !RESERVED.contains(&stem.as_str())
        && !name.eq_ignore_ascii_case("default")
}

/// The open workspace and the auto-save toggle.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The workspace last opened or saved; `None` is Default.
    pub current: Option<String>,
    /// Save every change to the open workspace too (off by default).
    pub auto_save: bool,
}

/// Why a workspace could not be read, written or removed.
#[derive(Debug)]
pub enum WorkspaceError {
    /// Not a valid workspace name (see [`valid_name`]).
    Name(String),
    /// No config directory.
    NoConfigDir,
    /// Reading, writing or removing `path` failed.
    Io { path: PathBuf, source: io::Error },
    /// `path` is not a workspace file.
    Corrupt { path: PathBuf, reason: String },
    /// `path` has a version this toppingctl doesn't know.
    Version { path: PathBuf, found: String },
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => write!(f, "{name:?} is not a valid workspace name"),
            Self::NoConfigDir => write!(f, "no config directory (set XDG_CONFIG_HOME or HOME)"),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Corrupt { path, reason } => {
                write!(f, "{} is not a valid workspace ({reason})", path.display())
            }
            Self::Version { path, found } => write!(
                f,
                "{} has workspace version {found}, this toppingctl reads version {VERSION}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The workspaces in one config directory.
#[derive(Clone, Debug)]
pub struct Workspaces {
    /// toppingctl's config directory (`…/toppingctl`).
    dir: PathBuf,
}

impl Workspaces {
    /// The workspaces under the config directory `dir` (`…/toppingctl`).
    pub fn at(dir: PathBuf) -> Self {
        Workspaces { dir }
    }

    /// The workspaces under the platform's config directory, read from the
    /// environment (as the saved state's, see `sync::config_dir_from`).
    pub fn from_env() -> Result<Self, WorkspaceError> {
        let var = |name| std::env::var_os(name).filter(|v: &OsString| !v.is_empty());
        let xdg = var("XDG_CONFIG_HOME").map(PathBuf::from);
        let home = var("HOME").map(PathBuf::from);
        let appdata = var("APPDATA").map(PathBuf::from);
        config_dir_from(xdg.as_deref(), home.as_deref(), appdata.as_deref())
            .map(Workspaces::at)
            .ok_or(WorkspaceError::NoConfigDir)
    }

    fn folder(&self) -> PathBuf {
        self.dir.join("workspaces")
    }

    fn settings_path(&self) -> PathBuf {
        self.dir.join("workspaces.json")
    }

    /// The file of workspace `name`, once the name is checked.
    pub fn path(&self, name: &str) -> Result<PathBuf, WorkspaceError> {
        if !valid_name(name) {
            return Err(WorkspaceError::Name(name.to_string()));
        }
        Ok(self.folder().join(format!("{name}.json")))
    }

    /// The saved workspaces' names, sorted. Files that aren't workspaces
    /// (another extension, a name [`valid_name`] refuses) are left out.
    pub fn list(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.folder()) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| {
                let path = e.path();
                if path.extension()? != "json" {
                    return None;
                }
                let stem = path.file_stem()?.to_str()?.to_string();
                valid_name(&stem).then_some(stem)
            })
            .collect();
        names.sort();
        names
    }

    /// Saves `state` as workspace `name` (48V and the readout are left
    /// out), atomically.
    pub fn save(&self, name: &str, state: &State) -> Result<(), WorkspaceError> {
        let path = self.path(name)?;
        let json = serde_json::to_vec_pretty(&serde_json::json!({
            "version": VERSION,
            "state": state,
        }))
        .expect("State serializes to JSON");
        write_atomic(&path, &json).map_err(|source| WorkspaceError::Io { path, source })
    }

    /// Workspace `name`'s state (with no 48V and no readout).
    pub fn load(&self, name: &str) -> Result<State, WorkspaceError> {
        let path = self.path(name)?;
        let bytes = fs::read(&path).map_err(|source| WorkspaceError::Io {
            path: path.clone(),
            source,
        })?;
        parse(&bytes).map_err(|e| match e {
            Parse::Corrupt(reason) => WorkspaceError::Corrupt { path, reason },
            Parse::Version(found) => WorkspaceError::Version { path, found },
        })
    }

    /// Removes workspace `name`.
    pub fn delete(&self, name: &str) -> Result<(), WorkspaceError> {
        let path = self.path(name)?;
        fs::remove_file(&path).map_err(|source| WorkspaceError::Io { path, source })
    }

    /// The open workspace and auto-save, or the defaults when the file is
    /// missing or unreadable. A pointer to a name [`valid_name`] refuses is
    /// dropped.
    pub fn settings(&self) -> Settings {
        let mut s: Settings = fs::read(self.settings_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        s.current = s.current.filter(|n| valid_name(n));
        s
    }

    /// Saves the open workspace and auto-save, atomically.
    pub fn save_settings(&self, settings: &Settings) -> Result<(), WorkspaceError> {
        let path = self.settings_path();
        let json = serde_json::to_vec_pretty(settings).expect("Settings serialize to JSON");
        write_atomic(&path, &json).map_err(|source| WorkspaceError::Io { path, source })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Why a workspace file didn't parse.
enum Parse {
    Corrupt(String),
    Version(String),
}

/// A workspace file's state: `{"version": 1, "state": {…}}`, or a bare
/// State object (the format from before workspaces).
fn parse(bytes: &[u8]) -> Result<State, Parse> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| Parse::Corrupt(e.to_string()))?;
    let Value::Object(mut map) = value else {
        return Err(Parse::Corrupt("not a JSON object".into()));
    };
    let state = match map.remove("version") {
        None => Value::Object(map),
        Some(v) if v.as_u64() == Some(VERSION) => map
            .remove("state")
            .ok_or_else(|| Parse::Corrupt("no state".into()))?,
        Some(v) => return Err(Parse::Version(v.to_string())),
    };
    serde_json::from_value(state).map_err(|e| Parse::Corrupt(e.to_string()))
}
