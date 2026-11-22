# Full-screen toppingctl mockup: real text + kitty-graphics pixel widgets.
# Run in kitty or Ghostty, terminal >= 120x40:  python3 -I mockup.py
import math, base64, zlib, sys

CW, CH = 12, 24                      # pixels per cell used for rendering (scaled by the terminal)
BG    = (26, 27, 38)
TXT   = (192, 202, 245)
DIM   = (86, 95, 137)
FAINT = (59, 66, 97)
ACC   = (122, 162, 247)
RED   = (247, 118, 142)
GREEN = (158, 206, 106)
AMBER = (224, 175, 104)
CHIP  = (41, 46, 66)
TRACK = (36, 40, 58)

# ---------------------------------------------------------------- raster
def clamp(v, a=0.0, b=1.0): return a if v < a else b if v > b else v

class Img:
    def __init__(s, cols, rows):
        s.cols, s.rows = cols, rows
        s.w, s.h = cols * CW, rows * CH
        s.p = [0.0] * (s.w * s.h * 4)
    def blend(s, x, y, c, a):
        if a <= 0: return
        i = (y * s.w + x) * 4; p = s.p
        da = p[i + 3]; oa = a + da * (1 - a)
        for k in range(3):
            p[i + k] = (c[k] * a + p[i + k] * da * (1 - a)) / oa
        p[i + 3] = oa
    def fill(s, sdf, box, color, alpha=1.0):
        x0, y0, x1, y1 = box
        x0 = max(0, int(x0) - 1); y0 = max(0, int(y0) - 1)
        x1 = min(s.w, int(x1) + 2); y1 = min(s.h, int(y1) + 2)
        fn = callable(color)
        for y in range(y0, y1):
            py = y + 0.5
            for x in range(x0, x1):
                d = sdf(x + 0.5, py)
                a = clamp(0.5 - d)
                if a > 0:
                    s.blend(x, y, color(x + 0.5, py) if fn else color, a * alpha)
    def bytes(s):
        p = s.p
        return bytes(int(clamp(p[i] * 255, 0, 255)) if i % 4 == 3 else int(clamp(p[i], 0, 255)) for i in range(len(p)))

def circle(cx, cy, r):
    return (lambda x, y: math.hypot(x - cx, y - cy) - r), (cx - r, cy - r, cx + r, cy + r)
def ring(cx, cy, r, t):
    return (lambda x, y: abs(math.hypot(x - cx, y - cy) - r) - t / 2), (cx - r - t, cy - r - t, cx + r + t, cy + r + t)
def capsule(ax, ay, bx, by, r):
    dx, dy = bx - ax, by - ay; L = dx * dx + dy * dy or 1e-9
    def f(x, y):
        t = clamp(((x - ax) * dx + (y - ay) * dy) / L)
        return math.hypot(x - ax - t * dx, y - ay - t * dy) - r
    return f, (min(ax, bx) - r, min(ay, by) - r, max(ax, bx) + r, max(ay, by) + r)
def rrect(x0, y0, x1, y1, rad):
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2; hx, hy = (x1 - x0) / 2 - rad, (y1 - y0) / 2 - rad
    def f(x, y):
        qx, qy = abs(x - cx) - hx, abs(y - cy) - hy
        return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - rad
    return f, (x0, y0, x1, y1)

def mix(a, b, t): return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))

# ---------------------------------------------------------------- widgets
def knob(cols, rows, v, dim=False, ticks=21):
    im = Img(cols, rows); S = min(im.w, im.h); cx, cy = im.w / 2, im.h / 2; R = S / 2 * 0.96
    for i in range(ticks):
        rel = i / (ticks - 1) * 270; a = math.radians(225 - rel)
        lit = (not dim) and rel <= 270 * v + 1e-6
        f, b = capsule(cx + math.cos(a) * 0.80 * R, cy - math.sin(a) * 0.80 * R,
                       cx + math.cos(a) * 0.94 * R, cy - math.sin(a) * 0.94 * R, 0.035 * R)
        im.fill(f, b, ACC if lit else (52, 57, 82))
    top, bot = ((70, 76, 102), (34, 37, 52)) if not dim else ((48, 52, 70), (32, 34, 46))
    f, b = circle(cx, cy, 0.62 * R)
    im.fill(f, b, lambda x, y: mix(top, bot, clamp((y - (cy - 0.62 * R)) / (1.24 * R))))
    f, b = ring(cx, cy, 0.62 * R, 0.035 * R)
    im.fill(f, b, (118, 124, 150) if not dim else (62, 66, 88))
    if not dim:
        a = math.radians(225 - 270 * v)
        f, b = capsule(cx + math.cos(a) * 0.12 * R, cy - math.sin(a) * 0.12 * R,
                       cx + math.cos(a) * 0.52 * R, cy - math.sin(a) * 0.52 * R, 0.045 * R)
        im.fill(f, b, (240, 240, 250))
    return im

def meter_color(frac_from_top):
    if frac_from_top < 0.10: return RED
    if frac_from_top < 0.30: return mix(AMBER, RED, clamp((0.30 - frac_from_top) / 0.20) ** 2)
    if frac_from_top < 0.45: return mix(GREEN, AMBER, clamp((0.45 - frac_from_top) / 0.15))
    return GREEN

def bar(im, x0, x1, top, bottom, level_frac):
    """Vertical meter bar; level_frac = fraction from top where signal reaches (1 = empty)."""
    f, b = rrect(x0, top, x1, bottom, (x1 - x0) / 2)
    im.fill(f, b, TRACK)
    ly = top + (bottom - top) * clamp(level_frac)
    if ly < bottom - 1:
        f, b = rrect(x0, ly, x1, bottom, (x1 - x0) / 2)
        im.fill(f, b, lambda x, y: meter_color((y - top) / (bottom - top)))
        f, b = capsule(x0 + 1, ly, x1 - 1, ly, 1.2)          # peak line
        im.fill(f, b, (235, 235, 245))

DB_STOPS = [(0, 0.06), (-6, 0.30), (-12, 0.50), (-24, 0.76), (None, 1.0)]
def db_frac(db):
    if db is None or db <= -60: return 1.0
    for (d0, f0), (d1, f1) in zip(DB_STOPS, DB_STOPS[1:]):
        d1v = -60 if d1 is None else d1
        if db >= d1v:
            return f0 + (f1 - f0) * (d0 - db) / (d0 - d1v)
    return 1.0

def fader(cols, rows, val_db, levels, track_col, meter_cols):
    """levels: list of meter dB values aligned with meter_cols."""
    im = Img(cols, rows); top, bot = CH * 0.5, im.h - CH * 0.5
    tx = track_col * CW + CW / 2
    for db, _ in DB_STOPS:                                 # tick marks on both sides
        r = round(db_frac(db) * (rows - 1)); y = r * CH + CH / 2
        for s in (-1, 1):
            f, b = capsule(tx + s * 9, y, tx + s * 16, y, 1.0); im.fill(f, b, FAINT)
    f, b = rrect(tx - 2.5, top, tx + 2.5, bot, 2.5); im.fill(f, b, (52, 57, 80))
    for mc, lv in zip(meter_cols, levels):
        mx = mc * CW + CW / 2
        bar(im, mx - 3.5, mx + 3.5, top, bot, db_frac(lv) if lv is not None else 1.0)
    y = top + (bot - top) * db_frac(val_db)
    f, b = rrect(tx - 17, y - 8, tx + 17, y + 8, 4)
    im.fill(f, b, lambda x, yy: mix((222, 226, 240), (140, 146, 170), clamp((yy - (y - 8)) / 16)))
    f, b = capsule(tx - 12, y, tx + 12, y, 1.1); im.fill(f, b, ACC)
    return im

def input_meter(cols, rows, db):
    im = Img(cols, rows); top, bot = CH * 0.5, im.h - CH * 0.5; cx = im.w / 2
    bar(im, cx - 6, cx + 6, top, bot, clamp(-db / 60) if db is not None else 1.0)
    return im

def clip_light(lit):
    im = Img(2, 1); cx, cy = im.w / 2, im.h / 2
    if lit:
        for k in range(6, 0, -1):
            f, b = circle(cx, cy, 5 + k * 1.3); im.fill(f, b, RED, 0.08)
    f, b = circle(cx, cy, 5); im.fill(f, b, RED if lit else (60, 44, 56))
    return im

def chip(cols, color, outline=None, rad=7):
    im = Img(cols, 1)
    f, b = rrect(1.5, 3, im.w - 1.5, im.h - 3, rad); im.fill(f, b, color)
    if outline:
        f, b = rrect(1.5, 3, im.w - 1.5, im.h - 3, rad)
        im.fill(lambda x, y: abs(f(x, y)) - 0.6, b, outline)
    return im

# ---------------------------------------------------------------- screen
W, H = 120, 40
chars = [[" "] * W for _ in range(H)]
fgs = [[TXT] * W for _ in range(H)]
bolds = [[False] * W for _ in range(H)]
images = []                                      # (x, y, Img, z)

def put(x, y, s, fg=TXT, bold=False):
    for i, ch in enumerate(s):
        if 0 <= x + i < W and 0 <= y < H:
            chars[y][x + i] = ch; fgs[y][x + i] = fg; bolds[y][x + i] = bold
def center(x, w, y, s, fg=TXT, bold=False): put(x + (w - len(s)) // 2, y, s, fg, bold)
def rput(x_end, y, s, fg=TXT, bold=False): put(x_end - len(s) + 1, y, s, fg, bold)
def img(x, y, im, z=0): images.append((x, y, im, z))

def frame(x, y, w, h, title=None, key=None, col=FAINT):
    put(x, y, "╭" + "─" * (w - 2) + "╮", col); put(x, y + h - 1, "╰" + "─" * (w - 2) + "╯", col)
    for r in range(y + 1, y + h - 1): put(x, r, "│", col); put(x + w - 1, r, "│", col)
    if title:
        put(x + 1, y, "┤", col); put(x + 2, y, key, ACC, True); put(x + 3, y, title, TXT, True)
        put(x + 3 + len(title), y, "├", col)

def pill(x, y, w, label, on=False, color=ACC):
    img(x, y, chip(w, color if on else CHIP), -1)
    center(x, w, y, label, BG if on else DIM, on)

def tab(x, y, w, label):
    img(x, y, chip(w, CHIP, rad=5), -1); center(x, w, y, label, TXT, True)

def fmt_db(db, unit=True):
    if db is None: return "−∞ dB" if unit else "−∞"
    s = ("−" if db <= 0 else "+") + f"{abs(db):.1f}"
    return s + (" dB" if unit else "")

# header
frame(0, 0, W, 3)
word = "toppingctl"
for i, ch in enumerate(word):
    put(2 + i, 1, ch, mix((187, 154, 247), ACC, i / (len(word) - 1)), True)
put(14, 1, "E2x2", TXT, True); put(19, 1, "fw 1.13", DIM)
segs = [("workspace ", DIM, False), ("default ▾", TXT, True), ("  │  ", FAINT, False),
        ("48 kHz ▾", TXT, True), ("  │  ", FAINT, False),
        ("s", ACC, True), (" save  ", DIM, False), ("l", ACC, True), (" load  ", DIM, False),
        ("w", ACC, True), (" store", DIM, False), ("  │  ", FAINT, False), ("●", GREEN, False), (" synced", DIM, False)]
x = 118 - sum(len(t) for t, _, _ in segs)
for t, c, bb in segs: put(x, 1, t, c, bb); x += len(t)

# ---------------- input panel (x0..40, y3..24)
frame(0, 3, 41, 22, "input", "¹", ACC)
inputs = [dict(name="IN1", pk=-12.4, gain=10, mon=True, p48=False, inst=False, solo=False, mute=False, ph=False, clip=False),
          dict(name="IN2", pk=-0.6, gain=16, mon=False, p48=True, inst=False, solo=False, mute=True, ph=False, clip=True)]
for k, ch in enumerate(inputs):
    sx = 2 + k * 19
    tab(sx, 4, 17, ch["name"])
    img(sx, 5, clip_light(ch["clip"]))
    put(sx + 3, 5, f"{fmt_db(ch['pk'], False)} pk", RED if ch["pk"] > -3 else GREEN, True)
    img(sx, 6, input_meter(2, 17, ch["pk"]))
    for db in (0, -6, -12, -24, -36, -48, -60):
        r = 6 + round(-db / 60 * 16); rput(sx + 5, r, ("−" if db else "") + str(abs(db)), DIM)
    for i, (lab, key, colr) in enumerate([("MON", "mon", ACC), ("48V", "p48", RED), ("INST", "inst", ACC),
                                          ("SOLO", "solo", ACC), ("MUTE", "mute", RED), ("Ø", "ph", ACC)]):
        pill(sx + 8, 6 + i + (1 if i >= 3 else 0), 8, lab, ch[key], colr)
    img(sx + 6, 15, knob(10, 5, ch["gain"] / 20))
    center(sx + 6, 10, 20, f"+{ch['gain']} dB" if ch["gain"] else "−0 dB", TXT, True)
    put(sx + 7, 21, "gain", DIM)

# ---------------- mixer panel (x41..119, y3..24)
frame(41, 3, 79, 22, "mixer", "²")
put(51, 3, "┤", FAINT)
img(52, 3, chip(9, GREEN, rad=5), -1); put(52, 3, " a mix A ", BG, True); put(53, 3, "a", BG, True)
put(61, 3, "  b mix B  ", ACC); put(63, 3, "b", ACC, True)
put(72, 3, "c mix C ", AMBER); put(72, 3, "c", AMBER, True)
put(80, 3, "├", FAINT)
strips = [dict(name="IN 1+2", pans=(0.30, 0.70), pl=("L40", "R40"), link=False, dbs=(-3.0, None), lv=(-14, -60), mono=True),
          dict(name="PB 1/2", pans=(0.0, 1.0), pl=("L100", "R100"), link=True, db=0.0, lv=(-9, -11)),
          dict(name="PB 3/4", pans=(0.20, 0.80), pl=("L60", "R60"), link=True, db=-12.0, lv=(-20, -18)),
          dict(name="PB 5/6", pans=(0.0, 1.0), pl=("L100", "R100"), link=True, db=None, lv=(None, None))]
for k, st in enumerate(strips):
    sx = 43 + k * 19
    tab(sx, 4, 17, st["name"])
    img(sx + 1, 5, knob(6, 3, st["pans"][0])); img(sx + 10, 5, knob(6, 3, st["pans"][1]))
    center(sx + 1, 6, 8, st["pl"][0], TXT, True); center(sx + 10, 6, 8, st["pl"][1], TXT, True)
    if st.get("mono"):
        center(sx, 17, 10, "LINK", FAINT)
        put(sx + 1, 11, fmt_db(st["dbs"][0], False).rjust(6), TXT, True)
        put(sx + 10, 11, "muted".rjust(6) if st["dbs"][1] is None else fmt_db(st["dbs"][1], False), DIM)
        img(sx, 12, fader(8, 10, st["dbs"][0], [st["lv"][0]], 5, [0]))
        img(sx + 9, 12, fader(8, 10, st["dbs"][1], [st["lv"][1]], 2, [7]))
        for db, _ in DB_STOPS:
            r = 12 + round(db_frac(db) * 9)
            lab = "−∞" if db is None else ("−" if db else "") + str(abs(db))
            rput(sx + 3, r, lab, DIM); put(sx + 13, r, lab, DIM)
        for half, muted in ((0, False), (9, True)):
            for j, lab in enumerate(("Ø", "S", "M")):
                pill(sx + half + j * 3, 22, 3, lab, muted and lab == "M", RED if lab == "M" else ACC)
    else:
        pill(sx + 5, 10, 7, "LINK", st["link"])
        center(sx, 17, 11, fmt_db(st["db"]), TXT, True)
        img(sx, 12, fader(17, 10, st["db"], list(st["lv"]), 8, [0, 16]))
        for db, _ in DB_STOPS:
            r = 12 + round(db_frac(db) * 9)
            lab = "−∞" if db is None else ("−" if db else "") + str(abs(db))
            rput(sx + 5, r, lab, DIM); put(sx + 11, r, lab, DIM)
        for j, lab in enumerate(("Ø", "SOLO", "MUTE")):
            pill(sx + j * 6, 22, 5, lab, False)

# ---------------- loopback panel (x0..40, y25..38)
frame(0, 25, 41, 14, "loopback", "³")
loops = [dict(name="LOOP 1+2", src="PB 3/4", link=True, mute=False, db=0.0, lv=(-16, -15)),
         dict(name="LOOP 3+4", src="PB 5/6", link=True, mute=True, db=-6.0, lv=(None, None))]
for k, lp in enumerate(loops):
    sx = 2 + k * 19
    tab(sx, 26, 17, lp["name"])
    center(sx, 17, 27, lp["src"] + " ▾", ACC)
    pill(sx + 1, 28, 7, "LINK", lp["link"]); pill(sx + 9, 28, 7, "MUTE", lp["mute"], RED)
    center(sx, 17, 29, fmt_db(lp["db"]), TXT, True)
    img(sx, 30, fader(17, 8, lp["db"], list(lp["lv"]), 8, [0, 16]))
    for db, _ in DB_STOPS:
        r = 30 + round(db_frac(db) * 7)
        lab = "−∞" if db is None else ("−" if db else "") + str(abs(db))
        rput(sx + 5, r, lab, DIM); put(sx + 11, r, lab, DIM)

# ---------------- output panel (x41..119, y25..38)
frame(41, 25, 79, 14, "output", "⁴")
sx = 43
tab(sx, 26, 26, "OUT 1+2")
center(sx, 26, 27, "mix A ▾", GREEN)
pill(sx + 2, 28, 7, "LINK", True); pill(sx + 10, 28, 6, "HP", True); pill(sx + 17, 28, 7, "LINE", True)
center(sx, 26, 29, fmt_db(0.0), TXT, True)
img(sx + 4, 30, fader(17, 8, 0.0, [-8, -10], 8, [0, 16]))
for db, _ in DB_STOPS:
    r = 30 + round(db_frac(db) * 7)
    lab = "−∞" if db is None else ("−" if db else "") + str(abs(db))
    rput(sx + 9, r, lab, DIM); put(sx + 15, r, lab, DIM)
sx = 71
tab(sx, 26, 22, "MONITOR MIX")
img(sx + 5, 28, knob(12, 6, 0.5, dim=True))
put(sx + 2, 34, "input", DIM); rput(sx + 19, 34, "playback", DIM)
center(sx, 22, 36, "knob on device only", FAINT)
sx = 95
tab(sx, 26, 23, "PHONE")
pill(sx + 7, 28, 9, "GAIN", False)
center(sx, 23, 30, "level  0 dBu", TXT)
img(sx + 7, 31, knob(10, 5, 0.73))
center(sx, 23, 36, "volume  73", DIM)

# footer
foot = [("tab", "panel"), ("1-4", "jump"), ("↑↓←→", "move"), ("⇧", "×10"), ("space", "toggle"),
        ("0", "reset"), ("a b c", "mix"), ("s", "save"), ("w", "store"), ("?", "help"), ("q", "quit")]
x = 1
for k, lab in foot:
    put(x, 39, k, ACC, True); x += len(k) + 1; put(x, 39, lab, DIM); x += len(lab) + 3

# ---------------------------------------------------------------- emit
out = ["\x1b[2J\x1b[H\x1b[?25l"]
def sgr(c, b): return f"\x1b[{'1;' if b else '22;'}38;2;{int(c[0])};{int(c[1])};{int(c[2])}m"
for y in range(H):
    out.append(f"\x1b[{y + 1};1H")
    last = None
    for x in range(W):
        key = (fgs[y][x], bolds[y][x])
        if key != last: out.append(sgr(*key)); last = key
        out.append(chars[y][x])
out.append("\x1b[0m")
sys.stdout.write("".join(out)); sys.stdout.flush()
for x, y, im, z in images:
    data = base64.b64encode(zlib.compress(im.bytes())).decode()
    chunks = [data[i:i + 4096] for i in range(0, len(data), 4096)]
    sys.stdout.write(f"\x1b[{y + 1};{x + 1}H")
    for i, ch in enumerate(chunks):
        m = 1 if i < len(chunks) - 1 else 0
        head = f"a=T,f=32,o=z,s={im.w},v={im.h},c={im.cols},r={im.rows},C=1,q=2,z={z}," if i == 0 else ""
        sys.stdout.write(f"\x1b_G{head}m={m};{ch}\x1b\\")
    sys.stdout.flush()
sys.stdout.write(f"\x1b[{H + 1};1H\x1b[?25h\n")
