//! Meter ballistics (#71): instant attack, a steady release, a peak hold,
//! and when the meters need the fast redraw. A fake clock throughout.

use std::time::{Duration, Instant};

use toppingctl::state::{Meters, METER_FLOOR_DB};
use toppingctl::tui::app::MeterId;
use toppingctl::tui::ballistics::{
    Ballistic, MeterBallistics, ANIMATE_TICK, PEAK_HOLD, RELEASE_DB_PER_S,
};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn the_constants() {
    assert_eq!(RELEASE_DB_PER_S, 20.0);
    assert_eq!(PEAK_HOLD, ms(1500));
    assert_eq!(ANIMATE_TICK, ms(16));
}

#[test]
fn starts_at_the_floor_and_still() {
    let b = Ballistic::default();
    assert_eq!(b.level(), METER_FLOOR_DB);
    assert_eq!(b.peak(), METER_FLOOR_DB);
    assert!(!b.moving(Instant::now()));
}

#[test]
fn attack_is_instant() {
    let t0 = Instant::now();
    let mut b = Ballistic::default();
    b.advance(-12.0, t0);
    assert_eq!(b.level(), -12.0);
    assert_eq!(b.peak(), -12.0);
    // A higher reading shows at once, whatever came before.
    b.advance(-30.0, t0 + ms(10));
    b.advance(-3.0, t0 + ms(20));
    assert_eq!(b.level(), -3.0);
    assert_eq!(b.peak(), -3.0);
}

#[test]
fn release_falls_at_the_release_rate() {
    let t0 = Instant::now();
    let mut b = Ballistic::default();
    b.advance(-6.0, t0);
    b.advance(METER_FLOOR_DB, t0 + ms(100));
    assert!(close(b.level(), -8.0), "{}", b.level());
    b.advance(METER_FLOOR_DB, t0 + ms(600));
    assert!(close(b.level(), -18.0), "{}", b.level());
    // Never below the reading: it stops where the signal is.
    b.advance(-20.0, t0 + ms(800));
    assert_eq!(b.level(), -20.0);
    b.advance(-20.0, t0 + ms(5000));
    assert_eq!(b.level(), -20.0);
    // Down to the floor, no further.
    b.advance(METER_FLOOR_DB, t0 + ms(60_000));
    assert_eq!(b.level(), METER_FLOOR_DB);
}

#[test]
fn release_is_the_same_however_often_it_is_advanced() {
    let t0 = Instant::now();
    let (mut a, mut b) = (Ballistic::default(), Ballistic::default());
    a.advance(0.0, t0);
    b.advance(0.0, t0);
    for i in 1..=60 {
        a.advance(METER_FLOOR_DB, t0 + ms(i * 16));
    }
    b.advance(METER_FLOOR_DB, t0 + ms(960));
    assert!(close(a.level(), b.level()), "{} {}", a.level(), b.level());
    assert!(close(a.level(), -19.2));
}

#[test]
fn peak_holds_then_falls_at_the_release_rate() {
    let t0 = Instant::now();
    let mut b = Ballistic::default();
    b.advance(-6.0, t0);
    b.advance(-40.0, t0 + ms(56));
    // Held for PEAK_HOLD after the peak...
    b.advance(-40.0, t0 + ms(1500));
    assert_eq!(b.peak(), -6.0);
    // ...then falls at the release rate from then.
    b.advance(-40.0, t0 + ms(1600));
    assert!(close(b.peak(), -8.0), "{}", b.peak());
    // Not below the bar.
    b.advance(-40.0, t0 + ms(10_000));
    assert_eq!(b.peak(), b.level());
    assert_eq!(b.peak(), -40.0);
}

#[test]
fn a_new_peak_restarts_the_hold() {
    let t0 = Instant::now();
    let mut b = Ballistic::default();
    b.advance(-10.0, t0);
    b.advance(-30.0, t0 + ms(100));
    b.advance(-10.0, t0 + ms(1000));
    b.advance(-30.0, t0 + ms(1100));
    b.advance(-30.0, t0 + ms(2400));
    assert_eq!(b.peak(), -10.0, "held from the second peak");
    // A reading just under the held peak does not restart it.
    b.advance(-11.0, t0 + ms(2450));
    b.advance(-30.0, t0 + ms(2600));
    assert!(close(b.peak(), -12.0), "{}", b.peak());
}

#[test]
fn moving_while_falling_or_a_hold_is_falling() {
    let t0 = Instant::now();
    let mut b = Ballistic::default();
    b.advance(-6.0, t0);
    assert!(!b.moving(t0), "at the reading, held");
    b.advance(-30.0, t0 + ms(50));
    assert!(b.moving(t0 + ms(50)), "the bar falls");
    // The bar reaches the reading; the marker is held, still.
    b.advance(-30.0, t0 + ms(1400));
    assert_eq!(b.level(), -30.0);
    assert!(!b.moving(t0 + ms(1400)), "held: nothing moves");
    b.advance(-30.0, t0 + ms(1600));
    assert!(b.moving(t0 + ms(1600)), "the hold falls");
    b.advance(-30.0, t0 + ms(10_000));
    assert!(!b.moving(t0 + ms(10_000)), "all still");
}

#[test]
fn nothing_moves_below_the_meter() {
    // Falling from −70 to the floor is out of sight: no fast redraw.
    let t0 = Instant::now();
    let mut b = Ballistic::default();
    b.advance(-70.0, t0);
    b.advance(METER_FLOOR_DB, t0 + ms(50));
    assert!(b.level() > METER_FLOOR_DB);
    assert!(!b.moving(t0 + ms(50)));
}

#[test]
fn a_bad_reading_is_the_floor() {
    let t0 = Instant::now();
    let mut b = Ballistic::default();
    b.advance(f64::NAN, t0);
    assert_eq!(b.level(), METER_FLOOR_DB);
    b.advance(-10.0, t0);
    b.advance(f64::NEG_INFINITY, t0 + ms(100));
    assert!(close(b.level(), -12.0));
}

#[test]
fn a_clock_going_back_changes_nothing() {
    let t0 = Instant::now() + ms(1000);
    let mut b = Ballistic::default();
    b.advance(-10.0, t0);
    b.advance(METER_FLOOR_DB, t0 - ms(500));
    assert_eq!(b.level(), -10.0);
}

#[test]
fn every_meter_has_its_own() {
    let t0 = Instant::now();
    let mut m = MeterBallistics::default();
    let loud = Meters {
        inputs: [-6.0, -50.0],
        playback: [[-1.0, -2.0], [-3.0, -4.0], [-5.0, -7.0]],
        output: [-8.0, -9.0],
        loopbacks: [[-10.0, -11.0], [-12.0, -13.0]],
    };
    m.advance(&loud, t0);
    assert_eq!(m.levels(&loud), loud);
    assert_eq!(m.peaks(&loud), loud);
    assert!(!m.moving(t0));
    let quiet = Meters::default();
    m.advance(&quiet, t0 + ms(500));
    let l = m.levels(&quiet);
    assert!(close(l.inputs[0], -16.0));
    assert!(close(l.playback[2][1], -17.0));
    assert!(close(l.loopbacks[1][1], -23.0));
    assert_eq!(m.level(MeterId::Output(1), -96.0), l.output[1]);
    assert_eq!(m.peak(MeterId::Playback(1, 0), -96.0), -3.0, "held");
    assert!(m.moving(t0 + ms(500)));
}

#[test]
fn never_shows_below_the_reading_drawn() {
    // A reading the ballistics have not seen yet (or never will, as in
    // a test that draws without a tick) shows as it is: attack is
    // instant.
    let m = MeterBallistics::default();
    let r = Meters {
        inputs: [-18.4, -4.0],
        ..Meters::default()
    };
    assert_eq!(m.levels(&r), r);
    assert_eq!(m.peaks(&r), r);
    assert_eq!(m.level(MeterId::Input(1), -4.0), -4.0);
}
