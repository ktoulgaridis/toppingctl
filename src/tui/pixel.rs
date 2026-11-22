//! Pixel widgets for terminals with the kitty graphics protocol (kitty,
//! Ghostty): knobs, faders, level meters, clip lights and chips (pills, mix
//! tabs, strip titles), drawn anti-aliased and placed directly with kitty
//! graphics commands. The look follows `docs/design/mockup.py`; the knobs
//! follow `docs/design/knobs/hires.py`.
//!
//! The text UI is always drawn first and records a [`PixelSpot`] for each
//! widget. [`PixelLayer::update`] then works out what changed since the
//! last frame and queues the kitty commands for it; the caller writes them
//! ([`PixelLayer::take_output`]) after the text frame, in one write.
//! [`clear_under`] blanks the text form of each placed widget, keeping the
//! labels of those drawn under the text (z=-1: pills, tabs, chips and the
//! faders' scales). Text stays real terminal text.
//!
//! - Each image is sent once (`a=t`, zlib RGBA, in ≤4096-byte base64
//!   chunks) under its own id and cached by what it shows (kind, size in
//!   cells, quantised value); placing it is a short `a=p` command.
//! - A placement is tied to its cells (one placement id per cell area). A
//!   new value places the new image there, then deletes the old placement:
//!   no gap, no flicker. An unchanged frame writes nothing.
//! - A resize, a too-small terminal or a modal deletes the placements; a
//!   full cache frees the terminal's copies and starts over; quitting frees
//!   every image ([`PixelLayer::FREE_ALL`]).
//! - At most [`TRANSMIT_BUDGET`] bytes of image data go out per frame, so
//!   a first frame never stalls the event loop; a widget whose image is
//!   not sent yet keeps its text form until a later frame.
//!
//! Without the protocol (or with `TOPPINGCTL_PIXELS=0`) there is no layer:
//! the text widgets stay, and so do the golden snapshots.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::Write as _;

use image::RgbaImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color as TermColor;
use ratatui_image::picker::{Picker, ProtocolType};
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LinearGradient, Paint, Path, PathBuilder, Pixmap,
    Point, SpreadMode, Stroke, Transform,
};

use crate::tui::theme::Theme;
use crate::tui::widgets::dial::{lit_span, START_DEG, SWEEP_DEG};
use crate::tui::widgets::fader::{fader_frac, fader_row, Fader};
use crate::tui::widgets::meter::meter_frac;

/// The knobs' palette (docs/design/knobs/hires.py, style J).
pub const KNOB_ACCENT_RGB: (u8, u8, u8) = (122, 162, 247);
pub const KNOB_DIM_RGB: (u8, u8, u8) = (59, 66, 97);
pub const KNOB_BODY_RGB: (u8, u8, u8) = (40, 44, 60);
pub const KNOB_BODY_TOP_RGB: (u8, u8, u8) = (58, 63, 84);
pub const KNOB_RIM_RGB: (u8, u8, u8) = (130, 136, 160);
pub const KNOB_POINTER_RGB: (u8, u8, u8) = (240, 240, 250);

/// The mockup's palette (docs/design/mockup.py).
pub const PX_ACCENT: (u8, u8, u8) = (122, 162, 247);
pub const PX_RED: (u8, u8, u8) = (247, 118, 142);
pub const PX_GREEN: (u8, u8, u8) = (158, 206, 106);
pub const PX_AMBER: (u8, u8, u8) = (224, 175, 104);
pub const PX_CHIP: (u8, u8, u8) = (41, 46, 66);
pub const PX_TRACK: (u8, u8, u8) = (36, 40, 58);
const PX_FAINT: (u8, u8, u8) = (59, 66, 97);
const PX_FADER_TRACK: (u8, u8, u8) = (52, 57, 80);
const PX_CLIP_OFF: (u8, u8, u8) = (60, 44, 56);
const PX_WHITE: (u8, u8, u8) = (235, 235, 245);

/// The ticks on the gain knob's arc (style J); pan and Monitor Mix have a
/// solid arc (styles K, L).
pub const GAIN_TICKS: u32 = 21;

/// Steps a meter's level is quantised to (0.5 dB over its 60 dB).
const METER_STEPS: f64 = 120.0;
/// Steps a fader's position is quantised to.
const FADER_STEPS: f64 = 400.0;
/// Steps a knob's value is quantised to.
const KNOB_STEPS: f64 = 200.0;

/// The most image data (base64 bytes) sent in one frame. One image always
/// goes, however big.
pub const TRANSMIT_BUDGET: usize = 256 * 1024;
/// The most base64 bytes in one kitty escape (the protocol's limit).
const CHUNK: usize = 4096;

/// Whether `TOPPINGCTL_PIXELS` (its value) allows pixel widgets: anything
/// but `0`.
pub fn pixels_wanted(env: Option<&OsStr>) -> bool {
    env.is_none_or(|v| v != "0")
}

/// Something drawn as a pixel image over a cell area.
#[derive(Clone, Debug, PartialEq)]
pub enum PixelWidget {
    /// A knob at `value` (0 ..= 1, `None` unknown).
    Knob {
        value: Option<f64>,
        ticks: u32,
        dimmed: bool,
        /// Lit from the top (a pan), not from the minimum.
        bipolar: bool,
    },
    /// A fader at `db` on a scale topping out at `max_db`; with `labels`
    /// its first and last 3 columns are the scale's text.
    Fader { db: f64, max_db: f64, labels: bool },
    /// A level meter's bar at `db`.
    Meter { db: f64 },
    /// A clip light.
    ClipLight { lit: bool },
    /// A rounded chip under a label: a pill, a mix tab, a strip title.
    Chip { rgb: (u8, u8, u8) },
}

impl PixelWidget {
    /// The z-index: under the text (-1) for widgets with text on them,
    /// else 0.
    pub fn z(&self) -> i32 {
        match self {
            PixelWidget::Chip { .. } | PixelWidget::Fader { .. } => -1,
            _ => 0,
        }
    }
}

/// A widget and the cells it covers.
#[derive(Clone, Debug, PartialEq)]
pub struct PixelSpot {
    pub rect: Rect,
    pub widget: PixelWidget,
}

/// What an image shows, for the cache: the kind, the size in cells, the
/// quantised values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key {
    Knob(u16, u16, Option<u16>, u32, bool, bool),
    Fader(u16, u16, u16, i16, bool),
    Meter(u16, u16, u16),
    Clip(u16, u16, bool),
    Chip(u16, u16, (u8, u8, u8)),
}

fn quantise(v: f64, steps: f64) -> u16 {
    (v.clamp(0.0, 1.0) * steps).round() as u16
}

impl PixelSpot {
    fn key(&self) -> Key {
        let (w, h) = (self.rect.width, self.rect.height);
        match self.widget {
            PixelWidget::Knob {
                value,
                ticks,
                dimmed,
                bipolar,
            } => Key::Knob(
                w,
                h,
                value.map(|v| quantise(v, KNOB_STEPS)),
                ticks,
                dimmed,
                bipolar,
            ),
            PixelWidget::Fader { db, max_db, labels } => Key::Fader(
                w,
                h,
                quantise(fader_frac(db, max_db), FADER_STEPS),
                max_db.round() as i16,
                labels,
            ),
            PixelWidget::Meter { db } => Key::Meter(w, h, quantise(meter_frac(db), METER_STEPS)),
            PixelWidget::ClipLight { lit } => Key::Clip(w, h, lit),
            PixelWidget::Chip { rgb } => Key::Chip(w, h, rgb),
        }
    }

    /// Blanks what the text draw put in this widget's cells, keeping its
    /// labels: a chip keeps its text (on no background), a fader with
    /// labels its scale columns.
    fn clear(&self, buf: &mut Buffer) {
        let r = self.rect.intersection(buf.area);
        let (x0, x1) = match self.widget {
            PixelWidget::Chip { .. } => {
                for y in r.top()..r.bottom() {
                    for x in r.left()..r.right() {
                        buf[(x, y)].set_bg(TermColor::Reset);
                    }
                }
                return;
            }
            PixelWidget::Fader { labels: true, .. } => (r.left() + 3, r.right().saturating_sub(3)),
            _ => (r.left(), r.right()),
        };
        for y in r.top()..r.bottom() {
            for x in x0..x1 {
                buf[(x, y)].reset();
            }
        }
    }
}

/// The image for `key` with `cell`-sized cells. It is drawn from the
/// quantised values, so every spot with the same key looks the same.
fn draw(key: Key, cell: (u16, u16)) -> RgbaImage {
    let px = |c: u16, r: u16| {
        (
            u32::from(c) * u32::from(cell.0),
            u32::from(r) * u32::from(cell.1),
        )
    };
    match key {
        Key::Knob(c, r, q, ticks, dimmed, bipolar) => {
            let (w, h) = px(c, r);
            let v = q.map(|q| f64::from(q) / KNOB_STEPS);
            knob_image_arc(w, h, v, ticks, dimmed, bipolar)
        }
        Key::Fader(c, r, q, max_db, labels) => fader_image_at(
            c,
            r,
            cell,
            f64::from(q) / FADER_STEPS,
            f64::from(max_db),
            labels,
        ),
        Key::Meter(c, r, q) => meter_image_at(c, r, cell, f64::from(q) / METER_STEPS),
        Key::Clip(c, r, lit) => clip_image(c, r, cell, lit),
        Key::Chip(c, r, rgb) => chip_image(c, r, cell, rgb),
    }
}

/// Blanks the text form of each spot the layer placed (see
/// [`PixelSpot`]'s labels rule); spots not placed yet keep their text.
pub fn clear_under(layer: &PixelLayer, spots: &[PixelSpot], buf: &mut Buffer) {
    for spot in spots.iter().filter(|s| layer.is_placed(s.rect)) {
        spot.clear(buf);
    }
}

/// The chip colour for a theme background: the mockup's palette for the
/// theme's pill, accent, red and mix-tab colours, else the colour itself.
pub fn chip_rgb(c: TermColor) -> Option<(u8, u8, u8)> {
    let TermColor::Rgb(r, g, b) = c else {
        return None;
    };
    Some(match c {
        Theme::PILL_OFF_BG => PX_CHIP,
        Theme::ACCENT => PX_ACCENT,
        Theme::WARN => PX_RED,
        c if c == Theme::MIX[0] => PX_GREEN,
        c if c == Theme::MIX[1] => PX_ACCENT,
        c if c == Theme::MIX[2] => PX_AMBER,
        _ => (r, g, b),
    })
}

/// The most images kept; past it the cache (and the terminal's copies) are
/// dropped and rebuilt.
const CACHE_LIMIT: usize = 1500;

/// An image placed in a cell area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shown {
    image: u32,
    placement: u32,
}

/// The pixel renderer: the terminal's cell size, the images it holds, and
/// what is placed where.
pub struct PixelLayer {
    cell: (u16, u16),
    images: HashMap<Key, u32>,
    next_image: u32,
    shown: HashMap<Rect, Shown>,
    next_placement: u32,
    area: Option<Rect>,
    out: String,
    built: usize,
}

impl PixelLayer {
    /// Asks the terminal (after the alternate screen and raw mode are on,
    /// before any event is read) whether it speaks the kitty graphics
    /// protocol. `None` otherwise, or when `TOPPINGCTL_PIXELS=0`.
    pub fn detect() -> Option<PixelLayer> {
        if !pixels_wanted(std::env::var_os("TOPPINGCTL_PIXELS").as_deref()) {
            return None;
        }
        let picker = Picker::from_query_stdio().ok()?;
        (picker.protocol_type() == ProtocolType::Kitty).then(|| PixelLayer::new(picker))
    }

    /// A layer with `picker`'s cell size (tests use a halfblocks picker).
    pub fn new(picker: Picker) -> PixelLayer {
        PixelLayer::with_cell_size(picker.font_size())
    }

    /// A layer drawing `cell`-sized cells (width, height in pixels).
    pub fn with_cell_size(cell: (u16, u16)) -> PixelLayer {
        PixelLayer {
            cell: (cell.0.max(1), cell.1.max(1)),
            images: HashMap::new(),
            next_image: 1,
            shown: HashMap::new(),
            next_placement: 1,
            area: None,
            out: String::new(),
            built: 0,
        }
    }

    /// Images the terminal holds (sent and cached).
    pub fn cached(&self) -> usize {
        self.images.len()
    }

    /// Images drawn so far (cache misses).
    pub fn built(&self) -> usize {
        self.built
    }

    /// Whether an image is placed on `rect` (after [`PixelLayer::update`]).
    pub fn is_placed(&self, rect: Rect) -> bool {
        self.shown.contains_key(&rect)
    }

    /// The kitty commands queued since the last call, to write after the
    /// text frame.
    pub fn take_output(&mut self) -> String {
        std::mem::take(&mut self.out)
    }

    /// The kitty sequence that frees every image the terminal holds.
    pub const FREE_ALL: &'static str = "\x1b_Ga=d,d=A,q=2\x1b\\";
    /// The kitty sequence that deletes every placement, keeping the images.
    const DELETE_PLACEMENTS: &'static str = "\x1b_Ga=d,d=a,q=2\x1b\\";

    /// Brings the terminal's placements in line with `spots` on a screen
    /// of `area`: places what is new or changed, deletes what went.
    pub fn update(&mut self, spots: &[PixelSpot], area: Rect) {
        if self.area != Some(area) {
            // A resize (or the first frame): the terminal may have moved or
            // cleared them, so every placement goes and is placed again.
            if self.area.is_some() {
                self.out.push_str(Self::DELETE_PLACEMENTS);
            }
            self.shown.clear();
            self.area = Some(area);
        }
        if self.images.len() > CACHE_LIMIT {
            self.out.push_str(Self::FREE_ALL);
            self.images.clear();
            self.shown.clear();
        }
        let mut budget = TRANSMIT_BUDGET;
        let mut sent_any = false;
        let mut old = std::mem::take(&mut self.shown);
        let mut deletes = Vec::new();
        for spot in spots {
            let r = spot.rect;
            if r.is_empty() || r.intersection(area) != r || self.shown.contains_key(&r) {
                continue;
            }
            let key = spot.key();
            let image = match self.images.get(&key) {
                Some(&id) => id,
                None if sent_any && budget == 0 => {
                    // Over this frame's budget: a later frame sends it.
                    if let Some(prev) = old.remove(&r) {
                        deletes.push(prev);
                    }
                    continue;
                }
                None => {
                    let id = self.next_image;
                    self.next_image = self.next_image.wrapping_add(1).max(1);
                    let sent = self.transmit(id, &draw(key, self.cell));
                    budget = budget.saturating_sub(sent);
                    sent_any = true;
                    self.built += 1;
                    self.images.insert(key, id);
                    id
                }
            };
            let shown = match old.remove(&r) {
                Some(prev) if prev.image == image => prev,
                prev => {
                    let placement = prev.map_or_else(|| self.new_placement(), |p| p.placement);
                    self.place(image, placement, r, spot.widget.z());
                    deletes.extend(prev);
                    Shown { image, placement }
                }
            };
            self.shown.insert(r, shown);
        }
        deletes.extend(old.into_values());
        for d in deletes {
            let _ = write!(
                self.out,
                "\x1b_Ga=d,d=i,i={},p={},q=2\x1b\\",
                d.image, d.placement
            );
        }
    }

    fn new_placement(&mut self) -> u32 {
        let p = self.next_placement;
        self.next_placement = self.next_placement.wrapping_add(1).max(1);
        p
    }

    /// Queues `img` under id `id` (zlib RGBA, chunked); returns the base64
    /// bytes queued.
    fn transmit(&mut self, id: u32, img: &RgbaImage) -> usize {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        let _ = z.write_all(img.as_raw());
        let data = z.finish().unwrap_or_default();
        let b64 = base64_simd::STANDARD.encode_to_string(&data);
        let chunks: Vec<&str> = b64
            .as_bytes()
            .chunks(CHUNK)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect();
        let n = chunks.len().max(1);
        for i in 0..n {
            let chunk = chunks.get(i).copied().unwrap_or("");
            let m = u8::from(i + 1 < n);
            if i == 0 {
                let _ = write!(
                    self.out,
                    "\x1b_Ga=t,i={id},f=32,o=z,s={},v={},q=2,m={m};{chunk}\x1b\\",
                    img.width(),
                    img.height()
                );
            } else {
                let _ = write!(self.out, "\x1b_Gm={m};{chunk}\x1b\\");
            }
        }
        b64.len()
    }

    /// Queues placing image `id` over `r`, under the text with `z` < 0.
    /// The cursor stays where it was (`C=1`).
    fn place(&mut self, id: u32, placement: u32, r: Rect, z: i32) {
        let _ = write!(
            self.out,
            "\x1b[{};{}H\x1b_Ga=p,i={id},p={placement},c={},r={},C=1,z={z},q=2\x1b\\",
            r.y + 1,
            r.x + 1,
            r.width,
            r.height
        );
    }
}

// ------------------------------------------------------------ SDF raster
//
// The mockup's renderer: each shape is a signed distance function, and a
// pixel's coverage is 0.5 − distance (one pixel of anti-aliasing).

/// A shape as a signed distance (pixels, negative inside).
#[derive(Clone, Copy, Debug)]
enum Shape {
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
    },
    Capsule {
        ax: f32,
        ay: f32,
        bx: f32,
        by: f32,
        r: f32,
    },
    RRect {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        rad: f32,
    },
}

impl Shape {
    fn dist(&self, x: f32, y: f32) -> f32 {
        match *self {
            Shape::Circle { cx, cy, r } => (x - cx).hypot(y - cy) - r,
            Shape::Capsule { ax, ay, bx, by, r } => {
                let (dx, dy) = (bx - ax, by - ay);
                let l = (dx * dx + dy * dy).max(1e-9);
                let t = (((x - ax) * dx + (y - ay) * dy) / l).clamp(0.0, 1.0);
                (x - ax - t * dx).hypot(y - ay - t * dy) - r
            }
            Shape::RRect {
                x0,
                y0,
                x1,
                y1,
                rad,
            } => {
                let rad = rad.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
                let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                let (hx, hy) = ((x1 - x0) / 2.0 - rad, (y1 - y0) / 2.0 - rad);
                let (qx, qy) = ((x - cx).abs() - hx, (y - cy).abs() - hy);
                qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad
            }
        }
    }

    fn bbox(&self) -> (f32, f32, f32, f32) {
        match *self {
            Shape::Circle { cx, cy, r } => (cx - r, cy - r, cx + r, cy + r),
            Shape::Capsule { ax, ay, bx, by, r } => (
                ax.min(bx) - r,
                ay.min(by) - r,
                ax.max(bx) + r,
                ay.max(by) + r,
            ),
            Shape::RRect { x0, y0, x1, y1, .. } => (x0, y0, x1, y1),
        }
    }
}

type Rgb = (u8, u8, u8);

/// A see-through canvas, straight (not premultiplied) colour.
struct Canvas {
    w: usize,
    h: usize,
    p: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Canvas {
        let (w, h) = (w.max(1) as usize, h.max(1) as usize);
        Canvas {
            w,
            h,
            p: vec![[0.0; 4]; w * h],
        }
    }

    fn blend(&mut self, x: usize, y: usize, c: [f32; 3], a: f32) {
        let p = &mut self.p[y * self.w + x];
        let da = p[3];
        let oa = a + da * (1.0 - a);
        if oa <= 0.0 {
            return;
        }
        for k in 0..3 {
            p[k] = (c[k] * a + p[k] * da * (1.0 - a)) / oa;
        }
        p[3] = oa;
    }

    /// Fills `shape` with `color(x, y)` at `alpha`.
    fn fill_with(&mut self, shape: Shape, alpha: f32, color: impl Fn(f32, f32) -> [f32; 3]) {
        let (x0, y0, x1, y1) = shape.bbox();
        let xa = (x0.floor() as i64 - 1).max(0) as usize;
        let ya = (y0.floor() as i64 - 1).max(0) as usize;
        let xb = ((x1.ceil() as i64 + 2).max(0) as usize).min(self.w);
        let yb = ((y1.ceil() as i64 + 2).max(0) as usize).min(self.h);
        for y in ya..yb {
            let py = y as f32 + 0.5;
            for x in xa..xb {
                let px = x as f32 + 0.5;
                let a = (0.5 - shape.dist(px, py)).clamp(0.0, 1.0);
                if a > 0.0 {
                    self.blend(x, y, color(px, py), a * alpha);
                }
            }
        }
    }

    fn fill(&mut self, shape: Shape, c: Rgb, alpha: f32) {
        let c = f3(c);
        self.fill_with(shape, alpha, |_, _| c);
    }

    fn into_image(self) -> RgbaImage {
        let mut out = RgbaImage::new(self.w as u32, self.h as u32);
        for (px, p) in out.pixels_mut().zip(self.p) {
            px.0 = [
                p[0].clamp(0.0, 255.0) as u8,
                p[1].clamp(0.0, 255.0) as u8,
                p[2].clamp(0.0, 255.0) as u8,
                (p[3] * 255.0).clamp(0.0, 255.0) as u8,
            ];
        }
        out
    }
}

fn f3(c: Rgb) -> [f32; 3] {
    [f32::from(c.0), f32::from(c.1), f32::from(c.2)]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// The meter colour at `f` of the way down from the top (0 dB at the top,
/// −60 at the bottom): green up to −12 dB, amber from −12 to −6, red above
/// −6, blended over a dB either side of each step.
fn meter_colour(f: f32) -> [f32; 3] {
    let db = -60.0 * f;
    let (g, a, r) = (f3(PX_GREEN), f3(PX_AMBER), f3(PX_RED));
    let step = |edge: f32| ((db - edge + 1.0) / 2.0).clamp(0.0, 1.0);
    mix(mix(g, a, step(-12.0)), r, step(-6.0))
}

/// A meter bar from `x0` to `x1` between `top` and `bot`, the signal
/// reaching `level` (0 = empty, 1 = full): a capsule track, the gradient
/// up to the level, and a peak line on it.
fn bar(cv: &mut Canvas, (x0, x1): (f32, f32), (top, bot): (f32, f32), level: f32, sy: f32) {
    let rad = (x1 - x0) / 2.0;
    let track = |inset: f32| Shape::RRect {
        x0: x0 + inset,
        y0: top - rad + inset,
        x1: x1 - inset,
        y1: bot + rad - inset,
        rad: rad - inset,
    };
    // A faint rim around the track, so an empty meter still reads as one.
    cv.fill(track(0.0), PX_FAINT, 1.0);
    cv.fill(track(0.8 * sy.max(0.75)), PX_TRACK, 1.0);
    if level <= 0.0 {
        return;
    }
    let ly = top + (bot - top) * (1.0 - level.clamp(0.0, 1.0));
    let span = (bot - top).max(1.0);
    cv.fill_with(
        Shape::RRect {
            x0,
            y0: ly,
            x1,
            y1: bot + rad,
            rad,
        },
        1.0,
        |_, y| meter_colour((y - top) / span),
    );
    cv.fill(
        Shape::Capsule {
            ax: x0 + 1.0,
            ay: ly,
            bx: x1 - 1.0,
            by: ly,
            r: 1.2 * sy,
        },
        PX_WHITE,
        1.0,
    );
}

fn canvas(cols: u16, rows: u16, cell: (u16, u16)) -> (Canvas, f32, f32) {
    let (cw, ch) = (f32::from(cell.0.max(1)), f32::from(cell.1.max(1)));
    let cv = Canvas::new(
        u32::from(cols) * u32::from(cell.0.max(1)),
        u32::from(rows) * u32::from(cell.1.max(1)),
    );
    (cv, cw, ch)
}

/// A level meter's bar, `cols`×`rows` cells, at `db`. Its top and bottom
/// are the first and last rows' centres (0 dB and −60 on the scale).
pub fn meter_image(cols: u16, rows: u16, cell: (u16, u16), db: f64) -> RgbaImage {
    meter_image_at(cols, rows, cell, meter_frac(db))
}

fn meter_image_at(cols: u16, rows: u16, cell: (u16, u16), level: f64) -> RgbaImage {
    let (mut cv, cw, ch) = canvas(cols, rows, cell);
    let (w, h) = (cv.w as f32, cv.h as f32);
    // One cell: a bar of about 60 %; two or more (the input meter): most
    // of them.
    let bw = if cols >= 2 {
        w * 0.8
    } else {
        (w * 0.6).min(cw)
    };
    let cx = w / 2.0;
    bar(
        &mut cv,
        (cx - bw / 2.0, cx + bw / 2.0),
        (0.5 * ch, h - 0.5 * ch),
        level as f32,
        ch / 24.0,
    );
    cv.into_image()
}

/// A clip light in `cols`×`rows` cells: a red lamp with a glow when lit, a
/// dark one when not.
pub fn clip_image(cols: u16, rows: u16, cell: (u16, u16), lit: bool) -> RgbaImage {
    let (mut cv, _, ch) = canvas(cols, rows, cell);
    let (cx, cy) = (cv.w as f32 / 2.0, cv.h as f32 / 2.0);
    let r = (ch * 5.0 / 24.0).min(cv.w as f32 * 0.3);
    if lit {
        for k in (1..=6).rev() {
            let gr = r + k as f32 * 0.26 * r;
            cv.fill(Shape::Circle { cx, cy, r: gr }, PX_RED, 0.08);
        }
    }
    let c = if lit { PX_RED } else { PX_CLIP_OFF };
    cv.fill(Shape::Circle { cx, cy, r }, c, 1.0);
    cv.into_image()
}

/// A chip in `cols`×`rows` cells: a rounded rect in `rgb`, under a label.
pub fn chip_image(cols: u16, rows: u16, cell: (u16, u16), rgb: Rgb) -> RgbaImage {
    let (mut cv, _, ch) = canvas(cols, rows, cell);
    let sy = ch / 24.0;
    let (w, h) = (cv.w as f32, cv.h as f32);
    cv.fill(
        Shape::RRect {
            x0: 1.5,
            y0: 3.0 * sy,
            x1: w - 1.5,
            y1: h - 3.0 * sy,
            rad: 7.0 * sy,
        },
        rgb,
        1.0,
    );
    cv.into_image()
}

/// A fader, `cols`×`rows` cells, at `db` on a scale topping out at
/// `max_db`: a capsule track, the value filled below a light thumb, and
/// tick marks on both sides. With `labels` the first and last 3 columns
/// are left clear for the scale's text.
pub fn fader_image(
    cols: u16,
    rows: u16,
    cell: (u16, u16),
    db: f64,
    max_db: f64,
    labels: bool,
) -> RgbaImage {
    fader_image_at(cols, rows, cell, fader_frac(db, max_db), max_db, labels)
}

fn fader_image_at(
    cols: u16,
    rows: u16,
    cell: (u16, u16),
    frac: f64,
    max_db: f64,
    labels: bool,
) -> RgbaImage {
    let (mut cv, cw, ch) = canvas(cols, rows, cell);
    let (sx, sy) = (cw / 12.0, ch / 24.0);
    let (c0, c1) = if labels && cols > 6 {
        (3.0, f32::from(cols) - 3.0)
    } else {
        (0.0, f32::from(cols))
    };
    let tx = (c0 + c1) / 2.0 * cw;
    let half = (c1 - c0) / 2.0 * cw;
    let (top, bot) = (0.5 * ch, cv.h as f32 - 0.5 * ch);
    // Tick marks on both sides, on the scale's rows.
    let inner = (0.75 * cw).min(half * 0.6);
    let outer = (1.33 * cw).min(half - 1.0);
    for (row, _) in Fader::ticks(max_db, rows) {
        let y = (f32::from(row) + 0.5) * ch;
        for s in [-1.0, 1.0] {
            cv.fill(
                Shape::Capsule {
                    ax: tx + s * inner,
                    ay: y,
                    bx: tx + s * outer,
                    by: y,
                    r: 1.0 * sy,
                },
                PX_FAINT,
                1.0,
            );
        }
    }
    let tw = 2.5 * sx;
    let track = |y0: f32| Shape::RRect {
        x0: tx - tw,
        y0,
        x1: tx + tw,
        y1: bot,
        rad: tw,
    };
    cv.fill(track(top), PX_FADER_TRACK, 1.0);
    // The thumb sits on the row the text cap would (rounded to a row
    // centre), so it lines up with the scale.
    let row = fader_row(frac, rows);
    let y = (f32::from(row) + 0.5) * ch;
    cv.fill(track(y), PX_ACCENT, 0.55);
    let hw = (17.0 * sx).min(half - 1.0);
    let hh = 8.0 * sy;
    let (light, dark) = (f3((222, 226, 240)), f3((140, 146, 170)));
    cv.fill_with(
        Shape::RRect {
            x0: tx - hw,
            y0: y - hh,
            x1: tx + hw,
            y1: y + hh,
            rad: 4.0 * sy,
        },
        1.0,
        |_, yy| mix(light, dark, ((yy - (y - hh)) / (2.0 * hh)).clamp(0.0, 1.0)),
    );
    let lw = (12.0 * sx).min(hw - 2.0);
    cv.fill(
        Shape::Capsule {
            ax: tx - lw,
            ay: y,
            bx: tx + lw,
            by: y,
            r: 1.1 * sy,
        },
        PX_ACCENT,
        1.0,
    );
    cv.into_image()
}

// ------------------------------------------------------------------ knob

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

fn solid(c: (u8, u8, u8)) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(rgb(c));
    p.anti_alias = true;
    p
}

fn vertical_gradient(top: f32, bottom: f32, stops: Vec<GradientStop>) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    if let Some(s) = LinearGradient::new(
        Point::from_xy(0.0, bottom),
        Point::from_xy(0.0, top),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        p.shader = s;
    }
    p
}

fn stroke(width: f32, cap: LineCap) -> Stroke {
    Stroke {
        width,
        line_cap: cap,
        ..Stroke::default()
    }
}

/// A point on a circle, screen coordinates (y down), `deg` counter-clockwise
/// from the right.
fn polar(cx: f32, cy: f32, r: f32, deg: f32) -> (f32, f32) {
    let a = deg * PI / 180.0;
    (cx + r * a.cos(), cy - r * a.sin())
}

/// An arc from `rel0` to `rel1` degrees along the knob's sweep.
fn arc(cx: f32, cy: f32, r: f32, rel0: f32, rel1: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let start = START_DEG as f32;
    let n = (((rel1 - rel0).abs() / 2.0).ceil() as usize).max(1);
    for i in 0..=n {
        let rel = rel0 + (rel1 - rel0) * i as f32 / n as f32;
        let (x, y) = polar(cx, cy, r, start - rel);
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.finish()
}

fn to_rgba(pm: &Pixmap) -> RgbaImage {
    let mut out = RgbaImage::new(pm.width(), pm.height());
    for (px, c) in out.pixels_mut().zip(pm.pixels()) {
        let c = c.demultiply();
        px.0 = [c.red(), c.green(), c.blue(), c.alpha()];
    }
    out
}

/// A knob (style J; K and L are the same with `ticks` 0 and `dimmed`): a
/// round, top-lit body with a rim, a 270° arc (in `ticks` ticks, or solid
/// with 0) lit in the accent up to `value`, and a white pointer, centred on
/// a see-through `w`×`h` image.
pub fn knob_image(w: u32, h: u32, value: Option<f64>, ticks: u32, dimmed: bool) -> RgbaImage {
    knob_image_arc(w, h, value, ticks, dimmed, false)
}

/// [`knob_image`], with the arc lit from the top to the value when
/// `bipolar` (a pan knob: nothing lit at the centre).
pub fn knob_image_arc(
    w: u32,
    h: u32,
    value: Option<f64>,
    ticks: u32,
    dimmed: bool,
    bipolar: bool,
) -> RgbaImage {
    let Some(mut pm) = Pixmap::new(w.max(1), h.max(1)) else {
        return RgbaImage::new(w.max(1), h.max(1));
    };
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let r = w.min(h) as f32 / 2.0;
    let sweep = SWEEP_DEG as f32;
    let lit = lit_span(value, dimmed, bipolar).map(|(a, b)| (a as f32, b as f32));
    let id = Transform::identity();
    let band = stroke(0.18 * r, LineCap::Butt);
    let segments: Vec<(f32, f32)> = if ticks == 0 {
        vec![(0.0, sweep)]
    } else {
        let p = sweep / ticks as f32;
        (0..ticks)
            .map(|i| (i as f32 * p, i as f32 * p + 0.6 * p))
            .collect()
    };
    let mut draw_arc = |a: f32, b: f32, c| {
        if b > a {
            if let Some(path) = arc(cx, cy, 0.87 * r, a, b) {
                pm.stroke_path(&path, &solid(c), &band, id, None);
            }
        }
    };
    for (a, b) in segments {
        // Each segment: dim, lit where it meets the lit span, dim again.
        let (la, lb) = match lit {
            Some((lo, hi)) if hi > a && lo < b => (lo.max(a), hi.min(b)),
            _ => (b, b),
        };
        draw_arc(a, la, KNOB_DIM_RGB);
        draw_arc(la, lb, KNOB_ACCENT_RGB);
        draw_arc(lb, b, KNOB_DIM_RGB);
    }
    if let Some(body) = PathBuilder::from_circle(cx, cy, 0.61 * r) {
        let paint = vertical_gradient(
            cy - 0.6 * r,
            cy + 0.6 * r,
            vec![
                GradientStop::new(0.0, rgb(KNOB_BODY_RGB)),
                GradientStop::new(1.0, rgb(KNOB_BODY_TOP_RGB)),
            ],
        );
        pm.fill_path(&body, &paint, FillRule::Winding, id, None);
        let rim = if dimmed { KNOB_DIM_RGB } else { KNOB_RIM_RGB };
        pm.stroke_path(
            &body,
            &solid(rim),
            &stroke(0.06 * r, LineCap::Butt),
            id,
            None,
        );
    }
    // The pointer shows a known value, dimmed or not.
    if let Some(v) = value {
        let deg = START_DEG as f32 - sweep * v as f32;
        let (x, y) = polar(cx, cy, 0.5 * r, deg);
        let mut pb = PathBuilder::new();
        pb.move_to(cx, cy);
        pb.line_to(x, y);
        if let Some(path) = pb.finish() {
            let s = stroke(0.09 * r, LineCap::Round);
            pm.stroke_path(&path, &solid(KNOB_POINTER_RGB), &s, id, None);
        }
    }
    to_rgba(&pm)
}
