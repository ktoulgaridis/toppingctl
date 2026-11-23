//! The TUI's shared widgets (#34): dial, fader, meter, pill. Rendered into a
//! plain `Buffer`; no terminal, no hardware.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::widgets::Widget;

use toppingctl::tui::theme::Theme;
use toppingctl::tui::widgets::dial::{knob_rect, Dial};
use toppingctl::tui::widgets::fader::{db_at_frac, fader_frac, fmt_db, level_text, Fader};
use toppingctl::tui::widgets::meter::{
    peak_text, ClipHold, Meter, Scale, CLIP_HOLD, INPUT_SCALE_DB, PEAK_HOT_DB,
};
use toppingctl::tui::widgets::pill::{button, pill, PillKind};

fn render(w: impl Widget, width: u16, height: u16) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
    w.render(buf.area, &mut buf);
    buf
}

fn rows(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

// ------------------------------------------------------------------- dial

/// The lit Braille dots of a render, as (x, y) in dot units (y down) with
/// their cell's colour.
fn dots(buf: &Buffer) -> Vec<(f64, f64, Color)> {
    // Unicode Braille: bit n of (ch - U+2800) is this dot (column, row).
    const BITS: [(usize, usize); 8] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    let mut out = Vec::new();
    for cy in 0..buf.area.height {
        for cx in 0..buf.area.width {
            let cell = &buf[(cx, cy)];
            let Some(ch) = cell.symbol().chars().next() else {
                continue;
            };
            let code = ch as u32;
            if !(0x2800..=0x28ff).contains(&code) {
                continue;
            }
            let bits = code - 0x2800;
            for (i, (dx, dy)) in BITS.iter().enumerate() {
                if bits & (1 << i) != 0 {
                    let x = f64::from(cx) * 2.0 + *dx as f64;
                    let y = f64::from(cy) * 4.0 + *dy as f64;
                    out.push((x, y, cell.fg));
                }
            }
        }
    }
    out
}

/// The mean angle (degrees, 0 = right, 90 = up) of the dots drawn in
/// `colour`, around the centre of a `w`×`h`-cell knob at the origin.
fn mean_angle(buf: &Buffer, w: u16, h: u16, colour: Color) -> Option<f64> {
    let (cx, cy) = (
        (f64::from(w) * 2.0 - 1.0) / 2.0,
        (f64::from(h) * 4.0 - 1.0) / 2.0,
    );
    let pts: Vec<(f64, f64)> = dots(buf)
        .into_iter()
        .filter(|d| d.2 == colour)
        .map(|(x, y, _)| (x - cx, cy - y))
        .collect();
    if pts.is_empty() {
        return None;
    }
    let n = pts.len() as f64;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    Some((sy / n).atan2(sx / n).to_degrees().rem_euclid(360.0))
}

fn count(buf: &Buffer, colour: Color) -> usize {
    buf.content()
        .iter()
        .filter(|c| c.fg == colour && c.symbol() != " ")
        .count()
}

fn braille(value: Option<f64>, dimmed: bool, w: u16, h: u16) -> Buffer {
    let d = Dial::new(Theme::color()).value(value).dimmed(dimmed);
    render(d, w, h)
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d) <= tol
}

/// The Braille knob (prototype style G): a thick ring lit in the accent up
/// to the value, a rim, and a pointer line from the centre. Min points
/// down-left, centre up, max down-right.
#[test]
fn braille_knob_min_centre_max() {
    for (w, h) in [(6, 3), (8, 4), (12, 6)] {
        let min = braille(Some(0.0), false, w, h);
        let mid = braille(Some(0.5), false, w, h);
        let max = braille(Some(1.0), false, w, h);
        for (b, want, what) in [
            (&min, 225.0, "min"),
            (&mid, 90.0, "centre"),
            (&max, 315.0, "max"),
        ] {
            let a = mean_angle(b, w, h, Theme::KNOB_POINTER)
                .unwrap_or_else(|| panic!("{w}x{h} {what}: no pointer"));
            assert!(close(a, want, 30.0), "{w}x{h} {what} points at {a}°");
        }
        let lit = |b: &Buffer| count(b, Theme::KNOB_ACCENT);
        assert!(
            lit(&min) < lit(&mid) && lit(&mid) < lit(&max),
            "{w}x{h}: the arc grows"
        );
        assert!(
            count(&min, Theme::KNOB_DIM) > 0,
            "{w}x{h}: the unlit arc is drawn"
        );
        assert!(
            count(&mid, Theme::KNOB_RIM) > 0,
            "{w}x{h}: the rim is drawn"
        );
        // The lit arc runs from min: at 0.5 its dots lean to the left.
        let a = mean_angle(&mid, w, h, Theme::KNOB_ACCENT).unwrap();
        assert!(close(a, 135.0, 50.0), "{w}x{h}: lit arc centre at {a}°");
    }
}

/// Monitor Mix (style L): dimmed, nothing lit, still a knob; the pointer
/// shows the balance the device reports (none while unknown).
#[test]
fn braille_knob_dimmed() {
    for v in [None, Some(0.5)] {
        let b = braille(v, true, 12, 6);
        assert_eq!(count(&b, Theme::KNOB_ACCENT), 0, "nothing lit");
        assert!(count(&b, Theme::KNOB_DIM) > 6, "the ring is drawn");
    }
    let b = braille(None, true, 12, 6);
    assert_eq!(count(&b, Theme::KNOB_POINTER), 0, "unknown: no pointer");
    let b = braille(Some(0.5), true, 12, 6);
    let a = mean_angle(&b, 12, 6, Theme::KNOB_POINTER).expect("a pointer");
    assert!(close(a, 90.0, 30.0), "centre points up: {a}°");
    // An unknown value without dimming draws no pointer either.
    let b = braille(None, false, 12, 6);
    assert_eq!(count(&b, Theme::KNOB_POINTER), 0);
}

/// The knob is centred in its area at most `max_cells`, with the value
/// under it; under NO_COLOR nothing is coloured.
#[test]
fn braille_knob_size_and_value() {
    let d = Dial::new(Theme::color())
        .value(Some(0.6))
        .max_cells(8, 4)
        .label(Some("+12 dB".into()));
    let buf = render(d, 14, 7);
    let r = rows(&buf);
    assert_eq!(r[6].trim(), "+12 dB");
    // Drawn only in the 8×4 knob rect, centred: columns 3..11, rows 1..5.
    let k = knob_rect(Rect::new(0, 0, 14, 7), true, (8, 4));
    assert_eq!(k, Rect::new(3, 1, 8, 4));
    for (y, l) in r.iter().enumerate().take(6) {
        for (x, ch) in l.chars().enumerate() {
            if ch != ' ' {
                assert!(k.contains((x as u16, y as u16).into()), "{x},{y} {ch:?}");
            }
        }
    }
    // A small area: the knob shrinks to fit, keeping its round shape.
    assert_eq!(
        knob_rect(Rect::new(0, 0, 9, 6), true, (12, 6)),
        Rect::new(0, 0, 8, 4)
    );
    assert_eq!(
        knob_rect(Rect::new(0, 0, 20, 4), false, (12, 6)),
        Rect::new(6, 0, 8, 4)
    );
    assert_eq!(
        knob_rect(Rect::new(0, 0, 20, 9), true, (12, 6)),
        Rect::new(4, 1, 12, 6)
    );

    let d = Dial::new(Theme::plain())
        .value(Some(0.6))
        .label(Some("L40".into()));
    let buf = render(d, 8, 5);
    assert!(buf
        .content()
        .iter()
        .all(|c| c.fg == Color::Reset && c.bg == Color::Reset));
    assert!(dots(&buf).len() > 10, "the knob is drawn without colour");
    assert_eq!(rows(&buf)[4].trim(), "L40");
}

// ------------------------------------------------------------------ fader

/// Ticks at 0 / −6 / −12 / −24 / −∞ on both sides; the cap sits on the
/// value's row.
#[test]
fn fader_ticks_and_cap() {
    // A mix fader tops out at +6 dB: six positions, +6 unlabelled.
    let buf = render(Fader::new(-12.0, 6.0, Theme::color()), 11, 6);
    let r = rows(&buf);
    let expected = [
        "     ┃     ",
        "  0  ┃  0  ",
        " −6  ┃  −6 ",
        "−12━━■━━−12",
        "−24  ┃  −24",
        " −∞  ┃  −∞ ",
    ];
    assert_eq!(r, expected);

    // 0 dB sits on the 0 tick; −∞ at the bottom.
    let r = rows(&render(Fader::new(0.0, 6.0, Theme::color()), 11, 6));
    assert_eq!(r[1], "  0━━■━━0  ");
    let r = rows(&render(
        Fader::new(f64::NEG_INFINITY, 6.0, Theme::color()),
        11,
        6,
    ));
    assert_eq!(r[5], " −∞━━■━━−∞ ");

    // Loopback and output faders top out at 0 dB, labelled −0 as in the
    // app (#15).
    let r = rows(&render(Fader::new(-6.0, 0.0, Theme::color()), 11, 5));
    assert_eq!(
        r,
        [
            " −0  ┃  −0 ",
            " −6━━■━━−6 ",
            "−12  ┃  −12",
            "−24  ┃  −24",
            " −∞  ┃  −∞ ",
        ]
    );

    // Narrower: a 3-wide cap.
    let r = rows(&render(Fader::new(-6.0, 0.0, Theme::color()), 9, 5));
    assert_eq!(r[1], " −6━■━−6 ");

    // Without labels: tick marks on both sides of the track.
    let r = rows(&render(
        Fader::new(-6.0, 0.0, Theme::color()).labels(false),
        3,
        5,
    ));
    assert_eq!(r, ["╶┃╴", "━■━", "╶┃╴", "╶┃╴", "╶┃╴"]);

    // Taller: the ticks spread out, the cap follows.
    let r = rows(&render(Fader::new(-24.0, 0.0, Theme::color()), 11, 9));
    let at = |label: &str| r.iter().position(|l| l.trim_start().starts_with(label));
    assert_eq!(at("−0"), Some(0));
    assert_eq!(at("−∞"), Some(8));
    assert!(r[6].contains('■'), "{r:?}");
}

#[test]
fn fader_scale_and_value_text() {
    assert_eq!(fader_frac(6.0, 6.0), 1.0);
    assert_eq!(fader_frac(0.0, 0.0), 1.0);
    assert_eq!(fader_frac(f64::NEG_INFINITY, 6.0), 0.0);
    assert_eq!(fader_frac(-90.0, 0.0), 0.0);
    assert!(fader_frac(-3.0, 0.0) > fader_frac(-6.0, 0.0));
    assert!(fader_frac(-40.0, 0.0) > 0.0 && fader_frac(-40.0, 0.0) < fader_frac(-24.0, 0.0));
    assert_eq!(fader_frac(9.0, 6.0), 1.0, "clamped");
    // A loopback or output fader runs down to the app's −90 (#15); a mix
    // fader to −60.
    assert!(fader_frac(-75.0, 0.0) > 0.0);
    assert_eq!(fader_frac(-75.0, 6.0), 0.0);
    assert_eq!(db_at_frac(0.0, 0.0), f64::NEG_INFINITY);
    assert!(db_at_frac(0.05, 0.0) < -60.0 && db_at_frac(0.05, 0.0) > -90.0);
    assert_eq!(level_text(-6.0), "−6");
    assert_eq!(level_text(0.0), "−0");
    assert_eq!(level_text(-89.0), "−89");
    assert_eq!(level_text(f64::NEG_INFINITY), "−∞");

    // 0 dB reads −0, as in the official app.
    assert_eq!(fmt_db(0.0), "−0.0 dB");
    assert_eq!(fmt_db(-0.0), "−0.0 dB");
    assert_eq!(fmt_db(-0.04), "−0.0 dB");
    assert_eq!(fmt_db(-6.0), "−6.0 dB");
    assert_eq!(fmt_db(-12.0), "−12.0 dB");
    assert_eq!(fmt_db(3.0), "+3.0 dB");
    assert_eq!(fmt_db(f64::NEG_INFINITY), "−∞ dB");
}

// ------------------------------------------------------------------ meter

fn is_reddish(c: Color) -> bool {
    matches!(c, Color::Rgb(r, g, _) if r > g)
}

/// The clip light stays on 5 s after the last clip, as the app's
/// `overload_IN` / `overload_OUT` lights (#15).
#[test]
fn clip_hold_with_fake_clock() {
    assert_eq!(CLIP_HOLD, Duration::from_secs(5));
    let t0 = Instant::now();
    let ms = |n| t0 + Duration::from_millis(n);
    let mut hold = ClipHold::default();
    assert!(!hold.lit(t0));
    hold.update(-20.0, t0);
    assert!(!hold.lit(t0), "no clip, no light");

    hold.update(0.0, ms(100));
    assert!(hold.lit(ms(100)));
    hold.update(-30.0, ms(600));
    assert!(hold.lit(ms(5000)), "held after the signal drops");
    assert!(!hold.lit(ms(5101)), "off 5 s after the clip");

    // A new clip restarts the hold.
    hold.update(0.0, ms(800));
    assert!(hold.lit(ms(5700)));
    assert!(!hold.lit(ms(5801)));
}

/// The input meter's scale: 0 … −60 in the steps the app shows.
#[test]
fn input_scale() {
    assert_eq!(
        INPUT_SCALE_DB,
        [0.0, -6.0, -12.0, -24.0, -36.0, -48.0, -60.0]
    );
    let r = rows(&render(Scale::new(&INPUT_SCALE_DB, Theme::color()), 3, 13));
    let labels: Vec<&str> = r
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(labels, ["0", "−6", "−12", "−24", "−36", "−48", "−60"]);
    assert_eq!(r[0], "  0");
    assert_eq!(r[12], "−60");
}

/// The peak readout: whole dB, red near 0.
#[test]
fn peak_readout() {
    assert_eq!(peak_text(-18.4), "−18 pk");
    assert_eq!(peak_text(-4.0), " −4 pk");
    assert_eq!(peak_text(-96.0), " −∞ pk");
    assert_eq!(peak_text(0.0), "  0 pk");
    assert_eq!(peak_text(-0.3), "  0 pk", "no −0");
    let t = Theme::color();
    let style = |db| t.peak(db);
    assert!(is_reddish(style(-1.0).fg.unwrap()));
    assert!(is_reddish(style(PEAK_HOT_DB).fg.unwrap()));
    assert!(!is_reddish(style(-18.0).fg.unwrap()));
}

// ------------------------------------------------------------------ pills

#[test]
fn pills() {
    let t = Theme::color();
    let on = |label, kind| pill(label, true, kind, false, t);
    assert_eq!(on("48V", PillKind::Danger).content, " 48V ");
    assert_eq!(on("48V", PillKind::Danger).style.bg, Some(Theme::WARN));
    assert_eq!(on("MUTE", PillKind::Danger).style.bg, Some(Theme::WARN));
    assert_eq!(on("HP", PillKind::Normal).style.bg, Some(Theme::ACCENT));
    assert_eq!(on("LINE", PillKind::Normal).style.bg, Some(Theme::ACCENT));
    let off = pill("MON", false, PillKind::Normal, false, t);
    assert_eq!(off.style.bg, Some(Theme::PILL_OFF_BG));
    // Disabled (IN 1+2 LINK off): a pill, its text greyed out.
    let dis = pill("LINK", false, PillKind::Disabled, false, t);
    assert_eq!(dis.style.fg, Some(Theme::SCALE));
    assert_eq!(dis.style.bg, Some(Theme::PILL_OFF_BG));
    assert_eq!(dis.content, " LINK ");
    // Focus is bold and underlined, with or without colour.
    let f = pill("SOLO", false, PillKind::Normal, true, t);
    assert!(f
        .style
        .add_modifier
        .contains(Modifier::BOLD | Modifier::UNDERLINED));

    // NO_COLOR: no colours; on is reversed.
    let p = Theme::plain();
    let on = pill("48V", true, PillKind::Danger, false, p);
    assert_eq!((on.style.fg, on.style.bg), (None, None));
    assert!(on.style.add_modifier.contains(Modifier::REVERSED));
    let off = pill("48V", false, PillKind::Danger, false, p);
    assert!(!off.style.add_modifier.contains(Modifier::REVERSED));
}

// ------------------------------------------- flat pixel widgets (#41)
//
// The rules of docs/design/mockup-flat.py, at its 12×24 cell.

use toppingctl::tui::pixel::{
    button_image, chip_image, clip_image, fader_image, knob_image, meter_image, PX_ACCENT,
    PX_AMBER, PX_ARC_OFF, PX_BG, PX_CHIP, PX_CHIP_EDGE, PX_CLIP_OFF, PX_DIM, PX_FADER_CAP,
    PX_FAINT, PX_GREEN, PX_KNOB_BODY, PX_KNOB_BODY_DIM, PX_POINTER, PX_RED,
};
use toppingctl::tui::widgets::fader::fader_row;

const CELL: (u16, u16) = (12, 24);

fn px(img: &toppingctl::tui::pixel::RgbaImage, x: f64, y: f64) -> [u8; 4] {
    img.get_pixel(x as u32, y as u32).0
}

/// Opaque and within `tol` of `c` on every channel.
fn is(p: [u8; 4], c: (u8, u8, u8), tol: i32) -> bool {
    p[3] > 250
        && (i32::from(p[0]) - i32::from(c.0)).abs() <= tol
        && (i32::from(p[1]) - i32::from(c.1)).abs() <= tol
        && (i32::from(p[2]) - i32::from(c.2)).abs() <= tol
}

/// `c` at 16 % on the background: an unlit LED.
fn tint(c: (u8, u8, u8)) -> (u8, u8, u8) {
    let m = |b: u8, c: u8| (f64::from(b) + (f64::from(c) - f64::from(b)) * 0.16) as u8;
    (m(PX_BG.0, c.0), m(PX_BG.1, c.1), m(PX_BG.2, c.2))
}

/// The meter is an LED ladder: square 5 px segments with 2 px gaps from
/// the bottom up (at the 24 px cell), 10 px wide in every meter. Lit by
/// height: red at the top 8 %, amber to 30 %, green below; unlit segments
/// are their colour at 16 % on the background.
#[test]
fn pixel_meter_led_segments() {
    // 10 rows of 24 px: the bar runs from 12 (0 dB) to 228 (−60).
    let full = meter_image(1, 10, CELL, 0.0);
    assert_eq!((full.width(), full.height()), (12, 240));
    // Segment k covers [223 − 7k, 228 − 7k): 5 px lit, then a 2 px gap.
    for k in [0u32, 1, 15, 30] {
        let y0 = 223 - 7 * k;
        for y in y0..y0 + 5 {
            assert_eq!(full.get_pixel(5, y).0[3], 255, "segment {k} at {y}");
        }
        if k < 30 {
            for y in y0 - 2..y0 {
                assert_eq!(full.get_pixel(5, y).0[3], 0, "gap above {k} at {y}");
            }
        }
        // Square ends: the segment's corner pixels are fully solid.
        for (x, y) in [(1, y0), (10, y0), (1, y0 + 4), (10, y0 + 4)] {
            assert_eq!(full.get_pixel(x, y).0[3], 255, "square corner {k} {x},{y}");
        }
        // 10 px wide, centred in the cell.
        assert_eq!(full.get_pixel(0, y0 + 2).0[3], 0);
        assert_eq!(full.get_pixel(11, y0 + 2).0[3], 0);
    }
    // 31 segments; nothing above the top one.
    assert_eq!(full.get_pixel(5, 11).0[3], 0);
    // Flat bands: green at the bottom, amber at ~21 %, red at the top.
    let seg = |img: &toppingctl::tui::pixel::RgbaImage, k: u32| img.get_pixel(5, 225 - 7 * k).0;
    assert!(is(seg(&full, 0), PX_GREEN, 0), "{:?}", seg(&full, 0));
    assert!(is(seg(&full, 24), PX_AMBER, 0), "{:?}", seg(&full, 24));
    assert!(is(seg(&full, 28), PX_AMBER, 0), "8 % is still amber");
    assert!(is(seg(&full, 30), PX_RED, 0), "{:?}", seg(&full, 30));

    // −30 dB lights the bottom half; the rest are tints of their band.
    let half = meter_image(1, 10, CELL, -30.0);
    assert!(is(seg(&half, 0), PX_GREEN, 0));
    assert!(is(seg(&half, 15), PX_GREEN, 0), "the segment at the level");
    assert!(
        is(seg(&half, 20), tint(PX_GREEN), 1),
        "{:?}",
        seg(&half, 20)
    );
    assert!(
        is(seg(&half, 26), tint(PX_AMBER), 1),
        "{:?}",
        seg(&half, 26)
    );
    assert!(is(seg(&half, 30), tint(PX_RED), 1), "{:?}", seg(&half, 30));
    // Silence: every segment unlit.
    let off = meter_image(1, 10, CELL, f64::NEG_INFINITY);
    assert!(is(seg(&off, 0), tint(PX_GREEN), 1));

    // One width everywhere: the input meter's two cells hold the same
    // 10 px bar, centred.
    let wide = meter_image(2, 10, CELL, 0.0);
    let opaque: Vec<u32> = (0..24)
        .filter(|&x| wide.get_pixel(x, 225).0[3] > 0)
        .collect();
    assert_eq!(opaque, (7..17).collect::<Vec<_>>());
    // It scales with the cell: 5 px at 24 px, 10 px at 48 px.
    let big = meter_image(1, 10, (24, 48), 0.0);
    let bot = 480 - 24;
    assert_eq!(big.get_pixel(12, bot - 1).0[3], 255);
    assert_eq!(big.get_pixel(12, bot - 10).0[3], 255);
    assert_eq!(big.get_pixel(12, bot - 11).0[3], 0, "the gap, 4 px");
}

/// The clip light is a square LED over the meter: red when lit, a dark red
/// when not, no glow.
#[test]
fn pixel_clip_light_square() {
    let lit = clip_image(2, 1, CELL, true);
    let off = clip_image(2, 1, CELL, false);
    assert_eq!((lit.width(), lit.height()), (24, 24));
    assert!(is(px(&lit, 12.0, 12.0), PX_RED, 0));
    assert!(is(px(&off, 12.0, 12.0), PX_CLIP_OFF, 0));
    // The meter's 10 px wide, 8 px tall, square corners.
    assert!(lit.get_pixel(7, 8).0[3] > 250, "square corner");
    assert!(lit.get_pixel(16, 15).0[3] > 250, "square corner");
    assert_eq!(lit.get_pixel(6, 12).0[3], 0, "no glow");
    assert_eq!(lit.get_pixel(12, 7).0[3], 0, "no glow");
    assert_eq!(lit.get_pixel(17, 12).0[3], 0, "no glow");
    let narrow = clip_image(1, 1, CELL, true);
    assert!(is(px(&narrow, 6.0, 12.0), PX_RED, 0));
}

/// The knob's track radius: the mockup's R = 0.80 of half the box, the
/// arc 0.13 R wide just inside it.
fn arc_at(img: &toppingctl::tui::pixel::RgbaImage, rel: f64) -> [u8; 4] {
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    let r = w.min(h) / 2.0 * 0.80;
    let ra = r - 0.13 * r / 2.0;
    let a = (225.0 - rel).to_radians();
    px(img, w / 2.0 + ra * a.cos(), h / 2.0 - ra * a.sin())
}

/// A flat knob: a solid track arc, the lit arc from the minimum, a flat
/// disc and a pointer. No ticks, no gradient.
#[test]
fn pixel_knob_flat() {
    let img = knob_image(120, 120, Some(0.5), false, false);
    assert_eq!((img.width(), img.height()), (120, 120));
    assert!(
        is(arc_at(&img, 30.0), PX_ACCENT, 8),
        "{:?}",
        arc_at(&img, 30.0)
    );
    assert!(
        is(arc_at(&img, 200.0), PX_ARC_OFF, 8),
        "{:?}",
        arc_at(&img, 200.0)
    );
    // Solid: no gap anywhere along the track.
    for rel in (2..=268).step_by(2) {
        assert!(arc_at(&img, f64::from(rel))[3] > 250, "gap at {rel}°");
    }
    // The disc is flat: the same colour above and below the centre.
    let r = 48.0;
    assert!(is(px(&img, 60.0 - 0.5 * r, 60.0), PX_KNOB_BODY, 0));
    assert!(is(px(&img, 60.0, 60.0 + 0.5 * r), PX_KNOB_BODY, 0));
    assert!(is(px(&img, 60.0, 60.0 - 0.64 * r), PX_KNOB_BODY, 0));
    // The pointer: up at 0.5, from 0.18 R to 0.56 R.
    assert!(is(px(&img, 60.0, 60.0 - 0.4 * r), PX_POINTER, 8));
    assert!(
        is(px(&img, 60.0, 60.0), PX_KNOB_BODY, 0),
        "not from the centre"
    );
    // Outside the track: see-through; the box's corners too.
    assert_eq!(px(&img, 60.0, 60.0 - r - 2.0)[3], 0);
    assert_eq!(img.get_pixel(0, 0).0[3], 0);
    // At the minimum, nothing is lit.
    let min = knob_image(120, 120, Some(0.0), false, false);
    assert!(is(arc_at(&min, 10.0), PX_ARC_OFF, 8));
    // A wide box: the knob takes 0.80 of its height.
    let wide = knob_image(240, 120, Some(1.0), false, false);
    assert!(is(arc_at(&wide, 260.0), PX_ACCENT, 8));

    // Pans are bipolar from 12 o'clock.
    let c = knob_image(120, 120, Some(0.5), false, true);
    for rel in [20.0, 100.0, 130.0, 140.0, 170.0, 250.0] {
        assert!(
            is(arc_at(&c, rel), PX_ARC_OFF, 8),
            "C: nothing lit at {rel}°"
        );
    }
    let l = knob_image(120, 120, Some(0.25), false, true);
    assert!(is(arc_at(&l, 100.0), PX_ACCENT, 8), "L: lit to the top");
    assert!(
        is(arc_at(&l, 30.0), PX_ARC_OFF, 8),
        "L: not below the pointer"
    );
    assert!(is(arc_at(&l, 170.0), PX_ARC_OFF, 8), "L: not past the top");
    let rt = knob_image(120, 120, Some(0.75), false, true);
    assert!(is(arc_at(&rt, 170.0), PX_ACCENT, 8), "R: lit");
    assert!(
        is(arc_at(&rt, 100.0), PX_ARC_OFF, 8),
        "R: not left of the top"
    );

    // Dimmed (Monitor Mix): nothing lit, a dim disc, and a dim dot at
    // 0.40 R toward the value instead of the pointer; none when unknown.
    let dim = knob_image(120, 120, Some(0.5), true, true);
    assert!(is(arc_at(&dim, 30.0), PX_ARC_OFF, 8));
    assert!(is(arc_at(&dim, 160.0), PX_ARC_OFF, 8));
    assert!(is(px(&dim, 60.0 - 0.5 * r, 60.0), PX_KNOB_BODY_DIM, 0));
    assert!(is(px(&dim, 60.0, 60.0 - 0.40 * r), PX_DIM, 8));
    assert!(
        is(px(&dim, 60.0, 60.0 - 0.2 * r), PX_KNOB_BODY_DIM, 0),
        "a dot, no line"
    );
    let left = knob_image(120, 120, Some(0.0), true, true);
    let a = 225f64.to_radians();
    let dot = px(&left, 60.0 + 0.4 * r * a.cos(), 60.0 - 0.4 * r * a.sin());
    assert!(is(dot, PX_DIM, 8), "{dot:?}");
    let unknown = knob_image(120, 120, None, true, true);
    assert!(is(px(&unknown, 60.0, 60.0 - 0.40 * r), PX_KNOB_BODY_DIM, 0));
}

/// The left and right ends of the opaque run on row `y`.
fn run(img: &toppingctl::tui::pixel::RgbaImage, y: u32) -> (u32, u32) {
    let xs: Vec<u32> = (0..img.width())
        .filter(|&x| img.get_pixel(x, y).0[3] > 0)
        .collect();
    (xs[0], *xs.last().unwrap())
}

/// A button: a flat square face inset in its cells, on its cell box
/// whatever its label (#47), so faces of one width line up. Unlit: the chip
/// face with a 1-px outline. Lit: a solid fill.
#[test]
fn pixel_button_face_on_cell_box() {
    let lit = button_image(6, 1, CELL, PX_ACCENT, None, false);
    assert_eq!((lit.width(), lit.height()), (72, 24));
    let (l, r) = run(&lit, 12);
    assert_eq!((l, r), (3, 68), "inset 3 px each side");
    // 4 px of margin top and bottom, so stacked buttons have gaps.
    for y in (0..4).chain(20..24) {
        assert_eq!(lit.get_pixel(36, y).0[3], 0, "margin at {y}");
    }
    for y in 4..20 {
        assert!(is(lit.get_pixel(36, y).0, PX_ACCENT, 0), "lit face at {y}");
    }
    // Square, solid: the face's corner is the face's colour.
    assert!(is(lit.get_pixel(3, 4).0, PX_ACCENT, 0));
    assert!(is(lit.get_pixel(68, 19).0, PX_ACCENT, 0));
    // Unlit: the chip face inside a 1-px outline.
    let off = button_image(6, 1, CELL, PX_CHIP, None, false);
    assert!(is(off.get_pixel(36, 12).0, PX_CHIP, 0));
    let edge = off.get_pixel(3, 12).0;
    let inner = off.get_pixel(5, 12).0;
    assert!(is(inner, PX_CHIP, 0), "{inner:?}");
    assert!(edge != inner && edge[0] > inner[0], "outline {edge:?}");
    assert!(edge[0] <= PX_CHIP_EDGE.0, "outline {edge:?}");
    assert_eq!(off.get_pixel(1, 12).0[3], 0, "no glow");
    // Three rows (PHONE): the face fills them, inset 4 px.
    let tall = button_image(9, 3, CELL, PX_ACCENT, None, false);
    assert_eq!(tall.get_pixel(54, 3).0[3], 0);
    assert!(is(tall.get_pixel(54, 4).0, PX_ACCENT, 0));
    assert!(is(tall.get_pixel(54, 67).0, PX_ACCENT, 0));
    assert_eq!(tall.get_pixel(54, 68).0[3], 0);
}

/// A chip with no label to centre on (the active mix tab): the face inset
/// 3 px across and 4 px down its cells.
#[test]
fn pixel_chip_inset() {
    let tab = chip_image(9, 1, CELL, PX_GREEN);
    assert_eq!(run(&tab, 12), (3, 104));
    assert_eq!(tab.get_pixel(50, 3).0[3], 0);
    assert!(is(tab.get_pixel(50, 4).0, PX_GREEN, 0));
    assert!(is(tab.get_pixel(3, 4).0, PX_GREEN, 0), "square");
    // It scales: at 24×48, 6 px and 8 px.
    let big = chip_image(9, 1, (24, 48), PX_GREEN);
    assert_eq!(run(&big, 24), (6, 209));
}

/// A flat fader: a thin track, the value filled in blue below a flat cap
/// with a dark centre line, flat tick marks; the label columns stay clear.
#[test]
fn pixel_fader_flat() {
    let img = fader_image(11, 10, CELL, -12.0, 6.0, true);
    assert_eq!((img.width(), img.height()), (132, 240));
    let tx = 5.5 * 12.0;
    let row = |r: u16| (f64::from(r) + 0.5) * 24.0;
    let cap = row(fader_row(fader_frac(-12.0, 6.0), 10));
    // The cap: flat (one colour top to bottom), 32 px wide, 14 tall.
    let (a, b) = (
        px(&img, tx - 10.0, cap - 5.0),
        px(&img, tx + 10.0, cap + 5.0),
    );
    assert!(
        is(a, PX_FADER_CAP, 0) && is(b, PX_FADER_CAP, 0),
        "{a:?} {b:?}"
    );
    assert!(is(px(&img, tx - 15.0, cap - 3.0), PX_FADER_CAP, 0));
    assert_eq!(px(&img, tx - 18.0, cap - 3.0)[3], 0);
    assert_eq!(px(&img, tx, cap - 9.0)[0], PX_ARC_OFF.0, "the track above");
    // Its dark centre line.
    assert!(is(px(&img, tx, cap), PX_BG, 0));
    assert!(is(px(&img, tx - 12.0, cap), PX_BG, 0));
    // Above the cap the bare track; below it the value fill.
    assert!(is(px(&img, tx, row(1)), PX_ARC_OFF, 0));
    assert!(is(px(&img, tx, row(8)), PX_ACCENT, 0));
    // The track is 4 px wide.
    assert_eq!(px(&img, tx - 3.0, row(8))[3], 0);
    assert_eq!(px(&img, tx + 2.5, row(8))[3], 0);
    // A flat tick on a tick row, 10 px out; nothing between rows.
    let ticks = Fader::ticks(6.0, 10);
    let (r24, _) = ticks.iter().find(|(_, l)| l == "−24").unwrap();
    assert!(px(&img, tx - 10.0, row(*r24))[3] > 100, "tick mark");
    assert_eq!(px(&img, tx - 10.0, row(*r24) + 6.0)[3], 0);
    let tick = px(&img, tx + 10.0, row(*r24));
    assert!(tick[0] <= PX_FAINT.0 + 1, "{tick:?}");
    // The label columns (0..3, 8..11) stay clear for the scale text.
    for y in 0..240 {
        for x in (0..36).chain(96..132) {
            assert_eq!(img.get_pixel(x, y).0[3], 0, "label column {x},{y}");
        }
    }
    // Turned up, the cap moves up.
    let up = fader_image(11, 10, CELL, 0.0, 6.0, true);
    let cap0 = row(fader_row(fader_frac(0.0, 6.0), 10));
    assert!(cap0 < cap);
    assert!(is(px(&up, tx - 10.0, cap0 - 5.0), PX_FADER_CAP, 0));
    // The narrow form (3 columns, no labels) fits its cap.
    let narrow = fader_image(3, 10, CELL, -12.0, 6.0, false);
    assert_eq!((narrow.width(), narrow.height()), (36, 240));
    assert!(is(px(&narrow, 18.0 - 10.0, cap - 5.0), PX_FADER_CAP, 0));
}

// ------------------------------------------------------ bipolar pan knobs

/// The Braille pan knob lights from 12 o'clock: nothing at C; at L, no
/// more lit dots than a gain knob at the same value, all on the left half.
#[test]
fn braille_pan_knobs_light_from_the_top() {
    let dial = |v: f64, bipolar: bool| {
        render(
            Dial::new(Theme::color()).value(Some(v)).bipolar(bipolar),
            12,
            6,
        )
    };
    assert_eq!(count(&dial(0.5, true), Theme::KNOB_ACCENT), 0);
    let l = dial(0.25, true);
    let lit = count(&l, Theme::KNOB_ACCENT);
    assert!(lit > 0 && lit < count(&dial(0.25, false), Theme::KNOB_ACCENT) + 1);
    let a = mean_angle(&l, 12, 6, Theme::KNOB_ACCENT).unwrap();
    assert!(close(a, 112.0, 30.0), "lit between pointer and top: {a}°");
}

// --------------------------------------------- flat text fallback (#41)

/// Every button label is bold, lit or not; a button of a given width
/// centres its label the mockup's way, `(w − len) / 2` cells in.
#[test]
fn buttons_bold_and_centred() {
    let t = Theme::color();
    for on in [false, true] {
        for kind in [PillKind::Normal, PillKind::Danger, PillKind::Disabled] {
            let p = pill("MON", on, kind, false, t);
            assert!(
                p.style.add_modifier.contains(Modifier::BOLD),
                "{on} {kind:?}"
            );
        }
    }
    assert!(pill("48V", false, PillKind::Danger, false, Theme::plain())
        .style
        .add_modifier
        .contains(Modifier::BOLD));
    let b = button("LINK", 7, true, PillKind::Normal, false, t);
    assert_eq!(b.content, " LINK  ");
    assert_eq!(b.style.bg, Some(Theme::ACCENT));
    assert!(b.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(
        button("MON", 7, false, PillKind::Normal, false, t).content,
        "  MON  "
    );
    assert_eq!(
        button("Ø", 3, false, PillKind::Normal, false, t).content,
        " Ø "
    );
    assert_eq!(
        button("+17 dBu", 9, false, PillKind::Normal, false, t).content,
        " +17 dBu "
    );
    // Too narrow: the label as is.
    assert_eq!(
        button("SOLO", 3, false, PillKind::Normal, false, t).content,
        "SOLO"
    );
}

/// The text meter mirrors the LED ladder: one square `▆` segment a row
/// (a gap between rows), flat bands by height (red at the top 8 %, amber
/// to 30 %, green below), unlit rows an empty track.
#[test]
fn text_meter_led_ladder() {
    let full = render(Meter::new(0.0, Theme::color()), 1, 25);
    let r = rows(&full);
    assert!(r.iter().all(|l| l == "▆"), "{r:?}");
    assert_eq!(full[(0, 0)].fg, Theme::METER_RED, "top 8 %");
    assert_eq!(full[(0, 1)].fg, Theme::METER_RED);
    assert_eq!(full[(0, 2)].fg, Theme::METER_AMBER, "row 2 is at 10 %");
    assert_eq!(full[(0, 6)].fg, Theme::METER_AMBER, "row 6 is at 26 %");
    assert_eq!(full[(0, 7)].fg, Theme::METER_GREEN, "row 7 is at 30 %");
    assert_eq!(full[(0, 24)].fg, Theme::METER_GREEN);
    // −30 dB on a 60 dB scale: the bottom half.
    let r = rows(&render(Meter::new(-30.0, Theme::color()), 1, 10));
    assert_eq!(r[5..], ["▆"; 5], "{r:?}");
    assert!(r[..5].iter().all(|l| l == "▕"), "{r:?}");
    // One column, however wide its area: one meter width.
    let r = rows(&render(Meter::new(0.0, Theme::color()), 2, 4));
    assert!(r.iter().all(|l| l == "▆ "), "{r:?}");
    // Silence: an empty track; the clip light a square on top.
    let r = rows(&render(Meter::new(-96.0, Theme::color()), 1, 4));
    assert!(r.iter().all(|l| l == "▕"));
    let lit = render(Meter::new(0.0, Theme::color()).clip(Some(true)), 1, 5);
    assert_eq!(rows(&lit)[0], "▀");
    assert_eq!(lit[(0, 0)].fg, Theme::METER_RED);
    assert_eq!(rows(&lit)[1..], ["▆"; 4]);
}

/// The text fader's cap is square, not a round knob.
#[test]
fn text_fader_square_cap() {
    let r = rows(&render(Fader::new(-12.0, 6.0, Theme::color()), 11, 6));
    assert_eq!(r[3], "−12━━■━━−12");
    let r = rows(&render(Fader::new(-6.0, 0.0, Theme::color()), 9, 5));
    assert_eq!(r[1], " −6━■━−6 ");
    let r = rows(&render(
        Fader::new(-6.0, 0.0, Theme::color()).labels(false),
        3,
        5,
    ));
    assert_eq!(r[1], "━■━");
    for line in rows(&render(Fader::new(0.0, 6.0, Theme::color()), 11, 6)) {
        assert!(!line.contains('●'), "{line}");
    }
}
