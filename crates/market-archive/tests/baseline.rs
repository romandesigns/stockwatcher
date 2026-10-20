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
    assert_eq!(l.prev_close_reason, "checked_through_evaluated_date");
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
        },
        SYM,
        dd(),
        45,
    );
    let pc = l.prev_close.unwrap();
    assert_eq!((pc.basis, pc.close_micros), (CloseBasis::Raw, 500_000));
    assert_eq!(pc.actions_checked_through, Some(p));
    assert_eq!(l.prev_close_reason, "checked_before_evaluated_date");
    assert!(l.baseline.is_none());
    assert_eq!(l.baseline_reason, "no_announcement_coverage");
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
        },
        SYM,
        dd(),
        45,
    );
    assert_eq!(l.checked_through, Some(d("2026-12-09")));
    assert!(l.baseline.is_none());
}

// ---- live: announcement coverage (weaker evidence)

use market_archive::baseline::Announced;
use market_archive::{ActionRetrieval, NameChange, SplitAction, SplitKind};
use stock_context::{ActionsSource, SplitRatio};

/// 07:00 ET on D: a pre-market retrieval.
const PREMARKET_D: &str = "2026-12-10T12:00:00Z";

fn retrieval(when: &str, covered: &[&str], splits: Vec<SplitAction>) -> ActionRetrieval {
    ActionRetrieval {
        schema: 1,
        retrieved_at: at(when),
        provider: "alpaca".into(),
        endpoint: "/v1/corporate-actions".into(),
        types: vec![
            "forward_split".into(),
            "reverse_split".into(),
            "unit_split".into(),
            "name_change".into(),
        ],
        start: d("2026-12-09"),
        end: dd(),
        symbols_requested: vec!["OTHER".into(), SYM.into()],
        symbols_covered: covered.iter().map(|s| s.to_string()).collect(),
        success: covered.len() == 2,
        errors: vec![],
        requests: 1,
        command: "test".into(),
        splits,
        unit_splits: vec![],
        name_changes: vec![],
    }
}

fn reverse_1_for_10(ex: &str) -> SplitAction {
    SplitAction {
        id: "rs-1".into(),
        kind: SplitKind::Reverse,
        symbol: SYM.into(),
        ex_date: d(ex),
        old_rate: 10.0,
        new_rate: 1.0,
    }
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

#[test]
fn a_pre_market_retrieval_without_a_split_vouches_for_d_as_announced_evidence() {
    let (raw, snap) = live_world();
    let anns = [retrieval(PREMARKET_D, &["OTHER", SYM], vec![])];
    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &anns,
        },
        SYM,
        dd(),
        45,
    );
    let pc = l.prev_close.unwrap();
    assert_eq!((pc.basis, pc.close_micros), (CloseBasis::Raw, 500_000));
    assert_eq!(pc.actions_checked_through, Some(dd()));
    assert_eq!(pc.actions.source, ActionsSource::Announced);
    assert_eq!(pc.actions.bar_verified_through, Some(d("2026-12-09")));
    assert_eq!(
        pc.actions.announcement.as_deref(),
        Some(anns[0].id().as_str())
    );
    assert_eq!(l.prev_close_reason, "announced_through_evaluated_date");
    assert_eq!(
        pc.observed_at,
        at(PREMARKET_D).timestamp_nanos_opt().unwrap(),
        "visible from the retrieval, not before"
    );
    let b = l.baseline.unwrap();
    assert_eq!(b.basis.source, ActionsSource::Announced);
    assert_eq!(b.basis.bar_verified_through, Some(d("2026-12-09")));
    assert!(b.sessions.iter().all(|s| s.volume == vol(s.date)));
    assert_eq!(l.baseline_reason, "ok");
}

#[test]
fn an_announced_reverse_split_on_d_restates_close_and_volumes_by_the_ratio() {
    let (raw, snap) = live_world();
    let anns = [retrieval(
        PREMARKET_D,
        &["OTHER", SYM],
        vec![reverse_1_for_10("2026-12-10")],
    )];
    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &anns,
        },
        SYM,
        dd(),
        45,
    );
    let pc = l.prev_close.unwrap();
    assert_eq!(pc.basis, CloseBasis::SplitAdjusted);
    assert_eq!(pc.close_micros, 5_000_000, "$0.50 x 10");
    assert_eq!(pc.actions_checked_through, Some(dd()));
    assert_eq!(
        pc.actions.applied_split,
        Some((
            SplitRatio {
                old_rate: 10,
                new_rate: 1
            },
            "rs-1".to_string()
        ))
    );
    let b = l.baseline.unwrap();
    assert!(
        b.sessions
            .iter()
            .all(|s| s.volume == (vol(s.date) as f64 / 10.0).round() as u64)
    );
    assert_eq!(b.basis.applied_split, pc.actions.applied_split);
}

#[test]
fn no_usable_retrieval_means_unknown() {
    let (raw, snap) = live_world();
    let p = d("2026-12-09");
    let renamed = {
        let mut r = retrieval(PREMARKET_D, &["OTHER", SYM], vec![]);
        r.name_changes.push(NameChange {
            id: "nc".into(),
            old_symbol: "OLD".into(),
            new_symbol: SYM.into(),
            process_date: dd(),
        });
        r
    };
    let mut second = reverse_1_for_10("2026-12-10");
    second.id = "rs-2".into();
    let cases: Vec<(Vec<ActionRetrieval>, &str)> = vec![
        // After the 09:30 open: too late to vouch for D.
        (
            vec![retrieval("2026-12-10T15:00:00Z", &["OTHER", SYM], vec![])],
            "no_announcement_coverage",
        ),
        // Before P's session ended: blind to P's evening.
        (
            vec![retrieval("2026-12-09T20:00:00Z", &["OTHER", SYM], vec![])],
            "no_announcement_coverage",
        ),
        // The chunk holding the symbol failed: not covered.
        (
            vec![retrieval(PREMARKET_D, &["OTHER"], vec![])],
            "no_announcement_coverage",
        ),
        // A rename into the symbol on D (F7: actions are filed under the old ticker).
        (vec![renamed], "symbol_changed"),
        // Two splits in the window.
        (
            vec![retrieval(
                PREMARKET_D,
                &["OTHER", SYM],
                vec![reverse_1_for_10("2026-12-10"), second],
            )],
            "multiple_splits",
        ),
    ];
    for (anns, why) in cases {
        let l = load(
            &Inputs {
                raw: &raw,
                split: Some(&snap),
                announcements: &anns,
            },
            SYM,
            dd(),
            45,
        );
        let pc = l.prev_close.unwrap();
        assert_eq!(
            pc.actions_checked_through,
            Some(p),
            "{why}: bars reach P only"
        );
        assert_eq!(pc.actions.source, ActionsSource::BarVerified);
        assert!(l.baseline.is_none(), "{why}");
        assert_eq!(l.baseline_reason, why);
        if why != "no_announcement_coverage" {
            assert_eq!(l.prev_close_reason, why);
        }
    }
    // A split announced for a later date is not D's business.
    let anns = [retrieval(
        PREMARKET_D,
        &["OTHER", SYM],
        vec![reverse_1_for_10("2026-12-11")],
    )];
    assert!(matches!(
        market_archive::baseline::announced(&anns, SYM, p, dd()),
        Announced::NoSplit { .. }
    ));
}

#[test]
fn bar_verification_wins_over_an_announcement_when_both_exist() {
    let m = Market::plain("2026-09-01", "2026-12-15");
    let raw = m.raw(AFTER_D);
    let snap = m.split(dd(), AFTER_D);
    let anns = [retrieval(PREMARKET_D, &["OTHER", SYM], vec![])];
    let l = load(
        &Inputs {
            raw: &raw,
            split: Some(&snap),
            announcements: &anns,
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
