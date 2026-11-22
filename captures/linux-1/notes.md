# Linux capture 1: replay checks with `hidsend`

2026-11-22, Arch Linux, kernel 7.3.5. The same E2x2 as the Mac captures (hardware V1.14,
firmware V1.13). `hidsend` was run with `sudo` (no udev rule yet, #19). Nothing was plugged into IN2.

## Echo checks (`echo.txt`, `48v-in2.txt`)

Every write below came back as an `rx` of the same register and value, with a valid CRC,
70–330 ms later, and the front-panel LED followed.

| Write | Control |
|---|---|
| `21:01=1`, `21:01=0` | IN1 MON |
| `21:03=1`, `21:03=0` | IN1 INST |
| `11:02=0`, `11:02=1` | Phone Gain (ends on) |
| `--crc 22:01=1`, `22:01=0` | IN2 MON, with the CRC16 filled in |
| `--allow-48v 22:02=1`, `22:02=0` | IN2 48V |

With `--crc`, the device's echo matched our frame byte for byte, so the CRC16 formula holds
for host→device frames too.

## Level checks

A 1 kHz tone at −30 dBFS (peak) was played on all six playback channels with `aplay -D hw:E2x2`,
while `arecord` recorded all six capture channels (IN1, IN2, LB1, LB2, LB3, LB4).
`tools/linux-capture/tone.py rec.wav 45 all` makes the tone, and `rms.py` prints RMS per 0.5 s window.

`levels-3.txt` (writes held for 3 s each):

| Write | Channel | Before | During | After |
|---|---|---|---|---|
| `32:01=0x1000000` | LB1 only | −33.0 dB | −39.0 dB | −33.0 dB |
| `33:02=0x1000000` | LB4 only | −33.0 dB | −39.0 dB | −33.0 dB |

The other channels did not move. That confirms the level encoding (half of `0x2000000` is −6 dB),
the L/R and 1+2/3+4 mapping, and that level writes take effect without an echo.

`levels-2.txt` (`hidsend --all`): the loopback meters `32 03/04` read about −299 (≈ −30 dB) with
source `32:05=0x100` (PB1/2) and dropped to about −960 after `32:05=0x200`. The output meters
`31 03/04` stayed at −299 throughout: they sit before the output level.

`levels.txt` and the recordings behind `levels-2.txt` are **invalid as level checks**: the ALSA
capture switch was off (see the gotcha below), so every recorded sample was zero.

## Not verified

- **Output 1+2 level (`31 01/02`):** it drives the line outs, which nothing was listening to.
  Writing `31:01=0x1000000` made no audible change in the headphones, which suggests the
  headphones are not fed through the output level.

## Gotcha: the ALSA capture switch

The E2x2's ALSA control `Mic`,0 (capture switch, 6 channels) was **off**, so every capture
returned exact zeros. PipeWire turns it off again whenever the card profile changes. Turn it on
after taking the card from PipeWire:

```sh
pactl set-card-profile alsa_card.usb-Topping_E2x2-00 off
amixer -c E2x2 sset 'Mic',0 cap
```
