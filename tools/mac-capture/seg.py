import sys
# usage: seg.py <from-epoch> <to-epoch> <capture file>
lo, hi = float(sys.argv[1]), float(sys.argv[2])
fr = []
for l in open(sys.argv[3]):
    p = l.split()
    if len(p) < 17 or p[1] != 'tx': continue
    t = float(p[0])
    if not (lo <= t < hi): continue
    b = p[2:]
    v = int(''.join(b[7:11]), 16)
    if v >= 1 << 31: v -= 1 << 32
    fr.append((t, b[5] + ' ' + b[6], v))
segs, cur = [], []
for f in fr:
    if cur and f[0] - cur[-1][0] > 0.35:
        segs.append(cur); cur = []
    cur.append(f)
if cur: segs.append(cur)
for s in segs:
    regs = {}
    for t, r, v in s: regs.setdefault(r, []).append(v)
    desc = '; '.join(f"{r}: {vs[0]:#x}->{vs[-1]:#x} (n={len(vs)}, min={min(vs):#x}, max={max(vs):#x})" if len(vs) > 1 else f"{r}={vs[0]:#x}" for r, vs in regs.items())
    print(f"{s[0][0]%10000:.2f}-{s[-1][0]%10000:.2f}  {desc}")
