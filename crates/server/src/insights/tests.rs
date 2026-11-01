use super::*;
use std::collections::HashMap;
use std::path::Path;

/// The owner's acceptance case, INO 2026-10-30, against the local copy of its tape and logs
/// (`C:/sw-wt/ino-data`, never committed: real market data). Run with `--ignored --nocapture`.
/// It must reproduce the reference replay: setups 1 and 2 muted by the warm-up after the
/// 16:38:05 restart (+108.5 % and +64.7 % after arming), 3 notified (+46.1 %), 4 and 5 held by
/// one-per-day, the later ones past the 30-minute window.
#[test]
#[ignore = "needs the local INO 2026-10-30 data (C:/sw-wt/ino-data)"]
fn ino_2026_10_30_reproduces_the_replay() {
    let data = Path::new("C:/sw-wt/ino-data");
    if !data.join("2026-10-30/INO.swa").is_file() {
        eprintln!("no INO data here; skipped");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let notices = tmp.path().join("notices");
    let transitions = tmp.path().join("transitions");
    std::fs::create_dir_all(&notices).unwrap();
    std::fs::create_dir_all(&transitions).unwrap();
    std::fs::copy(
        data.join("logs/notices-INO.jsonl"),
        notices.join("2026-10-30.jsonl"),
    )
    .unwrap();
    std::fs::copy(
        data.join("logs/transitions-INO.jsonl"),
        transitions.join("2026-10-30.jsonl"),
    )
    .unwrap();
    let day: NaiveDate = "2026-10-30".parse().unwrap();
    let make = |journal: bool, logs: Option<std::path::PathBuf>| {
        Insights::new(
            Sources {
                archive: ArchiveConfig {
                    root: data.to_path_buf(),
                    ..ArchiveConfig::default()
                },
                transitions_dir: if journal {
                    transitions.clone()
                } else {
                    tmp.path().join("none")
                },
                server_logs: logs,
                cache_dir: None,
            },
            NoticeFeed::open(Some(notices.clone()), "2026-10-31".parse().unwrap()),
        )
    };
    let now = market_events::session::et("2026-10-31".parse().unwrap(), 12, 0, 0, 0)
        .unwrap()
        .0;
    let started = std::time::Instant::now();
    let v = make(true, None).package_blocking("INO", day, now);
    eprintln!("built in {:?}; source {}", started.elapsed(), v["source"]);
    for a in v["alerts"].as_array().unwrap() {
        eprintln!(
            "alert {} {} {} {} -> {}",
            a["n"], a["notice"], a["status"], a["price"], a["outcome"]
        );
    }
    for m in v["missed"].as_array().unwrap() {
        eprintln!(
            "missed {} {} {}",
            m["episode_id"], m["reason"], m["outcome"]
        );
    }
    eprintln!("scorecard {}", v["scorecard"]);
    eprintln!("stats {}", v["stats"]);
    eprintln!("halts {}", v["halts"]);
    eprintln!("package {} bytes", v.to_string().len());

    let check = |v: &Value, evidence: &str| {
        assert_eq!(v["available"], true);
        let missed: HashMap<String, &Value> = v["missed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["episode_id"]
                        .as_str()
                        .unwrap()
                        .rsplit(':')
                        .next()
                        .unwrap()
                        .to_string(),
                    m,
                )
            })
            .collect();
        let reason = |n: &str| missed.get(n).map(|m| m["reason"].as_str().unwrap());
        let best = |n: &str| missed[n]["outcome"]["best_pct"].as_f64().unwrap();
        assert_eq!(reason("1"), Some("warmup"));
        assert_eq!(reason("2"), Some("warmup"));
        assert_eq!(missed["1"]["evidence"], evidence);
        assert_eq!(best("1"), 108.5);
        assert_eq!(best("2"), 64.7);
        assert_eq!(reason("3"), None, "setup 3 was notified");
        assert_eq!(reason("4"), Some("one_per_day"));
        assert_eq!(reason("5"), Some("one_per_day"));
        for n in ["6", "8", "11", "12", "13", "14"] {
            assert_eq!(reason(n), Some("window_closed"), "setup {n}");
        }
        for n in ["7", "9", "10"] {
            assert_eq!(reason(n), None, "setup {n} never armed");
        }
        let alerts = v["alerts"].as_array().unwrap();
        let setup = alerts
            .iter()
            .find(|a| a["event_id"] == "runners-pullback-live:2026-10-30:INO:micro_pullback:3")
            .unwrap();
        assert_eq!(setup["outcome"]["best_pct"], 46.1);
        // Its notice said $1.86 (price 1.855); the row says the same.
        assert_eq!(setup["price_text"], "$1.86");
        assert_eq!(setup["outcome"]["stop"], 1.84);
        assert!(setup["outcome"]["stop_hit_at"].is_null());
        let fast = &alerts[0];
        assert_eq!(fast["notice"], "runners.fast");
        assert_eq!(fast["outcome"]["best_pct"], 273.1);
        assert_eq!(v["scorecard"]["sent"], 6);
        assert_eq!(v["halts"]["outside_regular"], true);
        assert_eq!(v["halts"]["move_session"], "after");
    };
    check(&v, "journal");
    // The day's last print leads (the move was after hours); the close is quoted at 16:00.
    assert_eq!(v["stats"]["last"], 2.06);
    assert_eq!(v["stats"]["last_session"], "after");
    assert_eq!(
        v["stats"]["regular_close_at"],
        market_events::session::et(day, 16, 0, 0, 0).unwrap().0 / 1_000_000
    );
    // The chart opens on the after-hours move, not on 4 AM: from 30 min before its base (the
    // close's $0.5961 bar at 15:59, the low of the hour before the first alert) to 40 min after
    // the last sent alert (17:04:15, the shelf): about the design's 3:30-5:40 PM.
    let et_ms = |h, m, s| market_events::session::et(day, h, m, s, 0).unwrap().0 / 1_000_000;
    assert_eq!(v["focus"]["from"], et_ms(15, 29, 0), "{}", v["focus"]);
    let to = v["focus"]["to"].as_i64().unwrap();
    assert!(
        (et_ms(17, 44, 0)..et_ms(17, 45, 0)).contains(&to),
        "{}",
        v["focus"]
    );

    // Without the journal, the restart itself (the server log's boot at 16:38:05 ET) makes the
    // same two setups warm: the replay models what the live run did.
    let logs = tmp.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(
        logs.join("server-2026-10-30.jsonl"),
        "{\"timestamp\":\"2026-10-30T20:38:05.000000Z\",\"level\":\"INFO\",\"message\":\"listening\"}\n",
    )
    .unwrap();
    let v = make(false, Some(logs)).package_blocking("INO", day, now);
    check(&v, "replay");
    assert_eq!(v["missed"][0]["restart_at"], 1_793_392_685_000i64);
}

// ---- The synthetic INO-shaped day (fictional symbol, synthetic prices: safe to commit) ----
//
// INO 2026-10-30 in miniature, on the detector fixtures' Friday (2026-10-16): a Runners flag,
// a server restart in the middle of the first setup's pullback, then setups the rules treat as
// on that day: A muted by the warm-up, B notified, C held by one-per-day, D past the 30-minute
// window.

use market_events::{Price, Trade};
use stock_context::detect::fixture::{at, end};

const SYM: &str = "SYNTH";
const MS: i64 = 1_000_000;
const SEC: i64 = 1_000_000_000;

fn fday() -> NaiveDate {
    stock_context::detect::fixture::day()
}

/// Synthetic tape builder: prints in micro-dollars on a $0.01 grid, admitted 100 ms after exec.
#[derive(Default)]
struct Tape {
    events: Vec<MarketEvent>,
    seq: u64,
    last: i64,
}

impl Tape {
    fn print(&mut self, exec: i64, micros: i64, size: u64) {
        self.seq += 1;
        let mut t = Trade::new(
            SYM,
            UnixNanos(exec),
            Price::from_micros(micros),
            size,
            self.seq,
        );
        t.receive_ts = Some(UnixNanos(exec + 100 * MS));
        self.events.push(MarketEvent::Trade(t));
        self.last = micros;
    }

    /// Slot `k`'s bar `[o, h, l, c]` (micros), like the detector fixtures' `Script::bar`.
    fn bar(&mut self, k: i64, ohlc: [i64; 4], size: u64) {
        let s = end(k) - 15 * SEC;
        let [o, h, l, c] = ohlc;
        self.print(s + SEC, o, size);
        self.print(s + 4 * SEC, h, size);
        self.print(s + 8 * SEC, l, size);
        self.print(s + 12 * SEC, c, size);
    }

    /// Flat bars at the last price over `ks`.
    fn flat(&mut self, ks: std::ops::Range<i64>) {
        let p = self.last;
        for k in ks {
            self.bar(k, [p, p, p, p], 100);
        }
    }

    /// The detector fixtures' micro-pullback (3.00 -> 3.40, two-bar retreat to 3.32, armed at the
    /// cutoff of slot `k0 + 11`, then a crossing), scaled to start at `base` micros.
    fn setup(&mut self, k0: i64, base: i64) {
        let px =
            |c: i64| ((c * 10_000) as i128 * base as i128 / 3_000_000 / 10_000 * 10_000) as i64;
        for k in 0..6 {
            self.bar(k0 + k, [px(300); 4], 100);
        }
        let bars: [([i64; 4], u64); 8] = [
            ([300, 302, 300, 302], 200),
            ([302, 310, 302, 310], 300),
            ([310, 325, 310, 325], 300),
            ([325, 340, 325, 338], 400),
            ([338, 338, 333, 335], 150),
            ([335, 336, 332, 334], 120),
            ([335, 341, 335, 342], 300),
            ([342, 344, 339, 343], 300),
        ];
        for (i, (b, size)) in bars.iter().enumerate() {
            self.bar(k0 + 6 + i as i64, b.map(px), *size);
        }
    }
}

/// Slots: A 0..14, B 24..38, C 48..62, D 160..174 (10:10 ET, past 30 min from the restart).
fn synthetic_day() -> (Vec<MarketEvent>, i64, i64) {
    let mut t = Tape::default();
    t.setup(0, 3_000_000);
    t.flat(14..24);
    let b = t.last;
    t.setup(24, b);
    t.flat(38..48);
    let c = t.last;
    t.setup(48, c);
    t.flat(62..160);
    let d = t.last;
    t.setup(160, d);
    t.flat(174..180);
    // The Runners flag before A, the restart in A's pullback (between its retreat bars).
    let flag = at(9, 31, 0);
    let boot = end(9) + 5 * SEC;
    (t.events, flag, boot)
}

fn verdicts(out: &replay::ReplayOut) -> Vec<(String, &'static str)> {
    out.decisions
        .iter()
        .map(|d| {
            (
                d.episode_id.rsplit(':').next().unwrap().to_string(),
                d.verdict.code(),
            )
        })
        .collect()
}

#[test]
fn the_replay_decides_each_setup_by_the_live_rules() {
    let (events, flag, boot) = synthetic_day();
    let out = replay::run(
        events,
        ReplayInput {
            symbol: SYM.into(),
            watch_from: Some(flag),
            boots: vec![boot],
            ..ReplayInput::default()
        },
    );
    assert_eq!(
        verdicts(&out),
        vec![
            ("1".into(), "warmup"),
            ("2".into(), "notified"),
            ("3".into(), "one_per_day"),
            ("4".into(), "window_closed"),
        ],
        "{:?}",
        out.decisions
    );
    assert!(out.events > 0 && !out.bars.is_empty());
    assert!(out.prints.windows(2).all(|w| w[0].exec <= w[1].exec));

    // No restart: A is the day's one notice and B is held by it.
    let (events, flag, _) = synthetic_day();
    let out = replay::run(
        events,
        ReplayInput {
            symbol: SYM.into(),
            watch_from: Some(flag),
            ..ReplayInput::default()
        },
    );
    assert_eq!(verdicts(&out)[0].1, "notified");
    assert_eq!(verdicts(&out)[1].1, "one_per_day");

    // Not a Runners row and not a member: nothing has context.
    let (events, _, _) = synthetic_day();
    let out = replay::run(
        events,
        ReplayInput {
            symbol: SYM.into(),
            ..ReplayInput::default()
        },
    );
    assert!(verdicts(&out).iter().all(|(_, v)| *v == "no_context"));
}

/// A day whose server restarts with the notice memory ([`replay::RestartModel::Remembering`]):
/// A armed on bars the new process saw live, so it notifies though its impulse began before the
/// restart (the INO 2026-10-30 fix), and the restored memory keeps B and C to one per day.
#[test]
fn a_remembering_server_announces_a_setup_that_arms_after_the_restart() {
    let (events, flag, boot) = synthetic_day();
    let out = replay::run(
        events,
        ReplayInput {
            symbol: SYM.into(),
            watch_from: Some(flag),
            boots: vec![boot],
            restarts: replay::RestartModel::Remembering,
            ..ReplayInput::default()
        },
    );
    assert_eq!(
        verdicts(&out),
        vec![
            ("1".into(), "notified"),
            ("2".into(), "one_per_day"),
            ("3".into(), "one_per_day"),
            ("4".into(), "window_closed"),
        ],
        "{:?}",
        out.decisions
    );
    // The model follows the day the server ran.
    assert_eq!(
        replay::RestartModel::for_day("2026-10-30".parse().unwrap()),
        replay::RestartModel::Forgetful
    );
    assert_eq!(
        replay::RestartModel::for_day("2026-11-02".parse().unwrap()),
        replay::RestartModel::Remembering
    );
}

/// The live journal's warm flag wins over the modelled one (module docs of `replay`).
#[test]
fn the_live_journal_decides_warm_up() {
    let (events, flag, _) = synthetic_day();
    let ep = |n: u32| format!("live:{}:{SYM}:micro_pullback:{n}", fday());
    let out = replay::run(
        events,
        ReplayInput {
            symbol: SYM.into(),
            watch_from: Some(flag),
            // No boot known here, but the journal recorded A as warm.
            live_warm: [(ep(1), true)].into_iter().collect(),
            ..ReplayInput::default()
        },
    );
    assert_eq!(verdicts(&out)[0].1, "warmup");
    assert_eq!(verdicts(&out)[1].1, "notified");
}

fn notice(event_id: &str, kind: &str, at_ns: i64, price: f64, sent: bool) -> Vec<String> {
    ["desk", "phone"]
        .iter()
        .map(|path| {
            json!({"kind":"decision","v":1,"event_id":event_id,"notice":kind,"symbol":SYM,
                "price":price,"at_ns":at_ns,"decided_at":at_ns / MS,"path":path,
                "decision": if sent || *path == "desk" { "sent" } else { "held" },
                "held": if sent || *path == "desk" { Value::Null } else { json!("quiet_hours") },
                "devices":1})
            .to_string()
        })
        .collect()
}

/// The whole package from files: tape in the archive, ledger, cache.
#[test]
fn the_package_lists_alerts_outcomes_and_missed_setups() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("archive");
    let cfg = ArchiveConfig {
        root: root.clone(),
        min_free_bytes: 0,
        ..ArchiveConfig::default()
    };
    let (events, flag, boot) = synthetic_day();
    let archive = Archive::new(cfg.clone());
    let mut w = archive
        .writer(fday(), SYM, market_archive::FsyncPolicy::Never)
        .unwrap();
    for e in &events {
        w.append(e).unwrap();
    }
    archive.commit(fday(), w.finalize().unwrap()).unwrap();
    // The boot, as the server log has it.
    let logs = tmp.path().join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let ts = chrono::DateTime::from_timestamp_nanos(boot).to_rfc3339();
    std::fs::write(
        logs.join(format!("server-{}.jsonl", fday())),
        format!(
            "{}\n{}\n",
            json!({"timestamp": ts, "level":"INFO", "message":"listening"}),
            json!({"timestamp": ts, "message":"other"})
        ),
    )
    .unwrap();

    // Replay first, to learn B's arming (the ledger names its episode).
    let out = replay::run(
        events.clone(),
        ReplayInput {
            symbol: SYM.into(),
            watch_from: Some(flag),
            boots: vec![boot],
            ..ReplayInput::default()
        },
    );
    let b = out.decisions[1].clone();
    let notices = tmp.path().join("notices");
    std::fs::create_dir_all(&notices).unwrap();
    let mut ledger = notice(
        &format!("runners-fast-{}-{SYM}-f1", fday()),
        "runners.fast",
        flag,
        3.0,
        true,
    );
    ledger.extend(notice(
        &format!("runners-pullback-{}", b.episode_id),
        "runners.pullback",
        b.at,
        3.42,
        false,
    ));
    // Another symbol's notice stays out of this page.
    ledger.push(json!({"kind":"decision","event_id":"x","notice":"runners.fast","symbol":"OTHR","path":"desk","decision":"sent","decided_at":flag / MS}).to_string());
    std::fs::write(notices.join(format!("{}.jsonl", fday())), ledger.join("\n")).unwrap();

    let cache = tmp.path().join("cache");
    let ins = Insights::new(
        Sources {
            archive: cfg,
            transitions_dir: tmp.path().join("transitions"),
            server_logs: Some(logs),
            cache_dir: Some(cache.clone()),
        },
        NoticeFeed::open(Some(notices), "2026-10-19".parse().unwrap()),
    );
    let now = at(12, 0, 0) + 3 * 86_400 * SEC;
    let v = ins.package_blocking(SYM, fday(), now);
    assert_eq!(v["available"], true, "{v}");
    assert_eq!(v["symbol"], SYM);
    assert_eq!(v["prev_day"]["day"], "2026-10-15");
    assert_eq!(v["next_day"]["day"], "2026-10-19");
    assert_eq!(v["source"]["boots"].as_array().unwrap().len(), 1);
    // The chart opens on the move: a window inside the day's bars that holds every marker the
    // window is anchored on (the sent alert), and is not the whole day.
    let bars = v["bars"].as_array().unwrap();
    let (b0, b1) = (
        bars[0]["t"].as_i64().unwrap(),
        bars[bars.len() - 1]["t"].as_i64().unwrap(),
    );
    let (f0, f1) = (
        v["focus"]["from"].as_i64().unwrap(),
        v["focus"]["to"].as_i64().unwrap(),
    );
    assert!(
        b0 <= f0 && f0 < f1 && f1 <= b1,
        "{} in {b0}..{b1}",
        v["focus"]
    );
    assert_eq!(v["focus"]["reason"], "alerts");
    for m in v["markers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == "alert")
    {
        let at = m["at"].as_i64().unwrap();
        assert!(
            f0 <= at && at <= f1 + 60_000,
            "marker {at} outside {}",
            v["focus"]
        );
    }

    // Two rows, oldest first; only the sent one is a marker.
    let alerts = v["alerts"].as_array().unwrap();
    assert_eq!(alerts.len(), 2);
    assert_eq!(alerts[0]["notice"], "runners.fast");
    assert_eq!(alerts[0]["marker"], 1);
    assert_eq!(alerts[0]["status"], "sent");
    assert_eq!(alerts[1]["status"], "held");
    assert_eq!(alerts[1]["reason"], "quiet_hours");
    assert!(alerts[1].get("marker").is_none());
    // The fast alert at $3.00 saw the day's high ($4.48 × 344/300 ≈ the last setup's top).
    let o = &alerts[0]["outcome"];
    assert_eq!(o["ref_price"], 3.0);
    assert!(o["best_pct"].as_f64().unwrap() > 50.0, "{o}");
    assert!(o["plus5_at"].is_i64());
    assert!(o["stop"].is_null());
    // The setup alert carries its stop (B's pullback low) and its crossing.
    let o = &alerts[1]["outcome"];
    assert!(o["stop"].as_f64().unwrap() > 3.0, "{o}");
    assert!(o["crossing"].is_object(), "{o}");

    // Missed: A (warm-up after the restart), C (one per day), D (window closed).
    let missed: Vec<(&str, &str)> = v["missed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["episode_id"].as_str().unwrap(),
                m["reason"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(missed.len(), 3, "{missed:?}");
    assert_eq!(missed[0].1, "warmup");
    assert_eq!(missed[1].1, "one_per_day");
    assert_eq!(missed[2].1, "window_closed");
    assert_eq!(v["missed"][0]["restart_at"], boot / MS);
    assert_eq!(
        v["missed"][1]["first_notified_episode"],
        b.episode_id.as_str()
    );
    assert!(v["missed"][2]["flagged_at"].is_i64());
    assert_eq!(v["missed"][0]["evidence"], "replay");

    let sc = &v["scorecard"];
    assert_eq!(
        (
            sc["alerts"].as_i64(),
            sc["sent"].as_i64(),
            sc["held"].as_i64()
        ),
        (Some(2), Some(1), Some(1))
    );
    assert_eq!(sc["setups_armed"], 4);
    assert_eq!(sc["missed"], 3);
    assert_eq!(sc["best_entry"]["notice"], "runners.fast");

    // Stats: all regular session; no close or float on file; short interest never collected.
    let st = &v["stats"];
    assert_eq!(st["sessions"]["regular"]["open"], 3.0);
    assert!(st["sessions"]["pre"].is_null());
    assert!(st["prev_close"].is_null());
    assert_eq!(st["float"]["available"], false);
    assert_eq!(st["short_interest"]["reason"], "not_collected");
    // The move was in the regular session: LULD applied and has a closest approach.
    assert_eq!(v["halts"]["outside_regular"], false);
    assert!(v["halts"]["closest"]["up"]["bp"].is_i64(), "{}", v["halts"]);

    // Cached: the second read is the same package, from disk.
    let file = cache.join(fday().to_string()).join(format!("{SYM}.json"));
    assert!(file.is_file());
    let again = ins.package_blocking(SYM, fday(), now);
    assert_eq!(again["missed"], v["missed"]);
    assert_eq!(again["alerts"], v["alerts"]);

    // Without B in the ledger, B's notice was never sent: it is missed, not notified.
    let ledger: Vec<String> = notice(
        &format!("runners-fast-{}-{SYM}-f1", fday()),
        "runners.fast",
        flag,
        3.0,
        true,
    );
    let notices2 = tmp.path().join("notices2");
    std::fs::create_dir_all(&notices2).unwrap();
    std::fs::write(
        notices2.join(format!("{}.jsonl", fday())),
        ledger.join("\n"),
    )
    .unwrap();
    let ins2 = Insights::new(
        Sources {
            archive: ArchiveConfig {
                root: root.clone(),
                ..ArchiveConfig::default()
            },
            transitions_dir: tmp.path().join("transitions"),
            server_logs: None,
            cache_dir: None,
        },
        NoticeFeed::open(Some(notices2), "2026-10-19".parse().unwrap()),
    );
    let v2 = ins2.package_blocking(SYM, fday(), now);
    // No boot known: A is the notified one now, and the ledger does not have it.
    assert_eq!(
        v2["missed"][0]["reason"], "not_recorded",
        "{}",
        v2["missed"]
    );
}

fn candle(t_min: i64, l: f64, h: f64) -> chart_engine::wire::CandleWire {
    chart_engine::wire::CandleWire {
        t: t_min * 60_000,
        t_ns: t_min * 60_000_000_000,
        o: l,
        h,
        l,
        c: h,
        v: 100,
        n: 1,
        is_final: true,
    }
}

/// A 4 AM to 8 PM day (minutes 0..960), flat at $1 except for a run at minutes 750..800 off
/// a base at minute 745, peaking at minute 790 (INO's shape: the move after hours).
fn run_day() -> Vec<chart_engine::wire::CandleWire> {
    (0..960)
        .map(|m| match m {
            745 => candle(m, 0.59, 0.62),
            750..800 => {
                let up = (40 - (m - 790).abs()) as f64 * 0.04;
                candle(m, 0.7 + up, 0.8 + up)
            }
            _ => candle(m, 0.95, 1.0),
        })
        .collect()
}

#[test]
fn the_chart_opens_on_the_move_not_the_whole_day() {
    use package::{FOCUS_LEAD_MS, FOCUS_TAIL_MS, focus_window};
    const M: i64 = 60_000;
    let bars = run_day();

    // Alerts at 752 and 784, a muted setup at 760 and late missed setups up to 900: from the
    // base (745, before the first alert) less the lead, to the high (790, within reach of the
    // last alert) plus the tail. The late setups do not stretch it.
    let (from, to, why) =
        focus_window(&bars, &[752 * M, 784 * M], &[760 * M, 880 * M, 900 * M]).unwrap();
    assert_eq!(why, "alerts");
    assert_eq!(from, 745 * M - FOCUS_LEAD_MS);
    assert_eq!(to, 790 * M + FOCUS_TAIL_MS);

    // A setup armed before the first alert opens it earlier still.
    let (from, _, _) = focus_window(&bars, &[752 * M], &[700 * M]).unwrap();
    assert_eq!(from, 700 * M - FOCUS_LEAD_MS);

    // Only setups (nothing sent): anchored on them, the last one ends it.
    let (from, to, why) = focus_window(&bars, &[], &[755 * M, 770 * M]).unwrap();
    assert_eq!(why, "setups");
    assert_eq!(from, 745 * M - FOCUS_LEAD_MS);
    assert_eq!(to, 790 * M + FOCUS_TAIL_MS);

    // A day high hours after the last alert is another move: the window ends after the alert.
    let mut late = run_day();
    late[950] = candle(950, 0.95, 9.0);
    let (_, to, _) = focus_window(&late, &[752 * M], &[]).unwrap();
    assert_eq!(to, 752 * M + FOCUS_TAIL_MS);

    // No alert and no setup: the day's run, base to high.
    let (from, to, why) = focus_window(&bars, &[], &[]).unwrap();
    assert_eq!(why, "move");
    assert_eq!(
        (from, to),
        (745 * M - FOCUS_LEAD_MS, 790 * M + FOCUS_TAIL_MS)
    );

    // Clamped to the bars at both ends.
    let (from, to, _) = focus_window(&bars, &[5 * M, 955 * M], &[]).unwrap();
    assert_eq!((from, to), (0, 959 * M));
    assert!(focus_window(&[], &[M], &[]).is_none());
}

#[test]
fn row_prices_read_as_their_notice_did() {
    use package::price_text;
    let row = |v: Value| v.as_object().unwrap().clone();
    // The title's own price wins, verbatim.
    assert_eq!(
        price_text(&row(
            json!({"title": "INO · Pullback setting up · $1.86", "price": 1.855})
        )),
        Some("$1.86".into())
    );
    // No price in the title: the Runners rule, half away from zero on the tick (1.855 is
    // 1.8549999… as a float, which a plain two-place format reads as $1.85).
    assert_eq!(
        price_text(&row(json!({"title": "INO · Volume shelf", "price": 1.855}))),
        Some("$1.86".into())
    );
    assert_eq!(
        price_text(&row(json!({"price": 0.8636999726295471}))),
        Some("$0.8637".into())
    );
    assert_eq!(
        price_text(&row(json!({"price": 0.78}))),
        Some("$0.7800".into())
    );
    assert_eq!(
        price_text(&row(json!({"title": "INO · Fast move · $x", "price": 2.0}))),
        Some("$2.00".into())
    );
    assert_eq!(price_text(&row(json!({"title": "INO · Fast move"}))), None);
}

#[test]
fn the_close_is_quoted_at_the_scheduled_end_of_the_regular_session() {
    let at = |d: &str, h, m| {
        market_events::session::et(d.parse().unwrap(), h, m, 0, 0)
            .unwrap()
            .0
            / 1_000_000
    };
    assert_eq!(
        package::regular_close_at("2026-10-30".parse().unwrap()),
        Some(at("2026-10-30", 16, 0))
    );
    // The day after Thanksgiving closes early.
    assert_eq!(
        package::regular_close_at("2026-11-27".parse().unwrap()),
        Some(at("2026-11-27", 13, 0))
    );
    assert_eq!(
        package::regular_close_at("2026-10-31".parse().unwrap()),
        None
    );
}

#[test]
fn days_without_data_say_why() {
    let tmp = tempfile::tempdir().unwrap();
    let ins = Insights::new(
        Sources {
            archive: ArchiveConfig {
                root: tmp.path().to_path_buf(),
                retention_days: 30,
                ..ArchiveConfig::default()
            },
            transitions_dir: tmp.path().join("t"),
            server_logs: None,
            cache_dir: None,
        },
        NoticeFeed::idle(),
    );
    let now = at(12, 0, 0);
    let saturday: NaiveDate = "2026-10-17".parse().unwrap();
    let v = ins.package_blocking(SYM, saturday, now + 86_400 * SEC);
    assert_eq!(v["unavailable_reason"], "not_a_trading_day");
    assert_eq!(v["prev_day"]["day"], "2026-10-16");
    let v = ins.package_blocking(SYM, fday(), now);
    assert_eq!(v["unavailable_reason"], "not_archived");
    assert_eq!(v["today"], true);
    assert!(v["next_day"].is_null(), "no next day after today");
    let v = ins.package_blocking(SYM, "2026-08-03".parse().unwrap(), now);
    assert_eq!(v["unavailable_reason"], "retention_expired");
    let v = ins.package_blocking(SYM, "2026-10-19".parse().unwrap(), now);
    assert_eq!(v["unavailable_reason"], "in_the_future");
    assert_eq!(next_trading_day(fday()).to_string(), "2026-10-19");
    assert!(parse_request("ino", Some("2026-10-30")).is_ok());
    assert!(parse_request("../x", None).is_err());
    assert!(parse_request("INO", Some("10/30")).is_err());
}

#[test]
fn outcomes_measure_best_plus5_stop_and_crossing() {
    let p = |s: i64, c: i64| (s * SEC, c * 10_000);
    // Alert at t=10 s, $2.00, stop $1.90, trigger $2.05.
    let prints = vec![
        p(5, 200),
        p(10, 200),
        p(11, 204),
        p(12, 206), // crossing
        p(13, 212), // +5% from 2.00 (2.10) and the best before the stop
        p(14, 189), // stop hit
        p(15, 260), // after the stop: not counted
    ];
    let o = package::measure(
        &prints,
        10 * SEC,
        2_000_000,
        Some(1_900_000),
        Some(2_050_000),
    );
    assert_eq!(o.best, Some((13 * SEC, 2_120_000)));
    assert_eq!(o.plus5_at, Some(13 * SEC));
    assert_eq!(o.stop_hit_at, Some(14 * SEC));
    let c = o.crossing.unwrap();
    assert_eq!((c.at, c.price), (12 * SEC, 2_060_000));
    assert_eq!(c.plus5_at, None, "2.12 < 2.06 × 1.05");
    // No stop: the best is the day's.
    let o = package::measure(&prints, 10 * SEC, 2_000_000, None, None);
    assert_eq!(o.best, Some((15 * SEC, 2_600_000)));
    // Stopped before the trigger: the crossing came after the stop.
    let o = package::measure(
        &prints,
        10 * SEC,
        2_000_000,
        Some(2_050_000),
        Some(2_100_000),
    );
    assert!(o.crossing.is_none());
    assert_eq!(o.crossed_after_stop, Some(13 * SEC));
    assert_eq!(
        package::episode_of("runners-pullback_crossing-live:2026-10-30:INO:micro_pullback:3"),
        Some(("pullback_crossing", "live:2026-10-30:INO:micro_pullback:3"))
    );
    assert_eq!(package::episode_of("runners-fast-2026-10-30-INO-f1"), None);
}

#[test]
fn live_journal_and_boots_are_read_from_their_files() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("t.jsonl");
    std::fs::write(
        &f,
        [
            json!({"symbol":"AAA","episode_id":"e1","warmup":true,"to":"pullback_forming","anchors":{"h0":2_000_000,"pullback_low":1_840_000}}),
            json!({"symbol":"AAA","episode_id":"e1","warmup":true,"to":"armed","observed_at":5,"anchors":{"h0":2_000_000,"pullback_low":1_840_000}}),
            json!({"symbol":"AAAB","episode_id":"e2","warmup":false,"to":"armed"}),
        ]
        .map(|v| v.to_string())
        .join("\n")
            + "\n{torn",
    )
    .unwrap();
    let eps = live_episodes(&f, "AAA");
    assert_eq!(eps.len(), 1);
    assert_eq!(
        eps["e1"],
        LiveEpisode {
            warm: true,
            trigger: Some(2_000_000),
            stop: Some(1_840_000),
            armed_at: Some(5)
        }
    );
    let log = tmp.path().join("server.jsonl");
    std::fs::write(
        &log,
        "{\"timestamp\":\"2026-10-30T20:38:05.000000Z\",\"level\":\"INFO\",\"message\":\"listening\"}\n",
    )
    .unwrap();
    let b = boots_from_log(&log);
    assert_eq!(b, vec![1_793_392_685_000_000_000]);
}

/// A followed day is built once for every follower and pushed to each.
#[tokio::test]
async fn followers_share_one_build() {
    let ins = Insights::idle();
    let mut a = ins.follow("INO".into(), fday());
    let mut b = ins.follow("INO".into(), fday());
    a.changed().await.unwrap();
    let pa = a.borrow_and_update().clone().unwrap();
    let current = b.borrow_and_update().clone();
    let pb = match current {
        Some(p) => p,
        None => {
            b.changed().await.unwrap();
            b.borrow_and_update().clone().unwrap()
        }
    };
    assert!(Arc::ptr_eq(&pa, &pb), "one build, shared");
    let v: Value = serde_json::from_str(&pa).unwrap();
    assert_eq!(v["symbol"], "INO");
    assert_eq!(ins.0.watchers.lock().unwrap().len(), 1);
}

/// The cache keeps only the days the archive still keeps: older day folders go, the rest (and
/// anything that is not a day) stay; retention 0 keeps everything.
#[test]
fn the_cache_is_pruned_to_the_archive_retention() {
    let tmp = tempfile::tempdir().unwrap();
    for d in ["2026-09-01", "2026-10-20", "2026-10-30", "notes"] {
        std::fs::create_dir_all(tmp.path().join(d)).unwrap();
    }
    let today: NaiveDate = "2026-10-31".parse().unwrap();
    prune_cache(tmp.path(), 0, today);
    assert!(tmp.path().join("2026-09-01").is_dir());
    prune_cache(tmp.path(), 30, today);
    assert!(!tmp.path().join("2026-09-01").exists());
    for d in ["2026-10-20", "2026-10-30", "notes"] {
        assert!(tmp.path().join(d).is_dir(), "{d} kept");
    }
}
