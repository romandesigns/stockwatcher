//! The trade-report batch detector for the Fast-move tape (research/fast-0800, 2026-10-26).
//!
//! # What it detects
//!
//! FINRA's trade reporting facility (TRF, venue `D` on the tape) reports off-exchange trades
//! executed while it was closed in one batch when it opens, stamped with the report time and
//! the condition Form T (extended hours). That was 08:00 ET until 2026-03-27 and 04:00 ET since
//! 2026-03-30 (Regulatory Notice 25-15); FINRA has proposed near-24-hour operation
//! (SR-FINRA-2026-015), which would move it again. A batch puts hours of a stock's range and
//! hundreds of prints into a second, which is exactly what the Fast rule (+8 % in 60 s with
//! trades and dollars) reads as a move: about a fifth of current Fast first alerts sat in the
//! 04:00 batch, with a 7.6 % hit rate against 22.2 % with these prints left out of the tape.
//!
//! # How, from the tape and not the clock
//!
//! A batch is a TRF opening: venue-`D` prints market-wide go from **silence** (the facility was
//! closed) to a **flood** of Form T reports. The research profiles (`trf-profile-by-day.txt`,
//! `profile-0400-sample.jsonl`):
//!
//! - silence: 0 `D` prints in 07:50–07:59 on every day before 2026-03-30, and 0 before 04:00
//!   since; [`SILENCE_MAX`] prints in the last [`SILENCE_S`] seconds is "silent", and the
//!   detector must itself have watched those seconds (a process that just started has not, so
//!   a restart in the middle of a busy session cannot mistake normal traffic for a batch);
//! - flood: 17,855–66,275 `D` prints in the 04:00 minute (current), 156k–504k in the 08:00 one
//!   (before), against a real-time Form T rate of about 60 a second before 08:00 today;
//!   [`FLOOD_MIN`] Form T prints within [`PROBE_S`] seconds of the first print after silence
//!   confirms it;
//! - end: the batch decays over 2–4 minutes (04:03 already 1.2k–16k a minute); the window closes
//!   once [`MIN_S`] have passed and the last [`END_WINDOW_S`] seconds held fewer than
//!   [`END_COUNT`] Form T prints (50 a second), or at [`MAX_S`].
//!
//! Neither the 16:00 after-hours start nor today's 08:00 is a batch: `D` prints were flowing
//! right before both, so there is no silence to open one.
//!
//! # Which prints are left out
//!
//! Venue `D` with condition `T`, from the first `D` print after the silence (while the flood is
//! still being counted, provisionally) to the end of a confirmed window. If the flood never comes
//! the probe lapses after [`PROBE_S`] seconds: those few seconds of real-time reports were left
//! out of the filtered tape only, which is the shadow variant's whole cost there.
//!
//! O(1) per print (a counter and a ring of per-second counts).

use std::collections::VecDeque;

/// Seconds of `D` silence that make a following flood a TRF opening.
pub const SILENCE_S: i64 = 300;
/// At most this many `D` prints in those seconds still counts as silent.
pub const SILENCE_MAX: u64 = 50;
/// Seconds after the first `D` print within which the flood must arrive.
pub const PROBE_S: i64 = 10;
/// Form T `D` prints in the probe that confirm a batch (100 a second; real time is ~60).
pub const FLOOD_MIN: u64 = 1_000;
/// A confirmed window lasts at least this long, at most [`MAX_S`].
pub const MIN_S: i64 = 60;
pub const MAX_S: i64 = 300;
/// The window ends when the last `END_WINDOW_S` seconds held fewer than `END_COUNT` Form T
/// prints (50 a second).
pub const END_WINDOW_S: i64 = 30;
pub const END_COUNT: u64 = 1_500;

/// One closed batch window, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// First `D` print after the silence and the last second of the window, epoch seconds.
    pub start_sec: i64,
    pub end_sec: i64,
    /// Form T `D` prints inside it, and all `D` prints.
    pub form_t: u64,
    pub all_d: u64,
    /// `"rate"` (the flood died down) or `"max"` ([`MAX_S`] reached).
    pub reason: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// The TRF is flowing (or nothing is known yet).
    Idle,
    /// The first `D` print after a silence: counting the flood.
    Probe { start: i64, form_t: u64, all_d: u64 },
    /// A confirmed batch window.
    Batch { start: i64, form_t: u64, all_d: u64 },
}

/// The market-wide detector (module docs).
#[derive(Debug)]
pub struct BatchDetector {
    state: State,
    /// `(second, D prints)` for the seconds with `D` prints in the last [`SILENCE_S`], oldest
    /// first, and their total.
    d_secs: VecDeque<(i64, u64)>,
    d_total: u64,
    /// `(second, Form T D prints)` for the last [`END_WINDOW_S`] while a window is open.
    t_secs: VecDeque<(i64, u64)>,
    t_total: u64,
    /// The first second this detector saw anything (silence needs [`SILENCE_S`] watched).
    since: Option<i64>,
    closed: Vec<Window>,
}

impl Default for BatchDetector {
    fn default() -> Self {
        Self {
            state: State::Idle,
            d_secs: VecDeque::new(),
            d_total: 0,
            t_secs: VecDeque::new(),
            t_total: 0,
            since: None,
            closed: Vec::new(),
        }
    }
}

/// Whether a print is the kind a batch is made of: venue `D`, condition Form T.
pub fn is_form_t_trf(exchange: &str, conditions: &[String]) -> bool {
    exchange == "D" && conditions.iter().any(|c| c == "T")
}

fn push(q: &mut VecDeque<(i64, u64)>, total: &mut u64, sec: i64) {
    match q.back_mut() {
        Some((s, n)) if *s == sec => *n += 1,
        _ => q.push_back((sec, 1)),
    }
    *total += 1;
}

fn trim(q: &mut VecDeque<(i64, u64)>, total: &mut u64, keep_from: i64) {
    while q.front().is_some_and(|(s, _)| *s < keep_from) {
        let (_, n) = q.pop_front().expect("front");
        *total -= n;
    }
}

impl BatchDetector {
    /// Time passing without a print (the tick): closes a window whose flood stopped altogether.
    pub fn tick(&mut self, sec: i64) {
        self.since.get_or_insert(sec);
        self.advance(sec);
    }

    /// One print at `ts_ns`; returns whether the filtered tape leaves it out (module docs).
    pub fn observe(&mut self, ts_ns: i64, exchange: &str, conditions: &[String]) -> bool {
        let sec = ts_ns.div_euclid(1_000_000_000);
        let since = *self.since.get_or_insert(sec);
        self.advance(sec);
        if exchange != "D" {
            return false;
        }
        let form_t = is_form_t_trf(exchange, conditions);
        // Silence is judged on the seconds before this print, and only once they were watched.
        trim(&mut self.d_secs, &mut self.d_total, sec - SILENCE_S);
        let silent = sec - since >= SILENCE_S && self.d_total <= SILENCE_MAX;
        push(&mut self.d_secs, &mut self.d_total, sec);
        if self.state == State::Idle && silent {
            self.state = State::Probe {
                start: sec,
                form_t: 0,
                all_d: 0,
            };
            self.t_secs.clear();
            self.t_total = 0;
        }
        match &mut self.state {
            State::Idle => false,
            State::Probe {
                start,
                form_t: t,
                all_d,
            } => {
                *all_d += 1;
                if form_t {
                    *t += 1;
                    push(&mut self.t_secs, &mut self.t_total, sec);
                }
                if *t >= FLOOD_MIN {
                    self.state = State::Batch {
                        start: *start,
                        form_t: *t,
                        all_d: *all_d,
                    };
                }
                form_t
            }
            State::Batch {
                form_t: t, all_d, ..
            } => {
                *all_d += 1;
                if form_t {
                    *t += 1;
                    push(&mut self.t_secs, &mut self.t_total, sec);
                }
                form_t
            }
        }
    }

    /// Ends a lapsed probe or a finished window as of `sec`.
    fn advance(&mut self, sec: i64) {
        match self.state {
            State::Idle => {}
            State::Probe { start, .. } => {
                if sec - start >= PROBE_S {
                    self.state = State::Idle;
                }
            }
            State::Batch {
                start,
                form_t,
                all_d,
            } => {
                trim(&mut self.t_secs, &mut self.t_total, sec - END_WINDOW_S + 1);
                let age = sec - start;
                let reason = if age >= MAX_S {
                    Some("max")
                } else if age >= MIN_S && self.t_total < END_COUNT {
                    Some("rate")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    self.closed.push(Window {
                        start_sec: start,
                        end_sec: sec,
                        form_t,
                        all_d,
                        reason,
                    });
                    self.state = State::Idle;
                }
            }
        }
    }

    /// Whether a batch window is open (confirmed) right now.
    pub fn in_batch(&self) -> bool {
        matches!(self.state, State::Batch { .. })
    }

    /// The windows closed since the last call (each once).
    pub fn take_closed(&mut self) -> Vec<Window> {
        std::mem::take(&mut self.closed)
    }

    /// Forgets everything (a replay that went back in time).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;
    fn t() -> Vec<String> {
        vec!["@".into(), "T".into()]
    }
    fn plain() -> Vec<String> {
        vec!["@".into()]
    }

    /// A day: `quiet` seconds of exchange-only trading (no D), then a flood of `per_sec` Form T
    /// D prints a second for `flood_s` seconds, then `tail_per_sec` real-time ones.
    fn run(
        d: &mut BatchDetector,
        start: i64,
        quiet: i64,
        per_sec: u64,
        flood_s: i64,
        tail_per_sec: u64,
        tail_s: i64,
    ) -> (u64, u64) {
        let (mut dropped, mut kept) = (0, 0);
        for s in 0..quiet {
            assert!(!d.observe((start + s) * S, "V", &plain()));
        }
        let mut sec = start + quiet;
        for (n, len) in [(per_sec, flood_s), (tail_per_sec, tail_s)] {
            for _ in 0..len {
                d.tick(sec);
                for k in 0..n {
                    if d.observe(sec * S + k as i64, "D", &t()) {
                        dropped += 1;
                    } else {
                        kept += 1;
                    }
                }
                sec += 1;
            }
        }
        (dropped, kept)
    }

    #[test]
    fn a_batch_after_silence_is_found_and_closes_when_the_flood_ends() {
        // 04:00 today: silence overnight, ~1,000 Form T reports a second for 2 minutes, then
        // real time at 40 a second.
        let mut d = BatchDetector::default();
        let (dropped, kept) = run(&mut d, 1_000, 600, 1_000, 120, 40, 120);
        let w = d.take_closed();
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0].start_sec, 1_600);
        assert_eq!(w[0].reason, "rate");
        // The flood is left out; the window ends within the end window of the flood stopping.
        assert_eq!(dropped, 120_000 + 40 * (w[0].end_sec - 1_720) as u64);
        assert!(w[0].end_sec - 1_720 <= END_WINDOW_S, "{w:?}");
        assert!(kept > 0 && !d.in_batch());
    }

    #[test]
    fn the_old_08_00_batch_is_found_the_same_way() {
        // Before 2026-03-30: exchange trading from 04:00, D silent until 08:00, then 5,000 a
        // second; the window is capped at MAX_S.
        let mut d = BatchDetector::default();
        run(&mut d, 0, 4 * 3600, 5_000, 400, 60, 60);
        let w = d.take_closed();
        assert_eq!(w.len(), 1);
        assert_eq!(
            (w[0].start_sec, w[0].end_sec - w[0].start_sec, w[0].reason),
            (4 * 3600, MAX_S, "max")
        );
    }

    #[test]
    fn real_time_reports_into_a_busy_open_are_not_a_batch() {
        // Today's 08:00 (or any busy minute): D already flowing at 60 a second, then 300 a
        // second. No silence, no window, nothing left out.
        let mut d = BatchDetector::default();
        let (dropped, _) = run(&mut d, 0, 0, 60, 600, 300, 120);
        assert_eq!(dropped, 0);
        assert!(d.take_closed().is_empty());
    }

    #[test]
    fn a_trf_opening_without_a_flood_lapses_after_the_probe() {
        // Silence, then a slow real-time trickle (20 a second): the probe drops the first
        // PROBE_S seconds only, and no window is logged.
        let mut d = BatchDetector::default();
        let (dropped, kept) = run(&mut d, 0, 600, 20, 60, 0, 0);
        assert_eq!(dropped, 20 * PROBE_S as u64);
        assert_eq!(kept, 20 * (60 - PROBE_S) as u64);
        assert!(d.take_closed().is_empty());
    }

    #[test]
    fn a_fresh_detector_needs_the_silence_watched_before_it_calls_a_batch() {
        // A restart in the middle of an after-hours flood of real-time reports: nothing left out.
        let mut d = BatchDetector::default();
        let (dropped, _) = run(&mut d, 0, 0, 400, 120, 0, 0);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn only_venue_d_form_t_is_left_out() {
        let mut d = BatchDetector::default();
        run(&mut d, 0, 600, 1_200, 5, 0, 0);
        assert!(d.in_batch());
        assert!(!d.observe(605 * S, "D", &plain()));
        assert!(!d.observe(605 * S, "V", &t()));
        assert!(d.observe(605 * S, "D", &t()));
    }
}
