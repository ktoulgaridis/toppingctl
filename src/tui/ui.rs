//! Rendering: a pure function of [`App`] and [`Meters`] that also records
//! where every clickable thing landed (the [`HitMap`]).
//!
//! The screen follows the design reference (docs/design, #34): a header
//! with the wordmark, then four panels placed like the official app's —
//! input and mixer on top, loopback and output below — and a line of key
//! hints. Everything is drawn from the [`State`](crate::state::State).
//! The input panel, the mix tabs and the mixer are clickable; the loopback
//! and output controls come with #15. The layout scales with the window
//! (#14): panels, strips, faders, meters and knobs grow and shrink in the
//! app's proportions, each strip's content one compact group with spare
//! rows spread evenly. Only absurd sizes (past [`MAX_WIDTH`]×[`MAX_HEIGHT`])
//! stop growing, centred.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};
use ratatui::Frame;

use crate::rate::{format_khz, RATES};
use crate::state::{Meters, Source};
use crate::tui::app::{
    App, BusControl, ComboItem, Control, InputControl, MeterId, MixControl, Modal, Name, NameThen,
    Panel, Phase, BUS_MAX_DB, DOWNLOAD_OPTIONS, MAX_GAIN_DB, MIX_FADER_MAX_DB, OUT_BUS,
    SOURCE_MENU,
};
use crate::tui::pixel::{
    chip_rgb, clear_under, label_rgb, FaceLabel, PixelLayer, PixelSpot, PixelWidget,
};
use crate::tui::theme::Theme;
use crate::tui::widgets::dial::{knob_rect, Dial};
use crate::tui::widgets::fader::{fmt_db, level_text, Fader};
use crate::tui::widgets::meter::{peak_text, Meter, Scale, INPUT_SCALE_DB};
use crate::tui::widgets::pill::{button, pill, PillKind};

/// The smallest terminal the full screen is drawn in.
pub const MIN_WIDTH: u16 = 100;
pub const MIN_HEIGHT: u16 = 34;
/// The console grows with the window up to this, a size no real screen
/// needs; past it the console is centred.
pub const MAX_WIDTH: u16 = 320;
pub const MAX_HEIGHT: u16 = 100;
/// From this header width up, the header shows its long labels.
const WIDE_HEADER: u16 = 128;
/// The longest workspace name the header shows; longer ones end in "…".
const HEADER_NAME_MAX: usize = 20;

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
    /// Item `i` of a source menu ([`SOURCE_MENU`]).
    SourceOption(usize),
    /// The header's workspace combo, or its `l` hint: opens the combo.
    Workspaces,
    /// The header's `s` hint: saves the workspace.
    SaveKey,
    /// The header's `w` hint: opens the Download dialog.
    Download,
    /// Entry `i` of the workspace combo ([`App::combo_items`]).
    ComboItem(usize),
    /// Entry `i`'s Save button.
    ComboSave(usize),
    /// Entry `i`'s Delete button.
    ComboDelete(usize),
    /// Choice `i` of the Download dialog ([`DOWNLOAD_OPTIONS`]).
    DownloadOption(usize),
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
    /// The open modal's box, from the last render.
    modal: Option<Rect>,
    /// Whether the mix strips stacked their Ø / SOLO / MUTE.
    stacked: bool,
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

    /// The open modal's box, when one was drawn.
    pub fn modal_rect(&self) -> Option<Rect> {
        self.modal
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether the last render stacked the mix strips' Ø / SOLO / MUTE
    /// (a tall window) rather than setting them in a row: ↑↓ and ←→
    /// follow it ([`App::set_switches_stacked`]).
    pub fn switches_stacked(&self) -> bool {
        self.stacked
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
    /// style has a background (the active mix tab): a face inset in its
    /// cells.
    fn chip(&mut self, rect: Rect, style: Style) {
        if let Some(rgb) = style.bg.and_then(chip_rgb) {
            self.pixel(
                rect,
                PixelWidget::Chip {
                    rgb,
                    label: None,
                    focused: false,
                },
            );
        }
    }

    /// Records a button's face under `span` drawn over `rect` (its label
    /// on the rect's middle row), when the span has a background. The face
    /// sits on the button's cells, so a column of buttons lines up; a label
    /// that can't be centred in whole cells (its parity is not the box's)
    /// is drawn in the face, exactly centred. An underlined button is the
    /// focused one ([`button`]).
    fn pill(&mut self, rect: Rect, span: &Span) {
        let Some(rgb) = span.style.bg.and_then(chip_rgb) else {
            return;
        };
        let focused = span.style.add_modifier.contains(Modifier::UNDERLINED);
        let text = span.content.trim();
        let tw = Span::raw(text).width() as u16;
        let label = if rect.width > tw && (rect.width - tw) % 2 == 1 {
            FaceLabel::new(text, label_rgb(span.style.fg))
        } else {
            None
        };
        self.pixel(
            rect,
            PixelWidget::Chip {
                rgb,
                label,
                focused,
            },
        );
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
/// placed widget is blanked, labels kept. While a modal is open only the
/// widgets under its box go, so its text stays on top and the rest of the
/// screen neither flickers nor gets sent again.
pub fn draw_with(
    f: &mut Frame,
    app: &App,
    meters: &Meters,
    pixels: Option<&mut PixelLayer>,
) -> HitMap {
    let hits = draw_text(f, app, meters);
    if let Some(layer) = pixels {
        let spots: Vec<PixelSpot> = match hits.modal_rect() {
            None if app.modal().is_none() => hits.pixels().to_vec(),
            None => Vec::new(),
            Some(m) => hits
                .pixels()
                .iter()
                .filter(|s| !s.rect.intersects(m))
                .cloned()
                .collect(),
        };
        layer.update(&spots, f.area());
        clear_under(layer, &spots, f.buffer_mut());
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
        Some(Modal::Source { bus, sel }) => source_menu(buf, area, app, bus, sel, &mut hits),
        Some(Modal::Workspaces { sel }) => combo(buf, area, app, sel, &mut hits),
        Some(Modal::WorkspaceName { then }) => name_prompt(buf, area, app, then, &mut hits),
        Some(Modal::Overwrite { .. }) => confirm(
            buf,
            area,
            t,
            " Overwrite? ",
            &[&format!("Overwrite workspace {:?}?", app.prompt())],
            ("Overwrite", "Cancel"),
            &mut hits,
        ),
        Some(Modal::RemoveWorkspace) => confirm(
            buf,
            area,
            t,
            " Remove workspace? ",
            &[
                &format!("Remove workspace {:?}?", app.prompt()),
                "Its file is deleted.",
            ],
            ("Remove", "Cancel"),
            &mut hits,
        ),
        Some(Modal::Download { sel }) => download(buf, area, t, sel, &mut hits),
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
/// A span with a background (a button) gets its face under it on the
/// pixel layer.
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
            hits.pill(r, s);
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

    // The readings, and the levels after the faders.
    let both = [meters.clone(), meters.post(app.state(), app.mix())];
    draw_header(buf, header, app, t, hits);
    draw_input_panel(buf, input, app, meters, hits);
    draw_mixer_panel(buf, mixer, app, &both, hits);
    draw_loopback_panel(buf, loopback, app, &both, hits);
    draw_output_panel(buf, output, app, &both, hits);
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
    // The workspace combo (a click opens it) and the sample rate, read
    // from the OS (crate::rate; a click opens the list).
    let rate = Span::styled(format!("{} ▾", format_khz(app.sample_rate())), t.heading());
    let name = Span::styled(header_name(app.workspace()), t.heading());
    let mut l1 = if wide {
        vec![lab("workspace  ")]
    } else {
        vec![]
    };
    let name_at = l1.len();
    l1.extend([name, lab(" ▾"), sep()]);
    if wide {
        l1.push(lab("sample rate  "));
    }
    let rate_at = l1.len();
    l1.extend([rate, Span::raw(" ")]);
    let x1 = span_xs(inner, &l1);
    let combo_w = (l1[name_at].width() + l1[name_at + 1].width()) as u16;
    hits.push(Hit::Workspaces, Rect::new(x1[name_at], inner.y, combo_w, 1));
    hits.push(
        Hit::Rate,
        Rect::new(x1[rate_at], inner.y, l1[rate_at].width() as u16, 1),
    );
    let (dot, status) = match app.phase() {
        Phase::Synced if app.rx_stale() => (t.warn(), " no meters "),
        Phase::Synced => (t.fg(Theme::OK), " synced "),
        Phase::Connecting => (t.dim(), " connecting "),
    };
    let (save, store) = if wide {
        (" save workspace", " store to device")
    } else {
        (" save", " store")
    };
    let mut l2 = vec![
        key("s"),
        lab(save),
        sep(),
        key("l"),
        lab(" workspaces"),
        sep(),
        key("w"),
        lab(store),
    ];
    l2.extend([sep(), Span::styled("●", dot), lab(status)]);
    let x2 = span_xs(inner, &l2);
    for (at, hit) in [(0, Hit::SaveKey), (3, Hit::Workspaces), (6, Hit::Download)] {
        let w = (l2[at].width() + l2[at + 1].width()) as u16;
        hits.push(hit, Rect::new(x2[at], inner.y + 1, w, 1));
    }
    Line::from(l1).right_aligned().render(row(inner, 0), buf);
    Line::from(l2).right_aligned().render(row(inner, 1), buf);
}

/// The workspace's name for the header: "Default" when none is open,
/// cut to [`HEADER_NAME_MAX`] with "…".
fn header_name(name: Option<&str>) -> String {
    let name = name.unwrap_or("Default");
    if name.chars().count() > HEADER_NAME_MAX {
        let cut: String = name.chars().take(HEADER_NAME_MAX - 1).collect();
        format!("{cut}…")
    } else {
        name.to_string()
    }
}

/// The column each span starts at, with `spans` right-aligned in `area`.
fn span_xs(area: Rect, spans: &[Span]) -> Vec<u16> {
    let total: u16 = spans.iter().map(|s| s.width() as u16).sum();
    let mut x = area.right().saturating_sub(total);
    spans
        .iter()
        .map(|s| {
            let at = x;
            x += s.width() as u16;
            at
        })
        .collect()
}

fn draw_footer(buf: &mut Buffer, area: Rect, app: &App, t: Theme) {
    if let Some((name, _)) = app.editing() {
        // Renaming takes the keyboard: say so, and how to leave.
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(
                format!(" rename {} ", name.default_name()),
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
    // What is focused, what the keys do there, then the keys that always
    // work.
    let (what, keys) = focus_hint(app);
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(what, t.heading()),
        Span::raw("   "),
    ];
    let always = [
        ("←→↑↓", "move"),
        ("tab 1-4", "panel"),
        ("?", "help"),
        ("q", "quit"),
    ];
    for (k, what) in keys.iter().chain(&always) {
        spans.push(Span::styled(*k, t.focus()));
        spans.push(Span::styled(format!(" {what}   "), t.dim()));
    }
    Line::from(spans).render(area, buf);
}

/// The keys that adjust a knob or fader.
const ADJUST_KEYS: &[(&str, &str)] = &[("+/-", "adjust"), ("0", "reset")];
/// The key that flips a switch.
const TOGGLE_KEYS: &[(&str, &str)] = &[("space", "toggle")];

/// `on` / `off`.
fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// The footer's account of the focus: "strip · control value", and the
/// keys that act on it.
fn focus_hint(app: &App) -> (String, &'static [(&'static str, &'static str)]) {
    let state = app.state();
    let name = |n: Name| n.get(state).unwrap_or(n.default_name()).to_string();
    let side = |linked: bool, c: usize| if linked { "" } else { ["L ", "R "][c.min(1)] };
    let names: &[(&str, &str)] = &[("enter", "rename"), ("↓", "controls")];
    match app.focus() {
        Control::Input { ch, kind } => {
            let input = &state.inputs[ch];
            let strip = format!("IN{}", ch + 1);
            let (label, on) = match kind {
                InputControl::Gain => {
                    let what = format!("{strip} · gain {}", gain_text(input.gain_db));
                    return (what, ADJUST_KEYS);
                }
                InputControl::Mon => ("MON", input.mon),
                InputControl::Phantom => ("48V", input.phantom == Some(true)),
                InputControl::Inst => ("INST", input.inst),
                InputControl::Solo => ("SOLO", input.solo),
                InputControl::Mute => ("MUTE", input.mute),
                InputControl::Phase => ("Ø", input.phase),
            };
            (format!("{strip} · {label} {}", on_off(on)), TOGGLE_KEYS)
        }
        Control::Mixer(i) => (format!("{} · name", name(Name::Mixer(i))), names),
        Control::Loopback(i) => (format!("{} · name", name(Name::Loopback(i))), names),
        Control::Output(0) => (format!("{} · name", name(Name::Output)), names),
        Control::Output(1) => ("MONITOR MIX · read-only".into(), &[]),
        Control::Output(_) => {
            let level = if state.phone_gain { "+17 dBu" } else { "0 dBu" };
            (format!("PHONE · level {level}"), TOGGLE_KEYS)
        }
        Control::Mix { strip, ctl } => {
            let mix = &state.mixes[app.mix()];
            let linked = mix.linked(strip);
            let strip_name = name(Name::Mixer(strip));
            let ch = |c: usize| mix.channel(strip, c);
            let (what, keys) = match ctl {
                MixControl::Pan(c) => {
                    let lr = ["left", "right"][c.min(1)];
                    let pan = pan_text(mix.pans(strip)[c.min(1)]);
                    (format!("{lr} pan {pan}"), ADJUST_KEYS)
                }
                MixControl::Fader(c) => {
                    let db = fmt_db(ch(c).fader_db);
                    (format!("{}fader {db}", side(linked, c)), ADJUST_KEYS)
                }
                MixControl::Link => (format!("LINK {}", on_off(linked)), TOGGLE_KEYS),
                MixControl::Phase(c) => {
                    let on = on_off(ch(c).phase);
                    (format!("{}Ø {on}", side(linked, c)), TOGGLE_KEYS)
                }
                MixControl::Solo(c) => {
                    let on = on_off(ch(c).solo);
                    (format!("{}SOLO {on}", side(linked, c)), TOGGLE_KEYS)
                }
                MixControl::Mute(c) => {
                    let on = on_off(ch(c).mute);
                    (format!("{}MUTE {on}", side(linked, c)), TOGGLE_KEYS)
                }
            };
            (format!("{strip_name} · {what}"), keys)
        }
        Control::Bus { bus, ctl } => {
            let out = bus == OUT_BUS;
            let (strip_name, source, link, levels, mutes) = if out {
                let o = &state.output;
                let levels = [o.level_l_db, o.level_r_db];
                (name(Name::Output), o.source, o.link, levels, [false; 2])
            } else {
                let lb = &state.loopbacks[bus.min(1)];
                let levels = [lb.level_l_db, lb.level_r_db];
                let mutes = [lb.mute_l, lb.mute_r];
                (name(Name::Loopback(bus)), lb.source, lb.link, levels, mutes)
            };
            let (what, keys): (String, &'static [(&str, &str)]) = match ctl {
                BusControl::Source => (
                    format!("source {}", source_name(source)),
                    &[("enter", "menu")],
                ),
                BusControl::Link => (format!("LINK {}", on_off(link)), TOGGLE_KEYS),
                BusControl::Mute(c) => {
                    let on = on_off(mutes[c.min(1)]);
                    (format!("{}MUTE {on}", side(link, c)), TOGGLE_KEYS)
                }
                BusControl::Hp => (
                    format!("HP {}", on_off(state.output.headphones)),
                    TOGGLE_KEYS,
                ),
                BusControl::Line => (format!("LINE {}", on_off(state.output.line)), TOGGLE_KEYS),
                BusControl::Fader(c) => {
                    let level = level_text(levels[c.min(1)]);
                    (format!("{}level {level} dB", side(link, c)), ADJUST_KEYS)
                }
            };
            (format!("{strip_name} · {what}"), keys)
        }
    }
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

/// How a strip's box shows focus: not at all, on one of its controls
/// (the accent border and title), or on its name (the title reversed, with
/// the focus outline round it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StripFocus {
    None,
    Inside,
    Name,
}

impl StripFocus {
    fn of(inside: bool, name: bool) -> StripFocus {
        match (inside, name) {
            (_, true) => StripFocus::Name,
            (true, _) => StripFocus::Inside,
            _ => StripFocus::None,
        }
    }
}

/// A strip: an inner rounded box with its name set into the top border
/// in plain bold text (the accent when focused).
fn strip_box(
    buf: &mut Buffer,
    area: Rect,
    t: Theme,
    name: &str,
    focus: StripFocus,
    hits: &mut HitMap,
) -> Rect {
    let focused = focus != StripFocus::None;
    let title = match focus {
        StripFocus::None => t.heading(),
        StripFocus::Inside => t.focus(),
        StripFocus::Name => t.focus().add_modifier(Modifier::REVERSED),
    };
    let label = format!(" {name} ");
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(if focused { t.active() } else { t.track() })
        .title(Line::from(Span::styled(label.clone(), title)).centered());
    let inner = block.inner(area);
    block.render(area, buf);
    if focus == StripFocus::Name {
        // The title is centred between the corners.
        let tw = (Span::raw(&label).width() as u16).min(inner.width);
        let x = inner.x + (inner.width - tw) / 2;
        hits.pixel(Rect::new(x, area.y, tw, 1), PixelWidget::Focus);
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

/// The width of a button on its own (input switches, LINK, MUTE, HP,
/// LINE): even, so the four-letter labels stay terminal text centred in
/// whole cells (Ø, MON and 48V are drawn in the face).
const BUTTON_W: u16 = 6;

/// A round knob (two columns a row) about `cols` wide, at most `rows`
/// tall and at least 2: its (cols, rows).
fn knob_size(cols: u16, rows: u16) -> (u16, u16) {
    let r = (cols / 2).min(rows).max(2);
    (r * 2, r)
}

/// A pan knob for a dial column `half` wide: about 78 % of it (#34's
/// filled the column), and at most a fifth of the strip's `height`.
fn pan_knob(half: u16, height: u16) -> (u16, u16) {
    knob_size(half * 39 / 50, height / 5)
}

/// A knob to draw: the Braille knob as text, and its cells recorded for
/// the pixel layer.
struct Knob {
    value: Option<f64>,
    label: Option<String>,
    focused: bool,
    dimmed: bool,
    max: (u16, u16),
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
                dimmed: self.dimmed,
                bipolar: self.bipolar,
                focused: self.focused,
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
    Meter::new(db, t).clip(clip).render(area, buf);
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

/// A meter's sunken box over `rect`: a darker background in the text form,
/// and an inset image under the text for the pixel layer.
fn inset(buf: &mut Buffer, rect: Rect, t: Theme, hits: &mut HitMap) {
    let rect = rect.intersection(buf.area);
    if t.is_color() {
        for y in rect.top()..rect.bottom() {
            for x in rect.left()..rect.right() {
                buf[(x, y)].set_bg(Theme::INSET_BG);
            }
        }
    }
    hits.pixel(rect, PixelWidget::Inset);
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
        PixelWidget::Fader {
            db,
            max_db,
            labels,
            focused,
        },
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

/// A source as the app's menu labels it.
fn menu_label(s: Source) -> &'static str {
    match s {
        Source::MixA => "Mix A",
        Source::MixB => "Mix B",
        Source::MixC => "Mix C",
        Source::In1 => "IN 1",
        Source::In2 => "IN 2",
        Source::In12 => "IN1+2",
        Source::Pb12 => "PB 1/2",
        Source::Pb34 => "PB 3/4",
        Source::Pb56 => "PB 5/6",
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
    let sf = StripFocus::of(in_strip, false);
    let r = strip_box(buf, area, t, &format!("IN{}", ch + 1), sf, hits);
    if r.height < 2 || r.width < 8 {
        return;
    }
    // The meter takes the strip's height; beside it the six switches and
    // the gain knob are spread evenly down to the meter's foot.
    // The meter column, in its own sunken box: the peak readout on top,
    // then the clip light over the bar, the scale beside it.
    let meter_h = r.height;
    let db = meters.inputs[ch];
    let x0 = r.x + 1;
    let lit = app.clips().lit(MeterId::Input(ch));
    put(buf, r, x0, r.y, &peak_text(db), t.peak(db));
    if meter_h >= 3 {
        meter(
            buf,
            Rect::new(x0, r.y + 1, 2, meter_h - 1),
            db,
            2,
            Some(lit),
            t,
            hits,
        );
        Scale::new(&INPUT_SCALE_DB, t).render(Rect::new(x0 + 2, r.y + 2, 3, meter_h - 2), buf);
    }
    inset(buf, Rect::new(r.x, r.y, 7, meter_h), t, hits);

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
    // The gain knob: about 0.49 of the strip's width (78 % of #34's
    // 0.63), round, leaving rows for the switches.
    let avail = r.height.saturating_sub(1);
    let (kc, kr) = knob_size(r.width * 49 / 100, avail.saturating_sub(7));
    let knob_rows = if avail >= 6 + 3 { kr + 1 } else { 0 };
    // Spare rows go evenly into the six gaps after each switch (the
    // remainder to the last ones, so the knob never crowds the switches).
    let spare = avail.saturating_sub(6 + knob_rows);
    let gap = |i: u16| spare / 6 + u16::from(i >= 6 - spare % 6);
    let mut ys = Vec::with_capacity(7);
    let mut y = r.y + 1;
    for i in 0..7 {
        ys.push(y);
        y += 1 + gap(i);
    }
    for (i, (kind, label, on, pk)) in switches.into_iter().enumerate() {
        let y = ys[i];
        let span = button(label, BUTTON_W.min(right.width), on, pk, focused(kind), t);
        let rects = put_centered(buf, right, y, &[span], hits);
        hits.push(Hit::Control(Control::Input { ch, kind }), rects[0]);
    }
    // The gain knob after the last switch, at the same spacing: dial,
    // then value.
    let knob_y = ys[6];
    if knob_rows > 0 && r.bottom() >= knob_y + knob_rows {
        // From just right of the meter box to the edge, the knob centred.
        let kx = x0 + 6;
        let knob = Rect::new(kx, knob_y, r.right().saturating_sub(kx), knob_rows);
        let gain = input.gain_db.min(MAX_GAIN_DB);
        let k = Knob {
            value: Some(f64::from(gain) / f64::from(MAX_GAIN_DB)),
            label: Some(gain_text(gain)),
            focused: focused(InputControl::Gain),
            dimmed: false,
            max: (kc, kr),
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

fn draw_mixer_panel(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    meters: &[Meters; 2],
    hits: &mut HitMap,
) {
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
                    format!(" {key} {name} "),
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
        let key_x = start + 1 + u16::from(active);
        hits.push(
            Hit::MixTab(m),
            Rect::new(key_x, area.y, x.saturating_sub(key_x + 1), 1),
        );
    }
    for (strip, col) in columns(inner, 4).into_iter().enumerate() {
        draw_mix_strip(buf, col, app, meters, strip, hits);
    }
}

/// A strip's name: the user's, else the default; while it is being
/// edited, the text so far and a cursor.
fn strip_name(app: &App, name: Name) -> String {
    if let Some((n, text)) = app.editing() {
        if n == name {
            return format!("{text}▏");
        }
    }
    name.get(app.state())
        .unwrap_or(name.default_name())
        .to_string()
}

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

/// A loopback or output level as the app reads it (`−6`, `−∞`), with the
/// unit when asked; `muted` in red when muted.
fn level_span(db: f64, mute: bool, t: Theme, unit: bool) -> Span<'static> {
    if mute {
        return Span::styled("muted", t.warn());
    }
    let s = level_text(db);
    Span::styled(if unit { format!("{s} dB") } else { s }, t.heading())
}

/// Rows for a mix strip's two pan dials and their values: the pan knob
/// (it scales with the strip) and the label.
fn dial_rows(r: Rect) -> u16 {
    pan_knob(r.width / 2, r.height).1 + 1
}

/// The fewest fader rows a mix strip keeps before LINK gives up its blank
/// rows.
const MIX_FADER_MIN: u16 = 6;

/// A mix strip's content, top to bottom: the dials (`d` rows), a blank
/// row, LINK, a blank row, the value, the fader (every row left) and the
/// switches (`rows`), as in the mockup. Returns the blank rows' height (1,
/// or 0 when the fader would get too short) and the fader's height.
fn mix_group(r: Rect, d: u16, rows: u16) -> (u16, u16) {
    let base = r.height.saturating_sub(d + 2 + rows);
    let gap = u16::from(base >= MIX_FADER_MIN + 2);
    (gap, base - 2 * gap)
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
            max: pan_knob(h.width, area.height),
            bipolar: true,
        };
        k.draw(buf, h, app.theme(), hits);
        hits.push(Hit::Control(c), h);
    }
}

/// One channel's meters: the reading (pre-fader) and the level after the
/// fader (post-fader, [`Meters::post`]), each with its clip light (IN,
/// OUT).
#[derive(Clone, Copy, Debug)]
struct ChannelMeter {
    pre: f64,
    post: f64,
    clip_in: bool,
    clip_out: bool,
}

impl ChannelMeter {
    fn of(app: &App, meters: &[Meters; 2], id: MeterId) -> Self {
        let read = |m: &Meters| match id {
            MeterId::Input(c) => m.inputs[c],
            MeterId::Playback(p, c) => m.playback[p][c],
            MeterId::Output(c) => m.output[c],
            MeterId::Loopback(l, c) => m.loopbacks[l][c],
        };
        ChannelMeter {
            pre: read(&meters[0]),
            post: read(&meters[1]),
            clip_in: app.clips().lit(id),
            clip_out: app.clips().lit_out(id),
        }
    }
}

/// A channel's pre- and post-fader meters side by side, two columns at
/// `area.x`, in one sunken box.
fn meter_pair(buf: &mut Buffer, area: Rect, m: ChannelMeter, t: Theme, hits: &mut HitMap) {
    let col = |dx: u16| Rect {
        x: area.x + dx,
        width: 1,
        ..area
    };
    meter(buf, col(0), m.pre, 1, Some(m.clip_in), t, hits);
    meter(buf, col(1), m.post, 1, Some(m.clip_out), t, hits);
    inset(buf, Rect { width: 2, ..area }, t, hits);
}

/// The linked page's body: L meters · fader · R meters, centred in
/// `area`, a column apart when there is room. Returns the fader's cells.
#[allow(clippy::too_many_arguments)]
fn stereo_body(
    buf: &mut Buffer,
    area: Rect,
    t: Theme,
    db: f64,
    max_db: f64,
    chans: [ChannelMeter; 2],
    focused: bool,
    hits: &mut HitMap,
) -> Rect {
    let fw = Fader::width(true, area.width.saturating_sub(6));
    let gap = u16::from(area.width >= fw + 6);
    let total = fw + 4 + 2 * gap;
    if area.width < total || area.height == 0 {
        return Rect::default();
    }
    let x = area.x + (area.width - total) / 2;
    let col = |x: u16, w: u16| Rect {
        x,
        width: w,
        ..area
    };
    meter_pair(buf, col(x, 2), chans[0], t, hits);
    let f = col(x + 2 + gap, fw);
    fader(buf, f, db, max_db, true, focused, t, hits);
    meter_pair(buf, col(x + 2 + 2 * gap + fw, 2), chans[1], t, hits);
    Fader::rect(true, f)
}

/// The unlinked page's body: L meters · L fader · scale · R fader · R
/// meters, one scale between two thin faders, centred in `area`. Returns
/// the faders' cells.
#[allow(clippy::too_many_arguments)]
fn split_body(
    buf: &mut Buffer,
    area: Rect,
    t: Theme,
    dbs: [f64; 2],
    max_db: f64,
    chans: [ChannelMeter; 2],
    focused: [bool; 2],
    hits: &mut HitMap,
) -> [Rect; 2] {
    let gap = u16::from(area.width >= 15);
    let total = 13 + 2 * gap;
    if area.width < total || area.height == 0 {
        return [Rect::default(); 2];
    }
    let x = area.x + (area.width - total) / 2;
    let col = |x: u16, w: u16| Rect {
        x,
        width: w,
        ..area
    };
    let fx = [x + 2 + gap, x + 8 + gap];
    meter_pair(buf, col(x, 2), chans[0], t, hits);
    meter_pair(buf, col(x + 11 + 2 * gap, 2), chans[1], t, hits);
    let mut out = [Rect::default(); 2];
    for c in 0..2 {
        let f = col(fx[c], 3);
        fader(buf, f, dbs[c], max_db, false, focused[c], t, hits);
        out[c] = Fader::rect(false, f);
    }
    for (y, label) in Fader::ticks(max_db, area.height) {
        put(
            buf,
            area,
            x + 5 + gap,
            area.y + y,
            &format!("{label:>3}"),
            t.scale(),
        );
    }
    out
}

/// The control of mixer strip `strip`.
fn mix_ctl(strip: usize, ctl: MixControl) -> Control {
    Control::Mix { strip, ctl }
}

/// Mixer strip `strip` (IN 1+2, PB 1/2, 3/4, 5/6) on the page LINK picks:
/// linked, one value, fader and set of switches; unlinked, an L and an R
/// of each, side by side.
fn draw_mix_strip(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    meters: &[Meters; 2],
    strip: usize,
    hits: &mut HitMap,
) {
    let t = app.theme();
    let focus = app.focus();
    let mix = &app.state().mixes[app.mix()];
    let linked = mix.linked(strip);
    hits.push(Hit::Focus(Control::Mixer(strip)), area);
    let sf = StripFocus::of(strip_focused(app, strip), focus == Control::Mixer(strip));
    let r = strip_box(buf, area, t, &strip_name(app, Name::Mixer(strip)), sf, hits);
    if r.height < 8 {
        return;
    }
    let d = dial_rows(r);
    let rows = switch_rows(r);
    let (gap, fader_h) = mix_group(r, d, rows);
    pan_dials(buf, r, app, strip, mix.pans(strip), d, hits);
    let (link_y, value_y) = (r.y + d + gap, r.y + d + 2 * gap + 1);
    let lc = mix_ctl(strip, MixControl::Link);
    // IN 1+2's LINK greys out while off, as in the design.
    let kind = if strip == 0 {
        PillKind::Disabled
    } else {
        PillKind::Normal
    };
    let rects = put_centered(
        buf,
        r,
        link_y,
        &[button("LINK", BUTTON_W, linked, kind, focus == lc, t)],
        hits,
    );
    hits.push(Hit::Control(lc), rects[0]);
    let chans = [0, 1].map(|c| mix.channel(strip, c));
    let ids = [0, 1].map(|c| match strip {
        0 => MeterId::Input(c),
        s => MeterId::Playback(s - 1, c),
    });
    let cm = ids.map(|id| ChannelMeter::of(app, meters, id));
    let body = Rect::new(r.x, value_y + 1, r.width, fader_h);
    let halves = columns(row(r, value_y - r.y), 2);
    if linked {
        let c = chans[0];
        put_centered(
            buf,
            r,
            value_y,
            &[value_span(c.fader_db, c.mute, t, true)],
            hits,
        );
        let fc = mix_ctl(strip, MixControl::Fader(0));
        let f = stereo_body(
            buf,
            body,
            t,
            c.fader_db,
            MIX_FADER_MAX_DB,
            cm,
            focus == fc,
            hits,
        );
        hits.push(Hit::Control(fc), f);
        let on = [c.phase, c.solo, c.mute];
        strip_switches(buf, r, app, strip, 0, on, rows, false, hits);
        return;
    }
    for (h, c) in halves.iter().zip(chans) {
        put_centered(
            buf,
            *h,
            h.y,
            &[value_span(c.fader_db, c.mute, t, false)],
            hits,
        );
    }
    let fcs = [0, 1].map(|c| mix_ctl(strip, MixControl::Fader(c)));
    let fs = split_body(
        buf,
        body,
        t,
        chans.map(|c| c.fader_db),
        MIX_FADER_MAX_DB,
        cm,
        fcs.map(|fc| focus == fc),
        hits,
    );
    for (fc, f) in fcs.into_iter().zip(fs) {
        hits.push(Hit::Control(fc), f);
    }
    for (k, (h, c)) in halves.iter().zip(chans).enumerate() {
        let col = Rect {
            y: r.y,
            height: r.height,
            ..*h
        };
        let on = [c.phase, c.solo, c.mute];
        strip_switches(buf, col, app, strip, k, on, rows, true, hits);
    }
}

/// Rows for a mix strip's Ø / SOLO / MUTE: one row, as in the mockup at
/// 120×40; stacked when the strip is tall enough to spare them.
fn switch_rows(r: Rect) -> u16 {
    if r.height >= 21 {
        3
    } else {
        1
    }
}

/// Channel `ch`'s Ø / SOLO / MUTE of mixer strip `strip` (`on` in that
/// order) at the bottom of `r`, in `rows` rows (3 or 1); each is a click
/// target. `narrow` (half a strip): one row of tight pills.
#[allow(clippy::too_many_arguments)]
fn strip_switches(
    buf: &mut Buffer,
    r: Rect,
    app: &App,
    strip: usize,
    ch: usize,
    on: [bool; 3],
    rows: u16,
    narrow: bool,
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
        hits.stacked = true;
        for (k, label) in ["Ø", "SOLO", "MUTE"].into_iter().enumerate() {
            let span = button(label, BUTTON_W.min(r.width), on[k], kinds[k], focused(k), t);
            let rects = put_centered(buf, r, r.bottom() - 3 + k as u16, &[span], hits);
            hits.push(Hit::Control(mix_ctl(strip, ctls[k])), rects[0]);
        }
        return;
    }
    // A whole strip: Ø / SOLO / MUTE side by side, as in the mockup, a
    // column apart, when they fit. An even width keeps SOLO and MUTE text centred in whole cells (Ø is
    // drawn in its face): 6 columns, or 4 when that doesn't fit.
    let fit = r.width.saturating_sub(2) / 3;
    let bw = if fit >= BUTTON_W { BUTTON_W } else { 4 };
    if !narrow && fit >= 4 {
        let mut spans = Vec::new();
        for (k, label) in ["Ø", "SOLO", "MUTE"].into_iter().enumerate() {
            if k > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(button(label, bw, on[k], kinds[k], focused(k), t));
        }
        let rects = put_centered(buf, r, r.bottom() - 1, &spans, hits);
        for k in 0..3 {
            hits.push(Hit::Control(mix_ctl(strip, ctls[k])), rects[2 * k]);
        }
        return;
    }
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

// ------------------------------------------------- loopback and output

fn draw_loopback_panel(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    meters: &[Meters; 2],
    hits: &mut HitMap,
) {
    let (inner, _) = panel(buf, area, app, Panel::Loopback, "³", "loopback");
    for (i, col) in columns(inner, 2).into_iter().enumerate() {
        draw_bus_strip(buf, col, app, meters, i, hits);
    }
}

/// The fewest fader rows a loopback or output strip keeps before giving
/// up the blank row under its source: the scale's five ticks.
const BUS_FADER_MIN: u16 = 5;

/// The control of bus `bus`.
fn bus_ctl(bus: usize, ctl: BusControl) -> Control {
    Control::Bus { bus, ctl }
}

/// A loopback (`bus` 0, 1) or the output strip ([`OUT_BUS`]) on the page
/// LINK picks. Top to bottom: the source menu, LINK, the level, the faders
/// and meters, and MUTE (a loopback's; one, or one under each channel) or
/// HP and LINE (the output's) on the bottom row.
fn draw_bus_strip(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    meters: &[Meters; 2],
    bus: usize,
    hits: &mut HitMap,
) {
    let t = app.theme();
    let state = app.state();
    let focus = app.focus();
    let out = bus == OUT_BUS;
    let (strip, name, source, link, levels, mutes) = if out {
        let o = &state.output;
        (
            Control::Output(0),
            Name::Output,
            o.source,
            o.link,
            [o.level_l_db, o.level_r_db],
            [false; 2],
        )
    } else {
        let lb = &state.loopbacks[bus];
        (
            Control::Loopback(bus),
            Name::Loopback(bus),
            lb.source,
            lb.link,
            [lb.level_l_db, lb.level_r_db],
            [lb.mute_l, lb.mute_r],
        )
    };
    let focused = focus == strip || matches!(focus, Control::Bus { bus: b, .. } if b == bus);
    hits.push(Hit::Focus(strip), area);
    let sf = StripFocus::of(focused, focus == strip);
    let r = strip_box(buf, area, t, &strip_name(app, name), sf, hits);
    if r.height < 5 {
        return;
    }
    let is = |ctl| focus == bus_ctl(bus, ctl);

    // Top to bottom, as in the mockup: the source menu, a blank row (when
    // the fader keeps its five ticks), LINK, the level, the faders and
    // meters, then MUTE (or HP and LINE) on the bottom row.
    let blank = u16::from(r.height >= 4 + 1 + BUS_FADER_MIN);
    let link_y = r.y + 1 + blank;
    let value_y = link_y + 1;
    let bottom_y = r.bottom() - 1;
    let src_style = if is(BusControl::Source) {
        t.focus().add_modifier(Modifier::REVERSED)
    } else {
        t.active()
    };
    let rects = put_centered(
        buf,
        r,
        r.y,
        &[
            Span::styled(source_name(source), src_style),
            Span::styled(" ▾", t.dim()),
        ],
        hits,
    );
    let src = rects[0].union(rects[1]);
    hits.push(Hit::Control(bus_ctl(bus, BusControl::Source)), src);
    if is(BusControl::Source) {
        hits.pixel(src, PixelWidget::Focus);
    }
    let link_btn = button(
        "LINK",
        BUTTON_W,
        link,
        PillKind::Normal,
        is(BusControl::Link),
        t,
    );
    let rects = put_centered(buf, r, link_y, &[link_btn], hits);
    hits.push(Hit::Control(bus_ctl(bus, BusControl::Link)), rects[0]);

    // MUTE, or HP and LINE, on the bottom row.
    let halves = columns(row(r, bottom_y - r.y), 2);
    if out {
        let o = &state.output;
        let w = BUTTON_W.min(r.width.saturating_sub(2) / 2);
        let rects = put_centered(
            buf,
            r,
            bottom_y,
            &[
                button(
                    "HP",
                    w,
                    o.headphones,
                    PillKind::Normal,
                    is(BusControl::Hp),
                    t,
                ),
                Span::raw("  "),
                button("LINE", w, o.line, PillKind::Normal, is(BusControl::Line), t),
            ],
            hits,
        );
        hits.push(Hit::Control(bus_ctl(bus, BusControl::Hp)), rects[0]);
        hits.push(Hit::Control(bus_ctl(bus, BusControl::Line)), rects[2]);
    } else if link {
        let c = BusControl::Mute(0);
        let mute = button("MUTE", BUTTON_W, mutes[0], PillKind::Danger, is(c), t);
        let rects = put_centered(buf, r, bottom_y, &[mute], hits);
        hits.push(Hit::Control(bus_ctl(bus, c)), rects[0]);
    } else {
        for (ch, h) in halves.iter().enumerate() {
            let c = BusControl::Mute(ch);
            let w = BUTTON_W.min(h.width);
            let mute = button("MUTE", w, mutes[ch], PillKind::Danger, is(c), t);
            let rects = put_centered(buf, *h, h.y, &[mute], hits);
            hits.push(Hit::Control(bus_ctl(bus, c)), rects[0]);
        }
    }

    // The level, then the faders and meters down to the bottom row.
    let ids = [0, 1].map(|c| {
        if out {
            MeterId::Output(c)
        } else {
            MeterId::Loopback(bus, c)
        }
    });
    let cm = ids.map(|id| ChannelMeter::of(app, meters, id));
    let body = Rect::new(r.x, value_y + 1, r.width, bottom_y - (value_y + 1));
    if link {
        put_centered(
            buf,
            r,
            value_y,
            &[level_span(levels[0], mutes[0], t, true)],
            hits,
        );
        let fc = bus_ctl(bus, BusControl::Fader(0));
        let f = stereo_body(buf, body, t, levels[0], BUS_MAX_DB, cm, focus == fc, hits);
        hits.push(Hit::Control(fc), f);
    } else {
        for (ch, h) in columns(row(r, value_y - r.y), 2).into_iter().enumerate() {
            put_centered(
                buf,
                h,
                h.y,
                &[level_span(levels[ch], mutes[ch], t, false)],
                hits,
            );
        }
        let fcs = [0, 1].map(|c| bus_ctl(bus, BusControl::Fader(c)));
        let fs = split_body(
            buf,
            body,
            t,
            levels,
            BUS_MAX_DB,
            cm,
            fcs.map(|fc| focus == fc),
            hits,
        );
        for (fc, f) in fcs.into_iter().zip(fs) {
            hits.push(Hit::Control(fc), f);
        }
    }
    // Both outputs off is allowed (nothing forbids it), with a hint.
    if out && !state.output.headphones && !state.output.line {
        put_centered(
            buf,
            area,
            area.bottom() - 1,
            &[Span::styled(" outputs off ", t.dim())],
            hits,
        );
    }
}

fn draw_output_panel(
    buf: &mut Buffer,
    area: Rect,
    app: &App,
    meters: &[Meters; 2],
    hits: &mut HitMap,
) {
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

    for (k, a) in [mon_area, phone_area].into_iter().enumerate() {
        hits.push(Hit::Focus(Control::Output(k + 1)), a);
    }
    let focused = |k| app.focus() == Control::Output(k);
    draw_bus_strip(buf, out_area, app, meters, OUT_BUS, hits);

    // Monitor Mix: the balance the device reports when its knob turns
    // (`11 01`), shown read-only like the official app: dimmed, and no
    // click target.
    let r = strip_box(
        buf,
        mon_area,
        t,
        "MONITOR MIX",
        StripFocus::of(false, focused(1)),
        hits,
    );
    if r.height >= 6 {
        // The dial, about 0.47 of the box's width (78 % of #34's), and
        // its two text rows, centred.
        let (kc, kr) = knob_size(r.width * 47 / 100, r.height - 3);
        let dial_h = kr;
        let r = centered_rows(r, dial_h + 3);
        let balance = state.readout.monitor_mix;
        let k = Knob {
            value: balance.map(|v| f64::from(v.clamp(0, 100)) / 100.0),
            label: None,
            focused: false,
            dimmed: true,
            max: (kc, kr),
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
    let r = strip_box(
        buf,
        phone_area,
        t,
        "PHONE",
        StripFocus::of(false, focused(2)),
        hits,
    );
    if r.height >= 5 {
        // Two big buttons, three rows tall (the mockup's 9 columns each),
        // under the label.
        let r = centered_rows(r, 5);
        put_centered(buf, r, r.y, &[Span::styled("level", t.dim())], hits);
        let w = 9.min(r.width.saturating_sub(1) / 2);
        let spans = [
            button("+17 dBu", w, state.phone_gain, PillKind::Normal, false, t),
            Span::raw(" "),
            button("0 dBu", w, !state.phone_gain, PillKind::Normal, false, t),
        ];
        let x = r.x + r.width.saturating_sub(2 * w + 1) / 2;
        for (k, on) in [(0, true), (2, false)] {
            let span = &spans[k];
            let bx = x + if k == 0 { 0 } else { w + 1 };
            // The button's face: its background on all three rows (the
            // text form), its label on the middle one.
            for dy in 0..3 {
                let text = if dy == 1 {
                    span.content.to_string()
                } else {
                    " ".repeat(usize::from(w))
                };
                put(buf, r, bx, r.y + 2 + dy, &text, span.style);
            }
            let rect = Rect::new(bx, r.y + 2, w, 3).intersection(r);
            hits.pill(rect, span);
            hits.push(Hit::Phone(on), rect);
        }
    } else if r.height >= 4 {
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
fn modal_box(
    buf: &mut Buffer,
    area: Rect,
    t: Theme,
    title: &str,
    (w, h): (u16, u16),
    hits: &mut HitMap,
) -> Rect {
    let r = centered(area, w, h);
    hits.modal = Some(r);
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
    let inner = modal_box(buf, area, t, title, (w, h), hits);
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
    let inner = modal_box(buf, area, t, " Sample rate ", (30, h), hits);
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

/// A loopback's or the output's source menu: the nine sources under the
/// app's headings (Mixer, Input, Playback), the current one marked, `sel`
/// highlighted.
fn source_menu(buf: &mut Buffer, area: Rect, app: &App, bus: usize, sel: usize, hits: &mut HitMap) {
    let t = app.theme();
    let (name, current) = if bus == OUT_BUS {
        (Name::Output, app.state().output.source)
    } else {
        (
            Name::Loopback(bus),
            app.state().loopbacks[bus.min(1)].source,
        )
    };
    let title = format!(" {} source ", strip_name(app, name));
    let groups = [("Mixer", 0..3), ("Input", 3..6), ("Playback", 6..9)];
    let h = SOURCE_MENU.len() as u16 + groups.len() as u16 + 6;
    let inner = modal_box(buf, area, t, &title, (32, h), hits);
    let mut y = inner.y + 1;
    for (heading, items) in groups {
        put(buf, inner, inner.x + 3, y, heading, t.dim());
        y += 1;
        for i in items {
            let src = SOURCE_MENU[i];
            let mark = if src == current { "●" } else { " " };
            let line = format!(" {mark} {:<10}", menu_label(src));
            let style = if i == sel {
                t.focus().add_modifier(Modifier::REVERSED)
            } else {
                t.text()
            };
            let r = put(buf, inner, inner.x + 5, y, &line, style);
            hits.push(Hit::SourceOption(i), r);
            y += 1;
        }
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
        ("← →", "move: the control beside, or the next strip"),
        ("↑ ↓", "move: up / down a strip, from its name"),
        ("+  =  -  _", "adjust the knob or fader: 1 dB / 5 %"),
        ("PgUp  PgDn", "the same ×10"),
        ("0", "reset the knob or fader (gain −0 dB, fader 0 dB)"),
        ("space  enter", "toggle the switch; open a source's menu"),
        ("enter", "on a strip's name: rename it"),
        ("a  b  c", "show mix A / B / C"),
        ("r", "sample rate list (sets it through PipeWire)"),
        ("s", "save the workspace (on Default: save as…)"),
        ("l", "workspaces: pick, save, delete, new, save as"),
        ("w", "store to the device's memory (download)"),
        ("?", "this help"),
        ("q  Ctrl-C", "quit"),
        ("", ""),
        ("click", "focus a strip; flip a switch; pick a mix; s l w"),
        (
            "wheel",
            "turn a knob, move a fader (Shift ×10); next source",
        ),
        ("drag", "turn a knob, move a fader (up / down)"),
        ("double-click", "reset a knob or fader"),
        ("", ""),
        (
            "",
            "LINK: one fader and switch set, or an L and an R of each",
        ),
        (
            "",
            "loopback, output: 1 dB a step, ×10 is 5 dB, −90 dB is −∞",
        ),
        ("", "PHONE: space or a click sets the level switch"),
    ];
    let h = keys.len() as u16 + 6;
    let inner = modal_box(buf, area, t, " Help ", (80, h), hits);
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

/// A button-like label, highlighted when `on`.
fn button_style(t: Theme, on: bool) -> Style {
    if on {
        t.focus().add_modifier(Modifier::REVERSED)
    } else {
        t.text()
    }
}

/// The OK and Cancel buttons on the second-last row of `inner`.
fn ok_cancel(buf: &mut Buffer, inner: Rect, t: Theme, hits: &mut HitMap) {
    let ok = " [Enter] OK ";
    let cancel = " [Esc] Cancel ";
    let gap = 4;
    let total = (ok.chars().count() + gap + cancel.chars().count()) as u16;
    let y = inner.bottom().saturating_sub(2);
    let x = inner.x + inner.width.saturating_sub(total) / 2;
    let r = put(buf, inner, x, y, ok, button_style(t, true));
    hits.push(Hit::Yes, r);
    let r = put(buf, inner, r.right() + gap as u16, y, cancel, t.text());
    hits.push(Hit::No, r);
}

/// The workspace combo: Default, each workspace with its Save and Delete,
/// "New workspace…", "Save workspace as…" and auto-save; the open one
/// marked, `sel` highlighted.
fn combo(buf: &mut Buffer, area: Rect, app: &App, sel: usize, hits: &mut HitMap) {
    let t = app.theme();
    let items = app.combo_items();
    let h = items.len() as u16 + 7;
    let inner = modal_box(buf, area, t, " Workspaces ", (52, h), hits);
    let name_w = usize::from(inner.width.saturating_sub(26));
    for (i, item) in items.iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        let mark = |open: bool| if open { "●" } else { " " };
        let label = match *item {
            ComboItem::Default => format!(" {} Default", mark(app.workspace().is_none())),
            ComboItem::Named(k) => {
                let name = &app.workspaces()[k];
                let open = app.workspace() == Some(name.as_str());
                format!(" {} {name}", mark(open))
            }
            ComboItem::New => "   New workspace…".to_string(),
            ComboItem::SaveAs => "   Save workspace as…".to_string(),
            ComboItem::AutoSave => {
                let x = if app.auto_save() { "x" } else { " " };
                format!(" [{x}] Auto-save")
            }
        };
        let label = format!("{label:<name_w$}");
        let r = put(
            buf,
            inner,
            inner.x + 2,
            y,
            &label,
            button_style(t, i == sel),
        );
        hits.push(Hit::ComboItem(i), r);
        if let ComboItem::Named(_) = item {
            let r = put(buf, inner, r.right() + 1, y, "[save]", t.dim());
            hits.push(Hit::ComboSave(i), r);
            let r = put(buf, inner, r.right() + 1, y, "[delete]", t.dim());
            hits.push(Hit::ComboDelete(i), r);
        }
    }
    let keys = "enter pick   s save   d delete";
    put_centered(
        buf,
        inner,
        inner.bottom().saturating_sub(3),
        &[Span::styled(keys, t.dim())],
        hits,
    );
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

/// The workspace name being typed, for `then`.
fn name_prompt(buf: &mut Buffer, area: Rect, app: &App, then: NameThen, hits: &mut HitMap) {
    let t = app.theme();
    let title = match then {
        NameThen::New => " New workspace ",
        NameThen::SaveAs => " Save workspace as ",
        NameThen::SaveAsAndStore => " Save as workspace and download ",
    };
    let inner = modal_box(buf, area, t, title, (52, 9), hits);
    put(buf, inner, inner.x + 3, inner.y + 1, "Name:", t.text());
    let field = format!(
        " {}▏{} ",
        app.prompt(),
        " ".repeat(crate::workspace::NAME_MAX - app.prompt().chars().count())
    );
    put(
        buf,
        inner,
        inner.x + 9,
        inner.y + 1,
        &field,
        button_style(t, true),
    );
    put(
        buf,
        inner,
        inner.x + 3,
        inner.y + 3,
        "No / \\ : * ? \" < > |, up to 32 characters",
        t.dim(),
    );
    ok_cancel(buf, inner, t, hits);
}

/// The official app's Download dialog: its three choices, `sel` picked.
fn download(buf: &mut Buffer, area: Rect, t: Theme, sel: usize, hits: &mut HitMap) {
    let w = DOWNLOAD_OPTIONS
        .iter()
        .map(|o| o.chars().count())
        .max()
        .unwrap_or(0) as u16
        + 14;
    let h = DOWNLOAD_OPTIONS.len() as u16 + 8;
    let inner = modal_box(buf, area, t, " Download ", (w, h), hits);
    put(
        buf,
        inner,
        inner.x + 3,
        inner.y + 1,
        "Store the settings in the device's own memory:",
        t.text(),
    );
    for (i, option) in DOWNLOAD_OPTIONS.iter().enumerate() {
        let dot = if i == sel { "(•)" } else { "( )" };
        let line = format!(" {dot} {option} ");
        let y = inner.y + 3 + i as u16;
        let r = put(buf, inner, inner.x + 3, y, &line, button_style(t, i == sel));
        hits.push(Hit::DownloadOption(i), r);
    }
    ok_cancel(buf, inner, t, hits);
}
