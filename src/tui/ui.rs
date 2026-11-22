//! Rendering: a pure function of [`App`] and [`Meters`] that also records
//! where every clickable thing landed (the [`HitMap`]).
//!
//! The screen is a console strip: INPUTS on the left, then MIX and OUT, with
//! LOOPBACK underneath. MIX, OUT and LOOPBACK are placeholders until #14 and
//! #15 fill them in.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};
use ratatui::Frame;

use crate::state::{Meters, State};
use crate::tui::app::{App, Control, InputControl, Modal, Phase, MAX_GAIN_DB};
use crate::tui::theme::Theme;

#[derive(Clone, Copy)]
enum Zone {
    Normal,
    Hot,
    Clip,
}

/// The smallest terminal the full screen is drawn in.
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

/// The quietest level a meter shows, in dBFS.
const METER_RANGE_DB: f64 = 60.0;
/// Meter cells whose top is above this are amber.
const METER_HOT_DB: f64 = -12.0;
/// At or above this a meter's top cell is red.
const METER_CLIP_DB: f64 = -0.1;
/// The meter scale's labels.
const METER_TICKS_DB: [f64; 5] = [0.0, -12.0, -24.0, -36.0, -48.0];

const INPUTS_WIDTH: u16 = 28;
const OUT_WIDTH: u16 = 16;
/// One input's column inside the INPUTS panel.
const COLUMN_WIDTH: u16 = 11;
const GAIN_BAR_WIDTH: u16 = 10;
const FOCUS_MARK: &str = "▸";
/// The width of a dB value ([`gain_text`], [`meter_text`]).
const VALUE_WIDTH: u16 = 8;

/// Something clickable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Control(Control),
    /// The gain bar of IN1 (0) or IN2 (1): click or drag to set the gain.
    GainBar(usize),
    /// A modal's confirm button.
    Yes,
    /// A modal's cancel (or close) button.
    No,
}

/// Where clickable things are on screen, from the last render. Later
/// entries are on top.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HitMap {
    entries: Vec<(Hit, Rect)>,
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
}

/// Draws the whole screen and returns its hit map.
pub fn draw(f: &mut Frame, app: &App, meters: &Meters) -> HitMap {
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
        None => {}
    }
    hits
}

/// Writes `s` at `x`, `y`, cut at the end of `area`.
fn put(buf: &mut Buffer, area: Rect, x: u16, y: u16, s: &str, style: Style) -> Rect {
    if y < area.top() || y >= area.bottom() || x >= area.right() {
        return Rect::default();
    }
    let max = usize::from(area.right() - x);
    let (end, _) = buf.set_stringn(x, y, s, max, style);
    Rect::new(x, y, end - x, 1)
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

fn panel<'a>(t: Theme, title: &'a str) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.dim())
        .title(Span::styled(format!(" {title} "), t.heading()))
}

fn splash(buf: &mut Buffer, area: Rect, t: Theme, status: Option<&str>) {
    let mut lines = vec![Line::styled("toppingctl", t.heading())];
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

/// The full screen: header, panels, key hints.
fn console(buf: &mut Buffer, area: Rect, app: &App, meters: &Meters, hits: &mut HitMap) {
    let t = app.theme();
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let [inputs, right] =
        Layout::horizontal([Constraint::Length(INPUTS_WIDTH), Constraint::Fill(1)]).areas(body);
    let [top, loopback] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(3)]).areas(right);
    let [mix, out] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(OUT_WIDTH)]).areas(top);

    draw_header(buf, header, app.state(), t);
    draw_inputs(buf, inputs, app, meters, hits);
    draw_mix(buf, mix, t);
    draw_out(buf, out, t);
    draw_loopback(buf, loopback, t);
    draw_footer(buf, footer, app, t);
}

fn draw_header(buf: &mut Buffer, area: Rect, state: &State, t: Theme) {
    let mut spans = vec![
        Span::raw(" "),
        Span::styled("toppingctl", t.heading()),
        Span::styled("   TOPPING E2x2", t.text()),
    ];
    let versions = [
        ("hw", state.readout.hardware_version),
        ("fw", state.readout.firmware_version),
    ];
    for (name, v) in versions {
        if let Some(v) = v {
            spans.push(Span::styled(format!("  {name} {v}"), t.dim()));
        }
    }
    Line::from(spans).render(area, buf);
    Line::from(vec![
        Span::styled("?", t.heading()),
        Span::styled(" help ", t.dim()),
    ])
    .right_aligned()
    .render(area, buf);
}

fn draw_footer(buf: &mut Buffer, area: Rect, app: &App, t: Theme) {
    if let Some(notice) = app.notice() {
        Line::from(vec![Span::raw(" "), Span::styled(notice, t.heading())]).render(area, buf);
        return;
    }
    let hints = [
        ("Tab", "focus"),
        ("h/l", "adjust"),
        ("H/L", "×10"),
        ("space", "toggle"),
        ("m s p", "mute solo Ø"),
        ("q", "quit"),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (i, (k, what)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(*k, t.heading()));
        spans.push(Span::styled(format!(" {what}"), t.dim()));
    }
    Line::from(spans).render(area, buf);
}

fn draw_inputs(buf: &mut Buffer, area: Rect, app: &App, meters: &Meters, hits: &mut HitMap) {
    let t = app.theme();
    let block = panel(t, "INPUTS");
    let inner = block.inner(area);
    block.render(area, buf);
    // One column of padding on each side, two between the inputs.
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    for ch in 0..2 {
        let col = Rect {
            x: inner.x + ch as u16 * (COLUMN_WIDTH + 2),
            width: COLUMN_WIDTH.min(inner.width),
            ..inner
        };
        draw_input(buf, col, app, meters, ch, hits);
    }
}

/// Fixed width, right-aligned, so the units line up with [`meter_text`]:
/// `  +12 dB`, `   +0 dB`, `   −∞ dB`.
pub fn gain_text(gain_db: u8, muted: bool) -> String {
    if muted {
        "   −∞ dB".to_string()
    } else {
        format!("{:>+5} dB", gain_db.min(MAX_GAIN_DB))
    }
}

/// Fixed width, right-aligned: `−18.4 dB`, ` −4.0 dB`, `  0.0 dB`.
pub fn meter_text(db: f64) -> String {
    // `+ 0.0` turns −0.0 into 0.0.
    format!("{:>5.1} dB", db.clamp(-99.9, 0.0) + 0.0).replace('-', "−")
}

fn draw_input(
    buf: &mut Buffer,
    col: Rect,
    app: &App,
    meters: &Meters,
    ch: usize,
    hits: &mut HitMap,
) {
    let t = app.theme();
    let input = &app.state().inputs[ch];
    let focused = |kind| app.focus() == Control::Input { ch, kind };
    let rows = Layout::vertical([
        Constraint::Length(1), // name
        Constraint::Length(1), // gain value
        Constraint::Length(1), // gain bar
        Constraint::Length(1),
        Constraint::Fill(1),   // meter
        Constraint::Length(1), // meter readout
        Constraint::Length(1),
        Constraint::Length(1), // MON
        Constraint::Length(1), // 48V
        Constraint::Length(1), // INST
        Constraint::Length(1),
        Constraint::Length(1), // M S Ø
    ])
    .split(col);
    // Labels start after the focus mark; values end at the right edge.
    let content = col.x + 1;
    let right = col.right();
    let mark = |buf: &mut Buffer, y: u16, on: bool| {
        if on {
            put(buf, col, col.x, y, FOCUS_MARK, t.focus());
        }
    };

    put(
        buf,
        col,
        content,
        rows[0].y,
        &format!("IN{}", ch + 1),
        t.heading(),
    );

    // Gain: the value, then a bar of 2 dB cells with half cells for odd dB.
    let gain_focused = focused(InputControl::Gain);
    let y = rows[1].y;
    mark(buf, y, gain_focused);
    let value_style = if gain_focused { t.focus() } else { t.text() };
    let value = gain_text(input.gain_db, input.mute);
    put(buf, col, right - VALUE_WIDTH, y, &value, value_style);
    hits.push(
        Hit::Control(Control::Input {
            ch,
            kind: InputControl::Gain,
        }),
        Rect::new(col.x, y, col.width, 1),
    );
    let y = rows[2].y;
    let filled = if input.mute {
        t.dim()
    } else if gain_focused {
        t.focus()
    } else {
        t.text()
    };
    let gain = input.gain_db.min(MAX_GAIN_DB);
    let mut bar = Rect::new(content, y, 0, 1);
    for i in 0..GAIN_BAR_WIDTH {
        let lit = gain.saturating_sub(i as u8 * 2);
        let (s, style) = match lit {
            0 => ("─", t.dim()),
            1 => ("╸", filled),
            _ => ("━", filled),
        };
        let r = put(buf, col, content + i, y, s, style);
        bar.width += r.width;
    }
    hits.push(Hit::GainBar(ch), bar);

    draw_meter(buf, rows[4], right - 6, meters.inputs[ch], t);
    let readout = meter_text(meters.inputs[ch]);
    put(buf, col, right - VALUE_WIDTH, rows[5].y, &readout, t.dim());

    // MON, 48V, INST: a label and a lamp.
    let lamp_rows = [
        (InputControl::Mon, "MON", Some(input.mon), rows[7].y),
        (InputControl::Phantom, "48V", input.phantom, rows[8].y),
        (InputControl::Inst, "INST", Some(input.inst), rows[9].y),
    ];
    for (kind, label, on, y) in lamp_rows {
        let is_focused = focused(kind);
        mark(buf, y, is_focused);
        let label_style = if is_focused { t.focus() } else { t.text() };
        let (lamp, lamp_style) = match on {
            Some(true) => ("●", t.active()),
            Some(false) => ("○", t.dim()),
            None => ("·", t.dim()),
        };
        put(buf, col, content, y, label, label_style);
        put(buf, col, right - 1, y, lamp, lamp_style);
        hits.push(
            Hit::Control(Control::Input { ch, kind }),
            Rect::new(col.x, y, col.width, 1),
        );
    }

    // M S Ø: lit (reversed) when on, like hardware buttons.
    let y = rows[11].y;
    let switches = [
        (InputControl::Mute, "M", input.mute),
        (InputControl::Solo, "S", input.solo),
        (InputControl::Phase, "Ø", input.phase),
    ];
    mark(buf, y, switches.iter().any(|&(k, _, _)| focused(k)));
    for (i, (kind, label, on)) in switches.into_iter().enumerate() {
        let mut style = if on {
            t.active().add_modifier(Modifier::REVERSED)
        } else {
            t.dim()
        };
        if focused(kind) {
            style = style.patch(t.focus()).add_modifier(Modifier::UNDERLINED);
        }
        let x = right - 9 + i as u16 * 3;
        let r = put(buf, col, x, y, &format!(" {label} "), style);
        hits.push(Hit::Control(Control::Input { ch, kind }), r);
    }
}

/// A vertical meter, two cells wide, with eighth-cell resolution.
fn draw_meter(buf: &mut Buffer, area: Rect, x: u16, db: f64, t: Theme) {
    const EIGHTHS: [&str; 9] = ["  ", "▁▁", "▂▂", "▃▃", "▄▄", "▅▅", "▆▆", "▇▇", "██"];
    let h = area.height;
    if h == 0 {
        return;
    }
    let frac = ((db + METER_RANGE_DB) / METER_RANGE_DB).clamp(0.0, 1.0);
    let lit = (frac * f64::from(h) * 8.0).round() as i64;
    let clip = db >= METER_CLIP_DB;
    for i in 0..h {
        let y = area.bottom() - 1 - i;
        let fill = (lit - i64::from(i) * 8).clamp(0, 8) as usize;
        let top_db = -METER_RANGE_DB + f64::from(i + 1) / f64::from(h) * METER_RANGE_DB;
        let zone = if clip && i == h - 1 {
            Zone::Clip
        } else if top_db > METER_HOT_DB {
            Zone::Hot
        } else {
            Zone::Normal
        };
        let (s, style) = if fill == 0 {
            ("··", t.dim())
        } else {
            (
                EIGHTHS[fill],
                t.meter(match zone {
                    Zone::Normal => 0.2,
                    Zone::Hot => 0.6,
                    Zone::Clip => 1.0,
                }),
            )
        };
        put(buf, area, x, y, s, style);
    }
    // A scale to the right: each tick on the cell that holds it.
    let mut used = Vec::new();
    for tick in METER_TICKS_DB {
        let i = ((tick + METER_RANGE_DB) / METER_RANGE_DB * f64::from(h)).ceil() as u16;
        let i = i.clamp(1, h) - 1;
        if used.contains(&i) {
            continue;
        }
        used.push(i);
        let label = format!("{:>3}", tick as i32).replace('-', "−");
        put(buf, area, x + 3, area.bottom() - 1 - i, &label, t.dim());
    }
}

fn placeholder(buf: &mut Buffer, area: Rect, t: Theme) {
    if area.height == 0 {
        return;
    }
    let mid = area.y + area.height / 2;
    Line::styled("not available yet", t.dim())
        .centered()
        .render(
            Rect {
                y: mid,
                height: 1,
                ..area
            },
            buf,
        );
}

fn draw_mix(buf: &mut Buffer, area: Rect, t: Theme) {
    let block = panel(t, "MIX A");
    let inner = block.inner(area);
    block.render(area, buf);
    let strips = ["IN1", "IN2", "PB1/2", "PB3/4", "PB5/6"];
    let cols = Layout::horizontal([Constraint::Fill(1); 5]).split(Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    });
    for (name, col) in strips.iter().zip(cols.iter()) {
        Line::styled(*name, t.heading()).centered().render(
            Rect {
                y: inner.y,
                height: 1,
                ..*col
            },
            buf,
        );
    }
    placeholder(buf, inner, t);
}

fn draw_out(buf: &mut Buffer, area: Rect, t: Theme) {
    let block = panel(t, "OUT");
    let inner = block.inner(area);
    block.render(area, buf);
    put(buf, inner, inner.x + 1, inner.y, "1+2", t.heading());
    let y = inner.bottom().saturating_sub(4);
    for (i, label) in ["HP", "LINE", "SRC"].iter().enumerate() {
        let y = y + i as u16;
        put(buf, inner, inner.x + 1, y, label, t.text());
        put(buf, inner, inner.x + 7, y, "·", t.dim());
    }
}

fn draw_loopback(buf: &mut Buffer, area: Rect, t: Theme) {
    let block = panel(t, "LOOPBACK");
    let inner = block.inner(area);
    block.render(area, buf);
    let line = Line::from(vec![
        Span::styled(" 1+2", t.heading()),
        Span::styled("  ·", t.dim()),
        Span::styled("   │   ", t.dim()),
        Span::styled("3+4", t.heading()),
        Span::styled("  ·", t.dim()),
    ]);
    line.render(inner, buf);
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

fn help(buf: &mut Buffer, area: Rect, t: Theme, hits: &mut HitMap) {
    let keys: &[(&str, &str)] = &[
        ("Tab  Shift-Tab", "next / previous control"),
        ("← ↑ ↓ →", "move focus"),
        ("h  l", "adjust −1 / +1 dB"),
        ("H  L", "adjust −10 / +10 dB"),
        ("space", "toggle the focused switch"),
        ("m  s  p", "mute, solo, Ø of the focused input"),
        ("?", "this help"),
        ("q  Ctrl-C", "quit"),
        ("", ""),
        ("click", "focus; flip a switch"),
        ("wheel", "adjust (Shift ×10)"),
        ("drag", "set the gain on a gain bar"),
    ];
    let h = keys.len() as u16 + 6;
    let inner = modal_box(buf, area, t, " Help ", 60, h);
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
