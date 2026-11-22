import math, struct, sys, wave
# per-0.5 s RMS in dBFS for each channel of a 6ch S32_LE wav
w = wave.open(sys.argv[1]); ch, sr = w.getnchannels(), w.getframerate(); win = sr // 2; t = 0
while True:
    b = w.readframes(win)
    if len(b) < win * ch * 4: break
    s = struct.unpack(f'<{win*ch}i', b)
    db = [20*math.log10(max(1e-12, math.sqrt(sum(x*x for x in s[c::ch]) / win) / 2**31)) for c in range(ch)]
    print(f'{t:5.1f}s ' + ' '.join(f'{d:7.1f}' for d in db)); t += 0.5
