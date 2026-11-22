# Mac capture 1: host→device (tx) register map

Captured 2026-11-22 on macOS with Frida hooking `IOHIDDeviceSetReport` in an ad-hoc re-signed copy of
TOPPING Professional Control Center V1.09. Device: E2x2, hardware V1.14, firmware V1.13.
`rx` was not captured (no cargo on this Mac). Timestamps in `tx.txt` and `steps.txt` share a clock.

These are hypotheses from one capture. Each one matches the frames, but none has been replayed to the device yet.

## Frame

15 bytes: `22 33 20 01 01 <grp> <reg> <v3 v2 v1 v0> 00 00 66 77`

- The value is big-endian, signed 32-bit.
- **The CRC bytes (11–12) are `00 00` on every host→device frame**, unlike device→host frames, which carry CRC16.
- Every frame ends `66 77`.
- 69 frames at launch write the full state (inputs, all three mix matrices, loopback, output, settings).

## Levels

Linear gain, where `0x02000000` = 0 dB (≈ `0x1ffffe0` in the mixer) and the steps are about 1 dB.
A negative value means phase-inverted (Ø) and 0 means muted.

## Registers

| grp reg | control | value |
|---|---|---|
| `21`/`22` `01` | IN1/IN2 MON | 0/1 |
| `21`/`22` `02` | IN1/IN2 48V | 0/1 |
| `21`/`22` `03` | IN1/IN2 INST | 0/1 |
| `21`/`22` `04` | IN1/IN2 gain | level, +0…+20 dB (`0x02000000`…`0x14000000`). MUTE writes 0, Ø writes the negated level, SOLO writes 0 to the *other* input |
| `51`/`52`/`53` `<o><s>` | Mix A/B/C crosspoint | level. o: 1 = L, 2 = R. s: 1 IN1, 2 IN2, 3–8 PB1…PB6. Fader and pan (linear split, centre = 0.5 each) are computed in the app. Fader max ≈ +6 dB (`0x3fb2784`). Ø negates, MUTE writes 0, SOLO rewrites all 16, Link on = IN1 hard L / IN2 hard R |
| `31` `01`/`02` | Output 1+2 L/R level | level |
| `31` `05` | Output 1+2 source | see source encoding |
| `32`/`33` `01`/`02` | Loopback 1+2 / 3+4 L/R level | level, MUTE writes 0 |
| `32`/`33` `05` | Loopback 1+2 / 3+4 source | see source encoding |
| `34` `01` | Headphone out | 0/1 (also resends `31 01/02`) |
| `34` `02` | Line out | 0/1 |
| `11` `02` | Phone Out Gain | 0/1 |
| `10` `0a` | Brightness | 0/1/2 (1 = Normal) |
| `10` `0b` | Automatic standby | 0/1 |
| `10` `0c` | Mobile applications | 0/1 |
| `1a` `03` | Store to device | command, value 0 |

**Source encoding (`3x 05`):** Mix A/B/C = `0x10`/`0x20`/`0x30`. Playback 1/2, 3/4, 5/6 = `0x100`/`0x200`/`0x300`.
IN1 = `0x1000`, IN2 = `0x2000`, IN1+2 = `0x3000` (a bitmask).

## Gaps / notes

- **Monitor Mix knob:** device-only, so it doesn't appear in tx. It's probably `11 01` from the Linux rx capture.
- **Workspace save/load and app-only settings:** no frames.
- **Mix B/C:** only faders were exercised. Pan is assumed to match Mix A.
- **Hardware buttons:** pressed during the first pass, before the app pass. They show no tx, as expected; that pass is noted in `steps.txt`.
- **Sanity check:** an app click on IN1 MON sent 0 right after the device button had turned MON off. That suggests the app had a stale copy of the state.
