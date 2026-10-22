//! The strategy seam: [`RocketScorer`], the shipped placeholder, and the human-readable reasons.
//!
//! # Plugging a research scorer in
//!
//! 1. Implement [`RocketScorer`] for the frozen candidate (a new file beside this one is
//!    cleanest): `score` reads [`Features::x`] by the research's own names ([`super::ix`], the
//!    order of the research `FEATS`), so a frozen linear/logistic model is its weights dotted with
//!    `x` (with the research's NaN handling), a rule is a few comparisons.
//! 2. `fixed_threshold` returns the frozen.json threshold for the chosen budget, or `None` to let
//!    the alert policy adapt the threshold to `ROCKET_ALERTS_PER_DAY` (`super::Policy`).
//! 3. Change the ONE line in [`production_scorer`] to return it. `id`/`version` go into every
//!    alert, the forward log and the panel, so the track record never mixes strategies.
//!
//! Nothing else changes: the features, the budget and dedup, the forward log, the wire and both
//! panels are strategy-agnostic.

use super::features::{Features, ix};

/// A strategy that rates how likely a symbol is to start a rocket soon, from its causal state at
/// a completed 15 s barrier.
///
/// Called once per in-band symbol that traded in the last 30 minutes, at every barrier, under
/// the market's core lock: keep it to arithmetic on `x` (no allocation, no I/O). Higher is
/// stronger; the scale is the scorer's own.
pub trait RocketScorer: Send + Sync + 'static {
    /// Stable strategy id, e.g. `placeholder-b2`, `flow-F2`.
    fn id(&self) -> &str;
    /// The frozen candidate's version (commit or frozen.json hash).
    fn version(&self) -> &str;
    /// True for a stand-in that is not a research-selected strategy (the panel says so).
    fn placeholder(&self) -> bool {
        false
    }
    /// The score, or `None` when the symbol is not a candidate at this barrier (gated out).
    fn score(&self, f: &Features) -> Option<f64>;
    /// A fixed alert threshold (from the strategy's frozen.json), or `None` for the adaptive
    /// budget. `ROCKET_THRESHOLD` overrides either way.
    fn fixed_threshold(&self) -> Option<f64> {
        None
    }
    /// The lowest score the adaptive threshold may ever go down to.
    fn floor(&self) -> f64 {
        f64::NEG_INFINITY
    }
    /// The top 2–3 reasons shown on the alert (only called for alerts, so it may allocate).
    fn reasons(&self, f: &Features) -> Vec<String> {
        standard_reasons(f)
    }
}

/// The strategy the server runs. **The one line to change when a research scorer is plugged in.**
/// Since 2026-10-22: `precursors-R0`, the bake-off's round-1 pick (see `r0.rs` for its evidence).
pub fn production_scorer() -> Box<dyn RocketScorer> {
    Box::new(super::r0::PrecursorsR0)
}

/// The bake-off's baseline B2, shipped until the research leader is plugged in: volume
/// acceleration, the research's `vr3` (log of the last 3 minutes' volume per minute against the
/// 30 minutes before them, each +1), gated on the price being up against 30 minutes ago.
/// The research band gate is already applied by the features (unit barriers only).
pub struct PlaceholderB2;

impl RocketScorer for PlaceholderB2 {
    fn id(&self) -> &str {
        "placeholder-b2"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn placeholder(&self) -> bool {
        true
    }
    fn score(&self, f: &Features) -> Option<f64> {
        let vr3 = f.x[ix::VR3];
        (f.ret30 > 0.0 && vr3.is_finite()).then_some(vr3)
    }
    /// Volume at least ~3× its prior pace: below that "acceleration" is noise.
    fn floor(&self) -> f64 {
        3f64.ln()
    }
}

fn pct(lnr: f64) -> f64 {
    (lnr.exp() - 1.0) * 100.0
}

/// Reasons from the features, most telling first, at most three.
pub fn standard_reasons(f: &Features) -> Vec<String> {
    let mut out: Vec<(f64, String)> = Vec::new();
    let x = &f.x;
    if f.vol3_prior > 0.0 {
        let r = f.vol3 / f.vol3_prior;
        if r >= 2.0 {
            out.push((
                r.min(50.0) / 5.0 + 1.0,
                format!("volume {r:.1}× 30-min avg"),
            ));
        }
    } else if f.vol3 > 0.0 && x[ix::IDLE30] > 0.7 {
        out.push((3.0, "waking from a quiet base".to_string()));
    }
    let grind = x[ix::GRIND];
    if grind >= 3.0 {
        out.push((
            1.0 + grind / 4.0,
            format!("grinding: {grind:.0} higher closes"),
        ));
    }
    let dhi = x[ix::DHI];
    if dhi.is_finite() && dhi >= (0.98f64).ln() {
        let what = match f.session {
            Some(market_events::session::SessionKind::Pre) => "premarket high",
            _ => "day high",
        };
        let at = if dhi >= -1e-9 { "at" } else { "near" };
        out.push((1.5, format!("{at} {what}")));
    }
    let ret3 = x[ix::RET3];
    if ret3.is_finite() && ret3 > 0.02f64.ln_1p() {
        out.push((
            1.0 + pct(ret3) / 10.0,
            format!("+{:.0}% in 3 min", pct(ret3)),
        ));
    }
    let upt = x[ix::UPT3];
    if upt >= 0.7 && f.vol3 > 0.0 {
        out.push((1.2, format!("buyers {:.0}% of volume", upt * 100.0)));
    }
    let gain = x[ix::GAIN];
    if gain.is_finite() && gain > 0.1f64.ln_1p() {
        out.push((0.8, format!("+{:.0}% on the day", pct(gain))));
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.into_iter().take(3).map(|(_, s)| s).collect()
}
