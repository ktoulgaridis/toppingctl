//! A vertical level meter over the whole height it is given: eighth-cell
//! steps, a green → amber → red gradient up the bar, and an optional clip
//! light on top, held for [`CLIP_HOLD`] after the last clip (like the app's
//! overload lights). Also the dB [`Scale`] next to it and the input's peak
//! readout.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::tui::theme::Theme;

/// The meter's range: 0 dBFS at the top, −60 at the bottom.
pub const METER_RANGE_DB: f64 = 60.0;
/// The input meter's scale labels.
pub const INPUT_SCALE_DB: [f64; 7] = [0.0, -6.0, -12.0, -24.0, -36.0, -48.0, -60.0];
/// A reading at or above this lights the clip light.
pub const CLIP_DB: f64 = -0.1;
/// How long the clip light stays on after the last clip (the app holds its
/// `overload_IN` / `overload_OUT` lights 5 s).
pub const CLIP_HOLD: Duration = Duration::from_secs(5);
/// At or above this the peak readout turns red.
pub const PEAK_HOT_DB: f64 = -3.0;

const EIGHTHS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// `db` as a fraction of the meter: 0 = bottom, 1 = top.
pub fn meter_frac(db: f64) -> f64 {
    ((db + METER_RANGE_DB) / METER_RANGE_DB).clamp(0.0, 1.0)
}

/// A meter. [`Meter::clip`] with `Some` reserves the top row for the clip
/// light.
#[derive(Clone, Debug)]
pub struct Meter {
    db: f64,
    width: u16,
    clip: Option<bool>,
    theme: Theme,
}

impl Meter {
    pub fn new(db: f64, theme: Theme) -> Self {
        Meter {
            db,
            width: 1,
            clip: None,
            theme,
        }
    }

    /// Columns, 1 or 2.
    pub fn width(mut self, width: u16) -> Self {
        self.width = width.max(1);
        self
    }

    /// A clip light on top: `Some(lit)`.
    pub fn clip(mut self, clip: Option<bool>) -> Self {
        self.clip = clip;
        self
    }
}

impl Widget for Meter {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let t = self.theme;
        let w = self.width.min(area.width);
        let mut bar = Rect { width: w, ..area };
        if let Some(lit) = self.clip {
            let (s, st) = if lit {
                ("▀", t.warn())
            } else {
                ("▔", t.track())
            };
            buf.set_string(bar.x, bar.y, s.repeat(usize::from(w)), st);
            bar.y += 1;
            bar.height -= 1;
        }
        let h = bar.height;
        let lit = (meter_frac(self.db) * f64::from(h) * 8.0).round() as i64;
        for i in 0..h {
            let y = bar.bottom() - 1 - i;
            let fill = (lit - i64::from(i) * 8).clamp(0, 8) as usize;
            if fill == 0 {
                let empty = "▕".to_string() + &" ".repeat(usize::from(w - 1));
                buf.set_string(bar.x, y, empty, t.track());
            } else {
                let top = f64::from(i) / f64::from(h.max(1));
                buf.set_string(bar.x, y, EIGHTHS[fill].repeat(usize::from(w)), t.meter(top));
            }
        }
    }
}

/// The clip light's state for one meter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipHold {
    until: Option<Instant>,
}

impl ClipHold {
    /// A reading `db` arrived at `now`.
    pub fn update(&mut self, db: f64, now: Instant) {
        if db >= CLIP_DB {
            self.until = Some(now + CLIP_HOLD);
        }
    }

    /// Whether the light is on at `now`.
    pub fn lit(&self, now: Instant) -> bool {
        self.until.is_some_and(|u| now <= u)
    }
}

/// dB labels, right-aligned in 3 columns, each on the row of its level
/// (0 dB on the top row, −60 on the bottom one). A label whose row is
/// taken is dropped.
#[derive(Clone, Debug)]
pub struct Scale<'a> {
    ticks: &'a [f64],
    theme: Theme,
}

impl<'a> Scale<'a> {
    pub fn new(ticks: &'a [f64], theme: Theme) -> Self {
        Scale { ticks, theme }
    }
}

impl Widget for Scale<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let h = area.height;
        let mut used = Vec::new();
        for &db in self.ticks {
            let row = ((1.0 - meter_frac(db)) * f64::from(h - 1)).round() as u16;
            if used.contains(&row) {
                continue;
            }
            used.push(row);
            let label = format!("{:>3}", db as i32).replace('-', "−");
            buf.set_stringn(
                area.x,
                area.y + row,
                label,
                usize::from(area.width),
                self.theme.scale(),
            );
        }
    }
}

/// The peak readout: whole dB, `−∞` below the meter's range.
pub fn peak_text(db: f64) -> String {
    let v = if db.is_nan() || db < -METER_RANGE_DB {
        "−∞".to_string()
    } else {
        let r = db.round().min(0.0) + 0.0;
        format!("{}", r as i32).replace('-', "−")
    };
    format!("{v:>3} pk")
}
