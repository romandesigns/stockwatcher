//! N1 "early leg", live beside production (owner approval 2026-11-17; judge ruling J8 of
//! `JUDGE-RULINGS-002.md`: a finalist that passed the sealed holdout is released as labelled alerts
//! beside production, which it never replaces). ON by default since 2026-11-17 (5350cad3); at
//! X 8 % since 2026-11-18 as part of Engine A's T2-c4 (`crate::c4_live`, [`logic::GAIN_MIN`]).
//!
//! # Why it exists
//!
//! Engine A round 2's finalist **T2-c6** is "live production minus the armed micro-pullback, plus
//! N1". It caught more runners than live production on Validation (19.9 % vs 14.4 % of the +20 %
//! windows) and on the sealed holdout 2025-04..09 (20.1 % vs 14.6 %), with fewer alerts a day.
//! Every one of its new catches came from N1 (review r2/04). N1's own alerts are NOT good trades:
//! about 11 % reached +10 % before −5 % in research, a net of about −1.9 % a trade after costs.
//! What it adds is an earlier look at more of the stocks that go on to run. The text says what
//! happened and never says "buy".
//!
//! # What it does here
//!
//! Only ADDS notices ([`crate::rocket::NoticeKind::N1`], data kind `shadow.n1`, title
//! "SYM · N1 · shadow · $price"). Nothing production decides changes: no Runners row, no
//! follow-up, no budget, and the armed micro-pullback stays exactly as it is (removing it is a
//! separate owner decision). Every fire goes through the Runners notices' delivery (persisted
//! dedupe, devices, quiet hours, the 3-minute stale drop), so the ledger records it with the
//! server's decision time either way; its push and desktop switch `n1` is ON by default (it was
//! off while N1 was a shadow, until 5350cad3). The Runners board lists today's fires in a
//! "Shadow" section.
//!
//! The detector itself is [`logic`] (module docs there: every step with the research's
//! arithmetic). This module feeds it: every print the Runners board counts, with the chart
//! engine's verdict, 04:00–20:00 ET; the rocket task's clock, which decides each bar 2 s after it
//! ends (plus the task's ~50 ms settle); and the day's facts, read once a day from the archive:
//!
//! - **C1 / DADV20** from `daily-bars/raw/` (the research's `refdata::DailyBars::facts` rule, the
//!   same files), every kept date before the day. The archive keeps `DAILY_BARS_KEEP_DAYS` (63)
//!   dates, so a symbol with fewer than 20 sessions in them takes the mean over those it has
//!   (the research reached back years) and one with none has no C1 and never fires.
//! - **Security type** from `assets.json` names (`universe-v1`, [`logic::sec_type`]).
//!
//! They are read at 03:30 ET (after the nightly 21:15 import), never at the 20:00 roll.
//!
//! # A day not watched whole never alerts
//!
//! The budget (two a stock a day, 20 minutes apart, premarket included) and R20 (has the stock
//! already run +20 % today) need every print since 04:00. After a restart mid-day the earlier
//! prints are not replayed into this detector, so such a day is incomplete: it keeps deciding
//! (for the counts in the day summary) but sends nothing. So is a day whose facts were not loaded
//! in time. A deploy after 20:00 ET is watched whole the next day.
//!
//! # Known differences from the research stream (all measured or bounded in the parity report)
//!
//! - **Universe.** The research tape held only symbols whose close was in $0.05–$30 that day (an
//!   end-of-day membership, finding F3); live sees every symbol. N1's live volume is UNKNOWN
//!   (research: 26.2 a day on Validation, 33.8 on the sealed holdout).
//! - **Union with production.** T2-c6 dropped an N1 row at a symbol-second production already
//!   used; live sends it (it is a different notice kind).
//! - **Twin tickers** (J3) are an archive artefact; live has one ticker per security.
//! - **Arrival.** A print reaching the server after its bar was decided (2 s plus the settle
//!   after the bar's end) is in the research's bar but not in the live decision; one more than a
//!   window late is dropped (counted, `late` in the cost line).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use chrono::NaiveDate;
use market_events::Trade;
use market_events::UnixNanos;
use market_events::session::{SessionKind, sessions_for, trading_date};
use serde_json::{Value, json};

use crate::quotes::Counted;
use crate::rocket::{Notice, NoticeDetail, NoticeKind};

pub mod logic;
pub use logic::{Cand, Clock, Facts, SecType};

/// Days of raw daily bars read at most (the archive keeps 63 by default).
pub const FACTS_LOOKBACK: usize = 260;
/// When the day's facts are read: 30 minutes before 04:00 ET.
pub const FACTS_LEAD_NS: i64 = 30 * logic::MIN;
/// [`N1Live::complete`]'s grace after 04:00 ET.
const START_GRACE_NS: i64 = logic::MIN;
/// Notices held while nobody takes them.
const MAX_NOTICES: usize = 256;
/// Today's fires kept for the board (research: ~26–34 a day).
const MAX_FIRED: usize = 500;

/// The day's reference: C1 / DADV20 by symbol and the security types.
#[derive(Clone, Debug, Default)]
pub struct FactsTable {
    pub date: Option<NaiveDate>,
    pub facts: HashMap<String, Facts>,
    pub types: HashMap<String, SecType>,
    /// Raw daily-bar dates read, and the newest (the previous trading day when the nightly
    /// import ran).
    pub raw_dates: usize,
    pub newest_raw: Option<NaiveDate>,
    pub assets: usize,
}

impl FactsTable {
    pub fn sec_type(&self, sym: &str) -> SecType {
        self.types
            .get(sym)
            .copied()
            .unwrap_or_else(|| logic::sec_type(sym, None))
    }
}

/// Reads `date`'s facts from the archive (module docs). Off the ingest path: the market runs it
/// on a blocking thread.
pub fn load_facts(archive: &market_archive::Archive, date: NaiveDate) -> FactsTable {
    let dates: Vec<NaiveDate> = archive
        .raw_bar_dates()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| *d < date)
        .collect();
    let tail = &dates[dates.len().saturating_sub(FACTS_LOOKBACK)..];
    let mut by_sym: HashMap<String, Vec<logic::DailyIn>> = HashMap::new();
    let mut read = 0;
    for d in tail {
        let Some(b) = archive.read_raw_bars(*d).ok().flatten() else {
            continue;
        };
        read += 1;
        for (s, bar) in &b.bars {
            by_sym.entry(s.clone()).or_default().push(logic::DailyIn {
                h: bar.h,
                l: bar.l,
                c: Some(bar.c),
                v: bar.v.map(|v| v as f64),
                vw: bar.vw,
            });
        }
    }
    let facts = by_sym
        .into_iter()
        .map(|(s, v)| (s, logic::facts_of(&v)))
        .collect();
    let assets = archive.read_assets().ok().flatten().unwrap_or_default();
    let types: HashMap<String, SecType> = assets
        .iter()
        .map(|(s, a)| (s.clone(), logic::sec_type(s, Some(&a.name))))
        .collect();
    FactsTable {
        date: Some(date),
        facts,
        assets: types.len(),
        types,
        raw_dates: read,
        newest_raw: tail.last().copied(),
    }
}

/// One N1 fire's facts (the notice's [`NoticeDetail::N1`]).
#[derive(Clone, Debug, PartialEq)]
pub struct N1Notice {
    /// The 3-minute window's low and high (dollars) and its gain to the close (0.084 = +8.4 %).
    pub low: f64,
    pub high: f64,
    pub gain: f64,
    /// Dollars traded in the window and its volume-counted prints.
    pub dollars: f64,
    pub prints: u32,
    /// The window's dollars per minute over the symbol's usual (DADV20 / 390); `None`: no DADV20.
    pub rel: Option<f64>,
    /// When the server decided it (the rocket task's clock, epoch ns): receipt-time measurement.
    pub emitted_ns: i64,
}

/// A row of the Runners board's Shadow section.
#[derive(Clone, Debug, PartialEq)]
pub struct Fired {
    pub symbol: String,
    pub day: NaiveDate,
    /// The research decision instant (bar end + 2 s), epoch ns.
    pub t: i64,
    pub price: f64,
    pub detail: N1Notice,
    /// Sent as a notice (false on an incomplete day).
    pub notified: bool,
}

#[derive(Clone, Copy, Debug)]
struct Day {
    date: NaiveDate,
    clock: Clock,
}

fn day_of(date: NaiveDate) -> Option<Day> {
    let s = sessions_for(date);
    let t0 = s.iter().find(|(id, _, _)| id.kind == SessionKind::Pre)?.1.0;
    let end = s.iter().map(|(_, _, e)| e.0).max()?;
    Some(Day {
        date,
        clock: Clock {
            t0,
            session_end: end,
        },
    })
}

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    candidates: usize,
    in_universe: usize,
    kept: usize,
    fires: usize,
    notified: usize,
}

/// The detector's live state (module docs). Fed under the core lock with the Runners board: O(1)
/// per print, and per tick only the symbols with a bar due.
#[derive(Default)]
pub struct N1Live {
    day: Option<Day>,
    /// The first instant this detector saw (a print or a tick).
    started_at: Option<i64>,
    now: i64,
    facts: Option<Arc<FactsTable>>,
    /// The date whose facts the market should load, once.
    facts_wanted: Option<NaiveDate>,
    facts_asked: Option<NaiveDate>,
    syms: HashMap<Arc<str>, logic::Sym>,
    /// `(decision time, symbol)` of every bar not decided yet, earliest first.
    due: BinaryHeap<Reverse<(i64, Arc<str>)>>,
    counts: Counts,
    fired: Vec<Fired>,
    notices: Vec<Notice>,
    /// Facts arrived after decisions started (the day is incomplete).
    facts_late: bool,
}

impl N1Live {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the current day was watched from its 04:00 ET with its facts in time.
    pub fn complete(&self) -> bool {
        match (self.day, self.started_at) {
            (Some(d), Some(s)) => s <= d.clock.t0 + START_GRACE_NS && !self.facts_late,
            _ => false,
        }
    }

    fn see(&mut self, t: i64) {
        if self.started_at.is_none() {
            self.started_at = Some(t);
        }
        self.now = self.now.max(t);
    }

    /// Moves to `date` (a later day only). False for a print of an earlier day.
    fn roll(&mut self, date: NaiveDate) -> bool {
        if self.day.is_some_and(|d| d.date == date) {
            return true;
        }
        if self.day.is_some_and(|d| d.date > date) {
            return false;
        }
        let Some(d) = day_of(date) else { return false };
        if let Some(prev) = self.day {
            let c = self.counts;
            let late = self.late();
            tracing::info!(
                day = %prev.date,
                candidates = c.candidates,
                in_universe = c.in_universe,
                kept = c.kept,
                fires = c.fires,
                notified = c.notified,
                watched_whole_day = self.complete(),
                late_prints_dropped = late.prints_dropped,
                late_prints_after_decision = late.prints_after_decision,
                "n1 shadow: day summary"
            );
        }
        self.day = Some(d);
        self.syms.clear();
        self.due.clear();
        self.counts = Counts::default();
        self.fired.clear();
        self.facts_late = false;
        if self.facts.as_ref().is_some_and(|f| f.date != Some(date)) {
            self.facts = None;
        }
        true
    }

    /// One print the Runners board counted (`counted` is the chart engine's verdict). O(1).
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        let ts = t.event_ts.0;
        self.see(ts);
        let Some(s) = t.session else { return };
        if s.kind == SessionKind::Overnight || !(counted.ohlc || counted.volume) {
            return;
        }
        if !self.roll(s.date) {
            return;
        }
        let d = self.day.expect("rolled");
        // A symbol the facts rule out never fires: no state for it (most of the market's ETFs,
        // warrants and units, and anything without a previous close).
        if let Some(f) = self.facts.as_deref()
            && (f.sec_type(&t.symbol) == SecType::Excluded
                || f.facts
                    .get(t.symbol.as_str())
                    .is_none_or(|x| x.c1.is_none_or(|c| c <= 0.0)))
        {
            return;
        }
        let p = logic::Print {
            ns: ts,
            px: t.price.micros(),
            size: t.size,
            ohlc: counted.ohlc,
            vol: counted.volume,
        };
        let key: Arc<str> = match self.syms.get_key_value(t.symbol.as_str()) {
            Some((k, _)) => k.clone(),
            None => {
                let k: Arc<str> = Arc::from(t.symbol.as_str());
                self.syms.insert(k.clone(), logic::Sym::new());
                k
            }
        };
        let sym = self.syms.get_mut(&key).expect("inserted");
        let before = sym.next_due(&d.clock);
        sym.apply(&p, &d.clock);
        let after = sym.next_due(&d.clock);
        if let Some(t) = after
            && after != before
        {
            self.due.push(Reverse((t, key)));
        }
    }

    /// The clock reached `now` (the rocket task's tick): every bar whose decision time has come
    /// is decided, and the day's facts are asked for when due.
    pub fn tick(&mut self, now: i64) {
        self.see(now);
        let date = trading_date(UnixNanos(now));
        if !self.roll(date) {
            return;
        }
        let d = self.day.expect("rolled");
        if self.facts.as_ref().is_none_or(|f| f.date != Some(d.date))
            && self.facts_asked != Some(d.date)
            && now >= d.clock.t0 - FACTS_LEAD_NS
        {
            self.facts_asked = Some(d.date);
            self.facts_wanted = Some(d.date);
        }
        while let Some(Reverse((t, _))) = self.due.peek() {
            if *t > now {
                break;
            }
            let Reverse((_, key)) = self.due.pop().expect("peeked");
            self.decide(&key, now, d);
        }
    }

    fn decide(&mut self, key: &Arc<str>, now: i64, d: Day) {
        let facts = self.facts.clone().filter(|f| f.date == Some(d.date));
        if facts.is_none() && !self.facts_late {
            // A bar decided without the day's facts can never fire, and its budget would be
            // wrong from then on: the day is incomplete.
            self.facts_late = true;
            tracing::warn!(day = %d.date, "n1 shadow: a bar was due before the day's facts were loaded; nothing is sent today");
        }
        let (fx, sec) = match facts.as_deref() {
            Some(f) => (f.facts.get(&**key).copied(), f.sec_type(key)),
            None => (None, SecType::Unknown),
        };
        let complete = self.complete();
        let Some(sym) = self.syms.get_mut(key) else {
            return;
        };
        let cands = sym.decide_due(now, &d.clock, fx.as_ref(), sec);
        if let Some(next) = sym.next_due(&d.clock) {
            self.due.push(Reverse((next, key.clone())));
        }
        for c in cands {
            self.counts.candidates += 1;
            self.counts.in_universe += c.in_universe as usize;
            self.counts.kept += c.kept as usize;
            if !c.fire {
                continue;
            }
            self.counts.fires += 1;
            self.fire(key, &c, now, d, complete);
        }
    }

    fn fire(&mut self, key: &Arc<str>, c: &Cand, now: i64, d: Day, complete: bool) {
        let detail = N1Notice {
            low: c.low_u as f64 / 1e6,
            high: c.high_u as f64 / 1e6,
            gain: c.gain(),
            dollars: c.dollars,
            prints: c.prints,
            rel: c.rel,
            emitted_ns: now,
        };
        let price = c.close_u as f64 / 1e6;
        tracing::info!(
            symbol = %key, t_ns = c.t, emitted_ns = now, price, gain = c.gain(),
            dollars = c.dollars, rel = ?c.rel, complete,
            "n1 shadow: fire"
        );
        if self.fired.len() < MAX_FIRED {
            self.fired.push(Fired {
                symbol: key.to_string(),
                day: d.date,
                t: c.t,
                price,
                detail: detail.clone(),
                notified: complete,
            });
        }
        if !complete {
            return;
        }
        self.counts.notified += 1;
        if self.notices.len() < MAX_NOTICES {
            self.notices.push(Notice {
                kind: NoticeKind::N1,
                alert_id: format!("{}-{key}-n1-{}", d.date, c.t.div_euclid(logic::SEC)),
                symbol: key.to_string(),
                at_ns: c.t,
                price,
                from_flag_pct: None,
                reasons: Vec::new(),
                since_start_min: None,
                exec_ns: Some(c.close_ns),
                detail: Some(NoticeDetail::N1(detail)),
            });
        }
    }

    /// The notices decided since the last call.
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// The date whose facts the market should load now, once ([`load_facts`]).
    pub fn take_facts_request(&mut self) -> Option<NaiveDate> {
        self.facts_wanted.take()
    }

    pub fn set_facts(&mut self, table: FactsTable) {
        self.set_facts_shared(Arc::new(table));
    }

    /// [`N1Live::set_facts`] with a table shared with T2-c4's F1u (`crate::c4_live`).
    pub fn set_facts_shared(&mut self, table: Arc<FactsTable>) {
        tracing::info!(
            date = ?table.date,
            symbols = table.facts.len(),
            assets = table.assets,
            raw_dates = table.raw_dates,
            newest_raw = ?table.newest_raw,
            "n1 shadow: facts loaded"
        );
        self.facts = Some(table);
    }

    /// Today's fires, oldest first (the Runners board's Shadow section).
    pub fn fired(&self) -> &[Fired] {
        &self.fired
    }

    /// The board frame's `shadow` object, or `None` with nothing to show today.
    pub fn frame(&self) -> Option<Value> {
        if self.fired.is_empty() {
            return None;
        }
        let rows: Vec<Value> = self
            .fired
            .iter()
            .rev()
            .map(|f| {
                json!({
                    "s": f.symbol, "at": f.t / 1_000_000, "p": f.price,
                    "gain_pct": (f.detail.gain * 1000.0).round() / 10.0,
                    "low": f.detail.low, "high": f.detail.high,
                    "dollars": f.detail.dollars.round(),
                    "rel": f.detail.rel.map(|r| (r * 10.0).round() / 10.0),
                    "notified": f.notified,
                })
            })
            .collect();
        Some(json!({ "n1": { "label": "N1 · shadow", "config": logic::CONFIG, "rows": rows } }))
    }

    /// The cost line's `n1` entry.
    pub fn cost(&self) -> Value {
        json!({
            "day": self.day.map(|d| d.date.to_string()),
            "complete": self.complete(),
            "symbols": self.syms.len(),
            "held": self.syms.values().map(|s| s.held()).sum::<usize>(),
            "due": self.due.len(),
            "facts": self.facts.as_ref().map(|f| json!({"date": f.date.map(|d| d.to_string()), "symbols": f.facts.len(), "newest_raw": f.newest_raw.map(|d| d.to_string())})),
            "candidates": self.counts.candidates,
            "kept": self.counts.kept,
            "fires": self.counts.fires,
            "notified": self.counts.notified,
            "late_prints_dropped": self.late().prints_dropped,
        })
    }

    /// Late prints over today's symbols ([`logic::Late`]).
    fn late(&self) -> logic::Late {
        self.syms
            .values()
            .fold(logic::Late::default(), |a, s| logic::Late {
                prints_dropped: a.prints_dropped + s.late.prints_dropped,
                prints_after_decision: a.prints_after_decision + s.late.prints_after_decision,
            })
    }
}

#[cfg(test)]
mod tests;
