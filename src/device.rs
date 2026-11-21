//! Locating the E2x2 among enumerated HID devices.
//!
//! Selection is a pure function over [`HidEntry`], a minimal mirror of the
//! `hidapi::DeviceInfo` fields it needs, so it can be tested without hardware.

/// USB vendor ID of the TOPPING E2x2.
pub const VENDOR_ID: u16 = 0x152a;
/// USB product ID of the TOPPING E2x2.
pub const PRODUCT_ID: u16 = 0x8752;
/// USB interface number of the E2x2's HID control interface.
pub const HID_INTERFACE: i32 = 4;
/// What hidapi reports when the platform does not expose interface numbers.
pub const INTERFACE_UNKNOWN: i32 = -1;

/// The fields of an enumerated HID device that selection looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HidEntry {
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface_number: i32,
}

/// Why no single E2x2 HID entry could be chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectError {
    /// No enumerated entry is the E2x2's HID control interface.
    NotFound,
    /// Several E2x2 entries match and none reports interface numbers.
    Ambiguous { matches: usize },
}

/// Returns the index in `entries` of the E2x2 HID control interface.
///
/// Prefers the first E2x2 entry on interface [`HID_INTERFACE`]. If none
/// exists and the platform does not report interface numbers, falls back to
/// the E2x2 entry when exactly one exists.
pub fn select(entries: &[HidEntry]) -> Result<usize, SelectError> {
    let is_e2x2 = |e: &HidEntry| e.vendor_id == VENDOR_ID && e.product_id == PRODUCT_ID;
    let matches: Vec<usize> = (0..entries.len())
        .filter(|&i| is_e2x2(&entries[i]))
        .collect();
    if let Some(&i) = matches
        .iter()
        .find(|&&i| entries[i].interface_number == HID_INTERFACE)
    {
        return Ok(i);
    }
    let unknown: Vec<usize> = matches
        .into_iter()
        .filter(|&i| entries[i].interface_number == INTERFACE_UNKNOWN)
        .collect();
    match unknown.as_slice() {
        [] => Err(SelectError::NotFound),
        [i] => Ok(*i),
        many => Err(SelectError::Ambiguous {
            matches: many.len(),
        }),
    }
}
