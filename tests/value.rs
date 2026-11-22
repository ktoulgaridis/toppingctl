//! Unit tests for `toppingctl::value`, against the encodings in PROTOCOL.md.

use toppingctl::value::*;

const ZERO_DB: i32 = 0x0200_0000;

fn assert_near(actual: i32, expected: i32, tolerance: i32) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual:#x} not within {tolerance} of {expected:#x}"
    );
}

/// AC3.
#[test]
fn db_level_round_trip() {
    assert_eq!(level_from_db(0.0), ZERO_DB);
    assert_near(level_from_db(-6.0206), 0x0100_0000, 1);
    assert_near(level_from_db(20.0), 0x1400_0000, 1);
    assert_eq!(level_from_db(f64::NEG_INFINITY), 0);

    assert_eq!(db_from_level(ZERO_DB), 0.0);
    // −60 … +20 dB in 0.1 dB steps.
    for tenths in -600..=200 {
        let db = f64::from(tenths) / 10.0;
        let back = db_from_level(level_from_db(db));
        assert!((back - db).abs() <= 0.01, "{db} dB came back as {back} dB");
    }
}

/// AC4.
#[test]
fn pan_and_phase() {
    let l = ZERO_DB;
    assert_eq!(pan_split(l, 0.0), (l, 0));
    assert_eq!(pan_split(l, 0.5), (l / 2, l / 2));
    assert_eq!(pan_split(l, 1.0), (0, l));
    let odd = 0x0284_91d1;
    let (left, right) = pan_split(odd, 0.5);
    assert_near(left, odd / 2, 1);
    assert_near(right, odd / 2, 1);

    assert_eq!(phase(l, true), -l);
    assert_eq!(phase(l, false), l);
    assert_eq!(phase(0, true), 0);

    assert_eq!(db_from_level(-l), db_from_level(l));
    assert_eq!(db_from_level(-0x0100_0000), db_from_level(0x0100_0000));
    assert_eq!(db_from_level(0), f64::NEG_INFINITY);
}

/// AC5.
#[test]
fn meters_and_bools() {
    assert_eq!(meter_db(-299), -29.9);
    assert_eq!(meter_db(0), 0.0);
    assert_eq!(meter_db(-960), -96.0);

    assert_eq!(bool_value(false), 0);
    assert_eq!(bool_value(true), 1);
    assert!(!value_bool(0));
    assert!(value_bool(1));
}
