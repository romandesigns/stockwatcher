//! Key levels and opening ranges (crates/chart-engine/src/levels.rs) equal the clients' former
//! chart-core `keyLevels` / `openingRanges` on the fixtures that code generated
//! (packages/chart-core/src/levelFixtures.ts: two full trading days at 1m and 5m), bit for bit;
//! the skip rules (`KeyLevels::may_change`, `ranges_may_change`) never skip a change; and the
//! intended difference on an early-close day.

use chart_engine::levels::{KeyLevels, key_levels, opening_ranges, ranges_may_change};
use chart_engine::wire::CandleWire;
use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_events::session::{SessionId, SessionKind, classify, et};
use market_events::{MarketEvent, Price, Trade, UnixNanos};
use serde_json::Value;

fn fixtures() -> Vec<Value> {
    let path = format!(
        "{}/tests/fixtures/level_fixtures.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture file")).expect("json")
}

fn candles(f: &Value) -> Vec<(SessionId, CandleWire)> {
    f["candles"]
        .as_array()
        .expect("candles")
        .iter()
        .map(|r| {
            let t = r[0].as_i64().expect("t");
            let c = CandleWire {
                t,
                o: r[1].as_f64().expect("o"),
                h: r[2].as_f64().expect("h"),
                l: r[3].as_f64().expect("l"),
                c: r[4].as_f64().expect("c"),
                v: 1000,
                n: 10,
                is_final: r[5].as_bool().expect("final"),
            };
            // The engine's calendar; on these two full days it is the client's display schedule.
            (
                classify(UnixNanos::from_millis(t)).expect("in a session"),
                c,
            )
        })
        .collect()
}

/// `keyLevels`' list (id, price) in its order.
fn as_list(k: &KeyLevels) -> Vec<(&'static str, f64)> {
    let mut out = Vec::new();
    if let (Some(h), Some(l)) = (k.pm_high, k.pm_low) {
        out.push(("pmHigh", h));
        out.push(("pmLow", l));
    }
    if let (Some(h), Some(l)) = (k.hod, k.lod) {
        out.push(("hod", h));
        out.push(("lod", l));
    }
    if let Some(o) = k.open {
        out.push(("open", o));
    }
    if let Some(p) = k.prev_close {
        out.push(("prevClose", p));
    }
    out
}

#[test]
fn levels_and_ranges_match_the_ts() {
    let fx = fixtures();
    assert!(fx.len() >= 16);
    for f in &fx {
        let name = f["name"].as_str().unwrap();
        let cs = candles(f);
        let k = key_levels(cs.iter().rev().cloned()).expect("candles");
        let want: Vec<(String, u64)> = f["levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| {
                (
                    l["id"].as_str().unwrap().to_owned(),
                    l["price"].as_f64().unwrap().to_bits(),
                )
            })
            .collect();
        let got: Vec<(String, u64)> = as_list(&k)
            .into_iter()
            .map(|(i, p)| (i.to_owned(), p.to_bits()))
            .collect();
        assert_eq!(got, want, "{name}: levels");

        let ranges = opening_ranges(cs.iter().cloned());
        let want = f["ranges"].as_array().unwrap();
        assert_eq!(ranges.len(), want.len(), "{name}: ranges");
        for (r, w) in ranges.iter().zip(want) {
            assert_eq!(r.day.to_string(), w["day"].as_str().unwrap(), "{name}");
            assert_eq!(r.t, w["t"].as_i64().unwrap(), "{name}: t");
            assert_eq!(r.end, w["end"].as_i64().unwrap(), "{name}: end");
            assert_eq!(
                r.high.to_bits(),
                w["high"].as_f64().unwrap().to_bits(),
                "{name}: high"
            );
            assert_eq!(
                r.low.to_bits(),
                w["low"].as_f64().unwrap().to_bits(),
                "{name}: low"
            );
            assert_eq!(
                r.forming,
                w["forming"].as_bool().unwrap(),
                "{name}: forming"
            );
        }
    }
}

/// The skip rules, replayed candle by candle as a live chart grows (each candle first arrives
/// forming with a partial range, then final): whenever a rule says an increment cannot change
/// the levels or ranges, a full recompute agrees.
#[test]
fn the_skip_rules_never_skip_a_change() {
    let mut skipped = 0;
    let mut checked = 0;
    for f in fixtures() {
        let all = candles(&f);
        let mut cs: Vec<(SessionId, CandleWire)> = Vec::new();
        for (s, c) in all {
            // Three steps: a partial forming candle, its full range, then final.
            let mid = (c.h + c.l) / 2.0;
            let partial = CandleWire {
                h: mid.max(c.o),
                l: mid.min(c.o),
                c: mid,
                is_final: false,
                ..c
            };
            for step in [
                partial,
                CandleWire {
                    is_final: false,
                    ..c
                },
                c,
            ] {
                let before_levels = key_levels(cs.iter().rev().cloned());
                let before_ranges = opening_ranges(cs.iter().cloned());
                match cs.last_mut() {
                    Some(last) if last.1.t == step.t => last.1 = step.clone(),
                    _ => cs.push((s, step.clone())),
                }
                let after_levels = key_levels(cs.iter().rev().cloned());
                let after_ranges = opening_ranges(cs.iter().cloned());
                if let Some(b) = &before_levels
                    && !b.may_change(s, &step)
                {
                    skipped += 1;
                    assert_eq!(
                        as_list(b),
                        as_list(after_levels.as_ref().unwrap()),
                        "{}",
                        step.t
                    );
                }
                if !ranges_may_change(&before_ranges, s, step.t) {
                    assert_eq!(before_ranges, after_ranges, "{}", step.t);
                }
                checked += 1;
            }
        }
    }
    // Most increments are skippable: that is the point of the rule.
    assert!(skipped * 10 > checked * 8, "{skipped} of {checked} skipped");
}

/// An early close (2026-11-27, 13:00): the 13:30 candle is after hours, so it is not in the high
/// of day (the client's 16:00 schedule counted it), and the opening range ends at 13:00 (the
/// client drew 16:00 for any early close that was not the server's current session).
#[test]
fn early_close_days_end_at_13_00() {
    let day = NaiveDate::from_ymd_opt(2026, 11, 27).unwrap();
    let mut e = ChartEngine::default();
    let trades = [(9, 0, 300), (9, 31, 310), (12, 59, 320), (13, 30, 400)];
    for (i, (h, m, cents)) in trades.into_iter().enumerate() {
        e.apply(&MarketEvent::Trade(Trade::new(
            "ERLY",
            et(day, h, m, 5, 0).unwrap(),
            Price::from_cents(cents),
            100,
            i as u64 + 1,
        )));
    }
    let k = e.key_levels("ERLY", Timeframe::M1).unwrap();
    assert_eq!(
        (k.pm_high, k.hod, k.open),
        (Some(3.0), Some(3.2), Some(3.1))
    );
    let r = e.opening_ranges("ERLY", Timeframe::M1);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].t, et(day, 9, 31, 0, 0).unwrap().millis());
    assert_eq!(r[0].end, et(day, 13, 0, 0, 0).unwrap().millis());
    assert!(!r[0].forming, "13:30 finalized it");
    assert_eq!(
        classify(et(day, 13, 30, 0, 0).unwrap()).unwrap().kind,
        SessionKind::After
    );
}
