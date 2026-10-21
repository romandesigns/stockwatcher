//! Ross Panel acceptance cases (owner policy 2026-10-19, brief §4.1 and §9.1) through the engine.

use chrono::NaiveDate;
use market_events::session::et;
use market_events::{Price, Trade, UnixNanos};
use stock_context::*;

const VOL: Verdict = Verdict {
    ohlc: true,
    volume: true,
};
const S: i64 = 1_000_000_000;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 16).unwrap()
}
fn prev() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}
fn t(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).unwrap().nanos()
}
fn inst(sym: &str) -> InstrumentId {
    InstrumentId::symbol(sym)
}

fn engine() -> Engine {
    Engine::new(
        Policy::v1(),
        RunInfo {
            run_id: "fixture".into(),
            source: "replay".into(),
            fidelity: Fidelity::SyntheticFixture,
            numerator_coverage: Coverage::Complete,
        },
    )
}

fn print(e: &mut Engine, sym: &str, exec: i64, micros: i64, size: u64) {
    e.apply(
        &Trade::new(sym, UnixNanos(exec), Price::from_micros(micros), size, 1),
        VOL,
        UnixNanos(exec + 100_000_000),
    );
}

/// Evidence for a pass on every pillar given the close; `float` may be pushed over 20M to fail it.
fn evidence(e: &mut Engine, sym: &str, close: i64, float: u64) {
    e.add_prev_close(PrevClose {
        id: format!("close:{sym}"),
        version: 1,
        instrument: inst(sym),
        date: prev(),
        close_micros: close,
        basis: CloseBasis::SplitAdjusted,
        actions_checked_through: Some(day()),
        actions: Default::default(),
        observed_at: t(4, 0, 0),
    });
    let mut sessions = Vec::new();
    let mut d = prev();
    while sessions.len() < 30 {
        if market_events::is_trading_day(d) {
            sessions.push(SessionVolume {
                date: d,
                volume: 1_000,
                complete: true,
                comparable: true,
            });
        }
        d = d.pred_opt().unwrap();
    }
    e.add_daily(DailyBaseline {
        id: format!("daily:{sym}"),
        version: 1,
        instrument: inst(sym),
        sessions,
        basis: Default::default(),
        observed_at: t(4, 0, 0),
    });
    e.add_float(FloatObservation {
        id: format!("float:{sym}"),
        version: 1,
        instrument: inst(sym),
        shares: float,
        provider: "fixture".into(),
        effective_as_of: None,
        observed_at: t(4, 0, 0),
        quality: vec![],
    });
    e.add_news(NewsItem {
        id: format!("news:{sym}"),
        version: 1,
        instrument: inst(sym),
        event_type: "other".into(),
        source: "issuer".into(),
        credible_source: true,
        relevant: true,
        substantive: true,
        authenticity: Authenticity::IssuerAnnounced,
        favorability: Favorability::Favorable,
        published_at: Some(t(7, 0, 0)),
        observed_at: t(7, 0, 1),
        tag: Tag::Present,
        content_key: "fixture".into(),
        revision_at: Some(0),
        rules: "fixture".into(),
    });
}

/// A print every 5 s from `from` to `to` (exclusive), price moving linearly from `p0` to `p1`.
fn walk(e: &mut Engine, sym: &str, from: i64, to: i64, p0: i64, p1: i64) {
    let n = (to - from) / (5 * S);
    for k in 0..n {
        let p = p0 + (p1 - p0) * k / n.max(1);
        print(e, sym, from + k * 5 * S, p, 1_000);
    }
}

fn row<'a>(p: &'a PanelSnapshot, sym: &str) -> &'a PanelRow {
    p.rows
        .iter()
        .find(|r| r.symbol == sym)
        .unwrap_or_else(|| panic!("{sym} not on the panel"))
}

#[test]
fn stronger_last_minute_ranks_first_whatever_the_day_gain_or_pillar_count() {
    let mut e = engine();
    // A: close $4, trades near $5.80 (+45 % on the day), falling 2 % in the last minute.
    evidence(&mut e, "AAAA", 4_000_000, 5_000_000);
    // B: close $5, trades near $5.90 (+18 %), rising 3 % in the last minute; float 30M fails it
    // later, so B is retained at 4/5 while A stays 5/5.
    evidence(&mut e, "BBBB", 5_000_000, 5_000_000);
    e.add_float(FloatObservation {
        id: "float:BBBB".into(),
        version: 2,
        instrument: inst("BBBB"),
        shares: 30_000_000,
        provider: "fixture".into(),
        effective_as_of: None,
        observed_at: t(9, 34, 0),
        quality: vec![],
    });
    // Prints in time order, both stocks interleaved: flat for five minutes, then the last minute.
    for k in 0..72 {
        let at = t(9, 30, 0) + k * 5 * S;
        let (pa, pb) = if k < 60 {
            (5_800_000, 5_900_000)
        } else {
            (5_800_000 - (k - 60) * 9_666, 5_900_000 + (k - 60) * 14_750)
        };
        print(&mut e, "AAAA", at, pa, 1_000);
        print(&mut e, "BBBB", at + 2 * S, pb, 1_000);
    }
    e.advance_to(UnixNanos(t(9, 36, 3)));
    let p = e.panel().expect("a panel").clone();
    assert_eq!(p.bar_end, t(9, 36, 0));
    let (a, b) = (row(&p, "AAAA"), row(&p, "BBBB"));
    assert_eq!(a.membership.status, Some(MemberStatus::Qualified));
    assert_eq!(b.membership.status, Some(MemberStatus::Retained));
    assert_eq!(
        b.membership
            .nonpassing
            .iter()
            .map(|x| x.0)
            .collect::<Vec<_>>(),
        vec![PillarId::Float]
    );
    assert!(a.gain_bp.unwrap() > b.gain_bp.unwrap());
    assert!(a.momentum.r_primary_bp.unwrap() < 0 && b.momentum.r_primary_bp.unwrap() > 0);
    assert_eq!((b.rank, a.rank), (Some(1), Some(2)), "B above A");
    assert!(
        p.panel_policy.starts_with("ross-panel-v1@") && p.pillar_policy.starts_with("pillars-v1@")
    );
}

#[test]
fn thin_member_is_unranked_below_ranked_rows_not_zeroed() {
    let mut e = engine();
    evidence(&mut e, "LIVE", 5_000_000, 5_000_000);
    evidence(&mut e, "THIN", 5_000_000, 5_000_000);
    walk(
        &mut e,
        "LIVE",
        t(9, 30, 0),
        t(9, 33, 0),
        5_600_000,
        5_700_000,
    );
    // THIN prints once, enough to qualify, then goes quiet.
    print(&mut e, "THIN", t(9, 30, 1), 5_600_000, 60_000);
    e.advance_to(UnixNanos(t(9, 33, 3)));
    let p = e.panel().unwrap();
    assert_eq!(p.rows[0].symbol, "LIVE");
    assert_eq!(p.rows[0].rank, Some(1));
    let thin = row(p, "THIN");
    assert_eq!(thin.rank, None);
    assert_eq!(thin.momentum.status, MomentumStatus::Unavailable);
    assert_eq!(thin.momentum.r_primary_bp, None);
    // Still a member: momentum never decides membership.
    assert!(thin.membership.active().is_some());
}

#[test]
fn leaving_the_platform_band_removes_and_reentry_needs_five() {
    let mut e = engine();
    evidence(&mut e, "ABCD", 1_000_000, 5_000_000);
    print(&mut e, "ABCD", t(9, 30, 1), 2_000_000, 60_000); // +100 %, 5/5
    // $1.50: the $2-$20 price pillar fails, the stock is still in the $0.10-$25 platform band.
    print(&mut e, "ABCD", t(9, 30, 31), 1_500_000, 10);
    e.advance_to(UnixNanos(t(9, 31, 3)));
    let r = row(e.panel().unwrap(), "ABCD").clone();
    assert_eq!(r.membership.status, Some(MemberStatus::Retained));
    assert_eq!(r.membership.nonpassing[0].0, PillarId::Price);
    // $26: out of the platform band, out of the panel whatever the count.
    print(&mut e, "ABCD", t(9, 31, 31), 26_000_000, 10);
    e.advance_to(UnixNanos(t(9, 32, 3)));
    assert!(e.panel().unwrap().rows.is_empty());
    // Back to $3 (4/5 at most is not enough… here 5/5 again): a second episode.
    print(&mut e, "ABCD", t(9, 32, 31), 3_000_000, 10);
    let ds = e.advance_to(UnixNanos(t(9, 33, 3)));
    let m = &ds
        .iter()
        .rev()
        .find(|d| d.symbol == "ABCD")
        .unwrap()
        .membership;
    assert_eq!(m.episodes.len(), 2);
    assert_eq!(
        m.episodes[0].exit_reason,
        Some(ExitReason::LeftPlatformBand)
    );
    assert_eq!(m.episodes[1].id, "fixture:2026-10-16:ABCD:2");
}

#[test]
fn halted_member_keeps_its_place_but_loses_its_rank() {
    let mut e = engine();
    evidence(&mut e, "ABCD", 5_000_000, 5_000_000);
    walk(
        &mut e,
        "ABCD",
        t(9, 30, 0),
        t(9, 32, 0),
        5_600_000,
        5_800_000,
    );
    e.advance_to(UnixNanos(t(9, 32, 3)));
    assert_eq!(row(e.panel().unwrap(), "ABCD").rank, Some(1));
    e.set_halted("ABCD", true);
    e.advance_to(UnixNanos(t(9, 32, 18)));
    let r = row(e.panel().unwrap(), "ABCD");
    assert_eq!((r.rank, r.momentum.reason), (None, "halted"));
    assert!(r.membership.active().is_some());
}

#[test]
fn no_member_is_an_empty_panel_and_a_new_date_needs_a_new_admission() {
    let mut e = engine();
    // No evidence: nothing can pass five pillars, so the panel stays empty (no relaxed admission).
    walk(
        &mut e,
        "ABCD",
        t(9, 30, 0),
        t(9, 31, 0),
        5_600_000,
        5_700_000,
    );
    e.advance_to(UnixNanos(t(9, 31, 3)));
    assert!(e.panel().is_none_or(|p| p.rows.is_empty()));

    let mut e = engine();
    evidence(&mut e, "ABCD", 5_000_000, 5_000_000);
    walk(
        &mut e,
        "ABCD",
        t(19, 58, 0),
        t(19, 59, 50),
        5_600_000,
        5_700_000,
    );
    e.advance_to(UnixNanos(t(19, 59, 53)));
    assert!(
        row(e.panel().unwrap(), "ABCD")
            .membership
            .active()
            .is_some()
    );
    // 20:00 ET opens the next trading date: the member is gone until admitted again.
    e.advance_to(UnixNanos(t(20, 0, 18)));
    assert!(e.panel().unwrap().rows.is_empty());
}

fn scenario(e: &mut Engine) -> (Vec<String>, Vec<String>) {
    evidence(e, "AAAA", 4_000_000, 5_000_000);
    evidence(e, "BBBB", 5_000_000, 5_000_000);
    let mut ds = Vec::new();
    let mut k = 0;
    let mut at = t(9, 30, 0);
    while at < t(9, 50, 0) {
        k += 1;
        let wobble = (k % 9) as i64 * 7_000;
        e.apply(
            &Trade::new(
                "AAAA",
                UnixNanos(at),
                Price::from_micros(5_800_000 + wobble),
                900,
                k,
            ),
            VOL,
            UnixNanos(at + 80_000_000),
        )
        .into_iter()
        .for_each(|d| ds.push(d));
        e.apply(
            &Trade::new(
                "BBBB",
                UnixNanos(at + 2 * S),
                Price::from_micros(5_900_000 - wobble),
                700,
                k,
            ),
            VOL,
            UnixNanos(at + 2 * S + 90_000_000),
        )
        .into_iter()
        .for_each(|d| ds.push(d));
        at += 7 * S;
    }
    ds.extend(e.advance_to(UnixNanos(t(9, 51, 0))));
    let panels = e.take_panels();
    (
        ds.iter()
            .map(|d| serde_json::to_string(d).unwrap())
            .collect(),
        panels
            .iter()
            .map(|p| serde_json::to_string(p).unwrap())
            .collect(),
    )
}

#[test]
fn membership_and_ranking_replay_identically() {
    let a = scenario(&mut engine());
    let b = scenario(&mut engine());
    assert!(a.1.len() > 20, "panels {}", a.1.len());
    assert_eq!(a, b);
}
