//! Conversions between the device's value encodings and user-facing units
//! (PROTOCOL.md, "Value types").

/// The level word for 0 dB: gain = value / 2²⁵.
pub const ZERO_DB_LEVEL: i32 = 0x0200_0000;

/// The level word for a gain of `db` decibels, rounded to the nearest step.
/// `-inf` gives `0` (muted); gains beyond the `i32` range saturate.
pub fn level_from_db(db: f64) -> i32 {
    (f64::from(ZERO_DB_LEVEL) * 10f64.powf(db / 20.0)).round() as i32
}

/// The gain in decibels of a level word. The sign (Ø) is ignored; `0` gives
/// `-inf`.
pub fn db_from_level(level: i32) -> f64 {
    20.0 * (f64::from(level.unsigned_abs()) / f64::from(ZERO_DB_LEVEL)).log10()
}

/// Splits `level` across L and R for `pan` from `0.0` (left) to `1.0`
/// (right): a linear split, so the centre gives each side half the level.
/// `pan` outside `0.0..=1.0` is clamped.
pub fn pan_split(level: i32, pan: f64) -> (i32, i32) {
    let pan = pan.clamp(0.0, 1.0);
    let right = (f64::from(level) * pan).round() as i32;
    let left = (f64::from(level) * (1.0 - pan)).round() as i32;
    (left, right)
}

/// `level` with the phase inverted (negated) when `inverted`.
pub fn phase(level: i32, inverted: bool) -> i32 {
    if inverted {
        level.saturating_neg()
    } else {
        level
    }
}

/// The value word for a bool: `0` off, `1` on.
pub fn bool_value(on: bool) -> i32 {
    i32::from(on)
}

/// Whether a bool value word is on (any non-zero value).
pub fn value_bool(value: i32) -> bool {
    value != 0
}

/// A meter word in dB: the device reports tenths of a dB.
pub fn meter_db(value: i32) -> f64 {
    f64::from(value) / 10.0
}
