//! Engine A round-2 finalist **T2-c4**, live (owner approval 2026-11-18): production plus three
//! add-ons gated to 09:30–11:30 ET, minus one prune. The binding definition is the frozen
//! candidate `research/engine-a/r2/T2/frozen/T2-c4.json` (branch `research/engine-a-forward`).
//!
//! # Why
//!
//! On 234 forward days (`C:/wavystack/outputs/stockwatcher-engine-a/r2/07-forward-results.md`)
//! T2-c4 caught 21.4 % of the +20 % runners before they crossed, against 13.5 % for live
//! production, with 22 fewer alerts a day (246 vs 269) and precision, net and lead time inside the
//! guardrails (Δ recall +7.94 pp, 95 % LB +7.45; BH rejects). It is the most recall per alert of
//! the three forward-confirmed finalists. No stream makes money: net −2.3 % a trade; the win is
//! an earlier look at more of the stocks that go on to run. The texts say what happened, never
//! "buy".
//!
//! # The parts
//!
//! | Part | Where | Forward rate (a day) |
//! |---|---|---|
//! | **F1u** "first pullback after ignition": E4 + filter 10, uncapped | [`logic`], [`F1uLive`], notice [`NoticeKind::F1u`] (`shadow.f1u`) | 15.0, 6.6 % reached +10 % |
//! | **HALT**: production's bull Halt watcher gauges | [`halt_addon`], notice [`NoticeKind::HaltBull`] (`halt.bull`) | 20.0, 18.3 % |
//! | **N1** at X 8 % (was 6 %), W 3, k 10 | `crate::n1_live` | 10.9, 12.9 % |
//! | **Prune POST<1**: Run +20 %, shelf entry and every follow-up whose first or new path is Run, under $1 | [`prunes_post1`] (the Runners board's notices) | |
//!
//! The armed micro-pullback ("Pullback setting up") stays: the owner kept it on 2026-11-18, so
//! c4's tested prune `PB` is NOT applied. Its own switch still controls it.
//!
//! Each add-on is its own notice kind with its own switch (Settings → Alerts), ON by default, and
//! every decision is a line in the notice ledger (`crate::alerts::ledger`) under its data kind.
//! `C4_LIVE=0` turns the add-ons and the prune off together (N1's threshold is its own constant).
//!
//! # Known differences from the research stream
//!
//! - **Union rule.** T2-c4 dropped an add-on row at a symbol-second production (or an earlier
//!   add-on) already used. Live sends it: it is a different notice kind (N1's precedent).
//! - **Universe.** The research tape held the end-of-day $0.05–$30 membership; live sees every
//!   symbol (finding F3; N1's live volume matched research on the ledger days anyway).
//! - **HALT stamps** (forward review M2): the replay's halt watcher and the live one decide on
//!   different clocks (0 of 568 matched to the second, 60 % within 60 s), so HALT's live volume is
//!   uncertain, plausibly ±2× a day. That is the open risk the ledger query watches.
//! - **F1u's 1 s gate** opens at the earlier of its price trigger and the day's first halt; the
//!   research opened it at the first halt whenever the day had one ([`logic`] docs).
//! - **Twin tickers** (J3) are an archive artefact; live has one ticker per security.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use chrono::NaiveDate;
use market_events::Trade;
use market_events::UnixNanos;
use market_events::session::{SessionKind, sessions_for, trading_date};
use serde_json::{Value, json};

use crate::n1_live::{FactsTable, SecType};
use crate::quotes::Counted;
use crate::rocket::{AlertKind, Notice, NoticeDetail, NoticeKind};

pub mod logic;
pub use logic::{Cand, Clock};

/// [`F1uLive::complete`]'s grace after 04:00 ET (N1's).
const START_GRACE_NS: i64 = logic::MIN;
/// Notices held while nobody takes them.
const MAX_NOTICES: usize = 256;

/// One F1u fire's facts (the notice's [`NoticeDetail::F1u`]), dollars.
#[derive(Clone, Debug, PartialEq)]
pub struct F1uNotice {
    /// The impulse low and high, the pullback low and the trigger (the last retreat bar's high).
    pub l0: f64,
    pub h0: f64,
    pub lp: f64,
    pub trigger: f64,
    /// Filter 10's extension over the day's low (`None`: unknown).
    pub ext_base: Option<f64>,
    /// When the ignition bar ended (epoch ns).
    pub ign_end_ns: i64,
    /// When the server decided it (the rocket task's clock, epoch ns).
    pub emitted_ns: i64,
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

/// The F1u detector's live state. Fed under the core lock with the Runners board: O(1) per
/// print; per tick only the symbols with a bar or a watch due.
#[derive(Default)]
pub struct F1uLive {
    day: Option<Day>,
    started_at: Option<i64>,
    now: i64,
    facts: Option<Arc<FactsTable>>,
    facts_late: bool,
    /// The date whose facts the market should load, once (when N1 is off; N1 shares its own).
    facts_wanted: Option<NaiveDate>,
    facts_asked: Option<NaiveDate>,
    syms: HashMap<Arc<str>, logic::Sym>,
    due: BinaryHeap<Reverse<(i64, Arc<str>)>>,
    counts: Counts,
    notices: Vec<Notice>,
    /// Every symbol's latest OHLC-counted print `(exec ns, dollars)`: the HALT add-on's universe
    /// price ([`halt_addon`]). One entry a symbol.
    last_ohlc: HashMap<Arc<str>, (i64, f64)>,
}

impl F1uLive {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the current day was watched from its 04:00 ET with its facts in time: the
    /// ignition state and the budget need every print since 04:00 (N1's rule).
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
                "c4 f1u: day summary"
            );
        }
        self.day = Some(d);
        self.syms.clear();
        self.due.clear();
        self.counts = Counts::default();
        self.facts_late = false;
        if self.facts.as_ref().is_some_and(|f| f.date != Some(date)) {
            self.facts = None;
        }
        true
    }

    /// The day's facts (C1 and the security types), shared with N1, which loads them.
    pub fn set_facts(&mut self, table: Arc<FactsTable>) {
        self.facts = Some(table);
    }

    /// A symbol's C1 (`e3_vwap::c1_of`): `None` before the facts are loaded, `Some(None)` for a
    /// symbol that can never fire (an excluded type or no C1 > 0: the research scans nothing for
    /// it), else `Some(Some(c1))`.
    fn c1_of(&self, sym: &str) -> Option<Option<f64>> {
        let f = self.facts.as_deref()?;
        if f.sec_type(sym) == SecType::Excluded {
            return Some(None);
        }
        Some(f.facts.get(sym).and_then(|x| x.c1).filter(|c| *c > 0.0))
    }

    fn key(&mut self, sym: &str) -> Arc<str> {
        match self.syms.get_key_value(sym) {
            Some((k, _)) => k.clone(),
            None => {
                let k: Arc<str> = Arc::from(sym);
                self.syms.insert(k.clone(), logic::Sym::new());
                k
            }
        }
    }

    /// One print the Runners board counted. Every print opens its 15 s slot, counted or not
    /// (the research store's bars).
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        let ts = t.event_ts.0;
        self.see(ts);
        if counted.ohlc {
            let px = t.price.to_f64();
            match self.last_ohlc.get_mut(t.symbol.as_str()) {
                Some(x) => {
                    if ts >= x.0 {
                        *x = (ts, px);
                    }
                }
                None => {
                    self.last_ohlc
                        .insert(Arc::from(t.symbol.as_str()), (ts, px));
                }
            }
        }
        let Some(s) = t.session else { return };
        if s.kind == SessionKind::Overnight || !self.roll(s.date) {
            return;
        }
        let d = self.day.expect("rolled");
        if self.c1_of(&t.symbol).is_some_and(|c| c.is_none()) {
            return;
        }
        let p = logic::Print {
            ns: ts,
            px: t.price.micros(),
            size: t.size,
            ohlc: counted.ohlc,
            vol: counted.volume,
            // The research's entry flag also takes odd lots, which the counted verdict files as
            // volume-only; live has no reason code here, so every volume print is one.
            entry: counted.ohlc || counted.volume,
        };
        let key = self.key(&t.symbol);
        let sym = self.syms.get_mut(&key).expect("inserted");
        let before = sym.next_due(&d.clock, self.now);
        sym.apply(&p, &d.clock);
        let after = sym.next_due(&d.clock, self.now);
        if let Some(t) = after
            && after != before
        {
            self.due.push(Reverse((t, key)));
        }
    }

    /// A halt started (`end_ns` None) or resumed, from the chart engine's `chart.halt`.
    pub fn apply_halt(&mut self, symbol: &str, start_ns: i64, end_ns: Option<i64>) {
        if self.day.is_none() || self.c1_of(symbol).is_some_and(|c| c.is_none()) {
            return;
        }
        let key = self.key(symbol);
        let sym = self.syms.get_mut(&key).expect("inserted");
        match end_ns {
            None => sym.halt_start(start_ns),
            Some(e) => sym.halt_resume(start_ns, e),
        }
    }

    /// The rocket task's clock reached `now`.
    pub fn tick(&mut self, now: i64) {
        self.see(now);
        let date = trading_date(UnixNanos(now));
        if !self.roll(date) {
            return;
        }
        let d = self.day.expect("rolled");
        if self.facts.as_ref().is_none_or(|f| f.date != Some(d.date))
            && self.facts_asked != Some(d.date)
            && now >= d.clock.t0 - crate::n1_live::FACTS_LEAD_NS
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
        if self.facts.as_ref().is_none_or(|f| f.date != Some(d.date)) && !self.facts_late {
            self.facts_late = true;
            tracing::warn!(day = %d.date, "c4 f1u: a bar was due before the day's facts were loaded; nothing is sent today");
        }
        let Some(Some(c1)) = self.c1_of(key) else {
            // Ruled out by the facts (or none loaded, a day that sends nothing): no state.
            self.syms.remove(key);
            return;
        };
        let c1 = Some(c1);
        let complete = self.complete();
        let Some(sym) = self.syms.get_mut(key) else {
            return;
        };
        let cands = sym.advance(now, &d.clock, c1);
        if let Some(next) = sym.next_due(&d.clock, now) {
            self.due.push(Reverse((next, key.clone())));
        }
        for c in cands {
            self.counts.candidates += 1;
            self.counts.in_universe += c.in_universe as usize;
            self.counts.kept += c.kept as usize;
            if c.fire {
                self.counts.fires += 1;
                self.fire(key, &c, now, d, complete);
            }
        }
    }

    fn fire(&mut self, key: &Arc<str>, c: &Cand, now: i64, d: Day, complete: bool) {
        let u = |x: u32| x as f64 / 1e6;
        let price = u(c.px_u);
        tracing::info!(
            symbol = %key, t_ns = c.t, emitted_ns = now, price, score = c.score,
            ext_base = ?c.ext_base, g1 = c.g1, complete,
            "c4 f1u: fire"
        );
        if !complete {
            return;
        }
        self.counts.notified += 1;
        if self.notices.len() < MAX_NOTICES {
            self.notices.push(Notice {
                spread_bp: None,
                no_quote: false,
                kind: NoticeKind::F1u,
                alert_id: format!("{}-{key}-f1u-{}", d.date, c.t.div_euclid(logic::SEC)),
                symbol: key.to_string(),
                at_ns: c.t,
                price,
                from_flag_pct: None,
                reasons: Vec::new(),
                since_start_min: None,
                exec_ns: None,
                detail: Some(NoticeDetail::F1u(F1uNotice {
                    l0: u(c.l0),
                    h0: u(c.h0),
                    lp: u(c.lp),
                    trigger: u(c.trig_u),
                    ext_base: c.ext_base,
                    ign_end_ns: c.ign_end,
                    emitted_ns: now,
                })),
            });
        }
    }

    /// T2-c4's HALT add-on for the Halt watcher gauge notice `n` ([`halt_addon`]), with this
    /// symbol's latest OHLC print and today's types.
    pub fn halt_addon(&self, n: &Notice) -> Option<Notice> {
        let last = self.last_ohlc.get(n.symbol.as_str()).copied();
        let types = self
            .facts
            .as_deref()
            .filter(|f| self.day.is_some_and(|d| f.date == Some(d.date)));
        halt_addon(n, last, types)
    }

    /// The date whose facts the market should load now, once (`crate::n1_live::load_facts`).
    pub fn take_facts_request(&mut self) -> Option<NaiveDate> {
        self.facts_wanted.take()
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// The cost line's `f1u` entry.
    pub fn cost(&self) -> Value {
        json!({
            "day": self.day.map(|d| d.date.to_string()),
            "complete": self.complete(),
            "symbols": self.syms.len(),
            "armed": self.syms.values().filter(|s| s.is_armed()).count(),
            "held": self.syms.values().map(|s| s.held()).sum::<usize>(),
            "due": self.due.len(),
            "candidates": self.counts.candidates,
            "kept": self.counts.kept,
            "fires": self.counts.fires,
            "notified": self.counts.notified,
            "late_prints_dropped": self.late().prints_dropped,
        })
    }

    fn late(&self) -> logic::Late {
        self.syms
            .values()
            .fold(logic::Late::default(), |a, s| logic::Late {
                prints_dropped: a.prints_dropped + s.late.prints_dropped,
                prints_after_decision: a.prints_after_decision + s.late.prints_after_decision,
            })
    }
}

/// T2-c4's HALT add-on: the Halt watcher's gauge notice `n` (`crate::market`, a card appearing)
/// as a [`NoticeKind::HaltBull`] notice, when the research would have kept it:
///
/// - the gauge's side is **bull** (the bench import skipped bear halts: `eval::import_bench`);
/// - **universe at `t`** (the appearance instant): not an excluded type (`types`), and the last
///   OHLC-counted print strictly before `t`, `last` = `(exec ns, price)`, in $0.10–$25 and at most
///   60 s old (`universe::check`);
/// - **T2-c4's gate**: `t` in 09:30–11:30 ET.
///
/// The Halt watcher itself is unchanged: its own notice still goes out under its own switch.
pub fn halt_addon(
    n: &Notice,
    last: Option<(i64, f64)>,
    types: Option<&FactsTable>,
) -> Option<Notice> {
    let Some(NoticeDetail::Halt { bull: true, .. }) = &n.detail else {
        return None;
    };
    if n.kind != NoticeKind::Halt {
        return None;
    }
    let t = n.at_ns;
    let d = day_of(trading_date(UnixNanos(t)))?;
    let excluded = types.is_some_and(|f| f.sec_type(&n.symbol) == SecType::Excluded);
    let last = last.map(|(ns, px)| (ns, (px * 1e6).round() as i64));
    if !logic::halt_addon_keeps(t, last, excluded, &d.clock) {
        return None;
    }
    Some(Notice {
        spread_bp: None,
        no_quote: false,
        kind: NoticeKind::HaltBull,
        alert_id: format!("{}-{}-hb{}", d.date, n.symbol, t.div_euclid(logic::SEC)),
        ..n.clone()
    })
}

/// T2-c4's prune `POST<1` (SPEC §2): a post-cross notice under $1. Post-cross = "Run +20%", a
/// volume-shelf entry, and every follow-up whose first source or new path is Run +20%: they fire
/// after the +20 % cross by construction (0 runners on Train). The tier is the price the notice
/// carries (the research read the last print before the instant; the two agree to the tick but
/// for a print in between).
pub fn prunes_post1(n: &Notice) -> bool {
    let post = match (n.kind, &n.detail) {
        (NoticeKind::Run | NoticeKind::Shelf, _) => true,
        (NoticeKind::FollowUp, Some(NoticeDetail::FollowUp { path, first, .. })) => {
            *path == AlertKind::Run || *first == AlertKind::Run
        }
        _ => false,
    };
    post && n.price < 1.0
}

#[cfg(test)]
mod tests;
