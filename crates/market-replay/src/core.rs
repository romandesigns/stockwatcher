//! The synchronous, deterministic replay core. The async [`crate::driver`] only adds pacing and
//! commands on top; tests drive this directly.

use crate::FeedItem;
use crate::marks::MarkClock;
use crate::merge::Merger;
use crate::source::{EventSource, day_bounds};
use market_events::UnixNanos;
use std::sync::Arc;

pub struct ReplayCore {
    source: Arc<dyn EventSource>,
    merger: Merger,
    marks: MarkClock,
    now: i64,
    start: i64,
    end: i64,
    emitted: u64,
    finished: bool,
}

impl ReplayCore {
    pub fn new(source: Arc<dyn EventSource>, mark_step_ns: i64) -> Result<Self, String> {
        let (start, end) = day_bounds(source.day())
            .ok_or_else(|| format!("{} is not a trading day", source.day()))?;
        let merger = Merger::new(source.open()?);
        Ok(ReplayCore {
            source,
            merger,
            marks: MarkClock::new(mark_step_ns),
            now: start.nanos(),
            start: start.nanos(),
            end: end.nanos(),
            emitted: 0,
            finished: false,
        })
    }

    pub fn source(&self) -> &Arc<dyn EventSource> {
        &self.source
    }

    pub fn now(&self) -> UnixNanos {
        UnixNanos(self.now)
    }

    pub fn start(&self) -> UnixNanos {
        UnixNanos(self.start)
    }

    pub fn end(&self) -> UnixNanos {
        UnixNanos(self.end)
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Delivery time of the next event, if any.
    pub fn next_delivery(&self) -> Option<UnixNanos> {
        self.merger.peek_delivery().map(UnixNanos)
    }

    /// Re-opens the source and rewinds to the start of the day.
    pub fn reset(&mut self) -> Result<(), String> {
        self.merger = Merger::new(self.source.open()?);
        self.marks.reset();
        self.now = self.start;
        self.emitted = 0;
        self.finished = false;
        Ok(())
    }

    fn emit_next(&mut self, out: &mut Vec<FeedItem>) -> Result<bool, String> {
        let Some((d, e)) = self.merger.pop() else {
            return match self.merger.take_error() {
                Some(err) => Err(err),
                None => Ok(false),
            };
        };
        if let Some(m) = self.marks.mark(d) {
            out.push(FeedItem::Advance(m));
        }
        out.push(FeedItem::Event(e));
        self.now = self.now.max(d);
        self.emitted += 1;
        Ok(true)
    }

    fn check_finished(&mut self) {
        if self.now >= self.end && self.merger.peek_delivery().is_none_or(|d| d > self.end) {
            self.finished = true;
        }
    }

    /// Emits (into `out`) every event with delivery ≤ `until` (clamped to the end of the day), at
    /// most `max_events` of them, with engine marks in between. Returns `true` when `until` was
    /// reached: now = `until` and a final mark has been emitted. `false` means the batch limit
    /// stopped it; now is the last delivered event and the caller should call again.
    pub fn advance(
        &mut self,
        until: UnixNanos,
        max_events: usize,
        out: &mut Vec<FeedItem>,
    ) -> Result<bool, String> {
        let until = until.nanos().min(self.end).max(self.now);
        let mut n = 0;
        while let Some(d) = self.merger.peek_delivery() {
            if d > until {
                break;
            }
            if n >= max_events {
                return Ok(false);
            }
            self.emit_next(out)?;
            n += 1;
        }
        if let Some(err) = self.merger.take_error() {
            return Err(err);
        }
        self.now = until;
        if let Some(m) = self.marks.mark(until) {
            out.push(FeedItem::Advance(m));
        }
        self.check_finished();
        Ok(true)
    }

    /// Emits exactly the next event (if it is within the day). Returns `false` at the end.
    pub fn step(&mut self, out: &mut Vec<FeedItem>) -> Result<bool, String> {
        match self.merger.peek_delivery() {
            Some(d) if d <= self.end => {
                self.emit_next(out)?;
                self.check_finished();
                Ok(true)
            }
            _ => {
                self.advance(UnixNanos(self.end), 0, out)?;
                Ok(false)
            }
        }
    }
}
