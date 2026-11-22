import sys
def crc(d):
    c = 0
    for b in d:
        c ^= b
        for _ in range(8):
            c = (c >> 1) ^ 0xA001 if c & 1 else c >> 1
    return c ^ 0x785A
ok = bad = 0
for l in open(sys.argv[1]):
    p = l.split()
    if len(p) < 15 or p[1] != 'rx': continue
    b = bytes(int(x, 16) for x in p[2:])
    if crc(b[:11]) == (b[11] << 8 | b[12]): ok += 1
    else: bad += 1
print("rx crc ok", ok, "bad", bad)
