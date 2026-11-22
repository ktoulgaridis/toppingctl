//! A rotary knob, like the official app's: an arc of ticks over 270°, lit in
//! the accent colour up to the value, a round knob with a pointer from the
//! centre, and the value underneath.
//!
//! The knob sweeps from down-left (min) through up (centre) to down-right
//! (max), like a pot. A bipolar knob (pan) lights the arc from the centre.
//! With no value (Monitor Mix: the device never reports it) the ticks and
//! the knob are drawn dim, with no pointer and nothing lit.
//!
//! Two looks, both sized to the area they get ([`DialStyle`]):
//! [`DialStyle::Glyph`] draws with text glyphs (`●` lit ticks, `·` unlit
//! ones, a rounded knob outline and a line pointer); [`DialStyle::Braille`]
//! draws the same parts with Braille dots on a ratatui `Canvas`.

use std::f64::consts::PI;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::Line;
use ratatui::widgets::canvas::{Canvas, Points};
use ratatui::widgets::Widget;

use crate::tui::theme::Theme;

/// The smallest knob, in cells.
pub const DIAL_WIDTH: u16 = 5;
pub const DIAL_HEIGHT: u16 = 3;

/// How the knob is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DialStyle {
    /// Text glyphs: crisp at any size (the default).
    #[default]
    Glyph,
    /// Braille dots on a `Canvas`.
    Braille,
}

/// A knob. It fills its area; with a label, the last row holds the label.
#[derive(Clone, Debug)]
pub struct Dial {
    value: Option<f64>,
    bipolar: bool,
    focused: bool,
    label: Option<String>,
    style: DialStyle,
    theme: Theme,
}

impl Dial {
    pub fn new(theme: Theme) -> Self {
        Dial {
            value: None,
            bipolar: false,
            focused: false,
            label: None,
            style: DialStyle::default(),
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

    pub fn style(mut self, style: DialStyle) -> Self {
        self.style = style;
        self
    }

    /// Whether tick position `p` (0 ..= 1) is lit.
    fn lit(&self, p: f64) -> bool {
        let Some(v) = self.value else {
            return false;
        };
        let from = if self.bipolar { 0.5 } else { 0.0 };
        let (lo, hi) = if v < from { (v, from) } else { (from, v) };
        hi - lo > 1e-9 && p >= lo - 1e-9 && p <= hi + 1e-9
    }

    fn colours(&self) -> Colours {
        let t = self.theme;
        let c = |c: Color| if t.is_color() { c } else { Color::Reset };
        Colours {
            tick: c(Theme::DIM),
            lit: c(Theme::ACCENT),
            body: c(if self.value.is_some() {
                Theme::TEXT
            } else {
                Theme::DIM
            }),
            pointer: c(Theme::ACCENT),
        }
    }
}

struct Colours {
    tick: Color,
    lit: Color,
    body: Color,
    pointer: Color,
}

/// An arrow pointing along angle `a` (radians), to the nearest 45°.
fn arrow(a: f64) -> &'static str {
    const ARROWS: [&str; 8] = ["→", "↗", "↑", "↖", "←", "↙", "↓", "↘"];
    let i = (a.to_degrees().rem_euclid(360.0) / 45.0).round() as usize % 8;
    ARROWS[i]
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
        let t = self.theme;
        let label_row = self.label.is_some() && area.height > DIAL_HEIGHT;
        let knob = Rect {
            height: area.height - u16::from(label_row),
            ..area
        };
        match self.style {
            DialStyle::Glyph => self.render_glyph(knob, buf),
            DialStyle::Braille => self.render_braille(knob, buf),
        }
        if let (true, Some(label)) = (label_row, &self.label) {
            let style = if self.focused { t.focus() } else { t.heading() };
            let row = Rect::new(area.x, area.bottom() - 1, area.width, 1);
            Line::styled(label.clone(), style)
                .centered()
                .render(row, buf);
        }
    }
}

impl Dial {
    /// Text glyphs. Cells are about twice as tall as wide, so the ring is an
    /// ellipse twice as wide as tall, which looks round.
    fn render_glyph(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 5 || area.width < 9 {
            return self.render_box(area, buf);
        }
        let c = self.colours();
        let style = |col: Color| Style::new().fg(col);
        // The ring's radii in cells: as big as fits, round on screen.
        // Whole-cell radii, the width twice the height, keep the ticks even.
        let ry = f64::from(((area.height - 1) / 2).min((area.width - 1) / 4).max(1));
        let rx = ry * 2.0;
        // A whole-cell centre keeps the two sides mirror images.
        let cx = f64::from(area.x + (area.width - 1) / 2);
        let cy = f64::from(area.y + (area.height - 1) / 2);
        let cell = |x: f64, y: f64| -> Option<(u16, u16)> {
            let (x, y) = (x.round(), y.round());
            let inside = x >= f64::from(area.left())
                && x < f64::from(area.right())
                && y >= f64::from(area.top())
                && y < f64::from(area.bottom());
            inside.then_some((x as u16, y as u16))
        };
        let set = |buf: &mut Buffer, p: Option<(u16, u16)>, s: &str, st: Style| {
            if let Some(p) = p {
                buf[p].set_symbol(s).set_style(st);
            }
        };

        // Ticks: more of them on a bigger ring.
        let n = if rx >= 6.0 {
            13
        } else if rx >= 4.0 {
            9
        } else {
            7
        };
        for i in 0..n {
            let p = i as f64 / (n - 1) as f64;
            let a = dial_angle(p);
            let at = cell(cx + rx * a.cos(), cy - ry * a.sin());
            if self.lit(p) {
                set(buf, at, "●", style(c.lit));
            } else {
                set(buf, at, "·", style(c.tick));
            }
        }

        // The knob: a rounded outline inside the ring, when there is room.
        let (bx, by) = ((rx - 2.0).floor(), (ry - 1.0).floor());
        let body = bx >= 2.0 && by >= 1.0;
        if body {
            let (x0, x1) = (cx - bx, cx + bx);
            let (y0, y1) = (cy - by, cy + by);
            let b = style(c.body);
            let mut x = x0 + 1.0;
            while x < x1 {
                set(buf, cell(x, y0), "─", b);
                set(buf, cell(x, y1), "─", b);
                x += 1.0;
            }
            let mut y = y0 + 1.0;
            while y < y1 {
                set(buf, cell(x0, y), "│", b);
                set(buf, cell(x1, y), "│", b);
                y += 1.0;
            }
            set(buf, cell(x0, y0), "╭", b);
            set(buf, cell(x1, y0), "╮", b);
            set(buf, cell(x0, y1), "╰", b);
            set(buf, cell(x1, y1), "╯", b);
        }

        // The pointer: a line from the centre toward the value.
        let Some(v) = self.value else {
            set(buf, cell(cx, cy), "·", style(c.body));
            return;
        };
        let a = dial_angle(v);
        let (dx, dy) = (a.cos(), a.sin());
        let glyph = if dx.abs() > 0.92 {
            "─"
        } else if dy.abs() > 0.92 {
            "│"
        } else if dx * dy > 0.0 {
            "╱"
        } else {
            "╲"
        };
        if body && by < 2.0 {
            // One row inside the knob: the hub itself points.
            set(buf, cell(cx, cy), arrow(a), style(c.pointer));
            return;
        }
        let reach = if body {
            (bx - 1.0).min((by - 1.0) * 2.0)
        } else {
            (rx - 1.0).max(1.0)
        };
        let steps = (reach * 2.0).ceil() as i32;
        let mut seen = Vec::new();
        for k in 1..=steps {
            let d = reach * f64::from(k) / f64::from(steps);
            let p = cell(cx + d * dx, cy - d * dy / 2.0);
            if p.is_some() && !seen.contains(&p) && p != cell(cx, cy) {
                seen.push(p);
                set(buf, p, glyph, style(c.pointer));
            }
        }
        set(buf, cell(cx, cy), arrow(a), style(c.pointer));
    }

    /// A small knob: a rounded box whose border is the 270° scale — up the
    /// left side, across the top, down the right — drawn heavy in the
    /// accent up to the value, with an arrow in the middle pointing at it.
    fn render_box(&self, area: Rect, buf: &mut Buffer) {
        let c = self.colours();
        if area.width < 3 || area.height < 3 {
            return;
        }
        let w = area.width.clamp(3, 7);
        let h = area.height.clamp(3, 4);
        let x0 = area.x + (area.width - w) / 2;
        let y0 = area.y + (area.height - h) / 2;
        let (x1, y1) = (x0 + w - 1, y0 + h - 1);
        // The border cells along the scale, min to max.
        let mut path: Vec<(u16, u16)> = Vec::new();
        for y in (y0..=y1).rev() {
            path.push((x0, y));
        }
        for x in x0 + 1..x1 {
            path.push((x, y0));
        }
        for y in y0..=y1 {
            path.push((x1, y));
        }
        let n = path.len();
        for (i, &(x, y)) in path.iter().enumerate() {
            let p = i as f64 / (n - 1) as f64;
            let lit = self.lit(p);
            let corner = |light: &'static str, heavy: &'static str| if lit { heavy } else { light };
            let g = match (x == x0, x == x1, y == y0, y == y1) {
                (true, _, true, _) => corner("╭", "┏"),
                (_, true, true, _) => corner("╮", "┓"),
                (true, _, _, true) => corner("╰", "┗"),
                (_, true, _, true) => corner("╯", "┛"),
                (_, _, true, _) => corner("─", "━"),
                _ => corner("│", "┃"),
            };
            let col = if lit { c.lit } else { c.tick };
            buf[(x, y)].set_symbol(g).set_style(Style::new().fg(col));
        }
        let hub = (x0 + w / 2, y0 + (h - 1) / 2);
        match self.value {
            Some(v) => {
                buf[hub]
                    .set_symbol(arrow(dial_angle(v)))
                    .set_style(Style::new().fg(c.pointer));
            }
            None => {
                buf[hub].set_symbol("·").set_style(Style::new().fg(c.body));
            }
        }
    }

    /// Braille dots: ticks and the lit arc on the ring, a round knob
    /// outline, and the pointer.
    fn render_braille(&self, area: Rect, buf: &mut Buffer) {
        let c = self.colours();
        let (w, h) = (f64::from(area.width) * 2.0, f64::from(area.height) * 4.0);
        let r = (w.min(h) - 1.0) / 2.0;
        let mut ticks = Vec::new();
        let mut lit = Vec::new();
        let n = if r >= 8.0 { 13 } else { 11 };
        for i in 0..n {
            let p = i as f64 / (n - 1) as f64;
            let a = dial_angle(p);
            let dest = if self.lit(p) { &mut lit } else { &mut ticks };
            dest.push(on_circle(r, a));
            dest.push(on_circle(r - 1.0, a));
        }
        if self.value.is_some() {
            let steps = (r * 6.0).ceil() as usize;
            for i in 0..=steps {
                let p = i as f64 / steps as f64;
                if self.lit(p) {
                    lit.push(on_circle(r, dial_angle(p)));
                    lit.push(on_circle(r - 1.0, dial_angle(p)));
                }
            }
        }
        let body_r = (r * 0.55).max(1.5);
        let body: Vec<(f64, f64)> = (0..48)
            .map(|i| on_circle(body_r, f64::from(i) * PI / 24.0))
            .collect();
        let mut needle = Vec::new();
        if let Some(v) = self.value {
            let a = dial_angle(v);
            for i in 0..=8 {
                needle.push(on_circle(body_r * f64::from(i) / 8.0, a));
            }
        }
        let (hw, hh) = ((w - 1.0) / 2.0, (h - 1.0) / 2.0);
        Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([-hw, hw])
            .y_bounds([-hh, hh])
            .paint(|ctx| {
                ctx.draw(&Points {
                    coords: &ticks,
                    color: c.tick,
                });
                ctx.draw(&Points {
                    coords: &body,
                    color: c.body,
                });
                ctx.draw(&Points {
                    coords: &lit,
                    color: c.lit,
                });
                ctx.draw(&Points {
                    coords: &needle,
                    color: c.pointer,
                });
            })
            .render(area, buf);
    }
}
