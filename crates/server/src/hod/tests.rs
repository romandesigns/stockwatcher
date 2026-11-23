use super::logic::*;
use super::*;

const T0: i64 = 1_700_000_000 * SEC;
const CD: i64 = 60 * SEC;

fn policy() -> Policy {
    Policy::shipped()
}

/// A print of `px` at `secs` after 04:00, counted for both price and volume.
fn put(s: &mut Sym, secs: i64, px: f64, size: u64) -> bool {
    s.apply(T0, T0 + secs * SEC, px, size, true, true, false, CD)
}

fn facts(vol_base: f64) -> Facts {
    Facts {
        c1: Some(1.0),
        vol_base: Some(vol_base),
        runmax: None,
        hi52: None,
    }
}

fn inp(float: u64) -> Inputs {
    Inputs {
        c1: None,
        float: Some(float),
        tod5_base: None,
    }
}

/// A symbol that has traded `vol` shares, last at `px` (a new high), and what it matches.
fn cats_at(
    px: f64,
    vol: u64,
    float: Option<u64>,
    vol_base: f64,
    runmax: Option<f64>,
) -> Vec<&'static str> {
    let p = policy();
    let mut s = Sym::default();
    // Flat at the price, then a tick above it, so no squeeze matches.
    put(&mut s, 0, px, vol);
    put(&mut s, 1200, px + 0.0001, 0);
    let (ns, hi) = s.pending.expect("a new high");
    let f = Facts {
        c1: Some(px / 2.0),
        vol_base: Some(vol_base),
        runmax,
        hi52: None,
    };
    let i = Inputs {
        c1: None,
        float,
        tod5_base: None,
    };
    evaluate(&p, "T", &s, T0, ns, hi, &f, &i).map_or_else(Vec::new, |r| r.cat_ids())
}

#[test]
fn the_shipped_policy_lists_every_category_in_order() {
    let p = policy();
    let ids: Vec<&str> = p.categories.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, CATS);
    assert_eq!(p.id, DETECTOR);
    let on: Vec<&str> = p
        .categories
        .iter()
        .filter(|c| c.default_on)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(
        on,
        [
            "lf_momo",
            "lf_hrv",
            "lf_hrv_20",
            "lf_mrv",
            "sq_10_10",
            "sq_5_5"
        ]
    );
    assert!(
        p.categories
            .iter()
            .find(|c| c.id == "sq_52wk")
            .unwrap()
            .needs_history
    );
}

#[test]
fn a_new_high_is_strictly_above_every_earlier_print_and_never_the_first() {
    let mut s = Sym::default();
    assert!(
        !put(&mut s, 0, 1.00, 100),
        "the first print is not a new high"
    );
    assert!(!put(&mut s, 1, 1.00, 100), "equal is not above");
    assert!(!put(&mut s, 2, 0.95, 100));
    assert!(put(&mut s, 3, 1.01, 100));
    assert_eq!(s.pending, Some((T0 + 3 * SEC, 1.01)));
    // Another high before the decision replaces it, and is not a second pending one.
    assert!(!put(&mut s, 4, 1.02, 100));
    assert_eq!(s.pending, Some((T0 + 4 * SEC, 1.02)));
    assert_eq!(s.high, Some(1.02));
    assert_eq!(s.vol, 500);
}

#[test]
fn volume_only_prints_count_volume_but_never_make_the_high() {
    let mut s = Sym::default();
    put(&mut s, 0, 1.0, 100);
    assert!(!s.apply(T0, T0 + SEC, 2.0, 50, false, true, false, CD));
    assert_eq!(s.high, Some(1.0));
    assert_eq!(s.vol, 150);
    // An OHLC-only print makes the high, no volume.
    assert!(s.apply(T0, T0 + 2 * SEC, 1.5, 70, true, false, false, CD));
    assert_eq!(s.vol, 150);
}

#[test]
fn the_cooldown_holds_a_symbol_for_a_minute_after_its_row() {
    let mut s = Sym::default();
    put(&mut s, 0, 1.0, 100);
    assert!(put(&mut s, 10, 1.1, 100));
    s.pending = None;
    s.last_row = Some(T0 + 10 * SEC);
    assert!(!put(&mut s, 69, 1.2, 100), "59 s after the row");
    assert_eq!(s.high, Some(1.2), "the high still rises");
    assert!(put(&mut s, 70, 1.3, 100), "60 s after the row");
}

#[test]
fn windows_read_clock_minutes() {
    let mut s = Sym::default();
    put(&mut s, 0, 1.00, 100); // minute 0
    put(&mut s, 61, 0.90, 100); // minute 1
    put(&mut s, 300, 1.00, 100); // minute 5
    put(&mut s, 600, 1.20, 100); // minute 10
    assert_eq!(s.low_within(10, 10), Some(0.90), "minutes 1..=10");
    assert_eq!(s.low_within(10, 5), Some(1.20), "minutes 6..=10");
    assert_eq!(s.low_within(11, 10), Some(1.00), "minute 1 left the window");
    assert_eq!(s.low_within(16, 10), Some(1.20), "minute 5 too");
    assert_eq!(s.close_at(10, 5), Some(1.00));
    assert_eq!(
        s.close_at(10, 4),
        Some(0.90),
        "minute 4 is quiet: the last earlier close"
    );
    assert_eq!(s.vol_within(10, 5), 100);
    assert_eq!(s.vol_within(10, 6), 200);
}

#[test]
fn a_row_needs_the_band_a_previous_close_and_a_gain() {
    let p = policy();
    let mut s = Sym::default();
    put(&mut s, 0, 1.0, 1_000_000);
    put(&mut s, 1200, 1.01, 0);
    let (ns, px) = s.pending.unwrap();
    let f = facts(100_000.0);
    let i = inp(5_000_000);
    assert!(evaluate(&p, "T", &s, T0, ns, px, &f, &i).is_some());
    let no_c1 = Facts { c1: None, ..f };
    assert!(evaluate(&p, "T", &s, T0, ns, px, &no_c1, &i).is_none());
    let flat = Facts { c1: Some(px), ..f };
    assert!(
        evaluate(&p, "T", &s, T0, ns, px, &flat, &i).is_none(),
        "0 % is not up"
    );
    // The quote board's close wins over the daily bars'.
    let qc = Inputs { c1: Some(2.0), ..i };
    assert!(
        evaluate(&p, "T", &s, T0, ns, px, &f, &qc).is_none(),
        "down vs the quote board's close"
    );
    assert!(
        evaluate(
            &p,
            "T",
            &s,
            T0,
            ns,
            25.01,
            &Facts {
                c1: Some(10.0),
                ..f
            },
            &i
        )
        .is_none()
    );
    assert!(
        evaluate(
            &p,
            "T",
            &s,
            T0,
            ns,
            0.0999,
            &Facts {
                c1: Some(0.05),
                ..f
            },
            &i
        )
        .is_none()
    );
    assert!(
        evaluate(
            &p,
            "T",
            &s,
            T0,
            ns,
            25.0,
            &Facts {
                c1: Some(10.0),
                ..f
            },
            &i
        )
        .is_some()
    );
}

#[test]
fn float_boundaries() {
    // rvol 6x: high.
    let lf = cats_at(5.0, 600, Some(9_999_999), 100.0, None);
    assert_eq!(lf, ["lf_hrv"]);
    let mf = cats_at(5.0, 600, Some(10_000_000), 100.0, None);
    assert_eq!(mf, ["mf_hrv_u20"]);
    let mf_top = cats_at(5.0, 600, Some(49_999_999), 100.0, None);
    assert_eq!(mf_top, ["mf_hrv_u20"]);
    assert!(
        cats_at(5.0, 600, Some(50_000_000), 100.0, None).is_empty(),
        "large float"
    );
    assert!(
        cats_at(5.0, 600, None, 100.0, None).is_empty(),
        "unknown float"
    );
}

#[test]
fn rel_vol_boundaries() {
    let p = policy();
    let (h, m) = (p.rvol.high_min, p.rvol.medium_min);
    let at = |x: f64| {
        cats_at(
            5.0,
            (x * 1000.0).round() as u64,
            Some(1_000_000),
            1000.0,
            None,
        )
    };
    assert_eq!(at(h), ["lf_hrv"]);
    assert_eq!(at(h - 0.001), ["lf_mrv"]);
    assert_eq!(at(m), ["lf_mrv"]);
    assert!(at(m - 0.001).is_empty());
    // Unknown baseline: no rel vol category.
    let p = policy();
    let mut s = Sym::default();
    put(&mut s, 0, 5.0, 1_000_000);
    put(&mut s, 1200, 5.01, 0);
    let (ns, px) = s.pending.unwrap();
    let f = Facts {
        c1: Some(2.0),
        vol_base: None,
        runmax: Some(1.0),
        hi52: None,
    };
    assert!(evaluate(&p, "T", &s, T0, ns, px, &f, &inp(1_000_000)).is_none());
}

#[test]
fn price_twenty_boundaries() {
    assert_eq!(
        cats_at(20.0, 6000, Some(1_000_000), 1000.0, None),
        ["lf_hrv", "lf_hrv_20"]
    );
    assert_eq!(
        cats_at(19.99, 6000, Some(1_000_000), 1000.0, None),
        ["lf_hrv"]
    );
    assert_eq!(
        cats_at(20.0, 6000, Some(20_000_000), 1000.0, None),
        ["mf_hrv_20"]
    );
    assert_eq!(
        cats_at(19.99, 6000, Some(20_000_000), 1000.0, None),
        ["mf_hrv_u20"]
    );
    assert_eq!(
        cats_at(20.0, 4000, Some(20_000_000), 1000.0, None),
        ["mf_mrv_20"]
    );
    assert!(
        cats_at(19.99, 4000, Some(20_000_000), 1000.0, None).is_empty(),
        "no medium-float medium-rvol under $20"
    );
}

#[test]
fn former_momo_boundaries() {
    let p = policy();
    let r = p.momo.run_min;
    // rvol 1.0x exactly: the base the momo and squeeze categories need.
    assert_eq!(
        cats_at(5.0, 1000, Some(1_000_000), 1000.0, Some(r)),
        ["lf_momo"]
    );
    assert!(cats_at(5.0, 1000, Some(1_000_000), 1000.0, Some(r - 0.001)).is_empty());
    assert!(
        cats_at(5.0, 999, Some(1_000_000), 1000.0, Some(r)).is_empty(),
        "under the base rel vol"
    );
    assert!(
        cats_at(5.0, 1000, Some(20_000_000), 1000.0, Some(r)).is_empty(),
        "medium float"
    );
}

#[test]
fn squeeze_boundaries() {
    let p = policy();
    let run = |low: f64, px: f64, gap_min: i64| {
        let mut s = Sym::default();
        put(&mut s, 0, low, 1000);
        let at = 60 * gap_min;
        put(&mut s, at, px, 0);
        let (ns, hi) = s.pending.expect("high");
        let f = Facts {
            c1: Some(low / 2.0),
            vol_base: Some(1000.0),
            runmax: None,
            hi52: None,
        };
        evaluate(&p, "T", &s, T0, ns, hi, &f, &Inputs::default())
            .map_or_else(Vec::new, |r| r.cat_ids())
    };
    // 4 minutes later: inside both windows.
    assert_eq!(run(1.0, 1.10, 4), ["sq_10_10", "sq_5_5"]);
    assert_eq!(run(1.0, 1.0999, 4), ["sq_5_5"]);
    assert_eq!(run(1.0, 1.05, 4), ["sq_5_5"]);
    assert!(run(1.0, 1.0499, 4).is_empty());
    // 5 minutes later (minute 5): the low left the 5-minute window, not the 10-minute one.
    assert_eq!(run(1.0, 1.10, 5), ["sq_10_10"]);
    // 10 minutes later: out of both.
    assert!(run(1.0, 1.20, 10).is_empty());
}

#[test]
fn the_52_week_breakout_needs_history() {
    let p = policy();
    let mut s = Sym::default();
    put(&mut s, 0, 5.0, 1000);
    put(&mut s, 1200, 5.01, 0);
    let (ns, px) = s.pending.unwrap();
    let mut f = Facts {
        c1: Some(4.9),
        vol_base: Some(1000.0),
        runmax: None,
        hi52: None,
    };
    assert!(evaluate(&p, "T", &s, T0, ns, px, &f, &Inputs::default()).is_none());
    f.hi52 = Some(5.0);
    let r = evaluate(&p, "T", &s, T0, ns, px, &f, &Inputs::default()).unwrap();
    assert_eq!(r.cat_ids(), ["sq_52wk"]);
    f.hi52 = Some(5.01);
    assert!(evaluate(&p, "T", &s, T0, ns, px, &f, &Inputs::default()).is_none());
}

#[test]
fn the_row_carries_the_columns() {
    let p = policy();
    let mut s = Sym::default();
    // Pre-market first print 1.10 at minute 0, regular open 1.20 at 09:30.
    s.apply(T0, T0, 1.10, 1000, true, true, false, CD);
    let open = 330 * 60;
    s.apply(T0, T0 + open * SEC, 1.20, 1000, true, true, true, CD);
    s.apply(T0, T0 + (open + 240) * SEC, 1.30, 500, true, true, true, CD);
    s.apply(T0, T0 + (open + 600) * SEC, 1.50, 500, true, true, true, CD);
    let (ns, px) = s.pending.unwrap();
    let f = Facts {
        c1: Some(1.0),
        vol_base: Some(300.0),
        runmax: Some(0.5),
        hi52: None,
    };
    let i = Inputs {
        c1: None,
        float: Some(3_000_000),
        tod5_base: Some(250.0),
    };
    let r = evaluate(&p, "T", &s, T0, ns, px, &f, &i).unwrap();
    assert_eq!(r.vol, 3000);
    assert!((r.rvol.unwrap() - 10.0).abs() < 1e-9);
    assert!(
        (r.rvol5.unwrap() - 2.0).abs() < 1e-9,
        "500 shares in the last 5 minutes over 250"
    );
    assert!(
        (r.gap.unwrap() - 0.20).abs() < 1e-9,
        "the regular open, not the first print"
    );
    assert!((r.chg - 0.5).abs() < 1e-9);
    assert!((r.chg5.unwrap() - (1.5 / 1.3 - 1.0)).abs() < 1e-9);
    assert!((r.chg10.unwrap() - 0.25).abs() < 1e-9);
    assert_eq!(r.cat_ids(), ["lf_momo", "lf_hrv", "sq_10_10"]);
    let w = row_wire(&r);
    assert_eq!(w["cats"], json!(["lf_momo", "lf_hrv", "sq_10_10"]));
    assert_eq!(w["float"], json!(3_000_000));
}

#[test]
fn facts_from_daily_bars() {
    let p = policy();
    let bar = |d: i32, h: f64, c: f64, v: u64| DailyBar {
        d,
        h: Some(h),
        c,
        v: Some(v),
    };
    let mut bars: Vec<DailyBar> = (0..40).map(|d| bar(d, 1.05, 1.0, 1000)).collect();
    bars[20] = bar(20, 1.6, 1.4, 1000); // a +60 % run day
    let f = facts_of(&p, &bars, &[], 40);
    assert_eq!(f.c1, Some(1.0));
    assert!((f.vol_base.unwrap() - 1000.0).abs() < 1e-9);
    assert!((f.runmax.unwrap() - 0.6).abs() < 1e-9);
    assert_eq!(f.hi52, None, "40 bars: no 52-week high");
    // A 1-for-10 reverse split on day 35: older closes ×10, older volumes ÷10.
    let f = facts_of(&p, &bars, &[(35, 10.0)], 40);
    assert_eq!(f.c1, Some(1.0));
    let mean = (25.0 * 100.0 + 5.0 * 1000.0) / 30.0;
    assert!((f.vol_base.unwrap() - mean).abs() < 1e-6);
    // Fewer than 30 sessions with volume: no baseline (the engine's strict denominator).
    let f = facts_of(&p, &bars[..29], &[], 40);
    assert_eq!(f.vol_base, None);
    let long: Vec<DailyBar> = (0..300)
        .map(|d| bar(d, 1.0 + d as f64 / 1000.0, 1.0, 1))
        .collect();
    let f = facts_of(&p, &long, &[], 300);
    assert!((f.hi52.unwrap() - 1.299).abs() < 1e-9);
}

#[test]
fn the_switch_reads_the_detectors_document() {
    assert!(switch_on(None));
    assert!(switch_on(Some(&json!({}))));
    assert!(!switch_on(Some(&json!({"hod-1": {"on": false}}))));
    assert!(!switch_on(Some(&json!({"hod-1": false}))));
    assert!(switch_on(Some(&json!({"hod-1": {"on": true}}))));
}

#[test]
fn update_frames_carry_only_the_new_rows() {
    let mut b = HodScanner::new();
    b.enable();
    assert!(b.take_update().is_none());
    let snap: Value = serde_json::from_str(&b.snapshot_frame()).unwrap();
    assert_eq!(snap["type"], "hod.snapshot");
    assert_eq!(snap["policy"]["categories"].as_array().unwrap().len(), 10);
    assert_eq!(snap["hi52_available"], false);
}

/// The two Train replay days exported by the calibration (`55-hod/replay/<day>/`): every
/// synthetic print through [`Sym::apply`] and [`evaluate`], deciding at each print, with the
/// starting thresholds the fixture's `expected.json` was counted at, then the shipped ones.
/// `cargo test -p stockwatcher-server hod::tests::replay -- --ignored --nocapture`.
#[test]
#[ignore = "reads the research replay fixture on the owner's PC"]
fn replay_train_days() {
    let root = std::env::var("HOD_REPLAY_DIR").unwrap_or_else(|_| {
        r"C:\wavystack\outputs\stockwatcher-research-v2\55-hod\replay".to_string()
    });
    for day in ["2022-06-01", "2023-08-09"] {
        let dir = Path::new(&root).join(day);
        let exp: Value =
            serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
        let t0 = exp["t0_ns"].as_i64().unwrap();
        let mut facts: HashMap<String, (Facts, Option<u64>)> = HashMap::new();
        for l in std::fs::read_to_string(dir.join("facts.csv"))
            .unwrap()
            .lines()
            .skip(1)
        {
            let c: Vec<&str> = l.split(',').collect();
            let num = |i: usize| c.get(i).and_then(|s| s.parse::<f64>().ok());
            facts.insert(
                c[0].to_string(),
                (
                    Facts {
                        c1: num(2),
                        vol_base: num(3),
                        runmax: num(4),
                        hi52: None,
                    },
                    num(1).map(|f| f as u64),
                ),
            );
        }
        let prints = std::fs::read_to_string(dir.join("prints.csv")).unwrap();
        let mut start = Policy::shipped();
        start.rvol.medium_min = 2.0;
        start.momo.run_min = 0.5;
        for (label, p) in [("start (5, 2, 0.5)", start), ("shipped", Policy::shipped())] {
            let mut syms: HashMap<&str, Sym> = HashMap::new();
            let mut per: HashMap<&str, usize> = HashMap::new();
            let mut total = 0;
            let mut on_total = 0;
            let on_mask: u16 = p
                .categories
                .iter()
                .filter(|c| c.default_on)
                .map(|c| cat_bit(&c.id))
                .sum();
            for l in prints.lines().skip(1) {
                let mut it = l.split(',');
                let ns: i64 = it.next().unwrap().parse().unwrap();
                let sym = it.next().unwrap();
                let px: f64 = it.next().unwrap().parse().unwrap();
                let size: u64 = it.next().unwrap().parse().unwrap();
                let regular = ns >= t0 + 330 * MIN && ns < t0 + 720 * MIN;
                let s = syms.entry(sym).or_default();
                if s.apply(t0, ns, px, size, true, true, regular, p.cooldown_secs * SEC) {
                    let (ns, px) = s.pending.take().unwrap();
                    let (f, float) = facts.get(sym).copied().unwrap_or_default();
                    let i = Inputs {
                        c1: None,
                        float,
                        tod5_base: None,
                    };
                    if let Some(r) = evaluate(&p, sym, s, t0, ns, px, &f, &i) {
                        s.last_row = Some(ns);
                        total += 1;
                        if r.cats & on_mask != 0 {
                            on_total += 1;
                        }
                        for c in r.cat_ids() {
                            *per.entry(c).or_default() += 1;
                        }
                    }
                }
            }
            let mut per: Vec<_> = per.into_iter().collect();
            per.sort();
            println!(
                "{day} {label}: rows {total}, with a default-on tag {on_total}; per category {per:?}; python print_level all = {}",
                exp["print_level"]["all"]
            );
        }
    }
}
