//! The Ross Panel's per-row pullback cue (owner decision 2026-10-21).
//!
//! ONE pullback detector everywhere: each member's cue is read from the engine's
//! `micro_pullback` episodes under the product detection policy (`detection-v2`, variant MP5 of
//! the pre-registered setup comparison), never from a detector of its own. The levels, the states
//! and the episode ids are the ones the Detection Panel shows for the same symbol, so the two
//! panels cannot disagree. With the Detection Panel off the engine still runs that one family,
//! only for the members ([`crate::Engine::new`]); `bull_flag_v0` (`crate::setup`) is kept for
//! research reproducibility and no longer drives any row.
//!
//! A cue describes a STAGE of a pullback the engine can name, not an entry signal. On the
//! 2026-10-07..20 holdout a `trigger_observed` episode reached +3 % before −1.5 % only about one
//! time in four or five (precision 0.183). Nothing here ranks, scores or predicts:
//! `docs/stock-context/ross-panel-v1.md`.
//!
//! # From detector states to cue states
//!
//! The wire keeps the coarse tokens clients already decode; `stage` carries the detector state.
//!
//! | detector                       | cue           | shown as        |
//! |--------------------------------|---------------|-----------------|
//! | `pullback_forming`, `armed`    | `forming`     | "Pullback"      |
//! | `armed`, price within near     | `near`        | "At trigger"    |
//! | `crossing_provisional`         | `crossing`    | "Crossing"      |
//! | `trigger_observed` (held)      | `triggered`   | "Broke trigger" |
//! | `invalidated` (held)           | `invalidated` | "Failed"        |
//!
//! `near` is the detection policy's `assessment.near_trigger_bp` geometry on the latest admitted
//! price, exactly the Detection Panel's "Near trigger". A terminal episode stays on the row for
//! `ross.outcome_hold_secs` after its last transition unless a newer episode is active;
//! `expired`, `superseded` and `interrupted` episodes are not outcomes and show as `watching`
//! with their reason.

use crate::detect::{self, Detector, Episode, Family, Resolution, SEC, State, SymDetect};
use crate::pillar::Tri;
use crate::setup::Pending;
use serde::Serialize;

/// The coarse cue state (the wire tokens of the earlier `bull_flag_v0` cue, kept so clients that
/// have not updated still decode a row).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CueState {
    /// No pullback episode (or the data cannot support one: see `reason`).
    Watching,
    /// `pullback_forming`, or `armed` with the price not within the near distance.
    Forming,
    /// `armed`, the latest fresh price below the trigger by at most `near_trigger_bp`.
    Near,
    /// `crossing_provisional`: a post-arm print above the trigger, no full bar closed above yet.
    Crossing,
    /// `trigger_observed`, held for display. Descriptive: most of these failed on the holdout.
    Triggered,
    /// `invalidated`, held for display.
    Invalidated,
    /// The symbol is halted.
    Halted,
}

/// One version of a level, in the wire shape the earlier cue used (`version` a string).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CueLevel {
    pub price: i64,
    /// `<episode id>:<kind>:v<n>`.
    pub version: String,
    /// The cutoff (or admission time) the engine knew the level at.
    pub set_at: i64,
    pub reason: &'static str,
}

/// What a Ross row shows for one member at one barrier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RossCue {
    /// `micro_pullback`.
    pub family: &'static str,
    /// The policy's micro-pullback variant (`micro_pullback_mp5`).
    pub variant: String,
    /// `detection-v2@<hash>`.
    pub policy: String,
    pub experimental: bool,
    pub state: CueState,
    /// The detector state of the episode shown (`None` while watching with no episode).
    pub stage: Option<State>,
    /// The episode's last transition (`None` while watching).
    pub state_since: Option<i64>,
    pub reason: &'static str,
    /// The Detection Panel's episode id, when there is an episode.
    pub episode_id: Option<String>,
    pub trigger: Option<CueLevel>,
    pub invalidation: Option<CueLevel>,
    /// `(H0 − L0) / L0`, bp.
    pub impulse_bp: Option<i64>,
    /// Fraction of the impulse retraced, thousandths.
    pub depth_milli: Option<i64>,
    pub pullback_bars: Option<u32>,
    /// The latest admitted price the cue was judged on.
    pub price: Option<i64>,
    /// That price is older than the policy's `price_fresh_secs`.
    pub stale: bool,
    /// The Detection Panel's near-trigger geometry (`None` without an armed trigger or with a
    /// stale price).
    pub near_trigger: Option<bool>,
    /// `(trigger − price) / price`, bp, truncated: positive below the trigger.
    pub distance_to_trigger_bp: Option<i64>,
    /// `(price − invalidation) / (trigger − invalidation)` clamped to `[0, 1000]` thousandths.
    pub meter_milli: Option<u32>,
    /// `(price − invalidation) / price`, bp, truncated: how far above the failure level.
    pub to_invalidation_bp: Option<i64>,
    /// While `crossing`: a full post-arm bar must still close above the trigger.
    pub pending: Vec<Pending>,
    /// Price above the trigger by more than the micro rule's `extended_bp`.
    pub extended: bool,
    /// The held outcome's resolution (`triggered` / `invalidated` only).
    pub resolution: Option<Resolution>,
    /// The episode's H0 bar end, never on the wire: what identifies the same setup across a
    /// restart, whose new process numbers it differently (`crate::Engine::set_episode_floors`),
    /// so the server's notices can recognize one the previous process already announced.
    #[serde(skip)]
    pub h0_bar_end: Option<i64>,
}

impl RossCue {
    fn watching(det: &Detector, reason: &'static str, price: Option<i64>) -> RossCue {
        RossCue {
            family: Family::MicroPullback.as_str(),
            variant: det.policy.body.micro.id.clone(),
            policy: det.policy.tag(),
            experimental: det.policy.body.experimental,
            state: CueState::Watching,
            stage: None,
            state_since: None,
            reason,
            episode_id: None,
            trigger: None,
            invalidation: None,
            impulse_bp: None,
            depth_milli: None,
            pullback_bars: None,
            price,
            stale: false,
            near_trigger: None,
            distance_to_trigger_bp: None,
            meter_milli: None,
            to_invalidation_bp: None,
            pending: Vec::new(),
            extended: false,
            resolution: None,
            h0_bar_end: None,
        }
    }

    /// Whether the row needs a look now (`near`, `crossing`). A held `triggered` is history, so
    /// it no longer counts: the cue describes, it does not call for action.
    pub fn attention(&self) -> bool {
        matches!(self.state, CueState::Near | CueState::Crossing)
    }
}

fn level(ep: &Episode, kind: &str) -> Option<CueLevel> {
    ep.levels
        .iter()
        .rev()
        .find(|l| l.kind == kind)
        .map(|l| CueLevel {
            price: l.price,
            version: format!("{}:v{}", l.level_id, l.version),
            set_at: l.known_at,
            reason: l.reason,
        })
}

/// The cue for one member. `hold_reason` is why its inputs cannot be reported (history pending,
/// untrusted window), which the detector has already acted on by interrupting.
pub(crate) fn cue(
    det: &Detector,
    sd: &SymDetect,
    halted: bool,
    hold_reason: Option<&'static str>,
    cutoff: i64,
) -> RossCue {
    let price = sd.last_seen.map(|s| s.price);
    if let Some(why) = det.disabled {
        return RossCue::watching(det, why, price);
    }
    if halted {
        let mut c = RossCue::watching(det, "halted", price);
        c.state = CueState::Halted;
        return c;
    }
    let fresh = det.policy.body.timing.price_fresh_secs * SEC;
    let near_bp = det
        .policy
        .body
        .assessment
        .as_ref()
        .map(|a| a.near_trigger_bp);
    let active = sd
        .active
        .iter()
        .find(|e| e.view.family == Family::MicroPullback && !e.view.state.is_terminal());
    let (ep, extra, state, reason) = match active {
        Some(e) => {
            let a = detect::assess(&e.view, &e.extra, sd.last_seen, det.as_of, fresh, near_bp);
            let near = a.as_ref().and_then(|a| a.near_trigger) == Some(true);
            let (state, reason) = match e.view.state {
                State::CrossingProvisional => (CueState::Crossing, "awaiting_full_bar_close"),
                State::Armed if near => (CueState::Near, "within_near_of_trigger"),
                State::Armed => (CueState::Forming, "levels_fixed"),
                _ => (CueState::Forming, "pullback_forming"),
            };
            (&e.view, Some(&e.extra), state, reason)
        }
        None => {
            let hold = det
                .policy
                .body
                .ross
                .as_ref()
                .map_or(0, |r| r.outcome_hold_secs)
                * SEC;
            let held = sd.last_micro.as_ref().filter(|e| {
                matches!(e.state, State::TriggerObserved | State::Invalidated)
                    && cutoff - e.last_material_at < hold
            });
            match held {
                Some(e) => {
                    let state = if e.state == State::TriggerObserved {
                        CueState::Triggered
                    } else {
                        CueState::Invalidated
                    };
                    let reason = e.resolution.as_ref().map_or("resolved", |r| r.reason);
                    (e, None, state, reason)
                }
                None => {
                    // Why nothing shows: the inputs first, then a recent non-outcome ending.
                    let recent = sd
                        .last_micro
                        .as_ref()
                        .filter(|e| cutoff - e.last_material_at < hold)
                        .and_then(|e| e.resolution.as_ref())
                        .map(|r| r.reason);
                    let why = hold_reason.or(recent).unwrap_or("no_pullback");
                    return RossCue::watching(det, why, price);
                }
            }
        }
    };
    let mut c = RossCue::watching(det, reason, price);
    c.state = state;
    c.stage = Some(ep.state);
    c.state_since = Some(ep.last_material_at);
    c.episode_id = Some(ep.episode_id.clone());
    c.trigger = level(ep, "trigger");
    c.invalidation = level(ep, "invalidation");
    if let Some(a) = &ep.anchors {
        c.impulse_bp = Some(a.impulse_bp);
        c.depth_milli = Some(a.depth_milli);
        c.pullback_bars = Some(a.pullback_bars);
        c.h0_bar_end = Some(a.h0_bar_end);
    }
    if state == CueState::Triggered || state == CueState::Invalidated {
        c.resolution = ep.resolution.clone();
    }
    let now = det.as_of.unwrap_or(cutoff);
    c.stale = sd
        .last_seen
        .is_none_or(|s| now.max(s.admit) - s.exec > fresh);
    if let Some(extra) = extra {
        c.near_trigger = detect::assess(ep, extra, sd.last_seen, det.as_of, fresh, near_bp)
            .and_then(|a| a.near_trigger);
    }
    let (t, i) = (
        c.trigger.as_ref().map(|l| l.price),
        c.invalidation.as_ref().map(|l| l.price),
    );
    if let Some(p) = price.filter(|&p| p > 0) {
        c.distance_to_trigger_bp = t.map(|t| detect::math::bp(t - p, p));
        c.to_invalidation_bp = i.map(|i| detect::math::bp(p - i, p));
        c.meter_milli = match (t, i) {
            (Some(t), Some(i)) if t > i => {
                Some(((p - i).clamp(0, t - i) as i128 * 1000 / (t - i) as i128) as u32)
            }
            _ => None,
        };
        let ext = det.policy.body.micro.extended_bp as i128;
        c.extended = t.is_some_and(|t| p as i128 * 10_000 > t as i128 * (10_000 + ext));
    }
    if state == CueState::Crossing {
        c.pending = vec![Pending {
            condition: "close_above_trigger",
            state: Tri::Unknown,
        }];
    }
    c
}
