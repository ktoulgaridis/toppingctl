//! The README (#21): its sections exist, and its keybinding table matches
//! the TUI's help modal row for row.

use std::fs;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use toppingctl::state::{Meters, State};
use toppingctl::tui::app::{App, Modal};
use toppingctl::tui::theme::Theme;
use toppingctl::tui::ui;

fn readme() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    fs::read_to_string(path).expect("README.md must exist")
}

/// Whitespace collapsed to single spaces, backticks dropped.
fn normalize(s: &str) -> String {
    s.replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The rows of the first table under the `## Keys` heading, each as
/// "key action", normalized; blank rows dropped.
fn readme_keys(text: &str) -> Vec<String> {
    let section = text
        .split("\n## ")
        .find(|s| s.starts_with("Keys"))
        .expect("README needs a `## Keys` section");
    let rows: Vec<&str> = section
        .lines()
        .skip_while(|l| !l.trim_start().starts_with('|'))
        .take_while(|l| l.trim_start().starts_with('|'))
        .skip(2) // header and separator
        .collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<&str> = row.trim().trim_matches('|').split('|').collect();
            assert_eq!(cells.len(), 2, "a Keys row has two cells: {row}");
            normalize(&format!("{} {}", cells[0], cells[1]))
        })
        .filter(|r| !r.is_empty())
        .collect()
}

/// The help modal's rows, rendered 80 columns wide (the box spans the
/// screen, so nothing behind it shows): the rows between the title and the
/// close hint, border columns dropped, normalized; blank rows dropped.
fn help_rows() -> Vec<String> {
    let mut app = App::new(Theme::color());
    app.synced(State::default());
    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert_eq!(app.modal(), Some(Modal::Help));
    let (w, h) = (80, 60);
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|f| {
            ui::draw(f, &app, &Meters::default());
        })
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let lines: Vec<String> = (0..h)
        .map(|y| (1..w - 1).map(|x| buf[(x, y)].symbol()).collect())
        .collect();
    let top = lines
        .iter()
        .position(|l| l.contains("Help"))
        .expect("the help modal's title");
    let bottom = lines
        .iter()
        .position(|l| l.contains("[Esc] close"))
        .expect("the help modal's close hint");
    lines[top + 1..bottom]
        .iter()
        .map(|l| normalize(l))
        .filter(|l| !l.is_empty())
        .collect()
}

#[test]
fn keys_table_matches_help_modal() {
    let help = help_rows();
    assert!(help.len() > 10, "help rows: {help:#?}");
    assert_eq!(readme_keys(&readme()), help);
}

#[test]
fn readme_has_every_section() {
    let text = readme();
    let headings: Vec<String> = text
        .lines()
        .filter(|l| l.starts_with("## "))
        .map(|l| l[3..].trim().to_string())
        .collect();
    for want in [
        "Install",
        "Linux setup",
        "Pixel mode and the text fallback",
        "Keys",
        "Mouse",
        "Workspaces",
        "Store to the device",
        "Command line",
        "48V phantom power",
        "Sample rate",
        "The official TOPPING app",
        "Known limits",
        "License",
    ] {
        assert!(
            headings.iter().any(|h| h == want),
            "README needs a `## {want}` section; it has {headings:?}"
        );
    }
}

#[test]
fn readme_mentions_what_the_spec_asks_for() {
    let text = readme();
    for want in [
        "cargo install",
        "cargo build",
        "#31",
        "#22",
        "packaging/linux/README.md",
        "'Mic',0",
        "TOPPINGCTL_PIXELS=0",
        "kitty",
        "Ghostty",
        "PipeWire",
        "Screenshot",
        "toppingctl set",
        "toppingctl get",
        "toppingctl dump",
        "toppingctl store",
        "toppingctl init",
        "analog",
        "CoreAudio",
    ] {
        assert!(text.contains(want), "README should mention {want:?}");
    }
}
