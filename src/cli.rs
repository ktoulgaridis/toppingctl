//! The command line: `toppingctl set|get|dump|store|init`.
//!
//! Each command connects with [`sync::connect`] (which pushes the saved
//! state), does its one thing and exits. Paths name [`State`] fields
//! (`in1.mon`, `mix.b.pb34.pan`, …); see [`USAGE`]. Everything that touches
//! the outside world comes in through [`Env`], so the CLI runs without
//! hardware in tests.
//!
//! Exit codes: 0 on success, 2 for a usage error (including a confirmation
//! that was not given), 1 for a device or saved-state error.

use std::ffi::OsString;
use std::fmt;
use std::io::{BufRead, Write};
use std::sync::mpsc::Receiver;

use crate::conn::{Connection, Event};
use crate::frame::Frame;
use crate::state::{PbStrip, Source, State, Strip};
use crate::sync::{self, Store, StoreError, SyncError};

/// `toppingctl --help`.
pub const USAGE: &str = "\
Usage: toppingctl
       toppingctl set <path> <value> [--yes]
       toppingctl get <path>
       toppingctl dump [--json]
       toppingctl store [--yes]
       toppingctl init [--yes]

Controls the TOPPING E2x2 USB audio interface. With no arguments it starts
the terminal UI; press ? inside it for the keys. Every command first pushes
the state toppingctl last set (the device can't be read back).

Commands:
  set    change one setting and save it
  get    print one setting
  dump   print every setting (--json: as the saved-state JSON)
  store  store the current settings in the device's own memory (asks first)
  init   first run: push the default settings to the device (asks first)

Paths:
  in1|in2.gain|mute|solo|phase|mon|inst|48v
  mix.a|b|c.in1|in2|pb12|pb34|pb56.fader|pan|mute|solo|phase
  mix.a|b|c.link
  loopback.1|2.source|level|mute
  out.source|level|hp|line
  phone_gain
  system.brightness|standby|mobile

Values:
  switches    on, off, true, false, 1, 0
  gain        0 .. 20 dB, e.g. 12 or 12dB
  fader       -inf .. +6 dB, e.g. -6, -6dB or -inf
  level       -inf .. 0 dB (loopback and out levels; sets L and R)
  pan         L100 .. C .. R100, or 0 (left) .. 1 (right); on pb12,
              pb34 and pb56 a balance over the two channels' pans
  source      mixa mixb mixc pb12 pb34 pb56 in1 in2 in1+2
  brightness  0, 1, 2 (1 = normal)

Options:
  -y, --yes      don't ask before 48V on, store or init
  -h, --help     show this help
  -V, --version  show the version

Exit status: 0 ok, 2 usage error or not confirmed, 1 device error.

Environment:
  NO_COLOR       set to turn the TUI's colours off";

/// The valid paths, compactly (as in [`USAGE`]).
const PATHS: &str = "  in1|in2.gain|mute|solo|phase|mon|inst|48v
  mix.a|b|c.in1|in2|pb12|pb34|pb56.fader|pan|mute|solo|phase
  mix.a|b|c.link
  loopback.1|2.source|level|mute
  out.source|level|hp|line
  phone_gain
  system.brightness|standby|mobile";

/// What the CLI talks to.
pub struct Env<'a> {
    /// The saved state (an error when there is no config directory).
    pub store: Result<Store, StoreError>,
    /// Whether stdin is a terminal, so a `[y/N]` prompt can be answered.
    pub stdin_is_tty: bool,
    pub stdin: &'a mut dyn BufRead,
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    /// Opens the device; the error is a message for the user.
    #[allow(clippy::type_complexity)]
    pub open: Box<dyn FnMut() -> Result<(Connection, Receiver<Event>), String> + 'a>,
}

/// Runs the CLI with `args` (without the program name) and returns the exit
/// code.
pub fn run(args: &[OsString], env: &mut Env) -> u8 {
    let result = match parse_args(args) {
        Ok(cmd) => execute(cmd, env),
        Err(msg) => Err(Failure::Usage(format!(
            "{msg}\n\nRun 'toppingctl --help' for usage."
        ))),
    };
    match result {
        Ok(()) => 0,
        Err(Failure::Usage(msg)) => {
            let _ = writeln!(env.stderr, "toppingctl: {msg}");
            2
        }
        Err(Failure::Device(msg)) => {
            let _ = writeln!(env.stderr, "toppingctl: {msg}");
            1
        }
    }
}

// ---------------------------------------------------------------- paths

/// An input field (`in1.<field>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputField {
    Gain,
    Mute,
    Solo,
    Phase,
    Mon,
    Inst,
    Phantom,
}

/// A mix strip (`mix.a.<strip>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripId {
    In1,
    In2,
    Pb12,
    Pb34,
    Pb56,
}

/// A strip field (`mix.a.in1.<field>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripField {
    Fader,
    Pan,
    Mute,
    Solo,
    Phase,
}

/// A loopback field (`loopback.1.<field>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopbackField {
    Source,
    Level,
    Mute,
}

/// An output field (`out.<field>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutField {
    Source,
    Level,
    Headphones,
    Line,
}

/// A settings field (`system.<field>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemField {
    Brightness,
    Standby,
    Mobile,
}

const INPUT_FIELDS: [(&str, InputField); 7] = [
    ("gain", InputField::Gain),
    ("mute", InputField::Mute),
    ("solo", InputField::Solo),
    ("phase", InputField::Phase),
    ("mon", InputField::Mon),
    ("inst", InputField::Inst),
    ("48v", InputField::Phantom),
];
const STRIPS: [(&str, StripId); 5] = [
    ("in1", StripId::In1),
    ("in2", StripId::In2),
    ("pb12", StripId::Pb12),
    ("pb34", StripId::Pb34),
    ("pb56", StripId::Pb56),
];
const STRIP_FIELDS: [(&str, StripField); 5] = [
    ("fader", StripField::Fader),
    ("pan", StripField::Pan),
    ("mute", StripField::Mute),
    ("solo", StripField::Solo),
    ("phase", StripField::Phase),
];
const LOOPBACK_FIELDS: [(&str, LoopbackField); 3] = [
    ("source", LoopbackField::Source),
    ("level", LoopbackField::Level),
    ("mute", LoopbackField::Mute),
];
const OUT_FIELDS: [(&str, OutField); 4] = [
    ("source", OutField::Source),
    ("level", OutField::Level),
    ("hp", OutField::Headphones),
    ("line", OutField::Line),
];
const SYSTEM_FIELDS: [(&str, SystemField); 3] = [
    ("brightness", SystemField::Brightness),
    ("standby", SystemField::Standby),
    ("mobile", SystemField::Mobile),
];
const MIXES: [&str; 3] = ["a", "b", "c"];
const SOURCES: [(&str, Source); 9] = [
    ("mixa", Source::MixA),
    ("mixb", Source::MixB),
    ("mixc", Source::MixC),
    ("pb12", Source::Pb12),
    ("pb34", Source::Pb34),
    ("pb56", Source::Pb56),
    ("in1", Source::In1),
    ("in2", Source::In2),
    ("in1+2", Source::In12),
];

fn name_of<T: PartialEq + Copy>(table: &[(&'static str, T)], v: T) -> &'static str {
    table.iter().find(|(_, t)| *t == v).map_or("?", |(n, _)| n)
}

/// A setting, as named on the command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    /// `in1|in2.<field>`; the index is 0 or 1.
    Input(usize, InputField),
    /// `mix.a|b|c.<strip>.<field>`; the index is 0 ..= 2.
    Strip(usize, StripId, StripField),
    /// `mix.a|b|c.link`.
    Link(usize),
    /// `loopback.1|2.<field>`; the index is 0 or 1.
    Loopback(usize, LoopbackField),
    Out(OutField),
    PhoneGain,
    System(SystemField),
}

/// The kind of value a path takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Switch,
    Gain,
    Fader,
    Level,
    Pan,
    Source,
    Brightness,
}

/// A parsed value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    /// Fader or level, in dB (`-inf` allowed).
    Db(f64),
    /// Input gain, whole dB.
    Gain(u8),
    /// 0 = left, 0.5 = centre, 1 = right.
    Pan(f64),
    Source(Source),
    /// Display brightness.
    Level(u8),
}

impl Path {
    /// Every path, in the order of [`USAGE`].
    pub fn all() -> Vec<Path> {
        let mut out = Vec::with_capacity(106);
        for ch in 0..2 {
            out.extend(INPUT_FIELDS.iter().map(|&(_, f)| Path::Input(ch, f)));
        }
        for m in 0..3 {
            for &(_, s) in &STRIPS {
                out.extend(STRIP_FIELDS.iter().map(|&(_, f)| Path::Strip(m, s, f)));
            }
            out.push(Path::Link(m));
        }
        for n in 0..2 {
            out.extend(LOOPBACK_FIELDS.iter().map(|&(_, f)| Path::Loopback(n, f)));
        }
        out.extend(OUT_FIELDS.iter().map(|&(_, f)| Path::Out(f)));
        out.push(Path::PhoneGain);
        out.extend(SYSTEM_FIELDS.iter().map(|&(_, f)| Path::System(f)));
        out
    }

    /// The path named `s`; the error lists the valid paths.
    pub fn parse(s: &str) -> Result<Path, String> {
        Path::all()
            .into_iter()
            .find(|p| p.to_string() == s)
            .ok_or_else(|| format!("unknown path {s:?}; valid paths:\n{PATHS}"))
    }

    fn kind(self) -> Kind {
        match self {
            Path::Input(_, InputField::Gain) => Kind::Gain,
            Path::Input(..) => Kind::Switch,
            Path::Strip(_, _, StripField::Fader) => Kind::Fader,
            Path::Strip(_, _, StripField::Pan) => Kind::Pan,
            Path::Strip(..) | Path::Link(_) => Kind::Switch,
            Path::Loopback(_, LoopbackField::Source) | Path::Out(OutField::Source) => Kind::Source,
            Path::Loopback(_, LoopbackField::Level) | Path::Out(OutField::Level) => Kind::Level,
            Path::Loopback(..) | Path::Out(_) | Path::PhoneGain => Kind::Switch,
            Path::System(SystemField::Brightness) => Kind::Brightness,
            Path::System(_) => Kind::Switch,
        }
    }

    /// Parses a value for this path; the error says what is accepted.
    pub fn parse_value(self, s: &str) -> Result<Value, String> {
        let kind = self.kind();
        let bad = || format!("bad value {s:?} for {self}; expected {}", accepted(kind));
        let lower = s.trim().to_ascii_lowercase();
        let v = match kind {
            Kind::Switch => match lower.as_str() {
                "on" | "true" | "1" => Value::Bool(true),
                "off" | "false" | "0" => Value::Bool(false),
                _ => return Err(bad()),
            },
            Kind::Gain => {
                let gain: u8 = strip_db(&lower).parse().map_err(|_| bad())?;
                if gain > 20 {
                    return Err(bad());
                }
                Value::Gain(gain)
            }
            Kind::Fader => Value::Db(parse_db(&lower, 6.0).ok_or_else(bad)?),
            Kind::Level => Value::Db(parse_db(&lower, 0.0).ok_or_else(bad)?),
            Kind::Pan => Value::Pan(parse_pan(&lower).ok_or_else(bad)?),
            Kind::Source => Value::Source(
                SOURCES
                    .iter()
                    .find(|(n, _)| *n == lower)
                    .map(|&(_, src)| src)
                    .ok_or_else(bad)?,
            ),
            Kind::Brightness => match lower.as_str() {
                "0" | "1" | "2" => Value::Level(lower.parse().unwrap_or(1)),
                _ => return Err(bad()),
            },
        };
        Ok(v)
    }

    /// Sets this path in `state`. A value of the wrong kind is ignored.
    pub fn set(self, state: &mut State, value: Value) {
        match (self, value) {
            (Path::Input(ch, InputField::Gain), Value::Gain(g)) => state.inputs[ch].gain_db = g,
            (Path::Input(ch, f), Value::Bool(b)) => {
                let input = &mut state.inputs[ch];
                match f {
                    InputField::Gain => {}
                    InputField::Mute => input.mute = b,
                    InputField::Solo => input.solo = b,
                    InputField::Phase => input.phase = b,
                    InputField::Mon => input.mon = b,
                    InputField::Inst => input.inst = b,
                    InputField::Phantom => input.phantom = Some(b),
                }
            }
            (Path::Strip(m, s, f), v) => {
                let (fader, mute, solo, phase) = match strip_mut(state, m, s) {
                    StripMut::Mono(strip) => {
                        if let (StripField::Pan, Value::Pan(p)) = (f, v) {
                            strip.pan = p;
                        }
                        (
                            &mut strip.fader_db,
                            &mut strip.mute,
                            &mut strip.solo,
                            &mut strip.phase,
                        )
                    }
                    StripMut::Stereo(strip) => {
                        // On a stereo strip, `pan` is a balance over both pans.
                        if let (StripField::Pan, Value::Pan(p)) = (f, v) {
                            strip.set_balance(p);
                        }
                        (
                            &mut strip.fader_db,
                            &mut strip.mute,
                            &mut strip.solo,
                            &mut strip.phase,
                        )
                    }
                };
                match (f, v) {
                    (StripField::Fader, Value::Db(db)) => *fader = db,
                    (StripField::Mute, Value::Bool(b)) => *mute = b,
                    (StripField::Solo, Value::Bool(b)) => *solo = b,
                    (StripField::Phase, Value::Bool(b)) => *phase = b,
                    _ => {}
                }
            }
            (Path::Link(m), Value::Bool(b)) => state.mixes[m].link = b,
            (Path::Loopback(n, f), v) => {
                let lb = &mut state.loopbacks[n];
                match (f, v) {
                    (LoopbackField::Source, Value::Source(src)) => lb.source = src,
                    (LoopbackField::Level, Value::Db(db)) => lb.set_level(db),
                    (LoopbackField::Mute, Value::Bool(b)) => lb.mute = b,
                    _ => {}
                }
            }
            (Path::Out(f), v) => {
                let out = &mut state.output;
                match (f, v) {
                    (OutField::Source, Value::Source(src)) => out.source = src,
                    (OutField::Level, Value::Db(db)) => out.set_level(db),
                    (OutField::Headphones, Value::Bool(b)) => out.headphones = b,
                    (OutField::Line, Value::Bool(b)) => out.line = b,
                    _ => {}
                }
            }
            (Path::PhoneGain, Value::Bool(b)) => state.phone_gain = b,
            (Path::System(SystemField::Brightness), Value::Level(l)) => state.system.brightness = l,
            (Path::System(SystemField::Standby), Value::Bool(b)) => state.system.auto_standby = b,
            (Path::System(SystemField::Mobile), Value::Bool(b)) => state.system.mobile_apps = b,
            _ => {}
        }
    }

    /// This path's value in `state`, in a form [`Path::parse_value`] accepts
    /// (48V prints `unknown` until the device reports it).
    pub fn get(self, state: &State) -> String {
        match self {
            Path::Input(ch, f) => {
                let input = &state.inputs[ch];
                match f {
                    InputField::Gain => format!("{}dB", input.gain_db),
                    InputField::Mute => on_off(input.mute),
                    InputField::Solo => on_off(input.solo),
                    InputField::Phase => on_off(input.phase),
                    InputField::Mon => on_off(input.mon),
                    InputField::Inst => on_off(input.inst),
                    InputField::Phantom => input.phantom.map_or("unknown".into(), on_off),
                }
            }
            Path::Strip(m, s, f) => {
                let (fader, pan, mute, solo, phase) = match strip_ref(state, m, s) {
                    StripRef::Mono(s) => (s.fader_db, s.pan, s.mute, s.solo, s.phase),
                    StripRef::Stereo(s) => (s.fader_db, s.balance(), s.mute, s.solo, s.phase),
                };
                match f {
                    StripField::Fader => show_db(fader),
                    StripField::Pan => show_pan(pan),
                    StripField::Mute => on_off(mute),
                    StripField::Solo => on_off(solo),
                    StripField::Phase => on_off(phase),
                }
            }
            Path::Link(m) => on_off(state.mixes[m].link),
            Path::Loopback(n, f) => {
                let lb = &state.loopbacks[n];
                match f {
                    LoopbackField::Source => name_of(&SOURCES, lb.source).into(),
                    LoopbackField::Level => show_db(lb.level_l_db),
                    LoopbackField::Mute => on_off(lb.mute),
                }
            }
            Path::Out(f) => {
                let out = &state.output;
                match f {
                    OutField::Source => name_of(&SOURCES, out.source).into(),
                    OutField::Level => show_db(out.level_l_db),
                    OutField::Headphones => on_off(out.headphones),
                    OutField::Line => on_off(out.line),
                }
            }
            Path::PhoneGain => on_off(state.phone_gain),
            Path::System(f) => match f {
                SystemField::Brightness => state.system.brightness.to_string(),
                SystemField::Standby => on_off(state.system.auto_standby),
                SystemField::Mobile => on_off(state.system.mobile_apps),
            },
        }
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Path::Input(ch, field) => {
                write!(f, "in{}.{}", ch + 1, name_of(&INPUT_FIELDS, field))
            }
            Path::Strip(m, s, field) => write!(
                f,
                "mix.{}.{}.{}",
                MIXES[m],
                name_of(&STRIPS, s),
                name_of(&STRIP_FIELDS, field)
            ),
            Path::Link(m) => write!(f, "mix.{}.link", MIXES[m]),
            Path::Loopback(n, field) => {
                write!(f, "loopback.{}.{}", n + 1, name_of(&LOOPBACK_FIELDS, field))
            }
            Path::Out(field) => write!(f, "out.{}", name_of(&OUT_FIELDS, field)),
            Path::PhoneGain => write!(f, "phone_gain"),
            Path::System(field) => write!(f, "system.{}", name_of(&SYSTEM_FIELDS, field)),
        }
    }
}

/// A mix strip: mono (IN1, IN2) or stereo (PB).
enum StripMut<'a> {
    Mono(&'a mut Strip),
    Stereo(&'a mut PbStrip),
}

enum StripRef<'a> {
    Mono(&'a Strip),
    Stereo(&'a PbStrip),
}

fn strip_mut(state: &mut State, m: usize, s: StripId) -> StripMut<'_> {
    let mix = &mut state.mixes[m];
    match s {
        StripId::In1 => StripMut::Mono(&mut mix.in1),
        StripId::In2 => StripMut::Mono(&mut mix.in2),
        StripId::Pb12 => StripMut::Stereo(&mut mix.pb12),
        StripId::Pb34 => StripMut::Stereo(&mut mix.pb34),
        StripId::Pb56 => StripMut::Stereo(&mut mix.pb56),
    }
}

fn strip_ref(state: &State, m: usize, s: StripId) -> StripRef<'_> {
    let mix = &state.mixes[m];
    match s {
        StripId::In1 => StripRef::Mono(&mix.in1),
        StripId::In2 => StripRef::Mono(&mix.in2),
        StripId::Pb12 => StripRef::Stereo(&mix.pb12),
        StripId::Pb34 => StripRef::Stereo(&mix.pb34),
        StripId::Pb56 => StripRef::Stereo(&mix.pb56),
    }
}

/// What a value of `kind` may be, for error messages.
fn accepted(kind: Kind) -> &'static str {
    match kind {
        Kind::Switch => "on, off, true, false, 1, 0",
        Kind::Gain => "a whole number of dB, 0 .. 20 (e.g. 12 or 12dB)",
        Kind::Fader => "dB, -inf .. +6 dB (e.g. -6, -6dB or -inf)",
        Kind::Level => "dB, -inf .. 0 dB (e.g. -6, -6dB or -inf)",
        Kind::Pan => "L100 .. C .. R100, or 0 (left) .. 1 (right)",
        Kind::Source => "one of mixa mixb mixc pb12 pb34 pb56 in1 in2 in1+2",
        Kind::Brightness => "0, 1, 2 (1 = normal)",
    }
}

/// `s` without a trailing `db` (already lower-cased).
fn strip_db(s: &str) -> &str {
    s.strip_suffix("db").unwrap_or(s).trim_end()
}

/// dB from `-inf` up to `max` (already lower-cased).
fn parse_db(s: &str, max: f64) -> Option<f64> {
    let s = strip_db(s);
    if s == "-inf" {
        return Some(f64::NEG_INFINITY);
    }
    let db: f64 = s.parse().ok()?;
    (db.is_finite() && db <= max).then_some(db)
}

/// `L<n>`, `C`, `R<n>` (n 0 ..= 100) or a number 0 ..= 1 (already lower-cased).
fn parse_pan(s: &str) -> Option<f64> {
    if s == "c" {
        return Some(0.5);
    }
    let percent = |n: &str| n.parse::<f64>().ok().filter(|n| (0.0..=100.0).contains(n));
    if let Some(n) = s.strip_prefix('l') {
        return percent(n).map(|n| 0.5 - n / 200.0);
    }
    if let Some(n) = s.strip_prefix('r') {
        return percent(n).map(|n| 0.5 + n / 200.0);
    }
    s.parse::<f64>().ok().filter(|p| (0.0..=1.0).contains(p))
}

fn on_off(b: bool) -> String {
    if b { "on" } else { "off" }.into()
}

fn show_db(db: f64) -> String {
    if db.is_finite() {
        // + 0.0 turns -0 into 0.
        format!("{}dB", db + 0.0)
    } else {
        "-inf".into()
    }
}

fn show_pan(pan: f64) -> String {
    let percent = ((pan - 0.5) * 2000.0).round() / 10.0;
    if percent == 0.0 {
        "C".into()
    } else if percent < 0.0 {
        format!("L{}", -percent)
    } else {
        format!("R{percent}")
    }
}

// ---------------------------------------------------------------- commands

#[derive(Debug)]
enum Command {
    Help,
    Version,
    Set { path: Path, value: Value, yes: bool },
    Get(Path),
    Dump { json: bool },
    Store { yes: bool },
    Init { yes: bool },
}

/// Why a command failed.
enum Failure {
    /// Exit 2.
    Usage(String),
    /// Exit 1.
    Device(String),
}

fn parse_args(args: &[OsString]) -> Result<Command, String> {
    let args: Vec<&str> = args
        .iter()
        .map(|a| {
            a.to_str()
                .ok_or_else(|| format!("argument {a:?} is not UTF-8"))
        })
        .collect::<Result<_, _>>()?;
    if args.iter().any(|a| matches!(*a, "-h" | "--help")) {
        return Ok(Command::Help);
    }
    let Some((&cmd, rest)) = args.split_first() else {
        return Ok(Command::Help);
    };
    if matches!(cmd, "-V" | "--version") {
        return Ok(Command::Version);
    }
    // Flags start with "--" (or are -y); "-6" and "-inf" are values.
    let mut yes = false;
    let mut json = false;
    let mut positional = Vec::new();
    for &a in rest {
        match a {
            "-y" | "--yes" if matches!(cmd, "set" | "store" | "init") => yes = true,
            "--json" if cmd == "dump" => json = true,
            _ if a.starts_with("--") || a == "-y" => {
                return Err(format!("unknown option {a:?} for {cmd}"))
            }
            _ => positional.push(a),
        }
    }
    let want = |n: usize, what: &str| {
        if positional.len() == n {
            Ok(())
        } else {
            Err(format!("{cmd} takes {what}"))
        }
    };
    match cmd {
        "set" => {
            want(2, "a path and a value")?;
            let path = Path::parse(positional[0])?;
            let value = path.parse_value(positional[1])?;
            Ok(Command::Set { path, value, yes })
        }
        "get" => {
            want(1, "one path")?;
            Ok(Command::Get(Path::parse(positional[0])?))
        }
        "dump" => want(0, "no arguments").map(|()| Command::Dump { json }),
        "store" => want(0, "no arguments").map(|()| Command::Store { yes }),
        "init" => want(0, "no arguments").map(|()| Command::Init { yes }),
        _ => Err(format!("unknown command {cmd:?}")),
    }
}

fn execute(cmd: Command, env: &mut Env) -> Result<(), Failure> {
    let out = |e: std::io::Error| Failure::Device(format!("could not write output: {e}"));
    match cmd {
        Command::Help => writeln!(env.stdout, "{USAGE}").map_err(out),
        Command::Version => {
            writeln!(env.stdout, "toppingctl {}", env!("CARGO_PKG_VERSION")).map_err(out)
        }
        Command::Set {
            path: Path::Input(ch, InputField::Phantom),
            value,
            yes,
        } => {
            let on = value == Value::Bool(true);
            if on {
                let question = format!("Turn 48V phantom power on for IN{}?", ch + 1);
                confirm(env, yes, &question)?;
            }
            let s = connect(env)?;
            let f = phantom_frame(ch, on);
            // Never saved: 48V is not part of the persisted state.
            let sent = if on {
                s.conn.send_confirmed_phantom(f)
            } else {
                s.conn.send(f)
            };
            sent.map_err(device)?;
            finish(s.conn, s.events)
        }
        Command::Set { path, value, .. } => {
            let Session {
                conn,
                events,
                state,
                store,
            } = connect(env)?;
            let mut new = state.clone();
            path.set(&mut new, value);
            for f in state.diff(&new) {
                conn.send(f).map_err(device)?;
            }
            finish(conn, events)?;
            store.save(&new).map_err(device)
        }
        Command::Get(path) => {
            let s = connect(env)?;
            let value = path.get(&s.state);
            finish(s.conn, s.events)?;
            writeln!(env.stdout, "{value}").map_err(out)
        }
        Command::Dump { json } => {
            let s = connect(env)?;
            finish(s.conn, s.events)?;
            if json {
                let text = serde_json::to_string_pretty(&s.state).expect("State serializes");
                writeln!(env.stdout, "{text}").map_err(out)
            } else {
                for p in Path::all() {
                    writeln!(env.stdout, "{p} {}", p.get(&s.state)).map_err(out)?;
                }
                Ok(())
            }
        }
        Command::Store { yes } => {
            confirm(
                env,
                yes,
                "Store the current settings in the device's own memory?",
            )?;
            let s = connect(env)?;
            s.conn
                .send(Frame {
                    group: 0x1a,
                    param: 0x03,
                    value: 0,
                })
                .map_err(device)?;
            finish(s.conn, s.events)
        }
        Command::Init { yes } => init(env, yes),
    }
}

/// A connected device at the saved state.
struct Session {
    conn: Connection,
    events: Receiver<Event>,
    state: State,
    store: Store,
}

fn device(e: impl fmt::Display) -> Failure {
    Failure::Device(e.to_string())
}

fn open(env: &mut Env) -> Result<(Store, Connection, Receiver<Event>), Failure> {
    let store = env.store.as_ref().map_err(device)?.clone();
    let (conn, events) = (env.open)().map_err(Failure::Device)?;
    Ok((store, conn, events))
}

/// Opens the device and brings it to the saved state.
fn connect(env: &mut Env) -> Result<Session, Failure> {
    let (store, conn, events) = open(env)?;
    match sync::connect(&conn, &events, &store) {
        Ok(state) => Ok(Session {
            conn,
            events,
            state,
            store,
        }),
        Err(SyncError::NeedsInitialState(_)) => Err(Failure::Device(
            "no saved state yet: run `toppingctl init` first to push the default settings".into(),
        )),
        Err(e) => Err(device(e)),
    }
}

/// Flushes every queued frame, then reports whether the device failed.
fn finish(conn: Connection, events: Receiver<Event>) -> Result<(), Failure> {
    drop(conn);
    for event in events.try_iter() {
        if let Event::Disconnected(e) = event {
            return Err(Failure::Device(format!(
                "the TOPPING E2x2 disconnected: {e}"
            )));
        }
    }
    Ok(())
}

/// First run: pushes the defaults (with the device's settings) and saves them.
fn init(env: &mut Env, yes: bool) -> Result<(), Failure> {
    let (store, conn, events) = open(env)?;
    match sync::connect(&conn, &events, &store) {
        Ok(_) => {
            finish(conn, events)?;
            let _ = writeln!(
                env.stderr,
                "toppingctl: already set up (saved state in {}); nothing to do",
                store.path().display()
            );
            Ok(())
        }
        Err(SyncError::NeedsInitialState(state)) => {
            confirm(
                env,
                yes,
                "No saved state. Push the default settings to the device?",
            )?;
            for f in state.frames().into_iter().filter(|f| f.group != 0x10) {
                conn.send(f).map_err(device)?;
            }
            finish(conn, events)?;
            store.save(&state).map_err(device)
        }
        Err(e) => Err(device(e)),
    }
}

/// Asks `question` with `[y/N]` unless `yes`; refused without a terminal.
fn confirm(env: &mut Env, yes: bool, question: &str) -> Result<(), Failure> {
    if yes {
        return Ok(());
    }
    if !env.stdin_is_tty {
        return Err(Failure::Usage(format!(
            "{question} Not confirmed: stdin is not a terminal; pass --yes"
        )));
    }
    let _ = write!(env.stderr, "{question} [y/N] ");
    let _ = env.stderr.flush();
    let mut answer = String::new();
    let _ = env.stdin.read_line(&mut answer);
    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => Err(Failure::Usage("not confirmed; nothing was changed".into())),
    }
}

/// The 48V register of input `ch`.
fn phantom_frame(ch: usize, on: bool) -> Frame {
    Frame {
        group: 0x21 + ch as u8,
        param: 0x02,
        value: i32::from(on),
    }
}
