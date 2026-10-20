//! EXPERIMENTAL early-setup cues for Ross Panel members (`policy/setups-v0.json`, owner decision
//! 2026-10-20). One family so far, `bull_flag_v0`: an impulse (pole), a tight pullback (flag), and
//! a breakout over the flag's high. Design and every choice marked as ours:
//! `docs/stock-context/setups-v0.md`.
//!
//! The numbers in the policy are development starting values, not researched or validated ones.
//! Nothing here is a trading signal; it tells the UI where a member stands relative to levels the
//! engine can name, so it can show cues before confirmation.
//!
//! # Bars
//!
//! The base timeframe is the engine's own grid of completed 15 s bars, built from the same
//! admitted prints the pillars use (OHLC from OHLC-counted prints, volume from volume-counted
//! prints) and closed at the same barriers, whether or not any chart is open and whatever
//! timeframe a user views. A print that executed in an earlier bar but was admitted after that
//! bar's barrier (late) never edits a completed bar: it is left out of the setup bars.
//!
//! # Causality
//!
//! At barrier `b` the bar ending at `b` is the bar under test. The trigger level is the highest
//! high and the invalidation level the lowest low of the flag bars completed BEFORE it, so a bar
//! can never confirm against a level it set itself. After the test the bar joins the flag, and any
//! level it moves gets a new version id. A triggered or invalidated outcome keeps the level
//! version it crossed forever; transitions and level versions are append-only log entries.
//!
//! Every comparison is exact integer arithmetic on micro-dollars and shares (thresholds in bp and
//! thousandths); the displayed bp / milli values are truncated copies that classify nothing.

use crate::pillar::Tri;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;

pub const SETUPS_V0: &str = include_str!("../policy/setups-v0.json");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct BullFlagRules {
    pub id: String,
    /// Must equal the pillar policy's grid: the setup reads the engine's own bars.
    pub bar_secs: i64,
    /// The pole must lie within this many bar slots ending at the barrier (20 = 5 min).
    pub impulse_lookback_bars: i64,
    /// Swing low to swing high, at least this rise (300 = 3 %).
    pub impulse_min_bp: i64,
    /// Pole volume per bar slot at least this multiple (thousandths) of the symbol's average
    /// volume per bar slot so far today.
    pub impulse_vol_mult_milli: u64,
    pub flag_min_bars: u32,
    pub flag_max_bars: u32,
    /// Flag lows stay above `H − max_retrace × (H − L)` (thousandths of the pole height).
    pub max_retrace_milli: i64,
    /// `near` when the price is at most this far below the trigger.
    pub near_bp: i64,
    /// A breakout bar's volume at least this multiple of the average flag-bar volume.
    pub confirm_vol_mult_milli: u64,
    /// Extended when the price is above the trigger by more than this.
    pub extended_bp: i64,
    /// `triggered` / `invalidated` stay visible this long, then the family re-arms.
    pub display_hold_secs: i64,
    /// No OHLC print for this long: the setup is dropped (never kept on invented levels).
    pub max_quiet_secs: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct SetupPolicyBody {
    pub schema: u32,
    pub id: String,
    pub experimental: bool,
    pub note: String,
    pub bull_flag: BullFlagRules,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupPolicy {
    pub body: SetupPolicyBody,
    pub hash: String,
}

impl SetupPolicy {
    pub fn parse(text: &str) -> Result<SetupPolicy, String> {
        let body: SetupPolicyBody =
            serde_json::from_str(text).map_err(|e| format!("setup policy: {e}"))?;
        let r = &body.bull_flag;
        if body.schema != 1
            || r.bar_secs <= 0
            || r.impulse_lookback_bars <= 0
            || r.impulse_min_bp <= 0
            || r.flag_min_bars == 0
            || r.flag_max_bars < r.flag_min_bars
            || !(0..=1000).contains(&r.max_retrace_milli)
            || !(0..10_000).contains(&r.near_bp)
            || r.extended_bp < 0
            || r.display_hold_secs < 0
            || r.max_quiet_secs <= 0
        {
            return Err("setup policy: unsupported schema or invalid bull_flag rules".into());
        }
        let hash = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(SetupPolicy { body, hash })
    }

    pub fn v0() -> SetupPolicy {
        SetupPolicy::parse(SETUPS_V0).expect("embedded setups-v0.json parses")
    }

    pub fn tag(&self) -> String {
        format!("{}@{}", self.body.id, &self.hash[..12])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ohlc {
    pub open: i64,
    pub high: i64,
    pub low: i64,
    pub close: i64,
    open_exec: i64,
    close_exec: i64,
}

/// One completed bar `[end − bar, end)`. `ohlc` is `None` when only volume-counted prints fell in
/// it. Bars with no print at all are not stored (no trade, no bar: never interpolated).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    pub end: i64,
    pub ohlc: Option<Ohlc>,
    pub volume: u64,
}

impl Bar {
    pub fn new(end: i64) -> Bar {
        Bar {
            end,
            ohlc: None,
            volume: 0,
        }
    }

    /// Adds one admitted print. Open and close follow execution time (ties: the later admitted).
    pub fn add(&mut self, exec: i64, price: i64, size: u64, ohlc: bool, volume: bool) {
        if volume {
            self.volume += size;
        }
        if !ohlc {
            return;
        }
        match &mut self.ohlc {
            None => {
                self.ohlc = Some(Ohlc {
                    open: price,
                    high: price,
                    low: price,
                    close: price,
                    open_exec: exec,
                    close_exec: exec,
                })
            }
            Some(o) => {
                o.high = o.high.max(price);
                o.low = o.low.min(price);
                if exec < o.open_exec {
                    o.open = price;
                    o.open_exec = exec;
                }
                if exec >= o.close_exec {
                    o.close = price;
                    o.close_exec = exec;
                }
            }
        }
    }
}

/// Completed bars kept per symbol: the impulse lookback and the 20-close sparkline both fit.
pub const BARS_KEPT: usize = 64;

/// The closes of the last `n` completed bars that had an OHLC print, oldest first.
pub fn spark(bars: &VecDeque<Bar>, n: usize) -> Vec<i64> {
    let mut v: Vec<i64> = bars
        .iter()
        .rev()
        .filter_map(|b| b.ohlc.map(|o| o.close))
        .take(n)
        .collect();
    v.reverse();
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupState {
    /// No setup (or the data cannot support one: see `reason`).
    Watching,
    /// A valid flag, price below the trigger by more than `near`.
    Forming,
    /// Price within `near` below the trigger.
    Near,
    /// The bar under test traded above the trigger (or the price is above it) but the
    /// confirmation is incomplete: see `pending`.
    Crossing,
    /// A completed bar closed above the trigger with confirming volume.
    Triggered,
    Invalidated,
    /// The symbol is halted: the setup is paused, nothing advances.
    Halted,
}

/// One version of a level. A revision is a new version, never an edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Level {
    pub price: i64,
    /// `<setup id>:trigger:<n>` / `<setup id>:invalidation:<n>`.
    pub version: String,
    /// The cutoff the version was set at.
    pub set_at: i64,
    /// `flag_formed`, `flag_high_raised`, `flag_low_lowered`.
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Pole {
    pub low: i64,
    pub low_bar_end: i64,
    pub high: i64,
    pub high_bar_end: i64,
}

/// A confirmation condition with its three-valued state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Pending {
    /// `close_above_trigger` or `volume`.
    pub condition: &'static str,
    pub state: Tri,
}

/// A triggered or invalidated outcome, frozen with the level version it crossed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Outcome {
    /// `triggered` or `invalidated`.
    pub state: SetupState,
    pub reason: &'static str,
    /// The cutoff it was decided at.
    pub at: i64,
    /// End of the bar that decided it.
    pub bar_end: i64,
    /// The level version crossed (the trigger, or the invalidation level).
    pub level: Level,
    /// The deciding bar's close (or the latest price).
    pub price: i64,
    pub bar_volume: Option<u64>,
}

/// What the UI shows for one member's setup at one barrier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SetupView {
    /// `bull_flag_v0`.
    pub family: String,
    /// `setups-v0@<hash>`.
    pub policy: String,
    pub experimental: bool,
    pub state: SetupState,
    /// The cutoff of the transition into `state`.
    pub state_since: i64,
    pub reason: &'static str,
    /// The transition that entered `state`; `None` before the first one.
    pub transition_id: Option<String>,
    /// `<run>:<date>:<symbol>:bull_flag_v0:<n>` while a flag is tracked.
    pub setup_id: Option<String>,
    pub pole: Option<Pole>,
    pub trigger: Option<Level>,
    pub invalidation: Option<Level>,
    /// Completed flag bars (bars with an OHLC print after the pole high).
    pub flag_bars: u32,
    /// The latest admitted price the state was judged on.
    pub price: Option<i64>,
    /// `(trigger − price) / price`, bp, truncated: positive below the trigger.
    pub distance_to_trigger_bp: Option<i64>,
    /// `(price − invalidation) / (trigger − invalidation)` clamped to `[0, 1000]` thousandths.
    pub meter_milli: Option<u32>,
    /// `(price − invalidation) / price`, bp, truncated.
    pub risk_to_stop_bp: Option<i64>,
    /// Confirmation conditions while `crossing` (empty otherwise).
    pub pending: Vec<Pending>,
    /// The trigger version `pending` refers to.
    pub crossed_version: Option<String>,
    /// Price above the trigger by more than `extended_bp`.
    pub extended: bool,
    /// The latest triggered / invalidated outcome while it is displayed.
    pub outcome: Option<Outcome>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SetupTransition {
    /// `<run>:<date>:<symbol>:bull_flag_v0:t<n>`.
    pub id: String,
    pub symbol: String,
    pub setup_id: Option<String>,
    /// `None` for the first transition of a tracker (entering `watching`).
    pub from: Option<SetupState>,
    pub to: SetupState,
    pub reason: &'static str,
    pub at: i64,
    pub bar_end: i64,
    pub trigger_version: Option<String>,
    pub invalidation_version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LevelRecord {
    pub symbol: String,
    pub setup_id: String,
    /// `trigger` or `invalidation`.
    pub kind: &'static str,
    pub level: Level,
}

/// The append-only setup log: every transition and every level version, in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SetupLogEntry {
    Transition(SetupTransition),
    Level(LevelRecord),
}

struct Armed {
    id: String,
    pole: Pole,
    trigger: Level,
    invalidation: Level,
    trigger_n: u32,
    invalidation_n: u32,
    flag_bars: u32,
    flag_volume: u64,
    /// The last bar folded into the flag.
    last_bar_end: i64,
}

/// One member's `bull_flag_v0` state machine. Counters run per symbol per trading date, so ids
/// never repeat within a day even when a member leaves and re-enters.
#[derive(Default)]
pub struct BullFlag {
    state: Option<SetupState>,
    since: i64,
    reason: &'static str,
    transition_id: Option<String>,
    armed: Option<Armed>,
    outcome: Option<Outcome>,
    /// A pole must start after this bar end (after a re-arm the old pole is spent).
    search_from: i64,
    setups: u32,
    transitions: u32,
}

/// What one barrier gives the tracker.
pub struct Step<'a> {
    pub bar_end: i64,
    pub cutoff: i64,
    pub bar_ns: i64,
    pub bars: &'a VecDeque<Bar>,
    /// Latest OHLC print `(execution, price)` executed before the bar end, admitted by the cutoff.
    pub last_price: Option<(i64, i64)>,
    pub halted: bool,
    /// Volume-counted shares today and bar slots since the first counted print (inclusive).
    pub day_volume: u64,
    pub day_slots: u64,
    pub symbol: &'a str,
    /// `<run>:<date>:<symbol>`.
    pub prefix: &'a str,
}

fn bp(num: i64, den: i64) -> i64 {
    ((num as i128 * 10_000) / den as i128) as i64
}

impl BullFlag {
    /// A member left the panel: forget the setup (counters stay, so ids stay unique today).
    pub fn leave(&mut self) {
        let (s, t) = (self.setups, self.transitions);
        *self = BullFlag {
            setups: s,
            transitions: t,
            ..BullFlag::default()
        };
    }

    pub fn is_idle(&self) -> bool {
        self.state.is_none()
    }

    fn level(id: &str, kind: &str, n: u32, price: i64, at: i64, reason: &'static str) -> Level {
        Level {
            price,
            version: format!("{id}:{kind}:{n}"),
            set_at: at,
            reason,
        }
    }

    fn log_level(log: &mut Vec<SetupLogEntry>, sym: &str, id: &str, kind: &'static str, l: &Level) {
        log.push(SetupLogEntry::Level(LevelRecord {
            symbol: sym.to_string(),
            setup_id: id.to_string(),
            kind,
            level: l.clone(),
        }));
    }

    /// Advances one barrier and returns the view.
    pub fn step(
        &mut self,
        policy: &SetupPolicy,
        grid_bar_secs: i64,
        x: &Step,
        log: &mut Vec<SetupLogEntry>,
    ) -> SetupView {
        let r = &policy.body.bull_flag;
        let (state, reason, pending, crossed, pole) = self.decide(r, grid_bar_secs, x, log);
        if self.state != Some(state) {
            self.transitions += 1;
            let id = format!("{}:{}:t{}", x.prefix, r.id, self.transitions);
            log.push(SetupLogEntry::Transition(SetupTransition {
                id: id.clone(),
                symbol: x.symbol.to_string(),
                setup_id: self.armed.as_ref().map(|a| a.id.clone()),
                from: self.state,
                to: state,
                reason,
                at: x.cutoff,
                bar_end: x.bar_end,
                trigger_version: self.armed.as_ref().map(|a| a.trigger.version.clone()),
                invalidation_version: self.armed.as_ref().map(|a| a.invalidation.version.clone()),
            }));
            self.state = Some(state);
            self.since = x.cutoff;
            self.transition_id = Some(id);
        }
        self.reason = reason;
        let price = x.last_price.map(|(_, p)| p);
        let a = self.armed.as_ref();
        let (trig, inv) = (a.map(|a| a.trigger.price), a.map(|a| a.invalidation.price));
        let distance_to_trigger_bp = match (trig, price) {
            (Some(t), Some(p)) if p > 0 => Some(bp(t - p, p)),
            _ => None,
        };
        let meter_milli = match (trig, inv, price) {
            (Some(t), Some(i), Some(p)) if t > i => {
                Some(((p - i).clamp(0, t - i) as i128 * 1000 / (t - i) as i128) as u32)
            }
            (Some(t), Some(_), Some(p)) => Some(if p >= t { 1000 } else { 0 }),
            _ => None,
        };
        let risk_to_stop_bp = match (inv, price) {
            (Some(i), Some(p)) if p > 0 => Some(bp(p - i, p)),
            _ => None,
        };
        let extended = match (trig, price) {
            (Some(t), Some(p)) => p as i128 * 10_000 > t as i128 * (10_000 + r.extended_bp as i128),
            _ => false,
        };
        SetupView {
            family: r.id.clone(),
            policy: policy.tag(),
            experimental: policy.body.experimental,
            state,
            state_since: self.since,
            reason,
            transition_id: self.transition_id.clone(),
            setup_id: a.map(|a| a.id.clone()),
            pole: a.map(|a| a.pole.clone()).or(pole),
            trigger: a.map(|a| a.trigger.clone()),
            invalidation: a.map(|a| a.invalidation.clone()),
            flag_bars: a.map_or(0, |a| a.flag_bars),
            price,
            distance_to_trigger_bp,
            meter_milli,
            risk_to_stop_bp,
            pending,
            crossed_version: crossed,
            extended,
            outcome: self.outcome.clone(),
        }
    }

    #[allow(clippy::type_complexity)]
    fn decide(
        &mut self,
        r: &BullFlagRules,
        grid_bar_secs: i64,
        x: &Step,
        log: &mut Vec<SetupLogEntry>,
    ) -> (
        SetupState,
        &'static str,
        Vec<Pending>,
        Option<String>,
        Option<Pole>,
    ) {
        use SetupState::*;
        let none = |s, why| (s, why, Vec::new(), None, None);
        if r.bar_secs != grid_bar_secs {
            self.armed = None;
            return none(Watching, "bar_secs_mismatch");
        }
        if x.halted {
            // Paused: levels and any displayed outcome are kept, nothing is tested.
            return none(Halted, "halted");
        }
        let b = x.bar_end;
        if let Some(o) = &self.outcome {
            if x.cutoff - o.at < r.display_hold_secs * 1_000_000_000 {
                return none(o.state, o.reason);
            }
            // Cooldown over: re-arm and look for a new impulse after the spent one.
            self.search_from = o.bar_end;
            self.outcome = None;
            self.armed = None;
        }
        let price = match x.last_price {
            Some((e, p)) if b - e <= r.max_quiet_secs * 1_000_000_000 => p,
            _ => {
                self.armed = None;
                return none(Watching, "no_recent_prints");
            }
        };
        // The bar under test: completed at this barrier, with an OHLC print.
        let tested = x
            .bars
            .back()
            .filter(|t| t.end == b)
            .and_then(|t| t.ohlc.map(|o| (o, t.volume)));

        if self.armed.is_none() {
            match self.detect(r, x) {
                Detect::None(why) => return none(Watching, why),
                Detect::Awaiting(pole) => {
                    return (
                        Watching,
                        "impulse_awaiting_flag",
                        Vec::new(),
                        None,
                        Some(pole),
                    );
                }
                Detect::Flag(a) => {
                    Self::log_level(log, x.symbol, &a.id, "trigger", &a.trigger);
                    Self::log_level(log, x.symbol, &a.id, "invalidation", &a.invalidation);
                    self.armed = Some(a);
                    // The bar that completed the flag is part of it: nothing is tested this time.
                    return self.price_state(r, price, Vec::new(), None);
                }
            }
        }

        let a = self.armed.as_mut().expect("armed");
        let mut pending = Vec::new();
        let mut crossed = None;
        if let Some((o, vol)) = tested.filter(|_| b > a.last_bar_end) {
            let close_ok = o.close > a.trigger.price;
            let vol_ok = a.flag_bars > 0
                && vol as u128 * a.flag_bars as u128 * 1000
                    >= r.confirm_vol_mult_milli as u128 * a.flag_volume as u128;
            if close_ok && vol_ok {
                return self.finish(
                    Triggered,
                    "closed_above_trigger_with_volume",
                    x,
                    o.close,
                    vol,
                );
            }
            if o.close < a.invalidation.price {
                return self.finish(Invalidated, "closed_below_flag_low", x, o.close, vol);
            }
            if o.high > a.trigger.price {
                crossed = Some(a.trigger.version.clone());
                pending = vec![
                    Pending {
                        condition: "close_above_trigger",
                        state: if close_ok { Tri::Pass } else { Tri::Fail },
                    },
                    Pending {
                        condition: "volume",
                        state: if vol_ok { Tri::Pass } else { Tri::Fail },
                    },
                ];
            }
            // Not a breakout: the bar joins the flag, and levels it moves get new versions.
            a.flag_bars += 1;
            a.flag_volume += vol;
            a.last_bar_end = b;
            let (h, l) = (a.pole.high as i128, a.pole.low as i128);
            if (o.low as i128) * 1000 <= h * 1000 - r.max_retrace_milli as i128 * (h - l) {
                return self.finish(Invalidated, "retrace_beyond_max", x, o.close, vol);
            }
            if a.flag_bars > r.flag_max_bars {
                return self.finish(Invalidated, "flag_exceeded_max_bars", x, o.close, vol);
            }
            if o.high > a.trigger.price {
                a.trigger_n += 1;
                a.trigger = Self::level(
                    &a.id,
                    "trigger",
                    a.trigger_n,
                    o.high,
                    x.cutoff,
                    "flag_high_raised",
                );
                Self::log_level(log, x.symbol, &a.id, "trigger", &a.trigger);
            }
            if o.low < a.invalidation.price {
                a.invalidation_n += 1;
                a.invalidation = Self::level(
                    &a.id,
                    "invalidation",
                    a.invalidation_n,
                    o.low,
                    x.cutoff,
                    "flag_low_lowered",
                );
                Self::log_level(log, x.symbol, &a.id, "invalidation", &a.invalidation);
            }
        }
        if price < a.invalidation.price {
            return self.finish(Invalidated, "price_below_flag_low", x, price, 0);
        }
        self.price_state(r, price, pending, crossed)
    }

    /// Forming / near / crossing from the latest price against the current levels.
    #[allow(clippy::type_complexity)]
    fn price_state(
        &self,
        r: &BullFlagRules,
        price: i64,
        mut pending: Vec<Pending>,
        crossed: Option<String>,
    ) -> (
        SetupState,
        &'static str,
        Vec<Pending>,
        Option<String>,
        Option<Pole>,
    ) {
        let a = self.armed.as_ref().expect("armed");
        let t = a.trigger.price as i128;
        if crossed.is_some() {
            return (
                SetupState::Crossing,
                "awaiting_confirmation",
                pending,
                crossed,
                None,
            );
        }
        if price as i128 > t {
            // Above the current trigger with no completed bar to judge yet.
            pending = vec![
                Pending {
                    condition: "close_above_trigger",
                    state: Tri::Unknown,
                },
                Pending {
                    condition: "volume",
                    state: Tri::Unknown,
                },
            ];
            let v = Some(a.trigger.version.clone());
            return (
                SetupState::Crossing,
                "awaiting_confirmation",
                pending,
                v,
                None,
            );
        }
        if price as i128 * 10_000 >= t * (10_000 - r.near_bp as i128) {
            (
                SetupState::Near,
                "within_near_of_trigger",
                pending,
                None,
                None,
            )
        } else {
            (SetupState::Forming, "flag_valid", pending, None, None)
        }
    }

    #[allow(clippy::type_complexity)]
    fn finish(
        &mut self,
        state: SetupState,
        reason: &'static str,
        x: &Step,
        price: i64,
        vol: u64,
    ) -> (
        SetupState,
        &'static str,
        Vec<Pending>,
        Option<String>,
        Option<Pole>,
    ) {
        let a = self.armed.as_ref().expect("armed");
        let level = if state == SetupState::Triggered {
            a.trigger.clone()
        } else {
            a.invalidation.clone()
        };
        self.outcome = Some(Outcome {
            state,
            reason,
            at: x.cutoff,
            bar_end: x.bar_end,
            level,
            price,
            bar_volume: (vol > 0).then_some(vol),
        });
        (state, reason, Vec::new(), None, None)
    }

    fn detect(&mut self, r: &BullFlagRules, x: &Step) -> Detect {
        let b = x.bar_end;
        let from = (b - r.impulse_lookback_bars * x.bar_ns).max(self.search_from);
        let window: Vec<(&Bar, Ohlc)> = x
            .bars
            .iter()
            .filter(|t| t.end > from && t.end <= b)
            .filter_map(|t| t.ohlc.map(|o| (t, o)))
            .collect();
        if window.is_empty() {
            return Detect::None("no_impulse");
        }
        // Pole high: the earliest bar with the window's highest high (a later higher bar extends
        // the pole); pole low: the lowest low at or before it (the latest such bar).
        let (j, hi) = window
            .iter()
            .enumerate()
            .fold((0, &window[0]), |acc, (k, w)| {
                if w.1.high > acc.1.1.high { (k, w) } else { acc }
            });
        let (_, lo) = window[..=j]
            .iter()
            .enumerate()
            .fold((0, &window[0]), |acc, (k, w)| {
                if w.1.low <= acc.1.1.low { (k, w) } else { acc }
            });
        let (h, l) = (hi.1.high, lo.1.low);
        if l <= 0 || (h as i128) * 10_000 < (l as i128) * (10_000 + r.impulse_min_bp as i128) {
            return Detect::None("no_impulse");
        }
        let (lo_end, hi_end) = (lo.0.end, hi.0.end);
        let pole_volume: u64 = x
            .bars
            .iter()
            .filter(|t| t.end >= lo_end && t.end <= hi_end)
            .map(|t| t.volume)
            .sum();
        let pole_slots = ((hi_end - lo_end) / x.bar_ns + 1) as u128;
        if x.day_slots == 0
            || (pole_volume as u128) * (x.day_slots as u128) * 1000
                < r.impulse_vol_mult_milli as u128 * x.day_volume as u128 * pole_slots
        {
            return Detect::None("impulse_volume_low");
        }
        let pole = Pole {
            low: l,
            low_bar_end: lo_end,
            high: h,
            high_bar_end: hi_end,
        };
        let flag: Vec<&(&Bar, Ohlc)> = window[j + 1..].iter().collect();
        let floor = (h as i128) * 1000 - r.max_retrace_milli as i128 * (h - l) as i128;
        if flag.iter().any(|w| (w.1.low as i128) * 1000 <= floor) {
            return Detect::None("retrace_beyond_max");
        }
        let n = flag.len() as u32;
        if n < r.flag_min_bars {
            return Detect::Awaiting(pole);
        }
        if n > r.flag_max_bars {
            return Detect::None("flag_exceeded_max_bars");
        }
        self.setups += 1;
        let id = format!("{}:{}:{}", x.prefix, r.id, self.setups);
        let trig = flag.iter().map(|w| w.1.high).max().expect("flag bars");
        let inv = flag.iter().map(|w| w.1.low).min().expect("flag bars");
        Detect::Flag(Armed {
            trigger: Self::level(&id, "trigger", 1, trig, x.cutoff, "flag_formed"),
            invalidation: Self::level(&id, "invalidation", 1, inv, x.cutoff, "flag_formed"),
            id,
            pole,
            trigger_n: 1,
            invalidation_n: 1,
            flag_bars: n,
            flag_volume: flag.iter().map(|w| w.0.volume).sum(),
            last_bar_end: b,
        })
    }
}

enum Detect {
    None(&'static str),
    Awaiting(Pole),
    Flag(Armed),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v0_parses_and_is_experimental() {
        let p = SetupPolicy::v0();
        assert!(p.body.experimental);
        assert_eq!(p.body.bull_flag.id, "bull_flag_v0");
        assert!(p.tag().starts_with("setups-v0@"));
        let edited = SETUPS_V0.replacen("\"near_bp\": 100", "\"near_bp\": 101", 1);
        assert_ne!(SetupPolicy::parse(&edited).unwrap().hash, p.hash);
        let bad = SETUPS_V0.replacen("\"flag_min_bars\": 2", "\"flag_min_bars\": 0", 1);
        assert!(SetupPolicy::parse(&bad).is_err());
    }

    #[test]
    fn bars_follow_execution_order_and_counting() {
        let mut b = Bar::new(15);
        b.add(5, 110, 10, true, true);
        b.add(2, 100, 10, true, true); // earlier execution admitted later: the open
        b.add(9, 120, 50, true, false); // OHLC only
        b.add(7, 90, 30, false, true); // volume only
        let o = b.ohlc.unwrap();
        assert_eq!((o.open, o.high, o.low, o.close), (100, 120, 100, 120));
        assert_eq!(b.volume, 50);
    }
}
