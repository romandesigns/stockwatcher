//! # provider-sim
//!
//! A local surface compatible with the **subset** of Alpaca and FMP that Stockwatcher uses,
//! served from replay, so code written against the real providers runs unchanged in simulation
//! by swapping base URLs. It never calls an external service and accepts only the dummy local
//! credentials from [`SimConfig`].
//!
//! Mounted on its own router and port (`SIM_PORT`, default 8791, loopback by default):
//!
//! | Real base URL | Simulator base URL |
//! |---|---|
//! | `https://data.alpaca.markets` | `http://127.0.0.1:8791` |
//! | `https://paper-api.alpaca.markets` | `http://127.0.0.1:8791` |
//! | `wss://stream.data.alpaca.markets` | `ws://127.0.0.1:8791` |
//! | `https://financialmodelingprep.com` | `http://127.0.0.1:8791` |
//!
//! ## Implemented
//!
//! - `GET /v2/stocks/{symbol}/trades` (`start`, `end`, `limit` 1–10000, `page_token`, `sort`,
//!   `feed` = `sip` | `boats` | `overnight`)
//! - `GET /v2/stocks/{symbol}/bars` (`timeframe=1Min`/`1T` only; built by the chart engine)
//! - `GET /v2/stocks/snapshots?symbols=` (`latestTrade`, `latestQuote`, `minuteBar`, `dailyBar`;
//!   `prevDailyBar` is `null`)
//! - `GET /v2/assets` (from the archive manifest; `status`, `asset_class`, `exchange` filters)
//! - WebSocket `/v2/sip`, `/v1beta1/boats`, `/v1beta1/overnight` (auth, subscribe/unsubscribe
//!   `trades` `quotes` `bars` `updatedBars` `statuses`; `corrections`/`cancelErrors` follow
//!   `trades`; `dailyBars`/`lulds` are accepted and never emitted)
//! - WebSocket `/v1beta1/news` (auth, `news` subscription; emits nothing: the archive has no news)
//! - FMP `GET /stable/shares-float?symbol=` (fixture values, header `x-stockwatcher-fixture: true`)
//! - `GET /sim/universe?min=&max=` — **simulator-only**, not an Alpaca route: symbols whose latest
//!   trade at the replay clock is inside the $0.15–$25 band (or the given one)
//!
//! Everything else answers 404. The full list, including what is deliberately not implemented,
//! is in `docs/architecture/simulator-and-chart.md`, Appendix B.
//!
//! ## No future data
//!
//! REST reads the archive file up to the replay clock: an event is visible iff its delivery time
//! (`market-replay`'s rule) is ≤ the clock's `now`, and `end` is clamped to `now`. The stream
//! emits exactly what the replay emits, in order, as it emits it.

pub mod alpaca;
pub mod bars;
mod fmp;
mod rest;
mod stream;
pub mod universe;

use axum::Router;
use axum::routing::get;
use bars::BarBook;
use chart_engine::wire::ChartUpdate;
use chrono::NaiveDate;
use market_archive::Archive;
use market_events::session::SessionKind;
use market_events::{MarketEvent, classify};
use market_replay::{ClockState, FeedItem};
use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, watch};

/// Simulator configuration. The defaults are **dummy local credentials**, not secrets; real
/// provider credentials are never accepted or read here.
///
/// | Variable | Default |
/// |---|---|
/// | `SIM_ENABLED` | `1` (replay mode only) |
/// | `SIM_HOST` | `127.0.0.1` |
/// | `SIM_PORT` | `8791` |
/// | `SIM_ALPACA_KEY` / `SIM_ALPACA_SECRET` | `sim-key` / `sim-secret` |
/// | `SIM_FMP_KEY` | `sim-fmp-key` |
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimConfig {
    pub enabled: bool,
    pub host: IpAddr,
    pub port: u16,
    pub alpaca_key: String,
    pub alpaca_secret: String,
    pub fmp_key: String,
}

impl Default for SimConfig {
    fn default() -> Self {
        SimConfig {
            enabled: true,
            host: IpAddr::from([127, 0, 0, 1]),
            port: 8791,
            alpaca_key: "sim-key".into(),
            alpaca_secret: "sim-secret".into(),
            fmp_key: "sim-fmp-key".into(),
        }
    }
}

impl SimConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut c = SimConfig::default();
        if let Some(v) = var("SIM_ENABLED") {
            c.enabled = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Some(h) = var("SIM_HOST") {
            c.host = h
                .parse()
                .map_err(|_| format!("SIM_HOST is not an IP address: {h}"))?;
        }
        if let Some(p) = var("SIM_PORT") {
            c.port = p
                .parse()
                .map_err(|_| format!("SIM_PORT is not a port: {p}"))?;
        }
        if let Some(k) = var("SIM_ALPACA_KEY") {
            c.alpaca_key = k;
        }
        if let Some(s) = var("SIM_ALPACA_SECRET") {
            c.alpaca_secret = s;
        }
        if let Some(k) = var("SIM_FMP_KEY") {
            c.fmp_key = k;
        }
        Ok(c)
    }
}

/// Which WebSocket a stream message belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Trades,
    Quotes,
    Bars,
    UpdatedBars,
    Statuses,
    Corrections,
    CancelErrors,
}

/// One Alpaca stream message, serialized once and fanned out.
#[derive(Clone, Debug)]
pub struct StreamMsg {
    pub symbol: String,
    pub channel: Channel,
    /// In the overnight session: served on the BOATS/overnight streams, not on SIP.
    pub overnight: bool,
    pub json: String,
}

struct Live {
    book: BarBook,
    /// Last minute-bar start (ms) emitted as `"b"` per symbol; a later final revision of an
    /// earlier minute is an updated bar `"u"`.
    last_bar: HashMap<String, i64>,
    rebuilding: bool,
}

struct Inner {
    cfg: SimConfig,
    archive: Archive,
    day: NaiveDate,
    symbols: Vec<String>,
    fixture: bool,
    clock: watch::Receiver<ClockState>,
    tx: broadcast::Sender<Arc<Vec<StreamMsg>>>,
    live: Mutex<Live>,
}

/// The simulator: shared state plus its router. Feed it every replay item with [`Sim::ingest`].
#[derive(Clone)]
pub struct Sim(Arc<Inner>);

impl Sim {
    pub fn new(
        cfg: SimConfig,
        archive: Archive,
        day: NaiveDate,
        symbols: Vec<String>,
        fixture: bool,
        clock: watch::Receiver<ClockState>,
    ) -> Self {
        Sim(Arc::new(Inner {
            cfg,
            archive,
            day,
            symbols,
            fixture,
            clock,
            tx: broadcast::channel(4096).0,
            live: Mutex::new(Live {
                book: BarBook::default(),
                last_bar: HashMap::new(),
                rebuilding: false,
            }),
        }))
    }

    pub fn config(&self) -> &SimConfig {
        &self.0.cfg
    }

    pub(crate) fn now(&self) -> market_events::UnixNanos {
        self.0.clock.borrow().now
    }

    /// Applies one replay item: updates the minute-bar book and broadcasts the Alpaca-shaped
    /// stream messages it produces. Cheap; called from the server's single ingest task.
    pub fn ingest(&self, item: &FeedItem) {
        let mut out: Vec<StreamMsg> = Vec::new();
        {
            let mut live = self.0.live.lock().expect("sim state poisoned");
            match item {
                FeedItem::Reset(_) => {
                    *live = Live {
                        book: BarBook::default(),
                        last_bar: HashMap::new(),
                        rebuilding: true,
                    };
                }
                FeedItem::Clock(_) => live.rebuilding = false,
                FeedItem::Error(_) => {}
                FeedItem::Advance(t) => {
                    let ups = live.book.advance(*t);
                    bars_from(&mut live, &ups, &mut out);
                    live.book.prune(t.millis() - 15 * 60_000);
                }
                FeedItem::Event(e) => {
                    if let Some(m) = event_msg(e) {
                        out.push(m);
                    }
                    let ups = live.book.apply(e);
                    bars_from(&mut live, &ups, &mut out);
                }
            }
            if live.rebuilding {
                out.clear();
            }
        }
        if !out.is_empty() {
            let _ = self.0.tx.send(Arc::new(out));
        }
    }

    /// The simulator's router (REST + WebSocket). Serve it on its own listener.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/v2/stocks/snapshots", get(rest::snapshots))
            .route("/v2/stocks/{symbol}/trades", get(rest::trades))
            .route("/v2/stocks/{symbol}/bars", get(rest::bars))
            .route("/v2/assets", get(rest::assets))
            .route("/stable/shares-float", get(fmp::shares_float))
            .route("/sim/universe", get(rest::universe))
            .route("/v2/sip", get(stream::sip))
            .route("/v1beta1/boats", get(stream::boats))
            .route("/v1beta1/overnight", get(stream::overnight))
            .route("/v1beta1/news", get(stream::news))
            .fallback(rest::not_found)
            .with_state(self.clone())
    }
}

fn overnight(ts: market_events::UnixNanos) -> bool {
    classify(ts).is_some_and(|s| s.kind == SessionKind::Overnight)
}

fn msg(symbol: &str, channel: Channel, ts: market_events::UnixNanos, v: Value) -> StreamMsg {
    StreamMsg {
        symbol: symbol.to_string(),
        channel,
        overnight: overnight(ts),
        json: v.to_string(),
    }
}

fn event_msg(e: &MarketEvent) -> Option<StreamMsg> {
    Some(match e {
        MarketEvent::Trade(t) => msg(
            &t.symbol,
            Channel::Trades,
            t.event_ts,
            alpaca::stream_trade(t),
        ),
        MarketEvent::Quote(q) => msg(
            &q.symbol,
            Channel::Quotes,
            q.event_ts,
            alpaca::stream_quote(q),
        ),
        MarketEvent::Correction(c) => msg(
            &c.symbol,
            Channel::Corrections,
            c.event_ts,
            alpaca::stream_correction(c),
        ),
        MarketEvent::Cancel(c) => msg(
            &c.symbol,
            Channel::CancelErrors,
            c.event_ts,
            alpaca::stream_cancel(c),
        ),
        MarketEvent::Status(s) => msg(
            s.symbol.as_deref()?,
            Channel::Statuses,
            s.event_ts,
            alpaca::stream_status(s)?,
        ),
    })
}

fn bars_from(live: &mut Live, ups: &[ChartUpdate], out: &mut Vec<StreamMsg>) {
    for u in ups {
        let Some(c) = u.as_candle() else { continue };
        if c.timeframe != chart_engine::Timeframe::M1 || !c.candle.is_final {
            continue;
        }
        let last = live.last_bar.get(&c.symbol).copied();
        let (kind, channel) = if last.is_none_or(|l| c.candle.t > l) {
            live.last_bar.insert(c.symbol.clone(), c.candle.t);
            ("b", Channel::Bars)
        } else {
            ("u", Channel::UpdatedBars)
        };
        let vw = live.book.vw(&c.symbol, &c.candle);
        out.push(msg(
            &c.symbol,
            channel,
            market_events::UnixNanos::from_millis(c.candle.t),
            alpaca::stream_bar(kind, &c.symbol, &c.candle, vw),
        ));
    }
}
