//! Importing the official app's workspaces (#58).
//!
//! The official app (TOPPING Professional Control Center V1.09) keeps each
//! workspace as plain JSON, `<WS_DATA_PATH>/TPwork/E2X2_<name>.TPwork`
//! (its Default is `Default.config` beside them). `WS_DATA_PATH` comes from
//! the app's settings, `sound_card_workspace_file.TPconfig`; on macOS both
//! live under `~/Library/Application Support/TOPPING_PRO`. There is no
//! official app on Linux, so the folder is simply missing there.
//!
//! [`parse`] maps every field toppingctl models into a [`State`]; unknown
//! fields are ignored and missing ones keep the defaults. The mapping
//! (checked against PROTOCOL.md and a workspace whose screen in the app is
//! known, `tests/fixtures/official/`):
//!
//! - `IN1` / `IN2`: `IN_volume` gain dB (its `IN_volumenum` is the app's
//!   [`INPUT_GAIN_LEVELS`] word for it), `MUTEEnable`, `SOLOEnable`,
//!   `ANTIEnable` (Ø), `MONIEnable`, `INSTEnable`. **`P48VEnable` is never
//!   imported** (project rule): it is dropped, with a notice when it was on.
//! - `MixSaveA/B/C`, strips `MixParmeter_1` (IN 1+2) and `_2` … `_4` (PB 1/2,
//!   3/4, 5/6), `_L` = IN1 / the odd channel, `_R` = IN2 / the even one:
//!   `LinkEnable`; `Mix_volume_*num` the fader as the app's crosspoint step
//!   (335544 = 0 dB, see [`crate::state::crosspoint_level`]); `Mix_channel_*`
//!   the pan, 0 … 100 towards the channel's own side (50 = centre, 100 on
//!   `_L` = L100, on `_R` = R100); `MUTEEnable_*`, `SOLOEnable_*`,
//!   `ANTIEnable_*`.
//! - `Loopback12` / `Loopback34` and `OUT12`: `ChannelOUT_mode` the source
//!   register value, `LinkEnable`, `OUT_volume_*num` the level word
//!   (`0x02000000` = 0 dB); a loopback's `MUTEEnable_*`; the output's
//!   `HPEnable`, `LINEOUTEnable`.
//! - `Gain_Enable`: Phone Out +17 dBu (true) or 0 dBu.
//!
//! The output has no MUTE in toppingctl: a muted output channel comes in at
//! −∞, with a notice. Ø on a loopback or the output is not modelled and is
//! dropped, with a notice. `Mixer` / `Mixer_main` (the app's shown page),
//! the `*_text` labels, the solo flags and the slider positions
//! (`Mix_volume_*`, `OUT_volume_*` without `num`) are ignored.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::state::{Source, State, CROSSPOINT_MAX, CROSSPOINT_ZERO_DB, INPUT_GAIN_LEVELS};
use crate::value;
use crate::workspace::{valid_name, WorkspaceError, Workspaces};

/// The official app's folder under the home directory (macOS).
pub const APP_DIR: &str = "Library/Application Support/TOPPING_PRO";
/// The official app's settings file, in [`APP_DIR`].
pub const SETTINGS_FILE: &str = "sound_card_workspace_file.TPconfig";
/// The prefix of an E2x2 workspace's file name.
pub const PREFIX: &str = "E2X2_";
/// A workspace file's extension.
pub const EXTENSION: &str = "TPwork";

/// A workspace read from the official app.
#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    pub state: State,
    /// What could not be carried over (48V that was on, …), for the user.
    pub notices: Vec<String>,
}

/// Maps an official workspace file (JSON) into a [`State`], starting from
/// the defaults. The error says why it isn't one.
pub fn parse(bytes: &[u8]) -> Result<Imported, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let Value::Object(root) = v else {
        return Err("not a JSON object".into());
    };
    let known = ["IN1", "IN2", "MixSaveA", "Loopback12", "OUT12"];
    if !known
        .iter()
        .any(|k| root.get(*k).is_some_and(Value::is_object))
    {
        return Err("no TOPPING workspace sections (IN1, MixSaveA, OUT12, …)".into());
    }
    let mut state = State::default();
    let mut notices = Vec::new();

    for (i, input) in state.inputs.iter_mut().enumerate() {
        let Some(sec) = section(&root, &format!("IN{}", i + 1)) else {
            continue;
        };
        if let Some(db) = int(sec, "IN_volume") {
            input.gain_db = db.clamp(0, 20) as u8;
        } else if let Some(word) = int(sec, "IN_volumenum") {
            if let Some(db) = INPUT_GAIN_LEVELS.iter().position(|&w| i64::from(w) == word) {
                input.gain_db = db as u8;
            }
        }
        set(&mut input.mute, sec, "MUTEEnable");
        set(&mut input.solo, sec, "SOLOEnable");
        set(&mut input.phase, sec, "ANTIEnable");
        set(&mut input.mon, sec, "MONIEnable");
        set(&mut input.inst, sec, "INSTEnable");
        if flag(sec, "P48VEnable") == Some(true) {
            notices.push(format!(
                "48V phantom power was on for IN{} in the TOPPING workspace; it is never \
                 imported (turn it on in toppingctl if you need it)",
                i + 1
            ));
        }
    }

    for (m, mix) in state.mixes.iter_mut().enumerate() {
        let Some(sec) = section(&root, &format!("MixSave{}", ["A", "B", "C"][m])) else {
            continue;
        };
        for strip in 0..4 {
            let key = |k: &str| format!("MixParmeter_{}.{k}", strip + 1);
            if let Some(on) = flag(sec, &key("LinkEnable")) {
                *mix.link_mut(strip) = on;
            }
            for (ch, side) in ["L", "R"].into_iter().enumerate() {
                if let Some(p) = int(sec, &key(&format!("Mix_channel_{side}"))) {
                    let toward = (p as f64 / 100.0).clamp(0.0, 1.0);
                    *mix.pan_mut(strip, ch) = if ch == 0 { 1.0 - toward } else { toward };
                }
                let c = mix.channel_mut(strip, ch);
                if let Some(num) = int(sec, &key(&format!("Mix_volume_{side}num"))) {
                    *c.fader_db = fader_db(num);
                }
                set(c.mute, sec, &key(&format!("MUTEEnable_{side}")));
                set(c.solo, sec, &key(&format!("SOLOEnable_{side}")));
                set(c.phase, sec, &key(&format!("ANTIEnable_{side}")));
            }
        }
    }

    let names = ["Loopback 1+2", "Loopback 3+4"];
    for ((lb, key), name) in state
        .loopbacks
        .iter_mut()
        .zip(["Loopback12", "Loopback34"])
        .zip(names)
    {
        let Some(sec) = section(&root, key) else {
            continue;
        };
        if let Some(src) = source(sec, name, &mut notices) {
            lb.source = src;
        }
        set(&mut lb.link, sec, "LinkEnable");
        if let Some(db) = int(sec, "OUT_volume_Lnum").map(level_db) {
            lb.level_l_db = db;
        }
        if let Some(db) = int(sec, "OUT_volume_Rnum").map(level_db) {
            lb.level_r_db = db;
        }
        set(&mut lb.mute_l, sec, "MUTEEnable_L");
        set(&mut lb.mute_r, sec, "MUTEEnable_R");
        phase_notice(sec, name, &mut notices);
    }

    if let Some(sec) = section(&root, "OUT12") {
        let name = "Output 1+2";
        let out = &mut state.output;
        if let Some(src) = source(sec, name, &mut notices) {
            out.source = src;
        }
        set(&mut out.link, sec, "LinkEnable");
        if let Some(db) = int(sec, "OUT_volume_Lnum").map(level_db) {
            out.level_l_db = db;
        }
        if let Some(db) = int(sec, "OUT_volume_Rnum").map(level_db) {
            out.level_r_db = db;
        }
        set(&mut out.headphones, sec, "HPEnable");
        set(&mut out.line, sec, "LINEOUTEnable");
        for (side, level) in [("L", &mut out.level_l_db), ("R", &mut out.level_r_db)] {
            if flag(sec, &format!("MUTEEnable_{side}")) == Some(true) {
                *level = f64::NEG_INFINITY;
                notices.push(format!(
                    "{name} {side} was muted; toppingctl has no output MUTE, so its level \
                     is −∞"
                ));
            }
        }
        phase_notice(sec, name, &mut notices);
    }

    if let Some(on) = root.get("Gain_Enable").and_then(Value::as_bool) {
        state.phone_gain = on;
    }
    Ok(Imported { state, notices })
}

fn section<'a>(root: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    root.get(key)?.as_object()
}

fn flag(sec: &Map<String, Value>, key: &str) -> Option<bool> {
    sec.get(key)?.as_bool()
}

fn int(sec: &Map<String, Value>, key: &str) -> Option<i64> {
    sec.get(key)?.as_i64()
}

/// Sets `slot` from `sec[key]` when that is a bool.
fn set(slot: &mut bool, sec: &Map<String, Value>, key: &str) {
    if let Some(on) = flag(sec, key) {
        *slot = on;
    }
}

/// `x` rounded to 0.01 (dB), so the app's integer words come back as the
/// values it shows; `-inf` stays.
fn round_db(x: f64) -> f64 {
    if x.is_finite() {
        (x * 100.0).round() / 100.0
    } else {
        x
    }
}

/// A mix fader's crosspoint step (`Mix_volume_*num`, 335544 = 0 dB) in dB,
/// capped at the fader top; 0 or less is −∞.
fn fader_db(num: i64) -> f64 {
    if num <= 0 {
        return f64::NEG_INFINITY;
    }
    let zero = f64::from(CROSSPOINT_ZERO_DB / 100);
    let top = f64::from(CROSSPOINT_MAX / 100);
    round_db(20.0 * ((num as f64).min(top) / zero).log10())
}

/// An output or loopback level word in dB (0 dB at most); 0 is −∞.
fn level_db(word: i64) -> f64 {
    let word = word.clamp(0, i64::from(i32::MAX)) as i32;
    round_db(value::db_from_level(word)).min(0.0)
}

/// The source in `ChannelOUT_mode`, or `None` (with a notice) for a value
/// that isn't one.
fn source(sec: &Map<String, Value>, name: &str, notices: &mut Vec<String>) -> Option<Source> {
    let wire = int(sec, "ChannelOUT_mode")?;
    let found = [
        Source::MixA,
        Source::MixB,
        Source::MixC,
        Source::Pb12,
        Source::Pb34,
        Source::Pb56,
        Source::In1,
        Source::In2,
        Source::In12,
    ]
    .into_iter()
    .find(|s| i64::from(s.wire()) == wire);
    if found.is_none() {
        notices.push(format!(
            "{name}: unknown source {wire}; kept toppingctl's default"
        ));
    }
    found
}

fn phase_notice(sec: &Map<String, Value>, name: &str, notices: &mut Vec<String>) {
    if flag(sec, "ANTIEnable") == Some(true) {
        notices.push(format!(
            "{name} had Ø on; toppingctl has no Ø there, so it was dropped"
        ));
    }
}

/// The workspace name for an official file: its name without
/// [`PREFIX`] and the extension (`E2X2_Live.TPwork` → `Live`), if that is a
/// valid workspace name.
pub fn default_name(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let name = strip_prefix(stem).unwrap_or(stem);
    valid_name(name).then(|| name.to_string())
}

fn strip_prefix(stem: &str) -> Option<&str> {
    let head = stem.get(..PREFIX.len())?;
    head.eq_ignore_ascii_case(PREFIX)
        .then(|| &stem[PREFIX.len()..])
}

/// The official app's workspace folder for home directory `home`:
/// `<WS_DATA_PATH>/TPwork`, with `WS_DATA_PATH` from the app's settings
/// file, else `<home>/`[`APP_DIR`]`/TPwork/TPwork` (the macOS default).
pub fn folder(home: &Path) -> PathBuf {
    let app = home.join(APP_DIR);
    fs::read(app.join(SETTINGS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("WS_DATA_PATH")?.as_str().map(PathBuf::from))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| app.join("TPwork"))
        .join("TPwork")
}

/// [`folder`] for `$HOME`, if it is set.
pub fn folder_from_env() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(folder(Path::new(&home)))
}

/// An E2x2 workspace in the official app's folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Official {
    /// The name it imports as ([`default_name`]).
    pub name: String,
    pub path: PathBuf,
}

/// The E2x2 workspaces (`E2X2_*.TPwork`) in `folder`, by name; empty when
/// the folder is missing (as on Linux).
pub fn list(folder: &Path) -> Vec<Official> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<Official> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let path = e.path();
            if !path.extension()?.to_str()?.eq_ignore_ascii_case(EXTENSION) {
                return None;
            }
            strip_prefix(path.file_stem()?.to_str()?)?;
            let name = default_name(&path)?;
            Some(Official { name, path })
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Why an import failed.
#[derive(Debug)]
pub enum ImportError {
    /// Reading the file failed.
    Read { path: PathBuf, source: io::Error },
    /// The file is not an official workspace.
    Parse { path: PathBuf, reason: String },
    /// No valid workspace name (none given and the file's won't do).
    Name(String),
    /// A workspace of that name exists and `force` was not given.
    Exists(String),
    /// Saving the workspace failed.
    Save(WorkspaceError),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse { path, reason } => write!(
                f,
                "{} is not a TOPPING app workspace ({reason})",
                path.display()
            ),
            Self::Name(name) => write!(f, "{name:?} is not a valid workspace name"),
            Self::Exists(name) => write!(f, "workspace {name:?} already exists"),
            Self::Save(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// What [`import`] saved.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub name: String,
    pub notices: Vec<String>,
}

/// Reads the official workspace at `path` and saves it as a toppingctl
/// workspace, named `name` or else [`default_name`]. An existing workspace
/// is replaced only with `force`. Nothing is sent to the device.
pub fn import(
    workspaces: &Workspaces,
    path: &Path,
    name: Option<&str>,
    force: bool,
) -> Result<Saved, ImportError> {
    let name = match name {
        Some(n) if valid_name(n) => n.to_string(),
        Some(n) => return Err(ImportError::Name(n.to_string())),
        None => default_name(path).ok_or_else(|| {
            ImportError::Name(
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        })?,
    };
    let bytes = fs::read(path).map_err(|source| ImportError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let imported = parse(&bytes).map_err(|reason| ImportError::Parse {
        path: path.to_path_buf(),
        reason,
    })?;
    if !force && workspaces.list().contains(&name) {
        return Err(ImportError::Exists(name));
    }
    workspaces
        .save(&name, &imported.state)
        .map_err(ImportError::Save)?;
    Ok(Saved {
        name,
        notices: imported.notices,
    })
}
