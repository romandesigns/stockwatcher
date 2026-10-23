//! Price alerts (owner-approved design "StockWatcher alerts", 2026-10-19; the Strategy board is the
//! spec, restated in docs/architecture/chart-packages.md "Alerts").
//!
//! - **Rules** live in the synced settings ([`model`]): every device sees and edits the same
//!   alerts, and the server reads them from the same store.
//! - **Evaluation** ([`eval`]) runs in its own task, beside the LIVE market only (replay is a
//!   practice feature and never alerts). The market hands over a compact [`eval::Input`] per
//!   eligible trade, halt, resume and LULD band change through a bounded channel with
//!   `try_send`: ingest never waits for alerts. A full channel drops the input and counts it
//!   ([`AlertSink::dropped`], logged and reported in `alerts.status`), so a burst at the open
//!   can cost an alert, never a candle.
//! - **Delivery** ([`delivery`]): in-app always (an `alert.fired` frame to every socket, a toast,
//!   an Activity row), the phone through Expo's push service ([`push`]) within the no-spam rules.
//!   Every hold-back is written to Activity ([`history`]) with its reason.
//!
//! Wire (mirrored by packages/shared/src/protocol.ts):
//! - server `alerts.history {events, phoneToday, phoneBudget, devices, dropped}` once after
//!   `settings.snapshot`, and again to a lagged connection (the newest 500 events, oldest first);
//! - server `alert.fired {event, phoneToday}` / `alert.held {event}` to every connection as they
//!   happen; `alert.updated {event}` when a recorded event changes (a push that failed later);
//! - server `alerts.status {phoneToday, phoneBudget, devices, dropped}` when one of those moves;
//! - client `push.register {token, platform?, name?, runnersNew?, runnersBreakout?}` /
//!   `push.unregister {token}`: this phone's Expo push token (per device, never synced to other
//!   clients) and its Runners push opt-ins ([`runners`]; absent = unchanged).
//!
//! The same task also pushes the Runners board's new flags and breakouts ([`runners`]): the live
//! market offers them through [`AlertSink::offer_runner`], as non-blocking as the price inputs.

pub mod delivery;
pub mod eval;
pub mod history;
pub mod model;
pub mod push;
pub mod runners;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Timelike, Utc};
use chrono_tz::America::New_York;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use crate::market::Market;
use crate::settings::{Settings, SettingsOp};
use delivery::{Delivery, Phone};
use eval::{Evaluator, HoldReason, Input, MarketView, Outcome, Seed};
use history::{Event, EventKind, History};
use model::{Fire, PREFS_NS, Prefs, RULES_NS, STATE_NS, Status};
use push::{Devices, Job, PushConfig, PushResult, RunnersOptIn};
use runners::RunnersDelivery;

pub use model::server_owned;

/// How far the evaluator may fall behind the market before inputs are dropped (and counted).
pub const INPUT_CAPACITY: usize = 8192;
/// Runners notices waiting for the task (a handful a day; a full queue drops and counts).
pub const RUNNERS_CAPACITY: usize = 64;
/// Expiry, re-arming by the clock and the dropped-input report.
const TICK: Duration = Duration::from_secs(5);

/// The market's end of the channel: [`Market::apply`] offers each input with `try_send`.
pub struct AlertSink {
    tx: mpsc::Sender<Input>,
    dropped: AtomicU64,
    /// The Runners board's new flags and breakouts ([`runners`]).
    runners: mpsc::Sender<crate::rocket::Notice>,
    runners_dropped: AtomicU64,
    /// The symbols a pressure alert watches: the market scores only these, once a second
    /// (Market::publish_pressure), so alerts do not make the Halt watcher score every symbol.
    pressure_watch: Mutex<Arc<[String]>>,
}

impl AlertSink {
    pub fn pressure_watch(&self) -> Arc<[String]> {
        self.pressure_watch
            .lock()
            .expect("pressure watch lock")
            .clone()
    }

    /// Never blocks: a full or closed channel drops the input and counts it.
    pub fn offer(&self, input: Input) {
        if self.tx.try_send(input).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// A Runners notice for the phone push. Never blocks: a full or closed channel drops it
    /// (counted, logged) and the board carries on.
    pub fn offer_runner(&self, notice: crate::rocket::Notice) {
        if self.runners.try_send(notice).is_err() {
            let n = self.runners_dropped.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(
                dropped = n,
                "runners push: the alerts task is not keeping up; a notice was dropped"
            );
        }
    }

    pub fn runners_dropped(&self) -> u64 {
        self.runners_dropped.load(Ordering::Relaxed)
    }

    /// Tests: a sink whose two ends the test reads itself.
    #[cfg(test)]
    pub(crate) fn for_test() -> (
        Arc<AlertSink>,
        mpsc::Receiver<Input>,
        mpsc::Receiver<crate::rocket::Notice>,
    ) {
        let (tx, rx) = mpsc::channel(8);
        let (rtx, rrx) = mpsc::channel(2);
        let sink = Arc::new(AlertSink {
            tx,
            dropped: AtomicU64::new(0),
            runners: rtx,
            runners_dropped: AtomicU64::new(0),
            pressure_watch: Mutex::new(Arc::from(Vec::new())),
        });
        (sink, rx, rrx)
    }
}

enum Command {
    Register {
        token: String,
        platform: String,
        name: String,
        runners: RunnersOptIn,
    },
    Unregister {
        token: String,
    },
}

struct Shared {
    frames: broadcast::Sender<Arc<str>>,
    /// The current `alerts.history` frame, rebuilt by the task whenever it changes.
    snapshot: Mutex<Arc<str>>,
    cmd: Option<mpsc::Sender<Command>>,
    /// Desktop and web connections that show Runners system notifications, by connection, with
    /// their switches (`runners.notify`; [`DeskGuard`] removes one when its socket ends). Read
    /// by the task as the desk side's "devices" ([`runners`]): a kind no open client wants is
    /// held as `no_device` and costs no stock gap, exactly as a phone's switch does.
    desks: Mutex<HashMap<u64, DeskKinds>>,
    next_desk: AtomicU64,
}

impl Shared {
    fn desks_for(&self, kind: crate::rocket::NoticeKind) -> usize {
        self.desks
            .lock()
            .expect("desks lock")
            .values()
            .filter(|k| k.wants(kind))
            .count()
    }
}

/// One desktop or web client's Runners notification switches (`runners.notify`). Defaults are
/// the phone's ([`push::Devices::register`]): every kind on except breakouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeskKinds {
    pub new: bool,
    pub breakout: bool,
    pub burst: bool,
    pub running: bool,
    pub run: bool,
    pub fast: bool,
}

impl DeskKinds {
    /// An absent switch takes the phone's default.
    pub fn from_opt_in(o: RunnersOptIn) -> Self {
        DeskKinds {
            new: o.new.unwrap_or(true),
            breakout: o.breakout.unwrap_or(false),
            burst: o.burst.unwrap_or(true),
            running: o.running.unwrap_or(true),
            run: o.run.unwrap_or(true),
            fast: o.fast.unwrap_or(true),
        }
    }

    pub fn wants(self, kind: crate::rocket::NoticeKind) -> bool {
        use crate::rocket::NoticeKind as K;
        match kind {
            K::New => self.new,
            K::Breakout => self.breakout,
            K::Burst => self.burst,
            K::Running => self.running,
            K::Run => self.run,
            K::Fast => self.fast,
        }
    }
}

/// A connection's place among the desks; dropping it (the socket ended) takes it out.
pub struct DeskGuard {
    shared: Arc<Shared>,
    id: u64,
}

impl DeskGuard {
    pub fn set(&self, kinds: DeskKinds) {
        self.shared
            .desks
            .lock()
            .expect("desks lock")
            .insert(self.id, kinds);
    }
}

impl Drop for DeskGuard {
    fn drop(&mut self) {
        self.shared
            .desks
            .lock()
            .expect("desks lock")
            .remove(&self.id);
    }
}

/// The alerts service handle (cheap to clone). [`Alerts::idle`] has no task (tests, or a server
/// without a data directory): sockets get an empty history and nothing ever fires.
#[derive(Clone)]
pub struct Alerts(Arc<Shared>);

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn et(ms: i64) -> DateTime<chrono_tz::Tz> {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .with_timezone(&New_York)
}

/// "10:46" ET.
fn hhmm(ms: i64) -> String {
    et(ms).format("%H:%M").to_string()
}

#[derive(Serialize)]
struct HistoryFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    events: Vec<&'a Event>,
    #[serde(flatten)]
    status: StatusBody,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct StatusBody {
    phone_today: u32,
    phone_budget: u32,
    devices: usize,
    dropped: u64,
}

fn event_frame(kind: &'static str, event: &Event, phone_today: Option<u32>) -> String {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct F<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        event: &'a Event,
        #[serde(skip_serializing_if = "Option::is_none")]
        phone_today: Option<u32>,
    }
    serde_json::to_string(&F {
        kind,
        event,
        phone_today,
    })
    .expect("frames serialize")
}

fn status_frame(s: StatusBody) -> String {
    #[derive(Serialize)]
    struct F {
        #[serde(rename = "type")]
        kind: &'static str,
        #[serde(flatten)]
        status: StatusBody,
    }
    serde_json::to_string(&F {
        kind: "alerts.status",
        status: s,
    })
    .expect("frames serialize")
}

impl Alerts {
    pub fn idle() -> Self {
        let empty = serde_json::to_string(&HistoryFrame {
            kind: "alerts.history",
            events: Vec::new(),
            status: StatusBody {
                phone_today: 0,
                phone_budget: model::DEFAULT_PHONE_BUDGET,
                devices: 0,
                dropped: 0,
            },
        })
        .expect("frames serialize");
        Alerts(Arc::new(Shared {
            frames: broadcast::channel(64).0,
            snapshot: Mutex::new(empty.into()),
            cmd: None,
            desks: Mutex::new(HashMap::new()),
            next_desk: AtomicU64::new(0),
        }))
    }

    /// Every frame for every connection, in order. Subscribe BEFORE taking [`Alerts::snapshot_frame`].
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<str>> {
        self.0.frames.subscribe()
    }

    pub fn snapshot_frame(&self) -> Arc<str> {
        self.0
            .snapshot
            .lock()
            .expect("alerts snapshot lock")
            .clone()
    }

    /// `push.register`: false when the token is malformed (nothing is stored).
    pub fn register_device(
        &self,
        token: &str,
        platform: &str,
        name: &str,
        runners: RunnersOptIn,
    ) -> bool {
        if !push::valid_token(token) {
            return false;
        }
        if let Some(c) = &self.0.cmd {
            let _ = c.try_send(Command::Register {
                token: token.to_owned(),
                platform: platform.to_owned(),
                name: name.to_owned(),
                runners,
            });
        }
        true
    }

    /// `runners.notify`: this connection shows Runners system notifications for `kinds`. The
    /// guard keeps it listed until the socket ends.
    pub fn desk(&self, kinds: DeskKinds) -> DeskGuard {
        let id = self.0.next_desk.fetch_add(1, Ordering::Relaxed);
        let g = DeskGuard {
            shared: self.0.clone(),
            id,
        };
        g.set(kinds);
        g
    }

    pub fn unregister_device(&self, token: &str) {
        if let Some(c) = &self.0.cmd {
            let _ = c.try_send(Command::Unregister {
                token: token.to_owned(),
            });
        }
    }

    /// Starts the service on the live market: reads the rules and states from `settings`,
    /// Activity and the phones from `dir` (None = memory only), attaches the market's sink and
    /// spawns the task.
    pub fn start(
        settings: Settings,
        live: Market,
        dir: Option<PathBuf>,
        push_cfg: PushConfig,
    ) -> Alerts {
        let (res_tx, res_rx) = mpsc::channel(64);
        let pusher = push::spawn(push_cfg, res_tx);
        Self::start_with(settings, live, dir, pusher, res_rx)
    }

    /// [`Alerts::start`] with the push sender given: tests hand in a channel they read instead
    /// of Expo (no network), and the results channel they feed.
    fn start_with(
        settings: Settings,
        live: Market,
        dir: Option<PathBuf>,
        pusher: mpsc::Sender<Job>,
        res_rx: mpsc::Receiver<PushResult>,
    ) -> Alerts {
        let now = now_ms();
        let history = match &dir {
            Some(d) => History::open(d, now).unwrap_or_else(|e| {
                tracing::error!(error = %e, dir = %d.display(), "alert history unreadable; Activity starts empty");
                History::in_memory()
            }),
            None => History::in_memory(),
        };
        let devices = dir
            .as_deref()
            .map(Devices::open)
            .unwrap_or_else(Devices::in_memory);
        let runners = dir
            .as_deref()
            .map(RunnersDelivery::open)
            .unwrap_or_else(RunnersDelivery::in_memory);
        let desk_runners = dir
            .as_deref()
            .map(|d| RunnersDelivery::open_file(d, runners::DESK_FILE))
            .unwrap_or_else(RunnersDelivery::in_memory);
        let (in_tx, in_rx) = mpsc::channel(INPUT_CAPACITY);
        let (run_tx, run_rx) = mpsc::channel(RUNNERS_CAPACITY);
        let sink = Arc::new(AlertSink {
            tx: in_tx,
            dropped: AtomicU64::new(0),
            runners: run_tx,
            runners_dropped: AtomicU64::new(0),
            pressure_watch: Mutex::new(Arc::from(Vec::new())),
        });
        live.attach_alerts(sink.clone());
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let alerts = Alerts(Arc::new(Shared {
            frames: broadcast::channel(256).0,
            snapshot: Mutex::new("".into()),
            cmd: Some(cmd_tx),
            desks: Mutex::new(HashMap::new()),
            next_desk: AtomicU64::new(0),
        }));
        let mut task = Task {
            shared: alerts.0.clone(),
            settings,
            market: live,
            sink,
            eval: Evaluator::new(),
            delivery: Delivery::default(),
            prefs: Prefs::default(),
            history,
            devices,
            runners,
            desk_runners,
            pusher,
            seq: 0,
            op: 0,
            dropped_logged: 0,
        };
        task.rebuild_snapshot();
        tokio::spawn(async move { task.run(in_rx, cmd_rx, res_rx, run_rx).await });
        alerts
    }
}

/// The evaluator's window onto the live market (crate::market).
impl MarketView for Market {
    fn prev_close(&self, symbol: &str, date: NaiveDate) -> Option<f64> {
        self.current_closes()
            .before(symbol, date)
            .map(|(_, p)| p.to_f64())
    }

    fn seed(&self, symbol: &str) -> Option<Seed> {
        self.alert_seed(symbol)
    }
}

struct Task {
    shared: Arc<Shared>,
    settings: Settings,
    market: Market,
    sink: Arc<AlertSink>,
    eval: Evaluator,
    delivery: Delivery,
    prefs: Prefs,
    history: History,
    devices: Devices,
    runners: RunnersDelivery,
    /// The desktop and web notices' limits (the phones' rules, their own count: [`runners::desk_frame`]).
    desk_runners: RunnersDelivery,
    pusher: mpsc::Sender<Job>,
    seq: u64,
    op: u64,
    dropped_logged: u64,
}

impl Task {
    fn status(&self) -> StatusBody {
        let today = et(now_ms()).date_naive();
        StatusBody {
            phone_today: self.delivery.sent_today(today),
            phone_budget: self.prefs.phone_budget,
            devices: self.devices.len(),
            dropped: self.sink.dropped(),
        }
    }

    fn rebuild_snapshot(&mut self) {
        let frame = serde_json::to_string(&HistoryFrame {
            kind: "alerts.history",
            events: self.history.recent(),
            status: self.status(),
        })
        .expect("frames serialize");
        *self.shared.snapshot.lock().expect("alerts snapshot lock") = frame.into();
    }

    fn broadcast(&self, frame: String) {
        // No receivers = nobody connected; Activity still has it.
        let _ = self.shared.frames.send(frame.into());
    }

    /// Re-reads the rules (and with `prefs`, the preferences) from the store.
    async fn reload(&mut self, initial: bool) {
        let rules = model::parse_rules(self.settings.get(RULES_NS).await.map(|(_, v)| v).as_ref());
        self.prefs = model::parse_prefs(self.settings.get(PREFS_NS).await.map(|(_, v)| v).as_ref());
        let market = self.market.clone();
        self.eval.set_rules(rules, initial, &market);
        *self
            .sink
            .pressure_watch
            .lock()
            .expect("pressure watch lock") = Arc::from(self.eval.pressure_symbols());
    }

    /// Writes changed states to `alerts.state` (one patch), so every device shows them.
    async fn persist_states(&mut self) {
        let (set, remove) = self.eval.take_changes();
        if set.is_empty() && remove.is_empty() {
            return;
        }
        self.op += 1;
        let op_id = format!("srv-alerts-{}", self.op);
        let op = SettingsOp::Patch {
            set: model::patch_set(set),
            remove,
        };
        if let Err(e) = self.settings.apply(STATE_NS, &op_id, op).await {
            tracing::warn!(
                code = e.code(),
                "alerts: state not saved; clients show the last saved state until the next change"
            );
        }
    }

    async fn run(
        mut self,
        mut inputs: mpsc::Receiver<Input>,
        mut cmds: mpsc::Receiver<Command>,
        mut results: mpsc::Receiver<PushResult>,
        mut runner_notices: mpsc::Receiver<crate::rocket::Notice>,
    ) {
        let mut settings_feed = self.settings.subscribe();
        let stored =
            model::parse_states(self.settings.get(STATE_NS).await.map(|(_, v)| v).as_ref());
        self.eval.load_states(stored);
        self.reload(true).await;
        // Today's phone count and each stock's last notification, from Activity: a redeploy must
        // not hand out a fresh budget.
        let now = now_ms();
        let today = et(now).date_naive();
        let mut last_by_symbol = std::collections::HashMap::new();
        let mut sent = 0;
        for e in self
            .history
            .events()
            .filter(|e| e.phone == "sent" && et(e.at).date_naive() == today)
        {
            sent += 1;
            last_by_symbol.insert(e.symbol.clone(), e.at);
        }
        self.delivery.restore(today, sent, last_by_symbol);
        self.eval.tick(now);
        self.persist_states().await;
        self.rebuild_snapshot();
        tracing::info!(
            alerts = self.eval.rules().count(),
            phones = self.devices.len(),
            "alerts: evaluating the live market"
        );

        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_prune = now;
        loop {
            let mut changed = false;
            tokio::select! {
                input = inputs.recv() => {
                    let Some(first) = input else { break };
                    // Drain what is already queued: one state write and one snapshot per batch.
                    let mut batch = vec![first];
                    while batch.len() < 512 && let Ok(i) = inputs.try_recv() {
                        batch.push(i);
                    }
                    let market = self.market.clone();
                    for i in batch {
                        let now = now_ms();
                        for o in self.eval.on_input(i, now, &market) {
                            self.handle(o, now);
                            changed = true;
                        }
                    }
                }
                frame = settings_feed.recv() => match frame {
                    Ok(f) => {
                        // Only our namespaces; the prefix test avoids parsing every drawing change.
                        let ours = |ns: &str| f.starts_with(&format!("{{\"type\":\"settings.changed\",\"ns\":\"{ns}\""));
                        if ours(RULES_NS) || ours(PREFS_NS) {
                            self.reload(false).await;
                            changed = true;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        self.reload(false).await;
                        changed = true;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = tick.tick() => {
                    let now = now_ms();
                    self.eval.tick(now);
                    let dropped = self.sink.dropped();
                    if dropped > self.dropped_logged {
                        tracing::warn!(dropped, "alerts: the evaluator fell behind the market; inputs were dropped");
                        self.dropped_logged = dropped;
                        self.broadcast(status_frame(self.status()));
                    }
                    if now - last_prune > 3_600_000 {
                        last_prune = now;
                        changed |= self.history.prune(now);
                    }
                }
                cmd = cmds.recv() => {
                    match cmd {
                        Some(Command::Register { token, platform, name, runners }) => {
                            self.devices.register(&token, &platform, &name, now_ms(), runners);
                        }
                        Some(Command::Unregister { token }) => {
                            self.devices.remove(&token);
                        }
                        None => {}
                    }
                    self.broadcast(status_frame(self.status()));
                    changed = true;
                }
                Some(n) = runner_notices.recv() => self.runner(n, now_ms()),
                r = results.recv() => match r {
                    Some(PushResult::Failed { event_id, detail }) => {
                        tracing::warn!(event = %event_id, detail, "alerts: phone notification failed");
                        if let Some(e) = self.history.update(&event_id, |e| {
                            e.phone = "failed".into();
                            e.reason = Some("push_failed".into());
                            e.note = format!("{} · phone failed: {detail}", if e.app { "In app" } else { "Activity only" });
                        }) {
                            self.broadcast(event_frame("alert.updated", &e, None));
                            changed = true;
                        }
                    }
                    Some(PushResult::Unregistered { token }) if self.devices.remove(&token) => {
                        tracing::info!("alerts: a phone's push token is no longer registered; removed");
                        self.broadcast(status_frame(self.status()));
                        changed = true;
                    }
                    Some(PushResult::Unregistered { .. }) | None => {}
                },
            }
            self.persist_states().await;
            if changed {
                self.rebuild_snapshot();
            }
        }
    }

    /// One Runners notice: the push to the phones that opted in, within the limits
    /// ([`runners`]). Logged either way; never touches Activity (that is the price alerts').
    fn runner(&mut self, n: crate::rocket::Notice, now: i64) {
        let now_et = et(now);
        // Desktop and web first (runners::desk_frame): the same limits, decided once here with
        // the open clients that want this kind as the "devices", and their own count, so the
        // phones' switches and the desks' never hold each other back.
        let desk = self.desk_runners.decide(
            n.kind,
            &n.symbol,
            self.shared.desks_for(n.kind),
            &self.prefs,
            n.at_ns.div_euclid(1_000_000),
            now,
            now_et.date_naive(),
            now_et.hour() * 60 + now_et.minute(),
        );
        match desk {
            runners::Decision::Send => self.broadcast(runners::desk_frame(&n)),
            runners::Decision::Held(h) => {
                tracing::debug!(symbol = %n.symbol, kind = n.kind.as_str(), held = h.code(), "runners desk notice: held back");
            }
        }
        let tokens = self.devices.runner_tokens(n.kind);
        let decision = self.runners.decide(
            n.kind,
            &n.symbol,
            tokens.len(),
            &self.prefs,
            n.at_ns.div_euclid(1_000_000),
            now,
            now_et.date_naive(),
            now_et.hour() * 60 + now_et.minute(),
        );
        let kind = n.kind.as_str();
        if let runners::Decision::Held(h) = decision {
            tracing::info!(symbol = %n.symbol, kind, held = h.code(), "runners push: held back");
            return;
        }
        let (title, body, data) = runners::message(&n);
        let job = Job {
            // Not an Activity event: a failure later is only logged (history.update finds nothing).
            event_id: format!("runners-{kind}-{}", n.alert_id),
            tokens,
            title,
            body,
            data,
            // Sent at once, latency logged (runners, "Latency").
            runner: Some(push::RunnerLatency {
                symbol: n.symbol.clone(),
                kind,
                exec_ns: n.exec_ns,
                decided_ns: n.at_ns,
                queued_ns: now.saturating_mul(1_000_000),
            }),
        };
        if self.pusher.try_send(job).is_err() {
            tracing::warn!(symbol = %n.symbol, kind, "runners push: push queue full or closed; not sent");
        } else {
            tracing::info!(symbol = %n.symbol, kind, "runners push: sent to the push service");
        }
    }

    fn next_id(&mut self, at: i64) -> String {
        self.seq += 1;
        format!("h-{at:x}-{}", self.seq)
    }

    fn handle(&mut self, o: Outcome, now: i64) {
        match o {
            Outcome::Fired {
                alert_id,
                symbol,
                t,
                price,
                text,
                late,
                status,
                until,
            } => {
                let (app, wants_phone) = self
                    .eval
                    .rule(&alert_id)
                    .map_or((true, false), |r| (r.notify_app, r.notify_phone));
                let now_et = et(now);
                let phone = self.delivery.decide(
                    &symbol,
                    wants_phone,
                    &self.prefs,
                    self.devices.len(),
                    now,
                    now_et.date_naive(),
                    now_et.hour() * 60 + now_et.minute(),
                );
                let id = self.next_id(now);
                let after = match (status, until) {
                    (Status::Done, _) => "alert finished (once)".to_string(),
                    (Status::Cooling, Some(u)) => format!("cooling until {}", hhmm(u)),
                    _ => String::new(),
                };
                let (phone_word, reason, mut note) = match phone {
                    Phone::Send { .. } => (
                        "sent",
                        None,
                        if app {
                            "Phone + in app".to_string()
                        } else {
                            "Phone".to_string()
                        },
                    ),
                    Phone::Off => (
                        "off",
                        None,
                        if app {
                            "In app".to_string()
                        } else {
                            "Activity only".to_string()
                        },
                    ),
                    Phone::Held(h) => {
                        let why = match h {
                            delivery::PhoneHold::NoDevice => "no phone registered".to_string(),
                            delivery::PhoneHold::QuietHours => "quiet hours".to_string(),
                            delivery::PhoneHold::SymbolLimit => {
                                "same stock within 5 min, grouped into the next".to_string()
                            }
                            delivery::PhoneHold::Budget => {
                                format!("daily phone budget reached ({})", self.prefs.phone_budget)
                            }
                        };
                        (
                            "held",
                            Some(h.code().to_string()),
                            format!(
                                "{} · phone held: {why}",
                                if app { "In app only" } else { "Activity only" }
                            ),
                        )
                    }
                };
                if late {
                    note.push_str(" · late print");
                }
                if !after.is_empty() {
                    note.push_str(" · ");
                    note.push_str(&after);
                }
                let mut event = Event {
                    id: id.clone(),
                    alert_id: alert_id.clone(),
                    symbol: symbol.clone(),
                    kind: EventKind::Fired,
                    t,
                    at: now,
                    text: text.clone(),
                    price,
                    late,
                    app,
                    phone: phone_word.into(),
                    reason,
                    note,
                };
                if let Phone::Send { extra } = phone {
                    let body = if extra > 0 {
                        format!("{text} · +{extra} more since the last one")
                    } else {
                        text.clone()
                    };
                    let job = Job {
                        event_id: id.clone(),
                        tokens: self.devices.tokens(),
                        title: symbol.clone(),
                        body,
                        // The app opens its Alerts tab on a tap (apps/mobile src/notifications).
                        data: json!({ "alertId": alert_id, "symbol": symbol, "eventId": id, "url": "stockwatcher://alerts" }),
                        runner: None,
                    };
                    if self.pusher.try_send(job).is_err() {
                        event.phone = "failed".into();
                        event.reason = Some("push_failed".into());
                        event.note = format!(
                            "{} · phone failed: push queue full",
                            if app { "In app" } else { "Activity only" }
                        );
                    }
                }
                let today = self.delivery.sent_today(et(now).date_naive());
                self.broadcast(event_frame("alert.fired", &event, Some(today)));
                self.history.push(event);
            }
            Outcome::Held {
                alert_id,
                symbol,
                t,
                price,
                reason,
                text,
                until,
            } => {
                let id = self.next_id(now);
                let gap = self.eval.rule(&alert_id).and_then(|r| match r.fire {
                    Fire::Every { gap_min } => Some(gap_min),
                    Fire::Once => None,
                });
                let note = match reason {
                    HoldReason::Stale => format!(
                        "Not fired: the last trade before it was over {} min old (stale prices never fire an alert)",
                        if matches!(
                            market_events::classify(market_events::UnixNanos::from_millis(t))
                                .map(|s| s.kind),
                            Some(market_events::SessionKind::Regular)
                        ) {
                            10
                        } else {
                            30
                        }
                    ),
                    HoldReason::Halted => {
                        "Not fired: the stock is halted (only halt alerts fire)".to_string()
                    }
                    HoldReason::Gap => format!(
                        "Held back: {} min apart rule{}",
                        gap.unwrap_or(model::DEFAULT_GAP_MIN),
                        until
                            .map(|u| format!(
                                " · next eligible {} once the price moves back 0.5%",
                                hhmm(u)
                            ))
                            .unwrap_or_default()
                    ),
                };
                let event = Event {
                    id,
                    alert_id,
                    symbol,
                    kind: EventKind::Held,
                    t,
                    at: now,
                    text,
                    price,
                    late: false,
                    app: false,
                    phone: "none".into(),
                    reason: Some(reason.code().into()),
                    note,
                };
                self.broadcast(event_frame("alert.held", &event, None));
                self.history.push(event);
            }
        }
    }
}

/// `{type:"alerts.status"}` for tests that read frames.
pub fn parse_frame_type(frame: &str) -> Option<String> {
    serde_json::from_str::<Value>(frame)
        .ok()?
        .get("type")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn the_service_fires_records_and_persists_state() {
        let settings = Settings::in_memory();
        let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let alerts = Alerts::start(
            settings.clone(),
            live.clone(),
            None,
            PushConfig {
                base: "http://127.0.0.1:9".into(),
                access_token: None,
            },
        );
        let mut frames = alerts.subscribe();
        let now = now_ms();
        settings
            .apply(RULES_NS, "op1", SettingsOp::Patch {
                set: json!({"a1": {"symbol": "XLAB", "rule": {"kind": "price", "dir": "above", "price": 2.5}, "fire": "once", "sessions": ["pre", "regular", "after", "overnight"], "expiry": "never", "createdAt": now, "editedAt": now}}).as_object().unwrap().clone(),
                remove: vec![],
            })
            .await
            .unwrap();
        // The rules reload, and the new alert's state is written as armed.
        let mut armed = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if let Some((_, v)) = settings.get(STATE_NS).await
                && v["a1"]["status"] == "armed"
            {
                armed = true;
                break;
            }
        }
        assert!(armed, "state written");
        // Two prints a second apart, now, in the regular session (fixed, so the test does not
        // depend on the wall clock's session): a cross, so it fires (no phone registered: held).
        let t = now_ms();
        let sess = Some((et(t).date_naive(), market_events::SessionKind::Regular));
        let sink = live.alert_sink().expect("attached");
        sink.offer(Input::Trade {
            symbol: "XLAB".into(),
            price: 2.4,
            ts: t - 1000,
            session: sess,
        });
        sink.offer(Input::Trade {
            symbol: "XLAB".into(),
            price: 2.6,
            ts: t,
            session: sess,
        });
        let frame = loop {
            let f = tokio::time::timeout(Duration::from_secs(5), frames.recv())
                .await
                .unwrap()
                .unwrap();
            if f.starts_with("{\"type\":\"alert.") {
                break f;
            }
        };
        let v: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["type"], "alert.fired");
        assert_eq!(v["event"]["text"], "Crossed above 2.50 at 2.60");
        assert_eq!(v["event"]["phone"], "held");
        assert_eq!(v["event"]["reason"], "no_device");
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if settings.get(STATE_NS).await.unwrap().1["a1"]["status"] == "done" {
                assert!(alerts.snapshot_frame().contains("Crossed above 2.50"));
                assert_eq!(
                    parse_frame_type(&alerts.snapshot_frame()).as_deref(),
                    Some("alerts.history")
                );
                return;
            }
        }
        panic!("state not persisted as done");
    }

    fn notice(kind: crate::rocket::NoticeKind, symbol: &str, at_ms: i64) -> crate::rocket::Notice {
        crate::rocket::Notice {
            kind,
            alert_id: format!("2026-10-22-{symbol}-1"),
            symbol: symbol.into(),
            at_ns: at_ms * 1_000_000,
            price: 2.5,
            from_flag_pct: Some(0.0),
            reasons: vec!["volume 9x pace".into()],
            since_start_min: None,
            exec_ns: None,
        }
    }

    /// The Runners pushes through the real task with a fake sender (a channel read here; no
    /// Expo, no network): a new flag goes to the phones opted in to new flags, a breakout only to
    /// those opted in to breakouts, the per-stock cooldown holds the second, and a dead push
    /// queue costs the push, never the task.
    #[tokio::test]
    async fn runners_pushes_follow_the_opt_ins_and_limits() {
        use crate::rocket::NoticeKind;
        let settings = Settings::in_memory();
        let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let (job_tx, mut jobs) = mpsc::channel(16);
        let (_res_tx, res_rx) = mpsc::channel(4);
        let alerts = Alerts::start_with(settings, live.clone(), None, job_tx, res_rx);
        let mut frames = alerts.subscribe();
        let sink = live.alert_sink().expect("attached").clone();
        let next_job = |jobs: &mut mpsc::Receiver<Job>| {
            let j = jobs.try_recv();
            j.ok()
        };
        // No phone yet: held (no_device), nothing queued.
        sink.offer_runner(notice(NoticeKind::New, "AAA", now_ms()));
        // The notice and the registers below travel on different channels: let it land first.
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Two phones: the Fold with defaults (new on, breakout off), a tablet with breakouts on.
        assert!(alerts.register_device(
            "ExponentPushToken[fold]",
            "android",
            "Fold",
            RunnersOptIn::default()
        ));
        assert!(alerts.register_device(
            "ExponentPushToken[tab]",
            "android",
            "Tab",
            RunnersOptIn {
                new: Some(false),
                breakout: Some(true),
                burst: Some(false),
                running: None,
                run: None,
                fast: None,
            },
        ));
        // Each register answers with an alerts.status frame once handled.
        let mut statuses = 0;
        while statuses < 2 {
            let f = tokio::time::timeout(Duration::from_secs(5), frames.recv())
                .await
                .unwrap()
                .unwrap();
            if f.starts_with("{\"type\":\"alerts.status\"") {
                statuses += 1;
            }
        }
        assert!(
            next_job(&mut jobs).is_none(),
            "no phone was registered for AAA"
        );
        sink.offer_runner(notice(NoticeKind::New, "CJMB", now_ms()));
        let j = tokio::time::timeout(Duration::from_secs(5), jobs.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(j.tokens, vec!["ExponentPushToken[fold]".to_string()]);
        assert_eq!(j.title, "Runners: CJMB $2.50");
        assert_eq!(j.data["symbol"], "CJMB");
        assert_eq!(j.data["kind"], "runners.new");
        // A breakout of the same stock right after: the 10-minute cooldown holds it.
        sink.offer_runner(notice(NoticeKind::Breakout, "CJMB", now_ms()));
        // Another stock's breakout: only the tablet opted in.
        sink.offer_runner(notice(NoticeKind::Breakout, "BBB", now_ms()));
        let j = tokio::time::timeout(Duration::from_secs(5), jobs.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            j.data["symbol"], "BBB",
            "the CJMB breakout was held by the cooldown"
        );
        assert_eq!(j.tokens, vec!["ExponentPushToken[tab]".to_string()]);
        assert!(j.title.starts_with("BBB breaking out"));
        // A buying burst: the Fold (bursts on by default), not the tablet (turned them off).
        sink.offer_runner(notice(NoticeKind::Burst, "EEE", now_ms()));
        let j = tokio::time::timeout(Duration::from_secs(5), jobs.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(j.tokens, vec!["ExponentPushToken[fold]".to_string()]);
        assert_eq!(j.title, "EEE · Buying burst · $2.50");
        assert_eq!(j.data["kind"], "runners.burst");
        // Old news (a stalled board catching up) is not pushed.
        sink.offer_runner(notice(NoticeKind::New, "OLD", now_ms() - 10 * 60_000));
        // The push service gone: the task carries on (it still answers a register).
        drop(jobs);
        sink.offer_runner(notice(NoticeKind::New, "DDD", now_ms()));
        alerts.unregister_device("ExponentPushToken[tab]");
        loop {
            let f = tokio::time::timeout(Duration::from_secs(5), frames.recv())
                .await
                .unwrap()
                .unwrap();
            if f.starts_with("{\"type\":\"alerts.status\"") {
                assert!(f.contains("\"devices\":1"), "{f}");
                break;
            }
        }
        assert_eq!(sink.runners_dropped(), 0);
    }

    /// Desktop and web notices (runners::desk_frame) through the real task: none while no client
    /// listens (and that costs no gap), the phone's text once one does, its switches respected,
    /// the 10-minute gap applied, and a closed socket's guard takes it off the list.
    #[tokio::test]
    async fn desk_notices_follow_the_open_clients_and_limits() {
        use crate::rocket::NoticeKind;
        let settings = Settings::in_memory();
        let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let (job_tx, _jobs) = mpsc::channel(16);
        let (_res_tx, res_rx) = mpsc::channel(4);
        let alerts = Alerts::start_with(settings, live.clone(), None, job_tx, res_rx);
        let mut frames = alerts.subscribe();
        let sink = live.alert_sink().expect("attached").clone();
        let next_notice = async |frames: &mut broadcast::Receiver<Arc<str>>| loop {
            match tokio::time::timeout(Duration::from_millis(400), frames.recv()).await {
                Ok(Ok(f)) if f.starts_with("{") && f.contains("\"runners.notice\"") => {
                    break Some(serde_json::from_str::<Value>(&f).unwrap());
                }
                Ok(Ok(_)) => continue,
                _ => break None,
            }
        };
        // Nobody listening: held (no_device) and the stock's gap is not spent.
        sink.offer_runner(notice(NoticeKind::New, "CJMB", now_ms()));
        assert!(next_notice(&mut frames).await.is_none());
        let desk = alerts.desk(DeskKinds::from_opt_in(RunnersOptIn::default()));
        sink.offer_runner(notice(NoticeKind::New, "CJMB", now_ms()));
        let f = next_notice(&mut frames).await.expect("a notice");
        let (title, body, _) = runners::message(&notice(NoticeKind::New, "CJMB", 0));
        assert_eq!(f["title"], title.as_str());
        assert_eq!(f["body"], body.as_str());
        assert_eq!(f["kind"], "runners.new");
        assert_eq!(f["id"], "runners-new-2026-10-22-CJMB-1");
        // Same stock within 10 minutes: held. Breakouts are off by default.
        sink.offer_runner(notice(NoticeKind::Run, "CJMB", now_ms()));
        sink.offer_runner(notice(NoticeKind::Breakout, "BBB", now_ms()));
        assert!(next_notice(&mut frames).await.is_none());
        desk.set(DeskKinds {
            breakout: true,
            ..DeskKinds::from_opt_in(RunnersOptIn::default())
        });
        sink.offer_runner(notice(NoticeKind::Breakout, "BBB", now_ms()));
        let f = next_notice(&mut frames).await.expect("a breakout");
        assert_eq!(f["kind"], "runners.breakout");
        // The socket ended: nobody listens again.
        drop(desk);
        sink.offer_runner(notice(NoticeKind::Fast, "FFF", now_ms()));
        assert!(next_notice(&mut frames).await.is_none());
    }

    #[test]
    fn idle_snapshot_is_a_valid_frame() {
        let a = Alerts::idle();
        let v: Value = serde_json::from_str(&a.snapshot_frame()).unwrap();
        assert_eq!(v["type"], "alerts.history");
        assert_eq!(v["phoneBudget"], 20);
        assert!(!a.register_device("junk", "", "", RunnersOptIn::default()));
    }
}
