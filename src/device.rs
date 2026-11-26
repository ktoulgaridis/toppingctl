//! Locating a TOPPING interface among enumerated HID devices.
//!
//! Selection is a pure function over [`HidEntry`] / [`DeviceEntry`], minimal
//! mirrors of the `hidapi::DeviceInfo` fields it needs, so it can be tested
//! without hardware.
//!
//! The E2x2's control interface is known to be interface 4 ([`select`]).
//! For the other models ([`discover`], [`select_vendor`]) the interface
//! number is unknown, so the one HID interface whose usage page is
//! vendor-defined (`0xFF00..=0xFFFF`) is taken: the control interface
//! carries TOPPING's own reports, while any other HID interface on the
//! device (a volume knob, media keys) uses a standard page such as Consumer
//! (`0x0C`).

use crate::model::Model;

/// USB vendor ID of the TOPPING E2x2.
pub const VENDOR_ID: u16 = crate::model::VENDOR_ID;
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
    let on = |interface: i32| {
        entries.iter().enumerate().filter(move |(_, e)| {
            e.vendor_id == VENDOR_ID
                && e.product_id == PRODUCT_ID
                && e.interface_number == interface
        })
    };
    if let Some((i, _)) = on(HID_INTERFACE).next() {
        return Ok(i);
    }
    let unknown: Vec<usize> = on(INTERFACE_UNKNOWN).map(|(i, _)| i).collect();
    match unknown[..] {
        [] => Err(SelectError::NotFound),
        [i] => Ok(i),
        _ => Err(SelectError::Ambiguous {
            matches: unknown.len(),
        }),
    }
}

/// The usage pages the HID spec reserves for vendor-defined use.
pub const VENDOR_PAGES: std::ops::RangeInclusive<u16> = 0xff00..=0xffff;
/// What hidapi reports when it could not read the report descriptor.
pub const USAGE_PAGE_UNKNOWN: u16 = 0;

/// The fields of an enumerated HID device that [`discover`] looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface_number: i32,
    /// The top-level collection's usage page.
    pub usage_page: u16,
    /// bcdDevice: the firmware version, `0x0106` for 1.06 (0 if unknown).
    pub release_number: u16,
}

impl DeviceEntry {
    fn hid(&self) -> HidEntry {
        HidEntry {
            vendor_id: self.vendor_id,
            product_id: self.product_id,
            interface_number: self.interface_number,
        }
    }

    fn is(&self, model: Model) -> bool {
        Model::from_usb_id(self.vendor_id, self.product_id) == Some(model)
    }
}

/// The interface [`discover`] chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub model: Model,
    /// Its index in the enumerated entries.
    pub index: usize,
    /// Its bcdDevice (0 if unknown).
    pub release_number: u16,
}

/// Why no interface could be chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoverError {
    /// No known model is attached (or none has a control interface).
    NotFound,
    /// `model` is attached but its control interface can't be told apart.
    Ambiguous { model: Model, matches: usize },
}

/// Finds the first attached model, in [`Model::ALL`] order, and its control
/// interface. The E2x2 is chosen exactly as [`select`] does, so it wins
/// whatever else is attached; for the others see [`select_vendor`].
pub fn discover(entries: &[DeviceEntry]) -> Result<Found, DiscoverError> {
    let found = |model: Model, index: usize| Found {
        model,
        index,
        release_number: entries[index].release_number,
    };
    let hid: Vec<HidEntry> = entries.iter().map(DeviceEntry::hid).collect();
    match select(&hid) {
        Ok(i) => return Ok(found(Model::E2x2, i)),
        Err(SelectError::Ambiguous { matches }) => {
            return Err(DiscoverError::Ambiguous {
                model: Model::E2x2,
                matches,
            })
        }
        Err(SelectError::NotFound) => {}
    }
    for model in Model::ALL.into_iter().filter(|m| m.is_experimental()) {
        match select_vendor(entries, model) {
            Ok(i) => return Ok(found(model, i)),
            Err(SelectError::Ambiguous { matches }) => {
                return Err(DiscoverError::Ambiguous { model, matches })
            }
            Err(SelectError::NotFound) => {}
        }
    }
    Err(DiscoverError::NotFound)
}

/// Returns the index of `model`'s vendor-defined HID interface.
///
/// Among `model`'s entries, takes those on a vendor-defined usage page; if
/// none is, those whose page is unknown. If they all share one interface
/// number, the first is chosen (macOS lists an entry per top-level
/// collection). Entries without an interface number each count as their
/// own interface. More than one interface is ambiguous. An entry on a known
/// standard page is never chosen.
pub fn select_vendor(entries: &[DeviceEntry], model: Model) -> Result<usize, SelectError> {
    let on = |page: &dyn Fn(u16) -> bool| -> Vec<usize> {
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.is(model) && page(e.usage_page))
            .map(|(i, _)| i)
            .collect()
    };
    let mut candidates = on(&|p| VENDOR_PAGES.contains(&p));
    if candidates.is_empty() {
        candidates = on(&|p| p == USAGE_PAGE_UNKNOWN);
    }
    let Some(&first) = candidates.first() else {
        return Err(SelectError::NotFound);
    };
    let mut interfaces: Vec<i32> = Vec::new();
    let mut unnumbered = 0;
    for &i in &candidates {
        match entries[i].interface_number {
            INTERFACE_UNKNOWN => unnumbered += 1,
            n if !interfaces.contains(&n) => interfaces.push(n),
            _ => {}
        }
    }
    match interfaces.len() + unnumbered {
        1 => Ok(first),
        matches => Err(SelectError::Ambiguous { matches }),
    }
}
