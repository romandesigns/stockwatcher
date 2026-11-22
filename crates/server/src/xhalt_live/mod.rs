//! StockWatcher research v2 finalist **x-halt-1** "resumed up after a halt", live (owner approval
//! 2026-11-22, DEPLOY-PLAN addendum "ship proven wins now": the 8 items that proved better than
//! live). The binding definition is the frozen finalist
//! `C:/wavystack/outputs/stockwatcher-research-v2/30-w2/finalists/x-halt-1.json` (lane report
//! `30-w2/L-x-halt.md`, code `research/v2-x-halt` 860e7af1, pick `RU-u0.05-open0-Kany`).
//!
//! # Why
//!
//! When an LULD pause or a "News pending" halt follows a run-in of 5 % or more and the stock
//! reopens at or above its halt price, the reopen is a high-precision moment: on Train
//! (F2 ∪ F3, 501 sessions, walk-forward, the same pick in both folds) the stream was 4.4 alerts a
//! day at **25.5 % GOOD** (good-v1) against 20.3 % for the live alert set, +8.0 pp over a
//! same-stock random-time placebo; on Validation it added +0.07 pp recall and +0.10 pp precision
//! to the live set at about 8 a day (`30-w2/W2-REPORT.md`). 93 % of its alerts land on a stock
//! the Runners board already alerted that day: it is a "resumed up" re-alert on stocks already on
//! screen, not a source of new movers. The text says what happened, never "buy".
//!
//! # The parts
//!
//! - [`logic`]: the detector, std only (the parity harness compiles it unchanged); its docs say
//!   the definition step by step and every known difference from the research.
//! - [`XHaltLive`] (this file): the live plumbing. It builds each symbol's 1 s and 15 s bars
//!   from the Runners board's prints (the chart engine's counted verdicts, on execution time),
//!   takes the chart engine's halt spans (`chart.halt`, typed by their own reason code), decides
//!   on the rocket task's clock and turns a [`logic::Fire`] into a [`NoticeKind::ResumedUp`]
//!   notice (data kind `halt.resumed_up`).
//! - The day's facts (the previous close C1 and the security types) are N1's table
//!   (`crate::n1_live::load_facts`), shared with N1 and F1u: one read at 03:30 ET.
//!
//! # Switches
//!
//! - `XHALT_LIVE=0` (env, read at start) turns the detector off entirely.
//! - The **detectors** switch `x-halt-1` (synced settings namespace `detectors`,
//!   `crate::alerts::detectors`) holds its notices as `switched_off` at once, no restart; the
//!   detector keeps running and every would-be alert is still a ledger line.
//! - Each device's own `resumedUp` switch (Settings → Alerts), ON by default.
//!
//! # What a day needs
//!
//! An alert needs the 15 minutes before its halt watched (`started_at ≤ halt_ns − 15 min`: the
//! run-in reads that window and nothing older, and the halt price is the last print seen) and
//! the halt's start and resume statuses seen. A restart during a halt never alerts on it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::NaiveDate;
use market_events::Trade;
use market_events::UnixNanos;
use market_events::session::{SessionKind, sessions_for, trading_date};
use serde_json::{Value, json};

use crate::n1_live::{FactsTable, SecType};
use crate::quotes::Counted;
use crate::rocket::{Notice, NoticeDetail, NoticeKind};

pub mod logic;
pub use logic::{Fire, Res};

/// The detectors-switch id (`crate::alerts::detectors`) and the ledger's `detector`.
pub const DETECTOR: &str = "x-halt-1";
/// Notices held while nobody takes them.
const MAX_NOTICES: usize = 256;

/// One x-halt-1 fire's facts (the notice's [`NoticeDetail::ResumedUp`]), dollars.
#[derive(Clone, Debug, PartialEq)]
pub struct ResumedUpNotice {
    /// The halt's start and its resume status (epoch ns).
    pub halt_ns: i64,
    pub resume_ns: Option<i64>,
    /// The halt's reason as the text says it ("LULD pause", "News pending").
    pub reason: String,
    /// The last 15 s close before the halt, the 15-minute low and the run-in (fraction).
    pub halt_px: f64,
    pub low15: f64,
    pub run_in: f64,
    /// When the reopening bar started, and its length (1 s or 15 s).
    pub reopen_start_ns: i64,
    pub reopen_res_s: u8,
    /// When the server decided it (the rocket task's clock, epoch ns).
    pub emitted_ns: i64,
}

#[derive(Clone, Copy, Debug)]
struct Day {
    date: NaiveDate,
    t0: i64,
}

fn day_of(date: NaiveDate) -> Option<Day> {
    let s = sessions_for(date);
    let t0 = s.iter().find(|(id, _, _)| id.kind == SessionKind::Pre)?.1.0;
    Some(Day { date, t0 })
}

/// One symbol: its two bar builders and the detector's state.
struct SymLive {
    b1: logic::Builder,
    b15: logic::Builder,
    det: logic::Sym,
    /// Each waiting halt's reason label (by halt start), for the text.
    reasons: HashMap<i64, String>,
}

impl SymLive {
    fn new(t0: i64) -> Self {
        SymLive {
            b1: logic::Builder::new(t0, logic::SEC),
            b15: logic::Builder::new(t0, logic::SLOT),
            det: logic::Sym::new(),
            reasons: HashMap::new(),
        }
    }

    /// Hands the detector every bar complete at `now`.
    fn flush(&mut self, now: i64) {
        if let Some(b) = self.b1.close_through(now) {
            self.det.push_bar1(b);
        }
        if let Some(b) = self.b15.close_through(now) {
            self.det.push_bar15(b);
        }
    }
}

/// The x-halt-1 detector's live state. Fed under the core lock with the Runners board: O(1) per
/// print; per tick only the symbols with a halt waiting.
#[derive(Default)]
pub struct XHaltLive {
    day: Option<Day>,
    started_at: Option<i64>,
    facts: Option<Arc<FactsTable>>,
    /// The date whose facts the market should load, once (when N1 and F1u are off).
    facts_wanted: Option<NaiveDate>,
    facts_asked: Option<NaiveDate>,
    syms: HashMap<Arc<str>, SymLive>,
    /// Symbols with a halt not decided yet: the only ones a tick looks at.
    waiting: HashSet<Arc<str>>,
    notices: Vec<Notice>,
    fires: u64,
    notified: u64,
    unwatched: u64,
}

impl XHaltLive {
    pub fn new() -> Self {
        Self::default()
    }

    fn see(&mut self, t: i64) {
        if self.started_at.is_none() {
            self.started_at = Some(t);
        }
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
            let c = self.counts();
            tracing::info!(
                day = %prev.date,
                halts = c.halts,
                typed = c.typed,
                eligible = c.eligible,
                below_halt = c.below_halt,
                out_of_band = c.out_of_band,
                rehalted = c.rehalted,
                fires = self.fires,
                notified = self.notified,
                unwatched = self.unwatched,
                late_prints_dropped = self.late().prints_dropped,
                rehalt_after_fire = self.late().rehalt_after_fire,
                "x-halt-1: day summary"
            );
        }
        self.day = Some(d);
        self.syms.clear();
        self.waiting.clear();
        self.fires = 0;
        self.notified = 0;
        self.unwatched = 0;
        if self.facts.as_ref().is_some_and(|f| f.date != Some(date)) {
            self.facts = None;
        }
        true
    }

    /// The day's facts (C1 and the security types), shared with N1.
    pub fn set_facts(&mut self, table: Arc<FactsTable>) {
        self.facts = Some(table);
    }

    /// Whether `sym` may fire today: the facts are today's, its type is not excluded, and it
    /// has a previous close (`c1` known: the research's `c1.is_not_nan()`).
    fn c1_known(&self, sym: &str) -> bool {
        let Some(f) = self.facts.as_deref() else {
            return false;
        };
        if self.day.is_none_or(|d| f.date != Some(d.date)) || f.sec_type(sym) == SecType::Excluded {
            return false;
        }
        f.facts
            .get(sym)
            .and_then(|x| x.c1)
            .is_some_and(|c| c.is_finite() && c > 0.0)
    }

    fn sym(&mut self, sym: &str) -> Option<(Arc<str>, &mut SymLive)> {
        let d = self.day?;
        let key = match self.syms.get_key_value(sym) {
            Some((k, _)) => k.clone(),
            None => {
                let k: Arc<str> = Arc::from(sym);
                self.syms.insert(k.clone(), SymLive::new(d.t0));
                k
            }
        };
        let s = self.syms.get_mut(&key).expect("inserted");
        Some((key, s))
    }

    /// One print the Runners board saw, with the chart engine's verdict. Every print opens its
    /// bars (the research cache's rows); only an OHLC-counted one prices them.
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        let ts = t.event_ts.0;
        let Some(s) = t.session else { return };
        if s.kind == SessionKind::Overnight || !self.roll(s.date) {
            return;
        }
        self.see(ts);
        let px = t.price.micros();
        let Some((_, sl)) = self.sym(&t.symbol) else {
            return;
        };
        for (b, one) in [(&mut sl.b1, true), (&mut sl.b15, false)] {
            match b.print(ts, px, counted.ohlc) {
                Ok(Some(bar)) if one => sl.det.push_bar1(bar),
                Ok(Some(bar)) => sl.det.push_bar15(bar),
                Ok(None) => {}
                Err(logic::LatePrint) => {
                    // Counted once per print (the 1 s builder closes first).
                    if one {
                        sl.det.late.prints_dropped += 1;
                    }
                }
            }
        }
    }

    /// A halt started (`end_ns` None) or resumed (`chart.halt`), with its reason as the feed gave
    /// it (Alpaca `rc`).
    pub fn apply_halt(&mut self, symbol: &str, start_ns: i64, end_ns: Option<i64>, reason: &str) {
        if !self.roll(trading_date(UnixNanos(start_ns))) {
            return;
        }
        let Some((key, sl)) = self.sym(symbol) else {
            return;
        };
        match end_ns {
            None => {
                // Every bar ending at or before the halt is complete: the halt facts read them.
                sl.flush(start_ns);
                let label = crate::alerts::halts::reason_label(reason);
                sl.det.halt(start_ns, logic::typed_reason(&label));
                sl.reasons.insert(start_ns, label);
                self.waiting.insert(key);
            }
            Some(e) => sl.det.resume(start_ns, e),
        }
    }

    /// The rocket task's clock reached `now`.
    pub fn tick(&mut self, now: i64) {
        let date = trading_date(UnixNanos(now));
        if !self.roll(date) {
            return;
        }
        let d = self.day.expect("rolled");
        if self.facts.as_ref().is_none_or(|f| f.date != Some(d.date))
            && self.facts_asked != Some(d.date)
            && now >= d.t0 - crate::n1_live::FACTS_LEAD_NS
        {
            self.facts_asked = Some(d.date);
            self.facts_wanted = Some(d.date);
        }
        let keys: Vec<Arc<str>> = self.waiting.iter().cloned().collect();
        for key in keys {
            let known = self.c1_known(&key);
            let Some(sl) = self.syms.get_mut(&key) else {
                self.waiting.remove(&key);
                continue;
            };
            sl.flush(now);
            if sl.det.next_due().is_none_or(|t| t > now) {
                continue;
            }
            let fires = sl.det.decide(now, known);
            let reasons: Vec<String> = fires
                .iter()
                .map(|f| sl.reasons.remove(&f.halt_ns).unwrap_or_default())
                .collect();
            if !sl.det.waiting() {
                sl.reasons.clear();
                self.waiting.remove(&key);
            }
            for (f, reason) in fires.into_iter().zip(reasons) {
                self.fire(&key, &f, reason, now, d);
            }
        }
    }

    fn fire(&mut self, key: &Arc<str>, f: &Fire, reason: String, now: i64, d: Day) {
        self.fires += 1;
        let u = |x: i64| x as f64 / 1e6;
        let watched = self
            .started_at
            .is_some_and(|s| s <= f.halt_ns - logic::WIN15);
        tracing::info!(
            symbol = %key, at_ns = f.at_ns, emitted_ns = now, price = u(f.reopen_px),
            halt_ns = f.halt_ns, halt_px = u(f.pre.halt_px), run_in = f.pre.run_in,
            res = ?f.res, watched, "x-halt-1: fire"
        );
        if !watched {
            self.unwatched += 1;
            return;
        }
        self.notified += 1;
        if self.notices.len() < MAX_NOTICES {
            self.notices.push(Notice {
                spread_bp: None,
                kind: NoticeKind::ResumedUp,
                alert_id: format!("{}-{key}-xh{}", d.date, f.halt_ns.div_euclid(logic::SEC)),
                symbol: key.to_string(),
                at_ns: f.at_ns,
                price: u(f.reopen_px),
                from_flag_pct: None,
                reasons: Vec::new(),
                since_start_min: None,
                exec_ns: None,
                detail: Some(NoticeDetail::ResumedUp(ResumedUpNotice {
                    halt_ns: f.halt_ns,
                    resume_ns: f.resume_ns,
                    reason,
                    halt_px: u(f.pre.halt_px),
                    low15: u(f.pre.low15),
                    run_in: f.pre.run_in,
                    reopen_start_ns: f.reopen_start,
                    reopen_res_s: (f.res.len_ns() / logic::SEC) as u8,
                    emitted_ns: now,
                })),
            });
        }
    }

    /// The date whose facts the market should load now, once (`crate::n1_live::load_facts`).
    pub fn take_facts_request(&mut self) -> Option<NaiveDate> {
        self.facts_wanted.take()
    }

    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    fn counts(&self) -> logic::Counts {
        self.syms.values().fold(logic::Counts::default(), |a, s| {
            let c = s.det.counts;
            logic::Counts {
                halts: a.halts + c.halts,
                typed: a.typed + c.typed,
                eligible: a.eligible + c.eligible,
                reopened: a.reopened + c.reopened,
                below_halt: a.below_halt + c.below_halt,
                out_of_band: a.out_of_band + c.out_of_band,
                rehalted: a.rehalted + c.rehalted,
                fires: a.fires + c.fires,
            }
        })
    }

    fn late(&self) -> logic::Late {
        self.syms.values().fold(logic::Late::default(), |a, s| {
            let l = s.det.late;
            logic::Late {
                prints_dropped: a.prints_dropped + l.prints_dropped,
                rehalt_after_fire: a.rehalt_after_fire + l.rehalt_after_fire,
            }
        })
    }

    /// The cost line's `x_halt` entry.
    pub fn cost(&self) -> Value {
        let c = self.counts();
        json!({
            "day": self.day.map(|d| d.date.to_string()),
            "symbols": self.syms.len(),
            "waiting": self.waiting.len(),
            "halts": c.halts,
            "eligible": c.eligible,
            "fires": self.fires,
            "notified": self.notified,
            "unwatched": self.unwatched,
            "late_prints_dropped": self.late().prints_dropped,
            "rehalt_after_fire": self.late().rehalt_after_fire,
        })
    }
}

#[cfg(test)]
mod tests;
