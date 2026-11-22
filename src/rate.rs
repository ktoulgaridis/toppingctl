//! The interface's sample rate. It is not a HID register: the official app
//! reads and sets it through CoreAudio (captures/mac-2), so toppingctl asks
//! the OS.
//!
//! - **Linux:** the E2x2's ALSA card (found by its USB id in
//!   `/proc/asound/card*/usbid`) gives the rate of an open substream
//!   (`pcm*/sub*/hw_params`) or a running stream (`stream*`, "Momentary
//!   freq"). While the card is idle there is none, and PipeWire's graph
//!   rate stands in (`pw-metadata -n settings 0`: a forced rate, else
//!   `clock.rate`). Setting a rate forces PipeWire's graph to it
//!   (`clock.force-rate`).
//! - **Elsewhere:** unknown (`—`), and not settable from here yet.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use crate::device::{PRODUCT_ID, VENDOR_ID};

/// The E2x2's rates, in Hz.
pub const RATES: [u32; 6] = [44100, 48000, 88200, 96000, 176400, 192000];
/// How often [`poll`] reads the rate.
pub const POLL: Duration = Duration::from_secs(2);

/// A rate in kHz: `48 kHz`, `44.1 kHz`; `— kHz` unknown.
pub fn format_khz(rate: Option<u32>) -> String {
    match rate {
        None => "— kHz".into(),
        Some(r) if r % 1000 == 0 => format!("{} kHz", r / 1000),
        Some(r) => format!("{:.1} kHz", f64::from(r) / 1000.0),
    }
}

/// The rate of the ALSA card with USB id `usb_id` (`152a:8752`) under
/// `root` (`/proc/asound`): an open substream's, else a running stream's.
pub fn from_proc(root: &Path, usb_id: &str) -> Option<u32> {
    let card = fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            let is_card = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card"));
            is_card && fs::read_to_string(p.join("usbid")).is_ok_and(|id| id.trim() == usb_id)
        })?;
    let mut hw = None;
    let mut stream = None;
    for entry in fs::read_dir(&card).ok()?.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("pcm") {
            for sub in fs::read_dir(&path).into_iter().flatten().flatten() {
                let text = fs::read_to_string(sub.path().join("hw_params")).unwrap_or_default();
                hw = hw.or_else(|| hw_params_rate(&text));
            }
        } else if name.starts_with("stream") {
            let text = fs::read_to_string(&path).unwrap_or_default();
            stream = stream.or_else(|| momentary_rate(&text));
        }
    }
    hw.or(stream)
}

/// `rate: 48000 (48000/1)` in an open substream's hw_params.
fn hw_params_rate(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.strip_prefix("rate:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
}

/// `Momentary freq = 48000 Hz (0x6.0000)` in a running stream.
fn momentary_rate(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("Momentary freq ="))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
}

/// The rate in `pw-metadata -n settings 0`'s output: `clock.force-rate`
/// when set (not 0), else `clock.rate`.
pub fn from_pw_metadata(out: &str) -> Option<u32> {
    let value = |key: &str| -> Option<u32> {
        let tag = format!("key:'{key}' value:'");
        out.lines().find_map(|l| {
            let rest = &l[l.find(&tag)? + tag.len()..];
            rest[..rest.find('\'')?].parse().ok()
        })
    };
    value("clock.force-rate")
        .filter(|&r| r > 0)
        .or_else(|| value("clock.rate"))
        .filter(|&r| r > 0)
}

/// The `pw-metadata` arguments that force PipeWire's graph to `rate`; only
/// for one of [`RATES`].
pub fn force_rate_args(rate: u32) -> Option<[String; 5]> {
    RATES.contains(&rate).then(|| {
        [
            "-n".into(),
            "settings".into(),
            "0".into(),
            "clock.force-rate".into(),
            rate.to_string(),
        ]
    })
}

/// The E2x2's current rate, if this OS tells.
pub fn current() -> Option<u32> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let usb_id = format!("{VENDOR_ID:04x}:{PRODUCT_ID:04x}");
    from_proc(Path::new("/proc/asound"), &usb_id).or_else(|| {
        let out = Command::new("pw-metadata")
            .args(["-n", "settings", "0"])
            .output()
            .ok()?;
        from_pw_metadata(&String::from_utf8_lossy(&out.stdout))
    })
}

/// Sets the rate (Linux, through PipeWire). The error is for the user.
pub fn set(rate: u32) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("setting the sample rate is not supported on this OS yet".into());
    }
    let args = force_rate_args(rate).ok_or_else(|| format!("{rate} Hz is not an E2x2 rate"))?;
    match Command::new("pw-metadata").args(args).output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(_) => Err("pw-metadata could not set the sample rate".into()),
        Err(_) => Err("setting the sample rate needs PipeWire (pw-metadata)".into()),
    }
}

/// Reads [`current`] every [`POLL`] on a background thread; each reading
/// arrives on the channel. The thread ends when the receiver is dropped.
pub fn poll() -> Receiver<Option<u32>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || loop {
        if tx.send(current()).is_err() {
            return;
        }
        thread::sleep(POLL);
    });
    rx
}
