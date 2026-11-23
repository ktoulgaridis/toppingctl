//! Buttons: ` MON `, ` 48V `, … on a square background, lit when on, the
//! label always bold (#41). 48V and MUTE light red, everything else the
//! accent. A disabled button (IN 1+2 LINK while off) is a button like the
//! others with its text greyed out. Focus is underlined.

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
    let w = label.chars().count() + 2;
    button(label, w as u16, on, kind, focused, t)
}

/// A button `width` cells wide, its label `(width − len) / 2` cells in
/// (the mockup's rule: the pixel face centres on the label, so an odd
/// remainder still looks centred). Narrower than its label: the label.
pub fn button(
    label: &str,
    width: u16,
    on: bool,
    kind: PillKind,
    focused: bool,
    t: Theme,
) -> Span<'static> {
    let len = label.chars().count();
    let w = usize::from(width).max(len);
    let lead = (w - len) / 2;
    let text = format!("{}{label}{}", " ".repeat(lead), " ".repeat(w - len - lead));
    let mut style = match (on, kind) {
        (true, PillKind::Danger) => t
            .fg_bg(Theme::PILL_ON_FG, Theme::WARN)
            .add_modifier(Modifier::BOLD),
        (true, _) => t
            .fg_bg(Theme::PILL_ON_FG, Theme::ACCENT)
            .add_modifier(Modifier::BOLD),
        (false, PillKind::Disabled) => t.fg_bg(Theme::SCALE, Theme::PILL_OFF_BG),
        (false, _) => t.fg_bg(Theme::DIM, Theme::PILL_OFF_BG),
    };
    if on && !t.is_color() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    if focused {
        style = style.add_modifier(Modifier::UNDERLINED);
        if !on && t.is_color() {
            style = style.fg(Theme::ACCENT);
        }
    }
    let style = style.add_modifier(Modifier::BOLD);
    Span::styled(text, Style::default().patch(style))
}
