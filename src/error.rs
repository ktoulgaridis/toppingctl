//! User-facing error messages for opening the device.

use crate::device::{DiscoverError, SelectError, PRODUCT_ID, VENDOR_ID};
use crate::model::Model;
use hidapi::HidError;
use std::io;

/// The Linux udev rule that grants the seat user non-root hidraw access.
pub const UDEV_RULE: &str = "70-toppingctl.rules";

/// The install guide for [`UDEV_RULE`], relative to the repository root.
pub const UDEV_README: &str = "packaging/linux/README.md";

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
    /// An experimental model (E4x4 Pre, E8x8 Pre) is attached and
    /// `--experimental` was not given. Nothing was opened or sent.
    Experimental { model: Model },
    /// An experimental model is attached and `--experimental` was given,
    /// but its register map is not implemented yet (#74). Nothing was sent.
    NotImplemented { model: Model },
    /// The model's firmware is older than [`Model::min_firmware`], or its
    /// version (`release`, bcdDevice) could not be read (0).
    Firmware { model: Model, release: u16 },
    /// Like [`OpenFailure::Ambiguous`], for a model other than the E2x2.
    AmbiguousModel { model: Model, matches: usize },
}

impl OpenFailure {
    /// Whether this is the user's to fix with a flag (CLI exit 2) rather
    /// than a device error (exit 1): an experimental model without
    /// `--experimental`.
    pub fn is_usage(&self) -> bool {
        matches!(self, OpenFailure::Experimental { .. })
    }
}

impl From<SelectError> for OpenFailure {
    fn from(err: SelectError) -> Self {
        match err {
            SelectError::NotFound => OpenFailure::DeviceMissing,
            SelectError::Ambiguous { matches } => OpenFailure::Ambiguous { matches },
        }
    }
}

impl From<DiscoverError> for OpenFailure {
    fn from(err: DiscoverError) -> Self {
        match err {
            DiscoverError::NotFound => OpenFailure::DeviceMissing,
            DiscoverError::Ambiguous {
                model: Model::E2x2,
                matches,
            } => OpenFailure::Ambiguous { matches },
            DiscoverError::Ambiguous { model, matches } => {
                OpenFailure::AmbiguousModel { model, matches }
            }
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
             Non-root access needs the udev rule {UDEV_RULE}; \
             see {UDEV_README} to install it, then replug the device."
        ),
        OpenFailure::PermissionDenied { detail } => {
            format!("permission denied opening the TOPPING E2x2 HID device ({detail})")
        }
        OpenFailure::Other { detail } => {
            format!("could not open the TOPPING E2x2 HID device: {detail}")
        }
        OpenFailure::Experimental { model } => format!(
            "found TOPPING {} (USB {VENDOR_ID:04x}:{:04x}), which is experimental and \
             untested: nothing was sent. Opt in with --experimental or \
             TOPPINGCTL_EXPERIMENTAL=1",
            model.name(),
            model.product_id()
        ),
        OpenFailure::NotImplemented { model } => format!(
            "found TOPPING {} (experimental): control not implemented yet; nothing was sent",
            model.name()
        ),
        OpenFailure::Firmware { model, release } => {
            let floor = model
                .min_firmware()
                .map_or(String::new(), Model::firmware_text);
            let name = model.name();
            if *release == 0 {
                format!(
                    "found TOPPING {name} but could not read its firmware version \
                     (toppingctl needs {floor} or later): nothing was sent"
                )
            } else {
                format!(
                    "TOPPING {name} firmware {} is too old: toppingctl needs {floor} or \
                     later (update it with the official TOPPING app); nothing was sent",
                    Model::firmware_text(*release)
                )
            }
        }
        OpenFailure::AmbiguousModel { model, matches } => format!(
            "found {matches} TOPPING {name} HID interfaces and cannot tell which is the \
             control interface: nothing was sent",
            name = model.name()
        ),
    }
}
