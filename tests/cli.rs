use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use toppingctl::cli::{self, Env, Path, Value};
use toppingctl::conn::fake::{FakeHandle, FakeTransport};
use toppingctl::conn::Connection;
use toppingctl::frame::{Crc, Dir, Frame};
use toppingctl::state::{Source, State};
use toppingctl::sync::Store;

fn frame(group: u8, param: u8, value: i32) -> Frame {
    Frame {
        group,
        param,
        value,
    }
}

/// A fresh directory under the system temp dir, unique to this test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("toppingctl-cli-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Queues the device's reply to the `10 06` query, with brightness 0.
fn push_query_reply(fake: &FakeHandle) {
    for f in [
        frame(0x10, 0x06, 1),
        frame(0x10, 0x0b, 0),
        frame(0x10, 0x04, 0x0001_000d),
        frame(0x11, 0x01, 0x49),
        frame(0x10, 0x0a, 0),
        frame(0x10, 0x03, 0x0001_000e),
        frame(0x10, 0x0c, 0),
    ] {
        fake.push_report(&f.encode(Crc::Fill));
    }
}

fn decode_all(written: &[[u8; 16]]) -> Vec<Frame> {
    written
        .iter()
        .map(|r| Frame::decode(r, Dir::ToDevice).unwrap())
        .collect()
}

/// What one CLI run did.
struct Run {
    code: u8,
    out: String,
    err: String,
    /// Every frame written to the device, in order.
    written: Vec<Frame>,
    /// Whether the CLI opened the device.
    opened: bool,
}

/// Runs the CLI with `args` against a fake device that answers the query.
fn run(args: &[&str], store: &Store, stdin_is_tty: bool, stdin: &str) -> Run {
    let (transport, fake) = FakeTransport::new();
    push_query_reply(&fake);
    let mut transport = Some(transport);
    let opened = std::cell::Cell::new(false);
    let mut input = Cursor::new(stdin.as_bytes().to_vec());
    let mut out = Vec::new();
    let mut err = Vec::new();
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let code = {
        let mut env = Env {
            store: Ok(store.clone()),
            stdin_is_tty,
            stdin: &mut input,
            stdout: &mut out,
            stderr: &mut err,
            open: Box::new(|| {
                opened.set(true);
                let t = transport.take().expect("opened once");
                Ok(Connection::spawn(t))
            }),
        };
        cli::run(&args, &mut env)
    };
    Run {
        code,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
        written: decode_all(&fake.written()),
        opened: opened.get(),
    }
}

/// A saved state and the state `connect` leaves the device in: the saved
/// one with the device's settings (the query reply) applied.
fn saved_and_connected() -> (State, State) {
    let mut saved = State::default();
    saved.inputs[0].gain_db = 7;
    saved.mixes[1].pb34.set_balance(0.25);
    saved.output.set_level(-3.0);
    saved.system.brightness = 2;
    let mut connected = saved.clone();
    connected.system.brightness = 0;
    (saved, connected)
}

/// How many frames `connect` writes for `saved`: the query and the push.
fn connect_len(saved: &State) -> usize {
    1 + saved.frames().iter().filter(|f| f.group != 0x10).count()
}

/// Every path the refinement names, built independently of the CLI.
fn all_names() -> Vec<String> {
    let mut names = Vec::new();
    for ch in ["in1", "in2"] {
        for f in ["gain", "mute", "solo", "phase", "mon", "inst", "48v"] {
            names.push(format!("{ch}.{f}"));
        }
    }
    for m in ["a", "b", "c"] {
        for s in ["in1", "in2", "pb12", "pb34", "pb56"] {
            for f in ["fader", "pan", "mute", "solo", "phase"] {
                names.push(format!("mix.{m}.{s}.{f}"));
            }
        }
        names.push(format!("mix.{m}.link"));
    }
    for n in ["1", "2"] {
        for f in ["source", "level", "mute"] {
            names.push(format!("loopback.{n}.{f}"));
        }
    }
    for f in ["source", "level", "hp", "line"] {
        names.push(format!("out.{f}"));
    }
    names.push("phone_gain".into());
    for f in ["brightness", "standby", "mobile"] {
        names.push(format!("system.{f}"));
    }
    names
}

/// `state` after `path value` is applied.
fn set(state: &State, path: &str, value: &str) -> State {
    let path = Path::parse(path).unwrap();
    let value = path.parse_value(value).unwrap();
    let mut new = state.clone();
    path.set(&mut new, value);
    new
}

#[test]
fn parse_paths_and_values() {
    // Every path parses, and prints back as itself.
    let names = all_names();
    assert_eq!(names.len(), 106);
    for name in &names {
        let path = Path::parse(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(&path.to_string(), name);
    }
    let all: Vec<String> = Path::all().iter().map(|p| p.to_string()).collect();
    assert_eq!(all, names);

    // An unknown path's error lists the valid ones.
    let err = Path::parse("in3.gain").unwrap_err();
    assert!(err.contains("\"in3.gain\""), "{err}");
    for pattern in [
        "in1|in2.gain|mute|solo|phase|mon|inst|48v",
        "mix.a|b|c.in1|in2|pb12|pb34|pb56.fader|pan|mute|solo|phase",
        "mix.a|b|c.link",
        "loopback.1|2.source|level|mute",
        "out.source|level|hp|line",
        "phone_gain",
        "system.brightness|standby|mobile",
    ] {
        assert!(err.contains(pattern), "missing {pattern}: {err}");
    }

    // Switches.
    let mon = Path::parse("in1.mon").unwrap();
    for (s, on) in [
        ("on", true),
        ("true", true),
        ("1", true),
        ("off", false),
        ("false", false),
        ("0", false),
    ] {
        assert_eq!(mon.parse_value(s), Ok(Value::Bool(on)), "{s}");
    }
    let err = mon.parse_value("maybe").unwrap_err();
    assert!(err.contains("on, off, true, false, 1, 0"), "{err}");

    // dB: plain, with a unit, -inf; out of range is rejected.
    let fader = Path::parse("mix.a.in1.fader").unwrap();
    assert_eq!(fader.parse_value("-6"), Ok(Value::Db(-6.0)));
    assert_eq!(fader.parse_value("-6dB"), Ok(Value::Db(-6.0)));
    assert_eq!(fader.parse_value("+3.5db"), Ok(Value::Db(3.5)));
    assert_eq!(fader.parse_value("-inf"), Ok(Value::Db(f64::NEG_INFINITY)));
    let err = fader.parse_value("7").unwrap_err();
    assert!(err.contains("-inf .. +6 dB"), "{err}");
    let level = Path::parse("out.level").unwrap();
    assert_eq!(level.parse_value("-6"), Ok(Value::Db(-6.0)));
    let err = level.parse_value("1").unwrap_err();
    assert!(err.contains("-inf .. 0 dB"), "{err}");
    assert!(level.parse_value("loud").is_err());
    assert!(level.parse_value("nan").is_err());

    // Input gain: whole dB 0 ..= 20.
    let gain = Path::parse("in2.gain").unwrap();
    assert_eq!(gain.parse_value("12"), Ok(Value::Gain(12)));
    assert_eq!(gain.parse_value("12dB"), Ok(Value::Gain(12)));
    let err = gain.parse_value("21").unwrap_err();
    assert!(err.contains("0 .. 20"), "{err}");
    assert!(gain.parse_value("1.5").is_err());

    // Pan: L/C/R or 0 ..= 1.
    let pan = Path::parse("mix.b.pb34.pan").unwrap();
    assert_eq!(pan.parse_value("C"), Ok(Value::Pan(0.5)));
    assert_eq!(pan.parse_value("L100"), Ok(Value::Pan(0.0)));
    assert_eq!(pan.parse_value("L40"), Ok(Value::Pan(0.3)));
    assert_eq!(pan.parse_value("R25"), Ok(Value::Pan(0.625)));
    assert_eq!(pan.parse_value("r100"), Ok(Value::Pan(1.0)));
    assert_eq!(pan.parse_value("0.75"), Ok(Value::Pan(0.75)));
    for bad in ["L101", "1.5", "X", "L"] {
        let err = pan.parse_value(bad).unwrap_err();
        assert!(err.contains("L100 .. C .. R100"), "{bad}: {err}");
    }

    // Sources.
    let source = Path::parse("loopback.2.source").unwrap();
    for (s, src) in [
        ("mixa", Source::MixA),
        ("mixb", Source::MixB),
        ("mixc", Source::MixC),
        ("pb12", Source::Pb12),
        ("pb34", Source::Pb34),
        ("pb56", Source::Pb56),
        ("in1", Source::In1),
        ("in2", Source::In2),
        ("in1+2", Source::In12),
    ] {
        assert_eq!(source.parse_value(s), Ok(Value::Source(src)), "{s}");
    }
    let err = source.parse_value("mixd").unwrap_err();
    assert!(
        err.contains("mixa mixb mixc pb12 pb34 pb56 in1 in2 in1+2"),
        "{err}"
    );

    // Brightness.
    let brightness = Path::parse("system.brightness").unwrap();
    assert_eq!(brightness.parse_value("2"), Ok(Value::Level(2)));
    assert!(brightness.parse_value("3").unwrap_err().contains("0, 1, 2"));

    // Values land in the right fields.
    let s = State::default();
    assert!(set(&s, "in2.inst", "on").inputs[1].inst);
    assert_eq!(set(&s, "in1.gain", "9").inputs[0].gain_db, 9);
    assert_eq!(
        set(&s, "mix.c.pb56.fader", "-12").mixes[2].pb56.fader_db,
        -12.0
    );
    assert!(set(&s, "mix.a.in2.phase", "1").mixes[0].in2.phase);
    assert!(set(&s, "mix.b.link", "on").mixes[1].link);
    assert_eq!(
        set(&s, "loopback.1.source", "mixc").loopbacks[0].source,
        Source::MixC
    );
    let lb = &set(&s, "loopback.2.level", "-inf").loopbacks[1];
    assert_eq!(
        (lb.level_l_db, lb.level_r_db),
        (f64::NEG_INFINITY, f64::NEG_INFINITY),
        "level sets both channels"
    );
    let out = set(&s, "out.level", "-6").output;
    assert_eq!((out.level_l_db, out.level_r_db), (-6.0, -6.0));
    let pb = &set(&s, "mix.b.pb34.pan", "L50").mixes[1].pb34;
    assert_eq!((pb.pan_l, pb.pan_r), (0.0, 0.5), "a PB pan is a balance");
    assert_eq!(set(&s, "out.source", "in1+2").output.source, Source::In12);
    assert!(!set(&s, "out.hp", "off").output.headphones);
    assert!(!set(&s, "out.line", "off").output.line);
    assert!(!set(&s, "phone_gain", "off").phone_gain);
    assert!(set(&s, "system.standby", "on").system.auto_standby);
    assert!(set(&s, "system.mobile", "on").system.mobile_apps);
    assert_eq!(set(&s, "system.brightness", "0").system.brightness, 0);

    // `get` prints values in a form `set` accepts.
    let s = set(&s, "mix.b.pb34.pan", "L50");
    for (path, shown) in [
        ("mix.b.pb34.pan", "L50"),
        ("mix.a.in1.pan", "L100"),
        ("out.level", "0dB"),
        ("loopback.1.source", "pb34"),
        ("in1.mon", "off"),
        ("phone_gain", "on"),
        ("in1.48v", "unknown"),
        ("system.brightness", "1"),
    ] {
        assert_eq!(Path::parse(path).unwrap().get(&s), shown, "{path}");
    }
    let s = set(&s, "mix.a.in1.fader", "-inf");
    assert_eq!(Path::parse("mix.a.in1.fader").unwrap().get(&s), "-inf");

    // Usage errors exit 2 without touching the device.
    let dir = temp_dir("usage");
    let store = Store::at(dir.join("state.json"));
    for args in [
        &["set", "in3.gain", "1"][..],
        &["set", "in1.gain", "loud"],
        &["set", "in1.gain"],
        &["frobnicate"],
        &["dump", "--yaml"],
    ] {
        let r = run(args, &store, false, "");
        assert_eq!(r.code, 2, "{args:?}");
        assert!(!r.opened, "{args:?} opened the device");
        assert!(!r.err.is_empty(), "{args:?}");
    }
    let r = run(&["--help"], &store, false, "");
    assert_eq!(r.code, 0);
    assert!(r.out.contains("Usage: toppingctl"));
    // --help lists the same paths as the unknown-path error.
    let unknown = Path::parse("in3.gain").unwrap_err();
    let listed = unknown.split_once("valid paths:\n").unwrap().1;
    assert!(listed.starts_with("  in1|"), "{listed}");
    assert!(r.out.contains(listed), "{}", r.out);
    assert!(!r.opened);
    let r = run(&["--version"], &store, false, "");
    assert_eq!(r.code, 0);
    assert!(r.out.starts_with("toppingctl "));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn set_sends_diff() {
    let dir = temp_dir("set");
    let store = Store::at(dir.join("state.json"));
    let (_, connected) = saved_and_connected();

    let mut before = connected;
    for (path, value) in [
        ("in1.mon", "on"),
        ("mix.b.pb34.pan", "L100"),
        ("out.level", "-6"),
        ("out.hp", "off"),
    ] {
        store.save(&saved_from(&before)).unwrap();
        let saved_now = store.load().unwrap().unwrap();
        let r = run(&["set", path, value], &store, false, "");
        assert_eq!(r.code, 0, "{path}: {}", r.err);
        let after = set(&before, path, value);
        let expected = before.diff(&after);
        assert!(!expected.is_empty(), "{path} changes something");
        assert_eq!(r.written[0], frame(0x10, 0x06, 1));
        assert_eq!(
            &r.written[connect_len(&saved_now)..],
            &expected[..],
            "{path}"
        );
        // The new state is saved.
        assert_eq!(store.load().unwrap().unwrap(), saved_from(&after), "{path}");
        before = after;
    }
    let _ = fs::remove_dir_all(&dir);
}

/// `state` as the store keeps it (no 48V, no readout).
fn saved_from(state: &State) -> State {
    let mut s = state.clone();
    s.readout = Default::default();
    for input in &mut s.inputs {
        input.phantom = None;
    }
    s
}

#[test]
fn phantom_needs_yes() {
    let dir = temp_dir("phantom");
    let store = Store::at(dir.join("state.json"));
    let (saved, _) = saved_and_connected();
    store.save(&saved).unwrap();
    let file = fs::read(store.path()).unwrap();

    // No TTY, no --yes: refused before the device is opened.
    let r = run(&["set", "in2.48v", "on"], &store, false, "y\n");
    assert_eq!(r.code, 2);
    assert!(!r.opened);
    assert!(r.written.is_empty());
    assert!(r.err.contains("--yes"), "{}", r.err);

    // A TTY and a "no": refused.
    let r = run(&["set", "in2.48v", "on"], &store, true, "n\n");
    assert_eq!(r.code, 2);
    assert!(r.written.is_empty());
    assert!(r.err.contains("[y/N]"), "{}", r.err);

    // --yes: sent through send_confirmed_phantom (`send` would refuse it).
    let push = connect_len(&saved);
    let r = run(&["set", "in2.48v", "on", "--yes"], &store, false, "");
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(&r.written[push..], &[frame(0x22, 0x02, 1)]);

    // A TTY and a "y": sent.
    let r = run(&["set", "in1.48v", "on"], &store, true, "y\n");
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(&r.written[push..], &[frame(0x21, 0x02, 1)]);

    // Off needs no confirmation.
    let r = run(&["set", "in1.48v", "off"], &store, false, "");
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(&r.written[push..], &[frame(0x21, 0x02, 0)]);

    // 48V is never saved.
    assert_eq!(fs::read(store.path()).unwrap(), file);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dump_and_get() {
    let dir = temp_dir("dump");
    let store = Store::at(dir.join("state.json"));
    let (saved, connected) = saved_and_connected();
    store.save(&saved).unwrap();

    let r = run(&["dump", "--json"], &store, false, "");
    assert_eq!(r.code, 0, "{}", r.err);
    let dumped: State = serde_json::from_str(&r.out).unwrap();
    assert_eq!(dumped, saved_from(&connected));

    let r = run(&["dump"], &store, false, "");
    assert_eq!(r.code, 0, "{}", r.err);
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(lines.len(), 106);
    assert!(lines.contains(&"in1.gain 7dB"), "{}", r.out);
    assert!(lines.contains(&"out.level -3dB"), "{}", r.out);
    assert!(lines.contains(&"system.brightness 0"), "{}", r.out);

    let r = run(&["get", "mix.b.pb34.pan"], &store, false, "");
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "L50\n");
    let r = run(&["get", "in1.gain"], &store, false, "");
    assert_eq!(r.out, "7dB\n");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn store_and_first_run() {
    let dir = temp_dir("store");
    let store = Store::at(dir.join("state.json"));
    let (saved, _) = saved_and_connected();
    store.save(&saved).unwrap();
    let push = connect_len(&saved);

    let r = run(&["store", "--yes"], &store, false, "");
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(&r.written[push..], &[frame(0x1a, 0x03, 0)]);

    // Not confirmed: nothing sent.
    let r = run(&["store"], &store, false, "");
    assert_eq!(r.code, 2);
    assert!(!r.opened);
    let r = run(&["store"], &store, true, "\n");
    assert_eq!(r.code, 2);
    assert!(r.written.is_empty());

    // First run: `set` fails with the init hint and writes nothing.
    let empty_dir = dir.join("first");
    let empty = Store::at(empty_dir.join("state.json"));
    let r = run(&["set", "in1.mon", "on"], &empty, false, "");
    assert_eq!(r.code, 1);
    assert!(r.err.contains("toppingctl init"), "{}", r.err);
    assert_eq!(
        r.written,
        vec![frame(0x10, 0x06, 1)],
        "nothing after the query"
    );
    assert!(!empty_dir.exists(), "no file written");

    // `init` without confirmation pushes nothing.
    let r = run(&["init"], &empty, false, "");
    assert_eq!(r.code, 2);
    assert_eq!(r.written, vec![frame(0x10, 0x06, 1)]);
    assert!(!empty_dir.exists());

    // `init --yes` pushes the defaults (with the device's settings) and saves.
    let r = run(&["init", "--yes"], &empty, false, "");
    assert_eq!(r.code, 0, "{}", r.err);
    let mut defaults = State::default();
    defaults.system.brightness = 0;
    let expected: Vec<Frame> = defaults
        .frames()
        .into_iter()
        .filter(|f| f.group != 0x10)
        .collect();
    assert_eq!(&r.written[1..], &expected[..]);
    assert_eq!(empty.load().unwrap().unwrap(), defaults);

    // Now `set` works.
    let r = run(&["set", "in1.mon", "on"], &empty, false, "");
    assert_eq!(r.code, 0, "{}", r.err);
    let _ = fs::remove_dir_all(&dir);
}
