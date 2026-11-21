//! User-facing error messages for opening the device.

use crate::device::{SelectError, PRODUCT_ID, VENDOR_ID};
use hidapi::HidError;
use std::io;

/// The issue tracking the Linux udev rule for non-root hidraw access.
pub const UDEV_RULE_ISSUE: &str = "https://github.com/ktoulgaridis/toppingctl/issues/19";

/// Operating system, for messages whose advice differs per platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    Other,
}

impl Platform {
    /// The platform this binary was built for.
    pub fn current() -> Self {
        if cfg!(target_os = "linux") {
            Platform::Linux
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Other
        }
    }
}

/// Why the E2x2 could not be opened, in terms a user can act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenFailure {
    /// The device is not connected (or vanished before it could be opened).
    DeviceMissing,
    /// Several candidate entries and no way to tell which is the HID interface.
    Ambiguous { matches: usize },
    /// The OS refused access to the device node.
    PermissionDenied { detail: String },
    /// Anything else; `detail` is the underlying error text.
    Other { detail: String },
}

impl From<SelectError> for OpenFailure {
    fn from(err: SelectError) -> Self {
        match err {
            SelectError::NotFound => OpenFailure::DeviceMissing,
            SelectError::Ambiguous { matches } => OpenFailure::Ambiguous { matches },
        }
    }
}

/// Classifies a hidapi error from opening the device.
///
/// Backends flatten OS errors into message strings (hidraw on Linux, IOKit
/// on macOS), so besides `io::ErrorKind` this matches on their wording.
pub fn classify(err: &HidError) -> OpenFailure {
    let detail = err.to_string();
    if let HidError::IoError { error } = err {
        match error.kind() {
            io::ErrorKind::PermissionDenied => return OpenFailure::PermissionDenied { detail },
            io::ErrorKind::NotFound => return OpenFailure::DeviceMissing,
            _ => {}
        }
    }
    let text = detail.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
    if has(&["permission denied", "os error 13", "not permitted"]) {
        OpenFailure::PermissionDenied { detail }
    } else if has(&["no such file or directory", "os error 2)", "no such device"]) {
        OpenFailure::DeviceMissing
    } else {
        OpenFailure::Other { detail }
    }
}

/// The message to show the user for `failure` on `platform`.
pub fn message(failure: &OpenFailure, platform: Platform) -> String {
    match failure {
        OpenFailure::DeviceMissing => format!(
            "TOPPING E2x2 (USB {VENDOR_ID:04x}:{PRODUCT_ID:04x}) not found: \
             check that it is connected and powered on"
        ),
        OpenFailure::Ambiguous { matches } => format!(
            "found {matches} TOPPING E2x2 HID entries and cannot tell which is the \
             control interface: connect only one E2x2"
        ),
        OpenFailure::PermissionDenied { detail } if platform == Platform::Linux => format!(
            "permission denied opening the TOPPING E2x2 HID device ({detail}).\n\
             Non-root access needs the udev rule tracked in {UDEV_RULE_ISSUE}; \
             until it is installed, run hidsniff with sudo."
        ),
        OpenFailure::PermissionDenied { detail } => {
            format!("permission denied opening the TOPPING E2x2 HID device ({detail})")
        }
        OpenFailure::Other { detail } => {
            format!("could not open the TOPPING E2x2 HID device: {detail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let err = api_error(
            "failed to open device with path /dev/hidraw3: Permission denied (os error 13)",
        );
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
}
