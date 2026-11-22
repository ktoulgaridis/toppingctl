//! Static design mock for the toppingctl TUI: fake animated signal, no device.
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph},
    Frame,
};

const BORDER: Color = Color::Rgb(70, 74, 88);
const DIM: Color = Color::Rgb(115, 120, 135);
const TEXT: Color = Color::Rgb(205, 210, 220);
const ACCENT: Color = Color::Rgb(122, 196, 255);
const TRACK: Color = Color::Rgb(45, 48, 58);
const PILL_OFF_BG: Color = Color::Rgb(38, 41, 50);
const WARN: Color = Color::Rgb(236, 110, 96);

/// Green → amber → red, btop style.
fn grad(t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (a, b, u) = if t < 0.6 {
        ((80.0, 200.0, 120.0), (220.0, 200.0, 80.0), t / 0.6)
    } else {
        ((220.0, 200.0, 80.0), (236.0, 96.0, 84.0), (t - 0.6) / 0.4)
    };
    let l = |x: f64, y: f64| (x + (y - x) * u) as u8;
    Color::Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

fn title<'a>(key: &'a str, name: &'a str, extra: Vec<Span<'a>>) -> Line<'a> {
    let mut v = vec![
        Span::styled("┤", Style::new().fg(BORDER)),
        Span::styled(key, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled(name, Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
        Span::styled("├", Style::new().fg(BORDER)),
    ];
    v.extend(extra);
    Line::from(v)
}

fn boxed<'a>(t: Line<'a>) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(t)
}

fn pill(label: &str, on: bool, danger: bool) -> Span<'static> {
    let s = format!(" {label} ");
    if on {
        let bg = if danger { WARN } else { ACCENT };
        Span::styled(s, Style::new().fg(Color::Rgb(20, 22, 28)).bg(bg).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(s, Style::new().fg(DIM).bg(PILL_OFF_BG))
    }
}

/// Horizontal ■ meter: gradient over the full width, dim when empty.
fn hmeter(width: usize, frac: f64, solid: Option<Color>) -> Vec<Span<'static>> {
    let lit = (frac.clamp(0.0, 1.0) * width as f64).round() as usize;
    (0..width)
        .map(|i| {
            let c = if i < lit {
                solid.unwrap_or_else(|| grad(i as f64 / width as f64))
            } else {
                TRACK
            };
            Span::styled("■", Style::new().fg(c))
        })
        .collect()
}

fn db_to_frac(db: f64) -> f64 {
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn fmt_db(db: f64) -> String {
    if db <= -60.0 {
        "  −∞ dB".into()
    } else if db.abs() < 0.05 {
        "  0.0 dB".into()
    } else {
        format!("{:>+5.1} dB", db).replace('-', "−")
    }
}

struct Sig {
    t: f64,
}
impl Sig {
    fn lvl(&self, seed: f64, base: f64) -> f64 {
        let n = (self.t * 2.1 + seed).sin() * 6.0 + (self.t * 7.3 + seed * 3.0).sin() * 3.0
            + (self.t * 13.7 + seed * 5.0).sin() * 1.5;
        (base + n).min(0.0)
    }
}



/// Two-row block wordmark, one glyph pair per letter.
fn wordmark() -> [Vec<Span<'static>>; 2] {
    let glyphs: &[(char, &str, &str)] = &[
        ('t', "▄█▄", " █▄"),
        ('o', "▄▄▄", "█▄█"),
        ('p', "▄▄▄", "█▀▀"),
        ('i', "▄", "█"),
        ('n', "▄▄▄", "█ █"),
        ('g', "▄▄▄", "▀▀█"),
        ('c', "▄▄▄", "█▄▄"),
        ('l', "█ ", "█▄"),
    ];
    let word = "toppingctl";
    let n = word.len() as f64;
    let mut top = vec![Span::raw(" ")];
    let mut bot = vec![Span::raw(" ")];
    for (i, ch) in word.chars().enumerate() {
        let (_, a, b) = glyphs.iter().find(|g| g.0 == ch).unwrap();
        let u = i as f64 / (n - 1.0);
        let c = Color::Rgb((122.0 + (186.0 - 122.0) * u) as u8, (196.0 + (140.0 - 196.0) * u) as u8, 255);
        let st = Style::new().fg(c).add_modifier(Modifier::BOLD);
        top.push(Span::styled(format!("{a} "), st));
        bot.push(Span::styled(format!("{b} "), st));
    }
    [top, bot]
}

fn header(f: &mut Frame, r: Rect, _t: f64) {
    let b = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(BORDER));
    let inner = b.inner(r);
    f.render_widget(b, r);
    let [top, bot] = wordmark();
    let wm_w: u16 = top.iter().map(|s| s.width() as u16).sum();
    let narrow = inner.width < 128;
    let cols = Layout::horizontal([Constraint::Length(wm_w + 1), Constraint::Length(if narrow { 9 } else { 14 }), Constraint::Min(10)]).split(inner);
    f.render_widget(Paragraph::new(vec![Line::from(top), Line::from(bot)]), cols[0]);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("E2x2", Style::new().fg(TEXT).add_modifier(Modifier::BOLD))),
            Line::from(Span::styled("fw 1.13", Style::new().fg(DIM))),
        ]),
        cols[1],
    );
    let sep = || Span::styled("  │  ", Style::new().fg(BORDER));
    let key = |k: &'static str| Span::styled(k, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD));
    let lab = |t: &'static str| Span::styled(t, Style::new().fg(DIM));
    let val = |t: &'static str| Span::styled(t, Style::new().fg(TEXT).add_modifier(Modifier::BOLD));
    let l1 = Line::from(if narrow {
        vec![val("studio-default"), lab(" ▾"), sep(), val("48 kHz"), lab(" ▾ ")]
    } else {
        vec![lab("workspace  "), val("studio-default"), lab(" ▾"), sep(), lab("sample rate  "), val("48000"), lab(" Hz ▾ ")]
    })
    .alignment(Alignment::Right);
    let dot = Span::styled("●", Style::new().fg(Color::Rgb(80, 200, 120)));
    let l2 = Line::from(if narrow {
        vec![key("s"), lab(" save"), sep(), key("w"), lab(" store"), sep(), dot, lab(" synced ")]
    } else {
        vec![key("s"), lab(" save workspace"), sep(), key("l"), lab(" load"), sep(), key("w"), lab(" store to device"), sep(), dot, lab(" synced ")]
    })
    .alignment(Alignment::Right);
    f.render_widget(Paragraph::new(vec![l1, l2]), cols[2]);
}

/// A 3-line rotary knob: `frac` 0..1 sweeps ↙ … ↘ (like a 270° pot).
fn knob_lines(frac: Option<f64>, focused: bool) -> [Line<'static>; 3] {
    let arrows = ["↙", "←", "↖", "↑", "↗", "→", "↘"];
    let a = match frac {
        Some(v) => arrows[(v.clamp(0.0, 1.0) * 6.0).round() as usize],
        None => "·",
    };
    let rim = Style::new().fg(if focused { ACCENT } else { Color::Rgb(95, 100, 115) });
    let ptr = Style::new().fg(if focused { ACCENT } else { TEXT }).add_modifier(Modifier::BOLD);
    [
        Line::from(Span::styled("╭───╮", rim)),
        Line::from(vec![Span::styled("│ ", rim), Span::styled(a, ptr), Span::styled(" │", rim)]),
        Line::from(Span::styled("╰───╯", rim)),
    ]
}

const PARTIAL: [&str; 8] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇"];

/// One vertical meter cell for `row` (0 = top) of `h` rows at level `db`.
fn vcell(row: usize, h: usize, db: f64, width: usize) -> Span<'static> {
    let m = db_to_frac(db) * h as f64;
    let from_bottom = (h - 1 - row) as f64;
    let col = grad(from_bottom / h as f64);
    let g = if m >= from_bottom + 1.0 {
        "█"
    } else if m > from_bottom {
        PARTIAL[((m - from_bottom) * 8.0) as usize % 8]
    } else {
        return Span::styled("▕".repeat(width.min(1)) + &" ".repeat(width.saturating_sub(1)), Style::new().fg(TRACK));
    };
    Span::styled(g.repeat(width), Style::new().fg(col))
}

/// dB scale label for `row` of `h` rows, right-aligned in 3 columns.
fn scale(row: usize, h: usize) -> Span<'static> {
    for (db, lab) in [(0.0, "0"), (-12.0, "-12"), (-24.0, "-24"), (-48.0, "-48")] {
        let r = h - 1 - ((db_to_frac(db) * (h - 1) as f64).round() as usize);
        if r == row {
            return Span::styled(format!("{lab:>3}"), Style::new().fg(Color::Rgb(85, 90, 105)));
        }
    }
    Span::raw("   ")
}

/// Fader column (5 wide): track with a knob at `db` (−∞…+6).
fn fcell(row: usize, h: usize, db: f64, focused: bool) -> Span<'static> {
    let frac = ((db + 60.0) / 66.0).clamp(0.0, 1.0);
    let knob = h - 1 - ((frac * (h - 1) as f64).round() as usize);
    if row == knob {
        Span::styled("━━●━━", Style::new().fg(if focused { ACCENT } else { TEXT }).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("  ┃  ", Style::new().fg(TRACK))
    }
}

fn stack(items: &[(&str, bool, bool)]) -> Vec<Line<'static>> {
    items.iter().map(|(l, on, d)| Line::from(pill(&format!("{:^4}", l), *on, *d)).alignment(Alignment::Center)).collect()
}

fn sub_title(name: &str, focused: bool) -> Paragraph<'static> {
    let st = if focused {
        Style::new().fg(Color::Rgb(20, 22, 28)).bg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT).bg(PILL_OFF_BG).add_modifier(Modifier::BOLD)
    };
    Paragraph::new(Span::styled(format!(" {name} "), st)).alignment(Alignment::Center)
}



fn db_line(db: f64, mute: bool) -> Paragraph<'static> {
    let (t, st) = if mute { ("muted".to_string(), Style::new().fg(WARN)) } else { (fmt_db(db).trim().to_string(), Style::new().fg(TEXT)) };
    Paragraph::new(Span::styled(t, st)).alignment(Alignment::Center)
}

/// Stereo strip: L meter · fader · R meter, centred in `r`.
fn stereo_body(f: &mut Frame, r: Rect, fader_db: f64, l: f64, rr: f64, focused: bool, mute: bool) {
    let h = r.height as usize;
    let (l, rr) = if mute { (-90.0, -90.0) } else { (l, rr) };
    let lines: Vec<Line> = (0..h)
        .map(|row| Line::from(vec![vcell(row, h, l, 1), Span::raw(" "), scale(row, h), fcell(row, h, fader_db, focused), scale(row, h), Span::raw(" "), vcell(row, h, rr, 1)]))
        .collect();
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), r);
}

fn input_strip(f: &mut Frame, r: Rect, s: &Sig, i: usize) {
    let (name, gain, mon, p48, inst, mute, solo) = [("IN1", 12.0, true, false, false, false, false), ("IN2", 0.0, false, true, true, false, false)][i];
    let focused = i == 0;
    let b = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(if focused { ACCENT } else { TRACK }));
    let inner = b.inner(r);
    f.render_widget(b, r);
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(4), Constraint::Length(3), Constraint::Length(1)]).split(inner);
    f.render_widget(sub_title(name, focused), rows[0]);
    let lvl = s.lvl(i as f64 * 1.7, if i == 0 { -16.0 } else { -34.0 });
    let peak = (lvl + 7.0).min(0.0);
    let pc = grad(db_to_frac(peak));
    f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), Span::styled(format!("{:>4.0}", peak).replace('-', "−"), Style::new().fg(pc).add_modifier(Modifier::BOLD)), Span::styled(" pk", Style::new().fg(TRACK))])), rows[1]);
    let rows = [rows[0], rows[2], rows[3], rows[4]];
    let cols = Layout::horizontal([Constraint::Length(7), Constraint::Min(6)]).split(rows[1]);
    let h = cols[0].height as usize;
    let meter: Vec<Line> = (0..h).map(|row| Line::from(vec![Span::raw(" "), vcell(row, h, lvl, 2), scale(row, h)])).collect();
    f.render_widget(Paragraph::new(meter), cols[0]);
    let btn = |l: &str, on: bool, d: bool| Line::from(vec![Span::raw(" "), pill(&format!("{l:^4}"), on, d)]);
    let mut bl = vec![btn("MON", mon, false), btn("48V", p48, true), btn("INST", inst, false), Line::raw(""), btn("SOLO", solo, false), btn("MUTE", mute, true), btn("Ø", false, false)];
    bl.truncate(cols[1].height as usize);
    f.render_widget(Paragraph::new(bl), cols[1]);
    f.render_widget(Paragraph::new(knob_lines(Some(gain / 20.0), focused).to_vec()).alignment(Alignment::Center), rows[2]);
    f.render_widget(Paragraph::new(Span::styled(format!("{:+.0} dB", gain), Style::new().fg(TEXT).add_modifier(Modifier::BOLD))).alignment(Alignment::Center), rows[3]);
}

fn input_panel(f: &mut Frame, r: Rect, s: &Sig) {
    let b = boxed(title("¹", "input", vec![]));
    let inner = b.inner(r);
    f.render_widget(b, r);
    let cols = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(inner);
    input_strip(f, cols[0], s, 0);
    input_strip(f, cols[1], s, 1);
}

fn mix_strip(f: &mut Frame, r: Rect, s: &Sig, idx: usize) {
    let focused = idx == 1;
    let b = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(if focused { ACCENT } else { TRACK }));
    let inner = b.inner(r);
    f.render_widget(b, r);
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(3), Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(3)]).split(inner);
    let name = ["IN 1+2", "PB 1/2", "PB 3/4", "PB 5/6"][idx];
    let (pan1, pan2, linked) = [(0.3, 0.7, false), (0.0, 1.0, true), (0.2, 0.8, true), (0.0, 1.0, true)][idx];
    let ch = [["1", "2"], ["1", "2"], ["3", "4"], ["5", "6"]][idx];
    let [a0, a1, a2] = knob_lines(Some(pan1), false);
    let [b0, b1, b2] = knob_lines(Some(pan2), false);
    let join = |x: Line<'static>, y: Line<'static>| {
        let mut v = x.spans;
        v.push(Span::raw("  "));
        v.extend(y.spans);
        Line::from(v)
    };
    f.render_widget(Paragraph::new(vec![join(a0, b0), join(a1, b1), join(a2, b2)]).alignment(Alignment::Center), rows[1]);
    let pl = |p: f64| if (p - 0.5).abs() < 0.01 { "C".to_string() } else if p < 0.5 { format!("L{:.0}", (0.5 - p) * 200.0) } else { format!("R{:.0}", (p - 0.5) * 200.0) };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{} {:<4}", ch[0], pl(pan1)), Style::new().fg(DIM)),
            Span::raw("  "),
            Span::styled(format!("{} {:<4}", ch[1], pl(pan2)), Style::new().fg(DIM)),
        ]))
        .alignment(Alignment::Center),
        rows[2],
    );
    f.render_widget(Paragraph::new(pill("LINK", linked, false)).alignment(Alignment::Center), rows[3]);
    let rows = [rows[0], rows[4], rows[5], rows[4], rows[6]];
    f.render_widget(sub_title(name, focused), rows[0]);
    let pills = |m: bool| Paragraph::new(Line::from(vec![pill("Ø", false, false), Span::raw(" "), pill("S", false, false), Span::raw(" "), pill("M", m, true)])).alignment(Alignment::Center);
    if idx == 0 {
        // Two mono channels side by side.
        let halves = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(rows[2]);
        let dbs = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(rows[3]);
        let bts = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(rows[4]);
        for (k, (fd, pan, lvl, mute)) in [(-3.0, 0.3, s.lvl(0.0, -16.0), false), (0.0, 0.7, s.lvl(1.7, -34.0), true)].into_iter().enumerate() {
            let h = halves[k].height as usize;
            let lines: Vec<Line> = (0..h).map(|row| Line::from(vec![fcell(row, h, fd, false), Span::raw(" "), vcell(row, h, if mute { -90.0 } else { lvl }, 1)])).collect();
            f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), halves[k]);
            let _ = pan;
            f.render_widget(db_line(fd, mute), dbs[k]);
            f.render_widget(Paragraph::new(stack(&[("Ø", false, false), ("SOLO", false, false), ("MUTE", mute, true)])), bts[k]);
        }
        return;
    }
    let (fd, pan, base) = [(0.0, 0.0, 0.0), (0.0, 0.5, -8.0), (-12.0, 0.35, -22.0), (-70.0, 0.5, -90.0)][idx];
    let _ = pan;
    stereo_body(f, rows[2], fd, s.lvl(idx as f64, base), s.lvl(idx as f64 + 0.4, base - 1.5), focused, false);
    f.render_widget(db_line(fd, false), rows[3]);
    let _ = pills;
    f.render_widget(Paragraph::new(stack(&[("Ø", false, false), ("SOLO", idx == 2, false), ("MUTE", false, true)])), rows[4]);
}


fn mixer_panel(f: &mut Frame, r: Rect, s: &Sig) {
    let tab = |k: &'static str, n: &'static str, on: bool| -> Vec<Span<'static>> {
        if on {
            vec![Span::styled(k, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)), Span::styled(n, Style::new().fg(Color::Rgb(20, 22, 28)).bg(ACCENT).add_modifier(Modifier::BOLD))]
        } else {
            vec![Span::styled(k, Style::new().fg(ACCENT)), Span::styled(n, Style::new().fg(DIM))]
        }
    };
    let mut extra = vec![Span::styled("─┤", Style::new().fg(BORDER))];
    extra.extend(tab("a", " mix A ", true));
    extra.push(Span::raw(" "));
    extra.extend(tab("b", " mix B ", false));
    extra.push(Span::raw(" "));
    extra.extend(tab("c", " mix C ", false));
    extra.push(Span::styled("├", Style::new().fg(BORDER)));
    let b = boxed(title("²", "mixer", extra));
    let inner = b.inner(r);
    f.render_widget(b, r);
    let cols = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(inner);
    for (i, c) in cols.iter().enumerate() {
        mix_strip(f, *c, s, i);
    }
}

fn loop_strip(f: &mut Frame, r: Rect, s: &Sig, i: usize) {
    let (name, src, db, base) = [("LOOP 1+2", "PB 3/4", 0.0, -22.0), ("LOOP 3+4", "PB 5/6", -6.0, -90.0)][i];
    let b = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(TRACK));
    let inner = b.inner(r);
    f.render_widget(b, r);
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).split(inner);
    f.render_widget(sub_title(name, false), rows[0]);
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled(src, Style::new().fg(ACCENT)), Span::styled(" ▾", Style::new().fg(DIM))])).alignment(Alignment::Center), rows[1]);
    f.render_widget(Paragraph::new(pill("LINK", true, false)).alignment(Alignment::Center), rows[2]);
    f.render_widget(db_line(db, false), rows[3]);
    stereo_body(f, rows[4], db, s.lvl(2.0 + i as f64, base), s.lvl(2.3 + i as f64, base - 1.0), false, false);
    f.render_widget(Paragraph::new(pill("MUTE", false, true)).alignment(Alignment::Center), rows[5]);
}

fn loopback_panel(f: &mut Frame, r: Rect, s: &Sig) {
    let b = boxed(title("³", "loopback", vec![]));
    let inner = b.inner(r);
    f.render_widget(b, r);
    let cols = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(inner);
    loop_strip(f, cols[0], s, 0);
    loop_strip(f, cols[1], s, 1);
}

fn output_panel(f: &mut Frame, r: Rect, s: &Sig) {
    let b = boxed(title("⁴", "output", vec![]));
    let inner = b.inner(r);
    f.render_widget(b, r);
    let cols3 = Layout::horizontal([Constraint::Ratio(2, 5), Constraint::Ratio(3, 10), Constraint::Ratio(3, 10)]).split(inner);
    let cols = [cols3[0], cols3[2]];
    let mb = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(TRACK));
    let mi = mb.inner(cols3[1]);
    f.render_widget(mb, cols3[1]);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(" MONITOR MIX ", Style::new().fg(TEXT).bg(PILL_OFF_BG).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
            Line::raw(""),
            knob_lines(None, false)[0].clone().alignment(Alignment::Center),
            knob_lines(None, false)[1].clone().alignment(Alignment::Center),
            knob_lines(None, false)[2].clone().alignment(Alignment::Center),
            Line::from(vec![Span::styled("input", Style::new().fg(DIM)), Span::raw("     "), Span::styled("playback", Style::new().fg(DIM))]).alignment(Alignment::Center),
            Line::raw(""),
            Line::from(Span::styled("knob on device only", Style::new().fg(TRACK))).alignment(Alignment::Center),
        ]),
        mi,
    );
    // Output 1+2 strip.
    let sb = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(TRACK));
    let si = sb.inner(cols[0]);
    f.render_widget(sb, cols[0]);
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).split(si);
    f.render_widget(sub_title("OUT 1+2", false), rows[0]);
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled("mix A", Style::new().fg(ACCENT)), Span::styled(" ▾", Style::new().fg(DIM))])).alignment(Alignment::Center), rows[1]);
    f.render_widget(Paragraph::new(pill("LINK", true, false)).alignment(Alignment::Center), rows[2]);
    f.render_widget(db_line(0.0, false), rows[3]);
    stereo_body(f, rows[4], 0.0, s.lvl(0.3, -9.0), s.lvl(0.9, -10.5), false, false);
    f.render_widget(Paragraph::new(Line::from(vec![pill("HP", true, false), Span::raw(" "), pill("LINE", true, false)])).alignment(Alignment::Center), rows[5]);
    // Phone out.
    let pb = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(TRACK));
    let pi = pb.inner(cols[1]);
    f.render_widget(pb, cols[1]);
    let w = pi.width.saturating_sub(4) as usize;
    let mut vol = vec![Span::raw("  ")];
    vol.extend(hmeter(w, 0.73, Some(TEXT)));
    let lines = vec![
        Line::from(Span::styled(" PHONE ", Style::new().fg(TEXT).bg(PILL_OFF_BG).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
        Line::raw(""),
        Line::from(Span::styled("level", Style::new().fg(DIM))).alignment(Alignment::Center),
        Line::from(vol),
        Line::from(Span::styled("0 dBu", Style::new().fg(TEXT).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
        Line::raw(""),
        Line::from(pill(" GAIN ", false, false)).alignment(Alignment::Center),
        Line::raw(""),
        Line::from(vec![Span::styled("volume knob ", Style::new().fg(TRACK)), Span::styled("73", Style::new().fg(DIM))]).alignment(Alignment::Center),
    ];
    f.render_widget(Paragraph::new(lines), pi);
}

fn footer(f: &mut Frame, r: Rect) {
    let k = |key: &'static str, what: &'static str| vec![Span::styled(key, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)), Span::styled(format!(" {what}   "), Style::new().fg(DIM))];
    let mut v = vec![Span::raw(" ")];
    for (a, b) in [("tab", "focus"), ("←→", "adjust"), ("⇧", "×10"), ("space", "toggle"), ("a b c", "mix"), ("s", "save"), ("w", "store"), ("r", "rate"), ("?", "help"), ("q", "quit")] {
        v.extend(k(a, b));
    }
    f.render_widget(Paragraph::new(Line::from(v)), r);
}

fn draw(f: &mut Frame, t: f64) {
    let s = Sig { t };
    if f.area().width < 100 || f.area().height < 34 {
        let msg = Paragraph::new(vec![
            Line::from(Span::styled("toppingctl", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))),
            Line::from(Span::styled(format!("terminal is {}×{}, needs at least 100×34", f.area().width, f.area().height), Style::new().fg(DIM))),
        ])
        .alignment(Alignment::Center);
        let a = f.area();
        f.render_widget(msg, Rect { y: a.height / 2, height: 2.min(a.height), ..a });
        return;
    }
    let rows = Layout::vertical([Constraint::Length(4), Constraint::Ratio(1, 2), Constraint::Ratio(1, 2), Constraint::Length(1)]).split(f.area());
    header(f, rows[0], t);
    let top = Layout::horizontal([Constraint::Percentage(36), Constraint::Percentage(64)]).split(rows[1]);
    let bot = Layout::horizontal([Constraint::Percentage(36), Constraint::Percentage(64)]).split(rows[2]);
    input_panel(f, top[0], &s);
    mixer_panel(f, top[1], &s);
    loopback_panel(f, bot[0], &s);
    output_panel(f, bot[1], &s);
    footer(f, rows[3]);
}

fn main() -> std::io::Result<()> {
    if let Ok(dim) = std::env::var("MOCK_DUMP") {
        let (w, h) = dim.split_once('x').map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap())).unwrap();
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, 1.3)).unwrap();
        let b = t.backend().buffer();
        for y in 0..h { let l: String = (0..w).map(|x| b[(x, y)].symbol().to_string()).collect(); println!("{l}"); }
        return Ok(());
    }
    let mut term = ratatui::init();
    let start = Instant::now();
    let res = loop {
        let t = start.elapsed().as_secs_f64();
        if let Err(e) = term.draw(|f| draw(f, t)) {
            break Err(e);
        }
        if event::poll(Duration::from_millis(33))? {
            if let Event::Key(k) = event::read()? {
                if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
                    break Ok(());
                }
            }
        }
    };
    ratatui::restore();
    res
}
