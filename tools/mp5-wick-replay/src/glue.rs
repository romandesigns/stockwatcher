// Adapter copied from research/pullback-context replay.rs (2267a1c). Detector rules are not copied.
use chart_engine::Revision;
use market_events::{MarketEvent,UnixNanos};
use market_events::conditions::trade_effect;
use ctx::{CorrectedPrint,DayHigh,LastPrint,PrintRevision,RevisionTarget,Verdict};
pub fn revision_admit(e: &MarketEvent) -> UnixNanos {
    match e {
        MarketEvent::Correction(c) => c.receive_ts.unwrap_or(c.event_ts),
        MarketEvent::Cancel(c) => c.receive_ts.unwrap_or(c.event_ts),
        other => other.event_ts(),
    }
}

/// Server `context.rs` `revision_of`.
pub fn revision_of(e: &MarketEvent, resolved: Option<&Revision>) -> Option<PrintRevision> {
    let (symbol, target, corrected) = match e {
        MarketEvent::Correction(c) => (
            c.symbol.clone(),
            RevisionTarget {
                id: c.original_trade_id(),
                price: c.original_price.micros(),
                size: c.original_size,
            },
            Some(CorrectedPrint {
                price: c.corrected_price.micros(),
                size: c.corrected_size,
                verdict: {
                    let eff = trade_effect(&c.corrected_conditions, c.tape);
                    Verdict {
                        ohlc: eff.updates_ohlc,
                        volume: eff.updates_volume,
                    }
                },
            }),
        ),
        MarketEvent::Cancel(c) => (
            c.symbol.clone(),
            RevisionTarget {
                id: c.target_trade_id(),
                price: c.price.micros(),
                size: c.size,
            },
            None,
        ),
        _ => return None,
    };
    Some(PrintRevision {
        symbol,
        target,
        corrected,
        last: resolved.and_then(|r| r.last).map(|(p, ts)| LastPrint {
            exec: ts.nanos(),
            price: p.micros(),
        }),
        day_high: resolved.and_then(|r| r.day_high).map(|h| DayHigh {
            price: h.price.micros(),
            complete: h.complete,
        }),
    })
}
