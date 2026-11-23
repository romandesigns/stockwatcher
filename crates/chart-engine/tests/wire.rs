//! The JSON shapes the server forwards and the TypeScript chart consumes.
mod common;

use chart_engine::{ChartEngine, Timeframe};
use common::*;

#[test]
fn increment_json_shapes() {
    let mut e = ChartEngine::default();
    let ups = e.apply(&ev(
        trade(at(10, 0, 1, 250), "3.18", 100, 7).with_conditions(["@", "F"])
    ));
    let json: Vec<String> = ups
        .iter()
        .map(|u| serde_json::to_string(u).unwrap())
        .collect();
    let t = ms(at(10, 0, 1, 250));
    let ns = at(10, 0, 1, 250).nanos();
    assert_eq!(
        json[0],
        format!(
            r#"{{"type":"tape.trade","symbol":"SMPL","t":{t},"tsNs":{ns},"p":3.18,"s":100,"exchange":"V","seq":7,"conditions":["@","F"],"counted":"ohlc_and_volume","reason":null,"session":"2026-10-15/regular"}}"#
        )
    );
    let start = ms(at(10, 0, 0, 0));
    let start_ns = at(10, 0, 0, 0).nanos();
    assert_eq!(
        json[2],
        format!(
            r#"{{"type":"chart.update","symbol":"SMPL","timeframe":"15s","session":"2026-10-15/regular","candle":{{"t":{start},"tNs":{start_ns},"o":3.18,"h":3.18,"l":3.18,"c":3.18,"v":100,"n":1,"final":false}},"profileBin":0.01,"profile":[{{"p":3.18,"v":100}}]}}"#
        )
    );
    assert_eq!(
        ups.iter().filter_map(|u| u.timeframe()).collect::<Vec<_>>(),
        Timeframe::ALL.to_vec()
    );
    // The first trade of a session after 09:35 (no previous close known) publishes LULD bands
    // (10% of $3.18: 3.498 → 3.49 and 2.862 → 2.87), sent to every timeframe.
    assert_eq!(
        json[7],
        format!(
            r#"{{"type":"chart.luld","symbol":"SMPL","point":{{"t":{t},"tNs":{ns},"ref":3.18,"up":3.49,"down":2.87}}}}"#
        )
    );
    assert_eq!(ups.len(), 8);
    // Increments round-trip, so the server can also read them back (e.g. from a recording).
    for (u, j) in ups.iter().zip(&json) {
        assert_eq!(
            &serde_json::from_str::<chart_engine::wire::ChartUpdate>(j).unwrap(),
            u
        );
    }
}

#[test]
fn halt_json_shapes() {
    let mut e = ChartEngine::default();
    let halt = |kind, code: &str| {
        market_events::MarketEvent::Status(market_events::Status {
            symbol: Some(SYM.into()),
            event_ts: at(10, 0, 1, 250),
            receive_ts: None,
            source: market_events::Source::Fixture,
            kind,
            code: code.into(),
            reason: "LUDP".into(),
            tape: market_events::Tape::C,
        })
    };
    let ups = e.apply(&halt(market_events::StatusKind::Halt, "H"));
    let t = ms(at(10, 0, 1, 250));
    let ns = at(10, 0, 1, 250).nanos();
    assert_eq!(
        serde_json::to_string(&ups[0]).unwrap(),
        format!(
            r#"{{"type":"chart.halt","symbol":"SMPL","halt":{{"start":{t},"startNs":{ns},"end":null,"endNs":null,"reason":"LUDP","code":"H"}}}}"#
        )
    );
    let ups = e.apply(&halt(market_events::StatusKind::Resume, "T"));
    // The resume closes the same halt: same start and the halt's own code.
    assert_eq!(
        serde_json::to_string(&ups[0]).unwrap(),
        format!(
            r#"{{"type":"chart.halt","symbol":"SMPL","halt":{{"start":{t},"startNs":{ns},"end":{t},"endNs":{ns},"reason":"LUDP","code":"H"}}}}"#
        )
    );
    for u in &ups {
        let j = serde_json::to_string(u).unwrap();
        assert_eq!(
            &serde_json::from_str::<chart_engine::wire::ChartUpdate>(&j).unwrap(),
            u
        );
    }
}

#[test]
fn snapshot_json_shape() {
    let mut e = ChartEngine::default();
    apply_all(
        &mut e,
        &[
            ev(trade(at(10, 0, 1, 0), "3.18", 100, 1)),
            ev(trade(at(10, 3, 0, 0), "3.20", 50, 2)),
        ],
    );
    let v = serde_json::to_value(e.snapshot(SYM, Timeframe::M1, None).unwrap()).unwrap();
    assert_eq!(v["type"], "chart.snapshot");
    assert_eq!(v["timeframe"], "1m");
    assert_eq!(v["session"]["id"], "2026-10-15/regular");
    assert_eq!(v["session"]["start"], ms(at(9, 30, 0, 0)));
    assert_eq!(v["session"]["end"], ms(at(16, 0, 0, 0)));
    assert_eq!(v["session"]["startNs"], at(9, 30, 0, 0).nanos());
    assert_eq!(v["session"]["endNs"], at(16, 0, 0, 0).nanos());
    assert_eq!(v["candles"][0]["tNs"], at(10, 0, 0, 0).nanos());
    assert_eq!(v["candles"][0]["final"], true);
    assert_eq!(v["candles"][1]["final"], false);
    assert_eq!(
        v["gaps"][0],
        serde_json::json!({"t": ms(at(10, 1, 0, 0)), "tNs": at(10, 1, 0, 0).nanos(), "end": ms(at(10, 3, 0, 0)), "endNs": at(10, 3, 0, 0).nanos(), "reason": "no_trades", "v": 0, "intervals": 2})
    );
    assert_eq!(v["profile"]["anchor"], "2026-10-15/regular");
    assert_eq!(v["profile"]["poc"], 3.18);
    assert_eq!(
        v["profile"]["valueArea"],
        serde_json::json!({"low": 3.18, "high": 3.20, "v": 150})
    );
    assert_eq!(v["quality"]["duplicates"], 0);
    assert_eq!(v["quality"]["haltsUnmatched"], 0);
    assert!(v.get("clock").is_none());
    assert_eq!(v["halts"], serde_json::json!([]));
    assert_eq!(
        v["luld"],
        serde_json::json!([{"t": ms(at(10, 0, 1, 0)), "tNs": at(10, 0, 1, 0).nanos(), "ref": 3.18, "up": 3.49, "down": 2.87}])
    );
}

/// The snapshot carries the engine's own session schedule (`sessions.v1`): the trading date it
/// traded on, then the next trading date, from the NYSE calendar. On the day after Thanksgiving
/// (an early close) regular ends at 13:00 and after-hours at 17:00, which the clients' fixed
/// 09:30-16:00 schedule could not know.
#[test]
fn snapshot_carries_the_session_schedule_with_early_closes() {
    let mut e = ChartEngine::default();
    apply_all(&mut e, &[ev(trade(at(10, 0, 1, 0), "3.18", 100, 1))]);
    let v = serde_json::to_value(e.snapshot(SYM, Timeframe::M1, None).unwrap()).unwrap();
    let ids: Vec<&str> = v["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "2026-10-15/overnight",
            "2026-10-15/pre",
            "2026-10-15/regular",
            "2026-10-15/after",
            "2026-10-16/overnight",
            "2026-10-16/pre",
            "2026-10-16/regular",
            "2026-10-16/after",
        ]
    );
    assert_eq!(v["sessions"][2]["end"], ms(at(16, 0, 0, 0)));
    assert_eq!(v["sessions"][2]["endNs"], at(16, 0, 0, 0).nanos());

    // Friday 2026-11-27: regular 09:30-13:00, after 13:00-17:00; the next trading date is Monday.
    let friday = chrono::NaiveDate::from_ymd_opt(2026, 11, 27).unwrap();
    let et = |h, m| market_events::session::et(friday, h, m, 0, 0).unwrap();
    let mut e = ChartEngine::default();
    apply_all(&mut e, &[ev(trade(et(10, 0), "3.18", 100, 1))]);
    let v = serde_json::to_value(e.snapshot(SYM, Timeframe::M1, None).unwrap()).unwrap();
    let s = v["sessions"].as_array().unwrap();
    let find = |id: &str| s.iter().find(|x| x["id"] == id).unwrap().clone();
    assert_eq!(find("2026-11-27/regular")["end"], ms(et(13, 0)));
    assert_eq!(find("2026-11-27/after")["start"], ms(et(13, 0)));
    assert_eq!(find("2026-11-27/after")["end"], ms(et(17, 0)));
    assert!(s.iter().any(|x| x["id"] == "2026-11-30/regular"));
    assert!(
        !s.iter()
            .any(|x| x["id"].as_str().unwrap().starts_with("2026-11-28"))
    );
}

/// Every ms time has its exchange-ns twin, and the ms is that instant floored (wire module docs):
/// a halt inside a millisecond keeps its nanoseconds in the twin.
#[test]
fn ns_twins_floor_to_the_ms_fields() {
    let mut e = ChartEngine::default();
    let inside = market_events::UnixNanos(at(10, 0, 1, 250).nanos() + 999_999);
    let ups = e.apply(&market_events::MarketEvent::Status(market_events::Status {
        symbol: Some(SYM.into()),
        event_ts: inside,
        receive_ts: None,
        source: market_events::Source::Fixture,
        kind: market_events::StatusKind::Halt,
        code: "H".into(),
        reason: "LUDP".into(),
        tape: market_events::Tape::C,
    }));
    let h = ups[0].as_halt().unwrap();
    assert_eq!(h.start_ns, inside.nanos());
    assert_eq!(h.start, ms(at(10, 0, 1, 250)));
    // JSON from before the twins (a recording, an older golden file) still reads, twins zeroed.
    let old: chart_engine::wire::ChartUpdate = serde_json::from_str(
        r#"{"type":"chart.halt","symbol":"SMPL","halt":{"start":5,"end":null,"reason":"LUDP","code":"H"}}"#,
    )
    .unwrap();
    assert_eq!(old.as_halt().unwrap().start_ns, 0);
    assert_eq!(old.as_halt().unwrap().end_ns, None);
}
