//! Wall-clock pacing math. Pure functions, so they are tested without timers.

use serde::{Serialize, Serializer};
use std::fmt;
use std::time::Duration;

/// Replay speed: a whole multiplier of wall time, or as fast as the consumer accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Speed {
    Times(u32),
    Max,
}

/// Largest accepted multiplier (one hour per wall second).
pub const MAX_MULTIPLIER: u32 = 3600;

impl Speed {
    /// Wire form: the multiplier, `0` for max (the client convention in chart-core's `Clock`).
    pub fn as_wire(self) -> f64 {
        match self {
            Speed::Times(m) => f64::from(m),
            Speed::Max => 0.0,
        }
    }

    /// Parses the wire form: `0` = max, otherwise a whole multiplier in `1..=3600`.
    pub fn from_wire(v: f64) -> Result<Self, String> {
        if !v.is_finite() || v < 0.0 {
            return Err(format!("invalid speed {v}"));
        }
        if v == 0.0 {
            return Ok(Speed::Max);
        }
        if v.fract() != 0.0 || v > f64::from(MAX_MULTIPLIER) {
            return Err(format!(
                "speed must be 0 (max) or a whole multiplier 1..={MAX_MULTIPLIER}, got {v}"
            ));
        }
        Ok(Speed::Times(v as u32))
    }

    /// Parses `"10"`, `"10x"`, `"max"`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let t = s.trim().to_ascii_lowercase();
        if t == "max" {
            return Ok(Speed::Max);
        }
        let n: f64 = t
            .trim_end_matches(['x', '×'])
            .parse()
            .map_err(|_| format!("invalid speed {s:?}"))?;
        Speed::from_wire(n)
    }
}

impl fmt::Display for Speed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Speed::Times(m) => write!(f, "{m}x"),
            Speed::Max => f.write_str("max"),
        }
    }
}

impl Serialize for Speed {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(self.as_wire())
    }
}

/// Virtual nanoseconds that elapse during `wall` at `speed` (saturating). `None` for max, which
/// is not paced by wall time.
pub fn virtual_elapsed(wall: Duration, speed: Speed) -> Option<i64> {
    match speed {
        Speed::Max => None,
        Speed::Times(m) => {
            let v = wall.as_nanos().saturating_mul(u128::from(m));
            Some(i64::try_from(v).unwrap_or(i64::MAX))
        }
    }
}

/// Wall time until `virtual_ns` of virtual time has elapsed at `speed`, rounded up so the target
/// has been reached when the timer fires. `None` for max (no waiting).
pub fn wall_until(virtual_ns: i64, speed: Speed) -> Option<Duration> {
    match speed {
        Speed::Max => None,
        Speed::Times(m) => {
            let v = virtual_ns.max(0) as u128;
            let m = u128::from(m.max(1));
            Some(Duration::from_nanos(
                v.div_ceil(m).min(u64::MAX as u128) as u64
            ))
        }
    }
}

/// The virtual target time for a paced tick: `anchor + elapsed × speed`, clamped to `end`.
pub fn paced_target(
    anchor_virtual: i64,
    wall_since_anchor: Duration,
    speed: Speed,
    end: i64,
) -> i64 {
    match virtual_elapsed(wall_since_anchor, speed) {
        None => end,
        Some(v) => anchor_virtual.saturating_add(v).min(end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    #[test]
    fn multipliers_scale_wall_time() {
        let w = Duration::from_millis(250);
        assert_eq!(virtual_elapsed(w, Speed::Times(1)), Some(S / 4));
        assert_eq!(virtual_elapsed(w, Speed::Times(2)), Some(S / 2));
        assert_eq!(virtual_elapsed(w, Speed::Times(10)), Some(5 * S / 2));
        assert_eq!(
            virtual_elapsed(Duration::from_secs(1), Speed::Times(60)),
            Some(60 * S)
        );
        assert_eq!(virtual_elapsed(w, Speed::Max), None);
    }

    #[test]
    fn wall_until_is_the_ceiling_inverse() {
        assert_eq!(
            wall_until(60 * S, Speed::Times(60)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            wall_until(10, Speed::Times(3)),
            Some(Duration::from_nanos(4))
        );
        assert_eq!(wall_until(-5, Speed::Times(2)), Some(Duration::ZERO));
        assert_eq!(wall_until(S, Speed::Max), None);
        for m in [1u32, 2, 10, 60] {
            let w = wall_until(7 * S + 3, Speed::Times(m)).unwrap();
            assert!(virtual_elapsed(w, Speed::Times(m)).unwrap() >= 7 * S + 3);
        }
    }

    #[test]
    fn targets_clamp_and_saturate() {
        assert_eq!(
            paced_target(100, Duration::from_nanos(5), Speed::Times(10), 1_000),
            150
        );
        assert_eq!(
            paced_target(100, Duration::from_secs(5), Speed::Times(10), 1_000),
            1_000
        );
        assert_eq!(paced_target(100, Duration::ZERO, Speed::Max, 1_000), 1_000);
        assert_eq!(
            paced_target(
                0,
                Duration::from_secs(u64::MAX / 2),
                Speed::Times(3600),
                i64::MAX
            ),
            i64::MAX
        );
    }

    #[test]
    fn wire_speeds() {
        assert_eq!(Speed::from_wire(0.0), Ok(Speed::Max));
        assert_eq!(Speed::from_wire(60.0), Ok(Speed::Times(60)));
        assert!(Speed::from_wire(1.5).is_err());
        assert!(Speed::from_wire(-1.0).is_err());
        assert!(Speed::from_wire(f64::NAN).is_err());
        assert_eq!(Speed::parse("10x"), Ok(Speed::Times(10)));
        assert_eq!(Speed::parse("MAX"), Ok(Speed::Max));
        assert_eq!(Speed::Max.as_wire(), 0.0);
    }
}
