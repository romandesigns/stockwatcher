//! The Ross Panel (owner product policy, 2026-10-19; `policy/ross-panel-v1.json`): which stocks
//! are on it (`five_pillars_membership_v1`) and in what order (`bullish_momentum_order_v1`).
//!
//! Membership is separate from qualification: a stock enters when all five pillars PASS in ONE
//! decision while it is eligible on the platform, stays while at least four PASS (an UNKNOWN never
//! counts as a pass), and leaves below four or when it stops being eligible. Re-entry needs 5/5
//! again and opens a new episode. A new trading date starts from nothing.
//!
//! Order is current bullish price action, not the pillar count: one-minute return, then the
//! five-minute return, then the change between the last two 30 s returns, then the symbol. Every
//! comparison is an exact cross-multiplication of integer prices (no floating logarithms), so
//! display rounding can never reorder rows. A member whose one-minute endpoint is missing, stale,
//! in another session segment or halted is shown unranked below the ranked rows, never with an
//! invented zero.

use crate::evaluate::Coverage;
use crate::pillar::{Counts, PillarId, PillarResult, Tri};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

pub const ROSS_PANEL_V1: &str = include_str!("../policy/ross-panel-v1.json");
/// v1 with the news pillar optional (owner, 2026-11-22): 4/4 of the other pillars to enter, 3/4
/// to stay. The server runs it while the synced switch "Ross Panel: require a news catalyst" is
/// off, and v1 otherwise; both stay frozen by id.
pub const ROSS_PANEL_V2: &str = include_str!("../policy/ross-panel-v2.json");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PlatformBand {
    pub min_micros: i64,
    pub max_micros: i64,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct MembershipRules {
    pub id: String,
    /// The news pillar is not counted for membership (`ross-panel-v2`): admission and retention
    /// count passes among the other four pillars only. Absent from v1, so its bytes, hash and
    /// decisions are unchanged. The news pillar is still decided, kept in `nonpassing` and shown.
    #[serde(default)]
    pub news_optional: bool,
    /// Passes needed to enter, among the pillars counted (5 of 5, or 4 of 4 with news optional).
    pub admit_passes: u8,
    pub retain_passes: u8,
    pub platform_band: PlatformBand,
}

impl MembershipRules {
    /// How many pillars membership counts.
    pub fn counted(&self) -> u8 {
        if self.news_optional { 4 } else { 5 }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct MomentumRules {
    pub id: String,
    pub short_secs: i64,
    pub primary_secs: i64,
    pub context_secs: i64,
    pub endpoint_max_age_secs: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PanelPolicyBody {
    pub schema: u32,
    pub id: String,
    pub note: String,
    pub membership: MembershipRules,
    pub momentum: MomentumRules,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelPolicy {
    pub body: PanelPolicyBody,
    pub hash: String,
}

impl PanelPolicy {
    pub fn parse(text: &str) -> Result<PanelPolicy, String> {
        let body: PanelPolicyBody =
            serde_json::from_str(text).map_err(|e| format!("panel policy: {e}"))?;
        let m = &body.membership;
        // Admission is always every counted pillar passing in one decision: no partial entry.
        if body.schema != 1
            || m.admit_passes != m.counted()
            || m.retain_passes == 0
            || m.retain_passes > m.admit_passes
        {
            return Err("panel policy: unsupported schema or pass counts".into());
        }
        let mo = &body.momentum;
        if mo.short_secs <= 0
            || mo.primary_secs != 2 * mo.short_secs
            || mo.context_secs <= mo.primary_secs
            || mo.endpoint_max_age_secs <= 0
        {
            return Err("panel policy: momentum windows must be short > 0, primary = 2 x short, context > primary".into());
        }
        let hash = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(PanelPolicy { body, hash })
    }

    pub fn v1() -> PanelPolicy {
        PanelPolicy::parse(ROSS_PANEL_V1).expect("embedded ross-panel-v1.json parses")
    }

    /// The news-optional panel (owner switch off).
    pub fn v2() -> PanelPolicy {
        PanelPolicy::parse(ROSS_PANEL_V2).expect("embedded ross-panel-v2.json parses")
    }

    /// The panel the owner's switch selects: v1 while news is required, v2 while it is optional.
    pub fn for_news_required(required: bool) -> PanelPolicy {
        if required {
            PanelPolicy::v1()
        } else {
            PanelPolicy::v2()
        }
    }

    /// Whether this panel admits without a passing news pillar.
    pub fn news_optional(&self) -> bool {
        self.body.membership.news_optional
    }

    pub fn tag(&self) -> String {
        format!("{}@{}", self.body.id, &self.hash[..12])
    }
}

/// Why a stock is or is not on the panel right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    /// Never admitted on this trading date.
    NotMember,
    /// Active, every pillar passing: "Qualified 5/5".
    Qualified,
    /// Active at 4/5: "Retained 4/5".
    Retained,
    /// Was a member earlier today, not now.
    Exited,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    /// Fewer than four passes, every non-pass a FAIL.
    KnownFailures,
    /// Fewer than four passes, every non-pass UNKNOWN.
    InsufficientEvidence,
    /// Fewer than four passes, both kinds.
    FailuresAndUnknowns,
    /// Fresh price outside the platform band.
    LeftPlatformBand,
    /// No fresh price, or outside a supported session.
    Ineligible,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Episode {
    /// `<run>:<date>:<symbol>:<n>`, n from 1 per trading date.
    pub id: String,
    pub admitted_at: i64,
    pub admitted_decision: String,
    /// First cutoff it was held at 4/5 (not 5/5).
    pub retained_since: Option<i64>,
    pub exited_at: Option<i64>,
    pub exit_reason: Option<ExitReason>,
}

/// One symbol's membership on one trading date. Episodes are append-only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Membership {
    pub status: Option<MemberStatus>,
    pub episodes: Vec<Episode>,
    /// The pillars not passing at the latest decision, with their state and reason.
    pub nonpassing: Vec<(PillarId, Tri, &'static str)>,
}

impl Membership {
    pub fn active(&self) -> Option<&Episode> {
        self.episodes.last().filter(|e| e.exited_at.is_none())
    }
}

/// Platform eligibility at a decision: a fresh price inside the platform band, in a supported
/// session. Not a sixth pillar: failing it removes a member whatever its pillar count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eligibility {
    Eligible,
    OutOfBand,
    Ineligible,
}

/// Applies one decision's pillars to a symbol's membership. Returns whether it changed.
#[allow(clippy::too_many_arguments)]
pub fn update_membership(
    m: &mut Membership,
    rules: &MembershipRules,
    pillars: &[PillarResult],
    counts: Counts,
    eligibility: Eligibility,
    cutoff: i64,
    decision_id: &str,
    episode_prefix: &str,
) -> bool {
    let before = m.clone();
    // With news optional (ross-panel-v2) the news pillar is left out of the count, so admission
    // is 4/4 and retention 3/4 of the others; `nonpassing` still names it, so a row can say its
    // catalyst is unconfirmed.
    let counts = if rules.news_optional {
        let mut c = Counts::default();
        for p in pillars.iter().filter(|p| p.pillar != PillarId::News) {
            match p.state {
                Tri::Pass => c.pass += 1,
                Tri::Fail => c.fail += 1,
                Tri::Unknown => c.unknown += 1,
            }
        }
        c
    } else {
        counts
    };
    m.nonpassing = pillars
        .iter()
        .filter(|p| p.state != Tri::Pass)
        .map(|p| (p.pillar, p.state, p.reason))
        .collect();
    let eligible = eligibility == Eligibility::Eligible;
    let n = m.episodes.len();
    match m.episodes.last_mut().filter(|e| e.exited_at.is_none()) {
        None => {
            if eligible && counts.pass >= rules.admit_passes {
                m.episodes.push(Episode {
                    id: format!("{episode_prefix}:{}", n + 1),
                    admitted_at: cutoff,
                    admitted_decision: decision_id.to_string(),
                    retained_since: None,
                    exited_at: None,
                    exit_reason: None,
                });
            }
        }
        Some(e) => {
            let reason = match eligibility {
                Eligibility::OutOfBand => Some(ExitReason::LeftPlatformBand),
                Eligibility::Ineligible => Some(ExitReason::Ineligible),
                Eligibility::Eligible if counts.pass >= rules.retain_passes => None,
                Eligibility::Eligible => Some(match (counts.fail > 0, counts.unknown > 0) {
                    (true, false) => ExitReason::KnownFailures,
                    (false, true) => ExitReason::InsufficientEvidence,
                    _ => ExitReason::FailuresAndUnknowns,
                }),
            };
            match reason {
                Some(r) => {
                    e.exited_at = Some(cutoff);
                    e.exit_reason = Some(r);
                }
                None if counts.pass < rules.admit_passes && e.retained_since.is_none() => {
                    e.retained_since = Some(cutoff)
                }
                None => {}
            }
        }
    }
    m.status = Some(match m.active() {
        Some(_) if counts.pass >= rules.admit_passes => MemberStatus::Qualified,
        Some(_) => MemberStatus::Retained,
        None if m.episodes.is_empty() => MemberStatus::NotMember,
        None => MemberStatus::Exited,
    });
    *m != before
}

/// A price endpoint: the latest OHLC print before the endpoint, with its execution time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Endpoint {
    pub at: i64,
    pub price: i64,
    pub exec: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MomentumStatus {
    Ready,
    /// The one-minute measurement is unavailable (reason given): the member is unranked.
    Unavailable,
}

/// The momentum descriptors at one decision. Returns are shown in basis points of the simple
/// return (`P_b / P_e − 1`), rounded toward zero; ranking never reads them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Momentum {
    pub status: MomentumStatus,
    pub reason: &'static str,
    pub now: Option<Endpoint>,
    pub ago_short: Option<Endpoint>,
    pub ago_primary: Option<Endpoint>,
    pub ago_context: Option<Endpoint>,
    pub r_short_bp: Option<i64>,
    pub r_primary_bp: Option<i64>,
    pub r_context_bp: Option<i64>,
    /// Change between the last two short returns, in bp of `P_b·P_(b−2s) / P_(b−s)² − 1`.
    pub accel_bp: Option<i64>,
    /// Below the day's high so far, bp (0 at the high).
    pub below_high_bp: Option<i64>,
    /// The run's own collection coverage, so an unverified window is visible.
    pub coverage: Coverage,
}

fn ret_bp(now: i64, then: i64) -> i64 {
    (((now as i128 - then as i128) * 10_000) / then as i128) as i64
}

/// Momentum at bar end `b` from a symbol's per-second price history. `price_at(e)` gives the
/// latest OHLC print executed before `e` (admitted by the barrier), `segment(t)` the session
/// segment of an instant.
pub fn momentum(
    rules: &MomentumRules,
    b: i64,
    price_at: impl Fn(i64) -> Option<(i64, i64)>,
    segment: impl Fn(i64) -> Option<u8>,
    day_high: Option<i64>,
    halted: bool,
    coverage: Coverage,
) -> Momentum {
    const SEC: i64 = 1_000_000_000;
    let max_age = rules.endpoint_max_age_secs * SEC;
    let seg_b = segment(b);
    // An endpoint counts only with a recent print in the same session segment as `b`.
    let endpoint = |at: i64| -> Option<Endpoint> {
        let (exec, price) = price_at(at)?;
        (at - exec <= max_age
            && seg_b.is_some()
            && segment(at) == seg_b
            && segment(exec) == seg_b
            && price > 0)
            .then_some(Endpoint { at, price, exec })
    };
    let now = endpoint(b);
    let short = endpoint(b - rules.short_secs * SEC);
    let primary = endpoint(b - rules.primary_secs * SEC);
    let context = endpoint(b - rules.context_secs * SEC);
    let below_high_bp = match (now, day_high) {
        (Some(n), Some(h)) if h > 0 => {
            Some((((h - n.price).max(0) as i128 * 10_000) / h as i128) as i64)
        }
        _ => None,
    };
    let reason = if halted {
        "halted"
    } else if coverage == Coverage::Partial {
        "coverage_partial"
    } else if now.is_none() {
        "no_recent_price"
    } else if primary.is_none() {
        "no_one_minute_endpoint"
    } else {
        "ready"
    };
    let ready = reason == "ready";
    let r = |e: Option<Endpoint>| match (now, e) {
        (Some(n), Some(e)) => Some(ret_bp(n.price, e.price)),
        _ => None,
    };
    let accel_bp = match (now, short, primary) {
        (Some(n), Some(s), Some(p)) => Some(
            (((n.price as i128 * p.price as i128 - s.price as i128 * s.price as i128) * 10_000)
                / (s.price as i128 * s.price as i128)) as i64,
        ),
        _ => None,
    };
    Momentum {
        status: if ready {
            MomentumStatus::Ready
        } else {
            MomentumStatus::Unavailable
        },
        reason,
        now,
        ago_short: short,
        ago_primary: primary,
        ago_context: context,
        r_short_bp: r(short),
        r_primary_bp: r(primary),
        r_context_bp: r(context),
        accel_bp,
        below_high_bp,
        coverage,
    }
}

/// `a.now / a.then` against `b.now / b.then`, exactly.
fn cmp_ratio(an: i64, at: i64, bn: i64, bt: i64) -> Ordering {
    (an as i128 * bt as i128).cmp(&(bn as i128 * at as i128))
}

/// `a` before `b` in the default order (both Ready): primary return desc, context return desc
/// (missing after observed), acceleration desc (missing after observed), then symbol asc.
pub fn momentum_order(a: (&Momentum, &str), b: (&Momentum, &str)) -> Ordering {
    let (ma, sa) = a;
    let (mb, sb) = b;
    let (na, nb) = (ma.now.expect("ready"), mb.now.expect("ready"));
    let (pa, pb) = (
        ma.ago_primary.expect("ready"),
        mb.ago_primary.expect("ready"),
    );
    let primary = cmp_ratio(nb.price, pb.price, na.price, pa.price); // descending
    let context = match (ma.ago_context, mb.ago_context) {
        (Some(ca), Some(cb)) => cmp_ratio(nb.price, cb.price, na.price, ca.price),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    // P_b · P_(b−2s) / P_(b−s)²: compare a vs b by cross-multiplying the squares.
    let accel = match (ma.ago_short, mb.ago_short) {
        (Some(sa_), Some(sb_)) => {
            let ka = na.price as u128 * pa.price as u128 * (sb_.price as u128 * sb_.price as u128);
            let kb = nb.price as u128 * pb.price as u128 * (sa_.price as u128 * sa_.price as u128);
            kb.cmp(&ka)
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    primary.then(context).then(accel).then_with(|| sa.cmp(sb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pillar::{PillarResult, aggregate};

    fn pillars(states: [Tri; 5]) -> Vec<PillarResult> {
        PillarId::ALL
            .iter()
            .zip(states)
            .map(|(&p, s)| {
                PillarResult::new(
                    p,
                    s,
                    None,
                    "",
                    String::new(),
                    if s == Tri::Unknown { "missing" } else { "x" },
                )
            })
            .collect()
    }

    fn step(m: &mut Membership, states: [Tri; 5], elig: Eligibility, t: i64) -> bool {
        step_under(&PanelPolicy::v1(), m, states, elig, t)
    }

    fn step_under(
        policy: &PanelPolicy,
        m: &mut Membership,
        states: [Tri; 5],
        elig: Eligibility,
        t: i64,
    ) -> bool {
        let p = pillars(states);
        let (_, c) = aggregate(p.iter().map(|x| x.state));
        update_membership(
            m,
            &policy.body.membership,
            &p,
            c,
            elig,
            t,
            &format!("d{t}"),
            "run:2026-10-16:ABCD",
        )
    }

    use Eligibility::*;
    use Tri::*;

    #[test]
    fn policy_v1_parses() {
        let p = PanelPolicy::v1();
        assert_eq!(p.body.membership.id, "five_pillars_membership_v1");
        assert_eq!(p.body.momentum.id, "bullish_momentum_order_v1");
        assert!(p.tag().starts_with("ross-panel-v1@"));
    }

    #[test]
    fn policy_v2_is_v1_with_news_optional_and_both_are_frozen() {
        let (v1, v2) = (PanelPolicy::v1(), PanelPolicy::v2());
        assert!(!v1.news_optional() && v2.news_optional());
        assert_eq!(
            (
                v2.body.membership.admit_passes,
                v2.body.membership.retain_passes
            ),
            (4, 3)
        );
        assert_eq!(v2.body.momentum, v1.body.momentum);
        assert_eq!(
            v2.body.membership.platform_band,
            v1.body.membership.platform_band
        );
        assert!(v2.tag().starts_with("ross-panel-v2@"));
        assert_ne!(v1.hash, v2.hash);
        assert_eq!(PanelPolicy::for_news_required(true), v1);
        assert_eq!(PanelPolicy::for_news_required(false), v2);
        // Admission is every counted pillar: a news-optional file asking for 3 is refused, and so
        // is a news-required one asking for 4.
        let bad = ROSS_PANEL_V2.replacen("\"admit_passes\": 4", "\"admit_passes\": 3", 1);
        assert!(PanelPolicy::parse(&bad).is_err());
        let bad = ROSS_PANEL_V1.replacen("\"admit_passes\": 5", "\"admit_passes\": 4", 1);
        assert!(PanelPolicy::parse(&bad).is_err());
    }

    #[test]
    fn news_optional_enters_at_four_of_four_and_stays_at_three() {
        // Pillar order: price, gain, rvol, float, news.
        let v2 = PanelPolicy::v2();
        let mut m = Membership::default();
        // Three of the four others is not enough, whatever news says.
        step_under(&v2, &mut m, [Pass, Pass, Pass, Unknown, Pass], Eligible, 1);
        assert_eq!(m.status, Some(MemberStatus::NotMember));
        // All four others pass, news FAILs: admitted, Qualified (4/4), news named as non-passing.
        step_under(&v2, &mut m, [Pass, Pass, Pass, Pass, Fail], Eligible, 2);
        assert_eq!(m.status, Some(MemberStatus::Qualified));
        assert_eq!(m.active().unwrap().admitted_at, 2);
        assert_eq!(m.nonpassing, vec![(PillarId::News, Fail, "x")]);
        // Three of four (float unknown) and news unknown: retained at 3/4.
        step_under(
            &v2,
            &mut m,
            [Pass, Pass, Pass, Unknown, Unknown],
            Eligible,
            3,
        );
        assert_eq!(m.status, Some(MemberStatus::Retained));
        assert_eq!(m.active().unwrap().retained_since, Some(3));
        // News passing does not make up for a second missing pillar: 2/4 leaves.
        step_under(&v2, &mut m, [Pass, Fail, Pass, Unknown, Pass], Eligible, 4);
        assert_eq!(m.status, Some(MemberStatus::Exited));
        assert_eq!(
            m.episodes[0].exit_reason,
            Some(ExitReason::FailuresAndUnknowns)
        );
        // Under v1 the same 4 + news FAIL never admits.
        let mut v1m = Membership::default();
        step(&mut v1m, [Pass, Pass, Pass, Pass, Fail], Eligible, 5);
        assert_eq!(v1m.status, Some(MemberStatus::NotMember));
    }

    #[test]
    fn enter_at_five_stay_at_four_leave_below() {
        let mut m = Membership::default();
        step(&mut m, [Pass, Pass, Pass, Pass, Unknown], Eligible, 1);
        assert_eq!(m.status, Some(MemberStatus::NotMember), "4/5 never admits");
        step(&mut m, [Pass; 5], Eligible, 2);
        assert_eq!(m.status, Some(MemberStatus::Qualified));
        assert_eq!(m.active().unwrap().admitted_at, 2);
        // 4 + one FAIL: retained, the failure named.
        step(&mut m, [Pass, Fail, Pass, Pass, Pass], Eligible, 3);
        assert_eq!(m.status, Some(MemberStatus::Retained));
        assert_eq!(m.nonpassing, vec![(PillarId::Gain, Fail, "x")]);
        assert_eq!(m.active().unwrap().retained_since, Some(3));
        // 4 + one UNKNOWN: still retained.
        step(&mut m, [Pass, Pass, Pass, Unknown, Pass], Eligible, 4);
        assert_eq!(m.status, Some(MemberStatus::Retained));
        // 3 + 2 UNKNOWN: out, as insufficient evidence, not as failures.
        step(&mut m, [Pass, Pass, Pass, Unknown, Unknown], Eligible, 5);
        assert_eq!(m.status, Some(MemberStatus::Exited));
        assert_eq!(
            m.episodes[0].exit_reason,
            Some(ExitReason::InsufficientEvidence)
        );
        // Back to 4/5 is not enough to re-enter; 5/5 opens a second episode.
        step(&mut m, [Pass, Pass, Pass, Pass, Unknown], Eligible, 6);
        assert_eq!(m.status, Some(MemberStatus::Exited));
        step(&mut m, [Pass; 5], Eligible, 7);
        assert_eq!(m.episodes.len(), 2);
        assert_eq!(m.active().unwrap().id, "run:2026-10-16:ABCD:2");
    }

    #[test]
    fn platform_eligibility_overrides_the_count() {
        let mut m = Membership::default();
        step(&mut m, [Pass; 5], Eligible, 1);
        // The $2-$20 price pillar may fail while the stock stays in the platform band…
        step(&mut m, [Fail, Pass, Pass, Pass, Pass], Eligible, 2);
        assert_eq!(m.status, Some(MemberStatus::Retained));
        // …but leaving the platform band removes it whatever the count.
        step(&mut m, [Fail, Pass, Pass, Pass, Pass], OutOfBand, 3);
        assert_eq!(
            m.episodes[0].exit_reason,
            Some(ExitReason::LeftPlatformBand)
        );
        // 5/5 while ineligible never admits.
        let mut m = Membership::default();
        step(&mut m, [Pass; 5], Ineligible, 1);
        assert_eq!(m.status, Some(MemberStatus::NotMember));
    }

    #[test]
    fn known_failures_and_mixed_exits_are_told_apart() {
        let mut m = Membership::default();
        step(&mut m, [Pass; 5], Eligible, 1);
        step(&mut m, [Pass, Pass, Pass, Fail, Fail], Eligible, 2);
        assert_eq!(m.episodes[0].exit_reason, Some(ExitReason::KnownFailures));
        step(&mut m, [Pass; 5], Eligible, 3);
        step(&mut m, [Pass, Pass, Pass, Fail, Unknown], Eligible, 4);
        assert_eq!(
            m.episodes[1].exit_reason,
            Some(ExitReason::FailuresAndUnknowns)
        );
    }

    const S: i64 = 1_000_000_000;

    fn mom(prices: &[(i64, i64)], b: i64) -> Momentum {
        let price_at = |e: i64| {
            prices
                .iter()
                .filter(|(t, _)| *t < e)
                .max_by_key(|(t, _)| *t)
                .copied()
        };
        momentum(
            &PanelPolicy::v1().body.momentum,
            b,
            price_at,
            |_| Some(2),
            Some(prices.iter().map(|p| p.1).max().unwrap()),
            false,
            Coverage::Complete,
        )
    }

    #[test]
    fn one_minute_return_leads_the_order_whatever_the_day() {
        let b = 1_000 * S;
        // A: up big on the day elsewhere, but down 2 % in the last minute. B: up 3 %.
        let a = mom(
            &[
                (b - 301 * S, 10_000_000),
                (b - 61 * S, 10_000_000),
                (b - 31 * S, 9_900_000),
                (b - 1, 9_800_000),
            ],
            b,
        );
        let bb = mom(
            &[
                (b - 301 * S, 5_000_000),
                (b - 61 * S, 5_000_000),
                (b - 31 * S, 5_100_000),
                (b - 1, 5_150_000),
            ],
            b,
        );
        assert_eq!(a.r_primary_bp, Some(-200));
        assert_eq!(bb.r_primary_bp, Some(300));
        assert_eq!(
            momentum_order((&bb, "B"), (&a, "A")),
            Ordering::Less,
            "B ranks above A"
        );
    }

    #[test]
    fn missing_or_stale_endpoints_are_unavailable_not_zero() {
        let b = 1_000 * S;
        // Last print 20 s before the one-minute endpoint: older than the 15 s limit.
        let m = mom(&[(b - 80 * S, 5_000_000), (b - 1, 5_100_000)], b);
        assert_eq!(
            (m.status, m.reason, m.r_primary_bp),
            (MomentumStatus::Unavailable, "no_one_minute_endpoint", None)
        );
        // A missing five-minute endpoint stays None and sorts after an observed one.
        let x = mom(
            &[
                (b - 61 * S, 5_000_000),
                (b - 31 * S, 5_000_000),
                (b - 1, 5_100_000),
            ],
            b,
        );
        let y = mom(
            &[
                (b - 301 * S, 5_000_000),
                (b - 61 * S, 5_000_000),
                (b - 31 * S, 5_000_000),
                (b - 1, 5_100_000),
            ],
            b,
        );
        assert_eq!(x.r_context_bp, None);
        assert_eq!(momentum_order((&y, "Y"), (&x, "X")), Ordering::Less);
        // Equal returns fall back to the symbol: a stable tie.
        assert_eq!(momentum_order((&y, "A"), (&y, "B")), Ordering::Less);
    }

    #[test]
    fn halted_is_unranked_and_below_high_is_reported() {
        let b = 1_000 * S;
        let price_at = |_e: i64| Some((b - S, 5_000_000));
        let m = momentum(
            &PanelPolicy::v1().body.momentum,
            b,
            price_at,
            |_| Some(2),
            Some(6_000_000),
            true,
            Coverage::Complete,
        );
        assert_eq!(
            (m.status, m.reason),
            (MomentumStatus::Unavailable, "halted")
        );
        assert_eq!(m.below_high_bp, Some(1666));
    }

    #[test]
    fn ordering_is_exact_where_display_rounds() {
        let b = 1_000 * S;
        // +1.00004 % vs +1.00001 %: both display as 100 bp, the order still sees the difference.
        let hi = mom(&[(b - 61 * S, 10_000_000), (b - 1, 10_100_004)], b);
        let lo = mom(&[(b - 61 * S, 10_000_000), (b - 1, 10_100_001)], b);
        assert_eq!(hi.r_primary_bp, lo.r_primary_bp);
        assert_eq!(momentum_order((&hi, "Z"), (&lo, "A")), Ordering::Less);
    }
}
