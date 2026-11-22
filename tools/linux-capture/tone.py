import math, struct, sys, wave
# 6ch S32_LE 48k, 1 kHz sine at -30 dBFS peak on PB1/2 (or on all six channels with "all")
# usage: tone.py out.wav SECONDS [all]
sr, secs, amp = 48000, int(sys.argv[2]), 10**(-30/20) * (2**31 - 1)
every = len(sys.argv) > 3 and sys.argv[3] == 'all'
w = wave.open(sys.argv[1], 'wb'); w.setnchannels(6); w.setsampwidth(4); w.setframerate(sr)
frames = bytearray()
for n in range(sr):  # one second, repeated (1 kHz fits exactly)
    v = int(amp * math.sin(2 * math.pi * 1000 * n / sr))
    frames += struct.pack('<6i', *([v] * 6 if every else [v, v, 0, 0, 0, 0]))
for _ in range(secs): w.writeframes(frames)
w.close()
