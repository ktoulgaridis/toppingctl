//! Pixel widgets for terminals with the kitty graphics protocol (kitty,
//! Ghostty), drawn anti-aliased with `tiny-skia` and placed in their cells
//! with `ratatui-image`. Knobs today; meters, faders and the rest are meant
//! to join as more [`PixelWidget`] variants (a [`Key`] arm and a drawing
//! function each).
//!
//! The text UI is always drawn first; [`PixelLayer::render`] then puts an
//! image over the cells of each [`PixelSpot`] the draw recorded. Text
//! (labels, values, scales) stays real terminal text beside the images.
//! Without the protocol there is no layer and the text widgets stay (the
//! Braille knob, block meters, box-drawn faders), and so do the golden
//! snapshots.
//!
//! ratatui-image places kitty images with Unicode placeholders, so an image
//! lives in its cells: when the layout changes or a cell is drawn over,
//! the image goes with it. Images are cached by what they show (kind, size
//! in cells, quantised value): a redraw with the same values reuses the
//! transmitted image, and ratatui's diff then writes nothing. When the
//! cache is full it is dropped and the terminal told to free every image
//! ([`PixelLayer::take_cleanup`]); the next frame transmits afresh.

use std::collections::HashMap;
use std::f32::consts::PI;

use image::{DynamicImage, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LinearGradient, Paint, Path, PathBuilder, Pixmap,
    Point, SpreadMode, Stroke, Transform,
};

use crate::tui::widgets::dial::{START_DEG, SWEEP_DEG};

/// The shared palette (docs/design/knobs/hires.py, style J).
pub const KNOB_ACCENT_RGB: (u8, u8, u8) = (122, 162, 247);
pub const KNOB_DIM_RGB: (u8, u8, u8) = (59, 66, 97);
pub const KNOB_BODY_RGB: (u8, u8, u8) = (40, 44, 60);
pub const KNOB_BODY_TOP_RGB: (u8, u8, u8) = (58, 63, 84);
pub const KNOB_RIM_RGB: (u8, u8, u8) = (130, 136, 160);
pub const KNOB_POINTER_RGB: (u8, u8, u8) = (240, 240, 250);

/// The ticks on the gain knob's arc (style J); pan and Monitor Mix have a
/// solid arc (styles K, L).
pub const GAIN_TICKS: u32 = 21;

/// Something drawn as a pixel image over a cell area.
#[derive(Clone, Debug, PartialEq)]
pub enum PixelWidget {
    /// A knob at `value` (0 ..= 1, `None` unknown).
    Knob {
        value: Option<f64>,
        ticks: u32,
        dimmed: bool,
    },
}

/// A widget and the cells it covers.
#[derive(Clone, Debug, PartialEq)]
pub struct PixelSpot {
    pub rect: Rect,
    pub widget: PixelWidget,
}

/// What an image shows, for the cache: the kind, the size in cells, the
/// quantised values.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Knob(u16, u16, Option<u16>, u32, bool),
}

fn quantise(v: f64, steps: f64) -> u16 {
    (v.clamp(0.0, 1.0) * steps).round() as u16
}

impl PixelSpot {
    fn key(&self) -> Key {
        let (w, h) = (self.rect.width, self.rect.height);
        match &self.widget {
            PixelWidget::Knob {
                value,
                ticks,
                dimmed,
            } => Key::Knob(w, h, value.map(|v| quantise(v, 200.0)), *ticks, *dimmed),
        }
    }

    /// The image for `key`, `px_w`×`px_h` pixels. It is drawn from the
    /// quantised values, so every spot with the same key looks the same.
    fn draw(&self, key: &Key, px_w: u32, px_h: u32) -> RgbaImage {
        match key {
            Key::Knob(_, _, q, ticks, dimmed) => {
                let v = q.map(|q| f64::from(q) / 200.0);
                knob_image(px_w, px_h, v, *ticks, *dimmed)
            }
        }
    }
}

/// The most images kept; past it the cache (and the terminal's copies) are
/// dropped and rebuilt.
const CACHE_LIMIT: usize = 1500;

/// The pixel renderer: the terminal's protocol and cell size, and the
/// cache of placed images.
pub struct PixelLayer {
    picker: Picker,
    cache: HashMap<Key, Protocol>,
    cleanup: bool,
    built: usize,
}

impl PixelLayer {
    /// Asks the terminal (after the alternate screen and raw mode are on,
    /// before any event is read) whether it speaks the kitty graphics
    /// protocol. `None` otherwise, or when `TOPPINGCTL_PIXELS=0`.
    pub fn detect() -> Option<PixelLayer> {
        if std::env::var_os("TOPPINGCTL_PIXELS").is_some_and(|v| v == "0") {
            return None;
        }
        let picker = Picker::from_query_stdio().ok()?;
        (picker.protocol_type() == ProtocolType::Kitty).then(|| PixelLayer::new(picker))
    }

    /// A layer on `picker` (tests use a halfblocks picker set to kitty).
    pub fn new(picker: Picker) -> PixelLayer {
        PixelLayer {
            picker,
            cache: HashMap::new(),
            cleanup: false,
            built: 0,
        }
    }

    /// Images in the cache.
    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    /// Images drawn so far (cache misses).
    pub fn built(&self) -> usize {
        self.built
    }

    /// Whether the terminal should be told to free every image (the cache
    /// was dropped). Clears the flag.
    pub fn take_cleanup(&mut self) -> bool {
        std::mem::take(&mut self.cleanup)
    }

    /// The kitty sequence that frees every image the terminal holds.
    pub const FREE_ALL: &'static str = "\x1b_Ga=d,d=A,q=2\x1b\\";

    /// Puts each spot's image over its cells.
    pub fn render(&mut self, spots: &[PixelSpot], buf: &mut Buffer) {
        if self.cache.len() > CACHE_LIMIT {
            // Drop everything and send nothing this frame: the caller
            // frees the terminal's copies, and the next frame starts over.
            self.cache.clear();
            self.cleanup = true;
            return;
        }
        let (fw, fh) = self.picker.font_size();
        for spot in spots {
            let r = spot.rect.intersection(buf.area);
            if r.is_empty() || r != spot.rect {
                continue;
            }
            let key = spot.key();
            if !self.cache.contains_key(&key) {
                let px_w = u32::from(r.width) * u32::from(fw);
                let px_h = u32::from(r.height) * u32::from(fh);
                let img = DynamicImage::ImageRgba8(spot.draw(&key, px_w, px_h));
                let size = Rect::new(0, 0, r.width, r.height);
                let Ok(p) = self.picker.new_protocol(img, size, Resize::Fit(None)) else {
                    continue;
                };
                self.built += 1;
                self.cache.insert(key.clone(), p);
            }
            if let Some(p) = self.cache.get(&key) {
                // Clear first: no text of the fallback shows through.
                for y in r.top()..r.bottom() {
                    for x in r.left()..r.right() {
                        buf[(x, y)].reset();
                    }
                }
                ratatui_image::Image::new(p).render(r, buf);
            }
        }
    }
}

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
    let Some(mut pm) = Pixmap::new(w.max(1), h.max(1)) else {
        return RgbaImage::new(w.max(1), h.max(1));
    };
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let r = w.min(h) as f32 / 2.0;
    let sweep = SWEEP_DEG as f32;
    let lit_to = match (value, dimmed) {
        (Some(v), false) => sweep * v as f32,
        _ => -1.0,
    };
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
    for (a, b) in segments {
        let split = lit_to.clamp(a, b);
        if split > a {
            if let Some(path) = arc(cx, cy, 0.87 * r, a, split) {
                pm.stroke_path(&path, &solid(KNOB_ACCENT_RGB), &band, id, None);
            }
        }
        if split < b {
            if let Some(path) = arc(cx, cy, 0.87 * r, split, b) {
                pm.stroke_path(&path, &solid(KNOB_DIM_RGB), &band, id, None);
            }
        }
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
    if let (Some(v), false) = (value, dimmed) {
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
