//! The TUI's shared widgets (#34): dial, fader, meter, pill. Rendered into a
//! plain `Buffer`; no terminal, no hardware.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::widgets::Widget;

use toppingctl::tui::pixel::{knob_image, KNOB_ACCENT_RGB, KNOB_DIM_RGB, KNOB_POINTER_RGB};
use toppingctl::tui::theme::Theme;
use toppingctl::tui::widgets::dial::{knob_rect, Dial};
use toppingctl::tui::widgets::fader::{fader_frac, fmt_db, Fader};
use toppingctl::tui::widgets::meter::{
    peak_text, ClipHold, Meter, Scale, CLIP_HOLD, INPUT_SCALE_DB, PEAK_HOT_DB,
};
use toppingctl::tui::widgets::pill::{pill, PillKind};

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

/// Monitor Mix (style L): dimmed, no pointer, nothing lit, still a knob.
#[test]
fn braille_knob_dimmed() {
    for v in [None, Some(0.5)] {
        let b = braille(v, true, 12, 6);
        assert_eq!(count(&b, Theme::KNOB_POINTER), 0, "no pointer");
        assert_eq!(count(&b, Theme::KNOB_ACCENT), 0, "nothing lit");
        assert!(count(&b, Theme::KNOB_DIM) > 6, "the ring is drawn");
    }
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

// ------------------------------------------------------------ pixel knob

/// The colour of the pixel on the arc at `rel` degrees from the start
/// (clockwise), at the arc's mid radius.
fn arc_pixel(img: &image::RgbaImage, rel: f64) -> [u8; 4] {
    let (w, h) = (img.width() as f64, img.height() as f64);
    let r = w.min(h) / 2.0;
    let a = (225.0 - rel).to_radians();
    let x = w / 2.0 + 0.87 * r * a.cos();
    let y = h / 2.0 - 0.87 * r * a.sin();
    img.get_pixel(x as u32, y as u32).0
}

fn near(p: [u8; 4], c: (u8, u8, u8)) -> bool {
    p[3] > 200
        && (i32::from(p[0]) - i32::from(c.0)).abs() < 24
        && (i32::from(p[1]) - i32::from(c.1)).abs() < 24
        && (i32::from(p[2]) - i32::from(c.2)).abs() < 24
}

/// The pixel knob (prototype style J): the image has the requested size,
/// and the ticked arc is lit in the accent up to the value, dim after it.
#[test]
fn pixel_knob_lights_the_arc() {
    let ticks = 21;
    let period = 270.0 / f64::from(ticks);
    // The middle of tick 2 (early on the arc) and of tick 18 (late).
    let early = 2.3 * period;
    let late = 18.3 * period;

    let img = knob_image(120, 60, Some(0.5), ticks, false);
    assert_eq!((img.width(), img.height()), (120, 60));
    assert!(
        near(arc_pixel(&img, early), KNOB_ACCENT_RGB),
        "{:?}",
        arc_pixel(&img, early)
    );
    assert!(
        near(arc_pixel(&img, late), KNOB_DIM_RGB),
        "{:?}",
        arc_pixel(&img, late)
    );
    // Between two ticks: see-through.
    assert!(
        arc_pixel(&img, 3.0 * period - 1.0)[3] < 60,
        "a gap between ticks"
    );

    // Turned up past the late tick, it lights.
    let img = knob_image(120, 60, Some(0.95), ticks, false);
    assert!(near(arc_pixel(&img, late), KNOB_ACCENT_RGB));

    // The pointer is white, from the centre toward the value (up at 0.5).
    let img = knob_image(120, 120, Some(0.5), ticks, false);
    let p = img.get_pixel(60, 60 - 18).0;
    assert!(near(p, KNOB_POINTER_RGB), "pointer {p:?}");
    // Corners are transparent (round knob on a see-through square).
    assert_eq!(img.get_pixel(0, 0).0[3], 0);

    // Dimmed (Monitor Mix): nothing lit, no pointer.
    let img = knob_image(120, 120, Some(0.95), ticks, true);
    assert!(near(arc_pixel(&img, late), KNOB_DIM_RGB));
    assert!(!near(img.get_pixel(60, 60 - 18).0, KNOB_POINTER_RGB));
    // A solid arc (pan, style K) has no gaps.
    let img = knob_image(80, 80, Some(1.0), 0, false);
    assert!(near(arc_pixel(&img, 3.0 * period - 1.0), KNOB_ACCENT_RGB));
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
        "−12━━●━━−12",
        "−24  ┃  −24",
        " −∞  ┃  −∞ ",
    ];
    assert_eq!(r, expected);

    // 0 dB sits on the 0 tick; −∞ at the bottom.
    let r = rows(&render(Fader::new(0.0, 6.0, Theme::color()), 11, 6));
    assert_eq!(r[1], "  0━━●━━0  ");
    let r = rows(&render(
        Fader::new(f64::NEG_INFINITY, 6.0, Theme::color()),
        11,
        6,
    ));
    assert_eq!(r[5], " −∞━━●━━−∞ ");

    // Loopback and output faders top out at 0 dB.
    let r = rows(&render(Fader::new(-6.0, 0.0, Theme::color()), 11, 5));
    assert_eq!(
        r,
        [
            "  0  ┃  0  ",
            " −6━━●━━−6 ",
            "−12  ┃  −12",
            "−24  ┃  −24",
            " −∞  ┃  −∞ ",
        ]
    );

    // Narrower: a 3-wide cap.
    let r = rows(&render(Fader::new(-6.0, 0.0, Theme::color()), 9, 5));
    assert_eq!(r[1], " −6━●━−6 ");

    // Without labels: tick marks on both sides of the track.
    let r = rows(&render(
        Fader::new(-6.0, 0.0, Theme::color()).labels(false),
        3,
        5,
    ));
    assert_eq!(r, ["╶┃╴", "━●━", "╶┃╴", "╶┃╴", "╶┃╴"]);

    // Taller: the ticks spread out, the cap follows.
    let r = rows(&render(Fader::new(-24.0, 0.0, Theme::color()), 11, 9));
    let at = |label: &str| r.iter().position(|l| l.trim_start().starts_with(label));
    assert_eq!(at("0"), Some(0));
    assert_eq!(at("−∞"), Some(8));
    assert!(r[6].contains('●'), "{r:?}");
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

fn is_greenish(c: Color) -> bool {
    matches!(c, Color::Rgb(r, g, _) if g > r)
}

/// The meter fills its whole height, green at the bottom to red at the top.
#[test]
fn meter_fills_height_with_gradient() {
    let full = render(Meter::new(0.0, Theme::color()), 1, 10);
    let r = rows(&full);
    assert!(r.iter().all(|l| l == "█"), "{r:?}");
    assert!(is_reddish(full[(0, 0)].fg), "top is red");
    assert!(is_greenish(full[(0, 9)].fg), "bottom is green");

    // −30 dB on a 60 dB scale: the bottom half.
    let r = rows(&render(Meter::new(-30.0, Theme::color()), 1, 10));
    assert_eq!(r[5..].iter().filter(|l| *l == "█").count(), 5, "{r:?}");
    assert!(r[..5].iter().all(|l| !l.contains('█')), "{r:?}");

    // Two wide, with eighth-cell steps: −33 dB is 4.5 rows of 10.
    let r = rows(&render(Meter::new(-33.0, Theme::color()).width(2), 2, 10));
    assert_eq!(r[9], "██");
    assert_eq!(r[5], "▄▄", "{r:?}");

    // Silence: an empty track.
    let r = rows(&render(Meter::new(-96.0, Theme::color()), 1, 4));
    assert!(r.iter().all(|l| !l.contains('█')));

    // A clip light on top: lit or not, the bar starts one row lower.
    let lit = render(Meter::new(0.0, Theme::color()).clip(Some(true)), 1, 5);
    let off = render(Meter::new(0.0, Theme::color()).clip(Some(false)), 1, 5);
    assert_eq!(rows(&lit)[0], "▀");
    assert_eq!(rows(&off)[0], "▔");
    assert!(is_reddish(lit[(0, 0)].fg));
    assert_eq!(rows(&lit)[1..], ["█", "█", "█", "█"]);
}

/// The clip light stays on about a second after the last clip.
#[test]
fn clip_hold_with_fake_clock() {
    assert!(CLIP_HOLD >= Duration::from_millis(900) && CLIP_HOLD <= Duration::from_millis(1100));
    let t0 = Instant::now();
    let ms = |n| t0 + Duration::from_millis(n);
    let mut hold = ClipHold::default();
    assert!(!hold.lit(t0));
    hold.update(-20.0, t0);
    assert!(!hold.lit(t0), "no clip, no light");

    hold.update(0.0, ms(100));
    assert!(hold.lit(ms(100)));
    hold.update(-30.0, ms(600));
    assert!(hold.lit(ms(1000)), "held after the signal drops");
    assert!(!hold.lit(ms(1101)), "off a second after the clip");

    // A new clip restarts the hold.
    hold.update(0.0, ms(800));
    assert!(hold.lit(ms(1700)));
    assert!(!hold.lit(ms(1801)));
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
    // Disabled (IN 1+2 LINK off): greyed, no pill background.
    let dis = pill("LINK", false, PillKind::Disabled, false, t);
    assert_eq!(dis.style.fg, Some(Theme::TRACK));
    assert_eq!(dis.style.bg, None);
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
