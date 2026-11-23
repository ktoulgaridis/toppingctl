//! The official-app warning (#21): a mocked process list with the official
//! TOPPING app in it gives the warning; one without it gives none.

use toppingctl::official_app::{warning, Processes, System, WARNING};

/// A process list, as the OS would give it.
struct Fake(&'static [&'static str]);

impl Processes for Fake {
    fn names(&self) -> Vec<String> {
        self.0.iter().map(|s| s.to_string()).collect()
    }
}

#[test]
fn app_running_on_macos_warns() {
    // `ps -axo comm=` on macOS gives the executable's path.
    let ps = Fake(&[
        "/sbin/launchd",
        "/Applications/TOPPING Professional Control Center.app/Contents/MacOS/TOPPING Professional Control Center",
        "/usr/bin/zsh",
    ]);
    assert_eq!(warning(&ps), Some(WARNING));
}

#[test]
fn topping_pro_process_warns() {
    assert_eq!(warning(&Fake(&["bash", "ToppingPro"])), Some(WARNING));
    assert_eq!(
        warning(&Fake(&[
            "/Applications/ToppingPro.app/Contents/MacOS/ToppingPro"
        ])),
        Some(WARNING)
    );
}

#[test]
fn linux_comm_is_truncated_to_15_bytes() {
    // /proc/<pid>/comm keeps the first 15 bytes of the name (e.g. under Wine).
    assert_eq!(
        warning(&Fake(&["systemd", "TOPPING Profess"])),
        Some(WARNING)
    );
}

#[test]
fn no_app_no_warning() {
    assert_eq!(warning(&Fake(&[])), None);
    assert_eq!(
        warning(&Fake(&[
            "/sbin/launchd",
            "toppingctl",
            "/usr/local/bin/toppingctl",
            "pipewire",
            "TOPPING",
            "Topping",
        ])),
        None
    );
}

#[test]
fn warning_says_both_may_fight() {
    assert!(WARNING.contains("official"), "{WARNING}");
    assert!(WARNING.contains("fight over the device"), "{WARNING}");
}

#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn system_lists_processes() {
    // At least this test process is running.
    assert!(!System.names().is_empty());
}
