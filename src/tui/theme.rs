//! The TUI's palette (docs/design → Style): the terminal's own background,
//! dim borders, one soft-blue accent, a green → amber → red meter gradient,
//! red for 48V, MUTE and clips. The mix tabs (A green, B blue, C amber) are
//! the one exception to the single accent. With `NO_COLOR` set no colour is
//! used at all; bold, dim, underline and reverse still mark focus and state.

use std::ffi::OsStr;

use ratatui::style::{Color, Modifier, Style};

/// The styles every widget draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    color: bool,
}

impl Theme {
    /// The one accent colour: focus and active states.
    pub const ACCENT: Color = Color::Rgb(122, 196, 255);
    /// Panel borders.
    pub const BORDER: Color = Color::Rgb(70, 74, 88);
    /// Secondary text.
    pub const DIM: Color = Color::Rgb(115, 120, 135);
    /// Plain text.
    pub const TEXT: Color = Color::Rgb(205, 210, 220);
    /// Empty tracks, inner borders, greyed-out controls.
    pub const TRACK: Color = Color::Rgb(45, 48, 58);
    /// Meter scale labels.
    pub const SCALE: Color = Color::Rgb(85, 90, 105);
    /// The background of a pill that is off.
    pub const PILL_OFF_BG: Color = Color::Rgb(38, 41, 50);
    /// Text on a lit pill.
    pub const PILL_ON_FG: Color = Color::Rgb(20, 22, 28);
    /// 48V, MUTE, clips, a peak near 0.
    pub const WARN: Color = Color::Rgb(236, 110, 96);
    /// The sync dot.
    pub const OK: Color = Color::Rgb(80, 200, 120);
    /// The knobs' palette (docs/design/knobs): lit arc, unlit arc, rim,
    /// pointer.
    pub const KNOB_ACCENT: Color = Color::Rgb(122, 162, 247);
    pub const KNOB_DIM: Color = Color::Rgb(59, 66, 97);
    pub const KNOB_RIM: Color = Color::Rgb(120, 126, 150);
    pub const KNOB_POINTER: Color = Color::Rgb(235, 235, 245);
    /// Mix A, B, C tab colours.
    pub const MIX: [Color; 3] = [
        Color::Rgb(80, 200, 120),
        Color::Rgb(110, 140, 255),
        Color::Rgb(230, 180, 70),
    ];

    /// Colours on.
    pub fn color() -> Self {
        Theme { color: true }
    }

    /// No colours.
    pub fn plain() -> Self {
        Theme { color: false }
    }

    /// From the `NO_COLOR` environment variable.
    pub fn from_env() -> Self {
        Self::detect(std::env::var_os("NO_COLOR").as_deref())
    }

    /// From a value of `NO_COLOR`: set and not empty turns colour off
    /// (<https://no-color.org>).
    pub fn detect(no_color: Option<&OsStr>) -> Self {
        Theme {
            color: no_color.is_none_or(OsStr::is_empty),
        }
    }

    pub fn is_color(self) -> bool {
        self.color
    }

    /// `c` as the foreground, or nothing without colour.
    pub fn fg(self, c: Color) -> Style {
        if self.color {
            Style::new().fg(c)
        } else {
            Style::new()
        }
    }

    /// `fg` on `bg`, or nothing without colour.
    pub fn fg_bg(self, fg: Color, bg: Color) -> Style {
        if self.color {
            Style::new().fg(fg).bg(bg)
        } else {
            Style::new()
        }
    }

    /// Plain text.
    pub fn text(self) -> Style {
        self.fg(Self::TEXT)
    }

    /// Headings and values: bold.
    pub fn heading(self) -> Style {
        self.fg(Self::TEXT).add_modifier(Modifier::BOLD)
    }

    /// Hints, units and inactive states.
    pub fn dim(self) -> Style {
        if self.color {
            self.fg(Self::DIM)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }

    /// Panel borders.
    pub fn border(self) -> Style {
        self.fg(Self::BORDER)
    }

    /// Inner borders, empty tracks and greyed-out controls.
    pub fn track(self) -> Style {
        if self.color {
            self.fg(Self::TRACK)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }

    /// Meter scale labels.
    pub fn scale(self) -> Style {
        if self.color {
            self.fg(Self::SCALE)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }

    /// An active state, and shortcut keys.
    pub fn active(self) -> Style {
        self.fg(Self::ACCENT)
    }

    /// The focused control: accent and bold.
    pub fn focus(self) -> Style {
        self.fg(Self::ACCENT).add_modifier(Modifier::BOLD)
    }

    /// Red: muted, clipping.
    pub fn warn(self) -> Style {
        self.fg(Self::WARN)
    }

    /// A modal's border: the accent, so it reads as on top.
    pub fn modal_border(self) -> Style {
        self.fg(Self::ACCENT)
    }

    /// A meter cell `t` of the way up (0 = bottom, 1 = top).
    pub fn meter(self, t: f64) -> Style {
        self.fg(gradient(t))
    }

    /// The input peak readout at `db`: red near 0, else the gradient.
    pub fn peak(self, db: f64) -> Style {
        let c = if db >= crate::tui::widgets::meter::PEAK_HOT_DB {
            Self::WARN
        } else {
            gradient(((db + 60.0) / 60.0).clamp(0.0, 1.0) * 0.6)
        };
        self.fg(c).add_modifier(Modifier::BOLD)
    }
}

/// Green → amber → red, btop style.
pub fn gradient(t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (a, b, u) = if t < 0.6 {
        ((80.0, 200.0, 120.0), (220.0, 200.0, 80.0), t / 0.6)
    } else {
        ((220.0, 200.0, 80.0), (236.0, 96.0, 84.0), (t - 0.6) / 0.4)
    };
    let l = |x: f64, y: f64| (x + (y - x) * u) as u8;
    Color::Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}
