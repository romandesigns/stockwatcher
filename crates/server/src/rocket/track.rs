//! What happens to a Runners row after its flag, from the flag to 20:00 ET: the **later-move
//! outcomes** and the live **momentum state** (owner, 2026-10-22).
//!
//! Both advance at completed 15 s barriers. Later outcomes retain the frozen feature bars;
//! momentum highs use a separate exact integer accumulator, fed only for rows of the printing
//! symbol. An eligible high must be admitted after row creation and execute strictly after the
//! actual creation decision. A bar's later end label alone does not make its preflag high eligible.
//!
//! # Later-move outcomes
//!
//! The strict outcome (`rocket10`/`rocket30`, the 30-minute max gain / drawdown) is unchanged and
//! still settles 45 minutes after the flag. The owner saw strict "faded" tags on rows that ran
//! hours later (SHFS flagged ~04:01 at $0.21, $0.30 by 05:00; XXII flagged 04:00 at $0.28, $0.42
//! by 07:00), so each alert also keeps:
//!
//! - `max_gain_2h_pct` / `max_drawdown_2h_pct`: max bar high / min bar low over the 2 hours after
//!   the flag ([`LATER_WINDOW_NS`]), against the flag price;
//! - `max_gain_session_pct`: max bar high from the flag to 20:00 ET;
//! - `rocket_later`: a canonical +30 % ROCKET mark (`super::features::mark_at`) at any time after
//!   the flag that day, with its start and mark times. A strict hit counts too.
//!
//! "Ran later" on the panel is `max_gain_2h_pct >= RAN_LATER_PCT` (15 %) for a row that did not
//! rocket within the strict window.
//!
//! # Momentum state
//!
//! Every flagged row is watched for bullish momentum. At each barrier the candidate state is the
//! FIRST of these that holds (`P` the barrier price, `P0` the flag price, `H` the highest bar high
//! since the flag, at least `P0`; `VWAP` the day's from 04:00; "pace" the prior 30 minutes'
//! volume per 15 s slot; "below VWAP" means by more than [`VWAP_TOL`], and a missing VWAP never
//! blocks or fades):
//!
//! | state | rule |
//! |---|---|
//! | `halted` | the chart engine reports a trading halt / LULD pause in force (`chart.halt`) |
//! | `breaking_out` | this barrier's eligible source prints made a new exact high since creation, on whole-bar volume ≥ [`BREAK_PACE`]× pace (positive volume suffices with zero baseline); held [`BREAKOUT_HOLD_SLOTS`] more slots while `P` stays within [`BREAKOUT_HOLD_PCT`] of `H` |
//! | `running` | `P ≥ P0·(1 + RUN_MIN_PCT)`, ≥ [`RUN_HIGHS`] new-high bars in the last 3 minutes, `P ≥ VWAP` |
//! | `fading` | `P < VWAP`, or `P ≤ P0·(1 − FADE_BELOW_FLAG_PCT)` |
//! | `pullback` | `H ≥ P0·(1 + PULLBACK_MOVE_PCT)` (there was a move), `P ≤ H·(1 − PULLBACK_MIN_PCT)`, `P ≥ VWAP`, and `P ≥` the last higher low |
//! | `building` | higher lows over the last three rolling minutes (each minute's low above the one before), 3-minute volume ≥ [`BUILD_PACE`]× its prior-30-minute pace, `P ≥ VWAP` |
//! | `watching` | none of the above (the state at the flag) |
//!
//! The "last higher low" is the most recent rolling-minute low that was above the minute before
//! it. A candidate becomes the state once it holds on [`DEBOUNCE`] consecutive barriers (30 s),
//! so a state does not flicker every 15 s; `halted` (both ways) and `breaking_out` take effect at
//! once. `state_since` is the barrier the state took effect.
//!
//! Between barriers the panel shows `breaking_out` **provisionally** when the live last price is
//! above `H` (prints of the bar still forming; flagged `provisional`, never logged), and
//! `halted` as soon as the halt arrives.
//!
//! Every transition goes to the forward log as a `state` line (at most
//! [`MAX_TRANSITIONS`] per alert a day, so a choppy symbol cannot flood it), so the owner can
//! check later whether a state preceded a run.

use super::breakout::{self, Exact};
use super::{MIN_NS, Mark};
use serde_json::{Value, json};

/// The later-move window after the flag.
pub const LATER_WINDOW_NS: i64 = 120 * MIN_NS;
/// A row "ran later" when its 2-hour max gain reached this (percent).
pub const RAN_LATER_PCT: f64 = 15.0;

/// `building`: 3-minute volume at least this multiple of its prior-30-minute pace.
pub const BUILD_PACE: f64 = 1.5;
/// `breaking_out`: the new-high bar's volume at least this multiple of the per-slot pace.
pub const BREAK_PACE: f64 = 2.0;
/// `breaking_out` holds this many slots after the breakout bar…
pub const BREAKOUT_HOLD_SLOTS: u32 = 2;
/// …while the price stays within this fraction of the high.
pub const BREAKOUT_HOLD_PCT: f64 = 0.01;
/// `running`: at least this far above the flag price (fraction).
pub const RUN_MIN_PCT: f64 = 0.10;
/// `running`: at least this many new-high bars in the last 12 slots.
pub const RUN_HIGHS: usize = 2;
/// `fading`: this far below the flag price (fraction).
pub const FADE_BELOW_FLAG_PCT: f64 = 0.05;
/// `pullback`: the high since the flag at least this far above the flag price (fraction).
pub const PULLBACK_MOVE_PCT: f64 = 0.03;
/// `pullback`: at least this far off the high (fraction).
pub const PULLBACK_MIN_PCT: f64 = 0.02;
/// "Below VWAP" means below it by more than this fraction (0.1 %).
pub const VWAP_TOL: f64 = 0.001;
/// Consecutive barriers a candidate must hold to become the state.
pub const DEBOUNCE: u8 = 2;
/// `state` log lines per alert per day, at most: a guard against a broken feed, not a budget.
///
/// It was 60, and on 2026-10-22 SHFS and POM reached it by about 12:15 ET. Both stopped logging
/// while SHFS ran from $0.32 to $0.48, and a restart resumes a row from its last logged state,
/// so the state it resumed from was stale too. The state machine already limits churn:
/// transitions happen only at 15 s barriers, and every state except `halted` and `breaking_out`
/// must hold [`DEBOUNCE`] barriers. That day's busiest rows made about 7 transitions an hour,
/// about 120 over the full 16-hour 04:00–20:00 session. 1,000 is about 8 times that, an average
/// of one a minute all session, and bounds a pathological row at about 330 KB of log a day
/// (state lines are about 330 bytes). Each logged line is one real transition: nothing is
/// coalesced away, since a short breakout is exactly what the log is for.
pub const MAX_TRANSITIONS: u32 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MState {
    Watching,
    Building,
    BreakingOut,
    Running,
    Pullback,
    Fading,
    Halted,
}

impl MState {
    pub fn as_str(self) -> &'static str {
        match self {
            MState::Watching => "watching",
            MState::Building => "building",
            MState::BreakingOut => "breaking_out",
            MState::Running => "running",
            MState::Pullback => "pullback",
            MState::Fading => "fading",
            MState::Halted => "halted",
        }
    }
    pub fn parse(s: &str) -> Option<MState> {
        Some(match s {
            "watching" => MState::Watching,
            "building" => MState::Building,
            "breaking_out" => MState::BreakingOut,
            "running" => MState::Running,
            "pullback" => MState::Pullback,
            "fading" => MState::Fading,
            "halted" => MState::Halted,
            _ => return None,
        })
    }
}

/// One completed bar after the flag: `(key, end_ns, close, high, low)`.
pub type NewBar = (u32, i64, f64, f64, f64);

/// What the state machine reads at a barrier (`super::features::Window`, flattened so the
/// machine is testable on synthetic inputs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inputs {
    pub key: u32,
    pub at_ns: i64,
    pub price: f64,
    pub vwap: f64,
    pub bar_traded: bool,
    pub bar_high: f64,
    pub bar_vol: f64,
    pub vol12: f64,
    pub pace_slot: f64,
    pub min_lows: [f64; 3],
    pub close_1m: f64,
    pub halted: bool,
}

/// A transition, for the forward log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    pub at_ns: i64,
    pub from: MState,
    pub to: MState,
    pub price: f64,
    pub high: f64,
    pub vwap: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub(crate) exact: Option<Exact>,
    pub(crate) checkpoint_ns: i64,
    /// The flag: time and price.
    pub at_ns: i64,
    pub p0: f64,
    // Later-move outcomes.
    pub high_2h: f64,
    pub low_2h: f64,
    /// The highest bar high since the flag (`NaN` before the first bar).
    pub high_day: f64,
    pub later: Option<Mark>,
    /// The 2-hour `later` line was written; the final one (20:00) was.
    pub logged_2h: bool,
    pub logged_final: bool,
    /// Restored after a restart: the values may miss what happened while the process was down.
    pub resumed: bool,
    // Momentum.
    pub state: MState,
    pub since_ns: i64,
    candidate: Option<(MState, u8)>,
    /// Keys of the bars that made a new high since the flag, the last 12 slots' only.
    highs: Vec<u32>,
    breakout_key: Option<u32>,
    pub higher_low: f64,
    /// At the last barrier: price, VWAP, and the close a minute before it.
    pub price: f64,
    pub vwap: f64,
    pub close_1m: f64,
    pub transitions: u32,
}

/// `vol` at least `k` times `base`; with no base volume at all, any volume is.
fn pace_ok(vol: f64, base: f64, k: f64) -> bool {
    if base > 0.0 {
        vol >= k * base
    } else {
        vol > 0.0
    }
}

fn up(x: &mut f64, v: f64, max: bool) {
    *x = if x.is_nan() {
        v
    } else if max {
        x.max(v)
    } else {
        x.min(v)
    };
}

impl Track {
    pub fn new(at_ns: i64, p0: f64) -> Track {
        Track {
            exact: None,
            checkpoint_ns: at_ns,
            at_ns,
            p0,
            high_2h: f64::NAN,
            low_2h: f64::NAN,
            high_day: f64::NAN,
            later: None,
            logged_2h: false,
            logged_final: false,
            resumed: false,
            state: MState::Watching,
            since_ns: at_ns,
            candidate: None,
            highs: Vec::new(),
            breakout_key: None,
            higher_low: f64::NAN,
            price: p0,
            vwap: f64::NAN,
            close_1m: f64::NAN,
            transitions: 0,
        }
    }

    /// The high the momentum rules compare against: the highest bar high since the flag, at
    /// least the flag price.
    pub fn high(&self) -> f64 {
        if let Some(e) = &self.exact {
            return e.high_price_micros as f64 / 1_000_000.0;
        }
        if self.high_day.is_nan() {
            self.p0
        } else {
            self.high_day.max(self.p0)
        }
    }

    pub fn pct(&self, x: f64) -> Option<f64> {
        (x.is_finite() && self.p0 > 0.0).then(|| (x / self.p0 - 1.0) * 100.0)
    }

    /// Folds frozen feature bars for later-move outcomes. Exact momentum highs are advanced
    /// separately by `exact_bars`; model floats never establish a strict new-high claim.
    /// `mark` tests a bar for a canonical ROCKET mark while
    /// `rocket_later` is still open.
    pub fn bars(&mut self, bars: &[NewBar], mut mark: impl FnMut(u32, i64) -> Option<Mark>) {
        for &(key, end, _close, high, low) in bars {
            if end <= self.at_ns {
                continue;
            }
            if end <= self.at_ns + LATER_WINDOW_NS {
                up(&mut self.high_2h, high, true);
                up(&mut self.low_2h, low, false);
            }
            up(&mut self.high_day, high, true);
            if self.later.is_none() {
                self.later = mark(key, end);
            }
        }
    }

    /// The candidate state at a barrier (module docs, "Momentum state").
    fn candidate(&self, x: &Inputs) -> MState {
        if x.halted {
            return MState::Halted;
        }
        if self.fresh_breakout(x) {
            return MState::BreakingOut;
        }
        let p = x.price;
        let h = self.high();
        // Within VWAP_TOL counts as at VWAP: the bars are f32, so a flat tape's close and its
        // VWAP differ by rounding alone.
        let above_vwap = x.vwap.is_nan() || p >= x.vwap * (1.0 - VWAP_TOL);
        if let Some(k) = self.breakout_key
            && x.key > k
            && x.key - k <= BREAKOUT_HOLD_SLOTS
            && p >= h * (1.0 - BREAKOUT_HOLD_PCT)
        {
            return MState::BreakingOut;
        }
        let recent_highs = self
            .highs
            .iter()
            .filter(|&&k| k + 12 > x.key && k <= x.key)
            .count();
        if p >= self.p0 * (1.0 + RUN_MIN_PCT) && recent_highs >= RUN_HIGHS && above_vwap {
            return MState::Running;
        }
        if !above_vwap || p <= self.p0 * (1.0 - FADE_BELOW_FLAG_PCT) {
            return MState::Fading;
        }
        if h >= self.p0 * (1.0 + PULLBACK_MOVE_PCT)
            && p <= h * (1.0 - PULLBACK_MIN_PCT)
            && (self.higher_low.is_nan() || p >= self.higher_low)
        {
            return MState::Pullback;
        }
        let [m0, m1, m2] = x.min_lows;
        if m0 > m1 && m1 > m2 && pace_ok(x.vol12, x.pace_slot * 12.0, BUILD_PACE) {
            return MState::Building;
        }
        MState::Watching
    }

    /// This barrier's bar made a new high since the flag on volume ≥ [`BREAK_PACE`]× pace (what
    /// starts a breakout; the hold after it does not restart it).
    pub(crate) fn fresh_breakout(&self, x: &Inputs) -> bool {
        self.exact
            .as_ref()
            .is_some_and(|e| e.complete && e.fresh.is_some_and(|f| f.key == x.key))
            && x.bar_traded
            && self.highs.last() == Some(&x.key)
            && pace_ok(x.bar_vol, x.pace_slot, BREAK_PACE)
    }

    /// Advances the state at a barrier (after [`Track::bars`]). Returns the transition, if any.
    pub fn step(&mut self, x: &Inputs) -> Option<Transition> {
        if x.price.is_finite() {
            self.price = x.price;
        }
        self.vwap = x.vwap;
        self.close_1m = x.close_1m;
        self.highs.retain(|&k| k + 12 > x.key);
        let c = self.candidate(x);
        // The last higher low: a rolling minute whose low is above the minute before it.
        let [m0, m1, _] = x.min_lows;
        if m0 > m1 {
            self.higher_low = m0;
        }
        if c == MState::BreakingOut && self.fresh_breakout(x) {
            self.breakout_key = Some(x.key);
        }
        let immediate =
            matches!(c, MState::Halted | MState::BreakingOut) || self.state == MState::Halted;
        let to = if c == self.state {
            self.candidate = None;
            return None;
        } else if immediate {
            c
        } else {
            let n = match self.candidate {
                Some((s, n)) if s == c => n + 1,
                _ => 1,
            };
            self.candidate = Some((c, n));
            if n < DEBOUNCE {
                return None;
            }
            c
        };
        self.candidate = None;
        let t = Transition {
            at_ns: x.at_ns,
            from: self.state,
            to,
            price: x.price,
            high: self.high(),
            vwap: x.vwap,
        };
        self.state = to;
        self.since_ns = x.at_ns;
        Some(t)
    }

    /// The state to show now: the barrier state, with a live halt and a provisional breakout
    /// (the live price above the high, prints of the bar still forming). `(state, provisional)`.
    pub fn shown(&self, last: Option<f64>, halted_now: bool) -> (MState, bool) {
        if halted_now {
            return (MState::Halted, self.state != MState::Halted);
        }
        if let Some(p) = last
            && self.exact.as_ref().is_some_and(|e| {
                e.complete
                    && e.live.is_some_and(|(key, source)| {
                        key > e.checked_key
                            && source.micros > e.high_price_micros
                            && source.micros as f64 / 1_000_000.0 == p
                    })
            })
            && !matches!(
                self.state,
                MState::BreakingOut | MState::Halted | MState::Fading
            )
        {
            return (MState::BreakingOut, true);
        }
        (self.state, false)
    }

    pub(crate) fn with_exact(
        mut self,
        price: Option<i64>,
        known: i64,
        admission: u64,
        checked: u32,
        ahead: bool,
    ) -> Self {
        self.exact = price
            .filter(|p| *p > 0)
            .map(|p| Exact::new(known, admission, checked, p, ahead));
        self.checkpoint_ns = known;
        self
    }

    pub(crate) fn exact_bars(&mut self, key: u32, evaluated: i64) {
        if let Some(e) = &mut self.exact {
            self.highs.extend(e.fold(key, evaluated));
        }
    }

    pub(crate) fn clock_boundary(&mut self, end: i64, evaluated: i64) {
        if let Some(e) = &mut self.exact {
            let pending =
                e.unknown_reason.as_deref() == Some("future_barrier_ahead_of_decision_clock");
            if end > evaluated && (e.complete || pending) {
                e.complete = false;
                e.unknown_reason = Some("future_barrier_ahead_of_decision_clock".into());
            } else if end <= evaluated && pending {
                // Every source admission was still accumulated while the source clock was
                // ahead. Catch-up folds its floor without announcing an older bar anew.
                e.complete = true;
                e.unknown_reason = None;
            }
        }
    }

    /// Only an explicitly complete history adoption can close a restart gap. Full bars after
    /// the creation bar can be reconciled from original integer highs. A missing creation-bar
    /// suffix or an older-than-ring gap remains unknown rather than inheriting a preflag peak.
    pub(crate) fn reconcile(
        &mut self,
        s: &super::features::Sym,
        key: u32,
        evaluated: i64,
        t0: i64,
        ord: u32,
    ) {
        let Some(e) = &mut self.exact else {
            return;
        };
        if e.complete || !e.restored || !e.rebuild_ready || s.unrestored || s.unavailable {
            return;
        }
        let base = super::features::key_of(ord, 0);
        let end = t0 + (i64::from(key.saturating_sub(base)) + 1) * super::features::BAR_NS;
        if end > evaluated {
            e.unknown_reason = Some("history_barrier_ahead_of_decision_clock".into());
            return;
        }
        let creation_key = super::features::key_of(
            ord,
            (e.flag_known_ns - t0)
                .div_euclid(super::features::BAR_NS)
                .clamp(0, super::features::NSLOT - 1),
        );
        let reason = if e.creation_had_ahead {
            Some("precreation_future_admissions_unreconciled")
        } else if e.checked_key < creation_key {
            Some("creation_bar_suffix_unavailable")
        } else if key.saturating_sub(e.checked_key) >= super::features::RING as u32 {
            Some("restart_gap_exceeds_source_ring")
        } else if !super::features::retains_exact_after(s, e.checked_key) {
            Some("source_ring_advanced_past_checkpoint")
        } else {
            None
        };
        if let Some(reason) = reason {
            e.unknown_reason = Some(reason.into());
            return;
        }
        for (k, high) in super::features::exact_highs(s, e.checked_key, key) {
            if high > e.high_price_micros {
                e.high_price_micros = high;
                e.reference_known_ns = evaluated;
                e.reference_bar_key = Some(k);
                self.highs.push(k);
            }
        }
        e.pending.retain(|k, _| *k > key);
        e.checked_key = e.checked_key.max(key);
        e.complete = true;
        e.unknown_reason = None;
        e.fresh = None;
    }

    pub(crate) fn snapshot(&self) -> Value {
        json!({
            "v": 1, "policy_id": breakout::POLICY_ID, "exact": self.exact,
            "checkpoint_ns": self.checkpoint_ns,
            "highs": self.highs, "breakout_key": self.breakout_key,
            "candidate": self.candidate.map(|(s,n)| (s.as_str(), n)),
            "state": self.state.as_str(), "since_ns": self.since_ns,
            "higher_low": self.higher_low.is_finite().then_some(self.higher_low),
        })
    }

    /// Restored source state is a checkpoint, not proof that the downtime was observed.
    pub(crate) fn restore_exact(&mut self, v: &Value) -> Option<u64> {
        if v["v"] != 1 || v["policy_id"] != breakout::POLICY_ID {
            return None;
        }
        let mut e: Exact = serde_json::from_value(v.get("exact")?.clone()).ok()?;
        if e.flag_price_micros <= 0
            || e.high_price_micros < e.flag_price_micros
            || e.admission_cutoff < e.creation_admission
            || e.reference_known_ns < e.flag_known_ns
            || e.pending.len() > super::features::RING
            || e.pending.values().any(|p| {
                p.micros <= 0
                    || p.exec_ns <= e.flag_known_ns
                    || p.admission <= e.creation_admission
                    || p.admission > e.admission_cutoff
            })
        {
            return None;
        }
        let admission = e.admission_cutoff;
        e.complete = false;
        e.restored = true;
        e.rebuild_ready = false;
        e.fresh = None;
        e.live = None;
        e.unknown_reason = Some("awaiting_authoritative_history".into());
        self.highs = v["highs"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_u64().and_then(|k| u32::try_from(k).ok()))
                    .collect()
            })
            .unwrap_or_default();
        self.breakout_key = v["breakout_key"]
            .as_u64()
            .and_then(|k| u32::try_from(k).ok());
        if v["since_ns"]
            .as_i64()
            .is_some_and(|since| since >= self.since_ns)
        {
            self.candidate = v["candidate"].as_array().and_then(|a| {
                Some((
                    MState::parse(a.first()?.as_str()?)?,
                    u8::try_from(a.get(1)?.as_u64()?).ok()?,
                ))
            });
            if let Some(state) = v["state"].as_str().and_then(MState::parse) {
                self.state = state;
            }
            if let Some(since) = v["since_ns"].as_i64() {
                self.since_ns = since;
            }
        }
        self.higher_low = v["higher_low"].as_f64().unwrap_or(f64::NAN);
        self.exact = Some(e);
        Some(admission)
    }

    pub(crate) fn tracking_status(&self) -> Value {
        let complete = self.exact.as_ref().is_some_and(|e| e.complete);
        json!({"status": if complete {
            if self.exact.as_ref().is_some_and(|e| e.restored) { "restored" } else { "complete" }
        } else { "unknown" },
        "reason": self.exact.as_ref().and_then(|e| e.unknown_reason.as_deref()).or((self.exact.is_none()).then_some("legacy_exact_state_unavailable")),
        "policy_id": breakout::POLICY_ID})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 15_000_000_000;

    /// A synthetic barrier `k` slots after a flag at key 100 / time 0.
    fn inputs(k: u32, price: f64, vwap: f64) -> Inputs {
        Inputs {
            key: 100 + k,
            at_ns: i64::from(k) * S,
            price,
            vwap,
            bar_traded: true,
            bar_high: price,
            bar_vol: 100.0,
            vol12: 1200.0,
            pace_slot: 100.0,
            min_lows: [price, price, price],
            close_1m: price,
            halted: false,
        }
    }

    fn bar(k: u32, close: f64, high: f64, low: f64) -> NewBar {
        (100 + k, i64::from(k) * S, close, high, low)
    }

    /// One barrier with its bar.
    fn go(t: &mut Track, x: Inputs, low: f64) -> Option<Transition> {
        if t.exact.is_none() {
            *t = t.clone().with_exact(Some(1_000_000), 0, 0, 100, false);
        }
        t.exact.as_mut().unwrap().observe(
            x.key,
            breakout::Print {
                micros: breakout::original_micros(x.bar_high).unwrap(),
                exec_ns: x.at_ns - S + 1,
                receive_ns: None,
                admission: u64::from(x.key),
            },
        );
        t.exact_bars(x.key, x.at_ns);
        t.bars(&[bar(x.key - 100, x.price, x.bar_high, low)], |_, _| None);
        t.step(&x)
    }

    #[test]
    fn quiet_stays_watching_then_building_after_the_debounce() {
        let mut t = Track::new(0, 1.0);
        assert_eq!(go(&mut t, inputs(1, 1.0, 0.99), 0.995), None);
        assert_eq!(t.state, MState::Watching);
        // Higher lows over three rolling minutes on 2× pace volume.
        let mut x = inputs(2, 1.0, 0.99);
        x.min_lows = [0.99, 0.98, 0.97];
        x.vol12 = 2400.0;
        assert_eq!(go(&mut t, x, 0.99), None, "one barrier is not enough");
        x.key += 1;
        x.at_ns += S;
        let tr = go(&mut t, x, 0.99).expect("building");
        assert_eq!((tr.from, tr.to), (MState::Watching, MState::Building));
        assert_eq!(t.since_ns, 3 * S);
        // Below VWAP: fading (debounced).
        let x = inputs(4, 0.98, 0.99);
        assert_eq!(go(&mut t, x, 0.98), None);
        assert_eq!(
            go(&mut t, inputs(5, 0.98, 0.99), 0.98).unwrap().to,
            MState::Fading
        );
    }

    #[test]
    fn breakout_is_immediate_running_needs_highs_and_ten_percent() {
        let mut t = Track::new(0, 1.0);
        let mut x = inputs(1, 1.05, 1.0);
        x.bar_high = 1.05;
        x.bar_vol = 250.0;
        let tr = go(&mut t, x, 1.0).expect("breakout at once");
        assert_eq!(tr.to, MState::BreakingOut);
        // A new high on thin volume is not a breakout; the hold keeps it while near the high.
        let mut x = inputs(2, 1.049, 1.0);
        x.bar_high = 1.049;
        assert_eq!(go(&mut t, x, 1.04), None);
        assert_eq!(t.state, MState::BreakingOut);
        // Successive new highs, +12 %: running (after the debounce).
        for (k, p) in [(3u32, 1.08), (4, 1.12), (5, 1.13)] {
            let mut x = inputs(k, p, 1.0);
            x.bar_high = p;
            x.bar_vol = 150.0;
            go(&mut t, x, p - 0.01);
        }
        assert_eq!(t.state, MState::Running);
        assert!((t.pct(t.high()).unwrap() - 13.0).abs() < 1e-9);
    }

    #[test]
    fn pullback_then_breakout_then_halt() {
        let mut t = Track::new(0, 1.0);
        let mut x = inputs(1, 1.10, 1.0);
        x.bar_high = 1.10;
        x.bar_vol = 300.0;
        go(&mut t, x, 1.05);
        assert_eq!(t.state, MState::BreakingOut);
        // Off the high by 4.5 %, above VWAP, a higher low held: pullback after the debounce.
        for k in 2..=5 {
            let mut x = inputs(k, 1.05, 1.0);
            x.min_lows = [1.04, 1.03, 1.03];
            go(&mut t, x, 1.04);
        }
        assert_eq!(t.state, MState::Pullback);
        assert!(t.higher_low >= 1.04 - 1e-12);
        // A new high on volume: breaking out again, at once.
        let mut x = inputs(6, 1.12, 1.0);
        x.bar_high = 1.12;
        x.bar_vol = 400.0;
        assert_eq!(go(&mut t, x, 1.08).unwrap().to, MState::BreakingOut);
        // A halt wins at once, and leaving it is immediate too.
        let mut x = inputs(7, 1.12, 1.0);
        x.halted = true;
        assert_eq!(t.step(&x).unwrap().to, MState::Halted);
        let x = inputs(8, 1.12, 1.0);
        assert_ne!(t.step(&x).unwrap().to, MState::Halted);
    }

    #[test]
    fn losing_the_higher_low_is_not_a_pullback_and_far_below_the_flag_fades() {
        let mut t = Track::new(0, 1.0);
        let mut x = inputs(1, 1.10, 0.9);
        x.bar_high = 1.10;
        x.bar_vol = 300.0;
        go(&mut t, x, 1.05);
        t.higher_low = 1.06;
        for k in 2..=3 {
            go(&mut t, inputs(k, 1.04, 0.9), 1.04);
        }
        assert_eq!(t.state, MState::Watching, "below the last higher low");
        for k in 4..=5 {
            go(&mut t, inputs(k, 0.94, 0.9), 0.94);
        }
        assert_eq!(t.state, MState::Fading, "−6 % from the flag, above VWAP");
    }

    #[test]
    fn later_outcomes_window_and_mark() {
        let mut t = Track::new(0, 1.0);
        let m = Mark {
            start_ns: 100 * S,
            mark_ns: 110 * S,
            start_price: 1.0,
        };
        t.bars(
            &[
                bar(10, 1.1, 1.12, 0.95),
                (600, 480 * S, 1.4, 1.45, 1.3),
                (700, 1_000 * S, 1.6, 1.7, 1.5),
            ],
            |k, _| (k == 600).then_some(m),
        );
        assert!(
            (t.pct(t.high_2h).unwrap() - 45.0).abs() < 1e-9,
            "inside 2 h"
        );
        assert!((t.pct(t.low_2h).unwrap() + 5.0).abs() < 1e-9);
        assert!(
            (t.pct(t.high_day).unwrap() - 70.0).abs() < 1e-9,
            "the session"
        );
        assert_eq!(t.later, Some(m));
    }

    #[test]
    fn shown_state_is_provisional_above_the_high_and_halts_live() {
        let mut t = Track::new(0, 1.0).with_exact(Some(1_000_000), 0, 0, 100, false);
        t.exact.as_mut().unwrap().high_price_micros = 1_100_000;
        t.exact.as_mut().unwrap().observe(
            101,
            breakout::Print {
                micros: 1_110_000,
                exec_ns: 1,
                receive_ns: None,
                admission: 1,
            },
        );
        t.high_day = 1.10;
        assert_eq!(t.shown(Some(1.11), false), (MState::BreakingOut, true));
        assert_eq!(t.shown(Some(1.09), false), (MState::Watching, false));
        assert_eq!(t.shown(Some(1.09), true), (MState::Halted, true));
        t.state = MState::Fading;
        assert_eq!(t.shown(Some(1.2), false), (MState::Fading, false));
        assert_eq!(MState::parse("breaking_out"), Some(MState::BreakingOut));
        assert_eq!(
            MState::parse(MState::Pullback.as_str()),
            Some(MState::Pullback)
        );
    }

    #[test]
    fn legacy_unknown_does_not_invent_a_source_high_or_provisional_claim() {
        let mut t = Track::new(0, f64::from(1.83f32));
        t.high_day = 1.84;
        assert_eq!(t.shown(Some(2.0), false), (MState::Watching, false));
        assert_eq!(t.tracking_status()["status"], "unknown");
        let mut x = inputs(1, 2.0, 1.0);
        x.bar_vol = 1_000.0;
        t.bars(&[bar(1, 2.0, 2.0, 1.8)], |_, _| None);
        assert!(!t.fresh_breakout(&x));
    }

    #[test]
    fn provisional_quotes_require_the_latest_eligible_source_admission_and_execution() {
        let mut t = Track::new(0, 1.0).with_exact(Some(1_000_000), 10, 7, 100, true);
        assert_eq!(t.shown(Some(1.2), false), (MState::Watching, false));
        for (exec_ns, admission) in [(11, 7), (9, 8), (10, 9)] {
            t.exact.as_mut().unwrap().observe(
                101,
                breakout::Print {
                    micros: 1_200_000,
                    exec_ns,
                    receive_ns: None,
                    admission,
                },
            );
            assert_eq!(t.shown(Some(1.2), false), (MState::Watching, false));
        }
        t.exact.as_mut().unwrap().observe(
            101,
            breakout::Print {
                micros: 1_200_000,
                exec_ns: 11,
                receive_ns: None,
                admission: 10,
            },
        );
        assert_eq!(t.shown(Some(1.2), false), (MState::BreakingOut, true));
        assert_eq!(
            t.shown(Some(1.3), false),
            (MState::Watching, false),
            "a different raw quote is not evidence"
        );
        assert_eq!(t.shown(Some(1.2), true), (MState::Halted, true));
        t.exact_bars(101, S);
        assert_eq!(t.shown(Some(1.2), false), (MState::Watching, false));
        t.exact.as_mut().unwrap().observe(
            101,
            breakout::Print {
                micros: 1_300_000,
                exec_ns: 12,
                receive_ns: None,
                admission: 11,
            },
        );
        assert_eq!(
            t.shown(Some(1.3), false),
            (MState::Watching, false),
            "late source bar is no longer provisional"
        );
    }

    #[test]
    fn future_source_clock_withholds_notice_and_catchup_preserves_observed_floor() {
        let mut t = Track::new(0, 1.0).with_exact(Some(1_000_000), 0, 7, 100, false);
        t.exact.as_mut().unwrap().observe(
            101,
            breakout::Print {
                micros: 1_100_000,
                exec_ns: 1,
                receive_ns: None,
                admission: 8,
            },
        );
        t.clock_boundary(S, S - 1);
        t.exact_bars(101, S - 1);
        assert_eq!(t.tracking_status()["status"], "unknown");
        assert!(!t.fresh_breakout(&inputs(1, 1.1, 1.0)));
        t.exact.as_mut().unwrap().observe(
            101,
            breakout::Print {
                micros: 1_200_000,
                exec_ns: 2,
                receive_ns: None,
                admission: 9,
            },
        );
        t.clock_boundary(2 * S, 2 * S);
        t.exact_bars(102, 2 * S);
        assert_eq!(t.high(), 1.2);
        assert_eq!(t.tracking_status()["status"], "complete");
        assert!(
            !t.fresh_breakout(&inputs(2, 1.0, 1.0)),
            "old skewed candle never reannounces"
        );
    }

    #[test]
    fn source_checkpoints_need_authoritative_full_gap_before_future_strict_claims() {
        use crate::quotes::Counted;
        use market_events::{Price, Trade, UnixNanos};
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let mut tape = super::super::features::Tape::default();
        let (t0, ord) = tape.day(date);
        let base = super::super::features::key_of(ord, 0);
        let known = t0 + 1_000_000_000;
        let mut initial =
            Track::new(known, 16.0).with_exact(Some(16_000_000), known, 7, base - 1, false);
        initial.exact.as_mut().unwrap().observe(
            base,
            breakout::Print {
                micros: 16_000_001,
                exec_ns: known + 1,
                receive_ns: None,
                admission: 8,
            },
        );
        initial.exact_bars(base, t0 + S);
        let mut restored = Track::new(known, 16.0);
        assert_eq!(restored.restore_exact(&initial.snapshot()), Some(8));
        assert_eq!(restored.high(), 16.000001);
        assert_eq!(restored.tracking_status()["status"], "unknown");
        let trade = Trade::new(
            "X",
            UnixNanos(t0 + S + 1),
            Price::from_micros(16_000_002),
            100,
            1,
        );
        tape.apply_trade(
            &trade,
            Counted {
                ohlc: true,
                volume: true,
            },
        );
        let s = tape.sym("X").unwrap();
        restored.reconcile(s, base + 1, t0 + 2 * S, t0, ord);
        assert_eq!(
            restored.tracking_status()["status"],
            "unknown",
            "ordinary prints are not a complete history adoption"
        );
        restored.exact.as_mut().unwrap().rebuild_ready = true;
        restored.reconcile(s, base + 1, t0 + 2 * S, t0, ord);
        assert_eq!(restored.tracking_status()["status"], "restored");
        assert_eq!(restored.high(), 16.000002);
        assert!(!restored.fresh_breakout(&inputs(2, 16.000002, 16.0)));
        restored.exact.as_mut().unwrap().observe(
            base + 2,
            breakout::Print {
                micros: 16_000_003,
                exec_ns: t0 + 2 * S + 1,
                receive_ns: None,
                admission: 9,
            },
        );
        restored.exact_bars(base + 2, t0 + 3 * S);
        assert_eq!(
            restored.exact.as_ref().unwrap().fresh.unwrap().reference,
            16_000_002
        );
    }

    #[test]
    fn unavailable_creation_suffix_ahead_admissions_and_long_gaps_remain_unknown() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let mut tape = super::super::features::Tape::default();
        let (t0, ord) = tape.day(date);
        let base = super::super::features::key_of(ord, 0);
        tape.apply_trade(
            &market_events::Trade::new(
                "X",
                market_events::UnixNanos(t0 + S + 1),
                market_events::Price::from_micros(2_000_000),
                100,
                1,
            ),
            crate::quotes::Counted {
                ohlc: true,
                volume: true,
            },
        );
        for (ahead, checked, key, reason) in [
            (false, base - 1, base + 1, "creation_bar_suffix_unavailable"),
            (
                true,
                base,
                base + 1,
                "precreation_future_admissions_unreconciled",
            ),
            (
                false,
                base,
                base + super::super::features::RING as u32,
                "restart_gap_exceeds_source_ring",
            ),
        ] {
            let original =
                Track::new(t0 + 1, 1.0).with_exact(Some(1_000_000), t0 + 1, 7, base - 1, ahead);
            let mut restored = Track::new(t0 + 1, 1.0);
            restored.restore_exact(&original.snapshot()).unwrap();
            let e = restored.exact.as_mut().unwrap();
            e.checked_key = checked;
            e.rebuild_ready = true;
            let evaluated = t0 + i64::from(key - base + 1) * S;
            restored.reconcile(tape.sym("X").unwrap(), key, evaluated, t0, ord);
            assert_eq!(restored.tracking_status()["status"], "unknown");
            assert_eq!(restored.tracking_status()["reason"], reason);
        }
    }

    #[test]
    fn a_future_source_head_cannot_launder_an_overwritten_restart_gap() {
        use crate::quotes::Counted;
        use market_events::{Price, Trade, UnixNanos};
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let mut tape = super::super::features::Tape::default();
        let (t0, ord) = tape.day(date);
        let base = super::super::features::key_of(ord, 0);
        let known = t0 + 1;
        let mut original =
            Track::new(known, 1.0).with_exact(Some(1_000_000), known, 7, base - 1, false);
        original.exact_bars(base, t0 + S);
        let mut restored = Track::new(known, 1.0);
        restored.restore_exact(&original.snapshot()).unwrap();
        restored.exact.as_mut().unwrap().rebuild_ready = true;
        let both = Counted {
            ohlc: true,
            volume: true,
        };
        tape.apply_trade(
            &Trade::new(
                "X",
                UnixNanos(t0 + S + 1),
                Price::from_micros(2_000_000),
                100,
                1,
            ),
            both,
        );
        // The same ring index, 512 source slots later, overwrites the unseen $2 high.
        tape.apply_trade(
            &Trade::new(
                "X",
                UnixNanos(t0 + 513 * S + 1),
                Price::from_micros(1_100_000),
                100,
                2,
            ),
            both,
        );
        for key in [base + 1, base + 2, base + 3] {
            let end = t0 + i64::from(key - base + 1) * S;
            // Capped feature barriers can be ahead of the actual caller clock.
            restored.reconcile(tape.sym("X").unwrap(), key, end - 1, t0, ord);
            assert!(!restored.exact.as_ref().unwrap().complete);
            assert_eq!(restored.exact.as_ref().unwrap().checked_key, base);
            // Actual clock catchup still cannot turn missing bars into a complete prefix.
            restored.reconcile(tape.sym("X").unwrap(), key, end, t0, ord);
            assert_eq!(
                restored.tracking_status()["reason"],
                "source_ring_advanced_past_checkpoint"
            );
            assert_eq!(restored.exact.as_ref().unwrap().checked_key, base);
        }
    }
}
