import sys
lo = float(sys.argv[1])
meter = {('21','05'),('22','05'),('31','03'),('31','04'),('32','03'),('32','04'),('33','03'),('33','04'),('41','01'),('41','02'),('42','01'),('42','02'),('43','01'),('43','02'),('00','00')}
last = {}
for l in open(sys.argv[2]):
    p = l.split()
    if len(p) < 15 or p[1] not in ('rx','tx'): continue
    t = float(p[0])
    if t < lo: continue
    b = p[2:]; k = (b[5], b[6])
    if p[1] == 'rx' and k in meter: continue
    v = int(''.join(b[7:11]), 16)
    if v >= 1 << 31: v -= 1 << 32
    if p[1] == 'rx' and last.get(k) == v: continue
    if p[1] == 'rx': last[k] = v
    print(f"{t%10000:.3f} {p[1]} {k[0]} {k[1]} {v:#x}  crc={b[11]}{b[12]}")
