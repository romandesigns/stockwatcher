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
    assert_eq!(
        json[1],
        format!(
            r#"{{"type":"chart.update","symbol":"SMPL","timeframe":"15s","session":"2026-10-15/regular","candle":{{"t":{start},"o":3.18,"h":3.18,"l":3.18,"c":3.18,"v":100,"n":1,"final":false}},"profileBin":0.01,"profile":[{{"p":3.18,"v":100}}]}}"#
        )
    );
    assert_eq!(
        ups.iter().filter_map(|u| u.timeframe()).collect::<Vec<_>>(),
        Timeframe::ALL.to_vec()
    );
    // Increments round-trip, so the server can also read them back (e.g. from a recording).
    for (u, j) in ups.iter().zip(&json) {
        assert_eq!(
            &serde_json::from_str::<chart_engine::wire::ChartUpdate>(j).unwrap(),
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
    assert_eq!(v["candles"][0]["final"], true);
    assert_eq!(v["candles"][1]["final"], false);
    assert_eq!(
        v["gaps"][0],
        serde_json::json!({"t": ms(at(10, 1, 0, 0)), "end": ms(at(10, 3, 0, 0)), "reason": "no_trades", "v": 0, "intervals": 2})
    );
    assert_eq!(v["profile"]["anchor"], "2026-10-15/regular");
    assert_eq!(v["profile"]["poc"], 3.18);
    assert_eq!(
        v["profile"]["valueArea"],
        serde_json::json!({"low": 3.18, "high": 3.20, "v": 150})
    );
    assert_eq!(v["quality"]["duplicates"], 0);
    assert!(v.get("clock").is_none());
}
