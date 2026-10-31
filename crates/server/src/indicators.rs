//! Chart indicators computed here and pushed (`indicators.v1`): the registry of the studies the
//! connected charts asked for, owned by the market core next to the chart engine and guarded by
//! the same lock (`Core::indicators`).
//!
//! The math is crates/chart-engine/src/indicators.rs (bit-identical to the clients' former
//! chart-core code; its module docs list the three deliberate divergences). This module decides
//! WHAT is computed and turns it into frames:
//!
//! - **One computation per chart key, shared by every socket.** A chart key is (symbol,
//!   timeframe, rth); a study within it is named by its canonical spec key (`ema:9`), not by any
//!   client's spec id, so two clients asking for the same EMA share one series. The movers and
//!   pressure boards follow the same market-wide pattern; nothing here is per connection.
//! - **Demand vs computed.** A slot holding studies holds a lease (crate::market::IndicatorLease)
//!   that counts it in `demand`. The series themselves are built lazily, from the engine's
//!   retained candles (`ChartEngine::indicator_bars`), when a snapshot first needs them, and are
//!   cleared whenever the engine is rebuilt underneath them (a seek or day switch:
//!   [`IndicatorRegistry::clear_computed`]; a live symbol's history adopted:
//!   [`IndicatorRegistry::invalidate_symbol`]). Both are followed by a fresh snapshot to every
//!   slot concerned (`Resync`, `Loaded`), which rebuilds them, so the demand survives and the
//!   values never mix two engines.
//! - **Idle charts linger.** A chart whose demand fell to zero keeps its series for
//!   [`IDLE_DROP`], so flipping a timeframe and back, or a reconnect, does not rebuild it.
//! - **Feeding.** `Market::apply` hands every engine increment to
//!   [`IndicatorRegistry::on_updates`] under the core lock (also during a rebuild, so a series
//!   built mid-rebuild stays in step; the frames are then dropped). A symbol with no chart costs
//!   one hash lookup. A forming-bar trade is O(studies); see the chart-engine docs for the rest.
//!
//! Frames (JSON, times in epoch ms like `CandleWire.t`, NaN sent as `null`):
//!
//! - `chart.indicators {symbol, timeframe, rth, t: [ms], studies: [{key, lines: [{name, style,
//!   from, values}]}]}`: a snapshot of the studies it names. `t` is every input bar of this chart
//!   key (rth-filtered), ascending; a line's `values[i]` belongs to `t[from + i]` (the warm-up
//!   nulls before `from` are cut). It replaces the client's values of the keys it carries. Sent
//!   to one slot right after its `chart.snapshot` (same core lock, same seq), on
//!   `chart.indicators.set`, and on every resync; broadcast to all, carrying every computed key
//!   of the chart, instead of an increment that would move more than
//!   [`MAX_INCREMENT_POINTS`] points (a revision deep in the history).
//! - `chart.indicator {symbol, timeframe, rth, points: [{t, v: {key: [values]}}], removed?: [t]}`:
//!   one per chart key per engine apply that moved anything. A client drops the bars in `removed`
//!   first, then upserts each point's values per key (a point names only the keys that moved
//!   there; keys a slot did not ask for are ignored).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use chart_engine::indicators::{BarEdit, IndicatorSeries, IndicatorSpec};
use chart_engine::wire::ChartUpdate;
use chart_engine::{ChartEngine, Timeframe};
use serde::Serialize;
use serde_json::Value;

/// The `welcome.features` entry that tells a client the server computes indicators. A client
/// sends `indicators` on `chart.subscribe` only when it sees it, so a new client on an old server
/// keeps computing its own.
pub const FEATURE: &str = "indicators.v1";

/// How long a chart nobody watches keeps its series.
pub const IDLE_DROP: Duration = Duration::from_secs(60);

/// Points one `chart.indicator` may carry; past it the chart's snapshot goes out instead (a
/// revision near the start of a 4000-bar series would otherwise send it all as points).
pub const MAX_INCREMENT_POINTS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChartKey {
    pub symbol: String,
    pub timeframe: Timeframe,
    /// Regular session only (the client's extended hours off).
    pub rth: bool,
}

/// What one slot asked for: its specs, one per distinct key, in the client's order.
#[derive(Clone, Debug)]
pub struct Studies {
    pub chart: ChartKey,
    pub specs: Vec<IndicatorSpec>,
}

impl Studies {
    /// From a client's list (`IndicatorSpec::parse_list`, already capped): two ids computing the
    /// same thing become one study.
    pub fn new(chart: ChartKey, parsed: Vec<(String, IndicatorSpec)>) -> Self {
        let mut specs: Vec<IndicatorSpec> = Vec::new();
        for (_, s) in parsed {
            if !specs.iter().any(|x| x.key() == s.key()) {
                specs.push(s);
            }
        }
        Studies { chart, specs }
    }
}

#[derive(Debug, Default)]
struct Chart {
    /// Computed series by spec key.
    series: BTreeMap<String, IndicatorSeries>,
    /// Leases per spec key.
    demand: HashMap<String, u32>,
    /// When the demand fell to zero.
    idle_since: Option<Instant>,
}

/// One chart key an apply touched: the bar times whose values moved, per spec key, and the bar
/// times removed.
type Touched = (ChartKey, BTreeMap<String, BTreeSet<i64>>, BTreeSet<i64>);

#[derive(Debug, Default)]
pub struct IndicatorRegistry {
    charts: HashMap<ChartKey, Chart>,
    /// Symbols with any chart, so an increment for one of the other ten thousand costs one lookup
    /// and no allocation.
    symbols: HashSet<String>,
    idle: usize,
}

#[derive(Serialize)]
struct LineWire<'a> {
    name: &'static str,
    style: &'static str,
    from: usize,
    values: &'a [Option<f64>],
}

#[derive(Serialize)]
struct StudyWire<'a> {
    key: &'a str,
    lines: Vec<LineWire<'a>>,
}

#[derive(Serialize)]
struct SnapshotWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    symbol: &'a str,
    timeframe: Timeframe,
    rth: bool,
    t: Vec<i64>,
    studies: Vec<StudyWire<'a>>,
}

#[derive(Serialize)]
struct PointWire {
    t: i64,
    v: BTreeMap<String, Vec<Option<f64>>>,
}

#[derive(Serialize)]
struct IncrementWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    symbol: &'a str,
    timeframe: Timeframe,
    rth: bool,
    points: Vec<PointWire>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    removed: Vec<i64>,
}

fn opt(x: f64) -> Option<f64> {
    (!x.is_nan()).then_some(x)
}

/// A `chart.indicators` frame for `keys` of one chart (all of its series hold the same bars).
fn snapshot_json<'a>(
    chart: &ChartKey,
    series: impl Iterator<Item = (&'a String, &'a IndicatorSeries)>,
) -> String {
    let series: Vec<(&String, &IndicatorSeries)> = series.collect();
    let t: Vec<i64> = series
        .first()
        .map(|(_, s)| s.bars().iter().map(|b| b.t_ms).collect())
        .unwrap_or_default();
    // Per line: the values from its first non-null on.
    let columns: Vec<Vec<(usize, Vec<Option<f64>>)>> = series
        .iter()
        .map(|(_, s)| {
            (0..s.spec().lines().len())
                .map(|j| {
                    let from = (0..s.len())
                        .find(|&i| !s.values(i)[j].is_nan())
                        .unwrap_or(s.len());
                    (from, (from..s.len()).map(|i| opt(s.values(i)[j])).collect())
                })
                .collect()
        })
        .collect();
    let studies = series
        .iter()
        .zip(&columns)
        .map(|((key, s), cols)| StudyWire {
            key: key.as_str(),
            lines: s
                .spec()
                .lines()
                .iter()
                .zip(cols)
                .map(|((name, style), (from, values))| LineWire {
                    name,
                    style: style.as_str(),
                    from: *from,
                    values,
                })
                .collect(),
        })
        .collect();
    serde_json::to_string(&SnapshotWire {
        kind: "chart.indicators",
        symbol: &chart.symbol,
        timeframe: chart.timeframe,
        rth: chart.rth,
        t,
        studies,
    })
    .expect("indicator frames serialize")
}

impl IndicatorRegistry {
    /// A slot took `studies` (subscribe or `chart.indicators.set`).
    pub fn acquire(&mut self, studies: &Studies) {
        let chart = self.charts.entry(studies.chart.clone()).or_default();
        for s in &studies.specs {
            *chart.demand.entry(s.key()).or_default() += 1;
        }
        if chart.idle_since.take().is_some() {
            self.idle -= 1;
            // Lingering series of keys nobody asks for again would be fed for nothing.
            chart.series.retain(|k, _| chart.demand.contains_key(k));
        }
        self.symbols.insert(studies.chart.symbol.clone());
        self.sweep(Instant::now());
    }

    /// A slot let go of `studies` (it changed them, unsubscribed, or the connection ended).
    /// A series nobody asks for any more goes at once; the chart's other series stay, and a chart
    /// with no demand left lingers for [`IDLE_DROP`].
    pub fn release(&mut self, studies: &Studies) {
        let now = Instant::now();
        if let Some(chart) = self.charts.get_mut(&studies.chart) {
            for s in &studies.specs {
                let key = s.key();
                if let Some(n) = chart.demand.get_mut(&key) {
                    *n -= 1;
                    if *n == 0 {
                        chart.demand.remove(&key);
                    }
                }
            }
            if chart.demand.is_empty() {
                if chart.idle_since.is_none() {
                    chart.idle_since = Some(now);
                    self.idle += 1;
                }
            } else {
                chart.series.retain(|k, _| chart.demand.contains_key(k));
            }
        }
        self.sweep(now);
    }

    /// Drops the charts idle for longer than [`IDLE_DROP`].
    fn sweep(&mut self, now: Instant) {
        if self.idle == 0 {
            return;
        }
        let before = self.charts.len();
        self.charts.retain(|_, c| {
            c.idle_since
                .is_none_or(|t| now.duration_since(t) < IDLE_DROP)
        });
        if self.charts.len() != before {
            self.idle = self
                .charts
                .values()
                .filter(|c| c.idle_since.is_some())
                .count();
            self.symbols = self.charts.keys().map(|k| k.symbol.clone()).collect();
        }
    }

    /// The engine was replaced (a seek or day switch): every series goes, the demand stays, and
    /// the resync snapshots rebuild what is still asked for.
    pub fn clear_computed(&mut self) {
        for c in self.charts.values_mut() {
            c.series.clear();
        }
    }

    /// The engine adopted `symbol`'s history (live): its series were built from a partial chart.
    /// The `Loaded` snapshot that follows rebuilds them.
    pub fn invalidate_symbol(&mut self, symbol: &str) {
        if !self.symbols.contains(symbol) {
            return;
        }
        for (k, c) in self.charts.iter_mut() {
            if k.symbol == symbol {
                c.series.clear();
            }
        }
    }

    /// A `chart.indicators` frame for `studies`, building any series that is missing from the
    /// engine and dropping bars the engine no longer holds. Call with the same core lock as the
    /// `chart.snapshot` it accompanies, so both are at one seq.
    pub fn snapshot(&mut self, engine: &ChartEngine, studies: &Studies) -> String {
        let key = &studies.chart;
        if studies.specs.is_empty() {
            // A slot with every study off: an empty frame, and no chart entry nobody would
            // ever release.
            return snapshot_json(key, std::iter::empty());
        }
        let chart = self.charts.entry(key.clone()).or_default();
        self.symbols.insert(key.symbol.clone());
        let cap = engine.config().max_intervals_per_timeframe;
        let mut bars = None;
        for s in &studies.specs {
            if let std::collections::btree_map::Entry::Vacant(slot) = chart.series.entry(s.key()) {
                let input = bars
                    .get_or_insert_with(|| {
                        engine
                            .indicator_bars(&key.symbol, key.timeframe, key.rth)
                            .unwrap_or_default()
                    })
                    .clone();
                slot.insert(IndicatorSeries::build(*s, input, cap));
            }
        }
        if let Some(first) = engine.first_interval_ms(&key.symbol, key.timeframe) {
            for s in chart.series.values_mut() {
                s.trim_before(first);
            }
        }
        // In the slot's own order, as `computeIndicators` handed out colour slots.
        let series = &chart.series;
        snapshot_json(
            key,
            studies
                .specs
                .iter()
                .filter_map(|s| series.get_key_value(&s.key())),
        )
    }

    /// Folds one engine apply's increments into every computed series they touch and returns
    /// the frames to publish: per chart key that moved, a `chart.indicator`, or its whole
    /// `chart.indicators` when the increment would be too large.
    pub fn on_updates(&mut self, ups: &[ChartUpdate]) -> Vec<(ChartKey, String)> {
        if self.symbols.is_empty() {
            return Vec::new();
        }
        if self.idle > 0 {
            self.sweep(Instant::now());
        }
        // Per chart key touched: the bars changed and removed, by time.
        let mut touched: Vec<Touched> = Vec::new();
        for u in ups {
            let Some(tf) = u.timeframe() else {
                continue;
            };
            if !self.symbols.contains(u.symbol()) {
                continue;
            }
            for rth in [false, true] {
                let key = ChartKey {
                    symbol: u.symbol().to_string(),
                    timeframe: tf,
                    rth,
                };
                let Some(chart) = self.charts.get_mut(&key) else {
                    continue;
                };
                if chart.series.is_empty() {
                    continue;
                }
                let Some(edit) = BarEdit::of(u, rth) else {
                    continue;
                };
                let i = match touched.iter().position(|(k, _, _)| *k == key) {
                    Some(i) => i,
                    None => {
                        touched.push((key, BTreeMap::new(), BTreeSet::new()));
                        touched.len() - 1
                    }
                };
                let (_, changed, removed) = &mut touched[i];
                if let BarEdit::Remove(t) = edit {
                    removed.insert(t);
                }
                for (k, s) in chart.series.iter_mut() {
                    if let Some(r) = s.apply(edit) {
                        let set = changed.entry(k.clone()).or_default();
                        set.extend(r.map(|i| s.t(i)));
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (key, changed, removed) in touched {
            let chart = &self.charts[&key];
            // The series of one chart hold the same bars.
            let Some(any) = chart.series.values().next() else {
                continue;
            };
            let exists = |t: &i64| any.bars().binary_search_by_key(t, |b| b.t_ms).is_ok();
            let removed: Vec<i64> = removed.into_iter().filter(|t| !exists(t)).collect();
            let mut points: BTreeMap<i64, BTreeMap<String, Vec<Option<f64>>>> = BTreeMap::new();
            for (k, ts) in &changed {
                let s = &chart.series[k];
                for t in ts {
                    // Changed, then removed again within this apply.
                    let Ok(i) = s.bars().binary_search_by_key(t, |b| b.t_ms) else {
                        continue;
                    };
                    let v = s.values(i).iter().map(|x| opt(*x)).collect();
                    points.entry(*t).or_default().insert(k.clone(), v);
                }
            }
            if points.is_empty() && removed.is_empty() {
                continue;
            }
            let json = if points.len() > MAX_INCREMENT_POINTS {
                snapshot_json(&key, chart.series.iter())
            } else {
                serde_json::to_string(&IncrementWire {
                    kind: "chart.indicator",
                    symbol: &key.symbol,
                    timeframe: key.timeframe,
                    rth: key.rth,
                    points: points
                        .into_iter()
                        .map(|(t, v)| PointWire { t, v })
                        .collect(),
                    removed,
                })
                .expect("indicator frames serialize")
            };
            out.push((key, json));
        }
        out
    }

    /// Series currently computed (tests and the cost measurement).
    pub fn computed(&self) -> usize {
        self.charts.values().map(|c| c.series.len()).sum()
    }
}

/// `chart.subscribe` / `chart.indicators.set` input: the slot's list (`indicators`) and whether
/// extended hours are off (`rth`). `Ok(None)` when the client sent no list (every phone build
/// installed before `indicators.v1`, which computes its own); `Err` for a list that is not one.
pub fn parse_request(
    symbol: &str,
    timeframe: Timeframe,
    indicators: Option<&Value>,
    rth: Option<bool>,
) -> Result<Option<Studies>, &'static str> {
    let Some(raw) = indicators else {
        return Ok(None);
    };
    let parsed = IndicatorSpec::parse_list(raw)?;
    Ok(Some(Studies::new(
        ChartKey {
            symbol: symbol.to_string(),
            timeframe,
            rth: rth.unwrap_or(false),
        },
        parsed,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_events::session::et;
    use market_events::{MarketEvent, Price, Trade};
    use serde_json::json;

    fn day() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    }

    fn trade(h: u32, m: u32, s: u32, cents: i64, seq: u64) -> MarketEvent {
        MarketEvent::Trade(Trade::new(
            "SMPL",
            et(day(), h, m, s, 0).unwrap(),
            Price::from_cents(cents),
            100,
            seq,
        ))
    }

    fn studies(list: Value, rth: bool) -> Studies {
        parse_request("SMPL", Timeframe::M1, Some(&list), Some(rth))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn increments_follow_trades_and_only_name_what_moved() {
        let mut engine = ChartEngine::default();
        let mut reg = IndicatorRegistry::default();
        for (i, m) in (0..30).enumerate() {
            engine.apply(&trade(10, m, 5, 300 + (i as i64 % 7), i as u64 + 1));
        }
        let st = studies(
            json!([{"id": "e", "kind": "ema", "period": 3}, {"id": "v", "kind": "vwap"}]),
            false,
        );
        reg.acquire(&st);
        let snap: Value = serde_json::from_str(&reg.snapshot(&engine, &st)).unwrap();
        assert_eq!(snap["type"], "chart.indicators");
        assert_eq!(snap["t"].as_array().unwrap().len(), 30);
        assert_eq!(snap["studies"][0]["key"], "ema:3");
        assert_eq!(snap["studies"][0]["lines"][0]["from"], 2);
        // A trade at the same price in the forming bar: EMA unchanged, VWAP's volume moved.
        let ups = engine.apply(&trade(10, 29, 30, 300 + 29 % 7, 100));
        let frames = reg.on_updates(&ups);
        assert_eq!(frames.len(), 1);
        let f: Value = serde_json::from_str(&frames[0].1).unwrap();
        assert_eq!(f["type"], "chart.indicator");
        assert_eq!(f["points"].as_array().unwrap().len(), 1);
        let v = f["points"][0]["v"].as_object().unwrap();
        assert!(v.contains_key("vwap") && !v.contains_key("ema:3"), "{f}");
        // Nobody watches 5m or rth: no frames for those.
        assert!(
            frames
                .iter()
                .all(|(k, _)| k.timeframe == Timeframe::M1 && !k.rth)
        );
    }

    #[test]
    fn a_deep_revision_sends_a_snapshot_instead() {
        let mut engine = ChartEngine::default();
        let mut reg = IndicatorRegistry::default();
        // 300 one-minute bars from 09:30. The settle window refuses an old trade, so the revision
        // of the first bar is handed to the fold as the engine would emit it.
        for i in 0..300u32 {
            engine.apply(&trade(
                9 + (30 + i) / 60,
                (30 + i) % 60,
                1,
                300 + (i % 11) as i64,
                i as u64 + 1,
            ));
        }
        // VWAP: a changed early bar moves every later value of the day (an EMA forgets it
        // to the last bit within ~170 bars, and the tightened range then stays small).
        let st = studies(json!([{"id": "v", "kind": "vwap"}]), true);
        reg.acquire(&st);
        reg.snapshot(&engine, &st);
        let chart = reg.charts.get_mut(&st.chart).unwrap();
        let s = chart.series.values_mut().next().unwrap();
        let mut b = s.bars()[0];
        b.c += 0.5;
        // Pretend the engine revised the first bar: the fold sees a chart.update for it.
        let candle = chart_engine::wire::CandleWire {
            t: b.t_ms,
            o: b.c,
            h: b.h.max(b.c),
            l: b.l,
            c: b.c,
            v: b.v as u64,
            n: 1,
            is_final: true,
        };
        let u = ChartUpdate::Candle(chart_engine::wire::CandleUpdate {
            symbol: "SMPL".into(),
            timeframe: Timeframe::M1,
            session: "2026-10-15/regular".into(),
            candle,
            profile_bin: 0.01,
            profile: Vec::new(),
        });
        let frames = reg.on_updates(&[u]);
        let f: Value = serde_json::from_str(&frames[0].1).unwrap();
        assert_eq!(
            f["type"], "chart.indicators",
            "{} points would be too many",
            300
        );
        assert_eq!(f["rth"], true);
    }

    #[test]
    fn demand_is_shared_and_idle_charts_linger() {
        let engine = ChartEngine::default();
        let mut reg = IndicatorRegistry::default();
        let a = studies(
            json!([{"id": "x", "kind": "ema"}, {"id": "y", "kind": "rsi"}]),
            false,
        );
        let b = studies(
            json!([{"id": "other-id", "kind": "ema", "period": 9}]),
            false,
        );
        reg.acquire(&a);
        reg.acquire(&b);
        reg.snapshot(&engine, &a);
        assert_eq!(reg.computed(), 2, "ema:9 once for both slots");
        reg.release(&a);
        assert_eq!(reg.computed(), 1, "rsi went, b still holds ema:9");
        reg.release(&b);
        assert_eq!(reg.computed(), 1, "idle, not dropped yet");
        reg.charts.get_mut(&a.chart).unwrap().idle_since =
            Some(Instant::now() - IDLE_DROP - Duration::from_secs(1));
        reg.sweep(Instant::now());
        assert_eq!(reg.computed(), 0);
        assert!(reg.symbols.is_empty());
    }

    #[test]
    fn requests_parse_like_the_client() {
        assert!(
            parse_request("S", Timeframe::M1, None, Some(true))
                .unwrap()
                .is_none()
        );
        assert!(parse_request("S", Timeframe::M1, Some(&json!("ema")), None).is_err());
        let s = parse_request(
            "S",
            Timeframe::M1,
            Some(&json!([{"id": "a", "kind": "ema"}, {"id": "b", "kind": "ema", "period": 9}])),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(s.specs.len(), 1, "same key, one study");
        assert!(!s.chart.rth);
    }
}
