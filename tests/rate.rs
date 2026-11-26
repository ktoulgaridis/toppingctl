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

/// macOS (#55): the E2x2 is the CoreAudio device whose model UID ends in its
/// USB ids, as Apple's USB audio driver writes them.
#[test]
fn coreaudio_device_match() {
    use toppingctl::rate::is_e2x2_model_uid;
    assert!(is_e2x2_model_uid("E2x2:152A:8752"));
    assert!(is_e2x2_model_uid("E2x2:152a:8752"));
    // A name with colons in it.
    assert!(is_e2x2_model_uid("TOPPING: E2x2:152A:8752"));
    // Other devices, and the ids alone without a name.
    assert!(!is_e2x2_model_uid("Scarlett 2i2 USB:1235:8210"));
    assert!(!is_e2x2_model_uid("E2x2:152A:8753"));
    assert!(!is_e2x2_model_uid("E2x2:8752:152A"));
    assert!(!is_e2x2_model_uid("152A:8752"));
    assert!(!is_e2x2_model_uid("BuiltInSpeakerDevice"));
    assert!(!is_e2x2_model_uid(""));
}

/// CoreAudio's Float64 nominal rate to Hz and back; only E2x2 rates are set.
#[test]
fn coreaudio_rate_mapping() {
    use toppingctl::rate::{nominal_for, rate_from_nominal};
    // The values the official app set (captures/mac-2/pass2-coreaudio.txt).
    for (bytes, hz) in [
        ([0x00, 0x00, 0x00, 0x00, 0x80, 0x88, 0xe5, 0x40], 44100),
        ([0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0xe7, 0x40], 48000),
        ([0x00, 0x00, 0x00, 0x00, 0x80, 0x88, 0xf5, 0x40], 88200),
        ([0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0xf7, 0x40], 96000),
        ([0x00, 0x00, 0x00, 0x00, 0x80, 0x88, 0x05, 0x41], 176400),
        ([0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x07, 0x41], 192000),
    ] {
        let nominal = f64::from_le_bytes(bytes);
        assert_eq!(rate_from_nominal(nominal), Some(hz));
        assert_eq!(nominal_for(hz).map(f64::to_le_bytes), Some(bytes));
    }
    assert_eq!(rate_from_nominal(47999.9), Some(48000));
    assert_eq!(rate_from_nominal(0.0), None);
    assert_eq!(rate_from_nominal(-1.0), None);
    assert_eq!(rate_from_nominal(f64::NAN), None);
    assert_eq!(nominal_for(32000), None);
}

/// A failed CoreAudio set becomes the notice: its four-character code, with
/// a word when the user can do something about it.
#[test]
fn coreaudio_error_notice() {
    use toppingctl::rate::coreaudio_error;
    let code = |c: &[u8; 4]| i32::from_be_bytes(*c);
    assert_eq!(
        coreaudio_error(code(b"!hog")),
        "CoreAudio could not set the sample rate ('!hog': another app has the E2x2 to itself (hog mode))"
    );
    assert_eq!(
        coreaudio_error(code(b"nope")),
        "CoreAudio could not set the sample rate ('nope': the E2x2 refused it)"
    );
    assert_eq!(
        coreaudio_error(code(b"who?")),
        "CoreAudio could not set the sample rate ('who?')"
    );
    assert_eq!(
        coreaudio_error(-50),
        "CoreAudio could not set the sample rate (-50)"
    );
}

/// Read-only hardware check (macOS, E2x2 attached): CoreAudio finds it and
/// reads its rate. Never sets it. `cargo test --test rate -- --ignored`.
#[test]
#[ignore = "needs an attached E2x2 (macOS)"]
#[cfg(target_os = "macos")]
fn coreaudio_reads_attached_e2x2() {
    let rate = toppingctl::rate::current();
    eprintln!("E2x2 nominal sample rate: {rate:?}");
    assert!(rate.is_some_and(|r| RATES.contains(&r)), "{rate:?}");
}
