# usage: meters.py <hidsend --all output>  -> per tx window, mean |value| of each meter key
import sys
from collections import defaultdict
win = None; out = []
for l in open(sys.argv[1]):
    p = l.split(); b = p[2:17]; k = b[5] + b[6]
    v = int(''.join(b[7:11]), 16)
    if v >= 1 << 31: v -= 1 << 32
    if p[1] == 'tx': win = (f"{b[5]}:{b[6]}={''.join(b[7:11])}", defaultdict(list)); out.append(win)
    elif win: win[1][k].append(v)
keys = ['2105','2205','3103','3104','3203','3204','3303','3304','4101','4102','4201','4202','4301','4302']
print('tx'.ljust(22) + ''.join(k.rjust(11) for k in keys))
for name, d in out:
    print(name.ljust(22) + ''.join((str(sum(d[k]) // len(d[k])) if d[k] else '-').rjust(11) for k in keys))
