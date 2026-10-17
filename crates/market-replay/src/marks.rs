//! The deterministic engine clock: marks quantized to a fixed step (default 1 s).

use market_events::UnixNanos;
use market_events::time::NANOS_PER_SEC;

pub const DEFAULT_MARK_STEP_NS: i64 = NANOS_PER_SEC;

/// Produces monotone engine-clock marks `floor_step(t)`, each value at most once.
#[derive(Clone, Copy, Debug)]
pub struct MarkClock {
    step: i64,
    last: Option<i64>,
}

impl Default for MarkClock {
    fn default() -> Self {
        MarkClock::new(DEFAULT_MARK_STEP_NS)
    }
}

impl MarkClock {
    pub fn new(step_ns: i64) -> Self {
        MarkClock {
            step: step_ns.max(1),
            last: None,
        }
    }

    pub fn step(&self) -> i64 {
        self.step
    }

    /// The mark to emit for instant `t` (before an event delivered at `t`, or at the end of a
    /// step to `t`), or `None` if the clock is already there.
    pub fn mark(&mut self, t: i64) -> Option<UnixNanos> {
        let m = t.div_euclid(self.step) * self.step;
        if self.last.is_none_or(|l| m > l) {
            self.last = Some(m);
            Some(UnixNanos(m))
        } else {
            None
        }
    }

    pub fn last(&self) -> Option<UnixNanos> {
        self.last.map(UnixNanos)
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantizes_and_never_repeats() {
        let mut m = MarkClock::new(1_000);
        assert_eq!(m.mark(1_500), Some(UnixNanos(1_000)));
        assert_eq!(m.mark(1_999), None);
        assert_eq!(m.mark(2_000), Some(UnixNanos(2_000)));
        assert_eq!(m.mark(1_000), None, "never moves back");
    }
}
