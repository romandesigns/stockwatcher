//! Baseline observation and latency capture for stock context.
//!
//! # What this is, and what it is not
//!
//! This is the capture scaffolding the agreed measurement program needs **before** any forward
//! cohort accumulates: the segment timestamps of a panel's journey off the engine and onto the
//! wire, with honest counters for what was captured and what was dropped. Building it first is
//! the point — metrics chosen after seeing outcomes are not evidence, and an observation nobody
//! recorded cannot be recovered later.
//!
//! It is **not** a latency or efficacy claim. Nothing here is a guarantee, a service level or a
//! comparison with any other product. A summary with fewer than [`MIN_SAMPLES`] timed
//! observations reports [`LatencySummary::InsufficientData`] and no percentile at all, and the
//! percentiles it does report are over a bounded window of the most recent samples, which it
//! states.
//!
//! # Where the work happens
//!
//! [`ObservationSink::capture`] is called under the market's core lock, so it is a bounded
//! `try_send` and two atomics: never a wait, never disk, never a provider, and never a drop that
//! goes uncounted. [`ObservationSink::absorb`] runs with no other lock held and moves whatever is
//! queued into the summary.
//!
//! # Honest clocks
//!
//! A decision's cutoff is the **run's** clock. In a replay that is the simulated day, so the
//! difference between it and the wall clock is not a latency — those observations are counted and
//! never timed ([`ObservationReport::untimed_replay`]). Only a live run's segments are timed.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

/// Observations the queue holds before `capture` starts dropping them (and counting the drops).
pub(crate) const QUEUE_CAPACITY: usize = 4_096;

/// Timed samples a segment keeps: the percentiles are over the most recent `SAMPLE_WINDOW`.
pub(crate) const SAMPLE_WINDOW: usize = 2_048;

/// Quotes-task ticks between observation log lines (the task ticks about once a second).
pub(crate) const LOG_EVERY_TICKS: u64 = 60;

/// Timed samples a segment needs before any percentile is reported.
///
/// A reporting floor so a handful of observations cannot read as a measurement; it is not a
/// statistical power calculation, and clearing it is not evidence of anything by itself.
pub(crate) const MIN_SAMPLES: usize = 100;

/// One thing that happened to a panel, with the times it happened at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Taken off the engine's output journal by the viewer-independent drain. Its time is read
    /// **after** the core lock was acquired, so the segment includes waiting for the lock: a
    /// timestamp taken before the lock would under-report precisely under contention.
    Drained,
    /// Handed to this server's broadcast channel as a `ross.update`. The segment ends at the
    /// server's own send: nothing here measures network transport, a client's receipt or what a
    /// browser rendered, and none of those may be inferred from it.
    Published,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) stage: Stage,
    /// The engine's decision sequence this panel carries.
    pub(crate) revision: u64,
    /// End of the bar the panel covers, and the data barrier, both on the run's clock.
    pub(crate) bar_end: i64,
    pub(crate) cutoff: i64,
    pub(crate) rows: usize,
    /// `live` or `replay`: only a live run's clock is comparable with the wall clock.
    pub(crate) live: bool,
    /// Wall clock (UTC ns) when the stage happened.
    pub(crate) at: i64,
}

impl Observation {
    /// One stage of a panel snapshot at wall time `at`.
    pub(crate) fn of(stage: Stage, p: &stock_context::PanelSnapshot, at: i64) -> Self {
        Observation {
            stage,
            revision: p.revision,
            bar_end: p.bar_end,
            cutoff: p.cutoff,
            rows: p.rows.len(),
            live: p.source == "live",
            at,
        }
    }
}

/// The wall clock in UTC nanoseconds. Only ever compared with a **live** run's clock: a replay's
/// own clock is the simulated day, and the difference is not a latency.
pub(crate) fn wall_now() -> i64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX)
}

/// A bounded segment of timed samples, newest `SAMPLE_WINDOW` kept.
#[derive(Clone, Debug, Default)]
struct Segment {
    /// Samples ever taken (not just the ones in the window).
    taken: u64,
    window: Vec<i64>,
    next: usize,
    cap: usize,
}

impl Segment {
    fn new(cap: usize) -> Self {
        Segment {
            taken: 0,
            window: Vec::with_capacity(cap.min(64)),
            next: 0,
            cap: cap.max(1),
        }
    }

    fn push(&mut self, ns: i64) {
        self.taken += 1;
        if self.window.len() < self.cap {
            self.window.push(ns);
        } else {
            self.window[self.next] = ns;
            self.next = (self.next + 1) % self.cap;
        }
    }

    fn summary(&self) -> LatencySummary {
        if self.window.len() < MIN_SAMPLES {
            return LatencySummary::InsufficientData {
                samples: self.window.len(),
                min_samples: MIN_SAMPLES,
            };
        }
        let mut s = self.window.clone();
        s.sort_unstable();
        let at = |q: f64| s[(((s.len() - 1) as f64) * q).round() as usize];
        LatencySummary::Measured {
            taken: self.taken,
            window: s.len(),
            min_ns: s[0],
            p50_ns: at(0.50),
            p95_ns: at(0.95),
            p99_ns: at(0.99),
            max_ns: s[s.len() - 1],
        }
    }
}

/// What a segment's samples say, or why they say nothing yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LatencySummary {
    /// Under [`MIN_SAMPLES`]: no percentile is reported, by design.
    InsufficientData { samples: usize, min_samples: usize },
    /// Percentiles over the most recent `window` samples of `taken` ever taken. A description of
    /// what this process observed, not a guarantee.
    Measured {
        taken: u64,
        window: usize,
        min_ns: i64,
        p50_ns: i64,
        p95_ns: i64,
        p99_ns: i64,
        max_ns: i64,
    },
}

impl LatencySummary {
    pub(crate) fn measured(&self) -> bool {
        matches!(self, LatencySummary::Measured { .. })
    }

    /// One log-friendly line. `insufficient_data` is said out loud, never implied by absence.
    pub(crate) fn line(&self) -> String {
        match self {
            LatencySummary::InsufficientData {
                samples,
                min_samples,
            } => format!("insufficient_data ({samples}/{min_samples} samples)"),
            LatencySummary::Measured {
                taken,
                window,
                p50_ns,
                p95_ns,
                p99_ns,
                max_ns,
                ..
            } => format!(
                "p50 {:.0}ms p95 {:.0}ms p99 {:.0}ms max {:.0}ms (window {window} of {taken})",
                *p50_ns as f64 / 1e6,
                *p95_ns as f64 / 1e6,
                *p99_ns as f64 / 1e6,
                *max_ns as f64 / 1e6
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObservationReport {
    pub(crate) captured: u64,
    /// Captures refused because the queue was full. The reason a report is incomplete.
    pub(crate) dropped: u64,
    pub(crate) absorbed: u64,
    pub(crate) queue_capacity: usize,
    /// Observations counted but deliberately not timed: a replay's cutoff is its own simulated
    /// clock, so the gap to the wall clock is not a latency.
    pub(crate) untimed_replay: u64,
    /// Barrier (`cutoff`) to the viewer-independent drain, live runs only.
    pub(crate) barrier_to_drain: LatencySummary,
    /// Barrier to the `ross.update` broadcast, live runs only.
    pub(crate) barrier_to_publish: LatencySummary,
}

impl ObservationReport {
    /// Nothing was dropped, so what was absorbed is every observation that was captured.
    pub(crate) fn complete(&self) -> bool {
        self.dropped == 0
    }
}

#[derive(Debug, Default)]
struct Meter {
    absorbed: u64,
    untimed_replay: u64,
    barrier_to_drain: Segment,
    barrier_to_publish: Segment,
}

/// The bounded queue and the summary behind it.
pub(crate) struct ObservationSink {
    tx: SyncSender<Observation>,
    /// Taken only by `absorb`, never together with the market's core lock.
    rx: Mutex<Receiver<Observation>>,
    meter: Mutex<Meter>,
    captured: AtomicU64,
    dropped: AtomicU64,
    capacity: usize,
}

impl ObservationSink {
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (tx, rx) = sync_channel(capacity);
        ObservationSink {
            tx,
            rx: Mutex::new(rx),
            meter: Mutex::new(Meter {
                barrier_to_drain: Segment::new(SAMPLE_WINDOW),
                barrier_to_publish: Segment::new(SAMPLE_WINDOW),
                ..Meter::default()
            }),
            captured: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            capacity,
        }
    }

    /// Records one observation. Safe to call under the core lock: a bounded `try_send` and two
    /// atomics, never a wait, never I/O. A full queue is a counted drop, never a block and never
    /// a silent loss.
    pub(crate) fn capture(&self, o: Observation) {
        match self.tx.try_send(o) {
            Ok(()) => {
                self.captured.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Moves everything queued into the summary. Call with no other lock held. Returns how many
    /// observations moved.
    pub(crate) fn absorb(&self) -> usize {
        let rx = self.rx.lock().expect("observation receiver lock");
        let mut m = self.meter.lock().expect("observation meter lock");
        let mut n = 0;
        while let Ok(o) = rx.try_recv() {
            n += 1;
            m.absorbed += 1;
            if !o.live {
                m.untimed_replay += 1;
                continue;
            }
            let ns = o.at - o.cutoff;
            // A negative segment would mean the wall clock is behind the run clock; counted as an
            // observation, never folded into a percentile as if it were a measured delay.
            if ns < 0 {
                continue;
            }
            match o.stage {
                Stage::Drained => m.barrier_to_drain.push(ns),
                Stage::Published => m.barrier_to_publish.push(ns),
            }
        }
        n
    }

    pub(crate) fn report(&self) -> ObservationReport {
        let m = self.meter.lock().expect("observation meter lock");
        ObservationReport {
            captured: self.captured.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            absorbed: m.absorbed,
            queue_capacity: self.capacity,
            untimed_replay: m.untimed_replay,
            barrier_to_drain: m.barrier_to_drain.summary(),
            barrier_to_publish: m.barrier_to_publish.summary(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(stage: Stage, live: bool, cutoff: i64, at: i64) -> Observation {
        Observation {
            stage,
            revision: 1,
            bar_end: cutoff - 2_000_000_000,
            cutoff,
            rows: 1,
            live,
            at,
        }
    }

    #[test]
    fn a_full_queue_drops_and_says_so() {
        let s = ObservationSink::new(4);
        for k in 0..10 {
            s.capture(obs(Stage::Drained, true, 0, k));
        }
        let r = s.report();
        assert_eq!((r.captured, r.dropped), (4, 6), "{r:?}");
        assert!(!r.complete());
        assert_eq!(s.absorb(), 4);
        // With room again, capture resumes; the drop count is cumulative and never reset.
        s.capture(obs(Stage::Drained, true, 0, 11));
        let r = s.report();
        assert_eq!((r.captured, r.dropped, r.absorbed), (5, 6, 4));
    }

    #[test]
    fn a_few_samples_are_insufficient_data_not_a_latency() {
        let s = ObservationSink::new(QUEUE_CAPACITY);
        for k in 0..(MIN_SAMPLES as i64 - 1) {
            s.capture(obs(Stage::Drained, true, 0, k * 1_000_000));
        }
        s.absorb();
        let r = s.report();
        assert_eq!(
            r.barrier_to_drain,
            LatencySummary::InsufficientData {
                samples: MIN_SAMPLES - 1,
                min_samples: MIN_SAMPLES,
            }
        );
        assert!(r.barrier_to_drain.line().starts_with("insufficient_data"));
        assert!(!r.barrier_to_drain.measured());
        assert!(r.complete());

        // One more clears the reporting floor, and the percentiles describe the samples taken.
        s.capture(obs(Stage::Drained, true, 0, MIN_SAMPLES as i64 * 1_000_000));
        s.absorb();
        match s.report().barrier_to_drain {
            LatencySummary::Measured {
                taken,
                window,
                min_ns,
                p50_ns,
                max_ns,
                ..
            } => {
                assert_eq!((taken, window), (MIN_SAMPLES as u64, MIN_SAMPLES));
                assert_eq!(min_ns, 0);
                assert_eq!(max_ns, MIN_SAMPLES as i64 * 1_000_000);
                assert!(p50_ns > 0 && p50_ns < max_ns, "p50 {p50_ns}");
            }
            other => panic!("expected a measurement, got {other:?}"),
        }
        assert!(!s.report().barrier_to_publish.measured());
    }

    #[test]
    fn a_replay_is_counted_but_never_timed() {
        let s = ObservationSink::new(QUEUE_CAPACITY);
        // A replayed day in the past: the wall clock is far ahead of the run clock.
        for k in 0..MIN_SAMPLES as i64 {
            s.capture(obs(Stage::Drained, false, k, 9_000_000_000_000_000_000));
        }
        s.absorb();
        let r = s.report();
        assert_eq!(r.untimed_replay, MIN_SAMPLES as u64);
        assert!(
            !r.barrier_to_drain.measured(),
            "a simulated clock is not a latency: {r:?}"
        );
    }

    #[test]
    fn the_sample_window_is_bounded_and_the_total_is_kept() {
        let mut seg = Segment::new(8);
        for k in 0..100 {
            seg.push(k);
        }
        match seg.summary() {
            LatencySummary::InsufficientData { samples, .. } => {
                assert_eq!(
                    samples, 8,
                    "the window is bounded, so it stays insufficient"
                )
            }
            other => panic!("expected insufficient_data, got {other:?}"),
        }
        assert_eq!(seg.taken, 100, "the total is still counted");
    }
}
