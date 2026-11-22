//! Opening the E2x2's HID control interface, shared by the binaries.

use hidapi::{HidApi, HidDevice};

use crate::device::{self, HidEntry};
use crate::error::{classify, OpenFailure};

/// Opens the E2x2 HID control interface.
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
