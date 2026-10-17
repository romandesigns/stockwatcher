//! Validation of an imported day: deterministic replay through the chart engine, and the
//! diagnostic comparison of FMP minute bars with trade-built 1-minute candles.

use chart_engine::wire::ChartSnapshot;
use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_archive::{Archive, ArchiveError, Bar};
use market_events::{MarketEvent, UnixNanos};
use std::collections::BTreeMap;

/// What one replay of the day produced, per timeframe.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeframeSummary {
    pub tf: Timeframe,
    pub candles: usize,
    pub gaps: usize,
    pub volume: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReplaySummary {
    pub events: u64,
    pub timeframes: Vec<TimeframeSummary>,
    /// The profile of the session the day ended in (the engine keeps one per session).
    pub profile_anchor: Option<String>,
    pub profile_total: u64,
    pub profile_poc: Option<f64>,
    /// Two independent replays of the archived log gave byte-identical snapshots.
    pub deterministic: bool,
    /// Halts / statuses seen in the log (historical trades carry none).
    pub statuses: u64,
    pub halts: usize,
    /// Prints the engine counted as `excluded:*` by condition or provider update.
    pub condition_excluded: u64,
}

/// Replays the archived `(day, symbol)` log through a fresh engine twice and summarizes it.
pub fn replay_day(
    archive: &Archive,
    day: NaiveDate,
    symbol: &str,
) -> Result<(ReplaySummary, Vec<ChartSnapshot>), ArchiveError> {
    let events: Vec<MarketEvent> = archive
        .reader(day, symbol)?
        .map(|r| r.map(|v| v.event))
        .collect::<Result<_, _>>()?;
    let a = run(&events, symbol);
    let b = run(&events, symbol);
    let json = |s: &[ChartSnapshot]| serde_json::to_string(s).expect("snapshots serialize");
    let deterministic = json(&a) == json(&b);
    let timeframes = a
        .iter()
        .map(|s| TimeframeSummary {
            tf: s.timeframe,
            candles: s.candles.len(),
            gaps: s.gaps.len(),
            volume: s.candles.iter().map(|c| c.v).sum::<u64>()
                + s.gaps.iter().map(|g| g.v).sum::<u64>(),
        })
        .collect();
    let m1 = a.iter().find(|s| s.timeframe == Timeframe::M1);
    let profile = m1.and_then(|s| s.profile.as_ref());
    let summary = ReplaySummary {
        events: events.len() as u64,
        timeframes,
        profile_anchor: profile.map(|p| p.anchor.clone()),
        profile_total: profile.map_or(0, |p| p.total),
        profile_poc: profile.and_then(|p| p.poc),
        deterministic,
        statuses: events
            .iter()
            .filter(|e| matches!(e, MarketEvent::Status(_)))
            .count() as u64,
        halts: m1.map_or(0, |s| s.halts.len()),
        condition_excluded: m1.map_or(0, |s| s.quality.condition_excluded),
    };
    Ok((summary, a))
}

fn run(events: &[MarketEvent], symbol: &str) -> Vec<ChartSnapshot> {
    let mut engine = ChartEngine::default();
    let mut last = None;
    for e in events {
        engine.apply(e);
        last = Some(last.map_or(e.event_ts().0, |l: i64| l.max(e.event_ts().0)));
    }
    // Finalize every interval: move the clock past the last print's 5-minute interval.
    if let Some(l) = last {
        engine.advance_clock(UnixNanos(l + 10 * 60 * 1_000_000_000));
    }
    Timeframe::ALL
        .iter()
        .filter_map(|&tf| engine.snapshot(symbol, tf, None))
        .collect()
}

/// One minute where the two sources disagree, for the report.
#[derive(Clone, Debug, PartialEq)]
pub struct MinuteDiff {
    pub t_ms: i64,
    pub alpaca: Option<(f64, f64, f64, f64, u64)>,
    pub fmp: Option<(f64, f64, f64, f64, u64)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MinuteComparison {
    pub alpaca_minutes: usize,
    pub fmp_minutes: usize,
    pub both: usize,
    pub only_alpaca: usize,
    pub only_fmp: usize,
    /// Minutes present in both whose O/H/L/C all agree exactly (to the micro-dollar).
    pub ohlc_equal: usize,
    pub close_equal: usize,
    pub volume_equal: usize,
    /// Sum of volumes over the shared minutes.
    pub alpaca_volume: u64,
    pub fmp_volume: u64,
    /// The largest differences, for inspection (up to 10).
    pub examples: Vec<MinuteDiff>,
}

/// Compares trade-built 1-minute candles with provider minute bars. Diagnostic only: the two
/// legitimately differ (feed coverage, which conditions count, extended-hours inclusion,
/// corrections applied later, the provider's own aggregation), so nothing here is forced equal.
pub fn compare_minutes(m1: &ChartSnapshot, bars: &[Bar]) -> MinuteComparison {
    // Micro-dollars, the contract's unit: cents would call 0.449 and 0.45 equal, and sub-dollar
    // stocks trade in $0.0001 steps.
    let micros = |x: f64| (x * 1_000_000.0).round() as i64;
    let alpaca: BTreeMap<i64, (f64, f64, f64, f64, u64)> = m1
        .candles
        .iter()
        .map(|c| (c.t, (c.o, c.h, c.l, c.c, c.v)))
        .collect();
    let fmp: BTreeMap<i64, (f64, f64, f64, f64, u64)> = bars
        .iter()
        .map(|b| {
            let f = |m: i64| m as f64 / 1_000_000.0;
            (b.t / 1_000_000, (f(b.o), f(b.h), f(b.l), f(b.c), b.v))
        })
        .collect();
    let mut out = MinuteComparison {
        alpaca_minutes: alpaca.len(),
        fmp_minutes: fmp.len(),
        ..Default::default()
    };
    let mut diffs: Vec<(i64, MinuteDiff)> = Vec::new();
    let keys: std::collections::BTreeSet<i64> = alpaca.keys().chain(fmp.keys()).copied().collect();
    for t in keys {
        match (alpaca.get(&t), fmp.get(&t)) {
            (Some(a), Some(f)) => {
                out.both += 1;
                out.alpaca_volume += a.4;
                out.fmp_volume += f.4;
                let same = |x: f64, y: f64| micros(x) == micros(y);
                if same(a.0, f.0) && same(a.1, f.1) && same(a.2, f.2) && same(a.3, f.3) {
                    out.ohlc_equal += 1;
                }
                if same(a.3, f.3) {
                    out.close_equal += 1;
                }
                if a.4 == f.4 {
                    out.volume_equal += 1;
                }
                // In micro-dollars, with each 100 shares of volume difference weighing like $0.0001.
                let score = (micros(a.0) - micros(f.0)).abs()
                    + (micros(a.1) - micros(f.1)).abs()
                    + (micros(a.2) - micros(f.2)).abs()
                    + (micros(a.3) - micros(f.3)).abs()
                    + (a.4 as i64 - f.4 as i64).abs();
                if score > 0 {
                    diffs.push((
                        score,
                        MinuteDiff {
                            t_ms: t,
                            alpaca: Some(*a),
                            fmp: Some(*f),
                        },
                    ));
                }
            }
            (Some(_), None) => out.only_alpaca += 1,
            (None, Some(_)) => out.only_fmp += 1,
            (None, None) => {}
        }
    }
    diffs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.t_ms.cmp(&b.1.t_ms)));
    out.examples = diffs.into_iter().take(10).map(|(_, d)| d).collect();
    out
}
