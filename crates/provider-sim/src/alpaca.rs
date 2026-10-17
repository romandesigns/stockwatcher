//! Alpaca JSON shapes (market data v2 stream and REST), built from normalized events.
//!
//! Field names and types follow docs.alpaca.markets ("Real-time Stock Data", "Historical
//! trades/bars", "Snapshots"), read 2026-10-17. Times are RFC 3339 UTC with the fraction trimmed
//! to 0/3/6/9 digits (Alpaca prints nanosecond precision without trailing zero groups).

use chart_engine::wire::CandleWire;
use market_events::conditions::Tape;
use market_events::{
    Cancel, CancelAction, Correction, Quote, Status, StatusKind, Trade, UnixNanos,
};
use serde_json::{Value, json};

pub fn ts(t: UnixNanos) -> String {
    t.to_utc()
        .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

pub fn ts_ms(ms: i64) -> String {
    ts(UnixNanos::from_millis(ms))
}

pub fn tape(t: Tape) -> &'static str {
    match t {
        Tape::A => "A",
        Tape::B => "B",
        Tape::C => "C",
        Tape::O => "O",
        Tape::Unknown => "",
    }
}

/// REST trade entry: `{"t","x","p","s","c","i","z"}` (+ `"u"` when modified).
pub fn rest_trade(t: &Trade, update: Option<&str>) -> Value {
    let mut v = json!({
        "t": ts(t.event_ts),
        "x": t.exchange,
        "p": t.price.to_f64(),
        "s": t.size,
        "c": t.conditions,
        "i": t.source_seq,
        "z": tape(t.tape),
    });
    if let Some(u) = update {
        v["u"] = json!(u);
    }
    v
}

/// Stream trade: `{"T":"t","S","i","x","p","s","c","t","z"}`.
pub fn stream_trade(t: &Trade) -> Value {
    json!({
        "T": "t",
        "S": t.symbol,
        "i": t.source_seq,
        "x": t.exchange,
        "p": t.price.to_f64(),
        "s": t.size,
        "c": t.conditions,
        "t": ts(t.event_ts),
        "z": tape(t.tape),
    })
}

/// Quote body without `T`/`S` (REST snapshot `latestQuote`).
pub fn rest_quote(q: &Quote) -> Value {
    json!({
        "t": ts(q.event_ts),
        "ax": q.ask_exchange,
        "ap": q.ask_price.to_f64(),
        "as": q.ask_size,
        "bx": q.bid_exchange,
        "bp": q.bid_price.to_f64(),
        "bs": q.bid_size,
        "c": q.conditions,
        "z": tape(q.tape),
    })
}

pub fn stream_quote(q: &Quote) -> Value {
    let mut v = rest_quote(q);
    v["T"] = json!("q");
    v["S"] = json!(q.symbol);
    v
}

/// Bar body `{"t","o","h","l","c","v","n","vw"}`; `t` is the bar start (whole seconds).
pub fn rest_bar(c: &CandleWire, vw: f64) -> Value {
    json!({
        "t": ts_ms(c.t),
        "o": c.o,
        "h": c.h,
        "l": c.l,
        "c": c.c,
        "v": c.v,
        "n": c.n,
        "vw": round6(vw),
    })
}

/// Stream bar: `T` is `"b"` (minute bar) or `"u"` (updated bar).
pub fn stream_bar(kind: &str, symbol: &str, c: &CandleWire, vw: f64) -> Value {
    let mut v = rest_bar(c, vw);
    v["T"] = json!(kind);
    v["S"] = json!(symbol);
    v
}

pub fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn status_message(code: &str, kind: StatusKind) -> &'static str {
    match (code, kind) {
        ("H", _) | (_, StatusKind::Halt) => "Trading Halt",
        ("T", _) | (_, StatusKind::Resume) => "Trading Resumption",
        _ => "",
    }
}

fn reason_message(rc: &str) -> &'static str {
    match rc {
        "LUDP" => "Volatility Trading Pause",
        "T1" => "Halt News Pending",
        "T12" => "Trading Halted; For information requested by NASDAQ",
        _ => "",
    }
}

/// Status: `{"T":"s","S","sc","sm","rc","rm","t","z"}`. `None` for market-wide statuses, which
/// Alpaca's stock stream does not carry per symbol.
pub fn stream_status(s: &Status) -> Option<Value> {
    let sym = s.symbol.as_ref()?;
    Some(json!({
        "T": "s",
        "S": sym,
        "sc": s.code,
        "sm": status_message(&s.code, s.kind),
        "rc": s.reason,
        "rm": reason_message(&s.reason),
        "t": ts(s.event_ts),
        "z": tape(s.tape),
    }))
}

/// Correction: `{"T":"c","S","x","oi","op","os","oc","ci","cp","cs","cc","t","z"}`.
pub fn stream_correction(c: &Correction) -> Value {
    json!({
        "T": "c",
        "S": c.symbol,
        "x": c.exchange,
        "oi": c.original_id,
        "op": c.original_price.to_f64(),
        "os": c.original_size,
        "oc": c.original_conditions,
        "ci": c.corrected_id,
        "cp": c.corrected_price.to_f64(),
        "cs": c.corrected_size,
        "cc": c.corrected_conditions,
        "t": ts(c.event_ts),
        "z": tape(c.tape),
    })
}

/// Cancel/error: `{"T":"x","S","i","x","p","s","a","t","z"}`, `a` = `"C"` or `"E"`.
pub fn stream_cancel(c: &Cancel) -> Value {
    json!({
        "T": "x",
        "S": c.symbol,
        "i": c.trade_id,
        "x": c.exchange,
        "p": c.price.to_f64(),
        "s": c.size,
        "a": match c.action { CancelAction::Cancel => "C", CancelAction::Error => "E" },
        "t": ts(c.event_ts),
        "z": tape(c.tape),
    })
}

/// Control messages always arrive in arrays of one.
pub fn control(v: Value) -> String {
    Value::Array(vec![v]).to_string()
}

pub fn error_msg(code: u16, msg: &str) -> String {
    control(json!({"T": "error", "code": code, "msg": msg}))
}

pub fn success(msg: &str) -> String {
    control(json!({"T": "success", "msg": msg}))
}
