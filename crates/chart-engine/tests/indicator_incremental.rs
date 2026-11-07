//! Incremental equals batch: an [`IndicatorSeries`] edited bar by bar (forming updates, new bars,
//! revisions in the middle, inserts and removals) always holds exactly the values the batch
//! functions give over the same bars, bit for bit; and the range an edit returns covers every
//! value that moved, which is what lets the server send only those points (`chart.indicator`).
//! Seeded and synthetic (FIXTURE DATA, NOT MARKET DATA).

use std::collections::HashMap;

use chart_engine::indicators::{Bar, BarEdit, IndicatorSeries, IndicatorSpec, MacdStyle, batch};
use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_archive::fixture::{DEFAULT_SEED, fixture_symbol, symbol_params};
use market_events::session::et;

/// splitmix64: small, seeded, the same on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn specs() -> Vec<IndicatorSpec> {
    use IndicatorSpec::*;
    vec![
        Sma { period: 1 },
        Sma { period: 3 },
        Sma { period: 20 },
        Ema { period: 1 },
        Ema { period: 5 },
        Ema { period: 9 },
        Vwap,
        Bb {
            period: 1,
            mult: 2.0,
        },
        Bb {
            period: 5,
            mult: 0.5,
        },
        Bb {
            period: 20,
            mult: 2.0,
        },
        Rsi { period: 1 },
        Rsi { period: 3 },
        Rsi { period: 14 },
        Macd {
            fast: 3,
            slow: 5,
            signal: 2,
            style: MacdStyle::Classic,
        },
        Macd {
            fast: 12,
            slow: 26,
            signal: 9,
            style: MacdStyle::Classic,
        },
        Macd {
            fast: 26,
            slow: 12,
            signal: 9,
            style: MacdStyle::Classic,
        },
    ]
    .into_iter()
    // Every MACD style at the defaults and at short periods (the zero-lag lag and the simple
    // signal's window both read back past a revision).
    .chain(MacdStyle::ALL.into_iter().flat_map(|style| {
        [(12, 26, 9), (3, 5, 2), (1, 1, 1)].map(|(fast, slow, signal)| Macd {
            fast,
            slow,
            signal,
            style,
        })
    }))
    .collect()
}

fn day(n: i64) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap() + chrono::Duration::days(n)
}

/// A bar at slot `i` (one minute each; a new trading day every 300 slots, for VWAP).
fn bar(r: &mut Rng, i: i64, around: f64) -> Bar {
    let c = (around * (1.0 + (r.unit() - 0.5) * 0.03) * 100.0).round() / 100.0;
    let c = if r.below(8) == 0 { around } else { c.max(0.01) };
    Bar {
        t_ms: 1_792_000_000_000 + i * 60_000,
        day: day(i / 300),
        h: c + (r.below(5) as f64) * 0.01,
        l: (c - (r.below(5) as f64) * 0.01).max(0.01),
        c,
        v: if r.below(20) == 0 {
            0.0
        } else {
            (r.below(5000) * 100) as f64
        },
    }
}

fn bits(s: &IndicatorSeries, i: usize) -> [u64; 3] {
    let v = s.values(i);
    let mut out = [f64::NAN.to_bits(); 3];
    for (j, x) in v.iter().enumerate() {
        out[j] = canon(*x);
    }
    out
}

/// NaN compared as one value.
fn canon(x: f64) -> u64 {
    if x.is_nan() {
        f64::NAN.to_bits()
    } else {
        x.to_bits()
    }
}

fn assert_matches_batch(s: &IndicatorSeries, ctx: &str) {
    let want = batch(s.spec(), s.bars());
    assert_eq!(want.len(), s.len());
    for (i, w) in want.iter().enumerate() {
        let got = bits(s, i);
        for j in 0..s.spec().lines().len() {
            assert_eq!(
                got[j],
                canon(w[j]),
                "{ctx}: {} line {j} at {i}: series {} batch {}",
                s.spec().key(),
                f64::from_bits(got[j]),
                w[j]
            );
        }
    }
}

#[test]
fn random_edits_keep_the_series_equal_to_batch() {
    for spec in specs() {
        let seed = spec
            .key()
            .bytes()
            .fold(0x5eed_u64, |h, b| h.wrapping_mul(31) ^ u64::from(b));
        let mut r = Rng(seed);
        // Slots 0..400 with every fifth one left empty, so there are holes to insert into.
        let mut used: Vec<i64> = (0..400).filter(|i| i % 5 != 2).collect();
        let mut price = 10.0;
        let bars: Vec<Bar> = used.iter().map(|&i| bar(&mut r, i, price)).collect();
        let mut s = IndicatorSeries::build(spec, bars, 1_000_000);
        assert_matches_batch(&s, "build");
        let mut next = 400i64;
        for step in 0..600 {
            let before: HashMap<i64, [u64; 3]> =
                (0..s.len()).map(|i| (s.t(i), bits(&s, i))).collect();
            let (edit, what) = match r.below(10) {
                // A trade in the forming bar.
                0..=3 => {
                    let last = *used.last().unwrap();
                    price = s.bars().last().unwrap().c;
                    (BarEdit::Upsert(bar(&mut r, last, price)), "forming")
                }
                // A new bar.
                4 | 5 => {
                    used.push(next);
                    next += 1;
                    (BarEdit::Upsert(bar(&mut r, next - 1, price)), "new")
                }
                // A revision of an older bar (late trade, correction, cancel).
                6 | 7 => {
                    let k = used[r.below(used.len())];
                    (BarEdit::Upsert(bar(&mut r, k, price)), "revise")
                }
                // A bar appearing in a hole (a late trade into an empty interval).
                8 => {
                    let k = used[r.below(used.len() - 1)] + 1;
                    if !used.contains(&k) {
                        used.push(k);
                        used.sort();
                    }
                    (BarEdit::Upsert(bar(&mut r, k, price)), "insert")
                }
                // A bar disappearing (its prints cancelled).
                _ => {
                    if used.len() < 3 {
                        continue;
                    }
                    let k = used.remove(r.below(used.len() - 1));
                    (BarEdit::Remove(1_792_000_000_000 + k * 60_000), "remove")
                }
            };
            let changed = s.apply(edit);
            let ctx = format!("step {step} ({what})");
            assert_matches_batch(&s, &ctx);
            // Outside the returned range, nothing that existed before moved.
            for i in 0..s.len() {
                if changed.as_ref().is_some_and(|c| c.contains(&i)) {
                    continue;
                }
                if let Some(old) = before.get(&s.t(i)) {
                    assert_eq!(
                        *old,
                        bits(&s, i),
                        "{ctx}: {} moved at {i} outside {changed:?}",
                        spec.key()
                    );
                } else {
                    panic!("{ctx}: new bar at {i} outside the returned range {changed:?}");
                }
            }
        }
    }
}

/// The edits the server makes, driven by the engine's own increments over the fixture day
/// (late trades, out-of-order prints, a correction, a cancel, a halt): after every event the
/// series equals batch over the engine's bars, and holds exactly those bars.
#[test]
fn engine_increments_keep_every_series_equal_to_the_engine() {
    let date = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
    let mut engine = ChartEngine::default();
    let cap = engine.config().max_intervals_per_timeframe;
    let keys: Vec<(Timeframe, bool)> = vec![
        (Timeframe::S15, false),
        (Timeframe::M1, false),
        (Timeframe::M1, true),
        (Timeframe::M5, true),
    ];
    let wanted = [
        IndicatorSpec::Vwap,
        IndicatorSpec::Ema { period: 9 },
        IndicatorSpec::Bb {
            period: 20,
            mult: 2.0,
        },
        IndicatorSpec::Rsi { period: 14 },
        IndicatorSpec::Macd {
            fast: 12,
            slow: 26,
            signal: 9,
            style: MacdStyle::Classic,
        },
    ];
    let mut revisions = 0usize;
    for (i, sym) in ["SMPL", "FXLO"].iter().enumerate() {
        let (seed, scale) = symbol_params(DEFAULT_SEED, i, sym);
        let events = fixture_symbol(date, sym, seed, scale).unwrap();
        let mut series: Vec<((Timeframe, bool), IndicatorSeries)> = keys
            .iter()
            .flat_map(|k| {
                wanted
                    .iter()
                    .map(move |s| (*k, IndicatorSeries::build(*s, Vec::new(), cap)))
            })
            .collect();
        let check = |series: &Vec<((Timeframe, bool), IndicatorSeries)>,
                     engine: &ChartEngine,
                     ctx: &str| {
            for ((tf, rth), s) in series {
                let bars = engine.indicator_bars(sym, *tf, *rth).unwrap_or_default();
                assert_eq!(s.bars(), &bars[..], "{ctx}: {sym} {tf:?} rth {rth} bars");
                assert_matches_batch(s, ctx);
            }
        };
        for (n, e) in events.iter().enumerate() {
            let ups = engine.apply(e);
            for u in &ups {
                for ((tf, rth), s) in series.iter_mut() {
                    if u.timeframe() != Some(*tf) {
                        continue;
                    }
                    if let Some(edit) = BarEdit::of(u, *rth)
                        && let Some(r) = s.apply(edit)
                        && r.end - r.start > 1
                    {
                        revisions += 1;
                    }
                }
            }
            // Every event is too slow in a debug build over a whole day; every 251st, and every correction and cancel
            // (they revise older bars), is where a mistake would show.
            if n % 251 == 0
                || matches!(
                    e,
                    market_events::MarketEvent::Correction(_)
                        | market_events::MarketEvent::Cancel(_)
                )
            {
                check(&series, &engine, &format!("event {n}"));
            }
        }
        for u in engine.advance_clock(et(date, 20, 0, 0, 0).unwrap()) {
            for ((tf, rth), s) in series.iter_mut() {
                if u.timeframe() == Some(*tf)
                    && let Some(edit) = BarEdit::of(&u, *rth)
                {
                    s.apply(edit);
                }
            }
        }
        check(&series, &engine, "end of day");
    }
    assert!(
        revisions > 0,
        "the fixture day must revise an older bar somewhere"
    );
}
