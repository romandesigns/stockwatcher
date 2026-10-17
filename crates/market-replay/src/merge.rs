//! k-way merge of per-symbol arrival-ordered streams (see the crate docs for the key).

use crate::source::EventStream;
use market_events::{MarketEvent, Source};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct MergeKey {
    delivery: i64,
    event_ts: i64,
    seq: u64,
    exchange: String,
    source: Source,
    symbol: String,
    stream: usize,
}

/// `(seq, exchange, source)` tie-breakers of any event kind.
fn tie_fields(e: &MarketEvent) -> (u64, &str, Source) {
    match e {
        MarketEvent::Trade(t) => (t.source_seq, &t.exchange, t.source),
        MarketEvent::Quote(q) => (0, &q.ask_exchange, q.source),
        MarketEvent::Correction(c) => (c.corrected_id, &c.exchange, c.source),
        MarketEvent::Cancel(c) => (c.trade_id, &c.exchange, c.source),
        MarketEvent::Status(s) => (0, "", s.source),
    }
}

/// The time an event is delivered: its receive time if the provider path recorded one, else its
/// event time.
pub fn raw_delivery(e: &MarketEvent) -> i64 {
    let r = match e {
        MarketEvent::Trade(t) => t.receive_ts,
        MarketEvent::Quote(q) => q.receive_ts,
        MarketEvent::Correction(c) => c.receive_ts,
        MarketEvent::Cancel(c) => c.receive_ts,
        MarketEvent::Status(s) => s.receive_ts,
    };
    r.unwrap_or(e.event_ts()).nanos()
}

/// Per-stream running maximum of [`raw_delivery`] — the delivery time used for ordering.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeliveryClock(Option<i64>);

impl DeliveryClock {
    pub fn next(&mut self, e: &MarketEvent) -> i64 {
        let d = raw_delivery(e);
        let d = self.0.map_or(d, |p| p.max(d));
        self.0 = Some(d);
        d
    }
}

pub struct Merger {
    streams: Vec<EventStream>,
    clocks: Vec<DeliveryClock>,
    heads: Vec<Option<MarketEvent>>,
    heap: BinaryHeap<Reverse<MergeKey>>,
    error: Option<String>,
}

impl Merger {
    pub fn new(streams: Vec<EventStream>) -> Self {
        let n = streams.len();
        let mut m = Merger {
            streams,
            clocks: vec![DeliveryClock::default(); n],
            heads: vec![None; n],
            heap: BinaryHeap::with_capacity(n),
            error: None,
        };
        for i in 0..n {
            m.refill(i);
        }
        m
    }

    fn refill(&mut self, i: usize) {
        match self.streams[i].next() {
            Some(Ok(e)) => {
                let delivery = self.clocks[i].next(&e);
                let (seq, exchange, source) = tie_fields(&e);
                self.heap.push(Reverse(MergeKey {
                    delivery,
                    event_ts: e.event_ts().nanos(),
                    seq,
                    exchange: exchange.to_string(),
                    source,
                    symbol: e.symbol().unwrap_or("").to_string(),
                    stream: i,
                }));
                self.heads[i] = Some(e);
            }
            Some(Err(err)) if self.error.is_none() => self.error = Some(err),
            Some(Err(_)) => {}
            None => {}
        }
    }

    /// Delivery time of the next event, if any.
    pub fn peek_delivery(&self) -> Option<i64> {
        self.heap.peek().map(|Reverse(k)| k.delivery)
    }

    /// A stream error, reported once the merge would otherwise continue past it.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// The next `(delivery, event)` in merge order.
    pub fn pop(&mut self) -> Option<(i64, MarketEvent)> {
        let Reverse(k) = self.heap.pop()?;
        let e = self.heads[k.stream]
            .take()
            .expect("head present for heap entry");
        self.refill(k.stream);
        Some((k.delivery, e))
    }
}
