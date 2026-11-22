# TUI design reference

Agreed with Kyriakos on 2026-11-22. This is the target look for the `toppingctl` TUI. The
TUI merged in #13/#30 (a tall inputs panel with placeholders) gets replaced by this design.

## Run the mock

It's a standalone crate that draws fake animated signal and never opens the device. Run it in a
real terminal of at least 120×40 (the `!` prefix in Claude Code has no TTY):

```sh
cd docs/design/mock && cargo run --release        # q quits
MOCK_DUMP=120x40 cargo run --release -q           # print a frozen render (used for render-*.txt)
```

`render-120x40.txt` and `render-100x34.txt` are frozen renders at t = 1.3 s.

## Decisions

- **Arrangement:** a one-to-one copy of the official app's layout, done in our own style, with
  no TOPPING logo or artwork.
  - **Input (top left):** IN1 and IN2 strips. Each has a peak readout, a tall gradient meter with
    a dB scale, a stacked MON / 48V / INST / SOLO / MUTE / Ø column and a gain knob.
  - **Mixer (top right):** Mix A/B/C tabs set into the border. Strips IN 1+2 (two mono faders),
    PB 1/2, PB 3/4 and PB 5/6. Each has two pan knobs (one per channel), LINK, the value above
    the fader, an L meter · fader · R meter, and Ø / SOLO / MUTE stacked underneath.
  - **Loopback (bottom left):** LOOP 1+2 and 3+4, each with a source dropdown, LINK, the value,
    an L meter · fader · R meter and MUTE.
  - **Output (bottom right):** OUT 1+2 (source dropdown, LINK, fader and meters, HP / LINE), a
    MONITOR MIX knob (read-only and dimmed, mirroring the balance the device reports as `11 01`)
    and PHONE (the +17 dBu / 0 dBu level switch, `11 02`).
- **Style (btop-level, "pretty, not pretentious"):**
  - rounded dim borders, titles set into the border with the shortcut key highlighted
  - one accent colour (soft blue)
  - `■` and block meters with a green → amber → red gradient
  - pill toggles; 48V and MUTE light red when on
  - right-aligned fixed-width values, `−∞` for muted
- **Header:** a two-row gradient block wordmark `toppingctl`, the device and firmware, workspace ▾,
  sample rate ▾, `s` save, `l` load, `w` store to device, and a sync dot. It switches to a compact
  version below 128 columns.
- **Minimum size** is 100×34; smaller terminals get a "too small" message.
- **Mouse everywhere** (#13 amendment): click toggles and pills, scroll faders and knobs (Shift ×10),
  drag faders, click tabs and dropdowns. The mock itself has no mouse support.

## Open items for the real build

- **Knobs aren't right yet.** Kyriakos: "knobs are still not there". The plan is real dials: an
  arc of ticks lit up to the value, plus a pointer (ratatui `Canvas` with Braille markers), like
  the app's pan knobs. That replaces the `╭───╮ │ ↗ │` boxes.
- **Other gaps:** Kyriakos said "we got other things missing too" but didn't list them yet. Ask
  before re-refining.
- **Model gaps in `src/state.rs`:**
  - PB strips need a per-channel pan (two pans) plus LINK.
  - Loopback and output need LINK (L/R levels).
  - The sample rate isn't HID: it goes through CoreAudio `nsrt` on macOS and ALSA/PipeWire on
    Linux (PROTOCOL.md → Not HID). Its own story.
  - Buffer Size is Windows ASIO only and is left out.
- **Validation test** (agreed in principle, to be signed when the story is refined):
  1. golden test: with fixed fake state and frozen meters, the real TUI's 120×40 render matches the
     design layout (boxes, labels, positions, knob glyphs)
  2. every control sends its frame through `FakeTransport`
  3. 48V and store-to-device ask first; mouse click, scroll and drag work
  4. Kyriakos signs off on hardware
