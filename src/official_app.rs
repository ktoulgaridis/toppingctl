//! Is the official TOPPING app running? Both programs talk to the E2x2 over
//! the same HID interface and may fight over it, so toppingctl warns (a
//! notice in the TUI, a line on stderr from the CLI). It never blocks.
//!
//! The process list comes through [`Processes`], so tests can mock it:
//! [`System`] reads `ps -axo comm=` on macOS and `/proc/*/comm` on Linux.

/// The official app's process names.
pub const NAMES: [&str; 2] = ["TOPPING Professional Control Center", "ToppingPro"];

/// What toppingctl says when the official app is running.
pub const WARNING: &str = "the official TOPPING app is running; both may fight over the device";

/// Linux keeps the first 15 bytes of a process name in `/proc/<pid>/comm`.
const COMM_LEN: usize = 15;

/// A list of the running processes' names (or paths).
pub trait Processes {
    /// The names; empty when the list can't be read.
    fn names(&self) -> Vec<String>;
}

/// The OS's process list.
pub struct System;

impl Processes for System {
    #[cfg(target_os = "linux")]
    fn names(&self) -> Vec<String> {
        use std::fs;
        let Ok(dir) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        dir.flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .bytes()
                    .all(|b| b.is_ascii_digit())
            })
            .filter_map(|e| fs::read_to_string(e.path().join("comm")).ok())
            .map(|s| s.trim_end_matches('\n').to_string())
            .collect()
    }

    #[cfg(not(target_os = "linux"))]
    fn names(&self) -> Vec<String> {
        use std::process::Command;
        let out = match Command::new("ps").args(["-axo", "comm="]).output() {
            Ok(out) if out.status.success() => out.stdout,
            _ => return Vec::new(),
        };
        let names: Vec<String> = String::from_utf8_lossy(&out)
            .lines()
            .map(|l| l.trim().to_string())
            .collect();
        names
    }
}

/// Whether one process name (or executable path) is the official app's.
fn is_official(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name).trim();
    NAMES.iter().any(|n| {
        base.eq_ignore_ascii_case(n)
            || (base.len() == COMM_LEN
                && n.len() > COMM_LEN
                && n.as_bytes()[..COMM_LEN].eq_ignore_ascii_case(base.as_bytes()))
    })
}

/// [`WARNING`] when the official app is in `processes`, else `None`.
pub fn warning(processes: &dyn Processes) -> Option<&'static str> {
    processes
        .names()
        .iter()
        .any(|n| is_official(n))
        .then_some(WARNING)
}
