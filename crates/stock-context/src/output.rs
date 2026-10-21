//! Bounded output journals for a long-running engine.
//!
//! The engine appends panel snapshots and setup-log entries; its caller takes them. A caller that
//! stops taking — no client subscribed, a stalled writer, a path that was never wired up — must
//! cost bounded memory and leave a visible count of what went, never silent growth and never a
//! silent loss that a later reader would mistake for "nothing happened".

use serde::Serialize;
use std::collections::VecDeque;

/// Panel snapshots an engine holds before it drops the oldest.
///
/// A snapshot is published only when the panel changed, so this is roughly an hour of a busy
/// whole-market open at a 15 s grid. The latest snapshot is kept separately
/// ([`crate::Engine::panel`]), so a client's view never depends on this bound.
pub const PANEL_LOG_MAX: usize = 1024;

/// Setup-log entries an engine holds before it drops the oldest (transitions and level versions
/// across every member).
pub const SETUP_LOG_MAX: usize = 4096;

/// What the engine produced, handed over, and dropped.
///
/// `*_dropped` is the honest part: a journal that dropped anything is not a complete record of
/// the run, and [`OutputTelemetry::complete`] says so rather than letting a reader assume it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct OutputTelemetry {
    /// Panel snapshots the engine published (each one a change).
    pub panels_published: u64,
    /// Panel snapshots a caller took.
    pub panels_taken: u64,
    /// Panel snapshots dropped because the bound was reached before a caller took them.
    pub panels_dropped: u64,
    /// Panel snapshots held now.
    pub panels_held: usize,
    pub setup_entries_logged: u64,
    pub setup_entries_taken: u64,
    pub setup_entries_dropped: u64,
    pub setup_entries_held: usize,
    pub panel_log_max: usize,
    pub setup_log_max: usize,
}

impl OutputTelemetry {
    /// Nothing has been dropped: what a caller has taken, plus what is held, is the whole run.
    pub fn complete(&self) -> bool {
        self.panels_dropped == 0 && self.setup_entries_dropped == 0
    }

    /// Outputs dropped, both journals together.
    pub fn dropped(&self) -> u64 {
        self.panels_dropped + self.setup_entries_dropped
    }
}

/// An append-only journal with a hard bound: at the bound the oldest row goes and is counted.
///
/// The oldest is dropped rather than the newest, because the newest rows are the ones a stalled
/// consumer still needs (the current state of the market), and because the alternative — refusing
/// the append — would make the engine's own progress depend on its consumer.
#[derive(Clone, Debug)]
pub struct BoundedLog<T> {
    rows: VecDeque<T>,
    max: usize,
    logged: u64,
    taken: u64,
    dropped: u64,
}

impl<T> BoundedLog<T> {
    pub fn new(max: usize) -> Self {
        BoundedLog {
            rows: VecDeque::new(),
            max: max.max(1),
            logged: 0,
            taken: 0,
            dropped: 0,
        }
    }

    pub fn push(&mut self, row: T) {
        self.logged += 1;
        while self.rows.len() >= self.max {
            self.rows.pop_front();
            self.dropped += 1;
        }
        self.rows.push_back(row);
    }

    /// Everything held, oldest first; the journal is empty afterwards.
    pub fn take(&mut self) -> Vec<T> {
        let out: Vec<T> = self.rows.drain(..).collect();
        self.taken += out.len() as u64;
        out
    }

    pub fn held(&self) -> usize {
        self.rows.len()
    }
    pub fn logged(&self) -> u64 {
        self.logged
    }
    pub fn taken(&self) -> u64 {
        self.taken
    }
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
    pub fn max(&self) -> usize {
        self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oldest_goes_and_is_counted() {
        let mut l: BoundedLog<u32> = BoundedLog::new(3);
        for i in 0..5 {
            l.push(i);
        }
        assert_eq!(l.held(), 3);
        assert_eq!(l.dropped(), 2);
        assert_eq!(l.logged(), 5);
        assert_eq!(l.take(), vec![2, 3, 4], "the newest are the ones kept");
        assert_eq!(l.taken(), 3);
        assert_eq!(l.held(), 0);
        assert_eq!(l.logged(), l.taken() + l.dropped() + l.held() as u64);
    }

    #[test]
    fn a_zero_bound_still_holds_one_row() {
        let mut l: BoundedLog<u32> = BoundedLog::new(0);
        l.push(1);
        assert_eq!(l.take(), vec![1]);
    }
}
