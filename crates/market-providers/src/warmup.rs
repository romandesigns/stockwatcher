//! The server-side trigger for warming up intraday band entrants
//! (`docs/stock-context/daily-baseline-plan.md` §2, market_archive::warmup).
//!
//! A stock that enters the $0.10–$25 band intraday without an in-band close in the last 5 dates
//! has no daily-bar snapshot or announcement coverage for that day. The server notices it on the
//! market-event path, where nothing may block or touch the disk, so it calls
//! [`WarmupQueue::request`]: O(1), never waits for a lock (`try_lock`), bounded, deduplicated per
//! symbol and date. A background task calls [`WarmupQueue::flush`] to file the requests in the
//! archive's inbox, which `swimport maintain` works off on its refresh ticks (bounded per tick).
//! Until then, and if the symbol's history is short anyway, its evidence stays explicitly
//! unavailable / insufficient; nothing is guessed.
//!
//! Not wired into the server yet (increment 2b): this is the API it will call.

use chrono::{DateTime, NaiveDate, Utc};
use market_archive::{Archive, ArchiveError, WarmupRequest};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, TryLockError};

/// What [`WarmupQueue::request`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enqueue {
    Queued,
    /// Already requested for that date (queued or flushed): nothing to do.
    Known,
    /// The queue is at capacity; the request is dropped and counted.
    Full,
    /// Another thread held the lock this instant; try again on a later print.
    Busy,
}

#[derive(Default)]
struct State {
    queue: VecDeque<WarmupRequest>,
    /// Symbols already asked for, **per session date**. Keyed by date so the day's dedup can be
    /// dropped when the date rolls: a single flat set would accumulate one entry per symbol and
    /// date for the lifetime of the process, which a live market is meant to have a long one of.
    seen: BTreeMap<NaiveDate, BTreeSet<String>>,
    dropped: u64,
    /// Dates whose dedup state was dropped when a later date was asked about.
    rolled: u64,
}

/// Bounded, non-blocking, cloneable (shared) warmup request queue.
#[derive(Clone)]
pub struct WarmupQueue {
    state: Arc<Mutex<State>>,
    capacity: usize,
}

impl WarmupQueue {
    pub fn new(capacity: usize) -> Self {
        WarmupQueue {
            state: Arc::default(),
            capacity: capacity.max(1),
        }
    }

    /// Asks for `symbol` on session `d` to be warmed up. Safe on the market-event thread.
    ///
    /// Asking about a later date drops the dedup state of the earlier ones: a request only ever
    /// concerns one session date, and the archive inbox is the durable record of what was filed.
    pub fn request(&self, symbol: &str, d: NaiveDate, reason: &str, at: DateTime<Utc>) -> Enqueue {
        let mut s = match self.state.try_lock() {
            Ok(s) => s,
            Err(TryLockError::WouldBlock) => return Enqueue::Busy,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
        };
        if s.seen.keys().next_back().is_some_and(|newest| *newest < d) {
            let old = s.seen.len();
            s.seen.retain(|k, _| *k >= d);
            s.rolled += (old - s.seen.len()) as u64;
        }
        if s.seen.get(&d).is_some_and(|set| set.contains(symbol)) {
            return Enqueue::Known;
        }
        if s.queue.len() >= self.capacity {
            s.dropped += 1;
            return Enqueue::Full;
        }
        s.seen.entry(d).or_default().insert(symbol.to_string());
        s.queue.push_back(WarmupRequest {
            symbol: symbol.to_string(),
            session_date: d,
            requested_at: at,
            reason: reason.to_string(),
            outcome: None,
            done_at: None,
            attempts: 0,
            last_error: None,
        });
        Enqueue::Queued
    }

    /// Requests dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.state.lock().map(|s| s.dropped).unwrap_or(0)
    }

    /// Dates whose dedup state was dropped as the session date moved on.
    pub fn rolled(&self) -> u64 {
        self.state.lock().map(|s| s.rolled).unwrap_or(0)
    }

    /// Symbols already asked for on `d` (buffered or filed). A caller that selects candidates
    /// uses this to skip them **before** taking its request budget: taking the budget first and
    /// discarding the known ones afterwards means a batch that never resolves blocks every
    /// candidate behind it for the rest of the day.
    pub fn asked_for(&self, d: NaiveDate) -> BTreeSet<String> {
        self.state
            .lock()
            .map(|s| s.seen.get(&d).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    /// Symbols held in memory, waiting to be filed.
    pub fn pending(&self) -> usize {
        self.state.lock().map(|s| s.queue.len()).unwrap_or(0)
    }

    /// Dedup entries held, all dates (bounded by the symbols asked for on the current date).
    pub fn tracked(&self) -> usize {
        self.state
            .lock()
            .map(|s| s.seen.values().map(BTreeSet::len).sum())
            .unwrap_or(0)
    }

    /// Files every queued request in the archive inbox (call from a background task, never the
    /// market-event thread). Returns how many were new in the inbox. A request whose write fails
    /// is put back for the next flush.
    pub fn flush(&self, archive: &Archive) -> Result<usize, ArchiveError> {
        let batch: Vec<WarmupRequest> = {
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            s.queue.drain(..).collect()
        };
        let mut new = 0;
        let mut iter = batch.into_iter();
        while let Some(r) = iter.next() {
            match archive.request_warmup(&r) {
                Ok(true) => new += 1,
                Ok(false) => {}
                Err(e) => {
                    let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                    s.queue.push_front(r);
                    s.queue.extend(iter);
                    return Err(e);
                }
            }
        }
        Ok(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_archive::ArchiveConfig;

    #[test]
    fn bounded_deduplicated_and_flushed_to_the_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().to_string();
        let a = Archive::new(
            ArchiveConfig::from_lookup(|k| match k {
                "ARCHIVE_ROOT" => Some(root.clone()),
                "ARCHIVE_MIN_FREE_MB" => Some("0".into()),
                _ => None,
            })
            .unwrap(),
        );
        let d = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
        let t = Utc::now();
        let q = WarmupQueue::new(2);
        assert_eq!(q.request("NEWA", d, "band-entry", t), Enqueue::Queued);
        assert_eq!(q.request("NEWA", d, "band-entry", t), Enqueue::Known);
        assert_eq!(q.request("NEWB", d, "band-entry", t), Enqueue::Queued);
        assert_eq!(q.request("NEWC", d, "band-entry", t), Enqueue::Full);
        assert_eq!(q.dropped(), 1);
        // A lock held elsewhere never blocks the caller.
        {
            let _held = q.state.lock().unwrap();
            assert_eq!(q.request("NEWD", d, "band-entry", t), Enqueue::Busy);
        }
        assert_eq!(q.flush(&a).unwrap(), 2);
        let pending = a.pending_warmups(d, 10).unwrap();
        assert_eq!(
            pending
                .iter()
                .map(|r| r.symbol.as_str())
                .collect::<Vec<_>>(),
            ["NEWA", "NEWB"]
        );
        // Still known after the flush: no second request for the same day.
        assert_eq!(q.request("NEWA", d, "band-entry", t), Enqueue::Known);
        // Closing one moves it out of the open list; asking again is a no-op.
        a.finish_warmup(pending[0].clone(), "ok".into(), t).unwrap();
        assert_eq!(a.pending_warmups(d, 10).unwrap().len(), 1);
        assert!(!a.request_warmup(&pending[0]).unwrap());
    }
}
