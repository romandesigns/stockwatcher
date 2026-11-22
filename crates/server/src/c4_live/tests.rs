use super::*;
use crate::rocket::{AlertKind, NoticeDetail, NoticeKind};
use chrono::NaiveDate;

fn et(d: NaiveDate, h: u32, m: u32) -> i64 {
    market_events::session::et(d, h, m, 0, 0).unwrap().0
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 18).unwrap()
}

fn gauge(bull: bool, at_ns: i64, price: f64) -> Notice {
    Notice {
        spread_bp: None,
        no_quote: false,
        kind: NoticeKind::Halt,
        alert_id: format!("ABCD-h{}", at_ns / 1_000_000_000),
        symbol: "ABCD".into(),
        at_ns,
        price,
        from_flag_pct: None,
        reasons: Vec::new(),
        since_start_min: None,
        exec_ns: None,
        detail: Some(NoticeDetail::Halt {
            bull,
            risk: 70,
            band: price * 1.1,
            secs: Some(40),
        }),
    }
}

#[test]
fn the_halt_addon_keeps_bull_gauges_in_universe_inside_the_gate() {
    let t = et(day(), 9, 45);
    let fresh = Some((t - 2_000_000_000, 2.5));
    let n = halt_addon(&gauge(true, t, 2.5), fresh, None).expect("kept");
    assert_eq!(n.kind, NoticeKind::HaltBull);
    assert_eq!(n.at_ns, t);
    assert_eq!(
        n.alert_id,
        format!("2026-11-18-ABCD-hb{}", t / 1_000_000_000)
    );
    assert_ne!(n.event_id(), gauge(true, t, 2.5).event_id());
    // Bear, outside 09:30–11:30, a stale or out-of-band price, an excluded type: none.
    assert!(halt_addon(&gauge(false, t, 2.5), fresh, None).is_none());
    let early = et(day(), 9, 29);
    assert!(halt_addon(&gauge(true, early, 2.5), Some((early - 1, 2.5)), None).is_none());
    let late = et(day(), 11, 30);
    assert!(halt_addon(&gauge(true, late, 2.5), Some((late - 1, 2.5)), None).is_none());
    assert!(halt_addon(&gauge(true, t, 2.5), Some((t - 61_000_000_000, 2.5)), None).is_none());
    assert!(halt_addon(&gauge(true, t, 26.0), Some((t - 1, 26.0)), None).is_none());
    assert!(halt_addon(&gauge(true, t, 2.5), None, None).is_none());
    let mut types = FactsTable::default();
    types.types.insert("ABCD".into(), SecType::Excluded);
    assert!(halt_addon(&gauge(true, t, 2.5), fresh, Some(&types)).is_none());
}

#[test]
fn post1_prunes_post_cross_notices_under_a_dollar_only() {
    let base = gauge(true, et(day(), 10, 0), 0.8);
    let with = |kind, price, detail| Notice {
        spread_bp: None,
        no_quote: false,
        kind,
        price,
        detail,
        ..base.clone()
    };
    let fu = |path, first| {
        Some(NoticeDetail::FollowUp {
            path,
            first,
            first_at_ns: 0,
        })
    };
    assert!(prunes_post1(&with(NoticeKind::Run, 0.8, None)));
    assert!(prunes_post1(&with(NoticeKind::Shelf, 0.99, None)));
    assert!(prunes_post1(&with(
        NoticeKind::FollowUp,
        0.5,
        fu(AlertKind::Run, AlertKind::Fast)
    )));
    assert!(prunes_post1(&with(
        NoticeKind::FollowUp,
        0.5,
        fu(AlertKind::Running, AlertKind::Run)
    )));
    // $1 and up, or not post-cross: kept.
    assert!(!prunes_post1(&with(NoticeKind::Run, 1.0, None)));
    assert!(!prunes_post1(&with(
        NoticeKind::FollowUp,
        0.5,
        fu(AlertKind::Running, AlertKind::Fast)
    )));
    for k in [
        NoticeKind::New,
        NoticeKind::Fast,
        NoticeKind::Running,
        NoticeKind::Burst,
        NoticeKind::Pullback,
        NoticeKind::N1,
        NoticeKind::F1u,
    ] {
        assert!(!prunes_post1(&with(k, 0.5, None)), "{k:?}");
    }
}

#[test]
fn a_day_not_watched_from_four_am_never_notifies() {
    let mut f = F1uLive::new();
    f.tick(et(day(), 9, 0));
    assert!(!f.complete());
    let mut f = F1uLive::new();
    f.tick(et(day(), 3, 59));
    assert!(f.complete());
    assert_eq!(f.take_facts_request(), Some(day()));
    assert_eq!(f.take_facts_request(), None);
}
