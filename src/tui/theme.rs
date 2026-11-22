//! The TUI's palette: the terminal's own background, named ANSI colours only,
//! one accent, dim borders. With `NO_COLOR` set no colour is used at all;
//! bold and dim still mark focus and secondary text.

use std::ffi::OsStr;

use ratatui::style::{Color, Modifier, Style};

/// Which meter zone a cell is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Normal,
    Hot,
    Clip,
}

/// The styles every widget draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    color: bool,
}

impl Theme {
    /// The one accent colour: focus and active states.
    pub const ACCENT: Color = Color::Cyan;
    /// Borders and secondary text.
    pub const DIM: Color = Color::DarkGray;

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

    fn fg(self, c: Color) -> Style {
        if self.color {
            Style::new().fg(c)
        } else {
            Style::new()
        }
    }

    /// Plain text.
    pub fn text(self) -> Style {
        Style::new()
    }

    /// Headings: bold, no colour.
    pub fn heading(self) -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }

    /// Borders, hints, units and inactive states.
    pub fn dim(self) -> Style {
        if self.color {
            self.fg(Self::DIM)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }

    /// An active state (a switch that is on).
    pub fn active(self) -> Style {
        self.fg(Self::ACCENT)
    }

    /// The focused control: accent and bold.
    pub fn focus(self) -> Style {
        self.fg(Self::ACCENT).add_modifier(Modifier::BOLD)
    }

    /// A modal's border: the accent, so it reads as on top.
    pub fn modal_border(self) -> Style {
        self.fg(Self::ACCENT)
    }

    /// A meter cell: green, amber near the top, red only at clip.
    pub fn meter(self, zone: Zone) -> Style {
        self.fg(match zone {
            Zone::Normal => Color::Green,
            Zone::Hot => Color::Yellow,
            Zone::Clip => Color::Red,
        })
    }
}
