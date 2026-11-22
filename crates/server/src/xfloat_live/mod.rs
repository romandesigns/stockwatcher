//! x-float-1 "rotation ignition", live (owner 2026-11-22: ship the research-proven v2 wins before
//! the judge; `outputs/stockwatcher-research-v2/DEPLOY-PLAN.md`, last addendum). A low-float stock
//! (at most 50 M shares) that has already traded a quarter of its float since 04:00 ET, is up at
//! least 10 % on the previous close and prints a new session high.
//!
//! # Why it exists
//!
//! Research v2, wave 2, lane x-float (`30-w2/L-x-float.md`, finalist `30-w2/finalists/x-float-1.json`,
//! verified `x-float-1.verify.json`): added to the live baseline B0 it caught more of the +20 %
//! windows (Train +0.78 pp recall, Validation +1.1) at higher precision (Train +0.22 pp, Validation
//! +0.15), about 6.7 alerts a day on Train and 17 on Validation; its own alerts were GOOD 25 % of
//! the time on Train. It is a separate stream beside production, never a replacement. The text
//! says what happened and never says "buy".
//!
//! # What it does here
//!
//! Only ADDS notices ([`crate::rocket::NoticeKind::XFloat`], data kind `v2.xfloat`, title
//! "SYM · Float rotation · $price"). Nothing production decides changes. Every fire goes through
//! the Runners notices' delivery (persisted dedupe, devices, quiet hours, the 3-minute stale
//! drop) and the ledger, which records the detector id and version, the session, the close
//! print's arrival time and the facts it was judged on. It has two switches: the per-device /
//! per-desk delivery switch `xFloat` (on by default) and the server-wide runtime switch
//! `detectors."x-float-1".on` (`crate::alerts::detectors`), which holds every fire as
//! `switched_off` (still ledgered) the moment it is flipped, with no restart: the DEPLOY-PLAN's
//! kill rule.
//!
//! The detector itself is [`logic`] (module docs there: every step with the research's
//! arithmetic). This module feeds it: every print the Runners board counts, with the chart
//! engine's verdict, 04:00–20:00 ET; the rocket task's clock, which decides each bar 50 ms after
//! it ends (`crate::market::ROCKET_SETTLE`, the research's decision instant); and, per decision,
//! the symbol's float and previous close (read only when a bar is due, never per print):
//!
//! - **Float**: the stock-context engine's latest public float (`ContextBoard::float_shares`,
//!   FMP, `STOCK_CONTEXT=1`). The research used the SEC-derived `float_ex_insiders` as of the day
//!   with its qualification flags (median error of the FMP float against it ×1.14, declared in
//!   the finalist). This difference cannot be removed: live has no SEC float. Without
//!   `STOCK_CONTEXT` there is no float and x-float-1 never fires.
//! - **C1**: the quote board's previous close, put on the day's share basis by the
//!   corporate-actions ledger (`market_archive::PrevCloses::with_splits`); the research used the
//!   engine-a reference's split-restated daily close. A close the quote board withholds
//!   (`crate::quotes::split_guard`) never fires.
//! - **Security type** from the N1 facts table (`crate::n1_live::FactsTable::sec_type`,
//!   `universe-v1`): an excluded type never fires (the research's symbols were universe-v1's).
//!
//! # A day not watched whole never alerts
//!
//! The rotation is the shares traded since 04:00 and the new high is against every earlier bar,
//! so after a restart mid-day (earlier prints not replayed into this detector) the day keeps
//! deciding, for the counts, but sends nothing; so does a day whose security types were not
//! loaded before its first decision. A deploy after 20:00 ET is watched whole the next day.
//!
//! # Known differences from the research stream (measured or bounded in the parity report,
//! `C:/wavystack/outputs/stockwatcher-research-v2/50-ship/float/PARITY.md`)
//!
//! - **Float source** and **C1 source**, above.
//! - **Arrival.** A print reaching the server after its bar was decided is in the research's bar
//!   but not in the live decision; it still counts for the later bars (`late` in the cost line).
//! - **Canceled / corrected prints.** The research store dropped them with hindsight; live counts
//!   a print as the chart engine counted it when it arrived.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use chrono::NaiveDate;
use market_events::session::{SessionKind, sessions_for, trading_date};
use market_events::{Trade, UnixNanos};
use serde_json::{Value, json};

use crate::n1_live::{FactsTable, SecType};
use crate::quotes::Counted;
use crate::rocket::{Notice, NoticeDetail, NoticeKind};

pub mod logic;
pub use logic::{Cand, Clock, Inputs};

/// [`XFloatLive::complete`]'s grace after 04:00 ET.
const START_GRACE_NS: i64 = logic::MIN;
/// Notices held while nobody takes them.
const MAX_NOTICES: usize = 256;
/// Today's fires kept for the cost line (research: 6.7 a day on Train, 17 on Validation).
const MAX_FIRED: usize = 500;

/// One fire's facts (the notice's [`NoticeDetail::XFloat`], and the ledger's).
#[derive(Clone, Debug, PartialEq)]
pub struct XFloatNotice {
    /// The float it was judged against (shares) and where it came from.
    pub float: f64,
    pub float_source: &'static str,
    /// Shares traded since 04:00 through the bar, and over the float (0.31 = 31 %).
    pub shares: u64,
    pub rotation: f64,
    /// The previous close (dollars) and the gain to the bar's close (0.124 = +12.4 %).
    pub c1: f64,
    pub gain: f64,
    /// The highest earlier high the close broke (dollars; 0 = the day's first priced bar).
    pub prior_high: f64,
    /// The close print's arrival at the server (epoch ns; the ledger's `arrival_ns`), when the
    /// provider path stamps it.
    pub arrival_ns: Option<i64>,
    /// When the server decided it (the rocket task's clock, epoch ns).
    pub emitted_ns: i64,
}

/// One of today's fires (the cost line's counts).
#[derive(Clone, Debug, PartialEq)]
pub struct Fired {
    pub symbol: String,
    /// The research decision instant (bar end + 50 ms), epoch ns.
    pub t: i64,
    pub price: f64,
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
    Some(Day {
        date,
        clock: Clock { t0 },
    })
}

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    /// Bars decided with a float and a C1 (the ones that could fire).
    judged: u64,
    fires: usize,
    notified: usize,
}

/// The detector's live state (module docs). Fed under the core lock with the Runners board: O(1)
/// per print, and per tick only the symbols with a bar due.
#[derive(Default)]
pub struct XFloatLive {
    day: Option<Day>,
    /// The first instant this detector saw (a print or a tick).
    started_at: Option<i64>,
    /// The security types (N1's facts table, shared).
    facts: Option<Arc<FactsTable>>,
    facts_wanted: Option<NaiveDate>,
    facts_asked: Option<NaiveDate>,
    syms: HashMap<Arc<str>, logic::Sym>,
    /// `(decision time, symbol)` of every open bar's symbol, earliest first.
    due: BinaryHeap<Reverse<(i64, Arc<str>)>>,
    counts: Counts,
    fired: Vec<Fired>,
    notices: Vec<Notice>,
    /// A bar was decided before the day's types were loaded (the day is incomplete).
    facts_late: bool,
}

impl XFloatLive {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the current day was watched from its 04:00 ET with its security types in time.
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
            tracing::info!(
                day = %prev.date,
                symbols = self.syms.len(),
                judged = c.judged,
                fires = c.fires,
                notified = c.notified,
                watched_whole_day = self.complete(),
                late_prints_after_decision = self.late(),
                "x-float-1: day summary"
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
        if self
            .facts
            .as_deref()
            .is_some_and(|f| f.sec_type(&t.symbol) == SecType::Excluded)
        {
            return;
        }
        let p = logic::Print {
            ns: ts,
            recv_ns: t.receive_ts.map_or(0, |r| r.0),
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
        if sym.done() {
            // Fired today already: once a symbol-day, nothing it prints matters any more.
            return;
        }
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
    /// is decided against `inputs(symbol)` (the float and previous close; module docs), and the
    /// day's security types are asked for when due.
    pub fn tick(&mut self, now: i64, inputs: impl Fn(&str) -> Inputs) {
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
            self.decide(&key, now, d, &inputs);
        }
    }

    fn decide(&mut self, key: &Arc<str>, now: i64, d: Day, inputs: &dyn Fn(&str) -> Inputs) {
        let facts = self.facts.clone().filter(|f| f.date == Some(d.date));
        if facts.is_none() && !self.facts_late {
            self.facts_late = true;
            tracing::warn!(day = %d.date, "x-float-1: a bar was due before the day's security types were loaded; nothing is sent today");
        }
        let complete = self.complete();
        let Some(sym) = self.syms.get_mut(key) else {
            return;
        };
        let excluded = facts
            .as_deref()
            .is_some_and(|f| f.sec_type(key) == SecType::Excluded);
        // The float and C1 are looked up only for a symbol that can still fire.
        let inp = if excluded || sym.done() {
            Inputs::default()
        } else {
            inputs(key)
        };
        if inp.float.is_some() && inp.c1_u.is_some() {
            self.counts.judged += 1;
        }
        let cand = sym.decide_due(now, &d.clock, inp);
        if let Some(next) = sym.next_due(&d.clock) {
            self.due.push(Reverse((next, key.clone())));
        }
        if let Some(c) = cand {
            self.counts.fires += 1;
            self.fire(key, &c, now, d, complete);
        }
    }

    fn fire(&mut self, key: &Arc<str>, c: &Cand, now: i64, d: Day, complete: bool) {
        let detail = XFloatNotice {
            float: c.float,
            float_source: FLOAT_SOURCE,
            shares: c.cum,
            rotation: c.rotation(),
            c1: c.c1_u as f64 / 1e6,
            gain: c.gain(),
            prior_high: c.prior_high_u as f64 / 1e6,
            arrival_ns: (c.close_recv_ns != 0).then_some(c.close_recv_ns),
            emitted_ns: now,
        };
        let price = c.close_u as f64 / 1e6;
        tracing::info!(
            symbol = %key, t_ns = c.t, emitted_ns = now, price, gain = detail.gain,
            rotation = detail.rotation, float = c.float, complete,
            "x-float-1: fire"
        );
        if self.fired.len() < MAX_FIRED {
            self.fired.push(Fired {
                symbol: key.to_string(),
                t: c.t,
                price,
                notified: complete,
            });
        }
        if !complete {
            return;
        }
        self.counts.notified += 1;
        if self.notices.len() < MAX_NOTICES {
            self.notices.push(Notice {
                spread_bp: None,
                no_quote: false,
                kind: NoticeKind::XFloat,
                // Once a symbol-day: the date and symbol are the event.
                alert_id: format!("{}-{key}-xfloat", d.date),
                symbol: key.to_string(),
                at_ns: c.t,
                price,
                from_flag_pct: None,
                reasons: Vec::new(),
                since_start_min: None,
                exec_ns: Some(c.close_ns),
                detail: Some(NoticeDetail::XFloat(detail)),
            });
        }
    }

    /// The notices decided since the last call.
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// The date whose security types the market should load now, once (N1's facts table).
    pub fn take_facts_request(&mut self) -> Option<NaiveDate> {
        self.facts_wanted.take()
    }

    pub fn set_facts(&mut self, table: Arc<FactsTable>) {
        self.facts = Some(table);
    }

    /// Today's fires, oldest first.
    pub fn fired(&self) -> &[Fired] {
        &self.fired
    }

    /// The cost line's `xfloat` entry.
    pub fn cost(&self) -> Value {
        json!({
            "day": self.day.map(|d| d.date.to_string()),
            "complete": self.complete(),
            "symbols": self.syms.len(),
            "held": self.syms.values().map(|s| s.held()).sum::<usize>(),
            "due": self.due.len(),
            "judged": self.counts.judged,
            "fires": self.counts.fires,
            "notified": self.counts.notified,
            "late_prints_after_decision": self.late(),
        })
    }

    fn late(&self) -> u64 {
        self.syms
            .values()
            .map(|s| s.late.prints_after_decision)
            .sum()
    }
}

/// Where the live float comes from (the ledger's `float_source`; module docs).
pub const FLOAT_SOURCE: &str = "fmp";

/// The decision inputs from the live float (shares) and previous close (dollars): live floats
/// have no qualification flags, so only the research's finite / 10,000-share floor and the 50 M
/// ceiling apply.
pub fn inputs(float_shares: Option<u64>, prev_close: Option<f64>) -> Inputs {
    Inputs {
        float: logic::eligible_float(float_shares.map(|f| f as f64), true),
        c1_u: logic::c1_micros(prev_close),
    }
}

#[cfg(test)]
mod tests;
