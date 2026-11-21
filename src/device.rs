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

#[cfg(test)]
mod tests {
    use super::*;

    fn e2x2(interface_number: i32) -> HidEntry {
        HidEntry {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID,
            interface_number,
        }
    }

    fn other(vendor_id: u16, product_id: u16, interface_number: i32) -> HidEntry {
        HidEntry {
            vendor_id,
            product_id,
            interface_number,
        }
    }

    #[test]
    fn empty_list_is_not_found() {
        assert_eq!(select(&[]), Err(SelectError::NotFound));
    }

    #[test]
    fn picks_interface_four_among_unrelated_devices() {
        let list = [
            other(0x046d, 0xc52b, 2),
            other(VENDOR_ID, 0x0001, 4),
            other(0x0001, PRODUCT_ID, 4),
            e2x2(4),
        ];
        assert_eq!(select(&list), Ok(3));
    }

    #[test]
    fn prefers_interface_four_over_other_e2x2_interfaces() {
        let list = [e2x2(0), e2x2(5), e2x2(4), e2x2(INTERFACE_UNKNOWN)];
        assert_eq!(select(&list), Ok(2));
    }

    #[test]
    fn takes_first_interface_four_entry_when_listed_twice() {
        // macOS lists one entry per top-level collection of the same interface.
        let list = [other(0x046d, 0xc52b, 0), e2x2(4), e2x2(4)];
        assert_eq!(select(&list), Ok(1));
    }

    #[test]
    fn falls_back_to_only_entry_without_interface_number() {
        let list = [
            other(0x046d, 0xc52b, INTERFACE_UNKNOWN),
            e2x2(INTERFACE_UNKNOWN),
        ];
        assert_eq!(select(&list), Ok(1));
    }

    #[test]
    fn several_entries_without_interface_numbers_are_ambiguous() {
        let list = [e2x2(INTERFACE_UNKNOWN), e2x2(INTERFACE_UNKNOWN)];
        assert_eq!(select(&list), Err(SelectError::Ambiguous { matches: 2 }));
    }

    #[test]
    fn only_entry_on_a_known_non_hid_interface_is_not_found() {
        assert_eq!(select(&[e2x2(2)]), Err(SelectError::NotFound));
    }

    #[test]
    fn unrelated_devices_only_is_not_found() {
        let list = [
            other(0x046d, 0xc52b, INTERFACE_UNKNOWN),
            other(0x1234, 0x5678, 4),
        ];
        assert_eq!(select(&list), Err(SelectError::NotFound));
    }
}
