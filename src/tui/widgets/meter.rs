//! A vertical level meter over the whole height it is given, the text form
//! of the pixel LED ladder (#41): one square `▆` segment a row (the gap
//! above it parts the rows), flat bands by height (red at the top 8 %,
//! amber to 30 %, green below), unlit rows an empty track; and an optional
//! square clip light on top, held for [`CLIP_HOLD`] after the last clip
//! (like the app's overload lights). Also the dB [`Scale`] next to it and
//! the input's peak readout.

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

/// A lit segment, and an unlit one.
const LED: &str = "▆";
const OFF: &str = "▕";

/// `db` as a fraction of the meter: 0 = bottom, 1 = top.
pub fn meter_frac(db: f64) -> f64 {
    ((db + METER_RANGE_DB) / METER_RANGE_DB).clamp(0.0, 1.0)
}

/// A meter, one column wide in any area (one meter width everywhere).
/// [`Meter::clip`] with `Some` reserves the top row for the clip light.
#[derive(Clone, Debug)]
pub struct Meter {
    db: f64,
    clip: Option<bool>,
    theme: Theme,
}

impl Meter {
    pub fn new(db: f64, theme: Theme) -> Self {
        Meter {
            db,
            clip: None,
            theme,
        }
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
        let mut bar = Rect { width: 1, ..area };
        if let Some(lit) = self.clip {
            let (s, st) = if lit {
                ("▀", t.fg(Theme::METER_RED))
            } else {
                ("▔", t.track())
            };
            buf.set_string(bar.x, bar.y, s, st);
            bar.y += 1;
            bar.height -= 1;
        }
        let h = bar.height;
        let lit = (meter_frac(self.db) * f64::from(h)).round() as u16;
        for i in 0..h {
            let y = bar.bottom() - 1 - i;
            if i < lit {
                let from_top = (f64::from(h - 1 - i) + 0.5) / f64::from(h.max(1));
                buf.set_string(bar.x, y, LED, t.meter(from_top));
            } else {
                buf.set_string(bar.x, y, OFF, t.track());
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
