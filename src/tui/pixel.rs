//! Pixel widgets for terminals with the kitty graphics protocol (kitty,
//! Ghostty): knobs, faders, level meters, clip lights and chips (button
//! faces, the active mix tab), drawn anti-aliased and placed directly with kitty
//! graphics commands. The look is flat and follows the widget code of
//! `docs/design/mockup-flat.py` (#41), scaled from its 12×24 px cell to the
//! terminal's: LED-ladder meters, flat knobs, square button faces.
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
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::Write as _;
use std::time::{Duration, Instant};

use image::RgbaImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color as TermColor;
use ratatui_image::picker::{Picker, ProtocolType};

use crate::tui::theme::Theme;
use crate::tui::widgets::dial::{START_DEG, SWEEP_DEG};
use crate::tui::widgets::fader::{fader_frac, fader_row, Fader};
use crate::tui::widgets::meter::meter_frac;

/// The flat mockup's palette (docs/design/mockup-flat.py).
pub const PX_BG: Rgb = (26, 27, 38);
pub const PX_DIM: Rgb = (86, 95, 137);
pub const PX_FAINT: Rgb = (59, 66, 97);
pub const PX_ACCENT: Rgb = (122, 162, 247);
pub const PX_RED: Rgb = (247, 118, 142);
pub const PX_GREEN: Rgb = (158, 206, 106);
pub const PX_AMBER: Rgb = (224, 175, 104);
/// An unlit button's face, and its 1-px outline.
pub const PX_CHIP: Rgb = (41, 46, 66);
pub const PX_CHIP_EDGE: Rgb = (52, 58, 82);
/// A knob's disc (dimmed: Monitor Mix), its unlit track arc and pointer;
/// the fader's track is the same unlit colour.
pub const PX_KNOB_BODY: Rgb = (44, 49, 70);
pub const PX_KNOB_BODY_DIM: Rgb = (38, 42, 58);
pub const PX_ARC_OFF: Rgb = (48, 53, 76);
pub const PX_POINTER: Rgb = (236, 238, 248);
/// The fader's cap.
pub const PX_FADER_CAP: Rgb = (210, 214, 230);
/// An unlit clip light.
pub const PX_CLIP_OFF: Rgb = (58, 42, 54);

/// The mockup's cell, which its pixel sizes are given at.
const MOCK_CELL: (f32, f32) = (12.0, 24.0);
/// A meter's width (one for every meter), its LED segments' height and the
/// gap between them, at the mockup's cell. The mockup's bar is 14 px; ours
/// are pre/post pairs in adjacent cells, so the width is the cell less the
/// segments' 2 px gap.
const METER_W: f32 = 10.0;
const SEG_H: f32 = 5.0;
const SEG_GAP: f32 = 2.0;
/// An unlit LED: its colour at this much on the background.
const LED_OFF: f32 = 0.16;

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
    /// A flat square face under a label, lit in `rgb` (unlit: [`PX_CHIP`],
    /// outlined). A button gives its `label`'s first cell and width in the
    /// rect, and the face centres on it (the mockup's `pill()`); without,
    /// the face is inset in its cells (the active mix tab).
    Chip {
        rgb: (u8, u8, u8),
        label: Option<(u16, u16)>,
    },
    /// A meter's flat box, under its bar, clip light and labels.
    Inset,
}

impl PixelWidget {
    /// The z-index: under the text (-1) for widgets with text on them,
    /// else 0.
    pub fn z(&self) -> i32 {
        match self {
            PixelWidget::Inset => -2,
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
    Knob(u16, u16, Option<u16>, bool, bool),
    Fader(u16, u16, u16, i16, bool),
    Meter(u16, u16, u16),
    Clip(u16, u16, bool),
    Chip(u16, u16, (u8, u8, u8), Option<(u16, u16)>),
    Inset(u16, u16),
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
                dimmed,
                bipolar,
            } => Key::Knob(
                w,
                h,
                value.map(|v| quantise(v, KNOB_STEPS)),
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
            PixelWidget::Chip { rgb, label } => Key::Chip(w, h, rgb, label),
            PixelWidget::Inset => Key::Inset(w, h),
        }
    }

    /// Blanks what the text draw put in this widget's cells, keeping its
    /// labels: a chip keeps its text (on no background), a fader with
    /// labels its scale columns.
    fn clear(&self, buf: &mut Buffer) {
        let r = self.rect.intersection(buf.area);
        let (x0, x1) = match self.widget {
            PixelWidget::Chip { .. } | PixelWidget::Inset => {
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
        Key::Knob(c, r, q, dimmed, bipolar) => {
            let (w, h) = px(c, r);
            let v = q.map(|q| f64::from(q) / KNOB_STEPS);
            knob_image(w, h, v, dimmed, bipolar)
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
        Key::Chip(c, r, rgb, None) => chip_image(c, r, cell, rgb),
        Key::Chip(c, r, rgb, Some(label)) => pill_image(c, r, cell, rgb, label),
        Key::Inset(c, r) => inset_image(c, r, cell),
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
    /// While resizing: text only until this time.
    settle_until: Option<Instant>,
    settle: Duration,
    clock: Box<dyn Fn() -> Instant>,
}

/// How long the size must stay put after a resize before the images come
/// back: a drag-resize fires a storm of resizes.
pub const RESIZE_SETTLE: Duration = Duration::from_millis(150);

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
            settle_until: None,
            settle: RESIZE_SETTLE,
            clock: Box::new(Instant::now),
        }
    }

    /// The clock the settling time is measured on (tests use a fake one).
    pub fn set_clock(&mut self, clock: impl Fn() -> Instant + 'static) {
        self.clock = Box::new(clock);
    }

    /// How long to wait after the last resize ([`RESIZE_SETTLE`]).
    pub fn set_resize_settle(&mut self, settle: Duration) {
        self.settle = settle;
    }

    /// The terminal was resized at `now`, with `cell` its new cell size in
    /// pixels when known (TIOCGWINSZ). The first resize of a storm frees
    /// every image (the terminal's copies too, so nothing stays at old
    /// coordinates or at an old scale, and no placement can refer to an
    /// image from before); until the size has been still for the settling
    /// time, frames are text only. Write [`PixelLayer::take_output`] before
    /// the next frame.
    pub fn begin_resize(&mut self, now: Instant, cell: Option<(u16, u16)>) {
        if let Some((w, h)) = cell.filter(|&(w, h)| w > 0 && h > 0) {
            self.cell = (w, h);
        }
        if self.settle_until.is_none() {
            self.out.push_str(Self::FREE_ALL);
            self.images.clear();
            self.shown.clear();
        }
        self.settle_until = Some(now + self.settle);
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

    /// Brings the terminal's placements in line with `spots` on a screen
    /// of `area`: places what is new or changed, deletes what went.
    pub fn update(&mut self, spots: &[PixelSpot], area: Rect) {
        let now = (self.clock)();
        if self.area != Some(area) {
            // A resize the event loop did not report (or the first frame).
            if self.area.is_some() {
                self.begin_resize(now, None);
            }
            self.area = Some(area);
        }
        if let Some(until) = self.settle_until {
            if now < until {
                // Still resizing: text only.
                return;
            }
            self.settle_until = None;
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
    /// A ring segment of radius `r`, `t` wide, clockwise from `a0` to `a1`
    /// degrees (counter-clockwise from the right, `a0` > `a1`), with round
    /// caps: the mockup's `arc_sdf`.
    Arc {
        cx: f32,
        cy: f32,
        r: f32,
        t: f32,
        a0: f32,
        a1: f32,
    },
    /// A rectangle's edge as a line `w` wide (an outline).
    Edge {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        rad: f32,
        w: f32,
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
            Shape::Arc {
                cx,
                cy,
                r,
                t,
                a0,
                a1,
            } => {
                let ang = (-(y - cy)).atan2(x - cx).to_degrees().rem_euclid(360.0);
                if (a0 - ang).rem_euclid(360.0) <= a0 - a1 {
                    return ((x - cx).hypot(y - cy) - r).abs() - t / 2.0;
                }
                [a0, a1]
                    .map(|a| {
                        let (ex, ey) = polar(cx, cy, r, a);
                        (x - ex).hypot(y - ey) - t / 2.0
                    })
                    .into_iter()
                    .fold(f32::INFINITY, f32::min)
            }
            Shape::Edge {
                x0,
                y0,
                x1,
                y1,
                rad,
                w,
            } => {
                let rect = Shape::RRect {
                    x0,
                    y0,
                    x1,
                    y1,
                    rad,
                };
                rect.dist(x, y).abs() - w / 2.0
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
            Shape::Arc { cx, cy, r, t, .. } => (cx - r - t, cy - r - t, cx + r + t, cy + r + t),
            Shape::Edge {
                x0, y0, x1, y1, w, ..
            } => (x0 - w, y0 - w, x1 + w, y1 + w),
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

    /// Fills `shape` with `c`.
    fn fill(&mut self, shape: Shape, c: Rgb) {
        let c = f3(c);
        let (x0, y0, x1, y1) = shape.bbox();
        let xa = (x0.floor() as i64 - 1).max(0) as usize;
        let ya = (y0.floor() as i64 - 1).max(0) as usize;
        let xb = ((x1.ceil() as i64 + 2).max(0) as usize).min(self.w);
        let yb = ((y1.ceil() as i64 + 2).max(0) as usize).min(self.h);
        for y in ya..yb {
            let py = y as f32 + 0.5;
            for x in xa..xb {
                let a = (0.5 - shape.dist(x as f32 + 0.5, py)).clamp(0.0, 1.0);
                if a > 0.0 {
                    self.blend(x, y, c, a);
                }
            }
        }
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

/// `a` toward `b` by `t`, as the mockup's `mix` (truncated to bytes).
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// A canvas of `cols`×`rows` cells, and the scale from the mockup's cell
/// to ours (x, y).
fn canvas(cols: u16, rows: u16, cell: (u16, u16)) -> (Canvas, f32, f32) {
    let (cw, ch) = (cell.0.max(1), cell.1.max(1));
    let cv = Canvas::new(
        u32::from(cols) * u32::from(cw),
        u32::from(rows) * u32::from(ch),
    );
    (cv, f32::from(cw) / MOCK_CELL.0, f32::from(ch) / MOCK_CELL.1)
}

/// A point on a circle, screen coordinates (y down), `deg` counter-clockwise
/// from the right.
fn polar(cx: f32, cy: f32, r: f32, deg: f32) -> (f32, f32) {
    let a = deg.to_radians();
    (cx + r * a.cos(), cy - r * a.sin())
}

// ----------------------------------------------------------------- meter

/// An LED's colour at `f` of the way down from the top: red in the top
/// 8 %, amber to 30 %, green below; unlit, a tint on the background.
fn led_colour(f: f32, lit: bool) -> Rgb {
    let c = if f < 0.08 {
        PX_RED
    } else if f < 0.30 {
        PX_AMBER
    } else {
        PX_GREEN
    };
    if lit {
        c
    } else {
        mix(PX_BG, c, LED_OFF)
    }
}

/// A meter's columns in a `w`-px wide image: the one meter width, centred,
/// on whole pixels.
fn meter_x(w: f32, sx: f32) -> (f32, f32) {
    let mw = (METER_W * sx).round().clamp(1.0, w);
    let x0 = ((w - mw) / 2.0).floor();
    (x0, x0 + mw)
}

/// The LED ladder from `top` to `bot`, the signal reaching `level` (0 =
/// empty, 1 = full): square segments from the bottom up, each lit when
/// its middle is at or under the level.
fn ladder(cv: &mut Canvas, (x0, x1): (f32, f32), (top, bot): (f32, f32), level: f32, sy: f32) {
    let seg = (SEG_H * sy).round().max(1.0);
    let gap = (SEG_GAP * sy).round().max(1.0);
    let span = (bot - top).max(1.0);
    let ly = top + span * (1.0 - level.clamp(0.0, 1.0));
    let mut y = bot.round();
    while y - seg >= top {
        let mid = y - seg / 2.0;
        let lit = level > 0.0 && mid >= ly;
        let shape = Shape::RRect {
            x0,
            y0: y - seg,
            x1,
            y1: y,
            rad: 0.0,
        };
        cv.fill(shape, led_colour((mid - top) / span, lit));
        y -= seg + gap;
    }
}

/// A level meter's bar, `cols`×`rows` cells, at `db`. Its top and bottom
/// are the first and last rows' centres (0 dB and −60 on the scale).
pub fn meter_image(cols: u16, rows: u16, cell: (u16, u16), db: f64) -> RgbaImage {
    meter_image_at(cols, rows, cell, meter_frac(db))
}

fn meter_image_at(cols: u16, rows: u16, cell: (u16, u16), level: f64) -> RgbaImage {
    let (mut cv, sx, sy) = canvas(cols, rows, cell);
    let (w, h) = (cv.w as f32, cv.h as f32);
    let half = f32::from(cell.1.max(1)) / 2.0;
    ladder(&mut cv, meter_x(w, sx), (half, h - half), level as f32, sy);
    cv.into_image()
}

/// A clip light in `cols`×`rows` cells: a square LED the meter's width,
/// red when lit.
pub fn clip_image(cols: u16, rows: u16, cell: (u16, u16), lit: bool) -> RgbaImage {
    let (mut cv, sx, sy) = canvas(cols, rows, cell);
    let (x0, x1) = meter_x(cv.w as f32, sx);
    let cy = cv.h as f32 / 2.0;
    let shape = Shape::RRect {
        x0,
        y0: cy - 4.0 * sy,
        x1,
        y1: cy + 4.0 * sy,
        rad: 0.5,
    };
    cv.fill(shape, if lit { PX_RED } else { PX_CLIP_OFF });
    cv.into_image()
}

// --------------------------------------------------------------- buttons

/// A meter box's face (the text form's [`Theme::INSET_BG`]).
const PX_INSET: Rgb = (20, 21, 30);

/// A flat face `face_w` wide centred on `cx`, inset 4 px (at the mockup's
/// cell) from the top and bottom: solid when lit, the chip face and a
/// 1-px outline when not ([`PX_CHIP`]).
fn face(
    cols: u16,
    rows: u16,
    cell: (u16, u16),
    rgb: Rgb,
    cx: Option<f32>,
    face_w: Option<f32>,
) -> RgbaImage {
    let (mut cv, sx, sy) = canvas(cols, rows, cell);
    let (w, h) = (cv.w as f32, cv.h as f32);
    let cx = cx.unwrap_or(w / 2.0);
    let fw = face_w.unwrap_or(w - 6.0 * sx);
    let (x0, y0, x1, y1) = (cx - fw / 2.0, 4.0 * sy, cx + fw / 2.0, h - 4.0 * sy);
    let rad = 0.5;
    cv.fill(
        Shape::RRect {
            x0,
            y0,
            x1,
            y1,
            rad,
        },
        rgb,
    );
    if rgb == PX_CHIP {
        let edge = Shape::Edge {
            x0,
            y0,
            x1,
            y1,
            rad,
            w: 1.0,
        };
        cv.fill(edge, PX_CHIP_EDGE);
    }
    cv.into_image()
}

/// A chip in `cols`×`rows` cells (the active mix tab): a flat square face
/// inset 3 px across and 4 px down (at the mockup's cell), lit in `rgb`;
/// [`PX_CHIP`] is unlit, with an outline. The label goes on top (z −1).
pub fn chip_image(cols: u16, rows: u16, cell: (u16, u16), rgb: Rgb) -> RgbaImage {
    face(cols, rows, cell, rgb, None, None)
}

/// A button in `cols`×`rows` cells whose label takes `label.1` cells from
/// cell `label.0`: the mockup's `pill()`. The face is a cell narrower than
/// the button and centred on the label, so a label half a cell off the
/// button's centre (odd parity) still sits exactly in the face's middle.
pub fn pill_image(
    cols: u16,
    rows: u16,
    cell: (u16, u16),
    rgb: Rgb,
    label: (u16, u16),
) -> RgbaImage {
    let cw = f32::from(cell.0.max(1));
    let cx = (f32::from(label.0) + f32::from(label.1) / 2.0) * cw;
    let fw = f32::from(cols.saturating_sub(1).max(1)) * cw;
    face(cols, rows, cell, rgb, Some(cx), Some(fw))
}

/// A meter's box in `cols`×`rows` cells: a flat square face, a shade under
/// the strip.
pub fn inset_image(cols: u16, rows: u16, cell: (u16, u16)) -> RgbaImage {
    let (mut cv, _, _) = canvas(cols, rows, cell);
    let (w, h) = (cv.w as f32, cv.h as f32);
    let shape = Shape::RRect {
        x0: 0.0,
        y0: 0.0,
        x1: w,
        y1: h,
        rad: 0.0,
    };
    cv.fill(shape, PX_INSET);
    cv.into_image()
}

// ----------------------------------------------------------------- fader

/// A fader, `cols`×`rows` cells, at `db` on a scale topping out at
/// `max_db`: a thin flat track, the value filled in the accent below a
/// flat cap with a dark centre line, and flat tick marks on both sides.
/// With `labels` the first and last 3 columns are left clear for the
/// scale's text.
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
    let (mut cv, sx, sy) = canvas(cols, rows, cell);
    let (cw, ch) = (f32::from(cell.0.max(1)), f32::from(cell.1.max(1)));
    let (c0, c1) = if labels && cols > 6 {
        (3.0, f32::from(cols) - 3.0)
    } else {
        (0.0, f32::from(cols))
    };
    let tx = (c0 + c1) / 2.0 * cw;
    let half = (c1 - c0) / 2.0 * cw;
    let (top, bot) = (0.5 * ch, cv.h as f32 - 0.5 * ch);
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| Shape::RRect {
        x0,
        y0,
        x1,
        y1,
        rad: 0.0,
    };
    // Tick marks on both sides, on the scale's rows.
    let (tick_at, tick_w) = ((10.0 * sx).min(half * 0.6), (4.0 * sx).min(half * 0.3));
    for (row, _) in Fader::ticks(max_db, rows) {
        let y = (f32::from(row) + 0.5) * ch;
        for s in [-1.0, 1.0] {
            let x = tx + s * tick_at;
            cv.fill(
                rect(x - tick_w, y - 0.7 * sy, x + tick_w, y + 0.7 * sy),
                PX_FAINT,
            );
        }
    }
    let tw = 2.0 * sx;
    cv.fill(rect(tx - tw, top, tx + tw, bot), PX_ARC_OFF);
    // The cap sits on the row the text cap would (rounded to a row
    // centre), so it lines up with the scale.
    let y = (f32::from(fader_row(frac, rows)) + 0.5) * ch;
    cv.fill(rect(tx - tw, y, tx + tw, bot), PX_ACCENT);
    let (hw, hh) = ((16.0 * sx).min(half - 1.0), 7.0 * sy);
    cv.fill(
        Shape::RRect {
            x0: tx - hw,
            y0: y - hh,
            x1: tx + hw,
            y1: y + hh,
            rad: 1.5,
        },
        PX_FADER_CAP,
    );
    cv.fill(rect(tx - hw, y - sy, tx + hw, y + sy), PX_BG);
    cv.into_image()
}

// ------------------------------------------------------------------ knob

/// A flat knob centred in a see-through `w`×`h` image, its radius 0.80 of
/// half the shorter side: a solid track arc over the 270° sweep, the lit
/// arc (from the minimum, or from 12 o'clock when `bipolar`), a flat disc
/// and a pointer. Dimmed (Monitor Mix): nothing lit, a darker disc, and a
/// dim dot toward a known value instead of the pointer.
pub fn knob_image(w: u32, h: u32, value: Option<f64>, dimmed: bool, bipolar: bool) -> RgbaImage {
    let mut cv = Canvas::new(w, h);
    let (cx, cy) = (cv.w as f32 / 2.0, cv.h as f32 / 2.0);
    let r = cv.w.min(cv.h) as f32 / 2.0 * 0.80;
    let t = 0.13 * r;
    let ra = r - t / 2.0;
    let (start, end) = (START_DEG as f32, (START_DEG - SWEEP_DEG) as f32);
    let arc = |a0: f32, a1: f32| Shape::Arc {
        cx,
        cy,
        r: ra,
        t,
        a0,
        a1,
    };
    cv.fill(arc(start, end), PX_ARC_OFF);
    let at = value.map(|v| start - SWEEP_DEG as f32 * v.clamp(0.0, 1.0) as f32);
    if let (Some(av), false) = (at, dimmed) {
        let (a0, a1) = match bipolar {
            true if av < 90.0 => (90.0, av),
            true => (av, 90.0),
            false => (start, av),
        };
        if (a0 - a1).abs() > 0.5 {
            cv.fill(arc(a0, a1), PX_ACCENT);
        }
    }
    let body = if dimmed {
        PX_KNOB_BODY_DIM
    } else {
        PX_KNOB_BODY
    };
    cv.fill(
        Shape::Circle {
            cx,
            cy,
            r: 0.66 * r,
        },
        body,
    );
    match (at, dimmed) {
        (Some(a), false) => {
            let (ax, ay) = polar(cx, cy, 0.18 * r, a);
            let (bx, by) = polar(cx, cy, 0.56 * r, a);
            let r = 0.055 * r;
            cv.fill(Shape::Capsule { ax, ay, bx, by, r }, PX_POINTER);
        }
        (Some(a), true) => {
            let (dx, dy) = polar(cx, cy, 0.40 * r, a);
            cv.fill(
                Shape::Circle {
                    cx: dx,
                    cy: dy,
                    r: 0.06 * r,
                },
                PX_DIM,
            );
        }
        (None, _) => {}
    }
    cv.into_image()
}
