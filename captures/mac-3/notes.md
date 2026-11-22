# Mac capture 3: hardware replay checks with `hidsend`

2026-11-22, macOS, E2x2 firmware V1.13, official app not running. Nothing was plugged into IN2.
`*.args` files are the exact `hidsend` arguments for each run.

## Echo behaviour

- `sweep-same-value.txt`: every launch-state register written back with its current value. **No echo** for any of them, so the device echoes only on change.
- `10 06 = 1` is a **device-info query**. The reply is `10 03 = 0x0001000e` (hardware V1.14), `10 04 = 0x0001000d` (firmware V1.13) and `10 0a` (brightness). The app sends it first at launch.
- `sweep-change-restore.txt`: every register set to a different value, then restored. Only the button-backed registers echo, all with a valid CRC:
  - `21 01`, `22 01` (MON)
  - `21 03`, `22 03` (INST)
  - `22 02` (IN2 48V; IN1 48V was not touched)
  - `11 02` (Phone Gain)
- No echo for levels, crosspoints, sources, `34 01/02`, or `10 0a/0b/0c`.

## Device gain button

`device-gain-button.txt` (`hidsniff`, app closed): pressing the device's gain button sends rx `11 02` 0, then 1. That's the same register as the app's Phone Gain, and 1 = gain on.

## Meter check (`meters-tone.txt`)

A 1 kHz tone at −30 dBFS was played to the E2x2 (Playback 1/2), and the meters were read while registers changed.

Meter map, inferred:

| Meter | What it measures |
|---|---|
| `41 01/02` | Playback 1/2 L/R (`42`, `43` = PB3/4, PB5/6) |
| `31 03/04` | Output 1+2 L/R, before the output level |
| `32 03/04`, `33 03/04` | Loopback 1+2 / 3+4 L/R, before the loopback level. Sent only while the source has signal |

Verified by the meters:

| Write | Meter response |
|---|---|
| `51 13 = 0` (Mix A PB1→L) | `31 03` drops |
| `51 24 = 0` (Mix A PB2→R) | `31 04` drops |
| `31 05 = 0x20` (Output source Mix B) with `52 13 = 0` | `31 03` drops: Mix B and its crosspoints work |
| `31 05 = 0x30` (Mix C) with `53 24 = 0` | `31 04` drops |
| `32 05 = 0x100` / `33 05 = 0x100` (Loopback source PB1/2) | loopback meters show the tone; switching back to PB3/4 or PB5/6 drops them to the floor |

The meters sit before the level controls, so `31 01/02` and `32`/`33` `01/02` did not move them. Those levels still need checking by ear or by recording.

## Output fader range (`app-output-fader.txt`)

Captured from the official app (Frida rx+tx hook). The Output 1+2 fader runs from 0 to `0x2000000` (0 dB is the top of the fader) and writes `31 01` and `31 02` together.
