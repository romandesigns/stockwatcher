//! Header-aware request pacing for research acquisition (`swimport research-*`).
//!
//! The batch importer's [`super::batch::RateLimiter`] spaces requests at a fixed rate and trusts
//! the configured number. A research download runs on the same Alpaca account as the production
//! nightly importer (the VPS archiver), which this process cannot see, so it must also listen to
//! what the provider says:
//!
//! - `X-RateLimit-Limit` (the account's per-minute limit): if it is below twice our own cap, we
//!   drop to half of it, leaving the other half for whatever else uses the account.
//! - `X-RateLimit-Remaining` / `X-RateLimit-Reset` (unix seconds): when the remaining budget of
//!   the current window is nearly gone, pause until the reset instead of spending it.
//! - HTTP 429: count it, halve our rate (never below 1/min), and pause for `Retry-After`, else
//!   until `X-RateLimit-Reset`, else a minute. The pause is also returned so the HTTP seam can
//!   hand it to [`super::http::get_with_retry`] as the retry delay when the server sent no
//!   `Retry-After`.
//!
//! Pure bookkeeping over injected instants, so it is unit-tested without sleeping.

use serde::Serialize;
use std::time::{Duration, Instant};

/// Remaining-request count at or below which we stop and wait for the window to reset.
pub const LOW_REMAINING: u32 = 5;

/// The provider's rate-limit headers of one response, as far as they were present and parsed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RateHeaders {
    pub limit: Option<u32>,
    pub remaining: Option<u32>,
    /// Unix seconds at which the current window resets.
    pub reset_unix: Option<i64>,
}

/// What the pacer saw over its lifetime, for the run report.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PacerReport {
    pub cap_per_minute: u32,
    pub final_per_minute: u32,
    /// Largest and smallest `X-RateLimit-Limit` seen (normally one value: the account's limit).
    pub observed_limit_max: Option<u32>,
    pub observed_limit_min: Option<u32>,
    pub min_remaining: Option<u32>,
    pub throttled_429: u64,
    /// Pauses taken because `X-RateLimit-Remaining` ran low.
    pub low_remaining_pauses: u64,
    /// Total time spent waiting in the pacer (spacing + pauses).
    pub waited_ms: u64,
}

#[derive(Debug)]
pub struct Pacer {
    per_min: u32,
    next: Option<Instant>,
    pause_until: Option<Instant>,
    report: PacerReport,
}

impl Pacer {
    pub fn new(cap_per_minute: u32) -> Self {
        let cap = cap_per_minute.max(1);
        Pacer {
            per_min: cap,
            next: None,
            pause_until: None,
            report: PacerReport {
                cap_per_minute: cap,
                final_per_minute: cap,
                ..PacerReport::default()
            },
        }
    }

    pub fn per_minute(&self) -> u32 {
        self.per_min
    }

    pub fn spacing(&self) -> Duration {
        Duration::from_secs(60) / self.per_min.max(1)
    }

    /// Reserves the next request slot and returns how long to wait (from `now`) before sending.
    pub fn reserve(&mut self, now: Instant) -> Duration {
        let mut at = self.next.map_or(now, |n| n.max(now));
        if let Some(p) = self.pause_until {
            at = at.max(p);
        }
        self.next = Some(at + self.spacing());
        let wait = at.saturating_duration_since(now);
        self.report.waited_ms += wait.as_millis() as u64;
        wait
    }

    /// Feeds back one response. Returns the pause chosen for a 429 (so a missing `Retry-After`
    /// can be filled in), `None` otherwise.
    pub fn observe(
        &mut self,
        now: Instant,
        now_unix: i64,
        status: u16,
        h: &RateHeaders,
        retry_after: Option<Duration>,
    ) -> Option<Duration> {
        if let Some(l) = h.limit {
            let r = &mut self.report;
            r.observed_limit_max = Some(r.observed_limit_max.map_or(l, |m| m.max(l)));
            r.observed_limit_min = Some(r.observed_limit_min.map_or(l, |m| m.min(l)));
            let share = (l / 2).max(1);
            if share < self.per_min {
                self.per_min = share;
            }
        }
        let until_reset = h
            .reset_unix
            .map(|r| Duration::from_secs((r - now_unix).clamp(1, 120) as u64));
        if let Some(rem) = h.remaining {
            let r = &mut self.report;
            r.min_remaining = Some(r.min_remaining.map_or(rem, |m| m.min(rem)));
            if rem <= LOW_REMAINING && status != 429 {
                let wait = until_reset.unwrap_or(Duration::from_secs(60));
                self.pause(now + wait);
                self.report.low_remaining_pauses += 1;
            }
        }
        let out = if status == 429 {
            self.report.throttled_429 += 1;
            self.per_min = (self.per_min / 2).max(1);
            let wait = retry_after
                .or(until_reset)
                .unwrap_or(Duration::from_secs(60));
            self.pause(now + wait);
            Some(wait)
        } else {
            None
        };
        self.report.final_per_minute = self.per_min;
        out
    }

    fn pause(&mut self, until: Instant) {
        self.pause_until = Some(self.pause_until.map_or(until, |p| p.max(until)));
    }

    pub fn report(&self) -> PacerReport {
        self.report.clone()
    }
}

/// Parses the three `X-RateLimit-*` headers from a header lookup (case-insensitive by caller).
pub fn rate_headers(get: impl Fn(&str) -> Option<String>) -> RateHeaders {
    let num = |k: &str| get(k).and_then(|v| v.trim().parse::<i64>().ok());
    RateHeaders {
        limit: num("x-ratelimit-limit").and_then(|v| u32::try_from(v).ok()),
        remaining: num("x-ratelimit-remaining").and_then(|v| u32::try_from(v).ok()),
        reset_unix: num("x-ratelimit-reset"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_requests_at_the_cap() {
        let t0 = Instant::now();
        let mut p = Pacer::new(100);
        assert_eq!(p.reserve(t0), Duration::ZERO);
        assert_eq!(p.reserve(t0), Duration::from_millis(600));
        assert_eq!(p.reserve(t0), Duration::from_millis(1200));
        // Time passing frees the slot.
        assert_eq!(p.reserve(t0 + Duration::from_secs(10)), Duration::ZERO);
    }

    #[test]
    fn a_lower_account_limit_halves_into_our_share() {
        let t0 = Instant::now();
        let mut p = Pacer::new(100);
        let h = RateHeaders {
            limit: Some(200),
            remaining: Some(150),
            reset_unix: Some(1_000),
        };
        p.observe(t0, 990, 200, &h, None);
        assert_eq!(p.per_minute(), 100, "200/2 is not below our cap");
        let h = RateHeaders {
            limit: Some(60),
            ..h
        };
        p.observe(t0, 990, 200, &h, None);
        assert_eq!(p.per_minute(), 30);
        let r = p.report();
        assert_eq!(r.observed_limit_min, Some(60));
        assert_eq!(r.observed_limit_max, Some(200));
    }

    #[test]
    fn low_remaining_pauses_until_reset() {
        let t0 = Instant::now();
        let mut p = Pacer::new(100);
        p.reserve(t0);
        let h = RateHeaders {
            limit: Some(10_000),
            remaining: Some(3),
            reset_unix: Some(1_007),
        };
        p.observe(t0, 1_000, 200, &h, None);
        assert_eq!(p.reserve(t0), Duration::from_secs(7));
        assert_eq!(p.report().low_remaining_pauses, 1);
    }

    #[test]
    fn a_429_halves_the_rate_and_pauses() {
        let t0 = Instant::now();
        let mut p = Pacer::new(100);
        // Retry-After wins over the reset header.
        let h = RateHeaders {
            limit: None,
            remaining: Some(0),
            reset_unix: Some(1_030),
        };
        let w = p.observe(t0, 1_000, 429, &h, Some(Duration::from_secs(4)));
        assert_eq!(w, Some(Duration::from_secs(4)));
        assert_eq!(p.per_minute(), 50);
        assert_eq!(p.reserve(t0), Duration::from_secs(4));
        // No Retry-After: until the reset.
        let w = p.observe(t0, 1_000, 429, &h, None);
        assert_eq!(w, Some(Duration::from_secs(30)));
        // Nothing at all: a minute.
        let w = p.observe(t0, 1_000, 429, &RateHeaders::default(), None);
        assert_eq!(w, Some(Duration::from_secs(60)));
        assert_eq!(p.report().throttled_429, 3);
        assert_eq!(p.per_minute(), 12);
    }

    #[test]
    fn parses_headers() {
        let h = rate_headers(|k| match k {
            "x-ratelimit-limit" => Some("200".into()),
            "x-ratelimit-remaining" => Some(" 199 ".into()),
            "x-ratelimit-reset" => Some("1760000000".into()),
            _ => None,
        });
        assert_eq!(
            h,
            RateHeaders {
                limit: Some(200),
                remaining: Some(199),
                reset_unix: Some(1_760_000_000)
            }
        );
        assert_eq!(rate_headers(|_| None), RateHeaders::default());
    }
}
