//! `ChartEngine::adopt_symbol`: a symbol's history built in an engine of its own and moved into
//! the market-wide one gives the same candles, profile, quality and halts as feeding everything
//! to one engine in arrival order (the live market's lazy per-symbol load relies on it).

use chart_engine::{ChartEngine, Timeframe};
use market_events::{MarketEvent, Price, Trade, UnixNanos};

const SYMS: [&str; 3] = ["AAA", "BBB", "CCC"];

/// A seeded stream: prints spread over 10:00–10:30 ET on 2026-10-15, some delivered late (up to
/// 4 s, past the 2 s settle window), some redelivered, odd lots among them. Receive times are the
/// delivery times (monotone), as the live provider stamps them.
fn stream() -> Vec<MarketEvent> {
    let start = 1_792_072_800_000_000_000i64; // 2026-10-15 10:00 ET
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut out: Vec<MarketEvent> = Vec::new();
    let mut d = start;
    for i in 0..6_000u64 {
        let r = next();
        d += (r % 600) as i64 * 1_000_000; // up to 0.6 s apart
        let lag = if r % 17 == 0 { 4_000 } else { r % 40 } as i64 * 1_000_000;
        let sym = SYMS[(r >> 8) as usize % 3];
        let size = if r % 5 == 0 { 37 } else { 100 };
        let mut t = Trade::new(
            sym,
            UnixNanos(d - lag),
            Price::from_cents(500 + ((r >> 16) % 40) as i64),
            size,
            i,
        )
        .with_exchange("V")
        .with_conditions(if size < 100 {
            vec!["@", "I"]
        } else {
            vec!["@"]
        });
        t.receive_ts = Some(UnixNanos(d));
        let e = MarketEvent::Trade(t);
        if r % 97 == 0
            && let Some(prev) = out.last().cloned()
        {
            // A redelivery of the previous print, delivered now.
            let mut p = prev;
            if let MarketEvent::Trade(t) = &mut p {
                t.receive_ts = Some(UnixNanos(d));
            }
            out.push(p);
        }
        out.push(e);
    }
    out
}

fn delivery(e: &MarketEvent) -> i64 {
    match e {
        MarketEvent::Trade(t) => t.receive_ts.unwrap_or(t.event_ts).0,
        _ => e.event_ts().0,
    }
}

fn mark(t: i64) -> UnixNanos {
    UnixNanos(t.div_euclid(1_000_000_000) * 1_000_000_000)
}

fn feed(engine: &mut ChartEngine, e: &MarketEvent) {
    engine.advance_clock(mark(delivery(e)));
    engine.apply(e);
}

fn assert_same(a: &ChartEngine, b: &ChartEngine) {
    for sym in SYMS {
        for tf in Timeframe::ALL {
            assert_eq!(
                a.snapshot(sym, tf, None),
                b.snapshot(sym, tf, None),
                "{sym} {tf}"
            );
        }
        assert_eq!(a.quality(sym), b.quality(sym), "{sym}");
        for s in a.sessions(sym) {
            assert_eq!(a.profile(sym, s, None), b.profile(sym, s, None), "{sym}");
        }
    }
}

#[test]
fn adopting_a_symbol_built_alone_equals_feeding_it_with_the_market() {
    let events = stream();
    let end = mark(delivery(events.last().unwrap()) + 3_600_000_000_000);

    let mut eager = ChartEngine::default();
    for e in &events {
        feed(&mut eager, e);
    }

    // BBB is "not loaded" for the first two thirds: the market engine sees only the marks; BBB's
    // events go to an engine of its own. Then it is adopted and the rest streams in normally.
    let cut = events.len() * 2 / 3;
    let mut market = ChartEngine::default();
    let mut alone = ChartEngine::default();
    for e in &events[..cut] {
        if e.symbol() == Some("BBB") {
            feed(&mut alone, e);
            market.advance_clock(mark(delivery(e)));
        } else {
            feed(&mut market, e);
        }
    }
    market.adopt_symbol(&mut alone, "BBB");
    for e in &events[cut..] {
        feed(&mut market, e);
    }

    // Before the end: the forming candles agree too.
    assert_same(&eager, &market);
    eager.advance_clock(end);
    market.advance_clock(end);
    assert_same(&eager, &market);
    let q: u64 = eager
        .quality("BBB")
        .iter()
        .map(|(_, q)| q.late_excluded + q.duplicates)
        .sum();
    assert!(q > 0, "the late and duplicate paths were exercised");
}
