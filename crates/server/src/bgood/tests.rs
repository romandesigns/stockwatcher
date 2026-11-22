//! b-good's live plumbing. The decisions' parity with the research is proven outside the repo on
//! the research's own rows (module docs); these tests pin the rules this module adds around them.

use super::logic::*;
use super::*;

fn facts() -> DayFacts {
    DayFacts {
        c1: 2.0,
        h1: 2.2,
        adv20: 1e6,
        dadv20: 2e6,
        mdr20: 0.08,
        runs90: 1.0,
        runs365: 3.0,
        prior_sessions: 900.0,
    }
}

#[test]
fn models_load_with_the_finalists_columns() {
    let f46: Vec<String> = FEATS.iter().map(|s| s.to_string()).collect();
    assert_eq!(model2().features, f46);
    let f56: Vec<String> = FEATS
        .iter()
        .chain(U1_EXTRA.iter())
        .map(|s| s.to_string())
        .collect();
    assert_eq!(model1().features, f56);
    assert_eq!(version(model1()), "d8a1f8ff993e");
    assert_eq!(version(model2()), "e4273799d70e");
}

/// `kind_code` is the index in the B0 table's sorted kinds, and every B0 kind is a live kind.
#[test]
fn kind_codes_are_the_tables_and_cover_b0() {
    let mut s = B0_KINDS_SORTED.to_vec();
    s.sort();
    assert_eq!(s, B0_KINDS_SORTED.to_vec());
    let live: Vec<&str> = NoticeKind::ALL.iter().map(|k| k.as_str()).collect();
    for k in B0_KINDS_SORTED {
        assert!(live.contains(&k), "{k}");
    }
    assert_eq!(kind_code("burst"), Some(0));
    assert_eq!(kind_code("shelf"), Some(11));
    assert_eq!(kind_code("bgood"), None);
    assert_eq!(kind_code("ross_crossing"), None);
}

/// The xsim quote rule: last valid quote at or before t; stale over 60 s; non-market over 50 %
/// of the mid; spread only with clear flags.
#[test]
fn quote_rule() {
    let s = logic::SEC;
    let book = [
        (10 * s, 1_000_000, 1_010_000),
        (20 * s, 0, 1_010_000),       // invalid: skipped
        (30 * s, 1_000_000, 900_000), // crossed: skipped
    ];
    let q = QuoteAt::at(Some(&book), 40 * s);
    assert_eq!(
        (q.bid_u, q.ask_u, q.age_ns, q.flags),
        (1_000_000, 1_010_000, 30 * s, 0)
    );
    assert!((q.spread_bp() - 99.502_49).abs() < 1e-3);
    assert_eq!(q.age_ms_f(), 30_000.0);
    let st = QuoteAt::at(Some(&book), 71 * s);
    assert_eq!(st.flags, QF_STALE);
    assert!(st.spread_bp().is_nan());
    assert_eq!(QuoteAt::at(Some(&book), 5 * s).flags, QF_NO_QUOTE);
    assert_eq!(QuoteAt::at(None, 5 * s).flags, QF_NO_FILE);
    let wide = [(10 * s, 1_000_000, 2_000_000)];
    assert_eq!(QuoteAt::at(Some(&wide), 11 * s).flags, QF_NON_MARKET);
    let row = u1_row(&[0.0; NF], 3, 1, (2.0, 5.0, 9.0), &q, &st);
    assert_eq!(&row[NF..NF + 5], &[3.0, 1.0, 2.0, 5.0, 9.0]);
    assert!(row[NF + 6].is_nan());
    assert_eq!(row[NF + 8], 1.0);
    assert_eq!(row[NF + 9], QF_STALE as f32);
}

/// `u2.py pick`: 2 a stock, 600 s apart, none in [ns − 30 min, ns] after a B0 alert, 30 a day.
#[test]
fn pick_rule() {
    let s = logic::SEC;
    let mut p = Picker::default();
    assert_eq!(p.offer("A", 1000 * s, &[]), Ok(()));
    assert_eq!(p.offer("A", 1599 * s, &[]), Err(Skip::Spacing));
    assert_eq!(p.offer("A", 1600 * s, &[]), Ok(()));
    assert_eq!(p.offer("A", 9000 * s, &[]), Err(Skip::PerSym));
    assert_eq!(p.offer("B", 5000 * s, &[3201 * s]), Err(Skip::RecentB0));
    assert_eq!(p.offer("B", 5000 * s, &[5000 * s]), Err(Skip::RecentB0));
    assert_eq!(p.offer("B", 5000 * s, &[3200 * s, 5001 * s]), Ok(()));
    for i in 0..27 {
        assert_eq!(p.offer(&format!("C{i}"), 1000 * s, &[]), Ok(()));
    }
    assert_eq!(p.taken(), PICK_K);
    assert_eq!(p.offer("D", 1000 * s, &[]), Err(Skip::DayCap));
}

/// `stream_ctx`: strictly earlier alerts only, minutes since the symbol's previous one.
#[test]
fn alert_context() {
    let m = 60 * logic::SEC;
    let mut c = Ctx::default();
    for (s, t) in [("A", 10 * m), ("B", 11 * m), ("A", 13 * m), ("A", 13 * m)] {
        c.record(s, t);
    }
    let (n, since, day) = c.of("A", 10 * m);
    assert!(n == 0.0 && since.is_nan() && day == 0.0);
    let (n, since, day) = c.of("A", 13 * m);
    assert_eq!((n, since, day), (1.0, 3.0, 2.0));
    let (n, since, day) = c.of("A", 20 * m);
    assert_eq!((n, since, day), (3.0, 7.0, 4.0));
    assert!(c.of("Z", 20 * m).1.is_nan());
}

/// `refdata::DailyBars::facts` with a 1-for-10 reverse split between the bars.
#[test]
fn daily_facts_restate_splits() {
    let bars: Vec<DailyIn> = (0..30)
        .map(|i| DailyIn {
            day: i,
            h: if i == 25 { 1.4 } else { 1.05 },
            l: 0.95,
            c: 1.0,
            v: 1000.0,
            vw: 1.0,
        })
        .collect();
    // split effective day 28: earlier prices ×10, shares ÷10 on day 30's basis.
    let fx = |from: i64, to: i64| if from < 28 && to >= 28 { 10.0 } else { 1.0 };
    let f = day_facts(&bars, fx, 30);
    assert_eq!(f.c1, 1.0);
    assert_eq!(f.prior_sessions, 30.0);
    // bars 10..29: 18 restated (÷10) and 2 not.
    assert!((f.adv20 - (18.0 * 100.0 + 2.0 * 1000.0) / 20.0).abs() < 1e-9);
    assert_eq!(f.dadv20, 1000.0);
    // day 25 ran (+40 % on the previous close); day 28's split restates day 27's close to 10,
    // so it is not a run.
    assert_eq!((f.runs90, f.runs365), (1.0, 1.0));
    assert!((f.mdr20 - 0.1 / 0.95).abs() < 1e-12);
    let none = day_facts(&[], fx, 30);
    assert!(none.c1.is_nan() && none.prior_sessions == 0.0 && none.runs90 == 0.0);
}

#[test]
fn tod_files_round_trip_and_average_like_the_research() {
    let day = |d: u32, vol: u64| TodDay {
        date: NaiveDate::from_ymd_opt(2026, 11, d),
        present: vec!["A".into(), "B".into()],
        rows: vec![("A".into(), 3, 10, vol, vol as f32 * 2.0)],
    };
    let t = day(2, 100);
    assert_eq!(TodDay::parse(&t.to_tsv()).rows, t.rows);
    assert_eq!(TodDay::parse(&t.to_tsv()).present, t.present);
    let prior: Vec<TodDay> = (1..=20).map(|d| day(d, d as u64 * 10)).collect();
    let b = tod_baseline(&prior);
    let r = b["A"].rows[3].unwrap();
    // mean over 20 sessions of 10..200 = 105; over the last 5 (160..200) = 180.
    assert_eq!((r.b20_vol, r.b5_vol, r.b5_dol), (105.0, 180.0, 360.0));
    assert_eq!((r.present5, r.present20), (5, 20));
    assert_eq!(b["A"].vol_base(3), Some(105.0));
    assert!(b["A"].rows[4].is_none());
    assert!(!b.contains_key("B"));
}

fn trade(sym: &str, ns: i64, px: f64, size: u64) -> Trade {
    Trade::new(
        sym,
        UnixNanos(ns),
        market_events::Price::from_micros((px * 1e6).round() as i64),
        size,
        1,
    )
}

/// A tag is captured at `slot_for_ns(a + 30 s)` and handed over at `a + 30 s` only on a day the
/// detector may send; withheld (counted) otherwise.
#[test]
fn a_tag_waits_30_s_and_needs_complete_inputs() {
    let date = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
    let d = day_of(date).unwrap();
    let both = Counted {
        ohlc: true,
        volume: true,
    };
    for complete in [true, false] {
        let mut b = BGoodLive::new();
        b.tick(d.t0 - 20 * 60 * logic::SEC);
        let mut inp = Inputs {
            date: Some(date),
            raw_dates: if complete { MIN_HISTORY_DATES } else { 63 },
            tod_sessions: TOD_SESSIONS,
            ..Default::default()
        };
        inp.facts.insert("XLAB".into(), facts());
        assert!(b.take_inputs_request() == Some(date));
        b.set_inputs(inp);
        let t1 = d.t0 + 600 * logic::SEC;
        for k in 0..40 {
            b.apply_trade(
                &trade("XLAB", t1 + k * logic::SEC, 2.0 + k as f64 * 0.01, 500),
                both,
            );
        }
        b.tick(t1 + 41 * logic::SEC);
        let mut n = Notice::test(NoticeKind::Fast, "XLAB");
        n.at_ns = t1 + 40 * logic::SEC;
        b.record_alert(&n);
        b.record_alert(&n); // the same instant twice counts once
        let a = n.at_ns.max(t1 + 41 * logic::SEC);
        b.tick(a + 29 * logic::SEC);
        assert!(b.take_tag_requests().is_empty());
        b.tick(a + 30 * logic::SEC);
        let r = b.take_tag_requests();
        if complete {
            assert_eq!(r.len(), 1);
            assert_eq!(
                (r[0].t_ns, r[0].tier, r[0].kind_code),
                (a + 30 * logic::SEC, 1, 2)
            );
            assert_eq!(r[0].ctx.2, 0.0);
            // the features at the slot ending ≥ 2 s before t: a price, volume, the day's facts
            assert!(r[0].ts[37].is_finite() && r[0].ts[11] > 0.0);
            let c = score_tag(
                &r[0],
                QuoteAt::NO_DATA,
                QuoteAt::NO_DATA,
                a + 31 * logic::SEC,
            );
            assert_eq!(c.tagged, model1().passes(c.margin));
        } else {
            assert!(r.is_empty());
            assert_eq!(b.cost()["tags_withheld"], 1);
        }
        assert_eq!(b.cost()["alerts_scored"], 1);
    }
}

/// An alert outside `universe-v1` (stale last print, out of band) is not a B0 row.
#[test]
fn universe_selects_b0_rows() {
    let s = logic::SEC;
    assert_eq!(
        universe_ok(Some((100 * s, 2_000_000)), 160 * s),
        Some(2_000_000)
    );
    assert_eq!(universe_ok(Some((100 * s, 2_000_000)), 161 * s), None);
    assert_eq!(universe_ok(Some((100 * s, 99_999)), 101 * s), None);
    assert_eq!(universe_ok(Some((100 * s, 25_000_001)), 101 * s), None);
    assert_eq!(universe_ok(None, 101 * s), None);
    assert_eq!(price_tier(999_999), 0);
    assert_eq!(price_tier(1_000_000), 1);
    assert_eq!(price_tier(5_000_000), 2);
}

/// The feature state steps empty slots exactly as the dense grid (forward-filled price, EMAs
/// moving on it), so a stock first seen mid-day equals one stepped from 04:00.
#[test]
fn late_first_bar_equals_stepping_from_the_open() {
    let bar = Bar {
        c_u: 2_000_000,
        h_u: 2_100_000,
        l_u: 1_900_000,
        vol: 1000.0,
        dollars: 2000.0,
        n: 10.0,
    };
    let mut a = SymFeat::new();
    a.advance(500, Some(&bar));
    a.advance(520, None);
    let mut b = SymFeat::new();
    for s in 0..=520 {
        b.advance(s, if s == 500 { Some(&bar) } else { None });
    }
    let fa = a.features(&facts(), None);
    let fb = b.features(&facts(), None);
    for i in 0..NF {
        assert!(
            fa[i].to_bits() == fb[i].to_bits() || (fa[i].is_nan() && fb[i].is_nan()),
            "{}",
            FEATS[i]
        );
    }
    assert_eq!(fa[36], 20.0 * 15.0); // age_s
    assert_eq!(fa[34], 130.25); // tod_min
}

/// The runtime switches (crate::alerts, "Detector switches"): only an explicit `on: false` is off.
#[test]
fn detector_switches_parse() {
    use crate::alerts::Detectors;
    let v = serde_json::json!({"b-good-1": {"on": false}, "b-good-2": {"on": true}, "x": "junk"});
    let d = Detectors::parse(Some(&v));
    assert!(!d.on(TAG_ID) && d.on(PICK_ID) && d.on("x"));
    assert!(Detectors::parse(None).on(TAG_ID));
    assert!(Detectors::parse(Some(&serde_json::json!([1, 2]))).on(PICK_ID));
    let n = Notice::test(NoticeKind::BGood, "A");
    assert_eq!(Detectors::of(&n), Some(PICK_ID));
    assert_eq!(Detectors::of(&Notice::test(NoticeKind::Fast, "A")), None);
}

fn conf(tagged: bool) -> Confidence {
    Confidence {
        event_id: "runners-fast-X-1".into(),
        alert_id: "X-1".into(),
        symbol: "XLAB".into(),
        kind: NoticeKind::Fast,
        a_ns: 1_793_017_427_000_000_000,
        t_ns: 1_793_017_457_000_000_000,
        margin: -1.0,
        prob: 0.2689,
        tagged,
        q_a: QuoteAt::NO_DATA,
        q_t: QuoteAt::NO_DATA,
        decided_ns: 1_793_017_457_600_000_000,
    }
}

/// The frame clients key on (eventId, alertId) and the log item the ledger line regroups into.
#[test]
fn confidence_frame_and_log_item() {
    let f: Value =
        serde_json::from_str(&crate::alerts::runners::confidence_frame(&conf(true))).unwrap();
    assert_eq!(f["type"], "runners.confidence");
    assert_eq!(f["eventId"], "runners-fast-X-1");
    assert_eq!(f["alertId"], "X-1");
    assert_eq!(f["notice"], "runners.fast");
    assert_eq!(f["tier"], "high");
    assert_eq!(f["score"], 0.269);
    assert_eq!(conf(false).tier(), "none");
    let lines = [
        json!({"kind":"decision","v":1,"decided_at":1000,"event_id":"runners-fast-X-1","notice":"runners.fast","symbol":"XLAB","price":2.0,"path":"desk","decision":"sent","held":null,"devices":1,"title":"t","body":"b"}),
        json!({"kind":"confidence","v":1,"decided_at":31_000,"event_id":"runners-fast-X-1","tier":"high","score":0.27,"held":null}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let (items, _) = crate::notices::group(&text);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["confidence"]["tier"], "high");
    assert_eq!(items[0]["confidence"]["at"], 31_000);
    assert_eq!(
        crate::notices::held_text(Some("switched_off"), None),
        "detector switched off"
    );
    assert_eq!(crate::notices::kind_label("model.pick"), "Model pick");
}
