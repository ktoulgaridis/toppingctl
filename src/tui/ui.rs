//! Rendering: a pure function of [`App`] and [`Meters`] that also records
//! where every clickable thing landed (the [`HitMap`]).
//!
//! The screen follows the design reference (docs/design, #34): a header
//! with the wordmark, then four panels placed like the official app's —
//! input and mixer on top, loopback and output below — and a line of key
//! hints. Everything is drawn from the [`State`](crate::state::State); only the input panel and
//! the mix tabs are clickable in this version (the mixer, loopback and
//! output controls come with #14 and #15).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};
use ratatui::Frame;

use crate::state::{Meters, PbStrip, Source, Strip};
use crate::tui::app::{App, Control, InputControl, MeterId, Modal, Phase, MAX_GAIN_DB};
use crate::tui::theme::Theme;
use crate::tui::widgets::dial::{Dial, DIAL_HEIGHT};
use crate::tui::widgets::fader::{fmt_db, Fader};
use crate::tui::widgets::meter::{peak_text, Meter, Scale, INPUT_SCALE_DB};
use crate::tui::widgets::pill::{pill, PillKind};

/// The smallest terminal the full screen is drawn in.
pub const MIN_WIDTH: u16 = 100;
pub const MIN_HEIGHT: u16 = 34;
/// From this header width up, the header shows its long labels.
const WIDE_HEADER: u16 = 128;
/// A mix fader's top, in dB.
const MIX_FADER_MAX_DB: f64 = 6.0;

/// Something clickable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Control(Control),
    /// The tab of mix A (0), B (1) or C (2).
    MixTab(usize),
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
    if y < area.top() || y >= area.bottom() || x >= area.right() || x < area.left() {
        return Rect::default();
    }
    let max = usize::from(area.right() - x);
    let (end, _) = buf.set_stringn(x, y, s, max, style);
    Rect::new(x, y, end - x, 1)
}

/// Writes `spans` centred on row `y` of `area`; returns each span's rect.
fn put_centered(buf: &mut Buffer, area: Rect, y: u16, spans: &[Span]) -> Vec<Rect> {
    let w: u16 = spans.iter().map(|s| s.width() as u16).sum();
    let mut x = area.x + area.width.saturating_sub(w) / 2;
    spans
        .iter()
        .map(|s| {
            let r = put(buf, area, x, y, &s.content, s.style);
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

/// The full screen: header, the four panels, key hints.
fn console(buf: &mut Buffer, area: Rect, app: &App, meters: &Meters, hits: &mut HitMap) {
    let t = app.theme();
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

    draw_header(buf, header, app, t);
    draw_input_panel(buf, input, app, meters, hits);
    draw_mixer_panel(buf, mixer, app, meters, hits);
    draw_loopback_panel(buf, loopback, app, meters);
    draw_output_panel(buf, output, app, meters);
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

fn draw_header(buf: &mut Buffer, area: Rect, app: &App, t: Theme) {
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
    // No workspaces yet (#17) and the sample rate is not HID (its own
    // story): both are shown as placeholders.
    let l1 = if wide {
        vec![
            lab("workspace  "),
            val("default"),
            lab(" ▾"),
            sep(),
            lab("sample rate  "),
            val("—"),
            lab(" Hz ▾ "),
        ]
    } else {
        vec![val("default"), lab(" ▾"), sep(), val("—"), lab(" kHz ▾ ")]
    };
    let (dot, status) = match app.phase() {
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
    if let Some(notice) = app.notice() {
        Line::from(vec![Span::raw(" "), Span::styled(notice, t.heading())]).render(area, buf);
        return;
    }
    let hints = [
        ("tab", "focus"),
        ("↑↓", "move"),
        ("←→", "adjust"),
        ("⇧", "×10"),
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
fn panel(buf: &mut Buffer, area: Rect, t: Theme, key: &str, name: &str) -> (Rect, u16) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.border());
    let inner = block.inner(area);
    block.render(area, buf);
    let mut x = area.x + 1;
    for (s, st) in [
        ("┤", t.border()),
        (key, t.focus()),
        (name, t.heading()),
        ("├", t.border()),
    ] {
        x = put(buf, area, x, area.y, s, st).right().max(x);
    }
    (inner, x)
}

/// A strip: an inner rounded box with its name set into the top border.
fn strip_box(buf: &mut Buffer, area: Rect, t: Theme, name: &str, focused: bool) -> Rect {
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

/// A gain value: `+12 dB`.
pub fn gain_text(gain_db: u8) -> String {
    format!("{:+} dB", gain_db.min(MAX_GAIN_DB))
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
    let (inner, _) = panel(buf, area, app.theme(), "¹", "input");
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
    let Control::Input {
        ch: fch,
        kind: fkind,
    } = app.focus();
    let focused = |kind| fch == ch && fkind == kind;
    let r = strip_box(buf, area, t, &format!("IN{}", ch + 1), fch == ch);
    if r.height < 2 || r.width < 8 {
        return;
    }
    let db = meters.inputs[ch];
    let x0 = r.x + 1;
    let lit = app.clips().lit(MeterId::Input(ch));
    Meter::new(db, t)
        .width(2)
        .clip(Some(lit))
        .render(Rect::new(x0, r.y, 2, r.height), buf);
    Scale::new(&INPUT_SCALE_DB, t).render(Rect::new(x0 + 2, r.y + 1, 3, r.height - 1), buf);
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
        let rects = put_centered(buf, right, y, &[span]);
        hits.push(Hit::Control(Control::Input { ch, kind }), rects[0]);
    }
    let knob_h = DIAL_HEIGHT + 1;
    if r.height >= 1 + 6 + knob_h {
        let knob = Rect::new(right.x, r.bottom() - knob_h, right.width, knob_h);
        let gain = input.gain_db.min(MAX_GAIN_DB);
        Dial::new(t)
            .value(Some(f64::from(gain) / f64::from(MAX_GAIN_DB)))
            .focused(focused(InputControl::Gain))
            .label(Some(gain_text(gain)))
            .render(knob, buf);
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
    let (inner, mut x) = panel(buf, area, t, "²", "mixer");
    // The mix tabs, set into the border; the active one is boxed.
    x = put(buf, area, x, area.y, "─", t.border()).right().max(x);
    for (m, name) in ["mix A", "mix B", "mix C"].into_iter().enumerate() {
        let colour = Theme::MIX[m];
        let start = x;
        let active = app.mix() == m;
        let parts: Vec<(String, Style)> = if active {
            vec![
                ("┤".into(), t.border()),
                (
                    ((b'a' + m as u8) as char).to_string(),
                    t.fg(colour).add_modifier(Modifier::BOLD),
                ),
                (
                    format!(" {name}"),
                    t.fg_bg(Theme::PILL_ON_FG, colour)
                        .add_modifier(Modifier::BOLD),
                ),
                ("├".into(), t.border()),
            ]
        } else {
            vec![
                (" ".into(), t.border()),
                (((b'a' + m as u8) as char).to_string(), t.fg(colour)),
                (format!(" {name} "), t.dim()),
                (" ".into(), t.border()),
            ]
        };
        for (s, st) in &parts {
            x = put(buf, area, x, area.y, s, *st).right().max(x);
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
    draw_in12_strip(buf, cols[0], app, meters, mix.link, &mix.in1, &mix.in2);
    for (i, strip) in [&mix.pb12, &mix.pb34, &mix.pb56].into_iter().enumerate() {
        draw_pb_strip(buf, cols[i + 1], app, meters, i, strip);
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

/// Two pan dials side by side with their values under them.
fn pan_dials(buf: &mut Buffer, area: Rect, t: Theme, pans: [f64; 2]) {
    let halves = columns(Rect { height: 4, ..area }, 2);
    for (h, p) in halves.into_iter().zip(pans) {
        Dial::new(t)
            .value(Some(p))
            .bipolar(true)
            .label(Some(pan_text(p)))
            .render(h, buf);
    }
}

/// L meter · fader · R meter, centred in `area`.
#[allow(clippy::too_many_arguments)]
fn stereo_body(
    buf: &mut Buffer,
    area: Rect,
    t: Theme,
    db: f64,
    max_db: f64,
    levels: [f64; 2],
    clips: [bool; 2],
) {
    let fw = Fader::width(true, area.width.saturating_sub(4));
    let total = fw + 4;
    if area.width < total || area.height == 0 {
        return;
    }
    let x = area.x + (area.width - total) / 2;
    let col = |x: u16, w: u16| Rect {
        x,
        width: w,
        ..area
    };
    Meter::new(levels[0], t)
        .clip(Some(clips[0]))
        .render(col(x, 1), buf);
    Fader::new(db, max_db, t).render(col(x + 2, fw), buf);
    Meter::new(levels[1], t)
        .clip(Some(clips[1]))
        .render(col(x + 3 + fw, 1), buf);
}

fn draw_pb_strip(buf: &mut Buffer, area: Rect, app: &App, meters: &Meters, i: usize, s: &PbStrip) {
    let t = app.theme();
    let name = ["PB 1/2", "PB 3/4", "PB 5/6"][i];
    let r = strip_box(buf, area, t, name, false);
    if r.height < 8 {
        return;
    }
    pan_dials(buf, r, t, [s.pan_l, s.pan_r]);
    put_centered(
        buf,
        r,
        r.y + 4,
        &[pill("LINK", s.link, PillKind::Normal, false, t)],
    );
    put_centered(buf, r, r.y + 5, &[value_span(s.fader_db, s.mute, t, true)]);
    let body = Rect::new(r.x, r.y + 6, r.width, r.height - 7);
    let clips = [0, 1].map(|c| app.clips().lit(MeterId::Playback(i, c)));
    stereo_body(
        buf,
        body,
        t,
        s.fader_db,
        MIX_FADER_MAX_DB,
        meters.playback[i],
        clips,
    );
    let gap = || Span::raw(" ");
    put_centered(
        buf,
        r,
        r.bottom() - 1,
        &[
            pill("Ø", s.phase, PillKind::Normal, false, t),
            gap(),
            pill("S", s.solo, PillKind::Normal, false, t),
            gap(),
            pill("M", s.mute, PillKind::Danger, false, t),
        ],
    );
}

/// A pill with no padding, for tight rows.
fn tight(label: &str, on: bool, kind: PillKind, t: Theme) -> Span<'static> {
    let p = pill(label, on, kind, false, t);
    Span::styled(label.to_string(), p.style)
}

fn draw_in12_strip(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    meters: &Meters,
    link: bool,
    in1: &Strip,
    in2: &Strip,
) {
    let t = app.theme();
    let r = strip_box(buf, area, t, "IN 1+2", false);
    if r.height < 8 {
        return;
    }
    // Link puts IN1 hard L and IN2 hard R: show what is in effect.
    let pans = if link { [0.0, 1.0] } else { [in1.pan, in2.pan] };
    pan_dials(buf, r, t, pans);
    put_centered(
        buf,
        r,
        r.y + 4,
        &[pill("LINK", link, PillKind::Disabled, false, t)],
    );
    let halves = columns(row(r, 5), 2);
    for (h, s) in halves.iter().zip([in1, in2]) {
        put_centered(buf, *h, h.y, &[value_span(s.fader_db, s.mute, t, false)]);
    }

    // M1 · F1 · scale · F2 · M2: one scale between two mono faders, tick
    // marks on both sides of each.
    let body = Rect::new(r.x, r.y + 6, r.width, r.height - 7);
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
            Meter::new(meters.inputs[k], t)
                .clip(Some(app.clips().lit(MeterId::Input(k))))
                .render(col(mx, 1), buf);
            Fader::new(s.fader_db, MIX_FADER_MAX_DB, t)
                .labels(false)
                .render(col(fx, 3), buf);
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
    for (h, s) in halves.iter().zip([in1, in2]) {
        let h = Rect {
            y: r.bottom() - 1,
            ..*h
        };
        put_centered(
            buf,
            h,
            h.y,
            &[
                tight("Ø", s.phase, PillKind::Normal, t),
                Span::raw(" "),
                tight("S", s.solo, PillKind::Normal, t),
                Span::raw(" "),
                tight("M", s.mute, PillKind::Danger, t),
            ],
        );
    }
}

// --------------------------------------------------------------- loopback

fn draw_loopback_panel(buf: &mut Buffer, area: Rect, app: &App, meters: &Meters) {
    let t = app.theme();
    let (inner, _) = panel(buf, area, t, "³", "loopback");
    for (i, col) in columns(inner, 2).into_iter().enumerate() {
        let lb = &app.state().loopbacks[i];
        let r = strip_box(buf, col, t, ["LOOP 1+2", "LOOP 3+4"][i], false);
        if r.height < 5 {
            continue;
        }
        source_line(buf, r, t, lb.source);
        put_centered(
            buf,
            r,
            r.y + 1,
            &[
                pill("LINK", lb.link, PillKind::Normal, false, t),
                Span::raw(" "),
                pill("MUTE", lb.mute, PillKind::Danger, false, t),
            ],
        );
        put_centered(
            buf,
            r,
            r.y + 2,
            &levels_spans(lb.level_l_db, lb.level_r_db, lb.mute, t),
        );
        let body = Rect::new(r.x, r.y + 3, r.width, r.height - 3);
        let clips = [0, 1].map(|c| app.clips().lit(MeterId::Loopback(i, c)));
        stereo_body(buf, body, t, lb.level_l_db, 0.0, meters.loopbacks[i], clips);
    }
}

fn source_line(buf: &mut Buffer, r: Rect, t: Theme, s: Source) {
    put_centered(
        buf,
        r,
        r.y,
        &[
            Span::styled(source_name(s), t.active()),
            Span::styled(" ▾", t.dim()),
        ],
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

fn draw_output_panel(buf: &mut Buffer, area: Rect, app: &App, meters: &Meters) {
    let t = app.theme();
    let state = app.state();
    let (inner, _) = panel(buf, area, t, "⁴", "output");
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

    let out = &state.output;
    let r = strip_box(buf, out_area, t, "OUT 1+2", false);
    if r.height >= 5 {
        source_line(buf, r, t, out.source);
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
        );
        put_centered(
            buf,
            r,
            r.y + 2,
            &levels_spans(out.level_l_db, out.level_r_db, false, t),
        );
        let body = Rect::new(r.x, r.y + 3, r.width, r.height - 3);
        let clips = [0, 1].map(|c| app.clips().lit(MeterId::Output(c)));
        stereo_body(buf, body, t, out.level_l_db, 0.0, meters.output, clips);
    }

    // Monitor Mix: an analog knob the device never reports.
    let r = strip_box(buf, mon_area, t, "MONITOR MIX", false);
    if r.height >= 4 {
        let top = u16::from(r.height >= 8);
        Dial::new(t).render(Rect::new(r.x, r.y + top, r.width, DIAL_HEIGHT), buf);
        put_centered(
            buf,
            r,
            r.y + top + DIAL_HEIGHT,
            &[
                Span::styled("input", t.dim()),
                Span::raw("    "),
                Span::styled("playback", t.dim()),
            ],
        );
        put_centered(
            buf,
            r,
            r.y + top + DIAL_HEIGHT + 2,
            &[Span::styled(
                if r.width >= 19 {
                    "knob on device only"
                } else {
                    "on device only"
                },
                t.track(),
            )],
        );
    }

    // Phone out: the gain switch and the volume knob's raw position.
    let r = strip_box(buf, phone_area, t, "PHONE", false);
    if r.height >= 4 {
        put_centered(
            buf,
            r,
            r.y + 1,
            &[pill("GAIN", state.phone_gain, PillKind::Normal, false, t)],
        );
        put_centered(buf, r, r.y + 3, &[Span::styled("volume knob", t.dim())]);
        let knob = state
            .readout
            .volume_knob
            .map_or("—".to_string(), |v| v.to_string());
        put_centered(buf, r, r.y + 4, &[Span::styled(knob, t.heading())]);
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

fn help(buf: &mut Buffer, area: Rect, t: Theme, hits: &mut HitMap) {
    let keys: &[(&str, &str)] = &[
        ("Tab  Shift-Tab", "next / previous control"),
        ("↑ ↓", "move along an input"),
        ("← →", "adjust the gain −1 / +1 dB"),
        ("Shift ← →", "adjust −10 / +10 dB"),
        ("0", "reset the gain to +0 dB"),
        ("space", "toggle the focused switch"),
        ("a  b  c", "show mix A / B / C"),
        ("?", "this help"),
        ("q  Ctrl-C", "quit"),
        ("", ""),
        ("click", "focus; flip a switch; pick a mix"),
        ("wheel", "adjust a knob (Shift ×10)"),
        ("drag", "turn a knob (up / down)"),
        ("double-click", "reset a knob"),
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
