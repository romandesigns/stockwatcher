//! The Ross Panel row chart's **day line** (owner, 2026-10-21): how a member's price developed
//! over the day, so a glance at the panel shows the shape before the ticker is opened.
//!
//! # What the line is
//!
//! The 30 s closes of the member's latest trading date from 04:00 ET (pre, regular and after
//! hours; the overnight session is left out), from the interval of the day's first price up to
//! the last **completed** 30 s interval, one point per interval, never downsampled
//! ([`chart_engine::ChartEngine::day_line`]: quiet intervals repeat the previous close so point
//! `i` is always `day_line_start + i·30 s`). The forming interval is not a point; the row's own
//! `price` is the live end a client draws after the last point. The owner asked for 30 s points
//! rather than a downsampled minute line: "a more accurate, more live real-time line chart".
//!
//! # Where it comes from and what it costs
//!
//! The chart engine's 30 s candles, which the server keeps for every symbol anyway (4,000
//! intervals per timeframe; a 04:00–20:00 day is 1,920 at 30 s). Nothing is added on the ingest
//! path: the line is read under the core lock only when a Ross frame is serialized (a snapshot
//! for a subscriber, or the at-most-once-a-second `ross.update` while anyone is subscribed), and
//! only for the panel's members (a few dozen at most): one pass over each member's retained 30 s
//! ring.
//!
//! # Wire (additive; an older client ignores it)
//!
//! Each row of a Ross frame may carry:
//!
//! - `day_line_start` — epoch ns of the first point's interval;
//! - `day_line_step_s` — 30;
//! - `day_line_from` — the index of the first point in `day_line`;
//! - `day_line` — closes in integer micro-dollars from index `day_line_from` on;
//! - `prev_close` — the previous regular close (micros) from `daily-closes.json`, when known: the
//!   dotted baseline.
//!
//! A `ross.snapshot` always sends the whole line (`day_line_from: 0`). A `ross.update` sends only
//! what changed since the previous `ross.update`: `day_line_from` is the first index that differs
//! from the line that update carried (normally its old length, so the slice is the new points;
//! lower if a correction revised an earlier close), and `0` with the whole line for a member that
//! was not in it or whose line restarted. A client keeps its first `day_line_from` points and
//! replaces the rest with `day_line`. When it holds fewer than `day_line_from` points (a frame it
//! never saw) or a different `day_line_start`, it drops the line and asks for a fresh snapshot
//! (`ross.subscribe` again). A row without the fields has no line.
//!
//! The rule is safe across a snapshot because a snapshot is never older than the update before
//! it: an update's `day_line_from` is at most the length of the line the previous update carried,
//! which is at most the length of any line computed after it.

use std::collections::HashMap;

use chart_engine::{ChartEngine, Timeframe};
use market_archive::PrevCloses;
use serde_json::{Value, json};

/// The line's interval.
pub(crate) const STEP: Timeframe = Timeframe::S30;

/// What the lines are read from: the server's chart engine and the previous-close index.
#[derive(Clone, Copy)]
pub(crate) struct Source<'a> {
    pub(crate) engine: &'a ChartEngine,
    pub(crate) closes: &'a PrevCloses,
}

/// One member's line as published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) start: i64,
    /// Micros, one per [`STEP`] from `start`.
    pub(crate) closes: Vec<i64>,
    pub(crate) prev_close: Option<i64>,
}

/// The lines the last `ross.update` carried, by symbol: what the next one is a delta against.
#[derive(Debug, Default)]
pub(crate) struct Published(HashMap<String, Line>);

/// A symbol's current line, or `None` before its first price.
pub(crate) fn line(src: Source<'_>, symbol: &str) -> Option<Line> {
    let l = src.engine.day_line(symbol, STEP)?;
    Some(Line {
        start: l.start.0,
        closes: l.closes.iter().map(|p| p.micros()).collect(),
        prev_close: src
            .closes
            .before(symbol, l.session.date)
            .map(|(_, p)| p.micros()),
    })
}

/// Adds the day-line fields to every row of a serialized panel (`{rows: [{symbol, …}]}`).
///
/// `published`: `None` for a snapshot (whole lines, the baseline untouched); `Some` for an update
/// (slices against the baseline, which then becomes this frame's lines, members only).
pub(crate) fn decorate(panel: &mut Value, src: Source<'_>, published: Option<&mut Published>) {
    let Some(rows) = panel.get_mut("rows").and_then(Value::as_array_mut) else {
        if let Some(p) = published {
            p.0.clear();
        }
        return;
    };
    let mut next: HashMap<String, Line> = HashMap::new();
    for row in rows.iter_mut() {
        let Some(symbol) = row.get("symbol").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let Some(l) = line(src, &symbol) else {
            continue;
        };
        let from = match published.as_ref().and_then(|p| p.0.get(&symbol)) {
            Some(old) if old.start == l.start => old
                .closes
                .iter()
                .zip(&l.closes)
                .take_while(|(a, b)| a == b)
                .count(),
            _ => 0,
        };
        if let Some(obj) = row.as_object_mut() {
            obj.insert("day_line_start".into(), json!(l.start));
            obj.insert("day_line_step_s".into(), json!(STEP.secs()));
            obj.insert("day_line_from".into(), json!(from));
            obj.insert("day_line".into(), json!(&l.closes[from..]));
            if let Some(pc) = l.prev_close {
                obj.insert("prev_close".into(), json!(pc));
            }
        }
        if published.is_some() {
            next.insert(symbol, l);
        }
    }
    if let Some(p) = published {
        p.0 = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use market_events::session::et;
    use market_events::{MarketEvent, Source as Feed, Tape, Trade, UnixNanos};

    fn at(h: u32, m: u32, s: u32) -> UnixNanos {
        et(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap(), h, m, s, 0).unwrap()
    }

    fn trade(engine: &mut ChartEngine, sym: &str, t: UnixNanos, price: &str, seq: u64) {
        engine.apply(&MarketEvent::Trade(
            Trade::new(sym, t, price.parse().unwrap(), 100, seq)
                .with_exchange("V")
                .with_tape(Tape::C)
                .with_source(Feed::Fixture),
        ));
    }

    fn panel(symbols: &[&str]) -> Value {
        json!({ "rows": symbols.iter().map(|s| json!({ "symbol": s })).collect::<Vec<_>>() })
    }

    #[test]
    fn snapshot_carries_whole_lines_and_updates_carry_only_new_points() {
        let mut e = ChartEngine::default();
        let closes = PrevCloses::default();
        trade(&mut e, "AAAA", at(4, 0, 5), "2.00", 1);
        trade(&mut e, "AAAA", at(4, 0, 35), "2.10", 2);
        e.advance_clock(at(4, 1, 1));
        let src = || Source {
            engine: &e,
            closes: &closes,
        };
        let mut published = Published::default();

        let mut first = panel(&["AAAA", "NONE"]);
        decorate(&mut first, src(), Some(&mut published));
        let row = &first["rows"][0];
        assert_eq!(row["day_line_from"], 0);
        assert_eq!(row["day_line"], json!([2_000_000, 2_100_000]));
        assert_eq!(row["day_line_start"], at(4, 0, 0).0);
        assert_eq!(row["day_line_step_s"], 30);
        assert!(row.get("prev_close").is_none(), "no closes file entry");
        assert!(
            first["rows"][1].get("day_line").is_none(),
            "no price, no line"
        );

        // Nothing new: an update carries an empty slice from the old length.
        let mut same = panel(&["AAAA"]);
        decorate(&mut same, src(), Some(&mut published));
        assert_eq!(same["rows"][0]["day_line_from"], 2);
        assert_eq!(same["rows"][0]["day_line"], json!([]));

        trade(&mut e, "AAAA", at(4, 1, 10), "2.30", 3);
        e.advance_clock(at(4, 2, 0));
        let src = || Source {
            engine: &e,
            closes: &closes,
        };
        let mut next = panel(&["AAAA"]);
        decorate(&mut next, src(), Some(&mut published));
        assert_eq!(next["rows"][0]["day_line_from"], 2);
        assert_eq!(next["rows"][0]["day_line"], json!([2_300_000, 2_300_000]));

        // A snapshot is whole and leaves the update baseline alone.
        let mut snap = panel(&["AAAA"]);
        decorate(&mut snap, src(), None);
        assert_eq!(snap["rows"][0]["day_line_from"], 0);
        assert_eq!(snap["rows"][0]["day_line"].as_array().unwrap().len(), 4);
        let mut after = panel(&["AAAA"]);
        decorate(&mut after, src(), Some(&mut published));
        assert_eq!(after["rows"][0]["day_line_from"], 4);
    }

    #[test]
    fn a_member_that_left_and_came_back_gets_its_whole_line() {
        let mut e = ChartEngine::default();
        let closes = PrevCloses::default();
        trade(&mut e, "AAAA", at(9, 30, 5), "2.00", 1);
        e.advance_clock(at(9, 31, 0));
        let src = Source {
            engine: &e,
            closes: &closes,
        };
        let mut published = Published::default();
        decorate(&mut panel(&["AAAA"]), src, Some(&mut published));
        decorate(&mut panel(&[]), src, Some(&mut published));
        let mut back = panel(&["AAAA"]);
        decorate(&mut back, src, Some(&mut published));
        assert_eq!(back["rows"][0]["day_line_from"], 0);
        assert_eq!(back["rows"][0]["day_line"], json!([2_000_000, 2_000_000]));
    }

    /// The wire cost on a busy day: 30 members trading every second of the full 04:00–20:00 day
    /// (1,920 points each), sub-penny prices from $0.50 to $24. Run with `--nocapture` to print.
    #[test]
    fn wire_cost_of_thirty_full_day_members() {
        let mut e = ChartEngine::default();
        let closes = PrevCloses::default();
        let syms: Vec<String> = (0..30).map(|i| format!("S{i:03}")).collect();
        let mut seq = 0;
        let mut feed = |e: &mut ChartEngine, from: UnixNanos, to: UnixNanos| {
            let mut t = from.0;
            while t < to.0 {
                for (i, s) in syms.iter().enumerate() {
                    seq += 1;
                    // A deterministic walk with four decimals (micros like 3_456_700).
                    let base = 500_000 + i as i64 * 800_000;
                    let wobble = ((t / 1_000_000_000 + i as i64 * 37) % 997) * 137;
                    let micros = (base + wobble) / 100 * 100;
                    let price = format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000);
                    trade(e, s, UnixNanos(t), &price, seq);
                }
                t += 15_000_000_000;
            }
            e.advance_clock(to);
        };
        feed(&mut e, at(4, 0, 0), at(19, 59, 30));
        let rows = |e: &ChartEngine, published: Option<&mut Published>| {
            let names: Vec<&str> = syms.iter().map(String::as_str).collect();
            let mut p = panel(&names);
            let bare = p.to_string().len();
            decorate(
                &mut p,
                Source {
                    engine: e,
                    closes: &closes,
                },
                published,
            );
            let added = p.to_string().len() - bare;
            (p, added)
        };
        let mut published = Published::default();
        let (snap, snap_bytes) = rows(&e, None);
        assert_eq!(snap["rows"][0]["day_line"].as_array().unwrap().len(), 1_919);
        rows(&e, Some(&mut published));
        // One more completed interval: the next update carries one point per member.
        feed(&mut e, at(19, 59, 30), at(20, 0, 0));
        let (upd, upd_bytes) = rows(&e, Some(&mut published));
        assert_eq!(upd["rows"][0]["day_line"].as_array().unwrap().len(), 1);
        let (_, idle_bytes) = rows(&e, Some(&mut published));
        eprintln!(
            "day line wire, 30 members: snapshot +{snap_bytes} B, update +{upd_bytes} B (1 new point each), idle update +{idle_bytes} B"
        );
        assert!(snap_bytes < 30 * 1_920 * 10, "{snap_bytes}");
        assert!(upd_bytes < 30 * 120, "{upd_bytes}");
    }
}
