//! The experimental gate (#73): E4x4 Pre / E8x8 Pre are found but never
//! sent anything; without --experimental the CLI exits 2. No hardware.

use std::ffi::OsString;
use std::io::Cursor;

use toppingctl::cli::{self, split_experimental, Env, OpenError};
use toppingctl::device::Found;
use toppingctl::error::{message, OpenFailure, Platform};
use toppingctl::model::Model;
use toppingctl::open::admit;
use toppingctl::sync::Store;

fn found(model: Model, release_number: u16) -> Found {
    Found {
        model,
        index: 0,
        release_number,
    }
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

#[test]
fn e2x2_is_admitted_with_or_without_the_flag() {
    for experimental in [false, true] {
        assert_eq!(admit(&found(Model::E2x2, 0), experimental), Ok(()));
        // The E2x2 has no firmware floor, even when bcdDevice is unreadable.
        assert_eq!(admit(&found(Model::E2x2, 0x0001), experimental), Ok(()));
    }
}

#[test]
fn experimental_models_need_the_flag() {
    for model in [Model::E4x4Pre, Model::E8x8Pre] {
        assert_eq!(
            admit(&found(model, 0x0106), false),
            Err(OpenFailure::Experimental { model })
        );
    }
}

#[test]
fn with_the_flag_control_is_not_implemented_yet() {
    for model in [Model::E4x4Pre, Model::E8x8Pre] {
        assert_eq!(
            admit(&found(model, 0x0200), true),
            Err(OpenFailure::NotImplemented { model })
        );
    }
}

#[test]
fn e4x4_firmware_below_1_06_is_refused_with_or_without_the_flag() {
    for experimental in [false, true] {
        for release in [0x0105, 0x0100, 0] {
            assert_eq!(
                admit(&found(Model::E4x4Pre, release), experimental),
                Err(OpenFailure::Firmware {
                    model: Model::E4x4Pre,
                    release,
                }),
                "{release:#06x}"
            );
        }
        assert_eq!(
            admit(&found(Model::E4x4Pre, 0x0106), true),
            Err(OpenFailure::NotImplemented {
                model: Model::E4x4Pre
            })
        );
    }
}

#[test]
fn messages_say_found_experimental_and_nothing_sent() {
    let msg = message(
        &OpenFailure::Experimental {
            model: Model::E4x4Pre,
        },
        Platform::Linux,
    );
    for want in [
        "found TOPPING E4x4 Pre",
        "152a:8754",
        "experimental",
        "untested",
        "nothing was sent",
        "--experimental",
        "TOPPINGCTL_EXPERIMENTAL=1",
    ] {
        assert!(msg.contains(want), "missing {want:?}: {msg}");
    }

    let msg = message(
        &OpenFailure::NotImplemented {
            model: Model::E8x8Pre,
        },
        Platform::MacOs,
    );
    assert!(
        msg.contains("found TOPPING E8x8 Pre (experimental): control not implemented yet"),
        "{msg}"
    );
    assert!(msg.contains("nothing was sent"), "{msg}");

    let msg = message(
        &OpenFailure::Firmware {
            model: Model::E4x4Pre,
            release: 0x0105,
        },
        Platform::MacOs,
    );
    for want in ["E4x4 Pre", "1.05", "1.06", "nothing was sent"] {
        assert!(msg.contains(want), "missing {want:?}: {msg}");
    }
    let msg = message(
        &OpenFailure::Firmware {
            model: Model::E4x4Pre,
            release: 0,
        },
        Platform::MacOs,
    );
    assert!(msg.contains("could not read"), "{msg}");

    let msg = message(
        &OpenFailure::AmbiguousModel {
            model: Model::E8x8Pre,
            matches: 2,
        },
        Platform::MacOs,
    );
    assert!(msg.contains("E8x8 Pre") && msg.contains('2'), "{msg}");
}

#[test]
fn only_the_missing_flag_is_a_usage_error() {
    let m = Model::E4x4Pre;
    assert!(OpenFailure::Experimental { model: m }.is_usage());
    assert!(!OpenFailure::NotImplemented { model: m }.is_usage());
    assert!(!OpenFailure::Firmware {
        model: m,
        release: 0x0105
    }
    .is_usage());
    assert!(!OpenFailure::DeviceMissing.is_usage());
}

#[test]
fn the_flag_is_taken_out_of_the_arguments() {
    assert_eq!(split_experimental(os(&[]), None), (os(&[]), false));
    assert_eq!(
        split_experimental(os(&["--experimental"]), None),
        (os(&[]), true)
    );
    assert_eq!(
        split_experimental(os(&["get", "--experimental", "in1.gain"]), None),
        (os(&["get", "in1.gain"]), true)
    );
    assert_eq!(
        split_experimental(os(&["dump"]), None),
        (os(&["dump"]), false)
    );
}

#[test]
fn the_environment_variable_turns_it_on() {
    let on = |v: &str| split_experimental(os(&["dump"]), Some(OsString::from(v))).1;
    assert!(on("1"));
    assert!(!on("0"));
    assert!(!on(""));
    assert!(!on("yes"));
}

#[test]
fn usage_documents_the_flag_and_variable() {
    assert!(cli::USAGE.contains("--experimental"));
    assert!(cli::USAGE.contains("TOPPINGCTL_EXPERIMENTAL"));
}

/// Runs the CLI with `open` failing with `err`; returns (code, stderr).
fn run_refused(args: &[&str], err: OpenError) -> (u8, String) {
    let dir = std::env::temp_dir().join(format!(
        "toppingctl-experimental-{}-{}",
        std::process::id(),
        args.join("-")
    ));
    let store = Store::at(dir.join("state.json"));
    let mut input = Cursor::new(Vec::new());
    let mut out = Vec::new();
    let mut errout = Vec::new();
    let mut err = Some(err);
    let code = {
        let mut env = Env {
            store: Ok(store),
            stdin_is_tty: false,
            stdin: &mut input,
            stdout: &mut out,
            stderr: &mut errout,
            open: Box::new(|| Err(err.take().expect("opened once"))),
        };
        cli::run(&os(args), &mut env)
    };
    assert!(out.is_empty());
    (code, String::from_utf8(errout).unwrap())
}

#[test]
fn cli_exits_2_when_an_experimental_model_is_found_without_the_flag() {
    let msg = message(
        &OpenFailure::Experimental {
            model: Model::E4x4Pre,
        },
        Platform::Linux,
    );
    for args in [
        &["dump"][..],
        &["get", "in1.gain"],
        &["set", "in1.mute", "on"],
    ] {
        let (code, err) = run_refused(args, OpenError::Usage(msg.clone()));
        assert_eq!(code, 2, "{args:?}");
        assert!(err.contains("found TOPPING E4x4 Pre"), "{err}");
    }
}

#[test]
fn cli_exits_1_when_control_is_not_implemented() {
    let msg = message(
        &OpenFailure::NotImplemented {
            model: Model::E8x8Pre,
        },
        Platform::Linux,
    );
    let (code, err) = run_refused(&["dump"], OpenError::Device(msg));
    assert_eq!(code, 1);
    assert!(err.contains("control not implemented yet"), "{err}");
}

#[test]
fn open_failures_become_cli_open_errors() {
    let usage = OpenError::from_failure(
        &OpenFailure::Experimental {
            model: Model::E8x8Pre,
        },
        Platform::Linux,
    );
    assert!(matches!(usage, OpenError::Usage(m) if m.contains("E8x8 Pre")));
    let device = OpenError::from_failure(&OpenFailure::DeviceMissing, Platform::Linux);
    assert!(matches!(device, OpenError::Device(m) if m.contains("E2x2")));
}

/// The TUI's first frame for `app` at `w`×`h`, as text.
fn tui_text(app: &toppingctl::tui::app::App, w: u16, h: u16) -> String {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| {
            toppingctl::tui::ui::draw(f, app, &toppingctl::state::Meters::default());
        })
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn tui_splash_shows_the_refusal_instead_of_connecting() {
    use toppingctl::tui::app::App;
    use toppingctl::tui::theme::Theme;
    let mut app = App::new(Theme::plain());
    let why = message(
        &OpenFailure::Experimental {
            model: Model::E8x8Pre,
        },
        Platform::MacOs,
    );
    app.set_refused(Some(why));
    for width in [60, 120] {
        let text = tui_text(&app, width, 24);
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(!flat.contains("Connecting"), "{text}");
        for want in [
            "found TOPPING E8x8 Pre",
            "experimental",
            "nothing was sent",
            "--experimental",
            "press q to quit",
        ] {
            assert!(flat.contains(want), "missing {want:?} at {width}:\n{text}");
        }
    }
    app.set_refused(None);
    assert!(tui_text(&app, 120, 24).contains("Connecting"));
}

#[test]
fn tui_refusal_quits_on_q_esc_enter_and_ctrl_c_only() {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use toppingctl::tui::run::quits_refused;
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    for code in [KeyCode::Char('q'), KeyCode::Esc, KeyCode::Enter] {
        assert!(quits_refused(&k(code)), "{code:?}");
    }
    assert!(quits_refused(&KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL
    )));
    assert!(!quits_refused(&k(KeyCode::Char('c'))));
    assert!(!quits_refused(&k(KeyCode::Up)));
    let mut release = k(KeyCode::Char('q'));
    release.kind = KeyEventKind::Release;
    assert!(!quits_refused(&release));
}

#[test]
fn tui_shows_only_found_models_and_exits_2_only_without_the_flag() {
    use toppingctl::tui::run::Refused;
    let m = Model::E4x4Pre;
    assert!(Refused::of(OpenFailure::Experimental { model: m })
        .unwrap()
        .is_usage());
    assert!(!Refused::of(OpenFailure::NotImplemented { model: m })
        .unwrap()
        .is_usage());
    assert!(Refused::of(OpenFailure::Firmware {
        model: m,
        release: 0x0105
    })
    .is_ok());
    assert!(Refused::of(OpenFailure::DeviceMissing).is_err());
}
