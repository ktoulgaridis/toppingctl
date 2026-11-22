# Mac capture 2: rx + tx together, sample rate

Captured 2026-11-22. Same device, firmware and app versions as mac-1 (see `versions.txt`).

## Method

- **`hidsniff` can't run alongside the app on macOS.** It stops receiving as soon as the app opens the device: the app appears to take exclusive control of the device.
- **`rx` was captured inside the app instead,** with a Frida hook that wraps the callback registered through `IOHIDDeviceRegisterInputReportCallback`. `txrx.txt` interleaves both directions on one wall clock.
- **Sample rate isn't HID.** A Frida hook on CoreAudio `AudioObjectSetPropertyData` logged it in `pass2-coreaudio.txt`.

## Files

- `txrx.txt`: pass 3, rx + tx.
  - App actions: IN1 MON on/off, HP off/on.
  - Device actions: MON ×2, Monitor Mix knob, volume knob, headphone gain.
- `pass2-tx.txt`: pass 2, tx only.
  - App actions: MON, IN1 gain, Mix A fader, Loopback 1+2 source, HP toggle, store to device.
  - Its rx capture was lost: `hidsniff` stopped receiving, as described above.
- `pass2-coreaudio.txt`: sample-rate changes.
- `steps.txt`: pass start markers.

## Findings

- **rx CRC:** all 35326 rx frames match CRC16 (poly 0xA001 reflected, init 0, xorout 0x785A, big-endian in bytes 11–12). rx reports are 16 bytes, with a trailing `00`.
- **Echo:** app writes to `21 01` (MON) are echoed in rx 70–170 ms later, with a valid CRC. `34 01` (HP) was not echoed.
- **Device MON button:** sends rx `21 01` 0/1, the same frame as the echo.
- **Volume knob:** rx `11 01`, raw step count. Seen 0x23…0x4d.
- **Monitor Mix knob:** no rx and no tx, so probably analog only.
- **Sample rate:** CoreAudio `kAudioDevicePropertyNominalSampleRate` (`nsrt`, float64, global scope) on the E2x2 audio device. Seen: 44100, 48000, 88200, 96000, 176400, 192000. No HID frame accompanies it.
