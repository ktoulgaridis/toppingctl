//! The TUI's shared widgets (#34): dial, fader, meter, pill. Rendered into a
//! plain `Buffer`; no terminal, no hardware.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::widgets::Widget;

use toppingctl::tui::theme::Theme;
use toppingctl::tui::widgets::dial::{Dial, DIAL_HEIGHT, DIAL_WIDTH};
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

/// The lit Braille dots of a dial, as (x, y) in dot units, y down.
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

/// The dial's centre in dot units.
fn centre() -> (f64, f64) {
    (
        (f64::from(DIAL_WIDTH) * 2.0 - 1.0) / 2.0,
        (f64::from(DIAL_HEIGHT) * 4.0 - 1.0) / 2.0,
    )
}

/// The pointer: the dots near the centre. Returns their mean angle in
/// degrees (0 = right, 90 = up), or `None` when there are none.
fn pointer_angle(buf: &Buffer) -> Option<f64> {
    let (cx, cy) = centre();
    let inner: Vec<(f64, f64)> = dots(buf)
        .into_iter()
        .map(|(x, y, _)| (x - cx, cy - y))
        .filter(|(x, y)| (x * x + y * y).sqrt() < 2.9)
        .collect();
    if inner.is_empty() {
        return None;
    }
    let n = inner.len() as f64;
    let (sx, sy) = inner
        .iter()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    Some((sy / n).atan2(sx / n).to_degrees().rem_euclid(360.0))
}

/// Dots on the ring (ticks and the lit arc).
fn ring_dots(buf: &Buffer) -> usize {
    let (cx, cy) = centre();
    dots(buf)
        .into_iter()
        .filter(|(x, y, _)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() >= 2.9)
        .count()
}

fn accent_cells(buf: &Buffer) -> usize {
    buf.content()
        .iter()
        .filter(|c| c.fg == Theme::ACCENT)
        .count()
}

fn dial(value: Option<f64>, bipolar: bool) -> Buffer {
    let d = Dial::new(Theme::color()).value(value).bipolar(bipolar);
    render(d, DIAL_WIDTH, DIAL_HEIGHT)
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d) <= tol
}

/// A 270° dial: min points down-left, centre up, max down-right; the lit
/// arc grows with the value.
#[test]
fn dial_min_centre_max() {
    let min = dial(Some(0.0), false);
    let mid = dial(Some(0.5), false);
    let max = dial(Some(1.0), false);

    let a = pointer_angle(&min).expect("a pointer at min");
    assert!(close(a, 225.0, 30.0), "min points down-left: {a}°");
    let a = pointer_angle(&mid).expect("a pointer at centre");
    assert!(close(a, 90.0, 30.0), "centre points up: {a}°");
    let a = pointer_angle(&max).expect("a pointer at max");
    assert!(close(a, 315.0, 30.0), "max points down-right: {a}°");

    let (r0, r5, r1) = (ring_dots(&min), ring_dots(&mid), ring_dots(&max));
    assert!(r0 > 0, "scale ticks are drawn at min");
    assert!(r0 < r5 && r5 < r1, "the lit arc grows: {r0} {r5} {r1}");
    assert!(accent_cells(&max) > accent_cells(&min));
    assert!(rows(&min) != rows(&max));

    // A pan dial lights from the centre: nothing lit at centre, the two
    // extremes are mirror images.
    let c = dial(Some(0.5), true);
    let l = dial(Some(0.0), true);
    let r = dial(Some(1.0), true);
    assert!(ring_dots(&c) < ring_dots(&l));
    let (nl, nr) = (ring_dots(&l) as i64, ring_dots(&r) as i64);
    assert!(
        (nl - nr).abs() <= 3,
        "about the same arc each way: {nl} {nr}"
    );
    if std::env::var_os("SHOW_DIALS").is_some() {
        for b in [&min, &mid, &max, &c, &l, &r] {
            eprintln!("{}\n", rows(b).join("\n"));
        }
    }
    assert!(close(pointer_angle(&c).unwrap(), 90.0, 30.0));
}

/// Monitor Mix: the device never reports it, so the dial is dim with no
/// pointer and no lit arc, only the scale.
#[test]
fn dial_unknown_is_dimmed() {
    let unknown = dial(None, false);
    assert_eq!(pointer_angle(&unknown), None, "no pointer");
    assert!(ring_dots(&unknown) > 0, "the scale is still drawn");
    assert!(ring_dots(&unknown) <= ring_dots(&dial(Some(0.0), false)));
    assert_eq!(accent_cells(&unknown), 0, "nothing lit");
}

/// The value is printed under the dial when there is room.
#[test]
fn dial_shows_value() {
    let d = Dial::new(Theme::color())
        .value(Some(0.6))
        .label(Some("+12 dB".into()));
    let buf = render(d, 9, DIAL_HEIGHT + 1);
    assert_eq!(rows(&buf)[3].trim(), "+12 dB");

    // No colour under NO_COLOR, but the dial and value are still there.
    let d = Dial::new(Theme::plain())
        .value(Some(0.6))
        .label(Some("L40".into()));
    let buf = render(d, 9, DIAL_HEIGHT + 1);
    assert!(buf
        .content()
        .iter()
        .all(|c| c.fg == Color::Reset && c.bg == Color::Reset));
    assert!(pointer_angle(&buf).is_some() || ring_dots(&buf) > 0);
    assert_eq!(rows(&buf)[3].trim(), "L40");
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

    assert_eq!(fmt_db(0.0), "0.0 dB");
    assert_eq!(fmt_db(-0.0), "0.0 dB", "no −0.0");
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
