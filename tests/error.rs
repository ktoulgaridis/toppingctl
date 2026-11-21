//! Unit tests for `toppingctl::error`.

use hidapi::HidError;
use std::io;
use toppingctl::device::SelectError;
use toppingctl::error::*;

fn api_error(message: &str) -> HidError {
    HidError::HidApiError {
        message: message.to_string(),
    }
}

#[test]
fn maps_select_errors() {
    assert_eq!(
        OpenFailure::from(SelectError::NotFound),
        OpenFailure::DeviceMissing
    );
    assert_eq!(
        OpenFailure::from(SelectError::Ambiguous { matches: 3 }),
        OpenFailure::Ambiguous { matches: 3 }
    );
}

#[test]
fn classifies_linux_hidraw_permission_denied() {
    // What hidapi's linux-native backend returns when /dev/hidrawN is root-only.
    let err =
        api_error("failed to open device with path /dev/hidraw3: Permission denied (os error 13)");
    assert!(matches!(
        classify(&err),
        OpenFailure::PermissionDenied { detail } if detail.contains("/dev/hidraw3")
    ));
}

#[test]
fn classifies_io_permission_denied() {
    let err = HidError::IoError {
        error: io::Error::from(io::ErrorKind::PermissionDenied),
    };
    assert!(matches!(
        classify(&err),
        OpenFailure::PermissionDenied { .. }
    ));
}

#[test]
fn classifies_macos_not_permitted() {
    let err = api_error(
        "hid_open_path: failed to open IOHIDDevice from mach entry: (0xE00002E2) (iokit/common) not permitted",
    );
    assert!(matches!(
        classify(&err),
        OpenFailure::PermissionDenied { .. }
    ));
}

#[test]
fn classifies_vanished_device_as_missing() {
    let err = api_error(
        "failed to open device with path /dev/hidraw3: No such file or directory (os error 2)",
    );
    assert_eq!(classify(&err), OpenFailure::DeviceMissing);
    let err = HidError::IoError {
        error: io::Error::from(io::ErrorKind::NotFound),
    };
    assert_eq!(classify(&err), OpenFailure::DeviceMissing);
}

#[test]
fn other_errors_keep_their_text() {
    let err = api_error("failed to get report descriptor size: Broken pipe");
    assert_eq!(
        classify(&err),
        OpenFailure::Other {
            detail: err.to_string()
        }
    );
}

#[test]
fn missing_device_message_names_the_device() {
    for platform in [Platform::Linux, Platform::MacOs, Platform::Other] {
        let msg = message(&OpenFailure::DeviceMissing, platform);
        assert!(msg.contains("E2x2"), "{msg}");
        assert!(msg.contains("152a:8752"), "{msg}");
        assert!(msg.contains("not found"), "{msg}");
    }
}

#[test]
fn linux_permission_message_points_at_udev_rule_and_sudo() {
    let failure = OpenFailure::PermissionDenied {
        detail: "Permission denied (os error 13)".into(),
    };
    let msg = message(&failure, Platform::Linux);
    assert!(msg.contains("permission denied"), "{msg}");
    assert!(msg.contains("udev rule"), "{msg}");
    assert!(msg.contains(UDEV_RULE_ISSUE), "{msg}");
    assert!(msg.contains("sudo"), "{msg}");
    assert!(msg.contains("os error 13"), "{msg}");
}

#[test]
fn non_linux_permission_message_does_not_mention_udev() {
    let failure = OpenFailure::PermissionDenied {
        detail: "not permitted".into(),
    };
    for platform in [Platform::MacOs, Platform::Other] {
        let msg = message(&failure, platform);
        assert!(msg.contains("permission denied"), "{msg}");
        assert!(msg.contains("not permitted"), "{msg}");
        assert!(!msg.contains("udev"), "{msg}");
    }
}

#[test]
fn ambiguous_and_other_messages_carry_their_details() {
    let msg = message(&OpenFailure::Ambiguous { matches: 2 }, Platform::MacOs);
    assert!(msg.contains('2'), "{msg}");
    assert!(msg.contains("only one"), "{msg}");
    let msg = message(
        &OpenFailure::Other {
            detail: "Broken pipe".into(),
        },
        Platform::Linux,
    );
    assert!(msg.contains("Broken pipe"), "{msg}");
}

#[test]
fn interrupted_reads_are_recognised() {
    // Ctrl-C interrupts hidraw's poll() with EINTR; that must not end the capture as a failure.
    let err = HidError::IoError {
        error: io::Error::from(io::ErrorKind::Interrupted),
    };
    assert!(is_interrupted(&err));
    assert!(!is_interrupted(&HidError::IoError {
        error: io::Error::from(io::ErrorKind::BrokenPipe),
    }));
    assert!(!is_interrupted(&api_error(
        "unexpected poll error (device disconnected)"
    )));
}
