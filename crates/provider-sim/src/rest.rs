//! Alpaca REST subset. Every route reads the archive up to the replay clock (no future data).

use crate::Sim;
use crate::alpaca::{self, ts};
use crate::bars::BarBook;
use crate::universe::{UNIVERSE_MAX, UNIVERSE_MIN, in_price_band};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chart_engine::wire::CandleWire;
use chrono::{DateTime, Duration, NaiveDate, NaiveTime};
use market_events::session::{SessionKind, et_instant, session_bounds};
use market_events::{
    MarketEvent, Price, Quote, Resolution, SessionId, Trade, TradeId, TradeRef, UnixNanos, classify,
};
use market_replay::MarkClock;
use market_replay::merge::DeliveryClock;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use subtle::ConstantTimeEq;

const MAX_LIMIT: usize = 10_000;
const DEFAULT_LIMIT: usize = 1_000;

pub(crate) fn bad(msg: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"code": 40010001, "message": msg.into()})),
    )
        .into_response()
}

pub async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"message": "not found (route not implemented by the stockwatcher simulator)"})),
    )
        .into_response()
}

fn eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Alpaca auth: `APCA-API-KEY-ID` + `APCA-API-SECRET-KEY` headers, or HTTP Basic `key:secret`.
pub(crate) fn credentials_ok(sim: &Sim, headers: &HeaderMap) -> bool {
    let cfg = sim.config();
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
    if let (Some(k), Some(s)) = (h("apca-api-key-id"), h("apca-api-secret-key")) {
        return eq(k, &cfg.alpaca_key) & eq(s, &cfg.alpaca_secret);
    }
    if let Some(basic) = h(header::AUTHORIZATION.as_str()).and_then(|v| v.strip_prefix("Basic "))
        && let Ok(raw) = STANDARD.decode(basic.trim())
        && let Ok(raw) = String::from_utf8(raw)
        && let Some((k, s)) = raw.split_once(':')
    {
        return eq(k, &cfg.alpaca_key) & eq(s, &cfg.alpaca_secret);
    }
    false
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"code": 40110000, "message": "request is not authorized"})),
    )
        .into_response()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Feed {
    Sip,
    Boats,
}

impl Feed {
    fn parse(s: Option<&str>) -> Result<Feed, String> {
        match s.map(str::to_ascii_lowercase).as_deref() {
            None | Some("") | Some("sip") => Ok(Feed::Sip),
            Some("boats") | Some("overnight") => Ok(Feed::Boats),
            Some(other) => Err(format!(
                "feed {other:?} is not available in the simulator (sip, boats, overnight)"
            )),
        }
    }

    /// SIP carries the pre/regular/after sessions; BOATS/overnight carry the overnight session.
    fn accepts(self, ts: UnixNanos) -> bool {
        let overnight = classify(ts).is_some_and(|s| s.kind == SessionKind::Overnight);
        overnight == (self == Feed::Boats)
    }
}

/// What a scan of one symbol's archive file up to the clock found.
#[derive(Default)]
struct Scan {
    /// Every print visible at the clock, with Alpaca's `u` flag for modified prints.
    trades: Vec<(Trade, Option<&'static str>)>,
    latest_trade: Option<Trade>,
    latest_quote: Option<Quote>,
    book: Option<BarBook>,
}

fn scan(sim: &Sim, symbol: &str, feed: Feed, bars: bool) -> Result<Scan, String> {
    let now = sim.now().nanos();
    let reader = sim
        .0
        .archive
        .reader(sim.day_info().day, symbol)
        .map_err(|e| e.to_string())?;
    let mut dc = DeliveryClock::default();
    let mut marks = MarkClock::default();
    let mut out = Scan {
        book: bars.then(BarBook::default),
        ..Scan::default()
    };
    // Provider id → positions of the prints carrying it: several on venue `D`, where FINRA's TRFs
    // reuse ids, so a revision resolves among the still-standing ones as the chart engine does
    // (docs/architecture/trade-identity.md), and an ambiguous one marks nothing.
    let mut index: HashMap<TradeId, Vec<usize>> = HashMap::new();
    let standing = |trades: &[(Trade, Option<&'static str>)],
                    index: &HashMap<TradeId, Vec<usize>>,
                    r: &TradeRef| {
        let held = index.get(&r.id).into_iter().flatten().filter_map(|&i| {
            let (t, mark) = &trades[i];
            matches!(mark, None | Some("corrected")).then_some((i, t.price, t.size))
        });
        match r.resolve(held) {
            Resolution::Found(i) => Some(i),
            Resolution::NotFound | Resolution::Ambiguous => None,
        }
    };
    for ev in reader {
        let e = ev.map_err(|e| e.to_string())?.event;
        let d = dc.next(&e);
        if d > now {
            break; // delivery is monotone per file: nothing later is visible yet
        }
        if !feed.accepts(e.event_ts()) {
            continue;
        }
        if let Some(book) = out.book.as_mut() {
            if let Some(m) = marks.mark(d) {
                book.advance(m);
            }
            book.apply(&e);
        }
        match e {
            MarketEvent::Trade(t) => {
                index.entry(t.id()).or_default().push(out.trades.len());
                out.latest_trade = Some(t.clone());
                out.trades.push((t, None));
            }
            MarketEvent::Quote(q) => out.latest_quote = Some(q),
            MarketEvent::Cancel(c) => {
                if let Some(i) = standing(&out.trades, &index, &c.target()) {
                    out.trades[i].1 = Some("canceled");
                }
            }
            MarketEvent::Correction(c) => {
                if let Some(i) = standing(&out.trades, &index, &c.target()) {
                    out.trades[i].1 = Some("incorrect");
                    let mut t = out.trades[i].0.clone();
                    t.source_seq = c.corrected_id;
                    t.price = c.corrected_price;
                    t.size = c.corrected_size;
                    t.conditions = c.corrected_conditions.clone();
                    index.entry(t.id()).or_default().push(out.trades.len());
                    out.trades.push((t, Some("corrected")));
                }
            }
            MarketEvent::Status(_) => {}
        }
    }
    if let Some(book) = out.book.as_mut()
        && let Some(m) = marks.mark(now)
    {
        book.advance(m);
    }
    Ok(out)
}

fn et_midnight(d: NaiveDate) -> i64 {
    et_instant(d, NaiveTime::MIN).map_or(0, UnixNanos::nanos)
}

/// RFC 3339, or `YYYY-MM-DD` (ET midnight; as an `end`, the last instant of that ET day).
fn parse_time(s: &str, is_end: bool) -> Result<i64, String> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return dt
            .timestamp_nanos_opt()
            .ok_or_else(|| format!("time out of range: {s}"));
    }
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| format!("invalid time {s:?} (RFC 3339 or YYYY-MM-DD)"))?;
    Ok(if is_end {
        et_midnight(d + Duration::days(1)) - 1
    } else {
        et_midnight(d)
    })
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    start: Option<String>,
    end: Option<String>,
    limit: Option<String>,
    page_token: Option<String>,
    sort: Option<String>,
    feed: Option<String>,
    timeframe: Option<String>,
    adjustment: Option<String>,
}

struct Window {
    start: i64,
    end: i64,
    offset: usize,
    limit: usize,
    desc: bool,
}

fn window(sim: &Sim, q: &ListQuery) -> Result<Window, String> {
    let now = sim.now().nanos();
    let limit = match q.limit.as_deref() {
        None | Some("") => DEFAULT_LIMIT,
        Some(l) => match l.parse::<usize>() {
            Ok(n) if (1..=MAX_LIMIT).contains(&n) => n,
            _ => return Err(format!("invalid limit {l:?} (1..=10000)")),
        },
    };
    let desc = match q.sort.as_deref() {
        None | Some("") | Some("asc") => false,
        Some("desc") => true,
        Some(s) => return Err(format!("invalid sort {s:?}")),
    };
    let start = match q.start.as_deref() {
        Some(s) if !s.is_empty() => parse_time(s, false)?,
        _ => et_midnight(sim.day_info().day),
    };
    // The page token pins `end`, so later pages see the same set even as the clock moves.
    let (offset, pinned_end) = match q.page_token.as_deref() {
        Some(t) if !t.is_empty() => {
            let raw = URL_SAFE_NO_PAD
                .decode(t)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .ok_or_else(|| "invalid page_token".to_string())?;
            let (o, e) = raw
                .split_once(':')
                .ok_or_else(|| "invalid page_token".to_string())?;
            (
                o.parse().map_err(|_| "invalid page_token".to_string())?,
                Some(
                    e.parse::<i64>()
                        .map_err(|_| "invalid page_token".to_string())?,
                ),
            )
        }
        _ => (0, None),
    };
    let end = match (pinned_end, q.end.as_deref()) {
        (Some(e), _) => e,
        (None, Some(s)) if !s.is_empty() => parse_time(s, true)?,
        _ => now,
    }
    .min(now); // never past the replay clock
    Ok(Window {
        start,
        end,
        offset,
        limit,
        desc,
    })
}

/// Sorts, pages and returns `(items, next_page_token)`.
fn page<T>(mut items: Vec<T>, w: &Window) -> (Vec<T>, Option<String>) {
    if w.desc {
        items.reverse();
    }
    let total = items.len();
    let items: Vec<T> = items.into_iter().skip(w.offset).take(w.limit).collect();
    let next = (w.offset + items.len() < total)
        .then(|| URL_SAFE_NO_PAD.encode(format!("{}:{}", w.offset + items.len(), w.end)));
    (items, next)
}

fn known(sim: &Sim, symbol: &str) -> bool {
    sim.day_info().symbols.iter().any(|s| s == symbol)
}

pub async fn trades(
    State(sim): State<Sim>,
    headers: HeaderMap,
    Path(symbol): Path<String>,
    Query(q): Query<ListQuery>,
) -> Response {
    if !credentials_ok(&sim, &headers) {
        return unauthorized();
    }
    let symbol = symbol.to_ascii_uppercase();
    let feed = match Feed::parse(q.feed.as_deref()) {
        Ok(f) => f,
        Err(m) => return bad(m),
    };
    let w = match window(&sim, &q) {
        Ok(w) => w,
        Err(m) => return bad(m),
    };
    let mut rows: Vec<(Trade, Option<&'static str>)> = if known(&sim, &symbol) {
        match scan(&sim, &symbol, feed, false) {
            Ok(s) => s.trades,
            Err(e) => return internal(e),
        }
    } else {
        Vec::new()
    };
    rows.retain(|(t, _)| (w.start..=w.end).contains(&t.event_ts.nanos()));
    rows.sort_by_key(|(t, _)| (t.event_ts, t.source_seq));
    let (rows, next) = page(rows, &w);
    Json(json!({
        "trades": rows.iter().map(|(t, u)| alpaca::rest_trade(t, *u)).collect::<Vec<_>>(),
        "symbol": symbol,
        "currency": "USD",
        "next_page_token": next,
    }))
    .into_response()
}

fn internal(e: String) -> Response {
    tracing::error!(error = %e, "simulator archive read failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"code": 50010000, "message": "internal server error"})),
    )
        .into_response()
}

pub async fn bars(
    State(sim): State<Sim>,
    headers: HeaderMap,
    Path(symbol): Path<String>,
    Query(q): Query<ListQuery>,
) -> Response {
    if !credentials_ok(&sim, &headers) {
        return unauthorized();
    }
    match q.timeframe.as_deref() {
        Some("1Min") | Some("1T") => {}
        None | Some("") => return bad("timeframe is required"),
        Some(tf) => {
            return bad(format!(
                "timeframe {tf:?} is not supported by the simulator (only 1Min; Alpaca has no 15s bars)"
            ));
        }
    }
    if !matches!(q.adjustment.as_deref(), None | Some("") | Some("raw")) {
        return bad("the simulator serves raw (unadjusted) bars only");
    }
    let symbol = symbol.to_ascii_uppercase();
    let feed = match Feed::parse(q.feed.as_deref()) {
        Ok(f) => f,
        Err(m) => return bad(m),
    };
    let w = match window(&sim, &q) {
        Ok(w) => w,
        Err(m) => return bad(m),
    };
    let rows: Vec<Value> = if known(&sim, &symbol) {
        let s = match scan(&sim, &symbol, feed, true) {
            Ok(s) => s,
            Err(e) => return internal(e),
        };
        let book = s.book.expect("requested");
        let start_ms = w.start.div_euclid(1_000_000);
        let end_ms = w.end.div_euclid(1_000_000);
        book.candles(&symbol)
            .into_iter()
            .filter(|c| c.is_final && c.t >= start_ms && c.t <= end_ms)
            .map(|c| alpaca::rest_bar(&c, book.vw(&symbol, &c)))
            .collect()
    } else {
        Vec::new()
    };
    let (rows, next) = page(rows, &w);
    Json(json!({
        "bars": rows,
        "symbol": symbol,
        "currency": "USD",
        "next_page_token": next,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct SnapshotQuery {
    symbols: Option<String>,
    feed: Option<String>,
}

fn daily_bar(day: NaiveDate, sym: &str, book: &BarBook, candles: &[CandleWire]) -> Value {
    let Some((a, b)) = session_bounds(SessionId::new(day, SessionKind::Regular)) else {
        return Value::Null;
    };
    let reg: Vec<&CandleWire> = candles
        .iter()
        .filter(|c| c.t >= a.millis() && c.t < b.millis())
        .collect();
    let (Some(first), Some(last)) = (reg.first(), reg.last()) else {
        return Value::Null;
    };
    let v: u64 = reg.iter().map(|c| c.v).sum();
    let pv: f64 = reg.iter().map(|c| book.vw(sym, c) * c.v as f64).sum();
    json!({
        "t": ts(UnixNanos(et_midnight(day))),
        "o": first.o,
        "h": reg.iter().map(|c| c.h).fold(f64::MIN, f64::max),
        "l": reg.iter().map(|c| c.l).fold(f64::MAX, f64::min),
        "c": last.c,
        "v": v,
        "n": reg.iter().map(|c| c.n).sum::<u64>(),
        "vw": if v > 0 { alpaca::round6(pv / v as f64) } else { last.c },
    })
}

pub async fn snapshots(
    State(sim): State<Sim>,
    headers: HeaderMap,
    Query(q): Query<SnapshotQuery>,
) -> Response {
    if !credentials_ok(&sim, &headers) {
        return unauthorized();
    }
    let Some(list) = q.symbols.filter(|s| !s.trim().is_empty()) else {
        return bad("symbols is required");
    };
    let feed = match Feed::parse(q.feed.as_deref()) {
        Ok(f) => f,
        Err(m) => return bad(m),
    };
    let mut out = Map::new();
    for sym in list.split(',').map(|s| s.trim().to_ascii_uppercase()) {
        if sym.is_empty() || !known(&sim, &sym) || out.contains_key(&sym) {
            continue;
        }
        let s = match scan(&sim, &sym, feed, true) {
            Ok(s) => s,
            Err(e) => return internal(e),
        };
        let book = s.book.as_ref().expect("requested");
        let candles = book.candles(&sym);
        let minute = candles
            .iter()
            .rev()
            .find(|c| c.is_final)
            .map_or(Value::Null, |c| alpaca::rest_bar(c, book.vw(&sym, c)));
        let mut latest_trade = s
            .latest_trade
            .as_ref()
            .map_or(Value::Null, |t| alpaca::rest_trade(t, None));
        if let Value::Object(m) = &mut latest_trade {
            m.remove("u");
        }
        out.insert(
            sym.clone(),
            json!({
                "latestTrade": latest_trade,
                "latestQuote": s.latest_quote.as_ref().map_or(Value::Null, alpaca::rest_quote),
                "minuteBar": minute,
                "dailyBar": daily_bar(sim.day_info().day, &sym, book, &candles),
                "prevDailyBar": Value::Null,
            }),
        );
    }
    Json(Value::Object(out)).into_response()
}

#[derive(Deserialize)]
pub struct AssetQuery {
    status: Option<String>,
    asset_class: Option<String>,
    exchange: Option<String>,
}

fn asset_id(symbol: &str) -> String {
    let h = Sha256::digest(format!("stockwatcher-sim-asset:{symbol}").as_bytes());
    let x = hex(&h[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &x[0..8],
        &x[8..12],
        &x[12..16],
        &x[16..20],
        &x[20..32]
    )
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub async fn assets(
    State(sim): State<Sim>,
    headers: HeaderMap,
    Query(q): Query<AssetQuery>,
) -> Response {
    if !credentials_ok(&sim, &headers) {
        return unauthorized();
    }
    let class_ok = q
        .asset_class
        .as_deref()
        .is_none_or(|c| c.is_empty() || c == "us_equity");
    let status_ok = q
        .status
        .as_deref()
        .is_none_or(|s| s.is_empty() || s == "active");
    let exch_ok = q
        .exchange
        .as_deref()
        .is_none_or(|e| e.is_empty() || e.eq_ignore_ascii_case("NASDAQ"));
    let list: Vec<Value> = if class_ok && status_ok && exch_ok {
        let day = sim.day_info();
        day.symbols
            .iter()
            .map(|s| {
                json!({
                    "id": asset_id(s),
                    "class": "us_equity",
                    "exchange": "NASDAQ",
                    "symbol": s,
                    "name": if day.fixture {
                        format!("{s} Fixture Inc. (simulator fixture, not a real security)")
                    } else {
                        format!("{s} (archived; simulator)")
                    },
                    "status": "active",
                    "tradable": true,
                    "marginable": false,
                    "maintenance_margin_requirement": 100,
                    "margin_requirement_long": "100",
                    "margin_requirement_short": "100",
                    "shortable": false,
                    "easy_to_borrow": false,
                    "fractionable": false,
                    "attributes": [],
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    Json(list).into_response()
}

#[derive(Deserialize)]
pub struct UniverseQuery {
    min: Option<f64>,
    max: Option<f64>,
}

/// Simulator-only helper (not an Alpaca route): the price-band universe at the replay clock.
pub async fn universe(
    State(sim): State<Sim>,
    headers: HeaderMap,
    Query(q): Query<UniverseQuery>,
) -> Response {
    if !credentials_ok(&sim, &headers) {
        return unauthorized();
    }
    let min = q
        .min
        .and_then(Price::from_f64_rounded)
        .unwrap_or(UNIVERSE_MIN);
    let max = q
        .max
        .and_then(Price::from_f64_rounded)
        .unwrap_or(UNIVERSE_MAX);
    let mut rows = Vec::new();
    let day = sim.day_info();
    for sym in &day.symbols {
        let mut latest = None;
        for feed in [Feed::Sip, Feed::Boats] {
            if let Ok(s) = scan(&sim, sym, feed, false)
                && let Some(t) = s.latest_trade
                && latest
                    .as_ref()
                    .is_none_or(|l: &Trade| t.event_ts > l.event_ts)
            {
                latest = Some(t);
            }
        }
        let price = latest.map(|t| t.price);
        rows.push(json!({
            "symbol": sym,
            "price": price.map(Price::to_f64),
            "inBand": price.is_some_and(|p| in_price_band(p, min, max)),
        }));
    }
    Json(json!({
        "simulatorOnly": true,
        "asOf": ts(sim.now()),
        "min": min.to_f64(),
        "max": max.to_f64(),
        "symbols": rows,
    }))
    .into_response()
}
