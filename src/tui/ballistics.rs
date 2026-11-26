//! Meter ballistics (#71): what a meter shows between the device's
//! readings. The E2x2 sends each meter about every 56 ms (~18 Hz); drawn
//! raw, the bars jump. Here the attack is instant (a higher reading shows
//! at once), the release is a steady fall at [`RELEASE_DB_PER_S`], and a
//! peak marker holds the recent peak for [`PEAK_HOLD`] before it falls at
//! the same rate. Clip lights are not touched (see `ClipHold`).
//!
//! Pure functions of (reading, time): [`Ballistic::advance`] takes the
//! latest reading and the time, so the tests drive it with a fake clock.
//! A meter never shows below the reading being drawn ([`MeterBallistics::levels`]),
//! so a frame drawn without a tick (the golden snapshots) shows the
//! readings as they are.

use std::time::{Duration, Instant};

use crate::state::{Meters, METER_FLOOR_DB};
use crate::tui::app::MeterId;
use crate::tui::widgets::meter::METER_RANGE_DB;

/// How fast a bar (and a released peak marker) falls, in dB a second.
/// Tuned on the hardware: smooth at the device's 18 Hz, and a full-scale
/// bar is down the meter's 60 dB in 3 s.
pub const RELEASE_DB_PER_S: f64 = 20.0;
/// How long the peak marker stays at a peak before it falls. Tuned on the
/// hardware.
pub const PEAK_HOLD: Duration = Duration::from_millis(1500);
/// The redraw interval while a meter moves (~60 fps); otherwise the event
/// loop's normal tick.
pub const ANIMATE_TICK: Duration = Duration::from_millis(16);

/// One meter's ballistics: the level shown, the peak marker, until when
/// the marker holds, and the last reading and its time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ballistic {
    level: f64,
    peak: f64,
    hold_until: Option<Instant>,
    reading: f64,
    at: Option<Instant>,
}

impl Default for Ballistic {
    fn default() -> Self {
        Ballistic {
            level: METER_FLOOR_DB,
            peak: METER_FLOOR_DB,
            hold_until: None,
            reading: METER_FLOOR_DB,
            at: None,
        }
    }
}

/// `db` as a reading: at least the floor, the floor if not a number.
fn clean(db: f64) -> f64 {
    if db.is_nan() {
        METER_FLOOR_DB
    } else {
        db.max(METER_FLOOR_DB)
    }
}

/// Whether `db` is on the meter (above its bottom).
fn visible(db: f64) -> bool {
    db > -METER_RANGE_DB
}

impl Ballistic {
    /// The latest reading `db`, at `now`. A clock going back counts as no
    /// time passing.
    pub fn advance(&mut self, db: f64, now: Instant) {
        let db = clean(db);
        let last = self.at.map_or(now, |a| a.min(now));
        let fall = |secs: f64| RELEASE_DB_PER_S * secs;
        let dt = now.saturating_duration_since(last).as_secs_f64();
        self.level = db.max(self.level - fall(dt));
        if db >= self.peak {
            // A new peak: the marker goes to it and holds from now.
            self.peak = db;
            self.hold_until = Some(now + PEAK_HOLD);
        } else if let Some(until) = self.hold_until.filter(|&u| now > u) {
            let since = now.saturating_duration_since(until.max(last));
            self.peak -= fall(since.as_secs_f64());
        }
        // The marker is never under the bar.
        self.peak = self.peak.max(self.level);
        self.reading = db;
        self.at = Some(self.at.map_or(now, |a| a.max(now)));
    }

    /// The level shown, in dB.
    pub fn level(&self) -> f64 {
        self.level
    }

    /// The peak marker, in dB (at the level when there is none to show).
    pub fn peak(&self) -> f64 {
        self.peak
    }

    /// Whether, as of `now`, the bar is falling or the marker is (its hold
    /// over), on the meter: what needs the fast redraw. A marker that is
    /// only held is still.
    pub fn moving(&self, now: Instant) -> bool {
        let falling = self.level > self.reading && visible(self.level);
        let released = self.hold_until.is_some_and(|u| now >= u);
        falling || (self.peak > self.level && released && visible(self.peak))
    }
}

/// `m`'s readings, in [`MeterId`] index order (IN1, IN2, PB 1/2 L R …
/// 5/6, output L R, loopback 1+2 L R, 3+4 L R).
pub fn flat(m: &Meters) -> [f64; 14] {
    [
        m.inputs[0],
        m.inputs[1],
        m.playback[0][0],
        m.playback[0][1],
        m.playback[1][0],
        m.playback[1][1],
        m.playback[2][0],
        m.playback[2][1],
        m.output[0],
        m.output[1],
        m.loopbacks[0][0],
        m.loopbacks[0][1],
        m.loopbacks[1][0],
        m.loopbacks[1][1],
    ]
}

/// The inverse of [`flat`].
fn unflat(v: [f64; 14]) -> Meters {
    Meters {
        inputs: [v[0], v[1]],
        playback: [[v[2], v[3]], [v[4], v[5]], [v[6], v[7]]],
        output: [v[8], v[9]],
        loopbacks: [[v[10], v[11]], [v[12], v[13]]],
    }
}

/// Every meter's ballistics, on the readings (before the faders: the
/// post-fader meters are [`Meters::post`] of these).
#[derive(Clone, Debug, Default)]
pub struct MeterBallistics {
    meters: [Ballistic; 14],
}

impl MeterBallistics {
    /// The latest readings, at `now`.
    pub fn advance(&mut self, readings: &Meters, now: Instant) {
        for (b, db) in self.meters.iter_mut().zip(flat(readings)) {
            b.advance(db, now);
        }
    }

    /// The levels to draw for the readings `raw`: never below them.
    pub fn levels(&self, raw: &Meters) -> Meters {
        self.each(raw, Ballistic::level)
    }

    /// The peak markers to draw for the readings `raw`: never below them.
    pub fn peaks(&self, raw: &Meters) -> Meters {
        self.each(raw, Ballistic::peak)
    }

    /// `id`'s level to draw, for its reading `raw`.
    pub fn level(&self, id: MeterId, raw: f64) -> f64 {
        self.meters[id.index()].level.max(raw)
    }

    /// `id`'s peak marker to draw, for its reading `raw`.
    pub fn peak(&self, id: MeterId, raw: f64) -> f64 {
        self.meters[id.index()].peak.max(raw)
    }

    /// Whether any meter is moving at `now` ([`Ballistic::moving`]).
    pub fn moving(&self, now: Instant) -> bool {
        self.meters.iter().any(|b| b.moving(now))
    }

    fn each(&self, raw: &Meters, f: fn(&Ballistic) -> f64) -> Meters {
        let mut v = flat(raw);
        for (x, b) in v.iter_mut().zip(&self.meters) {
            *x = f(b).max(*x);
        }
        unflat(v)
    }
}
