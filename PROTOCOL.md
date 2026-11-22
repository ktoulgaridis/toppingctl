# TOPPING E2x2 HID protocol

The E2x2 (USB `152a:8752`) is controlled over its HID interface. This map comes only from our own
captures (`captures/`) and our own replays (clean room, see `CLAUDE.md`).

**Verified against:** E2x2 hardware V1.14, firmware V1.13; official app (TOPPING Professional Control
Center) V1.09 on macOS; replays from macOS and from Linux (kernel 7.3.5). Other firmware may differ.

## Frame

Every report is 16 bytes, in both directions:

```text
22 33 20 01 01  GG  PP  V3 V2 V1 V0  C1 C0  66 77  00
header          grp par value         crc    trailer pad
```

- **Value:** signed 32-bit, big-endian.
- **CRC:** CRC16 over bytes 0–10 (poly `0xA001` reflected, init `0`, final XOR `0x785A`), stored
  big-endian in bytes 11–12. `src/frame.rs` implements it.
- **Device→host frames always carry the CRC.** All 67 890 rx frames in `captures/` match it.
- **Host→device:** the official app sends `00 00` in the CRC bytes. **The device accepts both** `00 00`
  and the real CRC16 (macOS and Linux replays). With the real CRC, the device's echo matches our frame
  byte for byte.
- One frame sets one register. There is no batching and no acknowledgement frame (see Echo).

## Value encodings

| Name | Encoding |
|---|---|
| bool | `0` off, `1` on |
| level | linear gain, `0x02000000` = 0 dB (gain = value / 2²⁵). `0x01000000` = −6.02 dB, `0` = muted (−∞). Negative = phase-inverted (Ø) |
| source | bitmask: Mix A/B/C = `0x10`/`0x20`/`0x30`; Playback 1/2, 3/4, 5/6 = `0x100`/`0x200`/`0x300`; IN1 = `0x1000`, IN2 = `0x2000`, IN1+2 = `0x3000` |
| meter | signed, in tenths of a dB: `-299` ≈ −29.9 dBFS (peak). Floor ≈ `-960` |

The app computes SOLO, MUTE, Ø, pan and Link itself and writes the resulting levels: those controls have
no registers of their own.

## Echo

The device sends an rx frame for a register only when its value **changes**, and only for the
button-backed registers (MON, INST, 48V, Phone Gain). A hardware button press sends the same frame.
Same-value writes, levels, crosspoints, sources and settings are applied silently.

## Verified column

| Mark | Meaning |
|---|---|
| **echo** | replayed with `hidsend`; the device echoed the value and its LED followed (macOS and Linux) |
| **rec** | replayed; the effect was measured in a recording (`captures/linux-1`) |
| **meter** | replayed; the device's own meters responded (`captures/mac-3`) |
| **reply** | the device sends it in reply to a query or on its own; seen in rx |
| **app** | seen in the official app's writes only; the encoding is known but the effect was not independently checked |

Dir: **w** = host→device write, **r** = device→host report.

## Group `00`: idle

| param | meaning | encoding | range | dir | verified |
|---|---|---|---|---|---|
| `00` | idle / keep-alive report, value 0 | — | 0 | r | reply |

## Group `10`: device and settings

| param | meaning | encoding | range | dir | verified |
|---|---|---|---|---|---|
| `03` | hardware version | `0x00MM00mm` = VMM.mm | `0x0001000e` = V1.14 | r | reply |
| `04` | firmware version | `0x00MM00mm` = VMM.mm | `0x0001000d` = V1.13 | r | reply |
| `06` | device-info query (write 1; the device echoes it, then sends `10 0b`, `10 04`, `11 01`, `10 0a`, `10 03`, `10 0c`) | command | 1 | w, r | reply |
| `0a` | display brightness | int | 0, 1 (Normal), 2 | w, r | app (value read back in the query reply) |
| `0b` | automatic standby | bool | | w, r | app (value read back in the query reply) |
| `0c` | mobile applications | bool | | w, r | app (value read back in the query reply) |

## Group `11`: monitor

| param | meaning | encoding | range | dir | verified |
|---|---|---|---|---|---|
| `01` | Monitor Mix balance (input ↔ playback) | int, 50 = centre | 0…100 (seen `0x23`…`0x4d`) | r | reply (knob turned on the device); RE of app V1.09 (see below) |
| `02` | phone out level switch: +17 dBu / 0 dBu (the device's gain button) | bool | | w, r | echo; RE of app V1.09 for the labels |

## Group `1a`: commands

| param | meaning | encoding | range | dir | verified |
|---|---|---|---|---|---|
| `03` | store current state to device | command | 0 | w | app (not replayed: it writes flash) |

## Groups `21` / `22`: IN1 / IN2

| param | meaning | encoding | range | dir | verified |
|---|---|---|---|---|---|
| `01` | MON | bool | | w, r | echo |
| `02` | 48V phantom power | bool | | w, r | echo (IN2 only; IN1 never replayed on purpose) |
| `03` | INST (Hi-Z) | bool | | w, r | echo |
| `04` | input gain (also MUTE = 0, Ø = negated) | level | `0x02000000`…`0x14000000` (+0…+20 dB) | w | app |
| `05` | input meter | meter | | r | reply |

## Group `31`: Output 1+2

| param | meaning | encoding | range | dir | verified |
|---|---|---|---|---|---|
| `01` / `02` | output level L / R (the app's fader writes both) | level | `0`…`0x02000000` (0 dB max) | w | app (drives the line outs, which were not monitored) |
| `03` / `04` | output meter L / R, before the output level | meter | | r | meter |
| `05` | output source | source | Mix A/B/C | w | meter |

## Groups `32` / `33`: Loopback 1+2 / 3+4

| param | meaning | encoding | range | dir | verified |
|---|---|---|---|---|---|
| `01` / `02` | loopback level L / R (MUTE = 0) | level | `0`…`0x02000000` | w | rec (`32 01` → LB1, `33 02` → LB4, exactly −6.0 dB) |
| `03` / `04` | loopback meter L / R, before the level; sent only while the source has signal | meter | | r | meter |
| `05` | loopback source | source | any | w | meter (PB1/2, PB3/4, PB5/6) |

## Group `34`: output switches

| param | meaning | encoding | range | dir | verified |
|---|---|---|---|---|---|
| `01` | headphone out (the app also resends `31 01/02`) | bool | | w | app |
| `02` | line out | bool | | w | app |

## Groups `41` / `42` / `43`: playback meters

| param | meaning | encoding | range | dir | verified |
|---|---|---|---|---|---|
| `01` / `02` | Playback 1/2, 3/4, 5/6 meter L / R | meter | | r | meter |

## Groups `51` / `52` / `53`: Mix A / B / C crosspoints

Param `<o><s>`: o = output (`1` L, `2` R), s = source (`1` IN1, `2` IN2, `3`…`8` PB1…PB6). 16 per mix.

| param | meaning | encoding | range | dir | verified |
|---|---|---|---|---|---|
| `11`…`18` | source s → mix L | level | `0`…`0x03fb2784` (≈ +6 dB max) | w | meter for `51 13`, `52 13`; app for the rest |
| `21`…`28` | source s → mix R | level | `0`…`0x03fb2784` | w | meter for `51 24`, `53 24`; app for the rest |

The app's fader and pan are a linear split of the level across L and R (centre = 0.5 each).

## Monitor Mix and the phone out (static RE)

From a read-only look at the official app V1.09 (class and method names and UI strings, never run):

- `11 01` is the **Monitor Mix balance**, not the volume knob. The device reports it when its Monitor
  Mix knob turns (0…100, 50 = centre). The app shows it read-only: `Balance_big::setValue(v-50)`,
  with the mouse disabled and no write.
- `11 02` is the phone out level switch, labelled "+17dBu" / "0dBu". Which bool value is which label
  is not confirmed by a capture; toppingctl shows `1` as +17 dBu.
- The HP and LINE buttons (`34 01/02`) resend `31 01/02`, so the OUT 1+2 fader is the shared output
  level.

## Not HID

- **Sample rate:** set through CoreAudio / ALSA, not HID. No frame accompanies a change.
- **Headphone volume:** the device's analog volume knob. No frame sets it; software never changes it.
- **Workspace save/load:** app-only, no frames.

## Open questions

- **State on connect:** the device has no known full-state read. The `10 06` query returns the device
  info, the settings and the Monitor Mix balance only. The official app writes its own saved state at launch
  (69 frames), so a client that wants to show the current switches has to either push a state or
  track echoes from then on.
- **Output level (`31 01/02`):** encoding known, effect unmeasured.
- **Headphone path:** changing `31 01` made no audible change in the headphones, so the headphones are
  probably not fed through the output level.
- **Mix B/C pan:** not exercised in the app; assumed to match Mix A.
- **IN1 48V:** never replayed, by choice.

## Fixtures

`tests/fixtures/frames.tsv` holds up to three distinct frames for every (direction, group, param) seen
in `captures/`, with the file each came from. `tools/fixtures.py` regenerates it, and
`tests/frame.rs` checks that every fixture decodes and re-encodes to the same bytes.
