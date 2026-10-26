//! The daily scorecard over a real (synthetic) archive day: the maintainer's whole step, from the
//! alert logs and the imported tape to `scorecard/<date>.json`.

use chrono::{NaiveDate, Utc};
use market_archive::{Archive, ArchiveConfig, FsyncPolicy};
use market_events::session::et;
use market_events::{MarketEvent, Price, Source, Trade, UnixNanos};
use market_providers::import::scorecard;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 26).expect("valid")
}

fn t(h: u32, m: u32, s: u32) -> i64 {
    et(day(), h, m, s, 0).expect("valid").nanos()
}

fn trade(
    sym: &str,
    seq: u64,
    exec: i64,
    cents: i64,
    venue: &str,
    conditions: &[&str],
) -> MarketEvent {
    let tr = Trade::new(sym, UnixNanos(exec), Price::from_cents(cents), 100, seq)
        .with_conditions(conditions.iter().copied());
    MarketEvent::Trade(Trade {
        source: Source::AlpacaSip,
        exchange: venue.into(),
        ..tr
    })
}

fn write(a: &Archive, sym: &str, events: &[MarketEvent]) {
    let mut w = a
        .writer(day(), sym, FsyncPolicy::OnFinalize)
        .expect("writer");
    for e in events {
        w.append(e).expect("append");
    }
    let entry = w.finalize().expect("finalize");
    a.commit(day(), entry).expect("commit");
}

#[test]
fn a_ready_day_is_scored_once_and_again_only_when_its_logs_change() {
    let dir = tempfile::tempdir().expect("tmp");
    let root = dir.path().join("archive");
    let a = Archive::new(ArchiveConfig {
        root: root.clone(),
        retention_days: 0,
        min_free_bytes: 0,
    });
    // WIN: a run alert at 10:00 at $1.00, then $1.12 at 10:05 (follow-through).
    write(
        &a,
        "WIN",
        &[
            trade("WIN", 1, t(9, 40, 0), 90, "V", &["@"]),
            trade("WIN", 2, t(10, 5, 0), 112, "V", &["@"]),
        ],
    );
    // LOSE: a fast alert at 04:00:30 on a trade-report batch, then $0.94 (stopped).
    let mut lose: Vec<MarketEvent> = (0..10)
        .map(|i| {
            trade(
                "LOSE",
                10 + i,
                t(4, 0, 0) + i as i64 * 1_000_000,
                100,
                "D",
                &["T", "I"],
            )
        })
        .collect();
    lose.push(trade("LOSE", 30, t(4, 3, 0), 94, "Q", &["@", "T"]));
    write(&a, "LOSE", &lose);
    // HALT: a burst alert at 11:00 with no print after it (halted): fired, not scored.
    write(
        &a,
        "HALT",
        &[trade("HALT", 1, t(10, 59, 0), 200, "V", &["@"])],
    );

    let logs = root.join("rocket-watch");
    std::fs::create_dir_all(&logs).unwrap();
    let line = |k: &str, s: &str, at: i64, p: f64| {
        serde_json::json!({"kind": k, "v": 1, "symbol": s, "alert_ns": at, "price": p}).to_string()
    };
    std::fs::write(
        logs.join("2026-10-26.jsonl"),
        [
            line("run", "WIN", t(10, 0, 0), 1.0),
            line("fast", "LOSE", t(4, 0, 30), 1.0),
            line("burst", "HALT", t(11, 0, 0), 2.0),
        ]
        .join("\n"),
    )
    .unwrap();

    let r = scorecard::run(&a, Utc::now(), 25, &|| false);
    assert_eq!(r.written, vec![day()], "{r:?}");
    let c = scorecard::read_card(&root, day()).expect("card");
    let k = |kind: &str| {
        c.kinds
            .iter()
            .find(|x| x.kind == kind)
            .unwrap_or_else(|| panic!("{kind}: {:?}", c.kinds))
    };
    assert_eq!(
        (k("runners.run").tally.follow_through, k("runners.run").rate),
        (1, Some(1.0))
    );
    assert_eq!(k("runners.fast").tally.stopped, 1);
    assert_eq!(
        k("runners.fast").batch.fired,
        1,
        "fired on the 04:00 report batch"
    );
    assert_eq!(
        k("runners.fast")
            .segments
            .get("open_print")
            .map(|t| t.fired),
        Some(1)
    );
    let halt = &k("runners.burst").tally;
    assert_eq!((halt.fired, halt.scored), (1, 0));
    assert_eq!(
        k("runners.run").phone.get("unknown"),
        Some(&1),
        "no ledger that day"
    );

    // Nothing changed: not scored again. A grown log: scored again.
    assert_eq!(scorecard::run(&a, Utc::now(), 25, &|| false).unchanged, 1);
    std::fs::write(
        logs.join("2026-10-26.jsonl"),
        [line("run", "WIN", t(10, 0, 0), 1.0)].join("\n") + "\n\n\n",
    )
    .unwrap();
    assert_eq!(
        scorecard::run(&a, Utc::now(), 25, &|| false).written,
        vec![day()]
    );
}
