//! The bid-ask spread on every Runners-pipeline alert (owner, 2026-11-19): what it costs to trade
//! the alert, so a costly one can be skipped. Display only: nothing here feeds a detector, a
//! limit or a decision.
//!
//! # Why
//!
//! The 2026-11-19 diagnostics (`research/diagnostics`, Check 1) measured the alerts' edge at
//! about +0.2 % at the mid, against a round trip of about 2.85 pp when buying the ask and selling
//! the bid on these small caps (median entry spread 157 bp). Alerts with a spread of 100 bp or
//! less cost about 1 pp. The spread, not the price path, is what decides whether an alert is
//! tradable, so every surface shows it beside the alert (packages/shared `spread.ts` for the cost
//! bands, one module every client uses).
//!
//! # The number
//!
//! `spread_bp = round((ask − bid) / mid × 10 000)`, `mid = (ask + bid) / 2`, from the newest
//! quote **at or before** the notice's fire time (`Notice::at_ns`) that is
//!
//! - no older than [`MAX_AGE_NS`] (60 s): an older quote says what the market was, not what it is
//!   when the alert fires;
//! - a market quote: `bid > 0`, `ask > bid` (a locked or crossed book is not a price anyone can
//!   trade at both sides of), and a spread no wider than [`MAX_SPREAD_OF_MID`] (50 %) of the mid
//!   (the diagnostics' Amendment 1: a stub quote on an empty book, not a market).
//!
//! When none qualifies the notice carries `null`, never an estimate: a wrong low number would be
//! worse than none, because it is exactly the number used to decide to trade.
//!
//! # Where the quote comes from
//!
//! The live stream subscribes to **trades and statuses only** (`market_providers::alpaca_stream`,
//! `subscribe`); quotes for the whole market (`*`) would multiply the stream's volume many times
//! over for a number needed a few hundred times a day. So the quote is fetched when the notice
//! fires, from Alpaca's historical quotes endpoint (`GET /v2/stocks/{symbol}/quotes`), the same
//! vendor and keys the live stream already uses (no new dependency), with `end` = the fire time
//! and `start` = 60 s before it, newest first. That window is the rule above exactly: "at or
//! before fire time, no older than 60 s". The feed is the live stream's day feed (`sip` or `iex`);
//! any other (`test`) has no quote source, and the overnight session has no SIP quotes in the
//! window, so a night notice is honestly `null`.
//!
//! # The stage
//!
//! [`spawn_stage`] sits between the market's `offer_runner` and the alerts task: it takes every
//! notice the market offered, fetches the spreads of a batch concurrently (each bounded by
//! [`FETCH_TIMEOUT`], so a slow vendor costs a push at most that much, then `null`), records a
//! row's fire-time spread on the Runners board (`Market::set_runner_spread`, so the row shows it)
//! and hands the notices on in their original order. The market itself never waits: it still
//! offers with `try_send`. Without a quote source (no keys, `ALERT_SPREAD=0`, tests) there is no
//! stage at all and the notices go straight to the alerts task as before.
//!
//! Board rows show the **fire-time** spread only, not a live one: keeping a live spread for
//! every row would need quotes streamed (or polled) for those symbols all day. A row restored
//! from the forward log after a restart has none (the ledger keeps the notice's, `spread_bp`).

use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::market::Market;
use crate::rocket::{Notice, NoticeKind};

/// The oldest quote that may price a notice: 60 s before its fire time.
pub const MAX_AGE_NS: i64 = 60_000_000_000;
/// A spread wider than this fraction of the mid is a stub quote, not a market.
pub const MAX_SPREAD_OF_MID: f64 = 0.5;
/// How long one notice's quote fetch may take before it goes out with `null`: Runners pushes aim
/// at under 5 s from the print (`crate::alerts::runners`, "Latency"); a quote request normally
/// answers in well under half a second.
pub const FETCH_TIMEOUT: Duration = Duration::from_millis(1500);
/// Quotes asked for per notice, newest first. A burst of one-sided or crossed quotes longer than
/// this right before the alert leaves it `null`, which is the honest answer for such a book.
pub const QUOTE_LIMIT: u32 = 50;
/// Notices taken in one batch (fetched concurrently).
const BATCH: usize = 32;

/// One NBBO update: exchange time (epoch ns) and the two sides in dollars.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Nbbo {
    pub at_ns: i64,
    pub bid: f64,
    pub ask: f64,
}

/// The spread of one quote in basis points of its mid, rounded, or `None` for a non-market
/// quote (module docs, "The number").
pub fn spread_bp(bid: f64, ask: f64) -> Option<i32> {
    if !(bid.is_finite() && ask.is_finite()) || bid <= 0.0 || ask <= bid {
        return None;
    }
    let mid = (ask + bid) / 2.0;
    let frac = (ask - bid) / mid;
    if frac > MAX_SPREAD_OF_MID {
        return None;
    }
    Some((frac * 10_000.0).round() as i32)
}

/// The fire-time spread: the newest market quote at or before `fire_ns` and no older than
/// [`MAX_AGE_NS`], in any input order. `None` when none qualifies.
pub fn at_fire(quotes: impl IntoIterator<Item = Nbbo>, fire_ns: i64) -> Option<i32> {
    quotes
        .into_iter()
        .filter(|q| q.at_ns <= fire_ns && fire_ns - q.at_ns <= MAX_AGE_NS)
        .filter_map(|q| spread_bp(q.bid, q.ask).map(|bp| (q.at_ns, bp)))
        .max_by_key(|&(at, _)| at)
        .map(|(_, bp)| bp)
}

#[derive(Deserialize)]
struct QuotesPage {
    #[serde(default)]
    quotes: Option<Vec<RawQuote>>,
}

/// One element of the response's `quotes` (Alpaca: `t` RFC 3339 with nanoseconds, `bp`/`ap`
/// bid and ask price). Sizes, venues and conditions are not needed for the spread.
#[derive(Deserialize)]
struct RawQuote {
    t: String,
    #[serde(default)]
    bp: f64,
    #[serde(default)]
    ap: f64,
}

/// The quotes of one `GET /v2/stocks/{symbol}/quotes` page. A quote whose time does not parse is
/// skipped (it cannot be placed against the fire time); `quotes: null` is an empty window.
pub fn parse_quotes(body: &[u8]) -> Result<Vec<Nbbo>, String> {
    let page: QuotesPage = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    Ok(page
        .quotes
        .unwrap_or_default()
        .into_iter()
        .filter_map(|q| {
            let at_ns = DateTime::parse_from_rfc3339(&q.t)
                .ok()?
                .with_timezone(&Utc)
                .timestamp_nanos_opt()?;
            Some(Nbbo {
                at_ns,
                bid: q.bp,
                ask: q.ap,
            })
        })
        .collect())
}

/// Whether a notice is priced at all. Exchange halts and resumptions (`crate::alerts::halts`)
/// never reach the stage (the alerts task makes them itself) and have no book to price anyway.
pub fn wants_spread(n: &Notice) -> bool {
    !matches!(n.kind, NoticeKind::Halted | NoticeKind::Resumed)
        && !n.symbol.is_empty()
        && n.at_ns > 0
}

/// Whether a notice's `alert_id` is a Runners row's own id, so its spread is the row's
/// fire-time spread: a row's first entry. Follow-ups, shelves, cues and the shadow detectors
/// carry other ids (or a row's id with a suffix) and price only their own notice.
fn is_row_entry(kind: NoticeKind) -> bool {
    matches!(
        kind,
        NoticeKind::New
            | NoticeKind::Burst
            | NoticeKind::Running
            | NoticeKind::Run
            | NoticeKind::Fast
    )
}

/// Alpaca's quotes, for the live market (module docs, "Where the quote comes from").
#[derive(Clone)]
pub struct QuoteSource {
    http: reqwest::Client,
    base: String,
    key_id: String,
    secret: String,
    feed: &'static str,
}

impl std::fmt::Debug for QuoteSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the keys.
        f.debug_struct("QuoteSource")
            .field("base", &self.base)
            .field("feed", &self.feed)
            .finish_non_exhaustive()
    }
}

impl QuoteSource {
    /// The source for the live stream's configuration, or `None` (logged) when there is none:
    /// `ALERT_SPREAD=0`, no Alpaca keys, or a feed with no historical quotes (`boats`,
    /// `overnight` as the day feed, `test`). `ALPACA_DATA_BASE` overrides the REST base, as for
    /// the importer.
    pub fn from_config(
        live: &market_providers::LiveConfig,
        get: impl Fn(&str) -> Option<String>,
    ) -> Option<QuoteSource> {
        use market_providers::alpaca_stream::StreamFeed;
        if get("ALERT_SPREAD").is_some_and(|v| v.trim() == "0") {
            tracing::info!("alert spread: off (ALERT_SPREAD=0); notices carry no spread");
            return None;
        }
        let creds = live.credentials.as_ref()?;
        let feed = match live.feed {
            StreamFeed::Sip => "sip",
            StreamFeed::Iex => "iex",
            other => {
                tracing::info!(feed = ?other, "alert spread: no quote source for this feed; notices carry no spread");
                return None;
            }
        };
        let http = match crate::alerts::push::https_client(FETCH_TIMEOUT) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(error = %e, "alert spread: no HTTP client; notices carry no spread");
                return None;
            }
        };
        let base = get("ALPACA_DATA_BASE")
            .map(|b| b.trim().to_owned())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "https://data.alpaca.markets".to_owned());
        Some(QuoteSource {
            http,
            base: base.trim_end_matches('/').to_owned(),
            key_id: creds.key_id.clone(),
            secret: creds.secret.clone(),
            feed,
        })
    }

    /// The request for `symbol`'s quotes in the 60 s up to `fire_ns`, newest first.
    fn url(&self, symbol: &str, fire_ns: i64) -> String {
        let ts = |ns: i64| {
            DateTime::<Utc>::from_timestamp_nanos(ns).to_rfc3339_opts(SecondsFormat::Nanos, true)
        };
        format!(
            "{}/v2/stocks/{}/quotes?start={}&end={}&limit={QUOTE_LIMIT}&sort=desc&feed={}",
            self.base,
            path_segment(symbol),
            ts(fire_ns - MAX_AGE_NS),
            ts(fire_ns),
            self.feed
        )
    }

    /// `symbol`'s fire-time spread, or `None` on any failure (logged at debug: a missing spread
    /// is shown as missing, it is not an incident).
    pub async fn spread_at(&self, symbol: &str, fire_ns: i64) -> Option<i32> {
        let req = self
            .http
            .get(self.url(symbol, fire_ns))
            .header("APCA-API-KEY-ID", &self.key_id)
            .header("APCA-API-SECRET-KEY", &self.secret);
        let resp = match tokio::time::timeout(FETCH_TIMEOUT, req.send()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                // `without_url`: nothing secret is in this URL, but keep errors short.
                tracing::debug!(symbol, error = %e.without_url(), "alert spread: quote request failed");
                return None;
            }
            Err(_) => {
                tracing::debug!(symbol, "alert spread: quote request timed out");
                return None;
            }
        };
        let status = resp.status().as_u16();
        let body = match tokio::time::timeout(FETCH_TIMEOUT, resp.bytes()).await {
            Ok(Ok(b)) => b,
            _ => return None,
        };
        if status != 200 {
            tracing::warn!(symbol, status, "alert spread: quote request refused");
            return None;
        }
        match parse_quotes(&body) {
            Ok(q) => at_fire(q, fire_ns),
            Err(e) => {
                tracing::warn!(symbol, error = %e, "alert spread: unreadable quote response");
                None
            }
        }
    }
}

/// A symbol as one URL path segment (letters, digits and `.` as they are; anything else
/// percent-encoded, so a class suffix or an odd symbol cannot change the path).
fn path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'.' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Starts the stage (module docs, "The stage"): notices in from the market, out to the alerts
/// task with `spread_bp` filled where a quote qualified, in the order they came. It waits for room
/// on the way out (`send`, not `try_send`): the alerts task drains that channel at once, and the
/// market's own `try_send` into this stage is what keeps ingest from ever waiting.
pub fn spawn_stage(
    source: QuoteSource,
    market: Market,
    mut rx: mpsc::Receiver<Notice>,
    tx: mpsc::Sender<Notice>,
) {
    tokio::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut batch = vec![first];
            while batch.len() < BATCH
                && let Ok(n) = rx.try_recv()
            {
                batch.push(n);
            }
            let mut set = tokio::task::JoinSet::new();
            for (i, n) in batch.iter().enumerate() {
                if wants_spread(n) {
                    let src = source.clone();
                    let (symbol, at) = (n.symbol.clone(), n.at_ns);
                    set.spawn(async move { (i, src.spread_at(&symbol, at).await) });
                }
            }
            while let Some(Ok((i, bp))) = set.join_next().await {
                batch[i].spread_bp = bp;
            }
            for n in batch {
                if let Some(bp) = n.spread_bp
                    && is_row_entry(n.kind)
                {
                    market.set_runner_spread(&n.alert_id, bp);
                }
                if tx.send(n).await.is_err() {
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    fn q(at_s: i64, bid: f64, ask: f64) -> Nbbo {
        Nbbo {
            at_ns: at_s * S,
            bid,
            ask,
        }
    }

    #[test]
    fn the_spread_is_basis_points_of_the_mid_rounded() {
        // 2.00 / 2.02: 0.02 / 2.01 = 99.5 bp → 100.
        assert_eq!(spread_bp(2.00, 2.02), Some(100));
        // 1.00 / 1.01: 0.01 / 1.005 = 99.50 bp → 100 (half away from zero).
        assert_eq!(spread_bp(1.00, 1.01), Some(100));
        // 0.5000 / 0.5021: 0.0021 / 0.50105 = 41.9 bp → 42.
        assert_eq!(spread_bp(0.5, 0.5021), Some(42));
        // A one-tick spread on a $20 stock: 5 bp.
        assert_eq!(spread_bp(20.00, 20.01), Some(5));
    }

    #[test]
    fn non_market_quotes_have_no_spread() {
        assert_eq!(spread_bp(0.0, 1.0), None, "no bid");
        assert_eq!(spread_bp(-1.0, 1.0), None, "negative bid");
        assert_eq!(spread_bp(1.0, 1.0), None, "locked");
        assert_eq!(spread_bp(1.01, 1.0), None, "crossed");
        assert_eq!(spread_bp(1.0, 0.0), None, "no ask");
        assert_eq!(spread_bp(f64::NAN, 1.0), None);
        assert_eq!(spread_bp(1.0, f64::INFINITY), None);
        // 1.00 / 1.70: 0.70 / 1.35 = 51.9 % of the mid: a stub, not a market.
        assert_eq!(spread_bp(1.0, 1.7), None);
        // 1.00 / 1.60: 0.60 / 1.30 = 46.2 % of the mid: very wide, but still a market.
        assert_eq!(spread_bp(1.0, 1.6), Some(4615));
    }

    #[test]
    fn the_newest_market_quote_at_or_before_fire_time_wins() {
        let fire = 1_000 * S;
        let quotes = [
            q(950, 1.00, 1.10),  // older
            q(990, 2.00, 2.02),  // newest qualifying
            q(980, 1.00, 1.01),  // older still
            q(1_001, 3.0, 3.30), // after the fire: never used
        ];
        assert_eq!(at_fire(quotes, fire), Some(100));
        // At exactly the fire time counts.
        assert_eq!(at_fire([q(1_000, 20.0, 20.01)], fire), Some(5));
    }

    #[test]
    fn a_quote_older_than_a_minute_is_stale() {
        let fire = 1_000 * S;
        assert_eq!(
            at_fire([q(940, 2.0, 2.02)], fire),
            Some(100),
            "exactly 60 s"
        );
        assert_eq!(
            at_fire(
                [Nbbo {
                    at_ns: 940 * S - 1,
                    bid: 2.0,
                    ask: 2.02
                }],
                fire
            ),
            None,
            "60 s and 1 ns"
        );
        assert_eq!(at_fire([], fire), None, "no quote at all");
    }

    #[test]
    fn a_non_market_newest_quote_falls_back_to_an_older_market_one() {
        let fire = 1_000 * S;
        // The newest is one-sided; the one before it is a market within the minute.
        let quotes = [q(999, 0.0, 2.05), q(995, 2.0, 2.02)];
        assert_eq!(at_fire(quotes, fire), Some(100));
        // Only non-market quotes: null, never a guess.
        assert_eq!(
            at_fire(
                [q(999, 0.0, 2.05), q(998, 2.1, 2.0), q(997, 1.0, 9.0)],
                fire
            ),
            None
        );
    }

    #[test]
    fn alpaca_quote_pages_parse() {
        let body = br#"{"quotes":[
            {"t":"2026-11-19T14:30:00.123456789Z","ax":"Q","ap":2.02,"as":3,"bx":"K","bp":2.0,"bs":5,"c":["R"],"z":"C"},
            {"t":"not a time","ap":1.0,"bp":0.9},
            {"t":"2026-11-19T14:29:59Z","ap":2.03}
        ],"symbol":"SMPL","next_page_token":null}"#;
        let got = parse_quotes(body).unwrap();
        assert_eq!(got.len(), 2, "the unparseable time is skipped");
        let t0 = DateTime::parse_from_rfc3339("2026-11-19T14:30:00Z")
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap();
        assert_eq!(got[0].at_ns, t0 + 123_456_789);
        assert_eq!((got[0].bid, got[0].ask), (2.0, 2.02));
        // A missing side reads as 0: non-market, so it never prices anything.
        assert_eq!(got[1].bid, 0.0);
        assert_eq!(at_fire(got, t0 + S), Some(100));
        assert!(
            parse_quotes(br#"{"quotes":null,"symbol":"SMPL"}"#)
                .unwrap()
                .is_empty()
        );
        assert!(parse_quotes(b"<html>").is_err());
    }

    #[test]
    fn the_request_asks_for_the_minute_up_to_the_fire_newest_first() {
        let src = QuoteSource {
            http: crate::alerts::push::https_client(FETCH_TIMEOUT).unwrap(),
            base: "https://data.example".into(),
            key_id: "k".into(),
            secret: "s".into(),
            feed: "sip",
        };
        let fire = DateTime::parse_from_rfc3339("2026-11-19T14:30:00.5Z")
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap();
        assert_eq!(
            src.url("BRK.B", fire),
            "https://data.example/v2/stocks/BRK.B/quotes?start=2026-11-19T14:29:00.500000000Z\
             &end=2026-11-19T14:30:00.500000000Z&limit=50&sort=desc&feed=sip"
        );
        assert_eq!(path_segment("A/B C"), "A%2FB%20C");
        assert!(!format!("{src:?}").contains("\"s\""), "no secret in Debug");
    }

    #[test]
    fn halts_are_not_priced_and_only_entries_price_their_row() {
        let mut n = Notice::test(NoticeKind::Run, "SMPL");
        n.at_ns = S;
        assert!(wants_spread(&n));
        for k in [NoticeKind::Halted, NoticeKind::Resumed] {
            let mut h = n.clone();
            h.kind = k;
            assert!(!wants_spread(&h));
        }
        assert!(is_row_entry(NoticeKind::New) && is_row_entry(NoticeKind::Fast));
        assert!(!is_row_entry(NoticeKind::FollowUp) && !is_row_entry(NoticeKind::Shelf));
    }
}

#[cfg(test)]
mod stage_tests {
    use super::*;
    use axum::extract::Path;
    use axum::http::{HeaderMap, StatusCode};

    /// Alpaca's quotes endpoint, enough of it: GOOD has a market quote 1 s before the fire, WIDE
    /// only a stub, anything else answers 500. A request without both key headers is a 401.
    async fn alpaca() -> String {
        let app = axum::Router::new().route(
            "/v2/stocks/{sym}/quotes",
            axum::routing::get(|Path(sym): Path<String>, h: HeaderMap| async move {
                if h.get("APCA-API-KEY-ID").is_none() || h.get("APCA-API-SECRET-KEY").is_none() {
                    return (StatusCode::UNAUTHORIZED, String::new());
                }
                let q = |bp: f64, ap: f64| {
                    format!(
                        r#"{{"quotes":[{{"t":"2026-11-19T14:29:59Z","bp":{bp},"ap":{ap}}}],"symbol":"{sym}"}}"#
                    )
                };
                match sym.as_str() {
                    "GOOD" => (StatusCode::OK, q(2.0, 2.02)),
                    "WIDE" => (StatusCode::OK, q(1.0, 3.0)),
                    _ => (StatusCode::INTERNAL_SERVER_ERROR, String::new()),
                }
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn the_stage_prices_each_notice_in_order_and_never_guesses() {
        let base = alpaca().await;
        let src = QuoteSource {
            http: crate::alerts::push::https_client(FETCH_TIMEOUT).unwrap(),
            base,
            key_id: "k".into(),
            secret: "s".into(),
            feed: "sip",
        };
        let market = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let (in_tx, in_rx) = mpsc::channel(8);
        let (out_tx, mut out_rx) = mpsc::channel(8);
        spawn_stage(src, market, in_rx, out_tx);
        let fire = DateTime::parse_from_rfc3339("2026-11-19T14:30:00Z")
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap();
        for s in ["GOOD", "WIDE", "DOWN", "GOOD"] {
            let mut n = Notice::test(NoticeKind::Run, s);
            n.at_ns = fire;
            in_tx.send(n).await.unwrap();
        }
        let mut got = Vec::new();
        for _ in 0..4 {
            let n = tokio::time::timeout(Duration::from_secs(5), out_rx.recv())
                .await
                .unwrap()
                .unwrap();
            got.push((n.symbol, n.spread_bp));
        }
        assert_eq!(
            got,
            vec![
                ("GOOD".to_string(), Some(100)),
                ("WIDE".to_string(), None),
                ("DOWN".to_string(), None),
                ("GOOD".to_string(), Some(100)),
            ]
        );
        // A fire more than a minute after the only quote: stale, so null.
        let mut late = Notice::test(NoticeKind::Run, "GOOD");
        late.at_ns = fire + MAX_AGE_NS;
        in_tx.send(late).await.unwrap();
        assert_eq!(out_rx.recv().await.unwrap().spread_bp, None);
    }
}
