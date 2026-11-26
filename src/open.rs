//! Opening a TOPPING interface's HID control interface, shared by the
//! binaries.

use hidapi::{HidApi, HidDevice};

use crate::device::{self, DeviceEntry, Found, HidEntry};
use crate::error::{classify, OpenFailure};
use crate::model::Model;

/// Opens the E2x2 HID control interface (and nothing else: the raw tools
/// `hidsend` / `hidsniff` speak only the E2x2's map).
///
/// On macOS the device is opened non-exclusively, so it can be shared with
/// the official app.
pub fn open_e2x2() -> Result<HidDevice, OpenFailure> {
    let api = HidApi::new().map_err(|e| classify(&e))?;
    #[cfg(target_os = "macos")]
    api.set_open_exclusive(false);

    let infos: Vec<_> = api.device_list().collect();
    let entries: Vec<HidEntry> = infos
        .iter()
        .map(|info| HidEntry {
            vendor_id: info.vendor_id(),
            product_id: info.product_id(),
            interface_number: info.interface_number(),
        })
        .collect();
    let index = device::select(&entries)?;
    api.open_path(infos[index].path()).map_err(|e| classify(&e))
}

/// Whether a found model may be opened.
///
/// The E2x2 always may. An E4x4 Pre older than firmware 1.06 (or whose
/// version can't be read) is refused, as the official app does. The E4x4
/// Pre and E8x8 Pre otherwise need `experimental` ([`OpenFailure::Experimental`]);
/// with it they are still refused until their register map lands (#74),
/// so nothing is ever sent to them ([`OpenFailure::NotImplemented`]).
pub fn admit(found: &Found, experimental: bool) -> Result<(), OpenFailure> {
    let model = found.model;
    if let Some(floor) = model.min_firmware() {
        let release = found.release_number;
        // An unreadable version (0) is below every floor.
        if release < floor {
            return Err(OpenFailure::Firmware { model, release });
        }
    }
    match (model.is_experimental(), experimental) {
        (false, _) => Ok(()),
        (true, false) => Err(OpenFailure::Experimental { model }),
        (true, true) => Err(OpenFailure::NotImplemented { model }),
    }
}

/// Finds the first attached model ([`device::discover`]), checks it may be
/// opened ([`admit`]) and opens its control interface. A refused model is
/// never opened.
///
/// On macOS the device is opened non-exclusively, so it can be shared with
/// the official app.
pub fn open(experimental: bool) -> Result<(Model, HidDevice), OpenFailure> {
    let api = HidApi::new().map_err(|e| classify(&e))?;
    #[cfg(target_os = "macos")]
    api.set_open_exclusive(false);

    let infos: Vec<_> = api.device_list().collect();
    let entries: Vec<DeviceEntry> = infos
        .iter()
        .map(|info| DeviceEntry {
            vendor_id: info.vendor_id(),
            product_id: info.product_id(),
            interface_number: info.interface_number(),
            usage_page: info.usage_page(),
            release_number: info.release_number(),
        })
        .collect();
    let found = device::discover(&entries)?;
    admit(&found, experimental)?;
    let device = api
        .open_path(infos[found.index].path())
        .map_err(|e| classify(&e))?;
    Ok((found.model, device))
}
