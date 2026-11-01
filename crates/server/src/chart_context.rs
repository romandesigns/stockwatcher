//! Chart context computed here and pushed: the **key levels and opening ranges** (`levels.v1`)
//! and the **automatic supply/demand zones** (`zones.v1`) the charts draw. The clients used to
//! work all three out from the candles they held (packages/chart-core `keyLevels`,
//! `openingRanges`, `autoZones`); now Rust computes them once per chart and the clients only
//! render (owner rule: heavy and realtime work runs in Rust and is pushed). Owned by the market
//! core next to the chart engine, under the same lock (`Core::chart_context`), like the indicator
//! registry (crate::indicators), whose demand and lease pattern it follows.
//!
//! # What is computed
//!
//! - **Levels** (chart_engine::levels: premarket high/low, high/low of day, the regular open, the
//!   previous close) are a property of the symbol, read off its 1-minute candles once per symbol.
//!   The previous close is the server's official one (the closes file, as `chart.snapshot`'s
//!   `prevClose`) when known, else the latest earlier regular close the engine still holds: the
//!   client's rule, decided here.
//! - **Opening ranges** belong to (symbol, timeframe): the first regular candle of THAT timeframe
//!   per trading day, `forming` while it is open, ending at the NYSE calendar's regular close
//!   (13:00 on an early close, which the clients only knew for the current session).
//! - **Zones** belong to a chart key (symbol, timeframe, rth): crate::auto_zones' rule (the TS
//!   rule ported bit for bit) over every retained candle of that timeframe, the forming one
//!   included as the client did, regular session only with rth. The Runners rows' demand-zone
//!   mark reads the same results for (symbol, 1m, extended hours) (crate::auto_zones::ZoneCache),
//!   so the mark and the chart a tap opens never disagree.
//!
//! # When
//!
//! - **Levels and ranges** on every engine increment of a watched symbol, but only recomputed
//!   when chart_engine's skip rules say the increment can move a value (a new extreme, a revision
//!   of the candle that set one, a new day, a removal): most trades cost a comparison. A frame
//!   goes out only when the frame's content changed.
//! - **Zones** at once when a bar finalizes, or an older bar is revised or removed; a change to
//!   the forming bar alone marks the key dirty, and it is recomputed at most once per
//!   [`ZONE_FORMING_EVERY`] (inline when the last computation is older, else by the 1 Hz
//!   broadcast tick's [`ContextRegistry::flush`]). The limit is wall time on purpose: it budgets
//!   CPU under the core lock, and a replay at 10x must not cost 10x. It decides nothing about the
//!   market, so it is not the market clock's business.
//!
//! # Frames (JSON, times in epoch ms like `CandleWire.t`; absent values `null`)
//!
//! - `chart.levels {symbol, timeframe, day, pmHigh, pmLow, hod, lod, open, prevClose,
//!   openingRanges: [{day, t, end, high, low, forming}]}`: the whole state, replacing the
//!   client's. One per chart (symbol, timeframe), although the levels are the symbol's: the
//!   client keys everything by chart and routes by it, and a frame per symbol would need its own
//!   ordering across timeframes. Sent after a slot's `chart.snapshot`, on every resync, and
//!   broadcast when its content changes. `day` is null (and everything empty) for a symbol with
//!   no candle yet.
//! - `chart.zones {symbol, timeframe, rth, zones: [{kind, lo, hi, t, tested}]}`: the whole set,
//!   nearest first per side then by `t` as autoZones gave them. Only to a slot that asked
//!   (`zones: true`, the Levels menu layer on); sent when it asks, on every resync, and broadcast
//!   when the set changes.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chart_engine::indicators::Bar;
use chart_engine::levels::{KeyLevels, OpeningRange, ranges_may_change};
use chart_engine::wire::ChartUpdate;
use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_events::session::SessionId;
use serde::Serialize;

use crate::auto_zones::{AutoZone, ZoneKind, auto_zones};
use crate::indicators::ChartKey;

/// The `welcome.features` entry for `chart.levels`. A client asks (`levels: true` on
/// `chart.subscribe`) only when it sees it, so a new client on an old server computes its own.
pub const LEVELS_FEATURE: &str = "levels.v1";
/// The `welcome.features` entry for `chart.zones` (`zones: true` on `chart.subscribe` /
/// `chart.indicators.set`).
pub const ZONES_FEATURE: &str = "zones.v1";

/// How often, at most, a change to the forming bar alone recomputes a chart's zones.
pub const ZONE_FORMING_EVERY: Duration = Duration::from_secs(1);

/// The timeframe the symbol's levels are read from. Any would give the same extremes (the same
/// trades); 1 minute holds a whole 04:00–20:00 day in the ring with the fewest buckets.
const LEVELS_TF: Timeframe = Timeframe::M1;

/// A chart's levels frame: (symbol, timeframe).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LevelsKey {
    pub symbol: String,
    pub timeframe: Timeframe,
}

/// What a slot (or the Runners mark) holds demand for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextKey {
    Levels(LevelsKey),
    Zones(ChartKey),
}

/// A frame to broadcast, with what it is for.
#[derive(Debug)]
pub enum ContextFrame {
    Levels(LevelsKey, String),
    Zones(ChartKey, String),
}

#[derive(Debug, Default)]
struct LevelsChart {
    demand: u32,
    ranges: Vec<OpeningRange>,
    /// The last frame sent for this chart (snapshot or broadcast): an unchanged one is not resent.
    sent: Option<String>,
}

#[derive(Debug, Default)]
struct ZonesChart {
    /// Slots asking for `chart.zones`.
    slots: u32,
    /// Readers inside the server (the Runners mark): kept computed, never broadcast for them.
    internal: u32,
    /// `None` until computed (or after the engine changed underneath).
    zones: Option<Vec<AutoZone>>,
    /// The forming bar changed since `computed_at`.
    dirty: bool,
    computed_at: Option<Instant>,
    sent: Option<String>,
}

#[derive(Debug, Default)]
pub struct ContextRegistry {
    /// Per symbol with any levels chart: its levels, `None` until computed.
    levels: HashMap<String, Option<KeyLevels>>,
    charts: HashMap<LevelsKey, LevelsChart>,
    zones: HashMap<ChartKey, ZonesChart>,
    /// Symbols with any demand, so an increment for one of the other ten thousand costs a lookup.
    symbols: HashSet<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RangeWire {
    day: String,
    t: i64,
    end: i64,
    high: f64,
    low: f64,
    forming: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LevelsWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    symbol: &'a str,
    timeframe: Timeframe,
    day: Option<String>,
    pm_high: Option<f64>,
    pm_low: Option<f64>,
    hod: Option<f64>,
    lod: Option<f64>,
    open: Option<f64>,
    prev_close: Option<f64>,
    opening_ranges: Vec<RangeWire>,
}

#[derive(Serialize)]
struct ZoneWire {
    kind: &'static str,
    lo: f64,
    hi: f64,
    t: i64,
    tested: bool,
}

#[derive(Serialize)]
struct ZonesWire<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    symbol: &'a str,
    timeframe: Timeframe,
    rth: bool,
    zones: Vec<ZoneWire>,
}

/// The official previous close of `symbol` before `day`, in dollars (the quote board's closes).
pub type PrevCloseFn<'a> = &'a dyn Fn(&str, NaiveDate) -> Option<f64>;

fn levels_json(
    key: &LevelsKey,
    levels: Option<&KeyLevels>,
    ranges: &[OpeningRange],
    prev_close: PrevCloseFn,
) -> String {
    let official = levels.and_then(|k| prev_close(&key.symbol, k.day).filter(|p| *p > 0.0));
    serde_json::to_string(&LevelsWire {
        kind: "chart.levels",
        symbol: &key.symbol,
        timeframe: key.timeframe,
        day: levels.map(|k| k.day.to_string()),
        pm_high: levels.and_then(|k| k.pm_high),
        pm_low: levels.and_then(|k| k.pm_low),
        hod: levels.and_then(|k| k.hod),
        lod: levels.and_then(|k| k.lod),
        open: levels.and_then(|k| k.open),
        // The official close wins over the one read off the candles (levels.ts rule).
        prev_close: official.or(levels.and_then(|k| k.prev_close)),
        opening_ranges: ranges
            .iter()
            .map(|r| RangeWire {
                day: r.day.to_string(),
                t: r.t,
                end: r.end,
                high: r.high,
                low: r.low,
                forming: r.forming,
            })
            .collect(),
    })
    .expect("levels frames serialize")
}

fn zones_json(key: &ChartKey, zones: &[AutoZone]) -> String {
    serde_json::to_string(&ZonesWire {
        kind: "chart.zones",
        symbol: &key.symbol,
        timeframe: key.timeframe,
        rth: key.rth,
        zones: zones
            .iter()
            .map(|z| ZoneWire {
                kind: match z.kind {
                    ZoneKind::Supply => "supply",
                    ZoneKind::Demand => "demand",
                },
                lo: z.lo,
                hi: z.hi,
                t: z.t,
                tested: z.tested,
            })
            .collect(),
    })
    .expect("zone frames serialize")
}

fn compute_zones(engine: &ChartEngine, key: &ChartKey) -> Vec<AutoZone> {
    engine
        .chart_candles(&key.symbol, key.timeframe, key.rth)
        .map(|cs| auto_zones(&cs))
        .unwrap_or_default()
}

impl ContextRegistry {
    /// A slot took `key` (subscribe, or a zones change on `chart.indicators.set`).
    pub fn acquire(&mut self, key: &ContextKey) {
        match key {
            ContextKey::Levels(k) => {
                self.charts.entry(k.clone()).or_default().demand += 1;
                self.levels.entry(k.symbol.clone()).or_default();
                self.symbols.insert(k.symbol.clone());
            }
            ContextKey::Zones(k) => {
                self.zones.entry(k.clone()).or_default().slots += 1;
                self.symbols.insert(k.symbol.clone());
            }
        }
    }

    /// A slot let go of `key`. Nothing lingers: a recompute from the engine is cheap and exact,
    /// so a chart that comes back simply computes again.
    pub fn release(&mut self, key: &ContextKey) {
        match key {
            ContextKey::Levels(k) => {
                if let Some(c) = self.charts.get_mut(k) {
                    c.demand -= 1;
                    if c.demand == 0 {
                        self.charts.remove(k);
                        if !self.charts.keys().any(|o| o.symbol == k.symbol) {
                            self.levels.remove(&k.symbol);
                        }
                    }
                }
            }
            ContextKey::Zones(k) => {
                if let Some(z) = self.zones.get_mut(k) {
                    z.slots -= 1;
                    if z.slots == 0 && z.internal == 0 {
                        self.zones.remove(k);
                    }
                }
            }
        }
        self.symbols = self
            .charts
            .keys()
            .map(|k| k.symbol.clone())
            .chain(self.zones.keys().map(|k| k.symbol.clone()))
            .collect();
    }

    /// A reader inside the server (the Runners mark) keeps `key`'s zones computed.
    pub fn acquire_internal(&mut self, key: &ChartKey) {
        self.zones.entry(key.clone()).or_default().internal += 1;
        self.symbols.insert(key.symbol.clone());
    }

    pub fn release_internal(&mut self, key: &ChartKey) {
        if let Some(z) = self.zones.get_mut(key) {
            z.internal -= 1;
            if z.slots == 0 && z.internal == 0 {
                self.zones.remove(key);
                self.symbols = self
                    .charts
                    .keys()
                    .map(|k| k.symbol.clone())
                    .chain(self.zones.keys().map(|k| k.symbol.clone()))
                    .collect();
            }
        }
    }

    /// The engine was replaced (a seek or day switch): every computed value goes, the demand
    /// stays, and the resync snapshots compute afresh.
    pub fn clear_computed(&mut self) {
        for l in self.levels.values_mut() {
            *l = None;
        }
        for c in self.charts.values_mut() {
            c.ranges.clear();
            c.sent = None;
        }
        for z in self.zones.values_mut() {
            z.zones = None;
            z.dirty = false;
            z.sent = None;
        }
    }

    /// The engine adopted `symbol`'s history (live): what was computed from the partial chart
    /// goes; the `Loaded` snapshots that follow compute afresh.
    pub fn invalidate_symbol(&mut self, symbol: &str) {
        if !self.symbols.contains(symbol) {
            return;
        }
        if let Some(l) = self.levels.get_mut(symbol) {
            *l = None;
        }
        for (k, c) in self.charts.iter_mut() {
            if k.symbol == symbol {
                c.ranges.clear();
                c.sent = None;
            }
        }
        for (k, z) in self.zones.iter_mut() {
            if k.symbol == symbol {
                z.zones = None;
                z.sent = None;
            }
        }
    }

    /// A `chart.levels` frame for `key`, computed afresh from the engine (a snapshot never trusts
    /// a cache). Call under the core lock, like the `chart.snapshot` it follows.
    pub fn levels_frame(
        &mut self,
        engine: &ChartEngine,
        prev_close: PrevCloseFn,
        key: &LevelsKey,
    ) -> String {
        let levels = engine.key_levels(&key.symbol, LEVELS_TF);
        let ranges = engine.opening_ranges(&key.symbol, key.timeframe);
        let json = levels_json(key, levels.as_ref(), &ranges, prev_close);
        // Only a chart someone holds keeps the values (a frame for a key nobody holds yet, e.g.
        // asked just before its lease, would otherwise leave an entry nobody releases).
        if let Some(c) = self.charts.get_mut(key) {
            c.ranges = ranges;
            c.sent = Some(json.clone());
            if let Some(l) = self.levels.get_mut(&key.symbol) {
                *l = levels;
            }
        }
        json
    }

    /// A `chart.zones` frame for `key`, computed afresh from the engine.
    pub fn zones_frame(&mut self, engine: &ChartEngine, key: &ChartKey, now: Instant) -> String {
        let zones = compute_zones(engine, key);
        let json = zones_json(key, &zones);
        if let Some(z) = self.zones.get_mut(key) {
            z.zones = Some(zones);
            z.dirty = false;
            z.computed_at = Some(now);
            z.sent = Some(json.clone());
        }
        json
    }

    /// `key`'s zones for a reader inside the server (the Runners mark, which holds
    /// [`ContextRegistry::acquire_internal`]): the kept results, computed if there are none.
    pub fn zones_for(
        &mut self,
        engine: &ChartEngine,
        key: &ChartKey,
        now: Instant,
    ) -> Vec<AutoZone> {
        let Some(z) = self.zones.get_mut(key) else {
            return compute_zones(engine, key);
        };
        if z.zones.is_none() {
            z.zones = Some(compute_zones(engine, key));
            z.dirty = false;
            z.computed_at = Some(now);
        }
        z.zones.clone().unwrap_or_default()
    }

    /// Follows one engine apply's increments (module docs, "When") and returns the frames whose
    /// content changed.
    pub fn on_updates(
        &mut self,
        engine: &ChartEngine,
        prev_close: PrevCloseFn,
        ups: &[ChartUpdate],
        now: Instant,
    ) -> Vec<ContextFrame> {
        if self.symbols.is_empty() {
            return Vec::new();
        }
        let mut levels_due: Vec<String> = Vec::new();
        let mut ranges_due: Vec<LevelsKey> = Vec::new();
        let mut zones_due: Vec<ChartKey> = Vec::new();
        for u in ups {
            let Some(tf) = u.timeframe() else { continue };
            if !self.symbols.contains(u.symbol()) {
                continue;
            }
            let symbol = u.symbol();
            let candle = match u {
                ChartUpdate::Candle(c) => match c.session.parse::<SessionId>() {
                    Ok(s) => Some((s, &c.candle)),
                    Err(_) => continue,
                },
                // A removal (a cancel's emptied interval, a gap over a former candle) can take
                // away whatever a value came from: always recompute.
                _ => None,
            };
            if tf == LEVELS_TF
                && let Some(cached) = self.levels.get(symbol)
                && !levels_due.iter().any(|s| s == symbol)
            {
                let due = match (cached, candle) {
                    (Some(k), Some((s, c))) => k.may_change(s, c),
                    _ => true,
                };
                if due {
                    levels_due.push(symbol.to_string());
                }
            }
            let lk = LevelsKey {
                symbol: symbol.to_string(),
                timeframe: tf,
            };
            if let Some(c) = self.charts.get(&lk)
                && !ranges_due.contains(&lk)
                && candle.is_none_or(|(s, k)| ranges_may_change(&c.ranges, s, k.t))
            {
                ranges_due.push(lk);
            }
            for rth in [false, true] {
                let key = ChartKey {
                    symbol: symbol.to_string(),
                    timeframe: tf,
                    rth,
                };
                let Some(z) = self.zones.get_mut(&key) else {
                    continue;
                };
                match candle {
                    Some((s, _)) if !Bar::keep(s, rth) => {}
                    // The forming bar: throttled.
                    Some((_, c)) if !c.is_final => {
                        z.dirty = true;
                        let stale = z
                            .computed_at
                            .is_none_or(|t| now.duration_since(t) >= ZONE_FORMING_EVERY);
                        if stale && !zones_due.contains(&key) {
                            zones_due.push(key);
                        }
                    }
                    _ => {
                        if !zones_due.contains(&key) {
                            zones_due.push(key);
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for symbol in &levels_due {
            let fresh = engine.key_levels(symbol, LEVELS_TF);
            let changed = self.levels.get(symbol).is_none_or(|old| *old != fresh);
            self.levels.insert(symbol.clone(), fresh);
            if changed {
                for k in self.charts.keys() {
                    if k.symbol == *symbol && !ranges_due.contains(k) {
                        ranges_due.push(k.clone());
                    }
                }
            }
        }
        for key in ranges_due {
            let ranges = engine.opening_ranges(&key.symbol, key.timeframe);
            let levels = self.levels.get(&key.symbol).and_then(|l| l.as_ref());
            let json = levels_json(&key, levels, &ranges, prev_close);
            let Some(c) = self.charts.get_mut(&key) else {
                continue;
            };
            c.ranges = ranges;
            if c.sent.as_deref() != Some(json.as_str()) {
                c.sent = Some(json.clone());
                out.push(ContextFrame::Levels(key, json));
            }
        }
        for key in zones_due {
            out.extend(self.recompute_zones(engine, key, now));
        }
        out
    }

    /// The zones whose forming bar changed and whose last computation is at least
    /// [`ZONE_FORMING_EVERY`] old: from the 1 Hz broadcast tick.
    pub fn flush(&mut self, engine: &ChartEngine, now: Instant) -> Vec<ContextFrame> {
        let due: Vec<ChartKey> = self
            .zones
            .iter()
            .filter(|(_, z)| {
                z.dirty
                    && z.computed_at
                        .is_none_or(|t| now.duration_since(t) >= ZONE_FORMING_EVERY)
            })
            .map(|(k, _)| k.clone())
            .collect();
        due.into_iter()
            .filter_map(|k| self.recompute_zones(engine, k, now))
            .collect()
    }

    fn recompute_zones(
        &mut self,
        engine: &ChartEngine,
        key: ChartKey,
        now: Instant,
    ) -> Option<ContextFrame> {
        let zones = compute_zones(engine, &key);
        let z = self.zones.get_mut(&key)?;
        z.dirty = false;
        z.computed_at = Some(now);
        if z.zones.as_ref() == Some(&zones) && z.sent.is_some() {
            return None;
        }
        z.zones = Some(zones);
        // Only slots get frames; the Runners mark reads `zones` directly.
        if z.slots == 0 {
            return None;
        }
        let json = zones_json(&key, z.zones.as_deref().unwrap_or_default());
        if z.sent.as_deref() == Some(json.as_str()) {
            return None;
        }
        z.sent = Some(json.clone());
        Some(ContextFrame::Zones(key, json))
    }

    /// Charts with levels demand and chart keys with zones (tests and the cost measurement).
    pub fn held(&self) -> (usize, usize) {
        (self.charts.len(), self.zones.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chart_engine::wire::CandleWire;
    use market_events::session::{SessionKind, classify, et};
    use market_events::{MarketEvent, Price, Trade, UnixNanos};
    use serde_json::Value;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
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

    fn no_close(_: &str, _: NaiveDate) -> Option<f64> {
        None
    }

    fn lkey(tf: Timeframe) -> LevelsKey {
        LevelsKey {
            symbol: "SMPL".into(),
            timeframe: tf,
        }
    }

    fn zkey(tf: Timeframe, rth: bool) -> ChartKey {
        ChartKey {
            symbol: "SMPL".into(),
            timeframe: tf,
            rth,
        }
    }

    fn levels_of(frames: &[ContextFrame]) -> Vec<Value> {
        frames
            .iter()
            .filter_map(|f| match f {
                ContextFrame::Levels(_, j) => Some(serde_json::from_str(j).unwrap()),
                _ => None,
            })
            .collect()
    }

    /// The snapshot carries every level and the opening range; a trade inside the range sends
    /// nothing; a new high of day sends the new state; the official close wins over the candles'.
    #[test]
    fn levels_follow_the_trades_that_move_them() {
        let mut e = ChartEngine::default();
        let mut ctx = ContextRegistry::default();
        e.apply(&trade(8, 0, 5, 300, 1));
        e.apply(&trade(9, 10, 5, 280, 2));
        e.apply(&trade(9, 30, 5, 310, 3));
        e.apply(&trade(9, 30, 40, 320, 4));
        ctx.acquire(&ContextKey::Levels(lkey(Timeframe::M1)));
        ctx.acquire(&ContextKey::Levels(lkey(Timeframe::M5)));
        let official = |_: &str, d: NaiveDate| (d == day()).then_some(2.5);
        let snap: Value =
            serde_json::from_str(&ctx.levels_frame(&e, &official, &lkey(Timeframe::M1))).unwrap();
        assert_eq!(snap["type"], "chart.levels");
        assert_eq!(snap["day"], "2026-10-15");
        assert_eq!(
            (snap["pmHigh"].as_f64(), snap["pmLow"].as_f64()),
            (Some(3.0), Some(2.8))
        );
        assert_eq!(
            (snap["hod"].as_f64(), snap["lod"].as_f64()),
            (Some(3.2), Some(3.1))
        );
        assert_eq!(snap["open"], 3.1);
        assert_eq!(snap["prevClose"], 2.5, "the official close");
        let r = &snap["openingRanges"][0];
        assert_eq!(
            (r["high"].as_f64(), r["low"].as_f64()),
            (Some(3.2), Some(3.1))
        );
        assert_eq!(r["forming"], true);
        assert_eq!(r["end"], et(day(), 16, 0, 0, 0).unwrap().millis());
        ctx.levels_frame(&e, &official, &lkey(Timeframe::M5));

        // The 09:31 trade finalizes the 1m opening candle: only that chart's range flips.
        let ups = e.apply(&trade(9, 31, 0, 315, 5));
        let lv = levels_of(&ctx.on_updates(&e, &official, &ups, Instant::now()));
        assert_eq!(lv.len(), 1, "{lv:?}");
        assert_eq!(lv[0]["timeframe"], "1m");
        assert_eq!(lv[0]["openingRanges"][0]["forming"], false);
        // Inside the day's range, after the first candle: nothing moves.
        let ups = e.apply(&trade(9, 31, 5, 312, 6));
        assert!(
            ctx.on_updates(&e, &official, &ups, Instant::now())
                .is_empty()
        );
        // A new high: both charts, which share the symbol's levels.
        let ups = e.apply(&trade(9, 31, 9, 340, 7));
        let lv = levels_of(&ctx.on_updates(&e, &official, &ups, Instant::now()));
        assert_eq!(lv.len(), 2);
        assert!(lv.iter().all(|f| f["hod"] == 3.4));
        // The 5m opening candle (09:30-09:35) took the new high; the 1m one is final.
        let m5 = lv.iter().find(|f| f["timeframe"] == "5m").unwrap();
        assert_eq!(m5["openingRanges"][0]["high"], 3.4);
        // Released: no more frames, nothing kept.
        ctx.release(&ContextKey::Levels(lkey(Timeframe::M1)));
        ctx.release(&ContextKey::Levels(lkey(Timeframe::M5)));
        let ups = e.apply(&trade(9, 32, 0, 400, 8));
        assert!(
            ctx.on_updates(&e, &official, &ups, Instant::now())
                .is_empty()
        );
        assert_eq!(ctx.held(), (0, 0));
        assert!(ctx.levels.is_empty() && ctx.symbols.is_empty());
    }

    /// The forming bar recomputes the zones at most once per ZONE_FORMING_EVERY (inline or by
    /// `flush`); a finalized bar recomputes at once; an unchanged set sends nothing.
    #[test]
    fn zones_follow_finalized_bars_and_throttle_the_forming_one() {
        let mut e = ChartEngine::default();
        let mut ctx = ContextRegistry::default();
        let key = zkey(Timeframe::M1, false);
        ctx.acquire(&ContextKey::Zones(key.clone()));
        let t0 = Instant::now();
        let snap: Value = serde_json::from_str(&ctx.zones_frame(&e, &key, t0)).unwrap();
        assert_eq!(snap["type"], "chart.zones");
        assert_eq!(snap["zones"], serde_json::json!([]));
        // 14 ordinary minutes, then a quiet base and a decisive rally: a demand zone.
        let mut seq = 0;
        let mut minute = |e: &mut ChartEngine, m: u32, o: i64, h: i64, l: i64, c: i64| {
            let mut ups = Vec::new();
            for (s, p) in [(1, o), (10, h), (20, l), (50, c)] {
                seq += 1;
                ups.extend(e.apply(&MarketEvent::Trade(Trade::new(
                    "SMPL",
                    et(day(), 10, m, s, 0).unwrap(),
                    Price::from_cents(p),
                    100,
                    seq,
                ))));
            }
            ups
        };
        for m in 0..14 {
            minute(&mut e, m, 500, 506, 496, if m % 2 == 1 { 504 } else { 498 });
        }
        minute(&mut e, 14, 500, 504, 496, 501);
        minute(&mut e, 15, 501, 505, 498, 500);
        minute(&mut e, 16, 500, 540, 499, 538);
        // The 10:17 trades finalize 10:16: the zone appears at once, whatever the throttle.
        let ups = minute(&mut e, 17, 538, 550, 536, 548);
        let frames = ctx.on_updates(&e, &no_close, &ups, t0);
        let zones: Vec<Value> = frames
            .iter()
            .filter_map(|f| match f {
                ContextFrame::Zones(k, j) if *k == key => Some(serde_json::from_str(j).unwrap()),
                _ => None,
            })
            .collect();
        assert_eq!(zones.len(), 1, "one frame per apply that changed it");
        assert_eq!(zones[0]["zones"][0]["kind"], "demand");
        assert_eq!(zones[0]["zones"][0]["lo"], 4.96);
        // Forming-bar trades within the second: marked, not computed.
        let ups = e.apply(&trade(10, 17, 55, 549, 900));
        assert!(ctx.on_updates(&e, &no_close, &ups, t0).is_empty());
        assert!(ctx.zones[&key].dirty);
        assert!(ctx.flush(&e, t0 + Duration::from_millis(500)).is_empty());
        assert!(ctx.zones[&key].dirty, "still waiting");
        // A second later the flush computes it; the set did not change, so no frame.
        assert!(ctx.flush(&e, t0 + ZONE_FORMING_EVERY).is_empty());
        assert!(!ctx.zones[&key].dirty);
        // 10:18 opens (10:17 final: recomputed at once, unchanged, so no frame), then its forming
        // candle closes below the base, which breaks the zone: throttled, then the flush says so.
        let t1 = t0 + 2 * ZONE_FORMING_EVERY;
        let ups = e.apply(&trade(10, 18, 5, 549, 901));
        assert!(ctx.on_updates(&e, &no_close, &ups, t1).is_empty());
        let ups = e.apply(&trade(10, 18, 30, 480, 902));
        let later = t1 + Duration::from_millis(100);
        assert!(
            ctx.on_updates(&e, &no_close, &ups, later).is_empty(),
            "throttled"
        );
        let frames = ctx.flush(&e, t1 + ZONE_FORMING_EVERY);
        let Some(ContextFrame::Zones(_, j)) = frames.first() else {
            panic!("{frames:?}")
        };
        let v: Value = serde_json::from_str(j).unwrap();
        assert_eq!(
            v["zones"],
            serde_json::json!([]),
            "broken by the forming close"
        );
        // rth=true for the same chart is its own key: nobody asked, nothing computed.
        assert!(!ctx.zones.contains_key(&zkey(Timeframe::M1, true)));
    }

    /// The Runners mark's demand keeps zones computed but never broadcasts them.
    #[test]
    fn internal_demand_is_never_broadcast() {
        let mut e = ChartEngine::default();
        let mut ctx = ContextRegistry::default();
        let key = zkey(Timeframe::M1, false);
        ctx.acquire_internal(&key);
        assert!(ctx.zones_for(&e, &key, Instant::now()).is_empty());
        let ups = e.apply(&trade(10, 0, 1, 500, 1));
        let ups2 = e.apply(&trade(10, 1, 1, 501, 2));
        assert!(
            ctx.on_updates(&e, &no_close, &ups, Instant::now())
                .is_empty()
        );
        assert!(
            ctx.on_updates(&e, &no_close, &ups2, Instant::now())
                .is_empty()
        );
        assert!(ctx.zones[&key].zones.is_some(), "kept computed");
        ctx.release_internal(&key);
        assert_eq!(ctx.held(), (0, 0));
    }

    /// The zones over the engine's sessions equal autoZones over the client's (extended hours on
    /// and off) on the level fixtures, generated by the TS (packages/chart-core/src/
    /// levelFixtures.ts): two full trading days, where the two calendars agree.
    #[test]
    fn zones_match_the_ts_with_and_without_extended_hours() {
        let fx: Vec<Value> = serde_json::from_str(include_str!(
            "../../chart-engine/tests/fixtures/level_fixtures.json"
        ))
        .unwrap();
        let mut zones = 0;
        for f in &fx {
            let all: Vec<(SessionKind, CandleWire)> = f["candles"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    let t = r[0].as_i64().unwrap();
                    let c = CandleWire {
                        t,
                        o: r[1].as_f64().unwrap(),
                        h: r[2].as_f64().unwrap(),
                        l: r[3].as_f64().unwrap(),
                        c: r[4].as_f64().unwrap(),
                        v: 1000,
                        n: 10,
                        is_final: r[5].as_bool().unwrap(),
                    };
                    (classify(UnixNanos::from_millis(t)).unwrap().kind, c)
                })
                .collect();
            for (rth, field) in [(false, "zonesExt"), (true, "zonesRth")] {
                let cs: Vec<CandleWire> = all
                    .iter()
                    .filter(|(k, _)| !rth || *k == SessionKind::Regular)
                    .map(|(_, c)| c.clone())
                    .collect();
                let got = auto_zones(&cs);
                let want = f[field].as_array().unwrap();
                assert_eq!(got.len(), want.len(), "{} {field}", f["name"]);
                // Bit for bit (a JSON comparison would tell 9.0 from the TS's 9).
                for (g, w) in got.iter().zip(want) {
                    let kind = if g.kind == ZoneKind::Demand {
                        "demand"
                    } else {
                        "supply"
                    };
                    assert_eq!(kind, w["kind"], "{}", f["name"]);
                    assert_eq!(
                        g.lo.to_bits(),
                        w["lo"].as_f64().unwrap().to_bits(),
                        "{}",
                        f["name"]
                    );
                    assert_eq!(
                        g.hi.to_bits(),
                        w["hi"].as_f64().unwrap().to_bits(),
                        "{}",
                        f["name"]
                    );
                    assert_eq!(g.t, w["t"].as_i64().unwrap(), "{}", f["name"]);
                    assert_eq!(g.tested, w["tested"], "{}", f["name"]);
                }
                zones += got.len();
            }
        }
        assert!(zones > 40, "{zones}");
    }

    /// Module docs, "When": the cost under the core lock of following a watched symbol through the
    /// whole fixture day (every print, 04:00-20:00), with levels on four charts and zones on two
    /// keys, against the engine's own apply. Measured 2026-10-31: 1.6 us a print in a release build
    /// (the engine's apply 1.4 us), 3x the engine in debug. Fails only past 5x the engine in any
    /// build, so a skip rule that stops skipping is caught.
    #[test]
    fn cost_of_a_watched_symbol_over_a_day() {
        use market_archive::fixture::{DEFAULT_SEED, fixture_symbol, symbol_params};
        let (seed, scale) = symbol_params(DEFAULT_SEED, 0, "SMPL");
        let events = fixture_symbol(day(), "SMPL", seed, scale).unwrap();
        let mut e = ChartEngine::default();
        let mut ctx = ContextRegistry::default();
        for tf in [Timeframe::S15, Timeframe::S30, Timeframe::M1, Timeframe::M5] {
            ctx.acquire(&ContextKey::Levels(lkey(tf)));
        }
        ctx.acquire(&ContextKey::Zones(zkey(Timeframe::M1, false)));
        ctx.acquire(&ContextKey::Zones(zkey(Timeframe::S15, true)));
        let (mut in_engine, mut in_ctx) = (Duration::ZERO, Duration::ZERO);
        let (mut frames, mut prints) = (0, 0);
        let start = Instant::now();
        for ev in &events {
            let t = Instant::now();
            let ups = e.apply(ev);
            in_engine += t.elapsed();
            let t = Instant::now();
            // Wall time stands in for the throttle's clock: the run is far faster than real time,
            // so the forming bar is mostly left to finalizations, as at a high replay speed.
            frames += ctx
                .on_updates(&e, &no_close, &ups, start + t.duration_since(start))
                .len();
            in_ctx += t.elapsed();
            prints += 1;
        }
        eprintln!(
            "chart_context cost: {prints} prints, engine {in_engine:?}, levels+zones {in_ctx:?} ({:.2} us/print), {frames} frames",
            in_ctx.as_secs_f64() * 1e6 / prints as f64
        );
        assert!(frames > 0);
        assert!(
            in_ctx < in_engine * 5,
            "levels+zones {in_ctx:?} vs engine {in_engine:?}"
        );
    }
}
