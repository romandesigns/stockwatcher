use crate::config::EngineConfig;
use crate::luld::{LuldPoint, LuldState};
use crate::profile::{TickProfile, poc, value_area};
use crate::quality::DataQuality;
use crate::timeframe::Timeframe;
use crate::wire::{
    CandleRemove, CandleUpdate, CandleWire, ChartSnapshot, ChartUpdate, GapReason, GapUpdate,
    GapWire, HaltUpdate, HaltWire, LuldUpdate, LuldWire, ProfileRow, ProfileWire, SessionInfo,
    TapeTrade, ValueArea,
};
use market_events::{
    Cancel, Correction, EffectReason, MarketEvent, Price, SessionId, SessionKind, Source, Status,
    StatusKind, Trade, TradeId, UnixNanos, classify, trade_effect,
};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, VecDeque};

/// Percent of session volume inside the value area.
pub const VALUE_AREA_PERCENT: u32 = 70;

/// What an applied correction or cancel did to the engine's own records, for a consumer that keeps
/// running totals of the same prints (the server's quote board) and must stay equal to the candles
/// without re-deriving the rules: which print left, which one (if any) took its place, what each
/// counted for, and the newest price-qualifying print of that trading date afterwards.
///
/// Only for a revision the engine **applied** (its target was in retention). An unapplied one
/// changes no candle, so there is nothing to follow and [`ChartEngine::apply_revising`] returns
/// `None`, exactly as the candles ignore it.
#[derive(Clone, Debug, PartialEq)]
pub struct Revision {
    pub symbol: String,
    /// The session of the original print (a correction keeps it: same execution time).
    pub session: SessionId,
    /// The print as the engine had counted it.
    pub removed: RevisedPrint,
    /// The corrected print, run through the condition policy again; `None` for a cancel.
    pub added: Option<RevisedPrint>,
    /// The newest OHLC-qualifying print of `session.date` after the revision (the close of the
    /// symbol's latest candle that day) with its execution time. From the retained trades; when
    /// none of that date is retained, the latest base-timeframe candle of that date, its close
    /// stamped with the candle's start (only the interval is known then). `None` when no candle
    /// of that date is left.
    pub last: Option<(Price, UnixNanos)>,
}

/// One side of a [`Revision`]: price, size and what the candles count it for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisedPrint {
    pub price: Price,
    pub size: u64,
    pub ohlc: bool,
    pub volume: bool,
}

impl From<&TradeRec> for RevisedPrint {
    fn from(r: &TradeRec) -> Self {
        RevisedPrint {
            price: r.price,
            size: r.size,
            ohlc: r.ohlc,
            volume: r.volume,
        }
    }
}

/// Total order of trades within a symbol: execution time, then provider sequence (so
/// same-timestamp trades follow `source_seq`), then exchange and source as final tie-breakers.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct OrderKey {
    ts: i64,
    seq: u64,
    exchange: String,
    source: Source,
}

impl OrderKey {
    /// The smallest possible key at `ts` (for range bounds). `Source::AlpacaSip` is the first
    /// variant, so this sorts before every real key at `ts`.
    fn floor(ts: i64) -> Self {
        OrderKey {
            ts,
            seq: 0,
            exchange: String::new(),
            source: Source::AlpacaSip,
        }
    }

    fn of(ts: i64, id: &TradeId) -> Self {
        OrderKey {
            ts,
            seq: id.seq,
            exchange: id.exchange.clone(),
            source: id.source,
        }
    }
}

/// A retained trade, after the condition policy.
#[derive(Clone, Debug)]
struct TradeRec {
    id: TradeId,
    price: Price,
    size: u64,
    ohlc: bool,
    volume: bool,
    profile: bool,
    session: SessionId,
}

impl TradeRec {
    fn affects_candles(&self) -> bool {
        self.ohlc || self.volume
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ohlc {
    o: Price,
    h: Price,
    l: Price,
    c: Price,
}

#[derive(Clone, Debug)]
struct Bucket {
    start: i64,
    end: i64,
    session: SessionId,
    ohlc: Option<Ohlc>,
    volume: u64,
    n: u64,
    /// Key of the last trade folded in; a new trade with a greater key folds incrementally,
    /// anything else triggers an exact recompute from the retained trades.
    last_key: Option<OrderKey>,
    final_emitted: bool,
}

impl Bucket {
    fn new(start: i64, end: i64, session: SessionId) -> Self {
        Bucket {
            start,
            end,
            session,
            ohlc: None,
            volume: 0,
            n: 0,
            last_key: None,
            final_emitted: false,
        }
    }

    fn fold(&mut self, key: &OrderKey, rec: &TradeRec) {
        if rec.ohlc {
            let p = rec.price;
            self.ohlc = Some(match self.ohlc {
                None => Ohlc {
                    o: p,
                    h: p,
                    l: p,
                    c: p,
                },
                Some(x) => Ohlc {
                    o: x.o,
                    h: x.h.max(p),
                    l: x.l.min(p),
                    c: p,
                },
            });
        }
        if rec.volume {
            self.volume += rec.size;
            self.n += 1;
        }
        self.last_key = Some(key.clone());
    }

    fn recompute(&mut self, trades: &BTreeMap<OrderKey, TradeRec>) {
        self.ohlc = None;
        self.volume = 0;
        self.n = 0;
        self.last_key = None;
        for (k, r) in trades.range(OrderKey::floor(self.start)..OrderKey::floor(self.end)) {
            if r.affects_candles() {
                self.fold(k, r);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.ohlc.is_none() && self.volume == 0
    }

    fn candle(&self, wm: Option<i64>) -> Option<CandleWire> {
        let x = self.ohlc?;
        Some(CandleWire {
            t: ms(self.start),
            o: x.o.to_f64(),
            h: x.h.to_f64(),
            l: x.l.to_f64(),
            c: x.c.to_f64(),
            v: self.volume,
            n: self.n,
            is_final: is_final(self.end, wm),
        })
    }

    fn volume_gap(&self, tf: Timeframe) -> GapWire {
        GapWire {
            t: ms(self.start),
            end: ms(self.end),
            reason: GapReason::VolumeOnlyTrades,
            v: self.volume,
            intervals: ((self.end - self.start) / tf.nanos()) as u64,
        }
    }
}

fn ms(ns: i64) -> i64 {
    UnixNanos(ns).millis()
}

fn is_final(end: i64, wm: Option<i64>) -> bool {
    wm.is_some_and(|w| w >= end)
}

#[derive(Clone, Debug)]
struct Series {
    tf: Timeframe,
    buckets: VecDeque<Bucket>,
}

/// Per-call context shared by the series operations.
struct Ctx<'a> {
    symbol: &'a str,
    wm: Option<i64>,
    cap: usize,
    bin: Price,
    rows: &'a [ProfileRow],
}

impl Ctx<'_> {
    fn emit_bucket(&self, tf: Timeframe, b: &Bucket, out: &mut Vec<ChartUpdate>) {
        let session = b.session.to_string();
        match b.candle(self.wm) {
            Some(candle) => out.push(ChartUpdate::Candle(CandleUpdate {
                symbol: self.symbol.to_string(),
                timeframe: tf,
                session,
                candle,
                profile_bin: self.bin.to_f64(),
                profile: self.rows.to_vec(),
            })),
            None => out.push(ChartUpdate::Gap(GapUpdate {
                symbol: self.symbol.to_string(),
                timeframe: tf,
                session,
                gap: b.volume_gap(tf),
                profile_bin: self.bin.to_f64(),
                profile: self.rows.to_vec(),
            })),
        }
    }

    fn emit_remove(
        &self,
        tf: Timeframe,
        start: i64,
        session: SessionId,
        out: &mut Vec<ChartUpdate>,
    ) {
        out.push(ChartUpdate::Remove(CandleRemove {
            symbol: self.symbol.to_string(),
            timeframe: tf,
            session: session.to_string(),
            t: ms(start),
            profile_bin: self.bin.to_f64(),
            profile: self.rows.to_vec(),
        }));
    }
}

impl Series {
    /// Marks every ended interval final and emits the candle once with `final: true`.
    fn sweep(&mut self, ctx: &Ctx<'_>, out: &mut Vec<ChartUpdate>) {
        let Some(wm) = ctx.wm else { return };
        let mut idx = Vec::new();
        for (i, b) in self.buckets.iter().enumerate().rev() {
            if b.end > wm {
                continue;
            }
            if b.final_emitted {
                break;
            }
            idx.push(i);
        }
        // Finalization carries no profile change.
        let quiet = Ctx { rows: &[], ..*ctx };
        for i in idx.into_iter().rev() {
            self.buckets[i].final_emitted = true;
            if self.buckets[i].ohlc.is_some() {
                quiet.emit_bucket(self.tf, &self.buckets[i], out);
            }
        }
    }

    /// Inserts a bucket keeping order and the ring cap. Returns its index, or `None` if it is
    /// older than everything retained in a full ring.
    fn insert(&mut self, pos: usize, b: Bucket, cap: usize) -> Option<usize> {
        if self.buckets.len() >= cap && pos == 0 {
            return None;
        }
        self.buckets.insert(pos, b);
        if self.buckets.len() > cap {
            self.buckets.pop_front();
            return Some(pos - 1);
        }
        Some(pos)
    }

    /// Applies one new trade to its interval.
    fn apply(
        &mut self,
        key: &OrderKey,
        rec: &TradeRec,
        trades: &BTreeMap<OrderKey, TradeRec>,
        ctx: &Ctx<'_>,
        out: &mut Vec<ChartUpdate>,
    ) {
        let tf = self.tf;
        let start = tf.start_of(UnixNanos(key.ts)).0;
        let i = match self.buckets.binary_search_by_key(&start, |b| b.start) {
            Ok(i) => i,
            Err(pos) => {
                if pos == self.buckets.len()
                    && let Some(prev) = self.buckets.back()
                    && prev.session == rec.session
                    && prev.end < start
                {
                    out.push(ChartUpdate::Gap(GapUpdate {
                        symbol: ctx.symbol.to_string(),
                        timeframe: tf,
                        session: rec.session.to_string(),
                        gap: GapWire {
                            t: ms(prev.end),
                            end: ms(start),
                            reason: GapReason::NoTrades,
                            v: 0,
                            intervals: ((start - prev.end) / tf.nanos()) as u64,
                        },
                        profile_bin: ctx.bin.to_f64(),
                        profile: Vec::new(),
                    }));
                }
                match self.insert(
                    pos,
                    Bucket::new(start, start + tf.nanos(), rec.session),
                    ctx.cap,
                ) {
                    Some(i) => i,
                    None => return,
                }
            }
        };
        let b = &mut self.buckets[i];
        if b.last_key.as_ref().is_none_or(|lk| key > lk) {
            b.fold(key, rec);
        } else {
            b.recompute(trades);
        }
        b.final_emitted = is_final(b.end, ctx.wm);
        ctx.emit_bucket(tf, &self.buckets[i], out);
    }

    /// Recomputes the interval containing `ts` from retained trades (after a cancel/correction)
    /// and emits what changed.
    fn refresh(
        &mut self,
        ts: i64,
        session: SessionId,
        trades: &BTreeMap<OrderKey, TradeRec>,
        ctx: &Ctx<'_>,
        out: &mut Vec<ChartUpdate>,
    ) {
        let tf = self.tf;
        let start = tf.start_of(UnixNanos(ts)).0;
        match self.buckets.binary_search_by_key(&start, |b| b.start) {
            Ok(i) => {
                let had_candle = self.buckets[i].ohlc.is_some();
                let b = &mut self.buckets[i];
                b.recompute(trades);
                b.final_emitted = is_final(b.end, ctx.wm);
                if b.is_empty() {
                    let b = self.buckets.remove(i).expect("index from search");
                    ctx.emit_remove(tf, b.start, b.session, out);
                } else {
                    if had_candle && self.buckets[i].ohlc.is_none() {
                        ctx.emit_remove(tf, start, session, out);
                    }
                    ctx.emit_bucket(tf, &self.buckets[i], out);
                }
            }
            Err(pos) => {
                let mut b = Bucket::new(start, start + tf.nanos(), session);
                b.recompute(trades);
                if b.is_empty() {
                    return;
                }
                b.final_emitted = is_final(b.end, ctx.wm);
                if let Some(i) = self.insert(pos, b, ctx.cap) {
                    ctx.emit_bucket(tf, &self.buckets[i], out);
                }
            }
        }
    }
}

/// A trading halt, from a `Halt` status to its `Resume`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HaltRec {
    start: i64,
    end: Option<i64>,
    reason: String,
    code: String,
}

impl HaltRec {
    fn wire(&self) -> HaltWire {
        HaltWire {
            start: ms(self.start),
            end: self.end.map(ms),
            reason: self.reason.clone(),
            code: self.code.clone(),
        }
    }
}

fn luld_wire(p: &LuldPoint) -> LuldWire {
    LuldWire {
        t: ms(p.t),
        reference: p.reference.map(Price::to_f64),
        up: p.up.map(Price::to_f64),
        down: p.down.map(Price::to_f64),
    }
}

fn luld_updates(symbol: &str, points: Vec<LuldPoint>, out: &mut Vec<ChartUpdate>) {
    out.extend(points.iter().map(|p| {
        ChartUpdate::Luld(LuldUpdate {
            symbol: symbol.to_string(),
            point: luld_wire(p),
        })
    }));
}

#[derive(Clone, Debug)]
struct SymbolState {
    symbol: String,
    /// Latest execution time among accepted trades.
    own_wm: Option<i64>,
    max_key: Option<OrderKey>,
    current_session: Option<SessionId>,
    trades: BTreeMap<OrderKey, TradeRec>,
    index: HashMap<TradeId, OrderKey>,
    series: Vec<Series>,
    profiles: BTreeMap<SessionId, TickProfile>,
    quality: BTreeMap<Option<SessionId>, DataQuality>,
    /// The finalization deadline currently queued in the engine's heap.
    scheduled: Option<i64>,
    /// Retained halts, ascending by start; at most the last one is open.
    halts: Vec<HaltRec>,
    /// LULD bands of the latest regular session the symbol traded in.
    luld: Option<LuldState>,
}

impl SymbolState {
    fn q(&mut self, session: Option<SessionId>) -> &mut DataQuality {
        self.quality.entry(session).or_default()
    }

    /// The newest OHLC-qualifying print of `session`'s trading date ([`Revision::last`]). Trades are
    /// keyed by execution time, so the walk from the newest stops at the first older date; it
    /// runs only for a correction or cancel, never per print.
    fn last_print_of(&self, session: SessionId) -> Option<(Price, UnixNanos)> {
        let date = session.date;
        for (k, r) in self.trades.iter().rev() {
            if r.session.date < date {
                break;
            }
            if r.session.date == date && r.ohlc {
                return Some((r.price, UnixNanos(k.ts)));
            }
        }
        // None retained: the base timeframe's latest candle of that date still holds its close.
        let base = self.series.iter().min_by_key(|s| s.tf.nanos())?;
        base.buckets
            .iter()
            .rev()
            .filter(|b| b.session.date == date)
            .find_map(|b| b.ohlc.map(|x| (x.c, UnixNanos(b.start))))
    }

    fn next_deadline(&self) -> Option<i64> {
        let candles = self
            .series
            .iter()
            .filter_map(|s| s.buckets.back().filter(|b| !b.final_emitted).map(|b| b.end))
            .min();
        let luld = self.luld.as_ref().and_then(LuldState::next_deadline);
        candles.into_iter().chain(luld).min()
    }

    fn halted(&self) -> bool {
        self.halts.last().is_some_and(|h| h.end.is_none())
    }

    /// LULD for one accepted trade: a price-qualifying regular-session print while not halted
    /// feeds the reference; any other print only moves LULD time forward (so a print after the
    /// close ends the bands even if the clock has not got there yet).
    fn luld_trade(&mut self, ts: i64, rec: &TradeRec) -> Vec<LuldPoint> {
        let mut pts = Vec::new();
        let regular = rec.session.kind == SessionKind::Regular;
        if regular && rec.ohlc && !self.halted() {
            if self.luld.as_ref().is_none_or(|l| l.session < rec.session)
                && let Some((open, close)) = rec.session.bounds()
            {
                self.luld = Some(LuldState::new(rec.session, open.0, close.0));
            }
            if let Some(l) = self.luld.as_mut().filter(|l| l.session == rec.session) {
                l.on_trade(ts, rec.price, &mut pts);
                return pts;
            }
        }
        if let Some(l) = self.luld.as_mut() {
            l.advance(ts, true, &mut pts);
        }
        pts
    }

    fn profile_rows(&self, session: SessionId, bin: Price, prices: &[Price]) -> Vec<ProfileRow> {
        let mut bins: Vec<Price> = prices.iter().map(|p| p.floor_to(bin)).collect();
        bins.sort();
        bins.dedup();
        let prof = self.profiles.get(&session);
        bins.into_iter()
            .map(|b| ProfileRow {
                p: b.to_f64(),
                v: prof.map_or(0, |pr| pr.bin_total(bin, b)),
            })
            .collect()
    }
}

/// The chart engine: incremental per-symbol candles for every configured timeframe, gap markers,
/// session volume profiles and data-quality counters. Single-threaded and event-driven: no task,
/// thread or timer per symbol. See the crate docs for every policy.
#[derive(Clone, Debug)]
pub struct ChartEngine {
    cfg: EngineConfig,
    base_tf: Timeframe,
    max_tf: Timeframe,
    retention: i64,
    clock: Option<i64>,
    symbols: HashMap<String, SymbolState>,
    /// Min-heap of (interval end, symbol) so `advance_clock` touches only symbols with an
    /// interval to finalize, never every symbol.
    pending: BinaryHeap<Reverse<(i64, String)>>,
}

impl Default for ChartEngine {
    fn default() -> Self {
        ChartEngine::new(EngineConfig::default()).expect("default config is valid")
    }
}

impl ChartEngine {
    pub fn new(mut cfg: EngineConfig) -> Result<Self, String> {
        cfg.validate()?;
        cfg.timeframes.sort();
        let base_tf = cfg.timeframes[0];
        let max_tf = *cfg.timeframes.last().expect("validated non-empty");
        let retention = cfg.retention_ns.max(max_tf.nanos() + cfg.settle_window_ns);
        Ok(ChartEngine {
            cfg,
            base_tf,
            max_tf,
            retention,
            clock: None,
            symbols: HashMap::new(),
            pending: BinaryHeap::new(),
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.cfg
    }

    /// The latest value passed to [`Self::advance_clock`].
    pub fn clock(&self) -> Option<UnixNanos> {
        self.clock.map(UnixNanos)
    }

    /// The symbol's watermark: the later of its newest accepted trade and the clock.
    pub fn watermark(&self, symbol: &str) -> Option<UnixNanos> {
        let st = self.symbols.get(symbol)?;
        st.own_wm.max(self.clock).map(UnixNanos)
    }

    pub fn symbols(&self) -> impl Iterator<Item = &str> {
        self.symbols.keys().map(String::as_str)
    }

    /// Applies one normalized event and returns the increments it caused, in order: the tape row,
    /// then per timeframe (ascending) any finalized candles, a gap marker if a new interval opened
    /// after a hole, the touched candle, and any LULD band change. A symbol's `Halt`/`Resume`
    /// status produces `chart.halt` (and a LULD null point at a halt); quotes and every other
    /// status produce nothing.
    pub fn apply(&mut self, event: &MarketEvent) -> Vec<ChartUpdate> {
        self.apply_revising(event).0
    }

    /// [`ChartEngine::apply`], plus the [`Revision`] when the event is a correction or cancel the
    /// engine applied. The updates are identical; the revision is the engine's own account of
    /// which print it removed or replaced, so a running tally can follow it exactly (a correction
    /// or cancel carries the original's price and size, but what the engine had counted the print
    /// for, and whether it still held it, is only known here).
    pub fn apply_revising(&mut self, event: &MarketEvent) -> (Vec<ChartUpdate>, Option<Revision>) {
        match event {
            MarketEvent::Trade(t) => (self.apply_trade(t), None),
            MarketEvent::Correction(c) => self.apply_correction(c),
            MarketEvent::Cancel(c) => self.apply_cancel(c),
            MarketEvent::Status(s) => (self.apply_status(s), None),
            MarketEvent::Quote(_) => (Vec::new(), None),
        }
    }

    /// Halts (docs/architecture/halts-luld-fills.md section 1). A `Halt` opens a halt at its
    /// `event_ts` and a `Resume` closes the open one; a second `Halt` while halted and a `Resume`
    /// with nothing open are ignored and counted (`haltsUnmatched`). Market-wide statuses and
    /// every other kind change nothing.
    fn apply_status(&mut self, s: &Status) -> Vec<ChartUpdate> {
        let Some(symbol) = s.symbol.as_deref() else {
            return Vec::new();
        };
        if !matches!(s.kind, StatusKind::Halt | StatusKind::Resume) {
            return Vec::new();
        }
        let st = Self::state(&mut self.symbols, &self.cfg, symbol);
        let ts = s.event_ts.0;
        let mut out = Vec::new();
        match (s.kind, st.halted()) {
            (StatusKind::Halt, false) => {
                let halt = HaltRec {
                    start: ts,
                    end: None,
                    reason: s.reason.clone(),
                    code: s.code.clone(),
                };
                out.push(ChartUpdate::Halt(HaltUpdate {
                    symbol: symbol.to_string(),
                    halt: halt.wire(),
                }));
                st.halts.push(halt);
                let mut pts = Vec::new();
                if let Some(l) = st.luld.as_mut() {
                    l.on_halt(ts, &mut pts);
                }
                luld_updates(symbol, pts, &mut out);
            }
            (StatusKind::Resume, true) => {
                let h = st.halts.last_mut().expect("halted means an open halt");
                h.end = Some(ts.max(h.start));
                out.push(ChartUpdate::Halt(HaltUpdate {
                    symbol: symbol.to_string(),
                    halt: h.wire(),
                }));
            }
            _ => st.q(classify(s.event_ts)).halts_unmatched += 1,
        }
        Self::reschedule(&mut self.pending, st);
        out
    }

    /// Moves `symbol`'s whole state out of `other` into this engine (replacing any it had), then
    /// finalizes whatever this engine's clock has already passed. `other` must have this
    /// engine's configuration and should have been fed only that symbol's events, with the same
    /// marks the live ingest emitted before each (`floor_1s` of its delivery time): the engine
    /// keeps no state across symbols but the clock, and a symbol's late/out-of-order decisions
    /// depend only on the clock at its own events, so the adopted state is the one this engine
    /// would have built from the same events in the market-wide order (tested). This is how the
    /// live market loads one symbol's day from the archive while the rest of the market streams.
    /// Returns the finalization's updates; callers that re-snapshot the symbol can drop them.
    pub fn adopt_symbol(&mut self, other: &mut ChartEngine, symbol: &str) -> Vec<ChartUpdate> {
        let Some(mut st) = other.symbols.remove(symbol) else {
            return Vec::new();
        };
        st.scheduled = None;
        Self::reschedule(&mut self.pending, &mut st);
        self.symbols.insert(symbol.to_string(), st);
        match self.clock {
            Some(c) => self.advance_clock(UnixNanos(c)),
            None => {
                self.clock = other.clock;
                Vec::new()
            }
        }
    }

    /// Drops a symbol's state (the live market reloading it from the archive).
    pub fn remove_symbol(&mut self, symbol: &str) -> bool {
        self.symbols.remove(symbol).is_some()
    }

    /// Moves the clock forward (never back) and finalizes every interval that has ended, so a
    /// symbol that stops trading still gets its last candle marked final. Cost is proportional to
    /// the number of symbols with an interval ending before `now`.
    pub fn advance_clock(&mut self, now: UnixNanos) -> Vec<ChartUpdate> {
        let now = now.0;
        self.clock = Some(self.clock.map_or(now, |c| c.max(now)));
        let mut out = Vec::new();
        while let Some(Reverse((end, _))) = self.pending.peek()
            && *end <= now
        {
            let Some(Reverse((end, sym))) = self.pending.pop() else {
                break;
            };
            let Some(st) = self.symbols.get_mut(&sym) else {
                continue;
            };
            if st.scheduled == Some(end) {
                st.scheduled = None;
            }
            let wm = st.own_wm.max(self.clock);
            let ctx = Ctx {
                symbol: &st.symbol,
                wm,
                cap: self.cfg.max_intervals_per_timeframe,
                bin: self.cfg.profile_bin,
                rows: &[],
            };
            for s in &mut st.series {
                s.sweep(&ctx, &mut out);
            }
            let mut pts = Vec::new();
            if let Some(l) = st.luld.as_mut() {
                l.advance(now, true, &mut pts);
            }
            luld_updates(&st.symbol, pts, &mut out);
            Self::reschedule(&mut self.pending, st);
            Self::prune(
                st,
                wm,
                self.retention,
                self.max_tf,
                self.cfg.sessions_retained,
            );
        }
        out
    }

    fn reschedule(pending: &mut BinaryHeap<Reverse<(i64, String)>>, st: &mut SymbolState) {
        if let Some(next) = st.next_deadline()
            && st.scheduled != Some(next)
        {
            st.scheduled = Some(next);
            pending.push(Reverse((next, st.symbol.clone())));
        }
    }

    fn state<'a>(
        symbols: &'a mut HashMap<String, SymbolState>,
        cfg: &EngineConfig,
        symbol: &str,
    ) -> &'a mut SymbolState {
        symbols
            .entry(symbol.to_string())
            .or_insert_with(|| SymbolState {
                symbol: symbol.to_string(),
                own_wm: None,
                max_key: None,
                current_session: None,
                trades: BTreeMap::new(),
                index: HashMap::new(),
                series: cfg
                    .timeframes
                    .iter()
                    .map(|&tf| Series {
                        tf,
                        buckets: VecDeque::new(),
                    })
                    .collect(),
                profiles: BTreeMap::new(),
                quality: BTreeMap::new(),
                scheduled: None,
                halts: Vec::new(),
                luld: None,
            })
    }

    fn apply_trade(&mut self, t: &Trade) -> Vec<ChartUpdate> {
        let effect = t.effect();
        let (clock, base_tf, settle) = (self.clock, self.base_tf, self.cfg.settle_window_ns);
        let (cap, bin) = (self.cfg.max_intervals_per_timeframe, self.cfg.profile_bin);
        let (retention, max_tf, keep) = (self.retention, self.max_tf, self.cfg.sessions_retained);
        let st = Self::state(&mut self.symbols, &self.cfg, &t.symbol);
        let id = t.id();
        let mut out = Vec::new();
        let reason = effect.reason_code();
        let tape = |counted: String| {
            ChartUpdate::Tape(TapeTrade {
                symbol: t.symbol.clone(),
                t: t.event_ts.millis(),
                ts_ns: t.event_ts.0,
                p: t.price.to_f64(),
                s: t.size,
                exchange: t.exchange.clone(),
                seq: t.source_seq,
                conditions: t.conditions.clone(),
                counted,
                reason: reason.clone(),
                session: t.session.map(|s| s.to_string()),
            })
        };

        if st.index.contains_key(&id) {
            st.q(t.session).duplicates += 1;
            out.push(tape("excluded:duplicate".into()));
            return out;
        }
        let Some(session) = t.session else {
            st.q(None).outside_session += 1;
            out.push(tape("excluded:outside_session".into()));
            return out;
        };
        let ts = t.event_ts.0;
        let base_end = base_tf.end_of(t.event_ts).0;
        if let Some(w) = st.own_wm.max(clock) {
            if w >= base_end + settle {
                st.q(Some(session)).late_excluded += 1;
                out.push(tape("excluded:late".into()));
                return out;
            }
            if w >= base_end {
                st.q(Some(session)).late_applied += 1;
            }
        }
        let key = OrderKey::of(ts, &id);
        if st.max_key.as_ref().is_some_and(|m| key < *m) {
            st.q(Some(session)).out_of_order_reordered += 1;
        }
        if !effect.unrecognized.is_empty() {
            st.q(Some(session)).unrecognized_conditions += 1;
        }
        let rec = TradeRec {
            id: id.clone(),
            price: t.price,
            size: t.size,
            ohlc: effect.updates_ohlc,
            volume: effect.updates_volume,
            profile: effect.updates_ohlc
                || (effect.updates_volume && effect.reason == EffectReason::OddLot),
            session,
        };
        let counted = if rec.ohlc {
            "ohlc_and_volume".to_string()
        } else if rec.volume {
            st.q(Some(session)).volume_only += 1;
            "volume_only".to_string()
        } else {
            st.q(Some(session)).condition_excluded += 1;
            format!(
                "excluded:{}",
                reason.clone().unwrap_or_else(|| "condition".into())
            )
        };
        out.push(tape(counted));

        st.trades.insert(key.clone(), rec.clone());
        st.index.insert(id, key.clone());
        st.own_wm = Some(st.own_wm.map_or(ts, |w| w.max(ts)));
        if st.max_key.as_ref().is_none_or(|m| key > *m) {
            st.max_key = Some(key.clone());
        }
        if st.current_session.is_none_or(|s| session > s) {
            st.current_session = Some(session);
        }
        let mut rows = Vec::new();
        if rec.profile && rec.size > 0 {
            st.profiles
                .entry(session)
                .or_default()
                .add(rec.price, rec.size);
            rows = st.profile_rows(session, bin, &[rec.price]);
        }
        let wm = st.own_wm.max(clock);
        let ctx = Ctx {
            symbol: &t.symbol,
            wm,
            cap,
            bin,
            rows: &rows,
        };
        for s in &mut st.series {
            s.sweep(&ctx, &mut out);
            if rec.affects_candles() {
                s.apply(&key, &rec, &st.trades, &ctx, &mut out);
            }
        }
        let pts = st.luld_trade(ts, &rec);
        luld_updates(&t.symbol, pts, &mut out);
        Self::reschedule(&mut self.pending, st);
        Self::prune(st, wm, retention, max_tf, keep);
        out
    }

    fn apply_cancel(&mut self, c: &Cancel) -> (Vec<ChartUpdate>, Option<Revision>) {
        let (clock, cap, bin) = (
            self.clock,
            self.cfg.max_intervals_per_timeframe,
            self.cfg.profile_bin,
        );
        let st = Self::state(&mut self.symbols, &self.cfg, &c.symbol);
        let mut out = Vec::new();
        let Some(key) = st.index.remove(&c.target_trade_id()) else {
            st.q(classify(c.event_ts)).cancels_unapplied += 1;
            return (out, None);
        };
        let rec = st.trades.remove(&key).expect("index and trades agree");
        st.q(Some(rec.session)).cancels_applied += 1;
        let mut rows = Vec::new();
        if rec.profile {
            if let Some(p) = st.profiles.get_mut(&rec.session) {
                p.sub(rec.price, rec.size);
            }
            rows = st.profile_rows(rec.session, bin, &[rec.price]);
        }
        if rec.affects_candles() {
            let wm = st.own_wm.max(clock);
            let ctx = Ctx {
                symbol: &c.symbol,
                wm,
                cap,
                bin,
                rows: &rows,
            };
            for s in &mut st.series {
                s.refresh(key.ts, rec.session, &st.trades, &ctx, &mut out);
            }
        }
        Self::reschedule(&mut self.pending, st);
        let rev = Revision {
            symbol: c.symbol.clone(),
            session: rec.session,
            removed: RevisedPrint::from(&rec),
            added: None,
            last: st.last_print_of(rec.session),
        };
        (out, Some(rev))
    }

    fn apply_correction(&mut self, c: &Correction) -> (Vec<ChartUpdate>, Option<Revision>) {
        let (clock, cap, bin) = (
            self.clock,
            self.cfg.max_intervals_per_timeframe,
            self.cfg.profile_bin,
        );
        let st = Self::state(&mut self.symbols, &self.cfg, &c.symbol);
        let mut out = Vec::new();
        let Some(key) = st.index.remove(&c.original_trade_id()) else {
            st.q(classify(c.event_ts)).corrections_unapplied += 1;
            return (out, None);
        };
        let old = st.trades.remove(&key).expect("index and trades agree");
        let effect = trade_effect(&c.corrected_conditions, c.tape);
        let new_id = TradeId {
            source: c.source,
            exchange: c.exchange.clone(),
            seq: c.corrected_id,
        };
        let new_key = OrderKey::of(key.ts, &new_id);
        let new = TradeRec {
            id: new_id.clone(),
            price: c.corrected_price,
            size: c.corrected_size,
            ohlc: effect.updates_ohlc,
            volume: effect.updates_volume,
            profile: effect.updates_ohlc
                || (effect.updates_volume && effect.reason == EffectReason::OddLot),
            session: old.session,
        };
        st.q(Some(old.session)).corrections_applied += 1;
        let prof = st.profiles.entry(old.session).or_default();
        if old.profile {
            prof.sub(old.price, old.size);
        }
        if new.profile {
            prof.add(new.price, new.size);
        }
        let touched: Vec<Price> = [(old.profile, old.price), (new.profile, new.price)]
            .into_iter()
            .filter_map(|(f, p)| f.then_some(p))
            .collect();
        let rows = st.profile_rows(old.session, bin, &touched);
        let affects = old.affects_candles() || new.affects_candles();
        let (removed, added) = (RevisedPrint::from(&old), RevisedPrint::from(&new));
        st.trades.insert(new_key.clone(), new);
        st.index.insert(new_id, new_key);
        if affects {
            let wm = st.own_wm.max(clock);
            let ctx = Ctx {
                symbol: &c.symbol,
                wm,
                cap,
                bin,
                rows: &rows,
            };
            for s in &mut st.series {
                s.refresh(key.ts, old.session, &st.trades, &ctx, &mut out);
            }
        }
        Self::reschedule(&mut self.pending, st);
        let rev = Revision {
            symbol: c.symbol.clone(),
            session: old.session,
            removed,
            added: Some(added),
            last: st.last_print_of(old.session),
        };
        (out, Some(rev))
    }

    /// Drops trade records older than the retention window (aligned down to the largest
    /// timeframe, so every retained interval is retained whole and recomputes stay exact), and
    /// sessions beyond `keep`.
    fn prune(
        st: &mut SymbolState,
        wm: Option<i64>,
        retention: i64,
        max_tf: Timeframe,
        keep: usize,
    ) {
        let Some(wm) = wm else { return };
        let cutoff = max_tf.start_of(UnixNanos(wm - retention)).0;
        while let Some((k, _)) = st.trades.first_key_value()
            && k.ts < cutoff
        {
            let (_, rec) = st.trades.pop_first().expect("non-empty");
            st.index.remove(&rec.id);
        }
        while st.profiles.len() > keep {
            st.profiles.pop_first();
        }
        // Halts follow the candles: one that ended before every retained interval goes.
        if let Some(oldest) = st
            .series
            .iter()
            .filter_map(|s| s.buckets.front().map(|b| b.start))
            .min()
        {
            let gone = st
                .halts
                .iter()
                .take_while(|h| h.end.is_some_and(|e| e < oldest))
                .count();
            st.halts.drain(..gone);
        }
        while st.quality.keys().filter(|k| k.is_some()).count() > keep {
            let oldest = *st
                .quality
                .keys()
                .find(|k| k.is_some())
                .expect("counted above");
            st.quality.remove(&oldest);
        }
    }

    /// Full state for one symbol and timeframe: every retained candle, gap markers, the current
    /// session's profile at `bin` (default: the configured bin) and its data-quality counters.
    /// `None` if the symbol is unknown or the timeframe is not configured.
    pub fn snapshot(
        &self,
        symbol: &str,
        tf: Timeframe,
        bin: Option<Price>,
    ) -> Option<ChartSnapshot> {
        let st = self.symbols.get(symbol)?;
        let series = st.series.iter().find(|s| s.tf == tf)?;
        let wm = st.own_wm.max(self.clock);
        let mut candles = Vec::new();
        let mut gaps = Vec::new();
        let mut prev: Option<&Bucket> = None;
        for b in &series.buckets {
            if let Some(p) = prev
                && p.session == b.session
                && p.end < b.start
            {
                gaps.push(GapWire {
                    t: ms(p.end),
                    end: ms(b.start),
                    reason: GapReason::NoTrades,
                    v: 0,
                    intervals: ((b.start - p.end) / tf.nanos()) as u64,
                });
            }
            match b.candle(wm) {
                Some(c) => candles.push(c),
                None => gaps.push(b.volume_gap(tf)),
            }
            prev = Some(b);
        }
        let session = st.current_session;
        Some(ChartSnapshot {
            kind: "chart.snapshot".into(),
            symbol: symbol.to_string(),
            timeframe: tf,
            session: session.and_then(|s| {
                s.bounds().map(|(a, e)| SessionInfo {
                    id: s.to_string(),
                    start: a.millis(),
                    end: e.millis(),
                })
            }),
            candles,
            gaps,
            profile: session.and_then(|s| self.profile(symbol, s, bin)),
            quality: session
                .and_then(|s| st.quality.get(&Some(s)).copied())
                .unwrap_or_default(),
            clock: None,
            halts: st.halts.iter().map(HaltRec::wire).collect(),
            luld: st
                .luld
                .as_ref()
                .filter(|l| session.is_some_and(|s| s.date == l.session.date))
                .map(|l| l.points.iter().map(luld_wire).collect())
                .unwrap_or_default(),
            prev_close: None,
        })
    }

    /// One session's profile at any bin width (exact: computed from per-price volume).
    pub fn profile(
        &self,
        symbol: &str,
        session: SessionId,
        bin: Option<Price>,
    ) -> Option<ProfileWire> {
        let bin = bin
            .filter(|b| b.is_positive())
            .unwrap_or(self.cfg.profile_bin);
        let st = self.symbols.get(symbol)?;
        let prof = st.profiles.get(&session)?;
        let rows = prof.rows(bin);
        Some(ProfileWire {
            bin: bin.to_f64(),
            anchor: session.to_string(),
            rows: rows
                .iter()
                .map(|(p, v)| ProfileRow {
                    p: p.to_f64(),
                    v: *v,
                })
                .collect(),
            total: prof.total(),
            poc: poc(&rows).map(Price::to_f64),
            value_area: value_area(&rows, VALUE_AREA_PERCENT).map(|(l, h, v)| ValueArea {
                low: l.to_f64(),
                high: h.to_f64(),
                v,
            }),
        })
    }

    /// Exact `(bin start, volume)` rows of one session's profile.
    pub fn profile_rows(
        &self,
        symbol: &str,
        session: SessionId,
        bin: Price,
    ) -> Option<Vec<(Price, u64)>> {
        Some(self.symbols.get(symbol)?.profiles.get(&session)?.rows(bin))
    }

    /// Sessions with a retained profile, ascending.
    pub fn sessions(&self, symbol: &str) -> Vec<SessionId> {
        self.symbols
            .get(symbol)
            .map(|s| s.profiles.keys().copied().collect())
            .unwrap_or_default()
    }

    /// Data-quality counters per session (`None` = outside every session), ascending.
    pub fn quality(&self, symbol: &str) -> Vec<(Option<SessionId>, DataQuality)> {
        self.symbols
            .get(symbol)
            .map(|s| s.quality.iter().map(|(k, v)| (*k, *v)).collect())
            .unwrap_or_default()
    }

    /// Number of trade records currently retained for a symbol (for memory-bound tests/metrics).
    pub fn retained_trades(&self, symbol: &str) -> usize {
        self.symbols.get(symbol).map_or(0, |s| s.trades.len())
    }
}
