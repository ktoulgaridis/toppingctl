//! Rendering: a pure function of [`App`] and [`Meters`] that also records
//! where every clickable thing landed (the [`HitMap`]).
//!
//! The screen follows the design reference (docs/design, #34): a header
//! with the wordmark, then four panels placed like the official app's —
//! input and mixer on top, loopback and output below — and a line of key
//! hints. Everything is drawn from the [`State`](crate::state::State).
//! The input panel, the mix tabs and the mixer are clickable; the loopback
//! and output controls come with #15. Past [`MAX_WIDTH`]×[`MAX_HEIGHT`] the
//! console stops growing and is centred, and inside it faders, meters and
//! knobs are capped, each strip's content kept as one centred group (#14).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};
use ratatui::Frame;

use crate::rate::{format_khz, RATES};
use crate::state::{Meters, Mix, PbStrip, Source};
use crate::tui::app::{
    App, Control, InputControl, MeterId, MixControl, Modal, Panel, Phase, MAX_GAIN_DB,
    MIX_FADER_MAX_DB,
};
use crate::tui::pixel::{chip_rgb, clear_under, PixelLayer, PixelSpot, PixelWidget, GAIN_TICKS};
use crate::tui::theme::Theme;
use crate::tui::widgets::dial::{knob_rect, Dial};
use crate::tui::widgets::fader::{fmt_db, Fader};
use crate::tui::widgets::meter::{peak_text, Meter, Scale, INPUT_SCALE_DB};
use crate::tui::widgets::pill::{pill, PillKind};

/// The smallest terminal the full screen is drawn in.
pub const MIN_WIDTH: u16 = 100;
pub const MIN_HEIGHT: u16 = 34;
/// The largest the console grows: beyond this the terminal's extra space
/// is left around it (it is centred), so a large terminal keeps the app's
/// proportions instead of stretching strips, faders and meters (#14).
pub const MAX_WIDTH: u16 = 140;
pub const MAX_HEIGHT: u16 = 55;
/// The tallest fader track, in rows (+6 … −∞ in five even steps of 3).
pub const FADER_MAX_ROWS: u16 = 16;
/// The tallest meter bar, in rows (the input meter: 0 … −60 in six steps
/// of 3, under its clip light).
pub const METER_MAX_ROWS: u16 = 19;
/// From this header width up, the header shows its long labels.
const WIDE_HEADER: u16 = 128;

/// Something clickable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// A control: a click acts on it (and focuses it).
    Control(Control),
    /// A strip's area: a click only moves focus there.
    Focus(Control),
    /// The tab of mix A (0), B (1) or C (2).
    MixTab(usize),
    /// A PHONE level pill: +17 dBu (`true`) or 0 dBu.
    Phone(bool),
    /// The header's sample rate: opens the list.
    Rate,
    /// Rate `i` in the sample-rate list.
    RateOption(usize),
    /// A modal's confirm button.
    Yes,
    /// A modal's cancel (or close) button.
    No,
}

/// Where clickable things are on screen, from the last render. Later
/// entries are on top.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HitMap {
    entries: Vec<(Hit, Rect)>,
    /// The pixel widgets the last render placed (see [`draw_with`]).
    pixels: Vec<PixelSpot>,
}

impl HitMap {
    pub fn push(&mut self, hit: Hit, rect: Rect) {
        if !rect.is_empty() {
            self.entries.push((hit, rect));
        }
    }

    /// The topmost thing at column `x`, row `y`.
    pub fn get(&self, x: u16, y: u16) -> Option<Hit> {
        let pos = ratatui::layout::Position { x, y };
        self.entries
            .iter()
            .rev()
            .find(|(_, r)| r.contains(pos))
            .map(|&(hit, _)| hit)
    }

    /// Where `hit` is.
    pub fn rect(&self, hit: Hit) -> Option<Rect> {
        self.entries
            .iter()
            .rev()
            .find(|(h, _)| *h == hit)
            .map(|&(_, r)| r)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The pixel widgets of the last render, in drawing order.
    pub fn pixels(&self) -> &[PixelSpot] {
        &self.pixels
    }

    /// Records `widget` over `rect` for the pixel layer.
    fn pixel(&mut self, rect: Rect, widget: PixelWidget) {
        if !rect.is_empty() {
            self.pixels.push(PixelSpot { rect, widget });
        }
    }

    /// Records a chip under text drawn in `style` over `rect`, when the
    /// style has a background (a pill, a lit tab, a strip title).
    fn chip(&mut self, rect: Rect, style: Style) {
        if let Some(rgb) = style.bg.and_then(chip_rgb) {
            self.pixel(rect, PixelWidget::Chip { rgb });
        }
    }
}

/// Draws the whole screen in text and returns its hit map (the golden
/// snapshots use this).
pub fn draw(f: &mut Frame, app: &App, meters: &Meters) -> HitMap {
    draw_with(f, app, meters, None)
}

/// [`draw`], then, with a pixel layer, the pixel widgets: the layer queues
/// the kitty commands for what changed (the caller writes them after the
/// frame, see [`PixelLayer::take_output`]) and the text form under each
/// placed widget is blanked, labels kept. Not while a modal is open: every
/// placement goes, so the modal's text stays on top.
pub fn draw_with(
    f: &mut Frame,
    app: &App,
    meters: &Meters,
    pixels: Option<&mut PixelLayer>,
) -> HitMap {
    let hits = draw_text(f, app, meters);
    if let Some(layer) = pixels {
        let spots = if app.modal().is_none() {
            hits.pixels()
        } else {
            &[]
        };
        layer.update(spots, f.area());
        clear_under(layer, spots, f.buffer_mut());
    }
    hits
}

fn draw_text(f: &mut Frame, app: &App, meters: &Meters) -> HitMap {
    let area = f.area();
    let buf = f.buffer_mut();
    let mut hits = HitMap::default();
    let t = app.theme();
    match (app.phase(), app.modal()) {
        (Phase::Connecting, _) => splash(buf, area, t, Some("Connecting to the TOPPING E2x2…")),
        (Phase::Synced, Some(Modal::PushDefaults)) => splash(buf, area, t, None),
        _ if area.width < MIN_WIDTH || area.height < MIN_HEIGHT => too_small(buf, area, t),
        _ => console(buf, area, app, meters, &mut hits),
    }
    match app.modal() {
        Some(Modal::PushDefaults) => confirm(
            buf,
            area,
            t,
            " First run ",
            &[
                "No saved state.",
                "Push defaults to the device?",
                "",
                "No quits without writing anything.",
            ],
            ("Push", "Quit"),
            &mut hits,
        ),
        Some(Modal::Phantom { ch }) => confirm(
            buf,
            area,
            t,
            " 48V phantom power ",
            &[
                &format!("Turn on 48V phantom power on IN{}?", ch + 1),
                "Unplug ribbon mics.",
            ],
            ("Turn on", "Cancel"),
            &mut hits,
        ),
        Some(Modal::Help) => help(buf, area, t, &mut hits),
        Some(Modal::Rate { sel }) => rate_list(buf, area, t, app.sample_rate(), sel, &mut hits),
        None => {}
    }
    hits
}

/// Writes `s` at `x`, `y`, cut at the end of `area`.
fn put(buf: &mut Buffer, area: Rect, x: u16, y: u16, s: &str, style: Style) -> Rect {
    if y < area.top() || y >= area.bottom() || x >= area.right() || x < area.left() {
        return Rect::default();
    }
    let max = usize::from(area.right() - x);
    let (end, _) = buf.set_stringn(x, y, s, max, style);
    Rect::new(x, y, end - x, 1)
}

/// Writes `spans` centred on row `y` of `area`; returns each span's rect.
/// A span with a background (a pill) gets a chip under it on the pixel
/// layer.
fn put_centered(
    buf: &mut Buffer,
    area: Rect,
    y: u16,
    spans: &[Span],
    hits: &mut HitMap,
) -> Vec<Rect> {
    let w: u16 = spans.iter().map(|s| s.width() as u16).sum();
    let mut x = area.x + area.width.saturating_sub(w) / 2;
    spans
        .iter()
        .map(|s| {
            let r = put(buf, area, x, y, &s.content, s.style);
            hits.chip(r, s.style);
            x += s.width() as u16;
            r
        })
        .collect()
}

/// `area` shrunk to `w`×`h`, centred.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn row(area: Rect, dy: u16) -> Rect {
    Rect::new(area.x, area.y + dy, area.width, 1)
}

fn splash(buf: &mut Buffer, area: Rect, t: Theme, status: Option<&str>) {
    let mut lines = vec![Line::styled("toppingctl", t.focus())];
    if let Some(status) = status {
        lines.push(Line::raw(""));
        lines.push(Line::styled(status, t.dim()));
    }
    let h = lines.len() as u16;
    Paragraph::new(lines)
        .centered()
        .render(centered(area, area.width, h), buf);
}

fn too_small(buf: &mut Buffer, area: Rect, t: Theme) {
    let lines = vec![
        Line::styled("Terminal too small", t.heading()),
        Line::styled(
            format!(
                "need {MIN_WIDTH}×{MIN_HEIGHT}, have {}×{}",
                area.width, area.height
            ),
            t.dim(),
        ),
    ];
    Paragraph::new(lines)
        .centered()
        .render(centered(area, area.width, 2), buf);
}

/// The full screen: header, the four panels, key hints; at most
/// [`MAX_WIDTH`]×[`MAX_HEIGHT`], centred.
fn console(buf: &mut Buffer, area: Rect, app: &App, meters: &Meters, hits: &mut HitMap) {
    let t = app.theme();
    let area = centered(area, MAX_WIDTH, MAX_HEIGHT);
    let header = Rect { height: 4, ..area };
    let footer = row(area, area.height - 1);
    let body_h = area.height - 5;
    // The mixer needs the height; loopback and output fit in less.
    let top_h = body_h * 3 / 5;
    let left_w = (u32::from(area.width) * 36 / 100) as u16;
    let top = Rect::new(area.x, area.y + 4, area.width, top_h);
    let bottom = Rect::new(area.x, top.bottom(), area.width, body_h - top_h);
    let split = |r: Rect| {
        (
            Rect { width: left_w, ..r },
            Rect {
                x: r.x + left_w,
                width: r.width - left_w,
                ..r
            },
        )
    };
    let (input, mixer) = split(top);
    let (loopback, output) = split(bottom);

    draw_header(buf, header, app, t, hits);
    draw_input_panel(buf, input, app, meters, hits);
    draw_mixer_panel(buf, mixer, app, meters, hits);
    draw_loopback_panel(buf, loopback, app, meters, hits);
    draw_output_panel(buf, output, app, meters, hits);
    draw_footer(buf, footer, app, t);
}

/// Two-row block wordmark, one glyph pair per letter.
const WORDMARK: [(char, &str, &str); 8] = [
    ('t', "▄█▄", " █▄"),
    ('o', "▄▄▄", "█▄█"),
    ('p', "▄▄▄", "█▀▀"),
    ('i', "▄", "█"),
    ('n', "▄▄▄", "█ █"),
    ('g', "▄▄▄", "▀▀█"),
    ('c', "▄▄▄", "█▄▄"),
    ('l', "█ ", "█▄"),
];

fn wordmark(t: Theme) -> [Line<'static>; 2] {
    let word = "toppingctl";
    let n = word.len() as f64;
    let mut top = vec![Span::raw(" ")];
    let mut bot = vec![Span::raw(" ")];
    for (i, ch) in word.chars().enumerate() {
        let Some((_, a, b)) = WORDMARK.iter().find(|g| g.0 == ch) else {
            continue;
        };
        let u = i as f64 / (n - 1.0);
        let c = ratatui::style::Color::Rgb(
            (122.0 + (186.0 - 122.0) * u) as u8,
            (196.0 + (140.0 - 196.0) * u) as u8,
            255,
        );
        let st = t.fg(c).add_modifier(Modifier::BOLD);
        top.push(Span::styled(format!("{a} "), st));
        bot.push(Span::styled(format!("{b} "), st));
    }
    [Line::from(top), Line::from(bot)]
}

fn draw_header(buf: &mut Buffer, area: Rect, app: &App, t: Theme, hits: &mut HitMap) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.border());
    let inner = block.inner(area);
    block.render(area, buf);
    let [top, bot] = wordmark(t);
    let wm_w = top.width() as u16;
    let wide = inner.width >= WIDE_HEADER;
    top.render(row(inner, 0), buf);
    bot.render(row(inner, 1), buf);

    let dev_x = inner.x + wm_w + 1;
    let fw = match app.state().readout.firmware_version {
        Some(v) => format!("fw {}.{:02}", v.major, v.minor),
        None => "fw —".to_string(),
    };
    put(buf, inner, dev_x, inner.y, "E2x2", t.heading());
    put(buf, inner, dev_x, inner.y + 1, &fw, t.dim());

    let sep = || Span::styled("  │  ", t.border());
    let key = |k: &'static str| Span::styled(k, t.focus());
    let lab = |s: &'static str| Span::styled(s, t.dim());
    let val = |s: &'static str| Span::styled(s, t.heading());
    // No workspaces yet (#17): a placeholder. The sample rate is read
    // from the OS (crate::rate); a click on it opens the list.
    let rate = Span::styled(format!("{} ▾", format_khz(app.sample_rate())), t.heading());
    let mut l1 = if wide {
        vec![
            lab("workspace  "),
            val("default"),
            lab(" ▾"),
            sep(),
            lab("sample rate  "),
        ]
    } else {
        vec![val("default"), lab(" ▾"), sep()]
    };
    let rate_at = l1.len();
    l1.extend([rate, Span::raw(" ")]);
    let l1_w: u16 = l1.iter().map(|s| s.width() as u16).sum();
    let rate_x = inner.right().saturating_sub(l1_w)
        + l1[..rate_at].iter().map(|s| s.width() as u16).sum::<u16>();
    hits.push(
        Hit::Rate,
        Rect::new(rate_x, inner.y, l1[rate_at].width() as u16, 1),
    );
    let (dot, status) = match app.phase() {
        Phase::Synced if app.rx_stale() => (t.warn(), " no meters "),
        Phase::Synced => (t.fg(Theme::OK), " synced "),
        Phase::Connecting => (t.dim(), " connecting "),
    };
    let mut l2 = if wide {
        vec![
            key("s"),
            lab(" save workspace"),
            sep(),
            key("l"),
            lab(" load"),
            sep(),
            key("w"),
            lab(" store to device"),
        ]
    } else {
        vec![
            key("s"),
            lab(" save"),
            sep(),
            key("l"),
            lab(" load"),
            sep(),
            key("w"),
            lab(" store"),
        ]
    };
    l2.extend([sep(), Span::styled("●", dot), lab(status)]);
    Line::from(l1).right_aligned().render(row(inner, 0), buf);
    Line::from(l2).right_aligned().render(row(inner, 1), buf);
}

fn draw_footer(buf: &mut Buffer, area: Rect, app: &App, t: Theme) {
    if let Some((strip, _)) = app.editing() {
        // Renaming takes the keyboard: say so, and how to leave.
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(
                format!(" rename {} ", MIX_STRIPS[strip]),
                t.fg_bg(Theme::PILL_ON_FG, Theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
        ];
        for (k, what) in [("type", "the name"), ("enter", "keep"), ("esc", "cancel")] {
            spans.push(Span::styled(k, t.focus()));
            spans.push(Span::styled(format!(" {what}   "), t.dim()));
        }
        Line::from(spans).render(area, buf);
        return;
    }
    if let Some(notice) = app.notice() {
        Line::from(vec![Span::raw(" "), Span::styled(notice, t.heading())]).render(area, buf);
        return;
    }
    if let Control::Mixer(_) = app.focus() {
        // On a strip's name nothing adjusts: point at the controls.
        let mut spans = vec![Span::raw(" ")];
        for (k, what) in [
            ("↓", "into the strip's controls"),
            ("←→", "strip"),
            ("enter", "rename"),
            ("tab 1-4", "panel"),
            ("a b c", "mix"),
            ("?", "help"),
            ("q", "quit"),
        ] {
            spans.push(Span::styled(k, t.focus()));
            spans.push(Span::styled(format!(" {what}   "), t.dim()));
        }
        Line::from(spans).render(area, buf);
        return;
    }
    let hints = [
        ("tab 1-4", "panel"),
        ("↑↓←→", "move"),
        ("←→", "adjust ⇧×10"),
        ("space", "toggle"),
        ("0", "reset"),
        ("a b c", "mix"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (k, what) in hints {
        spans.push(Span::styled(k, t.focus()));
        spans.push(Span::styled(format!(" {what}   "), t.dim()));
    }
    Line::from(spans).render(area, buf);
}

/// A panel: a rounded border with `key` and `name` set into it. Returns
/// the inner area and the column after the title.
fn panel(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    which: Panel,
    key: &str,
    name: &str,
) -> (Rect, u16) {
    let t = app.theme();
    let focused = app.focus().panel() == which;
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(if focused { t.active() } else { t.border() });
    let inner = block.inner(area);
    block.render(area, buf);
    let mut x = area.x + 1;
    for (s, st) in [
        ("┤", t.border()),
        (key, t.focus()),
        (name, if focused { t.focus() } else { t.heading() }),
        ("├", t.border()),
    ] {
        x = put(buf, area, x, area.y, s, st).right().max(x);
    }
    (inner, x)
}

/// A strip: an inner rounded box with its name set into the top border.
fn strip_box(
    buf: &mut Buffer,
    area: Rect,
    t: Theme,
    name: &str,
    focused: bool,
    hits: &mut HitMap,
) -> Rect {
    let title = if focused {
        t.fg_bg(Theme::PILL_ON_FG, Theme::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        t.fg_bg(Theme::TEXT, Theme::PILL_OFF_BG)
            .add_modifier(Modifier::BOLD)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(if focused { t.active() } else { t.track() })
        .title(Line::from(Span::styled(format!(" {name} "), title)).centered());
    let inner = block.inner(area);
    block.render(area, buf);
    // The title's cells (the block centres it): a chip under them.
    if let Some(bg) = title.bg {
        let cells: Vec<u16> = (area.left()..area.right())
            .filter(|&x| buf[(x, area.y)].bg == bg)
            .collect();
        if let (Some(&x0), Some(&x1)) = (cells.first(), cells.last()) {
            hits.chip(Rect::new(x0, area.y, x1 + 1 - x0, 1), title);
        }
    }
    inner
}

/// `area` split into `n` columns, the remainder going to the last ones.
fn columns(area: Rect, n: u16) -> Vec<Rect> {
    let base = area.width / n;
    let extra = area.width % n;
    let mut x = area.x;
    (0..n)
        .map(|i| {
            let w = base + u16::from(i >= n - extra);
            let r = Rect {
                x,
                width: w,
                ..area
            };
            x += w;
            r
        })
        .collect()
}

/// The largest input gain and Monitor Mix knob, in cells (about 12×6).
const GAIN_KNOB: (u16, u16) = (12, 6);
/// An input strip's switches and gain knob, in rows: the peak readout and
/// six switches, the dial and its value.
const SWITCHES_AND_KNOB: u16 = 7 + GAIN_KNOB.1 + 1;
/// The largest pan knob, in cells (about 8×4).
const PAN_KNOB: (u16, u16) = (8, 4);

/// A knob to draw: the Braille knob as text, and its cells recorded for
/// the pixel layer.
struct Knob {
    value: Option<f64>,
    label: Option<String>,
    focused: bool,
    dimmed: bool,
    max: (u16, u16),
    ticks: u32,
    /// A pan (or balance): lit from the top.
    bipolar: bool,
}

impl Knob {
    fn draw(self, buf: &mut Buffer, area: Rect, t: Theme, hits: &mut HitMap) {
        let rect = knob_rect(area, self.label.is_some(), self.max);
        hits.pixels.push(PixelSpot {
            rect,
            widget: PixelWidget::Knob {
                value: self.value,
                ticks: self.ticks,
                dimmed: self.dimmed,
                bipolar: self.bipolar,
            },
        });
        Dial::new(t)
            .value(self.value)
            .dimmed(self.dimmed)
            .bipolar(self.bipolar)
            .focused(self.focused)
            .label(self.label)
            .max_cells(self.max.0, self.max.1)
            .render(area, buf);
    }
}

/// A level meter as text, its bar and clip light recorded for the pixel
/// layer (the clip light on the top row, as the text meter draws it).
fn meter(
    buf: &mut Buffer,
    area: Rect,
    db: f64,
    width: u16,
    clip: Option<bool>,
    t: Theme,
    hits: &mut HitMap,
) {
    Meter::new(db, t).width(width).clip(clip).render(area, buf);
    let mut bar = Rect {
        width: width.min(area.width),
        ..area
    };
    if bar.is_empty() {
        return;
    }
    if let Some(lit) = clip {
        hits.pixel(Rect { height: 1, ..bar }, PixelWidget::ClipLight { lit });
        bar.y += 1;
        bar.height -= 1;
    }
    hits.pixel(bar, PixelWidget::Meter { db });
}

/// A fader as text, recorded for the pixel layer over the cells the text
/// fader takes.
#[allow(clippy::too_many_arguments)]
fn fader(
    buf: &mut Buffer,
    area: Rect,
    db: f64,
    max_db: f64,
    labels: bool,
    focused: bool,
    t: Theme,
    hits: &mut HitMap,
) {
    Fader::new(db, max_db, t)
        .labels(labels)
        .focused(focused)
        .render(area, buf);
    hits.pixel(
        Fader::rect(labels, area),
        PixelWidget::Fader { db, max_db, labels },
    );
}

/// A gain value: `+12 dB`; 0 dB reads `−0 dB`, as in the official app.
pub fn gain_text(gain_db: u8) -> String {
    match gain_db.min(MAX_GAIN_DB) {
        0 => "−0 dB".into(),
        g => format!("+{g} dB"),
    }
}

/// A pan value: `C`, `L40`, `R100`.
pub fn pan_text(pan: f64) -> String {
    let p = ((pan.clamp(0.0, 1.0) - 0.5) * 200.0).round() as i32;
    match p {
        0 => "C".into(),
        p if p < 0 => format!("L{}", -p),
        p => format!("R{p}"),
    }
}

fn source_name(s: Source) -> &'static str {
    match s {
        Source::MixA => "mix A",
        Source::MixB => "mix B",
        Source::MixC => "mix C",
        Source::Pb12 => "PB 1/2",
        Source::Pb34 => "PB 3/4",
        Source::Pb56 => "PB 5/6",
        Source::In1 => "IN1",
        Source::In2 => "IN2",
        Source::In12 => "IN 1+2",
    }
}

// ------------------------------------------------------------------ input

fn draw_input_panel(buf: &mut Buffer, area: Rect, app: &App, meters: &Meters, hits: &mut HitMap) {
    let (inner, _) = panel(buf, area, app, Panel::Input, "¹", "input");
    for (ch, col) in columns(inner, 2).into_iter().enumerate() {
        draw_input_strip(buf, col, app, meters, ch, hits);
    }
}

fn draw_input_strip(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    meters: &Meters,
    ch: usize,
    hits: &mut HitMap,
) {
    let t = app.theme();
    let input = &app.state().inputs[ch];
    let focus = app.focus();
    let focused = |kind| focus == Control::Input { ch, kind };
    let in_strip = matches!(focus, Control::Input { ch: c, .. } if c == ch);
    hits.push(
        Hit::Focus(Control::Input {
            ch,
            kind: InputControl::ORDER[0],
        }),
        area,
    );
    let r = strip_box(buf, area, t, &format!("IN{}", ch + 1), in_strip, hits);
    if r.height < 2 || r.width < 8 {
        return;
    }
    // One group, centred: the meter (at most METER_MAX_ROWS under its clip
    // light) beside the switches with the gain knob right under them.
    let meter_h = r.height.min(METER_MAX_ROWS + 1);
    let r = centered_rows(r, meter_h.max(SWITCHES_AND_KNOB.min(r.height)));
    let db = meters.inputs[ch];
    let x0 = r.x + 1;
    let lit = app.clips().lit(MeterId::Input(ch));
    meter(
        buf,
        Rect::new(x0, r.y, 2, meter_h),
        db,
        2,
        Some(lit),
        t,
        hits,
    );
    Scale::new(&INPUT_SCALE_DB, t).render(Rect::new(x0 + 2, r.y + 1, 3, meter_h - 1), buf);
    put(buf, r, x0 + 3, r.y, &peak_text(db), t.peak(db));

    // The switches, top to bottom, then the gain knob at the bottom.
    let right = Rect {
        x: x0 + 6,
        width: r.right().saturating_sub(x0 + 6),
        ..r
    };
    let switches = [
        (InputControl::Mon, "MON", input.mon, PillKind::Normal),
        (
            InputControl::Phantom,
            "48V",
            input.phantom == Some(true),
            PillKind::Danger,
        ),
        (InputControl::Inst, "INST", input.inst, PillKind::Normal),
        (InputControl::Solo, "SOLO", input.solo, PillKind::Normal),
        (InputControl::Mute, "MUTE", input.mute, PillKind::Danger),
        (InputControl::Phase, "Ø", input.phase, PillKind::Normal),
    ];
    for (i, (kind, label, on, pk)) in switches.into_iter().enumerate() {
        let y = r.y + 1 + i as u16;
        let span = pill(&format!("{label:^4}"), on, pk, focused(kind), t);
        let rects = put_centered(buf, right, y, &[span], hits);
        hits.push(Hit::Control(Control::Input { ch, kind }), rects[0]);
    }
    // The gain knob right under the switches: dial, then value.
    let knob_y = r.y + 7;
    if r.bottom() >= knob_y + 4 {
        // From just right of the scale to the edge: as wide as the strip
        // allows (about 12×6 cells at 120 columns).
        let kx = x0 + 5;
        let knob = Rect::new(
            kx,
            knob_y,
            r.right().saturating_sub(kx),
            (r.bottom() - knob_y).min(GAIN_KNOB.1 + 1),
        );
        let gain = input.gain_db.min(MAX_GAIN_DB);
        let k = Knob {
            value: Some(f64::from(gain) / f64::from(MAX_GAIN_DB)),
            label: Some(gain_text(gain)),
            focused: focused(InputControl::Gain),
            dimmed: false,
            max: GAIN_KNOB,
            ticks: GAIN_TICKS,
            bipolar: false,
        };
        k.draw(buf, knob, t, hits);
        hits.push(
            Hit::Control(Control::Input {
                ch,
                kind: InputControl::Gain,
            }),
            knob,
        );
    }
}

// ------------------------------------------------------------------ mixer

fn draw_mixer_panel(buf: &mut Buffer, area: Rect, app: &App, meters: &Meters, hits: &mut HitMap) {
    let t = app.theme();
    let (inner, mut x) = panel(buf, area, app, Panel::Mixer, "²", "mixer");
    // The mix tabs, set into the border; the active one is boxed.
    x = put(buf, area, x, area.y, "─", t.border()).right().max(x);
    for (m, name) in ["mix A", "mix B", "mix C"].into_iter().enumerate() {
        let colour = Theme::MIX[m];
        let start = x;
        let active = app.mix() == m;
        let key = ((b'a' + m as u8) as char).to_string();
        // The active tab: key and name on one chip of the mix's colour.
        // The others: key (bold) and name in that colour, as in the mockup.
        let parts: Vec<(String, Style)> = if active {
            vec![
                ("┤".into(), t.border()),
                (
                    format!("{key} {name}"),
                    t.fg_bg(Theme::PILL_ON_FG, colour)
                        .add_modifier(Modifier::BOLD),
                ),
                ("├".into(), t.border()),
            ]
        } else {
            vec![
                (" ".into(), t.border()),
                (key, t.fg(colour).add_modifier(Modifier::BOLD)),
                (format!(" {name} "), t.fg(colour)),
                (" ".into(), t.border()),
            ]
        };
        for (s, st) in &parts {
            let r = put(buf, area, x, area.y, s, *st);
            hits.chip(r, *st);
            x = r.right().max(x);
        }
        // The click target starts at the key letter.
        let key_x = start + 1;
        hits.push(
            Hit::MixTab(m),
            Rect::new(key_x, area.y, x.saturating_sub(key_x + 1), 1),
        );
    }
    let mix = &app.state().mixes[app.mix()];
    let cols = columns(inner, 4);
    draw_in12_strip(buf, cols[0], app, meters, mix, hits);
    for (i, strip) in [&mix.pb12, &mix.pb34, &mix.pb56].into_iter().enumerate() {
        draw_pb_strip(buf, cols[i + 1], app, meters, i, strip, hits);
    }
}

/// Mixer strip `strip`'s name: the user's, else the default; while it is
/// being edited, the text so far and a cursor.
fn strip_name(app: &App, strip: usize) -> String {
    if let Some((i, text)) = app.editing() {
        if i == strip {
            return format!("{text}▏");
        }
    }
    app.state().strip_names[strip]
        .clone()
        .unwrap_or_else(|| MIX_STRIPS[strip].to_string())
}

/// The mixer strips' default names.
const MIX_STRIPS: [&str; 4] = ["IN 1+2", "PB 1/2", "PB 3/4", "PB 5/6"];

/// Whether mixer strip `strip` (its name or one of its controls) has focus.
fn strip_focused(app: &App, strip: usize) -> bool {
    match app.focus() {
        Control::Mixer(i) | Control::Mix { strip: i, .. } => i == strip,
        _ => false,
    }
}

/// The dB value of a strip: `muted` in red when muted.
fn value_span(db: f64, mute: bool, t: Theme, unit: bool) -> Span<'static> {
    if mute {
        return Span::styled("muted", t.warn());
    }
    let s = fmt_db(db);
    let s = if unit {
        s
    } else {
        s.trim_end_matches(" dB").to_string()
    };
    Span::styled(s, t.heading())
}

/// Rows for a mix strip's two pan dials and their values: more when the
/// strip is tall.
fn dial_rows(r: Rect) -> u16 {
    if r.height >= 16 {
        5
    } else {
        4
    }
}

/// A mix strip's content, top to bottom: the dials (`d` rows), LINK, the
/// value, the fader (as tall as `r` allows, at most [`FADER_MAX_ROWS`])
/// and the switches (`rows`). Returns the group's area, centred in `r`,
/// and the fader's height.
fn mix_group(r: Rect, d: u16, rows: u16) -> (Rect, u16) {
    let fixed = d + 2 + rows;
    let fader_h = r.height.saturating_sub(fixed).min(FADER_MAX_ROWS);
    (centered_rows(r, fixed + fader_h), fader_h)
}

/// `r` cut to `h` rows, centred vertically.
fn centered_rows(r: Rect, h: u16) -> Rect {
    let h = h.min(r.height);
    Rect {
        y: r.y + (r.height - h) / 2,
        height: h,
        ..r
    }
}

/// Mixer strip `strip`'s two pan dials side by side with their values
/// under them, in the top `rows` rows of `area`; each is a click target.
#[allow(clippy::too_many_arguments)]
fn pan_dials(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    strip: usize,
    pans: [f64; 2],
    rows: u16,
    hits: &mut HitMap,
) {
    let halves = columns(
        Rect {
            height: rows,
            ..area
        },
        2,
    );
    for (ch, (h, p)) in halves.into_iter().zip(pans).enumerate() {
        let c = Control::Mix {
            strip,
            ctl: MixControl::Pan(ch),
        };
        let k = Knob {
            value: Some(p),
            label: Some(pan_text(p)),
            focused: app.focus() == c,
            dimmed: false,
            max: PAN_KNOB,
            ticks: 0,
            bipolar: true,
        };
        k.draw(buf, h, app.theme(), hits);
        hits.push(Hit::Control(c), h);
    }
}

/// L meter · fader · R meter, centred in `area`. Returns the fader's
/// cells.
#[allow(clippy::too_many_arguments)]
fn stereo_body(
    buf: &mut Buffer,
    area: Rect,
    t: Theme,
    db: f64,
    max_db: f64,
    levels: [f64; 2],
    clips: [bool; 2],
    focused: bool,
    hits: &mut HitMap,
) -> Rect {
    let fw = Fader::width(true, area.width.saturating_sub(4));
    let total = fw + 4;
    if area.width < total || area.height == 0 {
        return Rect::default();
    }
    let x = area.x + (area.width - total) / 2;
    let col = |x: u16, w: u16| Rect {
        x,
        width: w,
        ..area
    };
    meter(buf, col(x, 1), levels[0], 1, Some(clips[0]), t, hits);
    let f = col(x + 2, fw);
    fader(buf, f, db, max_db, true, focused, t, hits);
    meter(
        buf,
        col(x + 3 + fw, 1),
        levels[1],
        1,
        Some(clips[1]),
        t,
        hits,
    );
    Fader::rect(true, f)
}

/// The control of mixer strip `strip`.
fn mix_ctl(strip: usize, ctl: MixControl) -> Control {
    Control::Mix { strip, ctl }
}

fn draw_pb_strip(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    meters: &Meters,
    i: usize,
    s: &PbStrip,
    hits: &mut HitMap,
) {
    let t = app.theme();
    let strip = i + 1;
    let focus = app.focus();
    hits.push(Hit::Focus(Control::Mixer(strip)), area);
    let r = strip_box(
        buf,
        area,
        t,
        &strip_name(app, strip),
        strip_focused(app, strip),
        hits,
    );
    if r.height < 8 {
        return;
    }
    let d = dial_rows(r);
    let rows = switch_rows(r);
    let (g, fader_h) = mix_group(r, d, rows);
    pan_dials(buf, g, app, strip, [s.pan_l, s.pan_r], d, hits);
    let link = mix_ctl(strip, MixControl::Link);
    let rects = put_centered(
        buf,
        g,
        g.y + d,
        &[pill("LINK", s.link, PillKind::Normal, focus == link, t)],
        hits,
    );
    hits.push(Hit::Control(link), rects[0]);
    put_centered(
        buf,
        g,
        g.y + d + 1,
        &[value_span(s.fader_db, s.mute, t, true)],
        hits,
    );
    let body = Rect::new(g.x, g.y + d + 2, g.width, fader_h);
    let clips = [0, 1].map(|c| app.clips().lit(MeterId::Playback(i, c)));
    let fc = mix_ctl(strip, MixControl::Fader(0));
    let f = stereo_body(
        buf,
        body,
        t,
        s.fader_db,
        MIX_FADER_MAX_DB,
        meters.playback[i],
        clips,
        focus == fc,
        hits,
    );
    hits.push(Hit::Control(fc), f);
    strip_switches(buf, g, app, strip, 0, [s.phase, s.solo, s.mute], rows, hits);
}

/// Rows for a mix strip's Ø / SOLO / MUTE: stacked when the strip is tall
/// enough (as in the design), else one row of `Ø S M`.
fn switch_rows(r: Rect) -> u16 {
    if r.height >= 15 {
        3
    } else {
        1
    }
}

/// Channel `ch`'s Ø / SOLO / MUTE of mixer strip `strip` (`on` in that
/// order) at the bottom of `r`, in `rows` rows (3 or 1); each is a click
/// target.
#[allow(clippy::too_many_arguments)]
fn strip_switches(
    buf: &mut Buffer,
    r: Rect,
    app: &App,
    strip: usize,
    ch: usize,
    on: [bool; 3],
    rows: u16,
    hits: &mut HitMap,
) {
    let t = app.theme();
    let kinds = [PillKind::Normal, PillKind::Normal, PillKind::Danger];
    let ctls = [
        MixControl::Phase(ch),
        MixControl::Solo(ch),
        MixControl::Mute(ch),
    ];
    let focused = |k: usize| app.focus() == mix_ctl(strip, ctls[k]);
    if rows >= 3 {
        for (k, label) in ["Ø", "SOLO", "MUTE"].into_iter().enumerate() {
            let span = pill(&format!("{label:^4}"), on[k], kinds[k], focused(k), t);
            let rects = put_centered(buf, r, r.bottom() - 3 + k as u16, &[span], hits);
            hits.push(Hit::Control(mix_ctl(strip, ctls[k])), rects[0]);
        }
        return;
    }
    // One row: tight pills on a narrow IN 1+2 half, padded ones else.
    let narrow = strip == 0;
    let mut spans = Vec::new();
    for (k, label) in ["Ø", "S", "M"].into_iter().enumerate() {
        if k > 0 {
            spans.push(Span::raw(" "));
        }
        let p = pill(label, on[k], kinds[k], focused(k), t);
        spans.push(if narrow {
            Span::styled(label.to_string(), p.style)
        } else {
            p
        });
    }
    let rects = put_centered(buf, r, r.bottom() - 1, &spans, hits);
    for k in 0..3 {
        hits.push(Hit::Control(mix_ctl(strip, ctls[k])), rects[2 * k]);
    }
}

fn draw_in12_strip(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    meters: &Meters,
    mix: &Mix,
    hits: &mut HitMap,
) {
    let t = app.theme();
    let focus = app.focus();
    let (link, in1, in2) = (mix.link, &mix.in1, &mix.in2);
    hits.push(Hit::Focus(Control::Mixer(0)), area);
    let r = strip_box(
        buf,
        area,
        t,
        &strip_name(app, 0),
        strip_focused(app, 0),
        hits,
    );
    if r.height < 8 {
        return;
    }
    // Link puts IN1 hard L and IN2 hard R: show what is in effect.
    let pans = if link { [0.0, 1.0] } else { [in1.pan, in2.pan] };
    let d = dial_rows(r);
    let rows = switch_rows(r);
    let (g, fader_h) = mix_group(r, d, rows);
    pan_dials(buf, g, app, 0, pans, d, hits);
    let lc = mix_ctl(0, MixControl::Link);
    let rects = put_centered(
        buf,
        g,
        g.y + d,
        &[pill("LINK", link, PillKind::Disabled, focus == lc, t)],
        hits,
    );
    hits.push(Hit::Control(lc), rects[0]);
    let halves = columns(row(g, d + 1), 2);
    for (h, s) in halves.iter().zip([in1, in2]) {
        put_centered(
            buf,
            *h,
            h.y,
            &[value_span(s.fader_db, s.mute, t, false)],
            hits,
        );
    }

    // M1 · F1 · scale · F2 · M2: one scale between two mono faders, tick
    // marks on both sides of each.
    let body = Rect::new(g.x, g.y + d + 2, g.width, fader_h);
    let total = 13;
    if body.width >= total && body.height > 0 {
        let x = body.x + (body.width - total) / 2;
        let col = |x: u16, w: u16| Rect {
            x,
            width: w,
            ..body
        };
        for (k, (s, mx, fx)) in [(in1, x, x + 2), (in2, x + 12, x + 8)]
            .into_iter()
            .enumerate()
        {
            let lit = app.clips().lit(MeterId::Input(k));
            meter(buf, col(mx, 1), meters.inputs[k], 1, Some(lit), t, hits);
            let fc = mix_ctl(0, MixControl::Fader(k));
            fader(
                buf,
                col(fx, 3),
                s.fader_db,
                MIX_FADER_MAX_DB,
                false,
                focus == fc,
                t,
                hits,
            );
            hits.push(Hit::Control(fc), Fader::rect(false, col(fx, 3)));
        }
        for (y, label) in Fader::ticks(MIX_FADER_MAX_DB, body.height) {
            put(
                buf,
                body,
                x + 5,
                body.y + y,
                &format!("{label:>3}"),
                t.scale(),
            );
        }
    }
    for (k, (h, s)) in halves.iter().zip([in1, in2]).enumerate() {
        let col = Rect {
            y: g.y,
            height: g.height,
            ..*h
        };
        strip_switches(buf, col, app, 0, k, [s.phase, s.solo, s.mute], rows, hits);
    }
}

// --------------------------------------------------------------- loopback

fn draw_loopback_panel(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    meters: &Meters,
    hits: &mut HitMap,
) {
    let t = app.theme();
    let (inner, _) = panel(buf, area, app, Panel::Loopback, "³", "loopback");
    for (i, col) in columns(inner, 2).into_iter().enumerate() {
        let lb = &app.state().loopbacks[i];
        let me = Control::Loopback(i);
        hits.push(Hit::Focus(me), col);
        let r = strip_box(
            buf,
            col,
            t,
            ["LOOP 1+2", "LOOP 3+4"][i],
            app.focus() == me,
            hits,
        );
        if r.height < 5 {
            continue;
        }
        let r = level_group(r);
        source_line(buf, r, t, lb.source, hits);
        put_centered(
            buf,
            r,
            r.y + 1,
            &[
                pill("LINK", lb.link, PillKind::Normal, false, t),
                Span::raw(" "),
                pill("MUTE", lb.mute, PillKind::Danger, false, t),
            ],
            hits,
        );
        put_centered(
            buf,
            r,
            r.y + 2,
            &levels_spans(lb.level_l_db, lb.level_r_db, lb.mute, t),
            hits,
        );
        let body = Rect::new(r.x, r.y + 3, r.width, r.height - 3);
        let clips = [0, 1].map(|c| app.clips().lit(MeterId::Loopback(i, c)));
        stereo_body(
            buf,
            body,
            t,
            lb.level_l_db,
            0.0,
            meters.loopbacks[i],
            clips,
            false,
            hits,
        );
    }
}

/// A loopback or output strip's content, centred in `r`: three text rows
/// and a fader at most [`FADER_MAX_ROWS`] tall.
fn level_group(r: Rect) -> Rect {
    centered_rows(r, 3 + r.height.saturating_sub(3).min(FADER_MAX_ROWS))
}

fn source_line(buf: &mut Buffer, r: Rect, t: Theme, s: Source, hits: &mut HitMap) {
    put_centered(
        buf,
        r,
        r.y,
        &[
            Span::styled(source_name(s), t.active()),
            Span::styled(" ▾", t.dim()),
        ],
        hits,
    );
}

/// One value when L and R agree, else both.
fn levels_spans(l: f64, r: f64, mute: bool, t: Theme) -> Vec<Span<'static>> {
    if mute || l == r {
        return vec![value_span(l, mute, t, true)];
    }
    vec![
        Span::styled("L ", t.dim()),
        value_span(l, false, t, false),
        Span::styled("  R ", t.dim()),
        value_span(r, false, t, false),
    ]
}

// ----------------------------------------------------------------- output

fn draw_output_panel(buf: &mut Buffer, area: Rect, app: &App, meters: &Meters, hits: &mut HitMap) {
    let t = app.theme();
    let state = app.state();
    let (inner, _) = panel(buf, area, app, Panel::Output, "⁴", "output");
    let w1 = inner.width * 2 / 5;
    let w2 = (inner.width - w1) / 2;
    let out_area = Rect { width: w1, ..inner };
    let mon_area = Rect {
        x: inner.x + w1,
        width: w2,
        ..inner
    };
    let phone_area = Rect {
        x: mon_area.right(),
        width: inner.right() - mon_area.right(),
        ..inner
    };

    for (k, a) in [out_area, mon_area, phone_area].into_iter().enumerate() {
        hits.push(Hit::Focus(Control::Output(k)), a);
    }
    let focused = |k| app.focus() == Control::Output(k);
    let out = &state.output;
    let r = strip_box(buf, out_area, t, "OUT 1+2", focused(0), hits);
    if r.height >= 5 {
        let r = level_group(r);
        source_line(buf, r, t, out.source, hits);
        put_centered(
            buf,
            r,
            r.y + 1,
            &[
                pill("LINK", out.link, PillKind::Normal, false, t),
                Span::raw(" "),
                pill("HP", out.headphones, PillKind::Normal, false, t),
                Span::raw(" "),
                pill("LINE", out.line, PillKind::Normal, false, t),
            ],
            hits,
        );
        put_centered(
            buf,
            r,
            r.y + 2,
            &levels_spans(out.level_l_db, out.level_r_db, false, t),
            hits,
        );
        let body = Rect::new(r.x, r.y + 3, r.width, r.height - 3);
        let clips = [0, 1].map(|c| app.clips().lit(MeterId::Output(c)));
        stereo_body(
            buf,
            body,
            t,
            out.level_l_db,
            0.0,
            meters.output,
            clips,
            false,
            hits,
        );
    }

    // Monitor Mix: the balance the device reports when its knob turns
    // (`11 01`), shown read-only like the official app: dimmed, and no
    // click target.
    let r = strip_box(buf, mon_area, t, "MONITOR MIX", focused(1), hits);
    if r.height >= 6 {
        // The dial (at most GAIN_KNOB) and its two text rows, centred.
        let dial_h = (r.height - 3).min(GAIN_KNOB.1);
        let r = centered_rows(r, dial_h + 3);
        let balance = state.readout.monitor_mix;
        let k = Knob {
            value: balance.map(|v| f64::from(v.clamp(0, 100)) / 100.0),
            label: None,
            focused: false,
            dimmed: true,
            max: GAIN_KNOB,
            ticks: 0,
            bipolar: true,
        };
        k.draw(buf, Rect::new(r.x, r.y, r.width, dial_h), t, hits);
        put_centered(
            buf,
            r,
            r.y + dial_h,
            &[
                Span::styled("input", t.dim()),
                Span::raw("    "),
                Span::styled("playback", t.dim()),
            ],
            hits,
        );
        put_centered(
            buf,
            r,
            r.y + dial_h + 2,
            &[Span::styled(monitor_mix_text(balance), t.heading())],
            hits,
        );
    }

    // Phone out: the level switch (`11 02`). Its volume is an analog knob
    // that software never sets.
    let r = strip_box(buf, phone_area, t, "PHONE", focused(2), hits);
    if r.height >= 4 {
        let r = centered_rows(r, 2);
        put_centered(buf, r, r.y, &[Span::styled("level", t.dim())], hits);
        let rects = put_centered(
            buf,
            r,
            r.y + 1,
            &[
                pill("+17 dBu", state.phone_gain, PillKind::Normal, false, t),
                Span::raw(" "),
                pill("0 dBu", !state.phone_gain, PillKind::Normal, false, t),
            ],
            hits,
        );
        hits.push(Hit::Phone(true), rects[0]);
        hits.push(Hit::Phone(false), rects[2]);
    }
}

/// The Monitor Mix balance as the official app shows it, `v − 50`:
/// `−20` toward input, `+20` toward playback, `0` centred; `—` unknown.
pub fn monitor_mix_text(balance: Option<i32>) -> String {
    match balance.map(|v| v.clamp(0, 100) - 50) {
        None => "—".into(),
        Some(0) => "0".into(),
        Some(d) if d < 0 => format!("−{}", -d),
        Some(d) => format!("+{d}"),
    }
}

/// A centred modal box; returns its inner area.
fn modal_box(buf: &mut Buffer, area: Rect, t: Theme, title: &str, w: u16, h: u16) -> Rect {
    let r = centered(area, w, h);
    Clear.render(r, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.modal_border())
        .title(Span::styled(title, t.heading()));
    let inner = block.inner(r);
    block.render(r, buf);
    inner
}

/// A yes/no question; No is the default.
fn confirm(
    buf: &mut Buffer,
    area: Rect,
    t: Theme,
    title: &str,
    lines: &[&str],
    (yes, no): (&str, &str),
    hits: &mut HitMap,
) {
    let text_w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    let w = (text_w + 8).max(40);
    let h = lines.len() as u16 + 6;
    let inner = modal_box(buf, area, t, title, w, h);
    for (i, line) in lines.iter().enumerate() {
        put(
            buf,
            inner,
            inner.x + 3,
            inner.y + 1 + i as u16,
            line,
            t.text(),
        );
    }
    let yes = format!(" [y] {yes} ");
    let no = format!(" [N] {no} ");
    let gap = 4;
    let total = (yes.chars().count() + gap + no.chars().count()) as u16;
    let y = inner.bottom().saturating_sub(2);
    let x = inner.x + inner.width.saturating_sub(total) / 2;
    let r = put(buf, inner, x, y, &yes, t.text());
    hits.push(Hit::Yes, r);
    let r = put(
        buf,
        inner,
        r.right() + gap as u16,
        y,
        &no,
        t.focus().add_modifier(Modifier::REVERSED),
    );
    hits.push(Hit::No, r);
}

/// The sample-rate list: the E2x2's rates, the current one marked, `sel`
/// highlighted.
fn rate_list(
    buf: &mut Buffer,
    area: Rect,
    t: Theme,
    current: Option<u32>,
    sel: usize,
    hits: &mut HitMap,
) {
    let h = RATES.len() as u16 + 6;
    let inner = modal_box(buf, area, t, " Sample rate ", 30, h);
    for (i, &r) in RATES.iter().enumerate() {
        let mark = if current == Some(r) { "●" } else { " " };
        let line = format!(" {mark} {:<10}", format_khz(Some(r)));
        let style = if i == sel {
            t.focus().add_modifier(Modifier::REVERSED)
        } else {
            t.text()
        };
        let y = inner.y + 1 + i as u16;
        let r = put(buf, inner, inner.x + 6, y, &line, style);
        hits.push(Hit::RateOption(i), r);
    }
    let close = " [Esc] close ";
    let x = inner.x + inner.width.saturating_sub(close.chars().count() as u16) / 2;
    let r = put(
        buf,
        inner,
        x,
        inner.bottom().saturating_sub(2),
        close,
        t.dim(),
    );
    hits.push(Hit::No, r);
}

fn help(buf: &mut Buffer, area: Rect, t: Theme, hits: &mut HitMap) {
    let keys: &[(&str, &str)] = &[
        ("Tab  Shift-Tab", "next / previous panel"),
        ("1  2  3  4", "input, mixer, loopback, output"),
        ("↑ ↓", "move along an input, or into a mixer strip"),
        ("← →", "next strip; on a knob or fader: 1 dB / 5 %"),
        ("Shift ← →", "the same ×10"),
        ("0", "reset the knob or fader (gain −0 dB, fader 0 dB)"),
        ("space", "toggle the focused switch"),
        ("enter", "on a mixer strip's name: rename it"),
        ("a  b  c", "show mix A / B / C"),
        ("r", "sample rate list (sets it through PipeWire)"),
        ("?", "this help"),
        ("q  Ctrl-C", "quit"),
        ("", ""),
        ("click", "focus a strip; flip a switch; pick a mix"),
        ("wheel", "turn a knob, move a fader (Shift ×10)"),
        ("drag", "turn a knob, move a fader (up / down)"),
        ("double-click", "reset a knob or fader"),
        ("", ""),
        ("", "PHONE: space or a click sets the level switch"),
        ("", "loopback, output: shown, set in a later version"),
    ];
    let h = keys.len() as u16 + 6;
    let inner = modal_box(buf, area, t, " Help ", 80, h);
    for (i, (k, what)) in keys.iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        put(buf, inner, inner.x + 3, y, k, t.heading());
        put(buf, inner, inner.x + 20, y, what, t.text());
    }
    let close = " [Esc] close ";
    let x = inner.x + inner.width.saturating_sub(close.chars().count() as u16) / 2;
    let r = put(
        buf,
        inner,
        x,
        inner.bottom().saturating_sub(2),
        close,
        t.dim(),
    );
    hits.push(Hit::No, r);
}
