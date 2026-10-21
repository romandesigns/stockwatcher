//! Stored daily bars → stock-context evidence (market_archive::baseline), on synthetic bars around
//! Thanksgiving 2026 (holiday 11-26, early close 11-27). Evaluated date D = 2026-12-10, P = 12-09.

use chrono::{DateTime, NaiveDate, Utc};
use market_archive::baseline::{Inputs, METRIC, load};
use market_archive::{Adjustment, BarFetch, DailyBar, RawDayBars, SplitSnapshot};
use market_events::session::{is_trading_day, previous_trading_day};
use std::collections::BTreeMap;
use stock_context::CloseBasis;

const SYM: &str = "XYZ";

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn at(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().to_utc()
}

fn fetch(adj: Adjustment, from: NaiveDate, to: NaiveDate, when: &str) -> BarFetch {
    BarFetch {
        retrieved_at: at(when),
        provider: "alpaca".into(),
        feed: "sip".into(),
        adjustment: adj,
        timeframe: "1Day".into(),
        from,
        to,
        symbols: 1,
        symbols_per_request: 200,
        requests: 1,
        asof: None,
        command: "test".into(),
    }
}

/// D, P and the evaluated-date constants.
fn dd() -> NaiveDate {
    d("2026-12-10")
}

/// After D's after-hours session (20:00 ET = 01:00Z 12-11).
const AFTER_D: &str = "2026-12-11T03:00:00Z";
/// The night of P (22:00 ET 12-09): what a live run on D would have.
const NIGHT_OF_P: &str = "2026-12-10T03:00:00Z";
/// Long after every fetch: bar-only tests see everything.
const LATE: &str = "2027-01-31T00:00:00Z";

fn policy() -> market_archive::CoveragePolicy {
    market_archive::CoveragePolicy::v1()
}

fn trading_dates(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    from.iter_days()
        .take_while(|x| *x <= to)
        .filter(|x| is_trading_day(*x))
        .collect()
}

/// Raw volume of a date: distinct per date so a wrong date or a double count shows.
fn vol(x: NaiveDate) -> u64 {
    1_000_000 + u64::from(x.format("%j").to_string().parse::<u32>().unwrap())
}

/// Synthetic market: XYZ trades every trading date in `from..=to`, close `pre` before `split_on`
/// and `post` from it on (raw, as printed).
struct Market {
    from: NaiveDate,
    to: NaiveDate,
    split_on: Option<NaiveDate>,
    /// Cumulative split factor f = raw/adjusted before `split_on` (0.1 = 1:10 reverse split).
    f: f64,
}

impl Market {
    fn plain(from: &str, to: &str) -> Self {
        Market {
            from: d(from),
            to: d(to),
            split_on: None,
            f: 1.0,
        }
    }

    fn raw_bar(&self, x: NaiveDate) -> DailyBar {
        let before = self.split_on.is_some_and(|s| x < s);
        let c = if before { 0.5 } else { 5.0 };
        DailyBar {
            o: Some(c),
            h: Some(c),
            l: Some(c),
            c,
            v: Some(vol(x)),
            n: Some(100),
            vw: Some(c),
        }
    }

    fn k(&self, x: NaiveDate) -> f64 {
        if self.split_on.is_some_and(|s| x < s) {
            self.f
        } else {
            1.0
        }
    }

    /// Raw date files, each fetched at `when` (dates after D get a later fetch: a later night).
    fn raw(&self, when: &str) -> BTreeMap<NaiveDate, RawDayBars> {
        let mut out = BTreeMap::new();
        for x in trading_dates(self.from, self.to) {
            let w = if x > dd() {
                "2026-12-20T03:00:00Z"
            } else {
                when
            };
            let mut day = RawDayBars::new(x, fetch(Adjustment::Raw, self.from, self.to, w));
            day.bars.insert(SYM.into(), self.raw_bar(x));
            out.insert(x, day);
        }
        out
    }

    /// The split-adjusted snapshot of `from..=to` fetched at `when` (bars up to `to` only).
    fn split(&self, to: NaiveDate, when: &str) -> SplitSnapshot {
        let mut bars = BTreeMap::new();
        for x in trading_dates(self.from, to) {
            let r = self.raw_bar(x);
            let k = self.k(x);
            bars.insert(
                x,
                DailyBar {
                    c: r.c / k,
                    o: r.o.map(|p| p / k),
                    h: r.h.map(|p| p / k),
                    l: r.l.map(|p| p / k),
                    vw: r.vw.map(|p| p / k),
                    v: r.v.map(|v| (v as f64 * k).round() as u64),
                    n: r.n,
                },
            );
        }
        SplitSnapshot {
            schema: 1,
            fetch: fetch(Adjustment::Split, self.from, to, when),
            symbols: vec![SYM.into()],
            bars: BTreeMap::from([(SYM.to_string(), bars)]),
            set: None,
        }
    }
}

fn comparable(l: &market_archive::baseline::Loaded) -> usize {
    l.baseline
        .as_ref()
        .unwrap()
        .sessions
        .iter()
        .filter(|s| s.complete && s.comparable)
        .count()
}

#[test]
fn no_split_checked_through_d_and_early_close_and_holiday_handled() {
    let m = Market::plain("2026-09-01", "2026-12-15");
    let raw = m.raw(AFTER_D);
    let snap = m.split(dd(), AFTER_D);
    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &[],
            policy: &policy(),
            as_of: at(LATE),
        },
        SYM,
        dd(),
        45,
    );
    let pc = l.prev_close.as_ref().unwrap();
    assert_eq!(pc.date, d("2026-12-09"));
    assert_eq!(pc.close_micros, 5_000_000);
    assert_eq!(pc.basis, CloseBasis::Raw);
    assert_eq!(pc.actions_checked_through, Some(dd()));
    assert_eq!(l.prev_close_reason, "bar_verified_through_evaluated_date");
    assert_eq!(pc.id, format!("{METRIC}:prev_close:{SYM}:2026-12-10"));

    let b = l.baseline.as_ref().unwrap();
    assert_eq!(b.sessions.len(), 45);
    // Thanksgiving is not a session; the day after (13:00 close) is present but not comparable.
    assert!(b.sessions.iter().all(|s| s.date != d("2026-11-26")));
    let early = b
        .sessions
        .iter()
        .find(|s| s.date == d("2026-11-27"))
        .unwrap();
    assert!(early.complete && !early.comparable);
    assert_eq!(comparable(&l), 44);
    assert_eq!(l.baseline_reason, "ok");
    // Volumes are the raw ones, exactly (no split between any session and D).
    assert!(b.sessions.iter().all(|s| s.volume == vol(s.date)));
    // observed_at: the latest retrieval used, in nanoseconds.
    assert_eq!(
        b.observed_at,
        at(AFTER_D).timestamp_nanos_opt().unwrap(),
        "evidence is visible only from its retrieval"
    );
}

#[test]
fn sessions_on_or_after_the_evaluated_date_are_never_used() {
    let m = Market::plain("2026-09-01", "2026-12-15");
    let raw = m.raw(AFTER_D);
    let snap = m.split(d("2026-12-15"), "2026-12-20T03:00:00Z");
    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &[],
            policy: &policy(),
            as_of: at(LATE),
        },
        SYM,
        dd(),
        45,
    );
    let b = l.baseline.unwrap();
    assert!(b.sessions.iter().all(|s| s.date < dd()));
    assert_eq!(b.sessions.last().unwrap().date, d("2026-12-09"));
    // A session list for D+1 sees D, and only up to D.
    let l2 = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &[],
            policy: &policy(),
            as_of: at(LATE),
        },
        SYM,
        d("2026-12-11"),
        45,
    );
    assert_eq!(
        l2.baseline.unwrap().sessions.last().unwrap().date,
        dd(),
        "D is a session only for later dates"
    );
}

#[test]
fn twenty_nine_comparable_sessions_are_not_thirty() {
    // Raw history begins so that exactly 30 comparable sessions precede D (the early close of
    // 11-27 is in range and does not count), then one date later: 29.
    let mut start = dd();
    let mut n = 0;
    while n < 30 {
        start = previous_trading_day(start);
        if start != d("2026-11-27") {
            n += 1;
        }
    }
    for (from, want, reason) in [
        (start, 30, "ok"),
        (start.succ_opt().unwrap(), 29, "fewer_than_30_comparable"),
    ] {
        let mut m = Market::plain("2026-09-01", "2026-12-15");
        m.from = from;
        let raw = m.raw(AFTER_D);
        let snap = m.split(dd(), AFTER_D);
        let l = load(
            &Inputs {
                raw: &raw,
                split: Some(&snap),
                announcements: &[],
                policy: &policy(),
                as_of: at(LATE),
            },
            SYM,
            dd(),
            45,
        );
        assert_eq!(comparable(&l), want);
        assert_eq!(l.baseline_reason, reason);
    }
}

#[test]
fn missing_dates_are_missing_not_zero() {
    let m = Market::plain("2026-09-01", "2026-12-15");
    let mut raw = m.raw(AFTER_D);
    let mut snap = m.split(dd(), AFTER_D);
    // 12-01: covered, no bar on either side (halted all day). 12-02: never requested.
    raw.get_mut(&d("2026-12-01")).unwrap().bars.clear();
    raw.get_mut(&d("2026-12-01"))
        .unwrap()
        .no_bar
        .insert(SYM.into());
    snap.bars.get_mut(SYM).unwrap().remove(&d("2026-12-01"));
    raw.remove(&d("2026-12-02"));
    // 12-03: the raw file was fetched before 12-03's session ended: not covered.
    let mut early = raw.remove(&d("2026-12-03")).unwrap();
    early.fetches[0].retrieved_at = at("2026-12-03T20:00:00Z");
    raw.insert(d("2026-12-03"), early);
    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &[],
            policy: &policy(),
            as_of: at(LATE),
        },
        SYM,
        dd(),
        45,
    );
    let b = l.baseline.unwrap();
    for gone in ["2026-12-01", "2026-12-02", "2026-12-03"] {
        assert!(
            b.sessions.iter().all(|s| s.date != d(gone)),
            "{gone} must be absent, not a zero"
        );
    }
    assert!(b.sessions.iter().all(|s| s.volume > 0));
    assert_eq!(b.sessions.len(), 42);
}

#[test]
fn a_reverse_split_on_d_restates_the_close_and_the_volumes_onto_ds_basis() {
    // 1:10 reverse split effective D: raw P close $0.50, raw D close $5.00. Against an unchecked
    // raw close a $5.00 print reads +900 %.
    let m = Market {
        split_on: Some(dd()),
        f: 0.1,
        ..Market::plain("2026-09-01", "2026-12-15")
    };
    let raw = m.raw(AFTER_D);
    let snap = m.split(dd(), AFTER_D);
    // Split-adjusted and raw differ before D.
    let p = d("2026-12-09");
    assert_eq!(raw[&p].bars[SYM].c, 0.5);
    assert_eq!(snap.bars[SYM][&p].c, 5.0);
    assert_eq!(
        snap.bars[SYM][&p].v,
        Some((vol(p) as f64 * 0.1).round() as u64)
    );

    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &[],
            policy: &policy(),
            as_of: at(LATE),
        },
        SYM,
        dd(),
        45,
    );
    let pc = l.prev_close.unwrap();
    assert_eq!(pc.basis, CloseBasis::SplitAdjusted);
    assert_eq!(pc.close_micros, 5_000_000, "on D's basis");
    assert_eq!(pc.actions_checked_through, Some(dd()));
    let b = l.baseline.unwrap();
    assert!(
        b.sessions
            .iter()
            .all(|s| s.volume == (vol(s.date) as f64 * 0.1).round() as u64),
        "prior volumes on D's (post-split) share basis"
    );

    // The same split seen from a live run on D: the snapshot fetched the night of P has no D bar,
    // so nothing can vouch for D's basis. The raw close is reported checked only through P (the
    // gain stays UNKNOWN) and there is no baseline (RVOL stays UNKNOWN).
    let live_raw = m.raw(NIGHT_OF_P);
    let live_raw: BTreeMap<_, _> = live_raw.into_iter().filter(|(x, _)| *x < dd()).collect();
    // What Alpaca would have served that night: no split taken effect yet, so raw == adjusted.
    let before = Market {
        split_on: None,
        ..Market::plain("2026-09-01", "2026-12-15")
    };
    let mut live_snap = before.split(p, NIGHT_OF_P);
    for b in live_snap.bars.get_mut(SYM).unwrap().values_mut() {
        b.c = 0.5;
        b.o = Some(0.5);
        b.h = Some(0.5);
        b.l = Some(0.5);
        b.vw = Some(0.5);
    }
    let live_raw: BTreeMap<_, _> = live_raw
        .into_iter()
        .map(|(x, mut day)| {
            day.bars.get_mut(SYM).unwrap().c = 0.5;
            (x, day)
        })
        .collect();
    let l = load(
        &Inputs {
            raw: &live_raw,
            split: Some(&live_snap),
            announcements: &[],
            policy: &policy(),
            as_of: at(LATE),
        },
        SYM,
        dd(),
        45,
    );
    let pc = l.prev_close.unwrap();
    assert_eq!((pc.basis, pc.close_micros), (CloseBasis::Raw, 500_000));
    assert_eq!(pc.actions_checked_through, Some(p));
    assert_eq!(l.prev_close_reason, "bar_verified_before_evaluated_date");
    assert!(l.baseline.is_none());
    assert_eq!(l.baseline_reason, "no_usable_retrieval");
}

#[test]
fn without_a_snapshot_or_when_the_sets_disagree_nothing_is_checked() {
    let m = Market::plain("2026-09-01", "2026-12-15");
    let raw = m.raw(AFTER_D);
    let l = load(
        &Inputs {
            raw: &raw,
            split: None,
            announcements: &[],
            policy: &policy(),
            as_of: at(LATE),
        },
        SYM,
        dd(),
        45,
    );
    let pc = l.prev_close.unwrap();
    assert_eq!(pc.actions_checked_through, None, "gain stays UNKNOWN");
    assert_eq!(pc.basis, CloseBasis::Raw);
    assert_eq!(l.prev_close_reason, "no_split_snapshot");
    assert!(l.baseline.is_none());
    assert_eq!(l.baseline_reason, "no_split_snapshot");

    // Coverage disagreement on P: the snapshot lacks P's bar that raw has.
    let mut snap = m.split(dd(), AFTER_D);
    snap.bars.get_mut(SYM).unwrap().remove(&d("2026-12-09"));
    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &[],
            policy: &policy(),
            as_of: at(LATE),
        },
        SYM,
        dd(),
        45,
    );
    assert_eq!(l.prev_close.unwrap().actions_checked_through, None);
    assert_eq!(l.prev_close_reason, "split_and_raw_disagree");

    // A price-only restatement (a dividend-style adjustment: price moved, volume did not) is not
    // a split adjustment: D is unverified, so neither record is checked.
    let mut snap = m.split(dd(), AFTER_D);
    snap.bars.get_mut(SYM).unwrap().get_mut(&dd()).unwrap().c = 4.9;
    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &[],
            policy: &policy(),
            as_of: at(LATE),
        },
        SYM,
        dd(),
        45,
    );
    assert_eq!(l.checked_through, Some(d("2026-12-09")));
    assert!(l.baseline.is_none());
}

// ---- live: announcement coverage (weaker evidence)
// ---- live: announcement coverage (corporate-actions-coverage-v1)

use market_archive::baseline::{ActionCoverage, UNCHANGED_BASIS_ASSUMED, coverage_for};
use market_archive::{ActionKind, ActionRetrieval, ChunkOutcome, CorporateAction};
use stock_context::{ActionsSource, CoverageState, SplitRatio};

/// 03:30 ET on D: the pre-session retrieval.
const PRE_SESSION_D: &str = "2026-12-10T08:30:00Z";
/// 04:45 ET on D: a pre-market decision 75 minutes later (freshness limit 90 min).
const DECISION: &str = "2026-12-10T09:45:00Z";

fn split(
    id: &str,
    kind: ActionKind,
    sym: &str,
    ex: Option<&str>,
    old: f64,
    new: f64,
) -> CorporateAction {
    CorporateAction {
        id: id.into(),
        kind,
        symbol: Some(sym.into()),
        new_symbol: None,
        alternate_symbol: None,
        effective_date: ex.map(d),
        process_date: ex.map(d),
        old_rate: Some(old),
        new_rate: Some(new),
        quality: None,
        complete: true,
        raw: serde_json::json!({"id": id}),
    }
}

fn rename(id: &str, old: &str, new: &str, on: &str) -> CorporateAction {
    CorporateAction {
        id: id.into(),
        kind: ActionKind::NameChange,
        symbol: Some(old.into()),
        new_symbol: Some(new.into()),
        alternate_symbol: None,
        effective_date: Some(d(on)),
        process_date: Some(d(on)),
        old_rate: None,
        new_rate: None,
        quality: None,
        complete: true,
        raw: serde_json::json!({"id": id}),
    }
}

fn retrieval(when: &str, covered: &[&str], actions: Vec<CorporateAction>) -> ActionRetrieval {
    let policy = policy();
    let (start, end) = policy.process_window(dd());
    let mut covered: Vec<String> = covered.iter().map(|s| s.to_string()).collect();
    covered.sort();
    ActionRetrieval {
        schema: 1,
        policy: policy.tag.clone(),
        retrieved_at: at(when),
        provider: "alpaca".into(),
        endpoint: policy.body.endpoint.clone(),
        types: policy.body.types.clone(),
        data_quality: "all".into(),
        session_date: dd(),
        process_start: start,
        process_end: end,
        symbols_requested: vec!["OTHER".into(), SYM.into()],
        symbols_covered: covered,
        chunks: vec![ChunkOutcome {
            symbols: vec!["OTHER".into(), SYM.into()],
            pages: 1,
            complete: true,
            error: None,
            followup: false,
        }],
        success: true,
        requests: 1,
        command: "test".into(),
        actions,
    }
}

fn reverse_1_for_10(ex: &str) -> CorporateAction {
    split("rs-1", ActionKind::ReverseSplit, SYM, Some(ex), 10.0, 1.0)
}

/// What a live run on D has: raw bars through P and a split snapshot, both fetched the night of
/// P. All prior closes are $0.50 and nothing had taken effect yet (adjusted == raw).
fn live_world() -> (BTreeMap<NaiveDate, RawDayBars>, SplitSnapshot) {
    let m = Market {
        split_on: Some(dd()),
        f: 1.0,
        ..Market::plain("2026-09-01", "2026-12-09")
    };
    (m.raw(NIGHT_OF_P), m.split(d("2026-12-09"), NIGHT_OF_P))
}

fn live(anns: &[ActionRetrieval], as_of: &str) -> market_archive::baseline::Loaded {
    let (raw, snap) = live_world();
    let p = policy();
    load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: anns,
            policy: &p,
            as_of: at(as_of),
        },
        SYM,
        dd(),
        45,
    )
}

#[test]
fn no_known_action_permits_unchanged_basis_and_says_it_is_an_assumption() {
    let anns = [retrieval(PRE_SESSION_D, &["OTHER", SYM], vec![])];
    let l = live(&anns, DECISION);
    let pc = l.prev_close.unwrap();
    assert_eq!((pc.basis, pc.close_micros), (CloseBasis::Raw, 500_000));
    assert_eq!(pc.actions_checked_through, Some(dd()));
    assert_eq!(
        pc.actions.source,
        ActionsSource::ProviderCheckedNoKnownAction
    );
    assert_eq!(
        pc.actions.coverage,
        Some(CoverageState::NoKnownActionProviderChecked)
    );
    assert_eq!(
        pc.actions.assumption.as_deref(),
        Some(UNCHANGED_BASIS_ASSUMED)
    );
    assert_eq!(
        pc.actions.coverage_policy.as_deref(),
        Some(policy().tag.as_str())
    );
    assert_eq!(pc.actions.bar_verified_through, Some(d("2026-12-09")));
    assert_eq!(pc.actions.basis_date, Some(dd()));
    assert_eq!(
        pc.observed_at,
        at(PRE_SESSION_D).timestamp_nanos_opt().unwrap()
    );
    let b = l.baseline.unwrap();
    assert_eq!(b.basis.source, ActionsSource::ProviderCheckedNoKnownAction);
    assert!(b.sessions.iter().all(|s| s.volume == vol(s.date)));
}

#[test]
fn a_confirmed_reverse_split_restates_price_up_and_volume_down() {
    let anns = [retrieval(
        PRE_SESSION_D,
        &["OTHER", SYM],
        vec![reverse_1_for_10("2026-12-10")],
    )];
    let l = live(&anns, DECISION);
    let pc = l.prev_close.unwrap();
    assert_eq!(pc.basis, CloseBasis::SplitAdjusted);
    assert_eq!(pc.close_micros, 5_000_000, "$0.50 x 10");
    assert_eq!(pc.actions.source, ActionsSource::AnnouncedAction);
    assert_eq!(
        pc.actions.applied_splits,
        [(
            SplitRatio {
                old_rate: 10,
                new_rate: 1
            },
            "rs-1".to_string()
        )]
    );
    let b = l.baseline.unwrap();
    assert!(
        b.sessions
            .iter()
            .all(|s| (s.volume as f64 - vol(s.date) as f64 / 10.0).abs() <= 0.5)
    );
}

#[test]
fn a_split_with_a_rename_reads_history_under_the_old_ticker() {
    // OLD did a 1-for-10 reverse split and became XYZ on D (Alpaca: reverse split with
    // new_symbol). The history (raw bars, snapshot) is OLD's.
    let m = Market {
        split_on: Some(dd()),
        f: 1.0,
        ..Market::plain("2026-09-01", "2026-12-09")
    };
    let mut raw = m.raw(NIGHT_OF_P);
    for day in raw.values_mut() {
        let b = day.bars.remove(SYM).unwrap();
        day.bars.insert("OLD".into(), b);
    }
    let mut snap = m.split(d("2026-12-09"), NIGHT_OF_P);
    snap.symbols = vec!["OLD".into()];
    let b = snap.bars.remove(SYM).unwrap();
    snap.bars.insert("OLD".into(), b);
    let mut rs = split(
        "rs-old",
        ActionKind::ReverseSplit,
        "OLD",
        Some("2026-12-10"),
        10.0,
        1.0,
    );
    rs.new_symbol = Some(SYM.into());
    let p = policy();
    let mk = |covered: &[&str]| [retrieval(PRE_SESSION_D, covered, vec![rs.clone()])];
    let anns = mk(&["OLD", "OTHER", SYM]);
    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &anns,
            policy: &p,
            as_of: at(DECISION),
        },
        SYM,
        dd(),
        45,
    );
    let pc = l.prev_close.unwrap();
    assert_eq!(pc.close_micros, 5_000_000);
    assert_eq!(pc.actions.history_symbol.as_deref(), Some("OLD"));
    assert_eq!(
        l.baseline.unwrap().basis.history_symbol.as_deref(),
        Some("OLD")
    );
    // The old ticker not covered by the check: the chain is not vouched for.
    let anns = mk(&["OTHER", SYM]);
    assert_eq!(
        coverage_for(&anns, &p, SYM, d("2026-12-09"), dd(), at(DECISION)).reason(),
        "rename_chain_not_covered"
    );
    // A plain rename (no split) into XYZ: same shares, history under OLD.
    let anns = [retrieval(
        PRE_SESSION_D,
        &["OLD", "OTHER", SYM],
        vec![rename("nc", "OLD", SYM, "2026-12-10")],
    )];
    match coverage_for(&anns, &p, SYM, d("2026-12-09"), dd(), at(DECISION)) {
        ActionCoverage::Confirmed {
            splits,
            history_symbol,
            ..
        } => {
            assert!(splits.is_empty());
            assert_eq!(history_symbol.as_deref(), Some("OLD"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_coverage_state_is_distinguished_and_only_two_permit_a_calculation() {
    let p = d("2026-12-09");
    let mut incomplete = reverse_1_for_10("2026-12-10");
    incomplete.complete = false;
    let undated = split("u", ActionKind::ForwardSplit, SYM, None, 1.0, 2.0);
    let mut unit = split(
        "unit",
        ActionKind::UnitSplit,
        SYM,
        Some("2026-12-10"),
        1.0,
        1.0,
    );
    unit.alternate_symbol = Some("XYZW".into());
    let conflicting = split(
        "rs-2",
        ActionKind::ReverseSplit,
        SYM,
        Some("2026-12-10"),
        20.0,
        1.0,
    );
    let fractional = split(
        "f",
        ActionKind::ForwardSplit,
        SYM,
        Some("2026-12-10"),
        2.0,
        3.5,
    );
    let cases: Vec<(Vec<ActionRetrieval>, &str, &str, CoverageState)> = vec![
        (
            vec![],
            DECISION,
            "no_usable_retrieval",
            CoverageState::UnavailableCoverage,
        ),
        // Made before P's session ended.
        (
            vec![retrieval("2026-12-09T20:00:00Z", &["OTHER", SYM], vec![])],
            DECISION,
            "no_usable_retrieval",
            CoverageState::UnavailableCoverage,
        ),
        // Made after the decision time: never used (no leakage).
        (
            vec![retrieval("2026-12-10T15:00:00Z", &["OTHER", SYM], vec![])],
            DECISION,
            "no_usable_retrieval",
            CoverageState::UnavailableCoverage,
        ),
        // The symbol's chunk failed: other symbols unaffected, this one unavailable.
        (
            vec![retrieval(PRE_SESSION_D, &["OTHER"], vec![])],
            DECISION,
            "no_usable_retrieval",
            CoverageState::UnavailableCoverage,
        ),
        // Last check 10.5 h before the decision: older than the 90-minute freshness limit.
        (
            vec![retrieval(PRE_SESSION_D, &["OTHER", SYM], vec![])],
            "2026-12-10T19:00:00Z",
            "stale_coverage",
            CoverageState::StaleCoverage,
        ),
        (
            vec![retrieval(PRE_SESSION_D, &["OTHER", SYM], vec![incomplete])],
            DECISION,
            "incomplete_action",
            CoverageState::UnresolvedAction,
        ),
        (
            vec![retrieval(PRE_SESSION_D, &["OTHER", SYM], vec![undated])],
            DECISION,
            "undated_action",
            CoverageState::UnresolvedAction,
        ),
        (
            vec![retrieval(PRE_SESSION_D, &["OTHER", SYM], vec![unit])],
            DECISION,
            "unit_split",
            CoverageState::UnresolvedAction,
        ),
        (
            vec![retrieval(
                PRE_SESSION_D,
                &["OTHER", SYM],
                vec![reverse_1_for_10("2026-12-10"), conflicting],
            )],
            DECISION,
            "conflicting_actions",
            CoverageState::UnresolvedAction,
        ),
        (
            vec![retrieval(PRE_SESSION_D, &["OTHER", SYM], vec![fractional])],
            DECISION,
            "non_integral_ratio",
            CoverageState::UnresolvedAction,
        ),
        // An earlier check listed a split the latest one omits: never read as "gone".
        (
            vec![
                retrieval(
                    PRE_SESSION_D,
                    &["OTHER", SYM],
                    vec![reverse_1_for_10("2026-12-10")],
                ),
                retrieval("2026-12-10T09:15:00Z", &["OTHER", SYM], vec![]),
            ],
            DECISION,
            "action_missing_from_latest_check",
            CoverageState::UnresolvedAction,
        ),
    ];
    for (anns, as_of, why, state) in cases {
        let c = coverage_for(&anns, &policy(), SYM, p, dd(), at(as_of));
        assert_eq!((c.reason(), c.state()), (why, state), "{why}");
        let l = live(&anns, as_of);
        let pc = l.prev_close.unwrap();
        assert_eq!(
            pc.actions_checked_through,
            Some(p),
            "{why}: bars reach P only"
        );
        assert_eq!(pc.actions.coverage, Some(state));
        assert!(l.baseline.is_none(), "{why}");
    }
    // A split effective after D, or a duplicate listing of the same terms, is no obstacle.
    let anns = [retrieval(
        PRE_SESSION_D,
        &["OTHER", SYM],
        vec![reverse_1_for_10("2026-12-11")],
    )];
    assert_eq!(
        coverage_for(&anns, &policy(), SYM, p, dd(), at(DECISION)).state(),
        CoverageState::NoKnownActionProviderChecked
    );
    let mut dup = reverse_1_for_10("2026-12-10");
    dup.id = "rs-dup".into();
    let anns = [retrieval(
        PRE_SESSION_D,
        &["OTHER", SYM],
        vec![reverse_1_for_10("2026-12-10"), dup],
    )];
    match coverage_for(&anns, &policy(), SYM, p, dd(), at(DECISION)) {
        ActionCoverage::Confirmed {
            price_multiplier, ..
        } => {
            assert!((price_multiplier - 10.0).abs() < 1e-9)
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn bar_verification_wins_over_an_announcement_when_both_exist() {
    let m = Market::plain("2026-09-01", "2026-12-15");
    let raw = m.raw(AFTER_D);
    let snap = m.split(dd(), AFTER_D);
    let anns = [retrieval(PRE_SESSION_D, &["OTHER", SYM], vec![])];
    let p = policy();
    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &anns,
            policy: &p,
            as_of: at(LATE),
        },
        SYM,
        dd(),
        45,
    );
    assert_eq!(
        l.prev_close.unwrap().actions.source,
        ActionsSource::BarVerified
    );
    assert_eq!(l.baseline.unwrap().basis.source, ActionsSource::BarVerified);
}

#[test]
fn a_late_discovered_action_is_recorded_with_the_records_it_affected() {
    use market_archive::corporate_actions::late_actions;
    let mut ledger = market_archive::ActionLedger::new();
    // Known before it took effect: not late.
    let early = retrieval(
        PRE_SESSION_D,
        &["OTHER", SYM],
        vec![reverse_1_for_10("2026-12-10")],
    );
    let ids = |a: &CorporateAction, f: NaiveDate, t: NaiveDate| {
        market_archive::baseline::record_ids(a.symbol.as_deref().unwrap_or(""), f, t)
    };
    assert!(late_actions(&mut ledger, &early, &ids).is_empty());
    // A split effective 12-08 first seen at 03:30 on 12-10: late; the 12-08 and 12-09 records
    // (and today's) are named, nothing is rewritten.
    let mut back = split(
        "rs-late",
        ActionKind::ReverseSplit,
        SYM,
        Some("2026-12-08"),
        5.0,
        1.0,
    );
    back.raw = serde_json::json!({"id": "rs-late", "v": 1});
    let r = retrieval(PRE_SESSION_D, &["OTHER", SYM], vec![back.clone()]);
    let late = late_actions(&mut ledger, &r, &ids);
    assert_eq!(late.len(), 1);
    assert_eq!(late[0].kind, "new");
    assert!(
        late[0]
            .affected_records
            .contains(&format!("{METRIC}:prev_close:{SYM}:2026-12-08"))
    );
    assert!(
        late[0]
            .affected_records
            .contains(&format!("{METRIC}:baseline:{SYM}:2026-12-10"))
    );
    // Its terms change in a later version: a second late entry; the ledger keeps both versions.
    back.old_rate = Some(4.0);
    back.raw = serde_json::json!({"id": "rs-late", "v": 2});
    let r2 = retrieval("2026-12-10T10:00:00Z", &["OTHER", SYM], vec![back]);
    let late = late_actions(&mut ledger, &r2, &ids);
    assert_eq!(late[0].kind, "changed");
    assert_eq!(ledger["rs-late"].versions.len(), 2);
    assert_eq!(ledger["rs-late"].first_observed, at(PRE_SESSION_D));
}

/// The assumption horizon travels on the record: a decision can outlive the build that made it,
/// so "a check found no known action" has to carry how long that is good for
/// (`corporate-actions-coverage-v1`, `freshness_secs`). A finding — bars that were consistent, or
/// an announcement whose terms were applied — carries no horizon, because it does not stop being
/// true because nobody looked again.
#[test]
fn the_assumption_carries_its_freshness_horizon_and_a_finding_does_not() {
    let p = policy();
    let fresh = p.body.freshness_secs * 1_000_000_000;
    let checked = at(PRE_SESSION_D).timestamp_nanos_opt().unwrap();

    let anns = [retrieval(PRE_SESSION_D, &["OTHER", SYM], vec![])];
    let l = live(&anns, DECISION);
    let pc = l.prev_close.unwrap();
    assert_eq!(
        pc.actions.source,
        ActionsSource::ProviderCheckedNoKnownAction
    );
    assert_eq!(pc.actions.coverage_at, Some(checked));
    assert_eq!(pc.actions.valid_until, Some(checked + fresh));
    assert!(pc.actions.valid_at(checked + fresh));
    assert!(!pc.actions.valid_at(checked + fresh + 1));
    let b = l.baseline.unwrap();
    assert_eq!(b.basis.valid_until, Some(checked + fresh));

    // A confirmed action: its terms were applied, so there is nothing to expire. The check it
    // came from is still named, so a reader can see how old it is.
    let anns = [retrieval(
        PRE_SESSION_D,
        &["OTHER", SYM],
        vec![reverse_1_for_10("2026-12-10")],
    )];
    let l = live(&anns, DECISION);
    let pc = l.prev_close.unwrap();
    assert_eq!(pc.actions.source, ActionsSource::AnnouncedAction);
    assert_eq!(pc.actions.coverage_at, Some(checked));
    assert_eq!(
        pc.actions.valid_until, None,
        "an applied announcement is a finding, not an assumption"
    );
    assert!(pc.actions.valid_at(i64::MAX));
}
