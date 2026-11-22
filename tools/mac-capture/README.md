# Mac capture tooling (#8)

Our own scripts used for the macOS captures in `captures/mac-*`. Nothing here comes from TOPPING's app.
Run Frida against an ad-hoc re-signed **copy** of the app, never the installed one, and never commit the app.

| file | what |
|---|---|
| `hidlog.js` | Frida: logs `IOHIDDeviceSetReport` (host→device, `tx`) |
| `hidrx.js` | Frida: `tx` plus `rx`, by wrapping the app's input-report callback. Use this on macOS, because `hidsniff` can't read while the app is running |
| `hidlog2.js` | Frida: CoreAudio `AudioObjectSetPropertyData` (sample rate) and the bundled vendor-request API |
| `step.sh` | adds a timestamped label to `$STEPS` |
| `seg.py` | groups `tx` frames into click gestures |
| `both.py` | interleaves non-meter `rx` and `tx` |
| `sweep.py` | echo table for a `hidsend` sweep |
| `meters.py` | mean meter values for each `hidsend --all` window |
| `crc.py` | checks CRC16 on every `rx` frame |

Example: `frida -f ~/re/TPCC.app/Contents/MacOS/ToppingPro -l hidrx.js -q -t inf > txrx.txt`

**Gotcha:** the app checks that only one copy is running. If it exits at launch with "There are running instances", delete `$TMPDIR/TOPPINGPROSocket` and retry after a few seconds.
