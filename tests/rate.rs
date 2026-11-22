//! The interface's sample rate (#34): read from ALSA's /proc files or
//! PipeWire, shown as `48 kHz`, set through PipeWire. No hardware, no
//! PipeWire: the readers take their input as arguments.

use std::fs;
use std::path::Path;

use toppingctl::rate::{force_rate_args, format_khz, from_proc, from_pw_metadata, RATES};

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn tempdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("toppingctl-rate-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

const STOPPED: &str = "Topping E2x2 at usb-0000:00:14.0-4.2, high speed : USB Audio\n\nPlayback:\n  Status: Stop\n  Interface 1\n    Rates: 44100, 48000, 88200, 96000, 176400, 192000\n";
const RUNNING: &str = "Topping E2x2 at usb-0000:00:14.0-4.2, high speed : USB Audio\n\nPlayback:\n  Status: Running\n    Interface = 1\n    Altset = 1\n    Packet Size = 104\n    Momentary freq = 88200 Hz (0xb.0666)\n";

/// The E2x2's card is found by its USB id; an open substream's hw_params
/// give the rate, else a running stream's momentary frequency.
#[test]
fn rate_from_alsa_proc() {
    let root = tempdir("proc");
    // Another USB card, running at 32 kHz: not ours.
    write(&root, "card1/usbid", "291a:3369\n");
    write(
        &root,
        "card1/pcm0c/sub0/hw_params",
        "access: RW_INTERLEAVED\nrate: 32000 (32000/1)\n",
    );
    // The E2x2, stopped: no rate.
    write(&root, "card2/usbid", "152a:8752\n");
    write(&root, "card2/pcm0p/sub0/hw_params", "closed\n");
    write(&root, "card2/pcm0c/sub0/hw_params", "closed\n");
    write(&root, "card2/stream0", STOPPED);
    assert_eq!(from_proc(&root, "152a:8752"), None);

    // Running: the stream's momentary frequency.
    write(&root, "card2/stream0", RUNNING);
    assert_eq!(from_proc(&root, "152a:8752"), Some(88200));

    // Open: hw_params wins.
    write(
        &root,
        "card2/pcm0c/sub0/hw_params",
        "access: MMAP_INTERLEAVED\nformat: S32_LE\nsubformat: STD\nchannels: 2\nrate: 96000 (96000/1)\n",
    );
    assert_eq!(from_proc(&root, "152a:8752"), Some(96000));

    // No E2x2 at all.
    assert_eq!(from_proc(&root, "dead:beef"), None);
    let _ = fs::remove_dir_all(&root);
}

/// PipeWire's settings: a forced rate first, else the graph's clock rate.
#[test]
fn rate_from_pipewire_settings() {
    let out = "Found \"settings\" metadata 31\nupdate: id:0 key:'log.level' value:'2' type:''\nupdate: id:0 key:'clock.rate' value:'48000' type:''\nupdate: id:0 key:'clock.allowed-rates' value:'[ 48000 ]' type:''\nupdate: id:0 key:'clock.force-rate' value:'0' type:''\n";
    assert_eq!(from_pw_metadata(out), Some(48000));
    let forced = out.replace(
        "clock.force-rate' value:'0'",
        "clock.force-rate' value:'96000'",
    );
    assert_eq!(from_pw_metadata(&forced), Some(96000));
    assert_eq!(from_pw_metadata(""), None);
}

/// `48 kHz`, `44.1 kHz`, `— kHz` while unknown.
#[test]
fn rate_formatting() {
    assert_eq!(format_khz(Some(48000)), "48 kHz");
    assert_eq!(format_khz(Some(44100)), "44.1 kHz");
    assert_eq!(format_khz(Some(176400)), "176.4 kHz");
    assert_eq!(format_khz(None), "— kHz");
}

/// Setting a rate asks PipeWire to force it; only the E2x2's rates.
#[test]
fn force_rate_command() {
    assert_eq!(RATES, [44100, 48000, 88200, 96000, 176400, 192000]);
    assert_eq!(
        force_rate_args(96000).unwrap(),
        ["-n", "settings", "0", "clock.force-rate", "96000"]
    );
    assert!(force_rate_args(32000).is_none());
}
