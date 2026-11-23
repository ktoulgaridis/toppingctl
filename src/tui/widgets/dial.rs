//! A rotary knob drawn with Braille dots (prototype style G in
//! `docs/design/knobs/dials.py`): a thick 270° ring lit in the accent up to
//! the value, a rim, and a pointer line from the centre, with the value
//! underneath. This is the knob in every terminal without the kitty
//! graphics protocol, and in the golden snapshots; kitty and Ghostty get
//! the pixel knob of [`crate::tui::pixel`] in the same cells.
//!
//! The knob sweeps from down-left (min) through up (centre) to down-right
//! (max), like a pot, and the arc is lit from the minimum. Dimmed (Monitor
//! Mix: read-only, mirroring the balance the device reports) draws the ring
//! dim with nothing lit, and the pointer only for a known value.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Widget;

use crate::tui::theme::Theme;

/// The degrees the knob sweeps, clockwise from its minimum at 225°.
pub const START_DEG: f64 = 225.0;
pub const SWEEP_DEG: f64 = 270.0;

/// The knob's cells inside `area`: at most `max` (cols, rows), at least
/// the knob's round shape (two columns per row), centred, above the label
/// row when there is one.
pub fn knob_rect(area: Rect, label: bool, max: (u16, u16)) -> Rect {
    let h_avail = area
        .height
        .saturating_sub(u16::from(label && area.height > 1));
    let rows = h_avail.min(max.1).min(area.width / 2).max(1);
    let cols = (rows * 2).min(max.0).min(area.width);
    let rows = rows.min(h_avail);
    Rect::new(
        area.x + (area.width - cols) / 2,
        area.y + (h_avail - rows) / 2,
        cols,
        rows,
    )
}

/// A knob. It is centred in its area, at most [`Dial::max_cells`], with
/// the label on the area's last row.
#[derive(Clone, Debug)]
pub struct Dial {
    value: Option<f64>,
    dimmed: bool,
    bipolar: bool,
    focused: bool,
    label: Option<String>,
    max: (u16, u16),
    theme: Theme,
}

/// The lit part of the arc, in degrees along the sweep (`None`: nothing):
/// from the minimum to the value, or for a bipolar knob (a pan) from the
/// top (12 o'clock) to the value.
pub fn lit_span(value: Option<f64>, dimmed: bool, bipolar: bool) -> Option<(f64, f64)> {
    let v = value.filter(|_| !dimmed)?;
    let at = SWEEP_DEG * v;
    if !bipolar {
        return Some((0.0, at));
    }
    let top = SWEEP_DEG / 2.0;
    ((at - top).abs() > 1e-9).then(|| (at.min(top), at.max(top)))
}

/// What a pixel is: the parts of the knob, in drawing priority.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Dim,
    Rim,
    Lit,
    Pointer,
}

impl Dial {
    pub fn new(theme: Theme) -> Self {
        Dial {
            value: None,
            dimmed: false,
            bipolar: false,
            focused: false,
            label: None,
            max: (12, 6),
            theme,
        }
    }

    /// The position, 0 ..= 1; `None` when unknown (no pointer, nothing lit).
    pub fn value(mut self, value: Option<f64>) -> Self {
        self.value = value.map(|v| v.clamp(0.0, 1.0));
        self
    }

    /// Dim everything, light no arc (Monitor Mix); a known value still
    /// gets its pointer.
    pub fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }

    /// Light the arc from the top to the value (a pan), not from the
    /// minimum.
    pub fn bipolar(mut self, bipolar: bool) -> Self {
        self.bipolar = bipolar;
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// The value text, printed under the knob.
    pub fn label(mut self, label: Option<String>) -> Self {
        self.label = label;
        self
    }

    /// The largest knob, in cells.
    pub fn max_cells(mut self, cols: u16, rows: u16) -> Self {
        self.max = (cols, rows);
        self
    }

    /// The knob's dot grid, `w`×`h` square dots: style G.
    fn grid(&self, w: usize, h: usize) -> Vec<Option<Part>> {
        let mut g = vec![None; w * h];
        let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
        let r = w.min(h) as f64 / 2.0;
        let lit = lit_span(self.value, self.dimmed, self.bipolar);
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f64 - cx, cy - y as f64);
                let d = dx.hypot(dy) / r;
                let rel = (START_DEG - dy.atan2(dx).to_degrees()).rem_euclid(360.0);
                g[y * w + x] = if (0.80..=1.0).contains(&d) && rel <= SWEEP_DEG {
                    let on = lit.is_some_and(|(a, b)| rel >= a && rel <= b);
                    Some(if on { Part::Lit } else { Part::Dim })
                } else if (0.58..0.68).contains(&d) {
                    Some(Part::Rim)
                } else {
                    None
                };
            }
        }
        // The pointer shows a known value, dimmed or not.
        if let Some(v) = self.value {
            let a = (START_DEG - SWEEP_DEG * v).to_radians();
            for i in 0..40 {
                let t = f64::from(i) / 39.0 * 0.62 * r;
                let x = (cx + t * a.cos()).round();
                let y = (cy - t * a.sin()).round();
                if x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h {
                    g[y as usize * w + x as usize] = Some(Part::Pointer);
                }
            }
        }
        g
    }
}

impl Widget for Dial {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let t = self.theme;
        let knob = knob_rect(area, self.label.is_some(), self.max);
        let (w, h) = (usize::from(knob.width) * 2, usize::from(knob.height) * 4);
        let g = self.grid(w, h);
        const BITS: [(usize, usize, u32); 8] = [
            (0, 0, 0x01),
            (0, 1, 0x02),
            (0, 2, 0x04),
            (1, 0, 0x08),
            (1, 1, 0x10),
            (1, 2, 0x20),
            (0, 3, 0x40),
            (1, 3, 0x80),
        ];
        let colour = |p: Part| {
            if !t.is_color() {
                return Color::Reset;
            }
            match p {
                Part::Dim => Theme::KNOB_DIM,
                Part::Rim => Theme::KNOB_RIM,
                Part::Lit => Theme::KNOB_ACCENT,
                Part::Pointer => Theme::KNOB_POINTER,
            }
        };
        for cy in 0..usize::from(knob.height) {
            for cx in 0..usize::from(knob.width) {
                let mut bits = 0;
                let mut best: Option<Part> = None;
                for (dx, dy, bit) in BITS {
                    if let Some(p) = g[(cy * 4 + dy) * w + cx * 2 + dx] {
                        bits |= bit;
                        best = best.max(Some(p));
                    }
                }
                if let (Some(p), Some(ch)) = (best, char::from_u32(0x2800 + bits)) {
                    let pos = (knob.x + cx as u16, knob.y + cy as u16);
                    buf[pos].set_char(ch).set_style(Style::new().fg(colour(p)));
                }
            }
        }
        if let (Some(label), true) = (&self.label, area.height > 1) {
            let style = if self.focused {
                t.focus().add_modifier(Modifier::REVERSED)
            } else {
                t.heading()
            };
            let row = Rect::new(area.x, area.bottom() - 1, area.width, 1);
            Line::styled(label.clone(), style)
                .centered()
                .render(row, buf);
        }
    }
}
