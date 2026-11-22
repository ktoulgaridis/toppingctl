# Linux capture tooling (#8)

| file | what |
|---|---|
| `tone.py` | `tone.py out.wav SECONDS [all]`: 6-channel S32_LE 48 kHz wav, 1 kHz sine at −30 dBFS peak on PB1/2, or on all six channels with `all` |
| `rms.py` | `rms.py rec.wav`: RMS in dBFS per channel for each 0.5 s window of a 6-channel S32_LE recording |

Turn the ALSA capture switch on first (see `captures/linux-1/notes.md`), or every recording is silent.
