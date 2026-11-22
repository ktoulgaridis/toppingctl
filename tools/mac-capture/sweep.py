import sys
tx=None; res={}
for l in open(sys.argv[1]):
    p=l.split(); d=p[1]; b=p[2:17]
    k=b[5]+':'+b[6]; v=''.join(b[7:11])
    if d=='tx': tx=(k,v); res.setdefault(k,[v,set()])
    else: res[tx[0]][1].add(f"{k}={v}")
for k,(v,rx) in res.items():
    echo = f"{k}={v}" in rx
    other = sorted(x for x in rx if x!=f"{k}={v}")
    print(f"{k}={v} {'ECHO' if echo else 'none'} {' '.join(other)}")
