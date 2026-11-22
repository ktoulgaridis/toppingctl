//! Pill toggles: ` MON `, ` 48V `, … lit when on. 48V and MUTE light red,
//! everything else the accent. A disabled pill (IN 1+2 LINK while off) is
//! greyed out with no pill background. Focus is bold and underlined.

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::tui::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PillKind {
    /// Lights the accent.
    Normal,
    /// Lights red (48V, MUTE).
    Danger,
    /// Greyed out while off.
    Disabled,
}

/// A pill: `label` padded by one space each side.
pub fn pill(label: &str, on: bool, kind: PillKind, focused: bool, t: Theme) -> Span<'static> {
    let mut style = match (on, kind) {
        (true, PillKind::Danger) => t
            .fg_bg(Theme::PILL_ON_FG, Theme::WARN)
            .add_modifier(Modifier::BOLD),
        (true, _) => t
            .fg_bg(Theme::PILL_ON_FG, Theme::ACCENT)
            .add_modifier(Modifier::BOLD),
        (false, PillKind::Disabled) => t.track(),
        (false, _) => t.fg_bg(Theme::DIM, Theme::PILL_OFF_BG),
    };
    if on && !t.is_color() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    if focused {
        style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        if !on && t.is_color() {
            style = style.fg(Theme::ACCENT);
        }
    }
    Span::styled(format!(" {label} "), Style::default().patch(style))
}
