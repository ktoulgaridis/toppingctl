//! The E2x2's nominal sample rate through CoreAudio (macOS), by direct FFI
//! to the CoreAudio and CoreFoundation frameworks: the same property the
//! official app sets (`nsrt`, captures/mac-2).

use std::ffi::{c_char, c_void};
use std::mem::size_of;
use std::ptr;

use super::{coreaudio_error, is_e2x2_model_uid, nominal_for, rate_from_nominal};

type AudioObjectId = u32;
type OsStatus = i32;
type CfStringRef = *const c_void;

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

/// `kAudioObjectSystemObject`.
const SYSTEM_OBJECT: AudioObjectId = 1;
/// `kAudioObjectPropertyScopeGlobal`.
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
/// `kAudioObjectPropertyElementMain`.
const ELEMENT_MAIN: u32 = 0;
/// `kAudioHardwarePropertyDevices`.
const DEVICES: u32 = fourcc(b"dev#");
/// `kAudioDevicePropertyModelUID`.
const MODEL_UID: u32 = fourcc(b"muid");
/// `kAudioDevicePropertyNominalSampleRate`.
const NOMINAL_SAMPLE_RATE: u32 = fourcc(b"nsrt");
/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
    ) -> OsStatus;
    fn AudioObjectGetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectSetPropertyData(
        object: AudioObjectId,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: u32,
        data: *const c_void,
    ) -> OsStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringGetCString(s: CfStringRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(cf: *const c_void);
}

fn global(selector: u32) -> PropertyAddress {
    PropertyAddress {
        selector,
        scope: SCOPE_GLOBAL,
        element: ELEMENT_MAIN,
    }
}

/// Every CoreAudio device id.
fn devices() -> Result<Vec<AudioObjectId>, OsStatus> {
    let address = global(DEVICES);
    let mut size = 0u32;
    // SAFETY: valid address and out-pointer; no qualifier.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &address, 0, ptr::null(), &mut size)
    };
    if status != 0 {
        return Err(status);
    }
    let mut ids = vec![0 as AudioObjectId; size as usize / size_of::<AudioObjectId>()];
    let mut size = (ids.len() * size_of::<AudioObjectId>()) as u32;
    // SAFETY: `ids` holds `size` bytes; CoreAudio writes at most that and
    // reports what it wrote.
    let status = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            &address,
            0,
            ptr::null(),
            &mut size,
            ids.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return Err(status);
    }
    ids.truncate(size as usize / size_of::<AudioObjectId>());
    Ok(ids)
}

/// A device's model UID (`E2x2:152A:8752` for USB audio class devices).
fn model_uid(device: AudioObjectId) -> Option<String> {
    let address = global(MODEL_UID);
    let mut cf: CfStringRef = ptr::null();
    let mut size = size_of::<CfStringRef>() as u32;
    // SAFETY: the property is a CFStringRef, which `cf` holds; we own the
    // returned reference and release it below.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            &address,
            0,
            ptr::null(),
            &mut size,
            (&mut cf as *mut CfStringRef).cast(),
        )
    };
    if status != 0 || cf.is_null() {
        return None;
    }
    let mut buf = [0 as c_char; 256];
    // SAFETY: `cf` is a live CFString; `buf` is 256 bytes.
    let ok = unsafe { CFStringGetCString(cf, buf.as_mut_ptr(), buf.len() as isize, UTF8) };
    // SAFETY: released once, as Get rules require.
    unsafe { CFRelease(cf) };
    if ok == 0 {
        return None;
    }
    // SAFETY: CFStringGetCString NUL-terminated it on success.
    let text = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(text.to_string_lossy().into_owned())
}

/// The E2x2's CoreAudio device, found by the USB ids in its model UID.
fn e2x2() -> Option<AudioObjectId> {
    devices()
        .ok()?
        .into_iter()
        .find(|&d| model_uid(d).is_some_and(|uid| is_e2x2_model_uid(&uid)))
}

/// The E2x2's nominal sample rate, if it is attached.
pub fn current() -> Option<u32> {
    let device = e2x2()?;
    let address = global(NOMINAL_SAMPLE_RATE);
    let mut rate = 0f64;
    let mut size = size_of::<f64>() as u32;
    // SAFETY: the property is a Float64, which `rate` holds.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            &address,
            0,
            ptr::null(),
            &mut size,
            (&mut rate as *mut f64).cast(),
        )
    };
    (status == 0).then(|| rate_from_nominal(rate)).flatten()
}

/// Sets the E2x2's nominal sample rate. The error is for the user.
pub fn set(rate: u32) -> Result<(), String> {
    let nominal = nominal_for(rate).ok_or_else(|| format!("{rate} Hz is not an E2x2 rate"))?;
    let device = e2x2().ok_or("no E2x2 audio device in CoreAudio")?;
    let address = global(NOMINAL_SAMPLE_RATE);
    // SAFETY: a Float64 of `size_of::<f64>()` bytes, as the property takes.
    let status = unsafe {
        AudioObjectSetPropertyData(
            device,
            &address,
            0,
            ptr::null(),
            size_of::<f64>() as u32,
            (&nominal as *const f64).cast(),
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(coreaudio_error(status))
    }
}
