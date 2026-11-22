import math, sys
ACC=(122,162,247); DIM=(59,66,97); BODY=(44,48,66); RIM=(120,126,150); PTR=(235,235,245); TXT=(200,200,210)
def fg(c): return f"\x1b[38;2;{c[0]};{c[1]};{c[2]}m"
def bg(c): return f"\x1b[48;2;{c[0]};{c[1]};{c[2]}m"
RST="\x1b[0m"
START, SWEEP = 225, 270   # degrees, clockwise from 7:30 to 4:30

def pix(w,h,v,ticks=None,dimmed=False,pointer=True):
    """v in 0..1; returns grid[y][x] of colour or None. Square pixels."""
    cx,cy=(w-1)/2,(h-1)/2; R=min(w,h)/2
    g=[[None]*w for _ in range(h)]
    va=START-SWEEP*v
    acc = DIM if dimmed else ACC
    for y in range(h):
        for x in range(w):
            dx,dy=x-cx,cy-y; d=math.hypot(dx,dy)/R
            a=math.degrees(math.atan2(dy,dx))
            rel=(START-a)%360    # 0..360 clockwise from start
            if 0.80<=d<=1.0 and rel<=SWEEP:
                if ticks and (rel/SWEEP*ticks)%1>0.55: continue
                g[y][x]= acc if rel<=SWEEP*v and not dimmed else DIM
            elif d<0.58: g[y][x]=BODY
            elif d<0.68: g[y][x]=RIM
    if pointer and not dimmed:
        for i in range(40):
            t=i/39*0.62*R
            x=round(cx+t*math.cos(math.radians(va))); y=round(cy-t*math.sin(math.radians(va)))
            if 0<=x<w and 0<=y<h: g[y][x]=PTR
    return g

def halfblock(g):
    out=[]
    for y in range(0,len(g),2):
        s=""
        for x in range(len(g[0])):
            t=g[y][x]; b=g[y+1][x] if y+1<len(g) else None
            if t is None and b is None: s+=RST+" "
            elif t is None: s+=RST+fg(b)+"▄"
            elif b is None: s+=RST+fg(t)+"▀"
            else: s+=RST+fg(t)+bg(b)+"▀"
        out.append(s+RST)
    return out

def braille(g):
    BITS=[(0,0,0x01),(0,1,0x02),(0,2,0x04),(1,0,0x08),(1,1,0x10),(1,2,0x20),(0,3,0x40),(1,3,0x80)]
    pri={PTR:4,ACC:3,RIM:2,DIM:1,BODY:0}
    out=[]
    for cy in range(0,len(g),4):
        s=""
        for cx in range(0,len(g[0]),2):
            m=0;best=None
            for dx,dy,bit in BITS:
                y,x=cy+dy,cx+dx
                if y<len(g) and x<len(g[0]) and g[y][x] is not None and g[y][x]!=BODY:
                    m|=bit; c=g[y][x]
                    if best is None or pri[c]>pri[best]: best=c
            s+= (fg(best)+chr(0x2800+m)) if m else " "
        out.append(s+RST)
    return out

def show(title, render, size, vals, labels, **kw):
    print(f"\n{fg(TXT)}{title}{RST}")
    blocks=[render(pix(size[0],size[1],v,**kw)) for v in vals]
    width=max(len(b) for b in blocks)
    for row in range(max(len(b) for b in blocks)):
        print("   ".join(b[row] if row<len(b) else "" for b in blocks))
    cols=size[0] if render is halfblock else size[0]//2
    print("   ".join(f"{fg(TXT)}{l:^{cols}}{RST}" for l in labels))

vals=[0,0.5,0.8]; lab=["+0 dB","+10 dB","+16 dB"]
show("A  half-block, solid arc  (input gain, 16×8 cells)", halfblock,(16,16),vals,lab)
show("B  half-block, ticked arc (input gain, 16×8 cells)", halfblock,(16,16),vals,lab,ticks=21)
show("C  braille, solid arc     (input gain, 10×5 cells)", braille,(20,20),vals,lab)
show("D  braille, ticked arc    (input gain, 10×5 cells)", braille,(20,20),vals,lab,ticks=21)
pv=[0,0.5,1]; pl=["L100","C","R100"]
show("E  half-block pan (10×5 cells)", halfblock,(10,10),pv,pl)
show("F  half-block pan, ticked (10×5 cells)", halfblock,(10,10),pv,pl,ticks=11)
show("G  braille pan (6×3 cells)", braille,(12,12),pv,pl)
show("H  monitor mix, dimmed / unknown (16×8)", halfblock,(16,16),[0.5],["input   playback"],dimmed=True)
