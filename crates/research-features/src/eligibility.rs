//! Historical price-band eligibility, version [`POLICY_ID`] (`eligibility-research-v1`).
//!
//! Production decides band membership on the day's latest price (and, historically, the universe
//! was taken from a close: future information). Research rebuilds it point in time:
//!
//! - **Price source.** The last *eligible* trade observed by decision time `t`: a print whose
//!   research condition effect updates the **last** price (`conditions-research-v1`, so odd lots,
//!   average-price prints, official open/close records and every unresolved print never move it),
//!   in a SIP session (pre 04:00, regular, after). Never the day's close, high, or anything later
//!   than `t`. Raw (unadjusted) prices: membership is about the price the stock trades at then.
//! - **Band.** Inclusive `[min, max]`, research default $0.15-$25.00 ([`EligibilityConfig`]);
//!   production's $0.10 floor is untouched (`market_events::PriceBand`).
//! - **Staleness.** A last eligible trade older than X cannot make a symbol currently eligible:
//!   X = 30 min in pre-market, 10 min in the regular session, 30 min after hours (evaluated with
//!   the session of `t`). A price never carries across trading dates: the first eligible print of
//!   the day is needed (the previous day's price is raw and may be pre-split).
//! - **Sessions.** Decisions are made only in SIP sessions (04:00-20:00 ET, early closes per the
//!   calendar). The overnight (BOATS) session is a separate venue with its own prints and is not
//!   in the archive: at an overnight `t` membership is [`Membership::Closed`] and overnight prints
//!   never set the SIP last price.
//! - **Entry / exit / re-entry.** Entry when membership becomes [`Membership::Eligible`]; exit
//!   when it stops being eligible (out of band, stale, session closed). Re-entry is an entry like
//!   any other, unlimited, with no cool-down. **No hysteresis**: membership stays a pure function
//!   of `(last eligible print, t)`. Flapping at a band edge only produces entry/exit events (which
//!   are counted), and costs nothing, because history retention does not depend on membership.
//! - **Warmup history.** Every symbol's data is retained from the start of its trading day's
//!   pre-market (04:00 ET, [`history_start`]) whether or not it is eligible, so a symbol entering
//!   the band at 10:42 has the same VWAP/EMA the chart would show (the chart's VWAP runs from the
//!   day's first candle; its EMA has no reset). Cross-day history (prior closes, multi-day warmup)
//!   is split-adjusted ([`crate::corporate`]).

use crate::conditions::ResearchEffect;
use market_events::session::session_bounds;
use market_events::time::NANOS_PER_SEC;
use market_events::{Price, SessionId, SessionKind, Trade, UnixNanos, classify};
use serde::{Deserialize, Serialize};

pub const POLICY_ID: &str = "eligibility-research-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EligibilityConfig {
    /// Inclusive band, micro-dollars.
    pub band_min: Price,
    pub band_max: Price,
    /// Staleness limits (nanoseconds), by the session of the decision time.
    pub stale_pre_ns: i64,
    pub stale_regular_ns: i64,
    pub stale_after_ns: i64,
}

impl Default for EligibilityConfig {
    fn default() -> Self {
        EligibilityConfig {
            band_min: Price::from_cents(15),
            band_max: Price::from_cents(2500),
            stale_pre_ns: 30 * 60 * NANOS_PER_SEC,
            stale_regular_ns: 10 * 60 * NANOS_PER_SEC,
            stale_after_ns: 30 * 60 * NANOS_PER_SEC,
        }
    }
}

impl EligibilityConfig {
    pub fn with_band(mut self, min: Price, max: Price) -> Result<Self, String> {
        if min.micros() <= 0 || max < min {
            return Err(format!("invalid band {min}..={max}"));
        }
        self.band_min = min;
        self.band_max = max;
        Ok(self)
    }

    pub fn in_band(&self, p: Price) -> bool {
        p >= self.band_min && p <= self.band_max
    }

    /// The staleness limit at a decision time in session `kind`; `None` outside SIP sessions.
    pub fn stale_limit(&self, kind: SessionKind) -> Option<i64> {
        match kind {
            SessionKind::Pre => Some(self.stale_pre_ns),
            SessionKind::Regular => Some(self.stale_regular_ns),
            SessionKind::After => Some(self.stale_after_ns),
            SessionKind::Overnight => None,
        }
    }
}

/// The last eligible print a symbol had at some time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastEligible {
    pub ts: UnixNanos,
    pub price: Price,
    pub session: SessionId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Membership {
    Eligible,
    OutOfBand,
    /// The last eligible print is older than the session's limit, or from an earlier trading date.
    Stale,
    /// No eligible print yet on this trading date.
    NoPrice,
    /// `t` is in no SIP session (overnight, closed evening, weekend, holiday).
    Closed,
}

/// Whether a print can set the SIP last price: its research effect updates the last, and it
/// executed in a SIP session.
pub fn is_eligible_print(t: &Trade, effect: &ResearchEffect) -> bool {
    effect.updates.last
        && matches!(
            classify(t.event_ts).map(|s| s.kind),
            Some(SessionKind::Pre | SessionKind::Regular | SessionKind::After)
        )
}

/// Membership at decision time `t`, from the last eligible print observed **by** `t`.
/// A `last` later than `t` is a caller error and counts as no price (it cannot have been known).
pub fn membership_at(
    last: Option<&LastEligible>,
    t: UnixNanos,
    cfg: &EligibilityConfig,
) -> Membership {
    let Some(now) = classify(t) else {
        return Membership::Closed;
    };
    let Some(limit) = cfg.stale_limit(now.kind) else {
        return Membership::Closed;
    };
    let Some(last) = last.filter(|l| l.ts <= t) else {
        return Membership::NoPrice;
    };
    if last.session.date != now.date {
        return Membership::Stale;
    }
    if t.0 - last.ts.0 > limit {
        return Membership::Stale;
    }
    if cfg.in_band(last.price) {
        Membership::Eligible
    } else {
        Membership::OutOfBand
    }
}

/// Where retained history starts for a symbol trading on `date`: that date's 04:00 ET.
pub fn history_start(date: chrono::NaiveDate) -> Option<UnixNanos> {
    session_bounds(SessionId::new(date, SessionKind::Pre)).map(|(s, _)| s)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    /// First entry of the trading date.
    Entry,
    /// A later entry on the same trading date.
    ReEntry,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    /// The evaluation time at which the change was observed (a trade's time, or the decision
    /// time passed to [`EligibilityTracker::advance`]).
    pub at: UnixNanos,
    pub kind: TransitionKind,
    pub from: Membership,
    pub to: Membership,
}

/// Per-symbol state machine over [`membership_at`]. Feed trades in `event_ts` order and call
/// [`EligibilityTracker::advance`] at each decision time; both return the transition, if any.
/// Deterministic: the same prints and decision times give the same transitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EligibilityTracker {
    cfg: EligibilityConfig,
    last: Option<LastEligible>,
    state: Membership,
    /// Trading date of the last evaluation, and entries on it.
    date: Option<chrono::NaiveDate>,
    entries_today: u32,
    exits_today: u32,
}

impl EligibilityTracker {
    pub fn new(cfg: EligibilityConfig) -> Self {
        EligibilityTracker {
            cfg,
            last: None,
            state: Membership::NoPrice,
            date: None,
            entries_today: 0,
            exits_today: 0,
        }
    }

    pub fn last(&self) -> Option<&LastEligible> {
        self.last.as_ref()
    }

    pub fn state(&self) -> Membership {
        self.state
    }

    pub fn entries_today(&self) -> u32 {
        self.entries_today
    }

    pub fn exits_today(&self) -> u32 {
        self.exits_today
    }

    /// Observes a print (with its research effect) and re-evaluates at its time.
    pub fn observe(&mut self, t: &Trade, effect: &ResearchEffect) -> Option<Transition> {
        if is_eligible_print(t, effect) {
            let session = classify(t.event_ts).expect("eligible prints are in a session");
            if self.last.is_none_or(|l| l.ts <= t.event_ts) {
                self.last = Some(LastEligible {
                    ts: t.event_ts,
                    price: t.price,
                    session,
                });
            }
        }
        self.advance(t.event_ts)
    }

    /// Re-evaluates membership at decision time `now`.
    pub fn advance(&mut self, now: UnixNanos) -> Option<Transition> {
        if let Some(s) = classify(now)
            && self.date != Some(s.date)
        {
            self.date = Some(s.date);
            self.entries_today = 0;
            self.exits_today = 0;
        }
        let to = membership_at(self.last.as_ref(), now, &self.cfg);
        let from = self.state;
        if to == from {
            return None;
        }
        self.state = to;
        let kind = match (from == Membership::Eligible, to == Membership::Eligible) {
            (false, true) => {
                self.entries_today += 1;
                if self.entries_today == 1 {
                    TransitionKind::Entry
                } else {
                    TransitionKind::ReEntry
                }
            }
            (true, false) => {
                self.exits_today += 1;
                TransitionKind::Exit
            }
            // A change between two non-eligible states (e.g. no price -> out of band).
            _ => return None,
        };
        Some(Transition {
            at: now,
            kind,
            from,
            to,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conditions::research_effect;
    use chrono::NaiveDate;
    use market_events::Tape;
    use market_events::session::et;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    }

    fn at(h: u32, m: u32, s: u32) -> UnixNanos {
        et(day(), h, m, s, 0).unwrap()
    }

    fn tr(ts: UnixNanos, cents: i64, conds: &[&str]) -> (Trade, ResearchEffect) {
        let t = Trade::new("X", ts, Price::from_cents(cents), 100, 1)
            .with_tape(Tape::C)
            .with_conditions(conds.iter().copied());
        let e = research_effect(&t.conditions, t.tape);
        (t, e)
    }

    fn last(ts: UnixNanos, cents: i64) -> LastEligible {
        LastEligible {
            ts,
            price: Price::from_cents(cents),
            session: classify(ts).unwrap(),
        }
    }

    #[test]
    fn band_is_inclusive_and_configurable() {
        let cfg = EligibilityConfig::default();
        assert_eq!(POLICY_ID, "eligibility-research-v1");
        assert!(cfg.in_band(Price::from_cents(15)));
        assert!(cfg.in_band(Price::from_cents(2500)));
        assert!(!cfg.in_band(Price::from_micros(149_999)));
        assert!(!cfg.in_band(Price::from_micros(25_000_001)));
        let prod = cfg
            .with_band(Price::from_cents(10), Price::from_cents(2500))
            .unwrap();
        assert!(prod.in_band(Price::from_cents(10)));
        assert!(
            cfg.with_band(Price::from_cents(0), Price::from_cents(1))
                .is_err()
        );
    }

    #[test]
    fn membership_rules() {
        let cfg = EligibilityConfig::default();
        let t = at(10, 0, 0);
        assert_eq!(membership_at(None, t, &cfg), Membership::NoPrice);
        assert_eq!(
            membership_at(Some(&last(at(9, 55, 0), 300)), t, &cfg),
            Membership::Eligible
        );
        assert_eq!(
            membership_at(Some(&last(at(9, 55, 0), 2600)), t, &cfg),
            Membership::OutOfBand
        );
        // Regular-session staleness: 10 min, inclusive.
        assert_eq!(
            membership_at(Some(&last(at(9, 50, 0), 300)), t, &cfg),
            Membership::Eligible
        );
        assert_eq!(
            membership_at(Some(&last(at(9, 49, 59), 300)), t, &cfg),
            Membership::Stale
        );
        // Pre-market: 30 min, judged by the session of t.
        assert_eq!(
            membership_at(Some(&last(at(8, 31, 0), 300)), at(9, 0, 0), &cfg),
            Membership::Eligible
        );
        assert_eq!(
            membership_at(Some(&last(at(8, 29, 0), 300)), at(9, 0, 0), &cfg),
            Membership::Stale
        );
        // A print from the future (relative to t) was not known: no price.
        assert_eq!(
            membership_at(Some(&last(at(10, 1, 0), 300)), t, &cfg),
            Membership::NoPrice
        );
        // Previous trading date: stale even though under the limit in wall time? (19:59 -> next
        // day 04:00 is 8 hours anyway; a date change is always stale.)
        let prev = et(NaiveDate::from_ymd_opt(2026, 10, 14).unwrap(), 19, 59, 0, 0).unwrap();
        assert_eq!(
            membership_at(Some(&last(prev, 300)), at(4, 0, 0), &cfg),
            Membership::Stale
        );
        // Overnight and closed times: no decision.
        assert_eq!(
            membership_at(Some(&last(at(19, 59, 0), 300)), at(20, 0, 0), &cfg),
            Membership::Closed
        );
        assert_eq!(
            membership_at(Some(&last(at(9, 55, 0), 300)), at(3, 0, 0), &cfg),
            Membership::Closed
        );
    }

    #[test]
    fn only_last_qualifying_sip_prints_set_the_price() {
        let (odd, e) = tr(at(10, 0, 0), 300, &["@", "I"]);
        assert!(!is_eligible_print(&odd, &e));
        let (unres, e) = tr(at(10, 0, 0), 300, &["@", "E"]);
        assert!(!is_eligible_print(&unres, &e));
        let (pre, e) = tr(at(4, 0, 0), 300, &["@", "T"]);
        assert!(is_eligible_print(&pre, &e));
        let (overnight, e) = tr(at(3, 59, 59), 300, &["@"]);
        assert!(!is_eligible_print(&overnight, &e));
        let (closing, e) = tr(at(16, 0, 0), 300, &["M"]);
        assert!(!is_eligible_print(&closing, &e));
    }

    #[test]
    fn tracker_entry_exit_reentry_and_odd_lots_do_not_move_it() {
        let mut tk = EligibilityTracker::new(EligibilityConfig::default());
        let (t, e) = tr(at(9, 0, 0), 10, &["@", "T"]); // $0.10: below the research floor
        assert_eq!(tk.observe(&t, &e), None);
        assert_eq!(tk.state(), Membership::OutOfBand);
        // An odd lot at $0.20 is not an eligible price.
        let (t, e) = tr(at(9, 1, 0), 20, &["@", "I"]);
        assert_eq!(tk.observe(&t, &e), None);
        let (t, e) = tr(at(9, 2, 0), 16, &["@", "T"]);
        let tr1 = tk.observe(&t, &e).unwrap();
        assert_eq!(tr1.kind, TransitionKind::Entry);
        assert_eq!(tr1.at, at(9, 2, 0));
        // Staleness exit at a decision time.
        assert_eq!(tk.advance(at(9, 20, 0)), None);
        let x = tk.advance(at(9, 32, 1)).unwrap(); // regular now: 30 min > 10 min
        assert_eq!((x.kind, x.to), (TransitionKind::Exit, Membership::Stale));
        // Trades again: re-entry, no cool-down.
        let (t, e) = tr(at(9, 33, 0), 17, &["@"]);
        assert_eq!(tk.observe(&t, &e).unwrap().kind, TransitionKind::ReEntry);
        assert_eq!(tk.entries_today(), 2);
        // Leaves the band upwards.
        let (t, e) = tr(at(9, 34, 0), 2501, &["@"]);
        assert_eq!(tk.observe(&t, &e).unwrap().to, Membership::OutOfBand);
        // 20:00: closed (no exit event: it is already out).
        assert_eq!(tk.advance(at(20, 0, 0)), None);
        assert_eq!(tk.state(), Membership::Closed);
        // Next day resets the per-day counters, and the old price is stale.
        let next = et(NaiveDate::from_ymd_opt(2026, 10, 16).unwrap(), 4, 0, 0, 0).unwrap();
        tk.advance(next);
        assert_eq!(tk.state(), Membership::Stale);
        assert_eq!(tk.entries_today(), 0);
    }

    #[test]
    fn history_starts_at_04_00_et() {
        assert_eq!(history_start(day()), Some(at(4, 0, 0)));
        assert_eq!(
            history_start(NaiveDate::from_ymd_opt(2026, 10, 17).unwrap()),
            None
        );
    }
}
