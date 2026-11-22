//! A rotary knob drawn with Braille dots on a ratatui `Canvas`: a 270° scale
//! of ticks, an arc lit up to the value, a pointer, and the value under it.
//!
//! The knob sweeps from down-left (min) through up (centre) to down-right
//! (max), like a pot. A bipolar knob (pan) lights the arc from the centre.
//! With no value (Monitor Mix: the device never reports it) only the dim
//! scale is drawn.

use std::f64::consts::PI;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::symbols::Marker;
use ratatui::text::Line;
use ratatui::widgets::canvas::{Canvas, Points};
use ratatui::widgets::Widget;

use crate::tui::theme::Theme;

/// The knob's size in cells (10×12 Braille dots).
pub const DIAL_WIDTH: u16 = 5;
pub const DIAL_HEIGHT: u16 = 3;

/// The scale's tick count over the 270° sweep.
const TICKS: usize = 11;
/// Radii, in dots: the scale and lit arc, and the pointer's length.
const RING: f64 = 4.5;
const RING_INNER: f64 = 3.5;
const POINTER: f64 = 2.6;

/// A knob. Its area's top [`DIAL_HEIGHT`] rows hold the knob, centred; a
/// row below that holds the label.
#[derive(Clone, Debug)]
pub struct Dial {
    value: Option<f64>,
    bipolar: bool,
    focused: bool,
    label: Option<String>,
    theme: Theme,
}

impl Dial {
    pub fn new(theme: Theme) -> Self {
        Dial {
            value: None,
            bipolar: false,
            focused: false,
            label: None,
            theme,
        }
    }

    /// The position, 0 ..= 1; `None` when unknown.
    pub fn value(mut self, value: Option<f64>) -> Self {
        self.value = value.map(|v| v.clamp(0.0, 1.0));
        self
    }

    /// Light the arc from the centre (pan) instead of from the minimum.
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
}

/// The angle (radians, 0 = right, counter-clockwise) for position `v`.
pub fn dial_angle(v: f64) -> f64 {
    (225.0 - 270.0 * v.clamp(0.0, 1.0)) * PI / 180.0
}

fn on_circle(r: f64, a: f64) -> (f64, f64) {
    (r * a.cos(), r * a.sin())
}

impl Widget for Dial {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let w = DIAL_WIDTH.min(area.width);
        let h = DIAL_HEIGHT.min(area.height);
        let knob = Rect::new(area.x + (area.width - w) / 2, area.y, w, h);
        let t = self.theme;
        let colour = |c: Color| if t.is_color() { c } else { Color::Reset };
        let tick = colour(Theme::TRACK);
        let lit = colour(if self.value.is_some() {
            Theme::ACCENT
        } else {
            Theme::TRACK
        });
        let pointer = colour(if self.focused {
            Theme::ACCENT
        } else {
            Theme::TEXT
        });

        let ticks: Vec<(f64, f64)> = (0..TICKS)
            .map(|i| on_circle(RING, dial_angle(i as f64 / (TICKS - 1) as f64)))
            .collect();
        let mut arc = Vec::new();
        if let Some(v) = self.value {
            let from = if self.bipolar { 0.5 } else { 0.0 };
            let (lo, hi) = if v < from { (v, from) } else { (from, v) };
            let steps = ((hi - lo) * 54.0).round() as usize;
            for i in 0..=steps {
                if hi - lo < 1e-9 {
                    break;
                }
                let a = dial_angle(lo + (hi - lo) * i as f64 / steps.max(1) as f64);
                arc.push(on_circle(RING, a));
                arc.push(on_circle(RING_INNER, a));
            }
        }
        let mut needle = Vec::new();
        if let Some(v) = self.value {
            let a = dial_angle(v);
            for i in 0..=6 {
                needle.push(on_circle(POINTER * f64::from(i) / 6.0, a));
            }
        }
        // One unit is one dot; the centre is the middle of the dot grid.
        let half_w = (f64::from(w) * 2.0 - 1.0) / 2.0;
        let half_h = (f64::from(h) * 4.0 - 1.0) / 2.0;
        Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([-half_w, half_w])
            .y_bounds([-half_h, half_h])
            .paint(|ctx| {
                ctx.draw(&Points {
                    coords: &ticks,
                    color: tick,
                });
                ctx.draw(&Points {
                    coords: &arc,
                    color: lit,
                });
                ctx.draw(&Points {
                    coords: &needle,
                    color: pointer,
                });
            })
            .render(knob, buf);

        if let Some(label) = self.label {
            if area.height > DIAL_HEIGHT {
                let style = if self.focused { t.focus() } else { t.heading() };
                let row = Rect::new(area.x, area.y + DIAL_HEIGHT, area.width, 1);
                Line::styled(label, style).centered().render(row, buf);
            }
        }
    }
}
