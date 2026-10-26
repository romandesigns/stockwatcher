//! The Favorites panel (owner, 2026-10-26: "a favorite panel where I can add stocks to keep an eye
//! on. Every single one of the added items should give me a live view of the chart price action
//! and any other relevant information so I can be fully informed before clicking on it").
//!
//! # The list
//!
//! The owner's favorites are the synced settings namespace [`NS`] (`crate::settings`): an ordered
//! array of tickers, written by the clients (star toggles, the panel's remove and reorder). The
//! server stores it opaquely like every namespace, but it also READS this one, because the rows
//! are built here: [`follow`] reads it at start and again on every `settings.changed` for it, and
//! hands it to both markets ([`crate::market::Market::set_favorites`]). [`parse`] is the same
//! validation the clients apply (`parseFavorites` in packages/app-state), so a junk write from
//! another build becomes a shorter list, never an error.
//!
//! # The rows
//!
//! One row per favorite, in the list's order, built only from what the server already computes
//! for other boards, never invented: the quote board's last price, previous close, volume and
//! trades and the last print's time (a `stale` flag after [`STALE_NS`] without a print); the
//! chart engine's day and premarket extremes (`ChartEngine::day_extremes`); the day line
//! (`crate::day_line`, the Ross and Runners row chart); today's Runners context
//! (`RocketBoard::symbol_context`) and halt state; the micro-pullback levels (favorites join the
//! stock-context engine's watched set, `ContextBoard::set_favorites`, exactly as Runners rows do);
//! and the five-pillar decision (passes, news, RVOL, Ross membership) when the context engine
//! runs. Dollar volume and news headlines are not kept anywhere server-side, so a row has none;
//! the qualifying story's headline CLASS is (`pillars.news_kind`, `financing` marks dilution).
//!
//! # Wire (after `welcome`; additive, an older client never subscribes)
//!
//! - client `favorites.subscribe` -> server `favorites.snapshot {t, symbols, rows}` at once, then
//!   `favorites.update` (same shape) when a row or the list changed, at most once a second.
//!   `t` is the market clock (epoch ms); `symbols` the list as the server has it.
//! - A row: `symbol`, and when known `price`, `pc` (previous close), `ref` (the quote board's
//!   change reference, for when there is no previous close), `volume`, `trades`,
//!   `last_at` (ms), `stale`, `high`, `low`, `pm_high`, `pm_low` (prices in micros), `runners
//!   {kind, at, latest_kind, latest_at, entries, state}`, `levels` (the Runners row's),
//!   `pillars {passes, of, news, rvol, member}`, `halted`. Plus the day line fields exactly as
//!   `ross.*` rows carry them (whole lines in a snapshot, slices in an update).
//! - client `favorites.unsubscribe` stops the updates; a seek or a lag sends a fresh snapshot.
//!
//! # Cost
//!
//! At most [`MAX`] rows, built under the core lock once a second while anyone is subscribed: per
//! favorite a few hash lookups, one pass over its 30 s ring for the extremes and the day line.

use std::time::Duration;

use chart_engine::DayExtremes;
use serde_json::{Map, Value, json};
use stock_context::Tri;

use crate::market::Market;
use crate::settings::Settings;

/// The settings namespace that holds the list.
pub const NS: &str = "favorites";
/// How many favorites a list keeps (the first ones; the clients cap it the same way).
pub const MAX: usize = 40;
/// No print for this long and the row is marked stale (market clock).
pub const STALE_NS: i64 = 5 * 60 * 1_000_000_000;

/// A ticker as the app writes them (`parseSymbol` in packages/app-state): a letter, then up to 11
/// of `A-Z 0-9 . -`, upper case.
fn symbol(raw: &Value) -> Option<String> {
    let s = raw.as_str()?.trim().to_ascii_uppercase();
    let ok = (1..=12).contains(&s.len())
        && s.as_bytes()[0].is_ascii_uppercase()
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'.' || b == b'-');
    ok.then_some(s)
}

/// The stored value as a list: valid tickers only, first occurrence kept, at most [`MAX`].
pub fn parse(raw: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in raw.as_array().into_iter().flatten().filter_map(symbol) {
        if !out.contains(&s) {
            out.push(s);
        }
        if out.len() >= MAX {
            break;
        }
    }
    out
}

/// A favorite's five-pillar decision, as much as a row shows of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Pillars {
    pub passes: usize,
    pub of: usize,
    pub news: Option<Tri>,
    /// The qualifying story's headline class (`stock_context::NewsItem::event_type`), when any:
    /// lets a row mark dilution news (`financing`) without the headline text.
    pub news_kind: Option<String>,
    /// Daily-denominator relative volume, thousandths (the RVOL pillar's value).
    pub rvol_milli: Option<i64>,
    pub member: bool,
}

/// Everything one row is built from, gathered by the market under its lock.
#[derive(Clone, Debug, Default)]
pub struct RowFacts {
    pub symbol: String,
    pub price: Option<i64>,
    pub prev_close: Option<i64>,
    /// The quote board's change reference (the regular open, else the day's first price): what
    /// the search and the chart header measure against when there is no previous close.
    pub reference: Option<i64>,
    pub volume: Option<u64>,
    pub trades: Option<u64>,
    pub last_trade_ns: Option<i64>,
    pub extremes: Option<DayExtremes>,
    pub runners: Option<Value>,
    pub levels: Option<Value>,
    pub pillars: Option<Pillars>,
    pub halted: bool,
}

fn tri(t: Tri) -> &'static str {
    match t {
        Tri::Pass => "pass",
        Tri::Fail => "fail",
        Tri::Unknown => "unknown",
    }
}

/// One row at the market clock `now_ns` (module docs, "Wire"). Missing facts are left out.
pub fn row(f: &RowFacts, now_ns: i64) -> Value {
    let mut m = Map::new();
    m.insert("symbol".into(), json!(f.symbol));
    let mut put = |k: &str, v: Option<Value>| {
        if let Some(v) = v {
            m.insert(k.into(), v);
        }
    };
    put("price", f.price.map(|p| json!(p)));
    put("pc", f.prev_close.map(|p| json!(p)));
    put("ref", f.reference.map(|p| json!(p)));
    put("volume", f.volume.map(|v| json!(v)));
    put("trades", f.trades.map(|v| json!(v)));
    put("last_at", f.last_trade_ns.map(|t| json!(t / 1_000_000)));
    put(
        "stale",
        f.last_trade_ns.map(|t| json!(now_ns - t > STALE_NS)),
    );
    if let Some(e) = f.extremes {
        put("high", Some(json!(e.high.micros())));
        put("low", Some(json!(e.low.micros())));
        put("pm_high", e.pm_high.map(|p| json!(p.micros())));
        put("pm_low", e.pm_low.map(|p| json!(p.micros())));
    }
    put("runners", f.runners.clone());
    put("levels", f.levels.clone());
    put(
        "pillars",
        f.pillars.as_ref().map(|p| {
            json!({
                "passes": p.passes,
                "of": p.of,
                "news": p.news.map(tri),
                "news_kind": p.news_kind,
                "rvol": p.rvol_milli.map(|r| (r as f64 / 10.0).round() / 100.0),
                "member": p.member,
            })
        }),
    );
    if f.halted {
        m.insert("halted".into(), json!(true));
    }
    Value::Object(m)
}

/// The market's side of the panel: the list, and what the last update carried.
#[derive(Debug, Default)]
pub struct Board {
    pub(crate) list: Vec<String>,
    /// The rows (without day lines) the last update carried: the next goes out only on a change.
    pub(crate) rows: Option<Value>,
    /// The day lines the last update carried (`crate::day_line`).
    pub(crate) lines: crate::day_line::Published,
    /// The list changed since the last update: send one even if no row did.
    pub(crate) list_changed: bool,
    /// Bumped by every update and read by every snapshot: the frames' sequence. Not the engine
    /// seq, which stands still while a replay is paused, when a list edit must still go out.
    pub(crate) generation: u64,
}

/// Whether an update carrying `frame` (already decorated with slices) says anything new beyond
/// `rows`: a day-line slice with points, or a line that restarted.
pub(crate) fn has_new_points(frame: &Value) -> bool {
    frame["rows"].as_array().is_some_and(|rows| {
        rows.iter().any(|r| {
            r.get("day_line")
                .and_then(Value::as_array)
                .is_some_and(|l| !l.is_empty())
        })
    })
}

/// Reads the list from `settings` now and after every change to [`NS`], and hands it to each
/// market. Runs until the settings store is dropped.
pub fn follow(settings: Settings, markets: Vec<Market>) {
    tokio::spawn(async move {
        let mut rx = settings.subscribe();
        let apply = |v: Option<Value>| {
            let list = v.as_ref().map(parse).unwrap_or_default();
            for m in &markets {
                m.set_favorites(list.clone());
            }
        };
        apply(settings.get(NS).await.map(|(_, v)| v));
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    // Only this namespace's changes (a snapshot frame never comes this way).
                    let ours = serde_json::from_str::<Value>(&frame)
                        .is_ok_and(|v| v["type"] == "settings.changed" && v["ns"] == NS);
                    if ours {
                        apply(settings.get(NS).await.map(|(_, v)| v));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    apply(settings.get(NS).await.map(|(_, v)| v));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
            // A burst of writes (a drag reorder) settles before the next read.
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_events::Price;

    #[test]
    fn parse_keeps_valid_unique_upper_case_tickers_in_order_capped() {
        let raw = json!([
            "aapl",
            " TSLA ",
            "AAPL",
            "",
            7,
            "BRK.B",
            "TOO-LONG-TICKER",
            "a b",
            "x"
        ]);
        assert_eq!(parse(&raw), vec!["AAPL", "TSLA", "BRK.B", "X"]);
        assert!(parse(&json!({"a": 1})).is_empty());
        let many: Vec<Value> = (0..60).map(|i| json!(format!("S{i}"))).collect();
        let list = parse(&Value::Array(many));
        assert_eq!(list.len(), MAX);
        assert_eq!(list[0], "S0");
    }

    #[test]
    fn a_row_carries_what_is_known_and_leaves_the_rest_out() {
        let now = 1_761_500_000_000_000_000;
        let bare = row(
            &RowFacts {
                symbol: "NONE".into(),
                ..RowFacts::default()
            },
            now,
        );
        assert_eq!(bare, json!({"symbol": "NONE"}));
        let p = |s: &str| s.parse::<Price>().unwrap();
        let full = row(
            &RowFacts {
                symbol: "SMPL".into(),
                price: Some(3_450_000),
                prev_close: Some(3_000_000),
                reference: Some(3_100_000),
                volume: Some(1_200_000),
                trades: Some(4_100),
                last_trade_ns: Some(now - STALE_NS - 1),
                extremes: Some(DayExtremes {
                    high: p("3.54"),
                    low: p("3.05"),
                    pm_high: Some(p("3.18")),
                    pm_low: None,
                }),
                runners: Some(json!({"kind": "burst"})),
                levels: None,
                pillars: Some(Pillars {
                    passes: 4,
                    of: 5,
                    news: Some(Tri::Pass),
                    news_kind: Some("financing".into()),
                    rvol_milli: Some(7_412),
                    member: false,
                }),
                halted: true,
            },
            now,
        );
        assert_eq!(full["price"], 3_450_000);
        assert_eq!(full["pc"], 3_000_000);
        assert_eq!(full["ref"], 3_100_000);
        assert_eq!(full["stale"], true);
        assert_eq!(full["high"], 3_540_000);
        assert_eq!(full["pm_high"], 3_180_000);
        assert!(full.get("pm_low").is_none());
        assert!(full.get("levels").is_none());
        assert_eq!(
            full["pillars"],
            json!({"passes": 4, "of": 5, "news": "pass", "news_kind": "financing", "rvol": 7.41, "member": false})
        );
        assert_eq!(full["halted"], true);
        assert_eq!(full["runners"]["kind"], "burst");
    }

    /// The list a client writes reaches the market, and a later write replaces it.
    #[tokio::test]
    async fn follow_hands_the_written_list_to_the_markets() {
        use crate::settings::SettingsOp;
        let settings = Settings::in_memory();
        let market = Market::idle(market_providers::ClockMode::Replay, "idle", None);
        let set = |v: Value| SettingsOp::Set {
            value: v,
            if_absent: false,
        };
        settings
            .apply(NS, "op1", set(json!(["aapl", "TSLA"])))
            .await
            .unwrap();
        follow(settings.clone(), vec![market.clone()]);
        let symbols = |m: &Market| -> Value {
            serde_json::from_str::<Value>(&m.favorites_snapshot().1).unwrap()["symbols"].clone()
        };
        let wait = |m: Market, want: Value| async move {
            for _ in 0..100 {
                if symbols(&m) == want {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            false
        };
        assert!(wait(market.clone(), json!(["AAPL", "TSLA"])).await);
        settings
            .apply("layers", "op2", set(json!({"halts": true})))
            .await
            .unwrap();
        settings
            .apply(NS, "op3", set(json!(["NVDA"])))
            .await
            .unwrap();
        assert!(wait(market.clone(), json!(["NVDA"])).await);
    }

    #[test]
    fn new_points_are_a_non_empty_slice() {
        assert!(!has_new_points(
            &json!({"rows": [{"symbol": "A", "day_line": []}]})
        ));
        assert!(!has_new_points(&json!({"rows": [{"symbol": "A"}]})));
        assert!(has_new_points(
            &json!({"rows": [{"symbol": "A", "day_line": [1]}]})
        ));
    }
}
