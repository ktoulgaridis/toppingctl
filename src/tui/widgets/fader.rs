//! A vertical fader: a track with a cap, and a scale of 0 / −6 / −12 / −24 /
//! −∞ ticks on both sides. The value above the cap is [`fmt_db`].
//!
//! The ticks are spread evenly up the track (as on the app's faders), with
//! the position in between linear in dB; below −24 dB it runs linearly to
//! the bottom, where −∞ sits. A mix fader tops out at +6 dB (an unlabelled
//! position above 0) and runs down to −60 dB; a loopback or output fader
//! tops out at 0 dB, labelled −0 as in the app, and runs down to −90 dB
//! (the app's slider range).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::tui::theme::Theme;

/// The labelled ticks, top to bottom.
pub const FADER_TICKS_DB: [f64; 5] = [0.0, -6.0, -12.0, -24.0, f64::NEG_INFINITY];
/// The bottom of a mix fader's scale: at or below this it is at −∞.
const FLOOR_DB: f64 = -60.0;
/// The bottom of a loopback or output fader's scale (the app's −90).
const BUS_FLOOR_DB: f64 = -90.0;

/// The bottom of the scale of a fader topping out at `max_db`.
pub fn floor_db(max_db: f64) -> f64 {
    if max_db > 0.0 {
        FLOOR_DB
    } else {
        BUS_FLOOR_DB
    }
}

/// The ticks' dB values, top to bottom, for a fader topping out at `max_db`.
fn anchors(max_db: f64) -> Vec<f64> {
    let floor = floor_db(max_db);
    let mut a = Vec::with_capacity(6);
    if max_db > 0.0 {
        a.push(max_db);
    }
    a.extend(FADER_TICKS_DB.iter().map(|&d| d.max(floor)));
    a
}

/// Where `db` sits on a fader topping out at `max_db`: 0 = bottom, 1 = top.
pub fn fader_frac(db: f64, max_db: f64) -> f64 {
    let a = anchors(max_db);
    let n = (a.len() - 1) as f64;
    if db.is_nan() || db <= floor_db(max_db) {
        return 0.0;
    }
    if db >= a[0] {
        return 1.0;
    }
    for i in 0..a.len() - 1 {
        let (hi, lo) = (a[i], a[i + 1]);
        if db <= hi && db >= lo {
            let u = (db - lo) / (hi - lo);
            return (n - (i + 1) as f64 + u) / n;
        }
    }
    0.0
}

/// The value at position `frac` (0 = bottom, 1 = top) on a fader topping
/// out at `max_db`: the inverse of [`fader_frac`]. The bottom, and
/// anything at or below the scale's floor, is −∞.
pub fn db_at_frac(frac: f64, max_db: f64) -> f64 {
    let a = anchors(max_db);
    let n = a.len() - 1;
    if frac.is_nan() || frac <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if frac >= 1.0 {
        return a[0];
    }
    let pos = (1.0 - frac) * n as f64;
    let i = (pos.floor() as usize).min(n - 1);
    let v = pos - i as f64;
    let db = a[i] + (a[i + 1] - a[i]) * v;
    if db <= floor_db(max_db) {
        f64::NEG_INFINITY
    } else {
        db
    }
}

/// A fader value: `−0.0 dB` (0 dB reads −0, as in the official app),
/// `−6.0 dB`, `+3.0 dB`, `−∞ dB`.
pub fn fmt_db(db: f64) -> String {
    if db.is_nan() || db <= FLOOR_DB {
        return "−∞ dB".into();
    }
    let r = (db * 10.0).round() / 10.0;
    if r == 0.0 {
        "−0.0 dB".into()
    } else {
        format!("{r:+.1} dB").replace('-', "−")
    }
}

/// The tick label for `db` on a fader topping out at `max_db`: a loopback
/// or output fader's top reads −0, as in the app.
fn tick_label(db: f64, max_db: f64) -> String {
    if db == f64::NEG_INFINITY {
        "−∞".into()
    } else if db == 0.0 && max_db <= 0.0 {
        "−0".into()
    } else {
        format!("{}", db as i32).replace('-', "−")
    }
}

/// A loopback or output level as the app reads it (`−%d`): `−6`, `−0`,
/// `−∞`.
pub fn level_text(db: f64) -> String {
    if db.is_nan() || db <= BUS_FLOOR_DB {
        "−∞".into()
    } else {
        format!("−{}", (-db.round()).max(0.0) as i32)
    }
}

/// The row (0 = top) of position `frac` on `h` rows.
pub fn fader_row(frac: f64, h: u16) -> u16 {
    ((1.0 - frac) * f64::from(h.saturating_sub(1))).round() as u16
}

/// A fader drawn in its whole area: `LLL track LLL` (track 5 wide from 11
/// columns, else 3), or with [`Fader::labels`] off, a 3-wide track with tick
/// marks on both sides.
#[derive(Clone, Debug)]
pub struct Fader {
    db: f64,
    max_db: f64,
    labels: bool,
    focused: bool,
    theme: Theme,
}

impl Fader {
    pub fn new(db: f64, max_db: f64, theme: Theme) -> Self {
        Fader {
            db,
            max_db,
            labels: true,
            focused: false,
            theme,
        }
    }

    pub fn labels(mut self, labels: bool) -> Self {
        self.labels = labels;
        self
    }

    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// The width this fader needs for `available` columns.
    pub fn width(labels: bool, available: u16) -> u16 {
        match (labels, available) {
            (false, _) => 3,
            (true, w) if w >= 11 => 11,
            _ => 9,
        }
    }

    /// The cells a fader takes in `area`: [`Fader::width`] columns,
    /// centred, the whole height.
    pub fn rect(labels: bool, area: Rect) -> Rect {
        let w = Fader::width(labels, area.width).min(area.width);
        Rect {
            x: area.x + (area.width - w) / 2,
            width: w,
            ..area
        }
    }

    /// The tick rows on `h` rows: `(row, label)`, top to bottom, dropping a
    /// tick whose row is taken.
    pub fn ticks(max_db: f64, h: u16) -> Vec<(u16, String)> {
        let mut out: Vec<(u16, String)> = Vec::new();
        for db in FADER_TICKS_DB {
            let row = fader_row(fader_frac(db, max_db), h);
            if !out.iter().any(|(r, _)| *r == row) {
                out.push((row, tick_label(db, max_db)));
            }
        }
        out
    }
}

impl Widget for Fader {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let t = self.theme;
        let h = area.height;
        let Rect { x, width: w, .. } = Fader::rect(self.labels, area);
        let cap_row = fader_row(fader_frac(self.db, self.max_db), h);
        let ticks = Fader::ticks(self.max_db, h);
        let cap_style = if self.focused { t.focus() } else { t.heading() };
        let put = |buf: &mut Buffer, dx: u16, y: u16, s: &str, st: Style| {
            if dx < w {
                buf.set_stringn(x + dx, area.y + y, s, usize::from(w - dx), st);
            }
        };
        for row in 0..h {
            let label = ticks.iter().find(|(r, _)| *r == row).map(|(_, l)| l);
            let cap = row == cap_row;
            if self.labels {
                let track = w - 6;
                if let Some(l) = label {
                    put(buf, 0, row, &format!("{l:>3}"), t.scale());
                    put(buf, 3 + track, row, &format!("{l:<3}"), t.scale());
                }
                let (s, st) = match (cap, track) {
                    (true, 5) => ("━━■━━", cap_style),
                    (true, _) => ("━■━", cap_style),
                    (false, 5) => ("  ┃  ", t.track()),
                    (false, _) => (" ┃ ", t.track()),
                };
                put(buf, 3, row, s, st);
            } else if cap {
                put(buf, 0, row, "━■━", cap_style);
            } else {
                if label.is_some() {
                    put(buf, 0, row, "╶", t.scale());
                    put(buf, 2, row, "╴", t.scale());
                }
                put(buf, 1, row, "┃", t.track());
            }
        }
    }
}
