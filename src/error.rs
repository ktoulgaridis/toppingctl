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

/// Whether a read failed only because a signal (such as Ctrl-C) interrupted it.
pub fn is_interrupted(err: &HidError) -> bool {
    matches!(err, HidError::IoError { error } if error.kind() == io::ErrorKind::Interrupted)
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
