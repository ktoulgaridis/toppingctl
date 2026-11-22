# Anti-aliased knob images shown via the kitty graphics protocol. Usage: python3 -I hires.py
import math, base64, sys, zlib
ACC=(122,162,247); DIM=(59,66,97); BODY=(40,44,60); BODY2=(58,63,84); RIM=(130,136,160); PTR=(240,240,250); BG=(30,32,46)
START,SWEEP=225,270
def knob(S, v, ticks=0, dimmed=False, ss=3):
    px=bytearray()
    c=(S-1)/2; R=S/2
    va=math.radians(START-SWEEP*v)
    def col(x,y):
        dx,dy=x-c,c-y; d=math.hypot(dx,dy)/R
        a=math.degrees(math.atan2(dy,dx)); rel=(START-a)%360
        # pointer: distance from point to segment centre->tip
        tx,ty=math.cos(va)*0.50*R, math.sin(va)*0.50*R
        t=max(0,min(1,(dx*tx+dy*ty)/(tx*tx+ty*ty)))
        pd=math.hypot(dx-t*tx,dy-t*ty)/R
        if not dimmed and pd<0.045 and d<0.56: return PTR
        if 0.78<=d<=0.96 and rel<=SWEEP:
            if ticks and (rel/SWEEP*ticks)%1>0.6: return None
            return ACC if (rel<=SWEEP*v and not dimmed) else DIM
        if d<0.58:
            k=max(0,min(1,(dy/R+0.6)/1.2))   # top-lit gradient
            return tuple(int(BODY[i]+(BODY2[i]-BODY[i])*k) for i in range(3))
        if d<0.64: return RIM if not dimmed else DIM
        return None
    for y in range(S):
        for x in range(S):
            acc=[0,0,0,0]
            for sy in range(ss):
                for sx in range(ss):
                    k=col(x-0.5+(sx+0.5)/ss, y-0.5+(sy+0.5)/ss)
                    if k: acc[0]+=k[0];acc[1]+=k[1];acc[2]+=k[2];acc[3]+=1
            n=ss*ss
            if acc[3]: px+=bytes([acc[0]//acc[3],acc[1]//acc[3],acc[2]//acc[3],255*acc[3]//n])
            else: px+=bytes([0,0,0,0])
    return px
def show(px,S,cols,rows):
    data=base64.b64encode(zlib.compress(bytes(px))).decode()
    chunks=[data[i:i+4096] for i in range(0,len(data),4096)]
    for i,ch in enumerate(chunks):
        m=1 if i<len(chunks)-1 else 0
        head=f"a=T,f=32,o=z,s={S},v={S},c={cols},r={rows},C=1," if i==0 else ""
        sys.stdout.write(f"\x1b_G{head}m={m};{ch}\x1b\\")
def row(title, S, cols, rows, items, **kw):
    print("\n"+title)
    for v,_ in items:
        show(knob(S,v,**kw),S,cols,rows); sys.stdout.write("\x1b["+str(cols+3)+"C")
    print("\n"*rows, end="")
    print("".join(f"{l:^{cols+3}}" for _,l in items))
row("I  input gain, 12×6 cells, solid arc",120,12,6,[(0,"+0 dB"),(0.5,"+10 dB"),(0.8,"+16 dB")])
row("J  input gain, 12×6 cells, ticked arc",120,12,6,[(0,"+0 dB"),(0.5,"+10 dB"),(0.8,"+16 dB")],ticks=21)
row("K  pan, 8×4 cells",80,8,4,[(0,"L100"),(0.5,"C"),(1,"R100")])
row("L  monitor mix, dimmed",120,12,6,[(0.5,"input  playback")],dimmed=True)
print()
