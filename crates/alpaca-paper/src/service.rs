//! The Alpaca paper route behind the chart trading messages: `order.place` / `order.modify` /
//! `order.cancel` in, `paper.state` / `paper.fill` out (through the `publish` callback the server
//! gives it, which puts the frames on the live market's broadcast).
//!
//! Every order attempt and every broker event is logged (target `alpaca_paper`, so it lands in
//! the server's durable JSONL log) with symbol, side, type, qty, ids and outcome, never a key.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use paper_broker::wire::{ModifyCommand, PlaceCommand};
use paper_broker::{NewOrder, check_modify, check_new_order};
use serde::Serialize;
use tokio::sync::{Notify, watch};

use crate::alpaca::{
    AlpacaAccount, AlpacaClock, AlpacaOrder, AlpacaPosition, FillActivity, order_body,
    replace_body, side_str, type_str, valid_client_order_id, valid_order_id,
};
use crate::book::{Book, LinkOut};
use crate::creds::{CredSource, Credentials};
use crate::http::{Api, ApiError, Http, Method, ReqwestHttp};
use crate::session::{ClockReading, Session, route, session_by_wall_clock};
use crate::stream::{StreamSink, TradeUpdate};

/// Receives pre-serialized frames, in order (state first, then fills).
pub type Publish = Arc<dyn Fn(Vec<String>) + Send + Sync>;

/// The route's health for the settings status (`broker.status`). No secrets, no account data.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LinkStatus {
    /// True once Alpaca paper accepted the server's keys (and false before the first answer).
    pub configured: bool,
    /// `connecting`, `live`, `down`, `unauthorized` or `not_configured`.
    pub state: &'static str,
    pub message: String,
    /// Which environment names the keys came from, e.g. "ALPACA_PAPER_KEY / ALPACA_PAPER_SECRET".
    pub keys: Option<&'static str>,
}

/// A modify or cancel that was refused; the state did not change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerError {
    pub code: &'static str,
    pub message: String,
}

pub const NOT_CONFIGURED: &str = "Alpaca paper is not configured on the server";

#[derive(Clone)]
pub struct Service(Arc<Inner>);

struct Inner {
    api: Option<Api>,
    keys: Option<&'static str>,
    book: Mutex<Book>,
    publish: Publish,
    clock: Mutex<Option<ClockReading>>,
    status: watch::Sender<LinkStatus>,
    refresh: Notify,
    local_ids: AtomicU64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Service {
    fn build(
        api: Option<Api>,
        keys: Option<&'static str>,
        status: LinkStatus,
        publish: Publish,
    ) -> Self {
        let link = LinkOut {
            state: status.state,
            message: status.message.clone(),
        };
        Service(Arc::new(Inner {
            api,
            keys,
            book: Mutex::new(Book::new(link)),
            publish,
            clock: Mutex::new(None),
            status: watch::Sender::new(status),
            refresh: Notify::new(),
            local_ids: AtomicU64::new(1),
        }))
    }

    /// No usable keys: every order is rejected with [`NOT_CONFIGURED`] and `reason` (which names
    /// the environment variables to set, never values).
    pub fn unconfigured(reason: String, publish: Publish) -> Self {
        Self::build(
            None,
            None,
            LinkStatus {
                configured: false,
                state: "not_configured",
                message: reason,
                keys: None,
            },
            publish,
        )
    }

    /// Over any [`Http`], with no background tasks (tests, and the base of [`Service::start`]).
    pub fn with_http(http: Arc<dyn Http>, keys: Option<&'static str>, publish: Publish) -> Self {
        Self::build(
            Some(Api::new(http)),
            keys,
            LinkStatus {
                configured: false,
                state: "connecting",
                message: "connecting to Alpaca paper".into(),
                keys,
            },
            publish,
        )
    }

    /// The real route: REST over reqwest, the trade stream and the account refresher as tasks on
    /// the current Tokio runtime.
    pub fn start(creds: Credentials, publish: Publish) -> Self {
        let keys = Some(creds.source.env_names());
        let http = match ReqwestHttp::new(creds.clone()) {
            Ok(h) => h,
            Err(e) => {
                return Self::unconfigured(
                    format!("{NOT_CONFIGURED}: TLS setup failed ({e})"),
                    publish,
                );
            }
        };
        let svc = Self::with_http(Arc::new(http), keys, publish);
        if creds.source == CredSource::Shared {
            tracing::info!(target: "alpaca_paper", "using the shared ALPACA_API_KEY for paper trading (no ALPACA_PAPER_KEY set)");
        }
        // A first snapshot right away, so the account shows even while the stream is still
        // connecting (the stream resyncs again once it listens).
        let s = svc.clone();
        tokio::spawn(async move {
            let _ = s.resync().await;
        });
        let s = svc.clone();
        tokio::spawn(async move { crate::stream::run(creds, s).await });
        let s = svc.clone();
        tokio::spawn(async move { s.refresher().await });
        svc
    }

    pub fn status(&self) -> watch::Receiver<LinkStatus> {
        self.0.status.subscribe()
    }

    pub fn configured(&self) -> bool {
        self.0.status.borrow().configured
    }

    /// The current `paper.state` (a connection's first frame, or a mode switch).
    pub fn state_frame(&self) -> String {
        self.0.book.lock().expect("book lock").render()
    }

    /// Broadcasts the state (new `seq`), then `fills`. Under the book lock, so frames go out in
    /// order whichever task changed the book.
    fn publish(&self, book: &mut Book, fills: &[crate::book::FillOut]) {
        let mut frames = vec![book.state_frame()];
        frames.extend(fills.iter().map(Book::fill_frame));
        (self.0.publish)(frames);
    }

    fn set_status(&self, configured: Option<bool>, state: &'static str, message: String) {
        self.0.status.send_if_modified(|s| {
            let next = LinkStatus {
                configured: configured.unwrap_or(s.configured),
                state,
                message: message.clone(),
                keys: self.0.keys,
            };
            let changed = *s != next;
            *s = next;
            changed
        });
        let mut book = self.0.book.lock().expect("book lock");
        if book.set_link(LinkOut { state, message }) {
            self.publish(&mut book, &[]);
        }
    }

    fn client_order_id(&self, given: Option<String>) -> String {
        match given {
            Some(c) if valid_client_order_id(&c) => c,
            _ => format!(
                "sw-{}-{}",
                now_ms(),
                self.0.local_ids.fetch_add(1, Ordering::Relaxed)
            ),
        }
    }

    fn reject(&self, order: &NewOrder, coid: &str, code: &str, message: &str) {
        tracing::info!(target: "alpaca_paper", event = "order.place", outcome = "rejected",
            symbol = %order.symbol, side = side_str(order.side), order_type = type_str(order.order_type),
            qty = order.qty, client_order_id = coid, code, reason = message);
        let mut book = self.0.book.lock().expect("book lock");
        book.reject_local(order, Some(coid.to_owned()), code, message, now_ms());
        self.publish(&mut book, &[]);
    }

    /// Regular or extended hours now: Alpaca's clock while its reading holds, else a fresh
    /// reading, else the wall-clock rule.
    async fn session(&self, api: &Api) -> Session {
        let now = now_ms();
        if let Some(s) = self
            .0
            .clock
            .lock()
            .expect("clock lock")
            .and_then(|c| c.at(now))
        {
            return s;
        }
        match get_json::<AlpacaClock>(api, "/v2/clock").await {
            Ok(c) => {
                let r = ClockReading::from_alpaca(&c);
                *self.0.clock.lock().expect("clock lock") = Some(r);
                r.at(now).unwrap_or(r.session)
            }
            Err(_) => session_by_wall_clock(now),
        }
    }

    /// `order.place`. Always changes the state (an accepted order, or a rejection), unless the
    /// outcome is unknown (network error on the POST), in which case a notice says so and the
    /// account resyncs.
    pub async fn place(&self, cmd: PlaceCommand) {
        let order = cmd.into_new_order();
        let coid = self.client_order_id(order.client_order_id.clone());
        let Some(api) = self.0.api.clone() else {
            let reason = self.0.status.borrow().message.clone();
            return self.reject(
                &order,
                &coid,
                "not_configured",
                &not_configured_text(&reason),
            );
        };
        if matches!(
            self.0.status.borrow().state,
            "unauthorized" | "not_configured"
        ) {
            let reason = self.0.status.borrow().message.clone();
            return self.reject(
                &order,
                &coid,
                "not_configured",
                &not_configured_text(&reason),
            );
        }
        if let Err(r) = check_new_order(&order) {
            let m = paper_broker::CommandError::Invalid(r).message();
            return self.reject(&order, &coid, r.code(), m);
        }
        let extended = match route(
            self.session(&api).await,
            order.order_type,
            order.bracket.is_some(),
        ) {
            Ok(x) => x,
            Err(r) => return self.reject(&order, &coid, r.code(), r.message()),
        };
        let body = order_body(&order, &coid, extended).to_string();
        match api.call(Method::Post, "/v2/orders", Some(body)).await {
            Ok(r) => match serde_json::from_str::<AlpacaOrder>(&r.body) {
                Ok(o) => {
                    tracing::info!(target: "alpaca_paper", event = "order.place", outcome = "accepted",
                        symbol = %order.symbol, side = side_str(order.side), order_type = type_str(order.order_type),
                        qty = order.qty, client_order_id = %coid, order_id = %o.id, extended_hours = extended,
                        status = %o.status);
                    let mut book = self.0.book.lock().expect("book lock");
                    book.upsert(&o);
                    self.publish(&mut book, &[]);
                }
                Err(e) => {
                    tracing::warn!(target: "alpaca_paper", event = "order.place", outcome = "unreadable_answer",
                        client_order_id = %coid, error = %e);
                    self.0.refresh.notify_one();
                }
            },
            Err(e @ ApiError::Network(_)) => {
                tracing::warn!(target: "alpaca_paper", event = "order.place", outcome = "unknown",
                    symbol = %order.symbol, client_order_id = %coid, error = %e);
                let mut book = self.0.book.lock().expect("book lock");
                book.set_notice(
                    now_ms(),
                    "Alpaca paper did not answer; the order may or may not have been placed. Resyncing the account.".into(),
                    None,
                );
                self.publish(&mut book, &[]);
                drop(book);
                let s = self.clone();
                tokio::spawn(async move {
                    let _ = s.resync().await;
                });
            }
            Err(e) => {
                let code = if e.status() == Some(429) {
                    "rate_limited"
                } else {
                    "broker_rejected"
                };
                self.reject(&order, &coid, code, &format!("Alpaca: {}", e.message()));
            }
        }
    }

    /// The order `id` if it is working here and addressable at Alpaca.
    fn working_id(&self, id: &str) -> Result<(), BrokerError> {
        let not_open = || BrokerError {
            code: "not_open",
            message: "order is not open".into(),
        };
        if !valid_order_id(id) {
            return Err(not_open());
        }
        let book = self.0.book.lock().expect("book lock");
        book.working(id).map(|_| ()).ok_or_else(not_open)
    }

    fn refused(&self, event: &'static str, id: &str, e: &ApiError) -> BrokerError {
        let cancel = event == "order.cancel";
        tracing::info!(target: "alpaca_paper", event, outcome = "rejected", order_id = id, error = %e);
        let message = format!("Alpaca: {}", e.message());
        let mut book = self.0.book.lock().expect("book lock");
        book.set_notice(now_ms(), message.clone(), Some(id.to_owned()));
        self.publish(&mut book, &[]);
        BrokerError {
            code: match e.status() {
                Some(404) => "not_open",
                // Cancel: "order is not cancelable" (already filled or cancelled).
                Some(422) if cancel => "not_open",
                Some(429) => "rate_limited",
                _ => "broker_rejected",
            },
            message,
        }
    }

    /// `order.modify`, as an Alpaca replace. The answer is the replacement order (a new id),
    /// which takes the old one's place in the state.
    pub async fn modify(&self, cmd: ModifyCommand) -> Result<(), BrokerError> {
        let (id, m) = cmd.into_modify();
        let Some(api) = self.0.api.clone() else {
            return Err(BrokerError {
                code: "not_configured",
                message: NOT_CONFIGURED.into(),
            });
        };
        self.working_id(&id)?;
        if m.qty.is_none() && m.limit_price.is_none() && m.stop_price.is_none() {
            return Err(BrokerError {
                code: "bad_order",
                message: "nothing to change".into(),
            });
        }
        if let Err(r) = check_modify(&m) {
            return Err(BrokerError {
                code: r.code(),
                message: paper_broker::CommandError::Invalid(r).message().into(),
            });
        }
        let body = replace_body(&m).to_string();
        match api
            .call(Method::Patch, &format!("/v2/orders/{id}"), Some(body))
            .await
        {
            Ok(r) => {
                if let Ok(mut o) = serde_json::from_str::<AlpacaOrder>(&r.body) {
                    tracing::info!(target: "alpaca_paper", event = "order.modify", outcome = "accepted",
                        order_id = %id, new_order_id = %o.id);
                    if o.replaces.is_none() {
                        o.replaces = Some(id.clone());
                    }
                    let mut book = self.0.book.lock().expect("book lock");
                    book.upsert(&o);
                    self.publish(&mut book, &[]);
                }
                Ok(())
            }
            Err(e) => Err(self.refused("order.modify", &id, &e)),
        }
    }

    /// `order.cancel`. On success the order is read back (the stream also reports it).
    pub async fn cancel(&self, id: &str) -> Result<(), BrokerError> {
        let Some(api) = self.0.api.clone() else {
            return Err(BrokerError {
                code: "not_configured",
                message: NOT_CONFIGURED.into(),
            });
        };
        self.working_id(id)?;
        match api
            .call(Method::Delete, &format!("/v2/orders/{id}"), None)
            .await
        {
            Ok(_) => {
                tracing::info!(target: "alpaca_paper", event = "order.cancel", outcome = "accepted", order_id = id);
                if let Ok(o) =
                    get_json::<AlpacaOrder>(&api, &format!("/v2/orders/{id}?nested=true")).await
                {
                    let mut book = self.0.book.lock().expect("book lock");
                    book.upsert(&o);
                    self.publish(&mut book, &[]);
                }
                Ok(())
            }
            Err(e) => Err(self.refused("order.cancel", id, &e)),
        }
    }

    /// A trade update from the stream: the order changes, a fill is recorded, and the account
    /// and positions are refreshed (debounced) since buying power moved.
    pub fn apply_update(&self, u: TradeUpdate) {
        tracing::info!(target: "alpaca_paper", event = "trade_update", kind = %u.event,
            order_id = %u.order.id, symbol = %u.order.symbol, status = %u.order.status,
            price = u.price.as_deref().unwrap_or(""), qty = u.qty.as_deref().unwrap_or(""));
        let mut book = self.0.book.lock().expect("book lock");
        book.upsert(&u.order);
        let mut fills = Vec::new();
        if u.is_fill()
            && let Some(f) = book.fill_from_update(
                &u.order,
                u.price.as_deref(),
                u.qty.as_deref(),
                u.timestamp.as_deref(),
            )
            && book.add_fill(f.clone())
        {
            fills.push(f);
        }
        self.publish(&mut book, &fills);
        drop(book);
        self.0.refresh.notify_one();
    }

    /// The REST snapshot: account, positions, open and recent orders (legs nested), fills.
    pub async fn resync(&self) -> Result<(), ApiError> {
        let Some(api) = self.0.api.clone() else {
            return Ok(());
        };
        let account = match get_json::<AlpacaAccount>(&api, "/v2/account").await {
            Ok(a) => a,
            Err(e) => {
                if matches!(e.status(), Some(401) | Some(403)) {
                    let keys = self
                        .0
                        .keys
                        .unwrap_or("ALPACA_PAPER_KEY / ALPACA_PAPER_SECRET");
                    self.set_status(
                        Some(false),
                        "unauthorized",
                        format!("Alpaca paper refused the server's keys (HTTP {}); {keys} must be paper-account keys", e.status().unwrap_or(0)),
                    );
                }
                return Err(e);
            }
        };
        if account.status != "ACTIVE" {
            tracing::warn!(target: "alpaca_paper", account_status = %account.status, "paper account is not ACTIVE");
        }
        let positions = get_json::<Vec<AlpacaPosition>>(&api, "/v2/positions").await?;
        let mut orders =
            get_json::<Vec<AlpacaOrder>>(&api, "/v2/orders?status=open&nested=true&limit=500")
                .await?;
        orders.extend(
            get_json::<Vec<AlpacaOrder>>(
                &api,
                "/v2/orders?status=closed&nested=true&limit=50&direction=desc",
            )
            .await?,
        );
        let fills = get_json::<Vec<FillActivity>>(
            &api,
            "/v2/account/activities/FILL?direction=desc&page_size=100",
        )
        .await
        .unwrap_or_default();
        {
            let mut book = self.0.book.lock().expect("book lock");
            book.replace_all(&account, &positions, &orders, &fills);
            self.publish(&mut book, &[]);
        }
        let st = self.0.status.borrow().state;
        let msg = if account.status == "ACTIVE" {
            String::new()
        } else {
            format!("the Alpaca paper account is {}", account.status)
        };
        self.set_status(Some(true), if st == "live" { "live" } else { st }, msg);
        tracing::info!(target: "alpaca_paper", event = "resync", orders = orders.len(), positions = positions.len(), fills = fills.len());
        Ok(())
    }

    /// Debounced account + positions refresh after trade updates (event-driven, not a poll).
    async fn refresher(&self) {
        let Some(api) = self.0.api.clone() else {
            return;
        };
        loop {
            self.0.refresh.notified().await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let (Ok(a), Ok(p)) = (
                get_json::<AlpacaAccount>(&api, "/v2/account").await,
                get_json::<Vec<AlpacaPosition>>(&api, "/v2/positions").await,
            ) else {
                continue;
            };
            let mut book = self.0.book.lock().expect("book lock");
            book.set_account(&a);
            book.set_positions(&p);
            self.publish(&mut book, &[]);
        }
    }
}

fn not_configured_text(reason: &str) -> String {
    if reason.is_empty() || reason.starts_with(NOT_CONFIGURED) {
        if reason.is_empty() {
            NOT_CONFIGURED.to_owned()
        } else {
            reason.to_owned()
        }
    } else {
        format!("{NOT_CONFIGURED}: {reason}")
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(api: &Api, path: &str) -> Result<T, ApiError> {
    let r = api.call(Method::Get, path, None).await?;
    serde_json::from_str(&r.body).map_err(|e| ApiError::Network(format!("unreadable answer: {e}")))
}

impl StreamSink for Service {
    async fn listening(&self) {
        self.set_status(None, "live", String::new());
        if let Err(e) = self.resync().await {
            tracing::warn!(target: "alpaca_paper", error = %e, "resync after connect failed");
        }
    }

    fn update(&self, u: TradeUpdate) {
        self.apply_update(u);
    }

    fn down(&self, reason: &str, unauthorized: bool) {
        if unauthorized {
            let keys = self
                .0
                .keys
                .unwrap_or("ALPACA_PAPER_KEY / ALPACA_PAPER_SECRET");
            self.set_status(
                Some(false),
                "unauthorized",
                format!("{reason}; {keys} must be paper-account keys"),
            );
        } else {
            self.set_status(
                None,
                "down",
                format!("trade stream down ({reason}); reconnecting"),
            );
        }
    }

    fn connecting(&self) {
        let st = self.0.status.borrow().state;
        if st != "unauthorized" {
            self.set_status(None, "connecting", "connecting to Alpaca paper".into());
        }
    }
}
