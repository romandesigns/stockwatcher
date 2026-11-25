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
//! - client `push.register {token, platform?, name?, runnersNew?, runnersBreakout?, …}` (every
//!   switch: `crate::ws`) /
//!   `push.unregister {token}`: this phone's Expo push token (per device, never synced to other
//!   clients) and its Runners push opt-ins ([`runners`]; absent = unchanged).
//!
//! The same task also pushes the Runners board's new flags and breakouts ([`runners`]): the live
//! market offers them through [`AlertSink::offer_runner`], as non-blocking as the price inputs.
//! And it turns the exchange's halts and resumptions of covered stocks into notices ([`halts`]),
//! from the same inputs, delivered exactly like the Runners ones.
//!
//! # Detector switches (no restart)
//!
//! The research detectors shipped live by the owner's 2026-11-22 decision carry a runtime switch
//! each, in the synced settings namespace [`DETECTORS_NS`] (`detectors`, parsed once for every
//! item in [`detectors`]): `{"b-good-1": {"on": false}, ...}`. An absent entry takes the
//! detector's default: ON for every shipped item, OFF for the cut `run` kind. The task re-reads it the moment a
//! `settings.changed` for it arrives, so the owner can switch one off from any client during
//! market hours (DEPLOY-PLAN.md kill rules: no restart, only switches). A switched-off detector
//! keeps running: its notices are decided and ledgered as `held: switched_off` on both paths, so
//! its would-be alerts are still counted. It is its own namespace, never inside `alerts.prefs`,
//! which clients write as one value and would erase it; clients may write it (it is not
//! server-owned). Ids: `run` (the cut Run +20% kind), `x-halt-1`, `x-float-1`, `x-gap-1`,
//! `x-ctx-1`, `rw-mp5-1` (the Signals crossing gate), `b-good-1` (the confidence tag),
//! `b-good-2` (model picks, kind `model.pick`), `rw2-vwapr-1` and `rw2-level-1` (kinds
//! `v2.vwapr` / `v2.level`, `crate::rw2_live`); [`detectors::ALL`].
//!
//! # Confidence (b-good-1)
//!
//! A B0-kind alert's tag is scored 30 s after it by `crate::bgood` (its quotes fetched by
//! `crate::bgood::stage`) and comes back here as a [`crate::bgood::Confidence`]. Every one is
//! ledgered (`confidence` line, `ledger.rs`); a tagged one, with its switch on, goes to every
//! connection as `runners.confidence {eventId, alertId, symbol, notice, tier, score, at}` and
//! marks its Runners row (`Market::set_runner_confidence`). A tag never sends a push of its own:
//! the alert it belongs to was already delivered. Clients show a quiet "High confidence" chip on
//! the alert (packages/shared `confidence.ts`).

pub mod delivery;
pub mod detectors;
pub mod eval;
pub mod geometry;
pub mod halts;
pub mod history;
pub mod ledger;
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

/// The synced settings namespace of the research detectors' no-restart switches, parsed in one
/// place for every item ([`detectors`], "What off does").
pub use detectors::NS as DETECTORS_NS;

/// How far the evaluator may fall behind the market before inputs are dropped (and counted).
pub const INPUT_CAPACITY: usize = 8192;
/// Runners notices waiting for the task (a handful a day; a full queue drops and counts).
pub const RUNNERS_CAPACITY: usize = 64;
/// Expiry, re-arming by the clock and the dropped-input report.
/// Expiry, snooze and cooling flips are decided on this tick (crate::alerts::eval `tick`) and
/// shown by the clients as written, so it is the latency of those flips on every screen.
const TICK: Duration = Duration::from_secs(1);

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
    /// b-good-1's tags due for quotes and a score (`crate::bgood::stage`), from the market.
    tags: Option<mpsc::Sender<crate::bgood::TagRequest>>,
    tags_dropped: AtomicU64,
}

impl AlertSink {
    /// A b-good-1 tag due now (module docs, "Confidence"). Never blocks: a full channel drops it,
    /// counted and logged; the alert itself was already offered.
    pub fn offer_tag(&self, r: crate::bgood::TagRequest) {
        let Some(tx) = &self.tags else { return };
        if tx.try_send(r).is_err() {
            let n = self.tags_dropped.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(
                dropped = n,
                "b-good-1: the confidence stage is not keeping up; a tag was dropped"
            );
        }
    }

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
            tags: None,
            tags_dropped: AtomicU64::new(0),
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
    /// held as `no_device`, exactly as a phone's switch does.
    desks: Mutex<HashMap<u64, DeskKinds>>,
    next_desk: AtomicU64,
    /// The symbols each open live connection has on a chart (its `chart.subscribe` slots, so a
    /// linked chart counts too), by connection ([`ChartGuard`]): part of the halt notices'
    /// coverage ([`halts`]).
    charts: Mutex<HashMap<u64, Vec<String>>>,
}

impl Shared {
    /// Whether an open live client has `symbol` on a chart.
    fn charted(&self, symbol: &str) -> bool {
        self.charts
            .lock()
            .expect("charts lock")
            .values()
            .any(|v| v.iter().any(|s| s == symbol))
    }

    /// How many desk connections want `n`: the desk side's recipients.
    fn desks_wanting(&self, n: &crate::rocket::Notice) -> usize {
        self.desks
            .lock()
            .expect("desks lock")
            .values()
            .filter(|k| k.wants(n))
            .count()
    }
}

/// One desktop or web client's Runners notification switches (`runners.notify`). Defaults are
/// the phone's ([`push::Devices::register`]): every kind on except breakouts.
///
/// The same struct is a phone's switches ([`push::Device::kinds`]): one place decides which
/// switch a notice answers to ([`DeskKinds::wants`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeskKinds {
    pub new: bool,
    pub breakout: bool,
    pub burst: bool,
    pub running: bool,
    pub run: bool,
    pub fast: bool,
    /// Volume-shelf entry of a Runners stock (on by default).
    pub shelf: bool,
    /// A later Runners path on a stock that already has a row (on by default).
    pub follow_up: bool,
    /// Ross / Signals "Crossing" (on by default).
    pub ross_crossing: bool,
    pub signals_crossing: bool,
    /// Ross / Signals "At trigger" (off by default).
    pub ross_trigger: bool,
    pub signals_trigger: bool,
    /// A Halt watcher gauge appearing (off by default).
    pub halt: bool,
    /// "Pullback setting up" and its crossing (`crate::cue_notices`, "Pullback"; on by default).
    pub pullback: bool,
    /// The exchange halted / resumed a covered stock ([`halts`]; both on by default).
    pub halted: bool,
    pub resumed: bool,
    /// The N1 detector's alerts (`crate::n1_live`): ON by default since T2-c6 was confirmed (owner, 2026-11-17).
    pub n1: bool,
    /// T2-c4's F1u "first pullback after ignition" (`crate::c4_live`): ON by default (owner, 2026-11-18).
    pub f1u: bool,
    /// T2-c4's HALT add-on, a bull Halt watcher gauge 09:30–11:30 (`crate::c4_live::halt_addon`):
    /// ON by default (owner, 2026-11-18).
    pub halt_bull: bool,
    /// Research v2's x-halt-1 "resumed up after a halt" (`crate::xhalt_live`): ON by default
    /// (owner, 2026-11-22).
    pub resumed_up: bool,
    /// Research v2's x-float-1 "rotation ignition" (`crate::xfloat_live`): ON by default (owner,
    /// 2026-11-22: ship the research-proven wins live).
    pub x_float: bool,
    /// The v2 finalists x-gap-1 and x-ctx-1 (`crate::gap_ctx`): ON by default (owner, 2026-11-22).
    pub x_gap: bool,
    pub x_ctx: bool,
    /// research v2 b-good-2's model picks (`crate::bgood`): ON by default (owner, 2026-11-22).
    pub model_pick: bool,
    /// The v2 rework-2 finalists rw2-vwapr-1 "VWAP reclaim" and rw2-level-1 "20-day high break"
    /// (`crate::rw2_live`): ON by default (owner, 2026-11-22).
    pub vwap_reclaim: bool,
    pub level_break: bool,
}

impl Default for DeskKinds {
    fn default() -> Self {
        DeskKinds::from_opt_in(RunnersOptIn::default())
    }
}

impl DeskKinds {
    /// An absent switch takes the phone's default (owner, 2026-10-23: every evidence-backed
    /// kind on; "At trigger", the Halt watcher and breakouts opt-in).
    pub fn from_opt_in(o: RunnersOptIn) -> Self {
        DeskKinds {
            new: o.new.unwrap_or(true),
            breakout: o.breakout.unwrap_or(false),
            burst: o.burst.unwrap_or(true),
            running: o.running.unwrap_or(true),
            run: o.run.unwrap_or(true),
            fast: o.fast.unwrap_or(true),
            shelf: o.shelf.unwrap_or(true),
            follow_up: o.follow_up.unwrap_or(true),
            ross_crossing: o.ross_crossing.unwrap_or(true),
            signals_crossing: o.signals_crossing.unwrap_or(true),
            ross_trigger: o.ross_trigger.unwrap_or(false),
            signals_trigger: o.signals_trigger.unwrap_or(false),
            halt: o.halt.unwrap_or(false),
            // Owner, 2026-10-25: asked to receive it, so on like the other evidence kinds.
            pullback: o.pullback.unwrap_or(true),
            // Owner-approved plan "halt sounds" (2026-11-16): both on.
            halted: o.halted.unwrap_or(true),
            resumed: o.resumed.unwrap_or(true),
            // Owner, 2026-11-17: on, as Engine A finalist T2-c6 (passed Validation, sealed look 2 and
            // 234 forward days); still switchable in Settings.
            n1: o.n1.unwrap_or(true),
            // Owner, 2026-11-18: on, as Engine A finalist T2-c4 (Validation, BH, and 234 forward
            // days: +7.9 pp recall with 22 fewer alerts a day); still switchable in Settings.
            f1u: o.f1u.unwrap_or(true),
            halt_bull: o.halt_bull.unwrap_or(true),
            // Owner, 2026-11-22: ship the v2 items that proved better than live (x-halt-1:
            // Validation +0.07 pp recall, +0.10 pp precision); still switchable in Settings.
            resumed_up: o.resumed_up.unwrap_or(true),
            // Owner, 2026-11-22: on, as research v2 finalist x-float-1 (Validation +1.1 pp recall,
            // +0.15 pp precision over the live baseline); still switchable in Settings.
            x_float: o.x_float.unwrap_or(true),
            // Owner, 2026-11-22: ship what beat production ("push it live"); still switchable here
            // and for everyone in the `detectors` settings (DETECTORS_NS).
            x_gap: o.x_gap.unwrap_or(true),
            x_ctx: o.x_ctx.unwrap_or(true),
            // Owner, 2026-11-22 (DEPLOY-PLAN addendum: the proven wins go live, no shadow
            // period); still switchable here and by the runtime detector switch `b-good-2`.
            model_pick: o.model_pick.unwrap_or(true),
            // Owner, 2026-11-22 (ship every proven win); FINALISTS-v2c, CONFIRMED-TRAIN. Still
            // switchable here and by the runtime detector switches `rw2-vwapr-1` / `rw2-level-1`.
            vwap_reclaim: o.vwap_reclaim.unwrap_or(true),
            level_break: o.level_break.unwrap_or(true),
        }
    }

    /// Whether these switches want `n`. A Ross notice whose episode the Signals panel shows too
    /// (`crate::cue_notices`, "One episode, one notice") is wanted through either switch.
    pub fn wants(self, n: &crate::rocket::Notice) -> bool {
        use crate::rocket::{NoticeDetail, NoticeKind as K};
        let also_signals = match &n.detail {
            Some(NoticeDetail::Cue(c)) => c.also_signals,
            Some(NoticeDetail::Pullback(p)) => p.also_signals,
            _ => false,
        };
        match n.kind {
            K::New => self.new,
            K::Breakout => self.breakout,
            K::Burst => self.burst,
            K::Running => self.running,
            K::Run => self.run,
            K::Fast => self.fast,
            K::Shelf => self.shelf,
            K::FollowUp => self.follow_up,
            K::RossCrossing => self.ross_crossing || (also_signals && self.signals_crossing),
            K::SignalsCrossing => self.signals_crossing,
            K::RossTrigger => self.ross_trigger || (also_signals && self.signals_trigger),
            K::SignalsTrigger => self.signals_trigger,
            K::Halt => self.halt,
            K::Pullback => self.pullback,
            // A Runners stock's crossing that the Signals panel lists too is the Signals
            // crossing as well (one episode, one notice), as the Ross kind is.
            K::PullbackCrossing => self.pullback || (also_signals && self.signals_crossing),
            K::Halted => self.halted,
            K::Resumed => self.resumed,
            K::N1 => self.n1,
            K::F1u => self.f1u,
            // One event, one notice: with the gauge's own switch on, its Halt watcher notice
            // already says it (as the Ross/Signals twins do).
            K::HaltBull => self.halt_bull && !self.halt,
            K::ResumedUp => self.resumed_up,
            K::XFloat => self.x_float,
            K::XGap => self.x_gap,
            K::XCtx => self.x_ctx,
            K::BGood => self.model_pick,
            K::VwapReclaim => self.vwap_reclaim,
            K::LevelBreak => self.level_break,
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

/// A live connection's charted symbols for the halt coverage ([`halts`]); dropping it (the socket
/// ended) takes them out.
pub struct ChartGuard {
    shared: Arc<Shared>,
    id: u64,
}

impl ChartGuard {
    /// The connection's chart slots changed: these are its symbols now.
    pub fn set<'a>(&self, symbols: impl IntoIterator<Item = &'a str>) {
        let mut v: Vec<String> = symbols.into_iter().map(str::to_owned).collect();
        v.sort();
        v.dedup();
        let mut charts = self.shared.charts.lock().expect("charts lock");
        if v.is_empty() {
            charts.remove(&self.id);
        } else {
            charts.insert(self.id, v);
        }
    }
}

impl Drop for ChartGuard {
    fn drop(&mut self) {
        self.shared
            .charts
            .lock()
            .expect("charts lock")
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
            charts: Mutex::new(HashMap::new()),
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

    /// A live connection's chart slots, for the halt notices' coverage ([`halts`]). The guard
    /// keeps them listed until the socket ends.
    pub fn charts(&self) -> ChartGuard {
        ChartGuard {
            shared: self.0.clone(),
            id: self.0.next_desk.fetch_add(1, Ordering::Relaxed),
        }
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
        ledger: ledger::Ledger,
        spread: Option<crate::spread::QuoteSource>,
    ) -> Alerts {
        let (res_tx, res_rx) = mpsc::channel(64);
        let pusher = push::spawn(push_cfg, res_tx, ledger.clone());
        Self::start_inner(settings, live, dir, pusher, res_rx, ledger, spread)
    }

    /// [`Alerts::start`] with the push sender given: tests hand in a channel they read instead
    /// of Expo (no network), and the results channel they feed.
    #[cfg(test)]
    fn start_with(
        settings: Settings,
        live: Market,
        dir: Option<PathBuf>,
        pusher: mpsc::Sender<Job>,
        res_rx: mpsc::Receiver<PushResult>,
        ledger: ledger::Ledger,
    ) -> Alerts {
        Self::start_inner(settings, live, dir, pusher, res_rx, ledger, None)
    }

    /// The two above, with the fire-time spread's quote source (`crate::spread`): with one,
    /// the Runners notices pass through the spread stage on their way to the task; without one
    /// (tests, no Alpaca keys, `ALERT_SPREAD=0`) they go straight to it and carry no spread.
    fn start_inner(
        settings: Settings,
        live: Market,
        dir: Option<PathBuf>,
        pusher: mpsc::Sender<Job>,
        res_rx: mpsc::Receiver<PushResult>,
        ledger: ledger::Ledger,
        spread: Option<crate::spread::QuoteSource>,
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
        // With a quote source the market's end of the Runners channel feeds the spread stage,
        // which hands each notice on, priced, through a second channel of the same size.
        let run_tx = match spread.clone() {
            Some(src) => {
                let (stage_tx, stage_rx) = mpsc::channel(RUNNERS_CAPACITY);
                crate::spread::spawn_stage(src, live.clone(), stage_rx, run_tx);
                stage_tx
            }
            None => run_tx,
        };
        // b-good-1's tags: market -> confidence stage (quotes, score) -> this task.
        let (tag_tx, tag_rx) = mpsc::channel(RUNNERS_CAPACITY);
        let (conf_tx, conf_rx) = mpsc::channel(RUNNERS_CAPACITY);
        crate::bgood::stage::spawn(spread.clone(), tag_rx, conf_tx);
        let sink = Arc::new(AlertSink {
            tx: in_tx,
            dropped: AtomicU64::new(0),
            runners: run_tx,
            runners_dropped: AtomicU64::new(0),
            pressure_watch: Mutex::new(Arc::from(Vec::new())),
            tags: Some(tag_tx),
            tags_dropped: AtomicU64::new(0),
        });
        live.attach_alerts(sink.clone());
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let alerts = Alerts(Arc::new(Shared {
            frames: broadcast::channel(256).0,
            snapshot: Mutex::new("".into()),
            cmd: Some(cmd_tx),
            desks: Mutex::new(HashMap::new()),
            next_desk: AtomicU64::new(0),
            charts: Mutex::new(HashMap::new()),
        }));
        let mut task = Task {
            shared: alerts.0.clone(),
            settings,
            market: live,
            sink,
            eval: Evaluator::new(),
            delivery: Delivery::default(),
            prefs: Prefs::default(),
            detectors: detectors::Switches::default(),
            history,
            devices,
            runners,
            desk_runners,
            halts: halts::HaltTracker::default(),
            pusher,
            ledger,
            mp5: crate::mp5_gate::Mp5Hold::default(),
            seq: 0,
            op: 0,
            dropped_logged: 0,
        };
        task.rebuild_snapshot();
        tokio::spawn(async move { task.run(in_rx, cmd_rx, res_rx, run_rx, conf_rx).await });
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

    fn slots(&self, symbol: &str, tf_ms: i64) -> Option<Vec<i64>> {
        self.alert_slots(symbol, tf_ms)
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
    /// The runtime detector switches (`detectors` namespace, [`detectors`]): re-read on every
    /// change, no restart.
    detectors: detectors::Switches,
    history: History,
    devices: Devices,
    runners: RunnersDelivery,
    /// The desktop and web notices' limits (the phones' rules, their own count: [`runners::desk_frame`]).
    desk_runners: RunnersDelivery,
    /// Every live symbol's halt state, for the Halted / Resumed notices ([`halts`]).
    halts: halts::HaltTracker,
    pusher: mpsc::Sender<Job>,
    /// Every decided notice, on the data volume (ledger.rs).
    ledger: ledger::Ledger,
    /// rw-mp5-1's Signals crossings waiting out the 5 s B0 window (`crate::mp5_gate`).
    mp5: crate::mp5_gate::Mp5Hold,
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
        self.reload_detectors(initial).await;
        let market = self.market.clone();
        self.eval.set_rules(rules, initial, &market);
        *self
            .sink
            .pressure_watch
            .lock()
            .expect("pressure watch lock") = Arc::from(self.eval.pressure_symbols());
        // Drawing alerts read their drawings from the same store, every reload (a rule may
        // have moved to another drawing or symbol).
        for symbol in self.eval.drawing_symbols() {
            self.reload_drawings(&symbol).await;
        }
    }

    /// The runtime detector switches ([`detectors`]); logged when they change.
    async fn reload_detectors(&mut self, initial: bool) {
        let d = detectors::Switches::parse(
            self.settings
                .get(detectors::NS)
                .await
                .map(|(_, v)| v)
                .as_ref(),
        );
        if initial || d != self.detectors {
            // One line per switchable detector with its frozen version: the daily checklist
            // reads which items were live from these.
            for x in detectors::ALL {
                tracing::info!(
                    detector = x.id,
                    version = x.version,
                    on = d.on(&x),
                    "detectors: switch"
                );
            }
        }
        self.detectors = d;
    }

    /// One symbol's `drawings:<SYMBOL>` for its drawing alerts (eval `set_drawings`).
    async fn reload_drawings(&mut self, symbol: &str) {
        let doc = self
            .settings
            .get(&format!("{DRAWINGS_PREFIX}{symbol}"))
            .await
            .map(|(_, v)| v);
        self.eval.set_drawings(symbol, doc.as_ref());
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
        mut confidence: mpsc::Receiver<crate::bgood::Confidence>,
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
                        // Before the evaluator, which keeps only the symbols its rules watch.
                        let halt_notices = self.halt_input(&i, now);
                        for o in self.eval.on_input(i, now, &market) {
                            self.handle(o, now);
                            changed = true;
                        }
                        for n in halt_notices {
                            self.runner(n, now);
                        }
                    }
                }
                frame = settings_feed.recv() => match frame {
                    Ok(f) => {
                        // Only our namespaces; the prefix test avoids parsing every drawing change.
                        let ours = |ns: &str| f.starts_with(&format!("{{\"type\":\"settings.changed\",\"ns\":\"{ns}\""));
                        if ours(detectors::NS) {
                            // A detector switched on or off (kill rules): the next notice obeys.
                            self.reload_detectors(false).await;
                        } else if ours(RULES_NS) || ours(PREFS_NS) {
                            self.reload(false).await;
                            // A snooze set or an edit shows at once, not on the next tick.
                            self.eval.tick(now_ms());
                            changed = true;
                        } else if let Some(symbol) = changed_drawings(&f)
                            && self.eval.drawing_symbols().contains(&symbol)
                        {
                            // A watched drawing moved or was deleted: the alert follows it.
                            self.reload_drawings(&symbol).await;
                            changed = true;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        self.reload(false).await;
                        // As above: a re-armed entry gets its `snoozed` before it is saved, so no
                        // client falls back to its own clock for a tick.
                        self.eval.tick(now_ms());
                        changed = true;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = tick.tick() => {
                    let now = now_ms();
                    self.eval.tick(now);
                    // Resumed notices whose reopening print did not come (halts::RESUME_WAIT_MS).
                    for n in self.halts.tick(now) {
                        self.runner(n, now);
                    }
                    // rw-mp5-1's crossings whose 5 s window closed.
                    self.release_mp5(now);
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
                Some(c) = confidence.recv() => self.confidence(c, now_ms()),
                r = results.recv() => match r {
                    Some(PushResult::Failed { event_id, detail }) => {
                        tracing::warn!(event = %event_id, detail, "alerts: phone notification failed");
                        self.ledger.failed(&event_id, &detail, now_ms());
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

    /// Feeds the halt tracker one input ([`halts`]): the notices it decides now. Coverage is
    /// checked only at a status (a handful a day), never per print.
    fn halt_input(&mut self, i: &Input, now: i64) -> Vec<crate::rocket::Notice> {
        match i {
            Input::Trade {
                symbol, price, ts, ..
            } => self
                .halts
                .on_trade(symbol, *price, *ts)
                .into_iter()
                .collect(),
            Input::Status {
                symbol,
                halted,
                ts,
                session,
                reason,
            } => {
                let covered = self.halt_covered(symbol);
                let prev_close = if covered && *halted {
                    session.and_then(|(d, _)| self.market.prev_close(symbol, d))
                } else {
                    None
                };
                self.halts
                    .on_status(symbol, *halted, *ts, reason, covered, prev_close, now)
            }
            _ => Vec::new(),
        }
    }

    /// Whether a halt or resume of `symbol` is notified ([`halts`], "Coverage"): a per-stock halt
    /// rule, an open live chart, or today's Runners board.
    fn halt_covered(&self, symbol: &str) -> bool {
        self.eval
            .rules()
            .any(|r| r.symbol == symbol && r.condition.is_halt())
            || self.shared.charted(symbol)
            || self.market.on_runners_board(symbol)
    }

    /// One Runners notice: the push to the phones that opted in, within the limits
    /// ([`runners`]). Logged either way; never touches Activity (that is the price alerts').
    /// A halt notice ([`halts`]) takes the same path; its phones are grouped by their sound.
    ///
    /// A Signals crossing first waits 5 s for rw-mp5-1's B0 dedup ([`crate::mp5_gate`]); every
    /// notice of a B0 kind is remembered for that window on its way through.
    fn runner(&mut self, n: crate::rocket::Notice, now: i64) {
        self.release_mp5(now);
        if self.mp5.observe(&n, now.saturating_mul(1_000_000)) {
            return;
        }
        self.deliver(n, now, None, None);
    }

    /// rw-mp5-1 (`crate::mp5_gate` module docs): every parked Signals crossing whose 5 s window
    /// closed by `now` is decided (switch, then the B0 dedup, then G9) and delivered or held.
    /// The gate's facts are ledgered with both decisions whatever the outcome.
    fn release_mp5(&mut self, now: i64) {
        use crate::mp5_gate::Outcome;
        for ((n, a_ns), dup) in self.mp5.release(now.saturating_mul(1_000_000)) {
            let (v, t0) = self.market.mp5_verdict(&n.symbol, n.price, a_ns);
            let outcome = if !self.detectors.on(&detectors::RW_MP5_1) {
                Outcome::SwitchedOff
            } else if dup {
                Outcome::Dedup
            } else if v.why.pass() {
                Outcome::Deliver(v)
            } else {
                Outcome::Gate(v)
            };
            let hold = match outcome {
                Outcome::Deliver(_) => None,
                Outcome::Dedup => Some(runners::Hold::Mp5Dedup),
                Outcome::Gate(_) => Some(runners::Hold::Mp5Gate),
                Outcome::SwitchedOff => Some(runners::Hold::SwitchedOff),
            };
            let mut extra = crate::mp5_gate::ledger_fields(&outcome, a_ns, t0);
            if matches!(outcome, Outcome::Dedup | Outcome::SwitchedOff) {
                // The gate's facts anyway, for the report (what G9 would have said).
                extra["gate_facts"] =
                    crate::mp5_gate::ledger_fields(&Outcome::Gate(v), a_ns, t0)["gate"].clone();
            }
            tracing::info!(symbol = %n.symbol, why = v.why.code(), dup, held = hold.map(|h| h.code()), "rw-mp5-1: signals crossing decided");
            self.deliver(n, now, hold, Some(extra));
        }
    }

    /// [`Task::runner`]'s delivery: `pre` holds the notice on both paths before any limit (a
    /// detector's own verdict, still ledgered with the device counts), `extra` adds fields to
    /// both ledger lines.
    fn deliver(
        &mut self,
        n: crate::rocket::Notice,
        now: i64,
        pre: Option<runners::Hold>,
        extra: Option<Value>,
    ) {
        let now_et = et(now);
        // Desktop and web first (runners::desk_frame): the same limits, decided once here with
        // the open clients that want this kind as the "devices", and their own count, so the
        // phones' switches and the desks' never hold each other back.
        let event_id = n.event_id();
        // Rendered once: the ledger keeps the text with both decisions, the phones get it.
        let (title, body, data) = runners::message(&n);
        // A detector switched off (detectors, "What off does") is held on both paths before any
        // other limit, still ledgered, and not marked sent; a detector's own verdict (`pre`,
        // rw-mp5-1's gate) is kept when it is already a hold.
        let pre = pre.or_else(|| {
            self.detectors
                .holds(n.kind)
                .then_some(runners::Hold::SwitchedOff)
        });
        let desks = self.shared.desks_wanting(&n);
        // "Hide repeat alerts" (runners, "Limits" 4; default off): every notice but an exchange
        // halt or resumption counts its stock.
        let repeat_of = (!matches!(
            n.kind,
            crate::rocket::NoticeKind::Halted | crate::rocket::NoticeKind::Resumed
        ))
        .then_some(n.symbol.as_str());
        // "Group repeat alerts" (runners, "Limits" 4a; default on): the notice's family.
        let family = runners::group_family(n.kind);
        let desk = match pre {
            Some(h) => runners::Decision::Held(h),
            None => self.desk_runners.decide_for(
                &event_id,
                repeat_of,
                family,
                Some(n.price),
                desks,
                &self.prefs,
                n.at_ns.div_euclid(1_000_000),
                now,
                now_et.date_naive(),
                now_et.hour() * 60 + now_et.minute(),
            ),
        };
        self.ledger.decision_ext(
            &n,
            &event_id,
            "desk",
            &desk,
            desks,
            &title,
            &body,
            now,
            extra.as_ref(),
        );
        match desk {
            runners::Decision::Send => {
                let count = self.desk_runners.count_for(&n.symbol);
                self.broadcast(runners::desk_frame_counted(&n, count));
            }
            // Folded into the stock's card: a silent update, never a notification.
            runners::Decision::Held(runners::Hold::Grouped) => {
                let count = self.desk_runners.count_for(&n.symbol);
                self.broadcast(runners::grouped_frame(&n, count));
            }
            runners::Decision::Held(h) => {
                tracing::debug!(symbol = %n.symbol, kind = n.kind.as_str(), held = h.code(), "runners desk notice: held back");
            }
        }
        // Every phone that wants this kind (runners, "Every distinct event is delivered").
        let tokens = self.devices.runner_tokens(&n);
        let phone = match pre {
            Some(h) => runners::Decision::Held(h),
            None => self.runners.decide_for(
                &event_id,
                repeat_of,
                family,
                Some(n.price),
                tokens.len(),
                &self.prefs,
                n.at_ns.div_euclid(1_000_000),
                now,
                now_et.date_naive(),
                now_et.hour() * 60 + now_et.minute(),
            ),
        };
        let kind = n.kind.as_str();
        self.ledger.decision_ext(
            &n,
            &event_id,
            "phone",
            &phone,
            tokens.len(),
            &title,
            &body,
            now,
            extra.as_ref(),
        );
        if let runners::Decision::Held(h) = phone {
            tracing::info!(symbol = %n.symbol, kind, held = h.code(), "runners push: held back");
            return;
        }
        let job = |tokens: Vec<String>,
                   data: Value,
                   channel: Option<&'static str>,
                   sound: Option<Option<&'static str>>| Job {
            // Not an Activity event: a failure later is only logged (history.update finds nothing).
            event_id: event_id.clone(),
            tokens,
            title: title.clone(),
            body: body.clone(),
            data,
            // Sent at once, latency logged (runners, "Latency").
            runner: Some(push::RunnerLatency {
                symbol: n.symbol.clone(),
                kind,
                notice: runners::data_kind(n.kind),
                exec_ns: n.exec_ns,
                decided_ns: n.at_ns,
                queued_ns: now.saturating_mul(1_000_000),
            }),
            channel,
            sound,
        };
        // A halt notice: one job per sound, each phone's own key in the data (and, on a phone
        // with the halt sound channels, its channel and iOS file: push::halt_channel).
        let jobs: Vec<Job> = if matches!(
            n.kind,
            crate::rocket::NoticeKind::Halted | crate::rocket::NoticeKind::Resumed
        ) {
            self.devices
                .halt_groups(&n)
                .into_iter()
                .map(|(key, channels, tokens)| {
                    let mut d = data.clone();
                    d["sound"] = Value::from(key);
                    job(
                        tokens,
                        d,
                        channels.then(|| push::halt_channel(key)),
                        channels.then(|| push::halt_ios_sound(key)),
                    )
                })
                .collect()
        } else {
            vec![job(tokens, data.clone(), None, None)]
        };
        for job in jobs {
            if self.pusher.try_send(job).is_err() {
                tracing::warn!(symbol = %n.symbol, kind, "runners push: push queue full or closed; not sent");
            } else {
                tracing::info!(symbol = %n.symbol, kind, "runners push: sent to the push service");
            }
        }
    }

    /// A scored b-good-1 tag (module docs, "Confidence"): ledgered; a tagged one with its switch
    /// on is shown on every client and marks its Runners row.
    fn confidence(&mut self, c: crate::bgood::Confidence, now: i64) {
        let on = self.detectors.on(&detectors::B_GOOD_1);
        self.ledger.confidence(&c, on, now);
        if !(on && c.tagged) {
            return;
        }
        self.market.set_runner_confidence(&c.alert_id);
        self.broadcast(runners::confidence_frame(&c));
        tracing::info!(symbol = %c.symbol, kind = c.kind.as_str(), margin = c.margin, "b-good-1: high confidence");
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
                tone,
            } => {
                let (app, wants_phone, sound) =
                    self.eval.rule(&alert_id).map_or((true, false, false), |r| {
                        (r.notify_app, r.notify_phone, r.sound)
                    });
                // A sounding drawing alert rings on its tone's channel; muted (the alert's chip
                // or the global switch), it is an ordinary alerts notification.
                let channel = tone
                    .filter(|_| sound && self.prefs.sound_on)
                    .map(drawing_channel);
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
                    tone: tone.map(|t| t.as_str().to_owned()),
                };
                self.ledger.alert(
                    &id,
                    &alert_id,
                    &symbol,
                    price,
                    &text,
                    phone_word,
                    event.reason.as_deref(),
                    self.devices.len(),
                    now,
                );
                if let Phone::Send { extra } = phone {
                    let body = if extra > 0 {
                        format!("{text} · +{extra} more since the last one")
                    } else {
                        text.clone()
                    };
                    let job = |tokens: Vec<String>, channel: Option<&'static str>| Job {
                        event_id: id.clone(),
                        tokens,
                        title: symbol.clone(),
                        body: body.clone(),
                        // The app opens its Alerts tab on a tap (apps/mobile src/notifications).
                        data: json!({ "alertId": alert_id, "symbol": symbol, "eventId": id, "url": "stockwatcher://alerts", "tone": tone.map(|t| t.as_str()) }),
                        runner: None,
                        channel,
                        sound: None,
                    };
                    let jobs = phone_jobs(&self.devices, channel, job);
                    if jobs.into_iter().any(|j| self.pusher.try_send(j).is_err()) {
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
                    tone: None,
                };
                self.broadcast(event_frame("alert.held", &event, None));
                self.history.push(event);
            }
        }
    }
}

/// The synced namespace prefix of one symbol's drawings (chart-core syncedSettings.ts `drawingsNs`).
pub const DRAWINGS_PREFIX: &str = "drawings:";

/// The symbol whose drawings a `settings.changed` frame is about, if it is one (a prefix test
/// first, so the other namespaces' frames cost nothing).
fn changed_drawings(frame: &str) -> Option<String> {
    const HEAD: &str = "{\"type\":\"settings.changed\",\"ns\":\"drawings:";
    let rest = frame.strip_prefix(HEAD)?;
    let end = rest.find('"')?;
    model::parse_symbol(Some(&Value::from(&rest[..end])))
}

/// One alert's phone pushes: a sounding drawing alert's tone channel only for the phones that
/// reported having it (push::Device::drawing_tones), the plain alerts channel for the rest, so a
/// phone on an older APK never gets a channel it does not have. One job per channel that has
/// phones (both carry the same event id: a failure on either marks the event).
fn phone_jobs(
    devices: &Devices,
    channel: Option<&'static str>,
    job: impl Fn(Vec<String>, Option<&'static str>) -> Job,
) -> Vec<Job> {
    let Some(ch) = channel else {
        return vec![job(devices.tokens(), None)];
    };
    let (with, without) = devices.tokens_by_drawing_tones();
    let mut out = Vec::new();
    if !with.is_empty() {
        out.push(job(with, Some(ch)));
    }
    if !without.is_empty() {
        out.push(job(without, None));
    }
    out
}

/// The Android channel a sounding drawing alert's push goes out on (push::Job `channel`). The
/// `-v1` suffix is the tone's version: a channel's sound is fixed when a phone creates it, so a
/// new tone needs new ids here and in apps/mobile src/notifications/setup.ts DRAWING_CHANNELS.
fn drawing_channel(tone: eval::Tone) -> &'static str {
    match tone {
        eval::Tone::Up => "drawing-up-v1",
        eval::Tone::Down => "drawing-down-v1",
        eval::Tone::Stop => "drawing-stop-v1",
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
            ledger::Ledger::default(),
            None,
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
            spread_bp: None,
            no_quote: false,
            kind,
            alert_id: format!("2026-10-22-{symbol}-1"),
            symbol: symbol.into(),
            at_ns: at_ms * 1_000_000,
            price: 2.5,
            from_flag_pct: Some(0.0),
            reasons: vec!["volume 9x pace".into()],
            since_start_min: None,
            exec_ns: None,
            detail: None,
        }
    }

    /// The runtime detector switch (`detectors`, [`detectors`]) through the real task: an
    /// x-float-1 notice goes out on both paths; once `detectors."x-float-1".on` is false (a
    /// settings patch, no restart) the next is held `switched_off` on both, ledgered, and not
    /// marked sent; switched back on, the next goes out. Another kind is never touched.
    #[tokio::test]
    async fn a_detector_switched_off_at_runtime_is_held_and_ledgered() {
        let settings = Settings::in_memory();
        let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let (job_tx, mut jobs) = mpsc::channel(16);
        let (_res_tx, res_rx) = mpsc::channel(4);
        let today = et(now_ms()).date_naive();
        let feed = crate::notices::NoticeFeed::open(None, today);
        let alerts = Alerts::start_with(
            settings.clone(),
            live.clone(),
            None,
            job_tx,
            res_rx,
            ledger::Ledger::default().with_feed(feed.clone()),
        );
        let mut frames = alerts.subscribe();
        let sink = live.alert_sink().expect("attached").clone();
        let next_notice = async |frames: &mut broadcast::Receiver<Arc<str>>| loop {
            match tokio::time::timeout(Duration::from_millis(400), frames.recv()).await {
                Ok(Ok(f)) if f.contains("\"runners.notice\"") => {
                    break Some(serde_json::from_str::<Value>(&f).unwrap());
                }
                Ok(Ok(_)) => continue,
                _ => break None,
            }
        };
        let _desk = alerts.desk(DeskKinds::default());
        assert!(alerts.register_device(
            "ExponentPushToken[fold]",
            "android",
            "Fold",
            RunnersOptIn::default()
        ));
        // The register goes to the task as a command, the notice below on its own channel, and
        // the task's select! takes whichever is ready: on a slow runner both are queued and the
        // notice can win, find no phone and be held `no_device` (CI failed here twice on
        // 2026-11-23). The alerts.status frame says the task has the device.
        loop {
            let f = tokio::time::timeout(Duration::from_secs(5), frames.recv())
                .await
                .unwrap()
                .unwrap();
            if f.starts_with("{\"type\":\"alerts.status\"") {
                break;
            }
        }
        let notice = |kind: crate::rocket::NoticeKind, sym: &str| crate::rocket::Notice {
            at_ns: now_ms() * 1_000_000,
            ..crate::rocket::Notice::test(kind, sym)
        };
        let set = async |on: bool, op: &str| {
            settings
                .apply(
                    detectors::NS,
                    op,
                    SettingsOp::Patch {
                        set: json!({"x-float-1": {"on": on}})
                            .as_object()
                            .unwrap()
                            .clone(),
                        remove: vec![],
                    },
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
        };
        use crate::rocket::NoticeKind as K;
        sink.offer_runner(notice(K::XFloat, "AAAA"));
        let f = next_notice(&mut frames).await.expect("on by default");
        assert_eq!(f["kind"], "v2.xfloat");
        let j = tokio::time::timeout(Duration::from_secs(5), jobs.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(j.data["kind"], "v2.xfloat");

        set(false, "d1").await;
        sink.offer_runner(notice(K::XFloat, "BBBB"));
        assert!(next_notice(&mut frames).await.is_none(), "held on the desk");
        assert!(jobs.try_recv().is_err(), "held on the phone");
        // Other kinds are not this switch's.
        sink.offer_runner(notice(K::N1, "CCCC"));
        assert_eq!(
            next_notice(&mut frames).await.expect("n1")["kind"],
            "shadow.n1"
        );
        let (items, _, _) = feed.items(today);
        let held = items
            .iter()
            .find(|i| i["symbol"] == "BBBB")
            .expect("the held x-float-1 is in the log");
        assert!(held.to_string().contains("switched_off"), "{held}");

        set(true, "d2").await;
        sink.offer_runner(notice(K::XFloat, "DDDD"));
        assert_eq!(
            next_notice(&mut frames).await.expect("on again")["data"]["symbol"],
            "DDDD"
        );
    }

    /// Exchange halts through the real task (plan "halt sounds", 2026-11-16): a covered stock's
    /// halt and resume are exactly one Halted and one Resumed on every surface; an uncovered stock
    /// gets nothing; a replayed status is a duplicate; quiet hours hold (and log) one; a phone's
    /// own sound rides in its push's data, on the plain channel with the default sound until its
    /// app has the halt channels.
    #[tokio::test]
    async fn halt_notices_follow_coverage_and_the_runners_limits() {
        let settings = Settings::in_memory();
        let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let (job_tx, mut jobs) = mpsc::channel(16);
        let (_res_tx, res_rx) = mpsc::channel(4);
        let today = et(now_ms()).date_naive();
        let feed = crate::notices::NoticeFeed::open(None, today);
        let alerts = Alerts::start_with(
            settings.clone(),
            live.clone(),
            None,
            job_tx,
            res_rx,
            ledger::Ledger::default().with_feed(feed.clone()),
        );
        let mut frames = alerts.subscribe();
        let sink = live.alert_sink().expect("attached").clone();
        let next_notice = async |frames: &mut broadcast::Receiver<Arc<str>>| loop {
            match tokio::time::timeout(Duration::from_millis(400), frames.recv()).await {
                Ok(Ok(f)) if f.contains("\"runners.notice\"") => {
                    break Some(serde_json::from_str::<Value>(&f).unwrap());
                }
                Ok(Ok(_)) => continue,
                _ => break None,
            }
        };
        let _desk = alerts.desk(DeskKinds::default());
        assert!(alerts.register_device(
            "ExponentPushToken[fold]",
            "android",
            "Fold",
            RunnersOptIn::default()
        ));
        let tab = push::MoreOptIn {
            halted_sound: Some(push::SoundKey("klaxon")),
            ..Default::default()
        };
        assert!(alerts.register_device(
            "ExponentPushToken[tab]",
            "android",
            "Tab",
            tab.with(RunnersOptIn::default())
        ));
        // Each register answers with an alerts.status frame once the task has it.
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
        let charts = alerts.charts();
        charts.set(["XLAB"]);
        let now = now_ms();
        let sess = Some((et(now).date_naive(), market_events::SessionKind::Regular));
        let trade = |s: &str, price: f64, ts: i64| Input::Trade {
            symbol: s.into(),
            price,
            ts,
            session: sess,
        };
        let status = |s: &str, halted: bool, ts: i64, reason: &str| Input::Status {
            symbol: s.into(),
            halted,
            ts,
            session: sess,
            reason: reason.into(),
        };
        let halt_at = now - 125_000;
        // An uncovered stock: tracked, never notified.
        sink.offer(trade("NOPE", 4.0, halt_at - 1_000));
        sink.offer(status("NOPE", true, halt_at, "T1"));
        // The charted one.
        sink.offer(trade("XLAB", 2.76, halt_at - 1_000));
        sink.offer(status("XLAB", true, halt_at, "LUDP"));
        let f = next_notice(&mut frames).await.expect("a desk notice");
        assert_eq!(f["kind"], "halt.halted");
        assert_eq!(f["title"], "XLAB · Halted · $2.76");
        assert_eq!(f["body"], "LULD pause");
        assert_eq!(f["data"]["sound"], "siren");
        let mut got = Vec::new();
        for _ in 0..2 {
            got.push(
                tokio::time::timeout(Duration::from_secs(5), jobs.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        got.sort_by(|a, b| a.tokens.cmp(&b.tokens));
        assert_eq!(got[0].tokens, vec!["ExponentPushToken[fold]".to_string()]);
        assert_eq!(got[0].data["sound"], "siren");
        assert_eq!(got[1].tokens, vec!["ExponentPushToken[tab]".to_string()]);
        assert_eq!(got[1].data["sound"], "klaxon");
        for j in &got {
            assert_eq!(j.title, "XLAB · Halted · $2.76");
            assert_eq!(j.data["kind"], "halt.halted");
            // Release 1: no phone has the halt channels, so the plain channel and default sound.
            assert_eq!((j.channel, j.sound), (None, None));
        }
        // The stream reconnects and replays the status: not news.
        sink.offer(status("XLAB", true, halt_at, "LUDP"));
        assert!(next_notice(&mut frames).await.is_none());
        assert!(jobs.try_recv().is_err());
        // The resume waits for its reopening print.
        sink.offer(status("NOPE", false, now - 5_000, ""));
        sink.offer(status("XLAB", false, now - 5_000, ""));
        assert!(next_notice(&mut frames).await.is_none());
        sink.offer(trade("NOPE", 4.4, now - 4_000));
        sink.offer(trade("XLAB", 3.10, now - 4_000));
        let f = next_notice(&mut frames).await.expect("the resume");
        assert_eq!(f["kind"], "halt.resumed");
        assert_eq!(f["title"], "XLAB · Resumed · $3.10");
        assert_eq!(f["body"], "Halted 2 min · +12.3% from $2.76");
        assert_eq!(f["data"]["sound"], "chime");
        // Both phones keep the default Resumed sound: one push to both.
        let j = tokio::time::timeout(Duration::from_secs(5), jobs.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(j.data["kind"], "halt.resumed");
        assert_eq!(j.tokens.len(), 2);
        // Exactly one of each: nothing for NOPE, nothing more for XLAB.
        assert!(next_notice(&mut frames).await.is_none());
        assert!(jobs.try_recv().is_err());

        // Quiet hours (ET wall time) around now: held on both paths, logged, nothing sent.
        let m = {
            let t = et(now_ms());
            t.hour() * 60 + t.minute()
        };
        let hm = |m: u32| format!("{:02}:{:02}", (m % 1440) / 60, m % 60);
        settings
            .apply(
                PREFS_NS,
                "q1",
                SettingsOp::Patch {
                    set:
                        json!({"quietHours": {"on": true, "start": hm(m + 1439), "end": hm(m + 3)}})
                            .as_object()
                            .unwrap()
                            .clone(),
                    remove: vec![],
                },
            )
            .await
            .unwrap();
        // A per-stock halt rule covers its symbol too.
        settings
            .apply(RULES_NS, "r1", SettingsOp::Patch {
                set: json!({"h1": {"symbol": "RULE", "rule": {"kind": "halt"}, "fire": "once", "sessions": ["pre", "regular", "after", "overnight"], "expiry": "never", "createdAt": now, "editedAt": now}}).as_object().unwrap().clone(),
                remove: vec![],
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sink.offer(trade("RULE", 1.2, now_ms() - 2_000));
        sink.offer(status("RULE", true, now_ms() - 1_000, "T1"));
        assert!(next_notice(&mut frames).await.is_none());
        assert!(jobs.try_recv().is_err());
        let (items, _, _) = feed.items(today);
        let rule = items
            .iter()
            .find(|i| i["symbol"] == "RULE")
            .expect("the held halt is in the log");
        assert_eq!(rule["notice"], "halt.halted");
        assert!(
            rule.to_string().contains("quiet_hours"),
            "held for quiet hours: {rule}"
        );
        drop(charts);
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
        let alerts = Alerts::start_with(
            settings.clone(),
            live.clone(),
            None,
            job_tx,
            res_rx,
            ledger::Ledger::default(),
        );
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
                ..RunnersOptIn::default()
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
        // A breakout of the same stock right after: sent to every phone that wants breakouts
        // (only the tablet), and a Run +20% a moment later to every phone that wants that (both
        // default on): no per-stock gap, no merge (runners, "Every distinct event is delivered").
        sink.offer_runner(notice(NoticeKind::Breakout, "CJMB", now_ms()));
        let j = tokio::time::timeout(Duration::from_secs(5), jobs.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(j.data["symbol"], "CJMB");
        assert_eq!(j.tokens, vec!["ExponentPushToken[tab]".to_string()]);
        // The Run +20% notice is cut by default (detectors, `run`): held, nothing queued. Switched
        // back on from Settings (the synced `detectors` namespace), the next one goes out.
        sink.offer_runner(notice(NoticeKind::Run, "CJMB", now_ms()));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(next_job(&mut jobs).is_none(), "the run cut holds it");
        settings
            .apply(
                detectors::NS,
                "d1",
                SettingsOp::Patch {
                    set: serde_json::from_value(json!({"run": {"on": true}})).unwrap(),
                    remove: Vec::new(),
                },
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut run = notice(NoticeKind::Run, "CJMB", now_ms());
        run.alert_id = "2026-10-22-CJMB-2".into();
        sink.offer_runner(run);
        let j = tokio::time::timeout(Duration::from_secs(5), jobs.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(j.data["kind"], "runners.run");
        let mut t = j.tokens.clone();
        t.sort();
        assert_eq!(t, vec!["ExponentPushToken[fold]", "ExponentPushToken[tab]"]);
        // Another stock's breakout: only the tablet opted in.
        sink.offer_runner(notice(NoticeKind::Breakout, "BBB", now_ms()));
        let j = tokio::time::timeout(Duration::from_secs(5), jobs.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(j.data["symbol"], "BBB");
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

    /// The v2 deploy plan's runtime switch (`crate::gap_ctx`, DETECTORS_NS): switching x-gap-1
    /// off in the synced settings holds its next notice for every phone at once, with no restart,
    /// and ledgers it as `switched_off`; x-ctx-1 is untouched; switching it back on sends again.
    #[tokio::test]
    async fn a_detector_switched_off_in_settings_is_held_for_everyone_at_once() {
        use crate::rocket::NoticeKind;
        let settings = Settings::in_memory();
        let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let (job_tx, mut jobs) = mpsc::channel(16);
        let (_res_tx, res_rx) = mpsc::channel(4);
        let today = et(now_ms()).date_naive();
        let feed = crate::notices::NoticeFeed::open(None, today);
        let alerts = Alerts::start_with(
            settings.clone(),
            live.clone(),
            None,
            job_tx,
            res_rx,
            ledger::Ledger::default().with_feed(feed.clone()),
        );
        let mut frames = alerts.subscribe();
        let sink = live.alert_sink().expect("attached").clone();
        assert!(alerts.register_device(
            "ExponentPushToken[fold]",
            "android",
            "Fold",
            RunnersOptIn::default()
        ));
        loop {
            let f = tokio::time::timeout(Duration::from_secs(5), frames.recv())
                .await
                .unwrap()
                .unwrap();
            if f.starts_with("{\"type\":\"alerts.status\"") {
                break;
            }
        }
        let job = async |jobs: &mut mpsc::Receiver<Job>| {
            tokio::time::timeout(Duration::from_secs(5), jobs.recv())
                .await
                .unwrap()
                .unwrap()
        };
        sink.offer_runner(notice(NoticeKind::XGap, "GAPA", now_ms()));
        let j = job(&mut jobs).await;
        assert_eq!(
            (j.data["kind"].as_str(), j.data["symbol"].as_str()),
            (Some("v2.xgap"), Some("GAPA"))
        );
        let set = |v: Value| SettingsOp::Set {
            value: v,
            if_absent: false,
        };
        settings
            .apply(DETECTORS_NS, "d1", set(json!({"x-gap-1": {"on": false}})))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sink.offer_runner(notice(NoticeKind::XGap, "GAPB", now_ms()));
        sink.offer_runner(notice(NoticeKind::XCtx, "CTXA", now_ms()));
        let j = job(&mut jobs).await;
        assert_eq!(
            j.data["symbol"], "CTXA",
            "x-ctx-1 is still on; GAPB was held"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(jobs.try_recv().is_err());
        let (items, _, _) = feed.items(today);
        let held = items
            .iter()
            .find(|i| i["symbol"] == "GAPB")
            .expect("the held notice is in the log");
        assert!(
            held.to_string().contains("switched_off") || held.to_string().contains("switched off"),
            "{held}"
        );
        settings
            .apply(DETECTORS_NS, "d2", set(json!({"x-gap-1": {"on": true}})))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sink.offer_runner(notice(NoticeKind::XGap, "GAPC", now_ms()));
        assert_eq!(job(&mut jobs).await.data["symbol"], "GAPC");
    }

    /// Desktop and web notices (runners::desk_frame) through the real task: none while no client
    /// listens, the phone's text once one does, every distinct event on a stock, its switches
    /// respected, and a closed socket's guard takes it off the list.
    #[tokio::test]
    async fn desk_notices_follow_the_open_clients_and_limits() {
        use crate::rocket::NoticeKind;
        let settings = Settings::in_memory();
        let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let (job_tx, _jobs) = mpsc::channel(16);
        let (_res_tx, res_rx) = mpsc::channel(4);
        let alerts = Alerts::start_with(
            settings,
            live.clone(),
            None,
            job_tx,
            res_rx,
            ledger::Ledger::default(),
        );
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
        // Nobody listening: held (no_device).
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
        // The same stock a moment later: its own notification, nothing merged or replaced. (A
        // Run +20% notice is cut by default, detectors `run`; a burst stands in for it here.)
        sink.offer_runner(notice(NoticeKind::Run, "CJMB", now_ms()));
        assert!(next_notice(&mut frames).await.is_none(), "the run cut");
        // Only the `run` kind is cut: a follow-up whose new (or first) path is Run still goes out
        // (sc2-1 was leave-one-KIND-out; the board's Run row still makes later paths follow-ups).
        let mut fu = notice(NoticeKind::FollowUp, "CJMB", now_ms());
        fu.detail = Some(crate::rocket::NoticeDetail::FollowUp {
            path: crate::rocket::AlertKind::Run,
            first: crate::rocket::AlertKind::Run,
            first_at_ns: now_ms() * 1_000_000 - 60_000_000_000,
        });
        sink.offer_runner(fu);
        let f = next_notice(&mut frames).await.expect("the run follow-up");
        assert_eq!(f["kind"], "runners.followup");
        sink.offer_runner(notice(NoticeKind::Burst, "CJMB", now_ms()));
        let f = next_notice(&mut frames).await.expect("the burst");
        assert_eq!(f["kind"], "runners.burst");
        assert!(f.get("mergeInto").is_none(), "{f}");
        // Breakouts are off by default.
        sink.offer_runner(notice(NoticeKind::Breakout, "BBB", now_ms()));
        assert!(next_notice(&mut frames).await.is_none());
        desk.set(DeskKinds {
            breakout: true,
            ..DeskKinds::from_opt_in(RunnersOptIn::default())
        });
        sink.offer_runner(notice(NoticeKind::Breakout, "BBB", now_ms()));
        let f = next_notice(&mut frames).await.expect("a breakout");
        assert_eq!(f["kind"], "runners.breakout");
        // The panel kinds: a Ross crossing on both panels reaches a desk whose Ross switch is
        // off but Signals on, once; the Halt watcher stays off by default.
        desk.set(DeskKinds {
            ross_crossing: false,
            ..DeskKinds::default()
        });
        let mut x = notice(NoticeKind::RossCrossing, "XING", now_ms());
        x.detail = Some(crate::rocket::NoticeDetail::Cue(crate::rocket::CueNotice {
            episode_id: "ep".into(),
            trigger: Some(2.4),
            also_signals: true,
        }));
        sink.offer_runner(x.clone());
        let f = next_notice(&mut frames).await.expect("a crossing");
        assert_eq!(f["kind"], "ross.crossing");
        assert_eq!(f["alsoSignals"], true);
        assert_eq!(f["id"], x.event_id());
        sink.offer_runner(x);
        sink.offer_runner(notice(NoticeKind::Halt, "HALT", now_ms()));
        assert!(next_notice(&mut frames).await.is_none(), "once; Halt off");
        // The socket ended: nobody listens again.
        drop(desk);
        sink.offer_runner(notice(NoticeKind::Fast, "FFF", now_ms()));
        assert!(next_notice(&mut frames).await.is_none());
    }

    /// rw-mp5-1 (`crate::mp5_gate`) through the real task: a B0 kind goes out at once, a Signals
    /// crossing waits out the 5 s window and is then held (here `mp5_gate`: the market's inputs are
    /// off, so the day is not watched whole), both decisions ledgered with the gate's facts; with
    /// `detectors` → `rw-mp5-1` off it is held `switched_off`, read live from the settings.
    #[tokio::test]
    async fn signals_crossings_wait_for_the_gate_and_follow_the_detector_switch() {
        use crate::rocket::NoticeKind;
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::in_memory();
        let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let (job_tx, _jobs) = mpsc::channel(16);
        let (_res_tx, res_rx) = mpsc::channel(4);
        let alerts = Alerts::start_with(
            settings.clone(),
            live.clone(),
            None,
            job_tx,
            res_rx,
            ledger::Ledger::start(dir.path()),
        );
        let mut frames = alerts.subscribe();
        let sink = live.alert_sink().expect("attached").clone();
        let _desk = alerts.desk(DeskKinds::from_opt_in(RunnersOptIn::default()));
        let next_notice = async |frames: &mut broadcast::Receiver<Arc<str>>, ms: u64| loop {
            match tokio::time::timeout(Duration::from_millis(ms), frames.recv()).await {
                Ok(Ok(f)) if f.contains("\"runners.notice\"") => {
                    break Some(serde_json::from_str::<Value>(&f).unwrap());
                }
                Ok(Ok(_)) => continue,
                _ => break None,
            }
        };
        let ledger_lines = |sym: &str| -> Vec<Value> {
            let Ok(rd) = std::fs::read_dir(dir.path().join(crate::oplog::NOTICES_DIR)) else {
                return Vec::new();
            };
            rd.filter_map(|e| e.ok())
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .flat_map(|t| {
                    t.lines()
                        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                        .collect::<Vec<_>>()
                })
                .filter(|v| v["symbol"] == sym && v["kind"] == "decision")
                .collect()
        };
        let wait_lines = async |sym: &str| {
            for _ in 0..100 {
                let v = ledger_lines(sym);
                if v.len() >= 2 {
                    return v;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("no ledger lines for {sym}");
        };
        sink.offer_runner(notice(NoticeKind::SignalsCrossing, "SIG", now_ms()));
        sink.offer_runner(notice(NoticeKind::New, "NEWS", now_ms()));
        let f = next_notice(&mut frames, 2000)
            .await
            .expect("the B0 kind at once");
        assert_eq!(f["kind"], "runners.new");
        assert!(
            next_notice(&mut frames, 3000).await.is_none(),
            "the crossing waits 5 s"
        );
        let lines = wait_lines("SIG").await;
        for l in &lines {
            assert_eq!(l["decision"], "held", "{l}");
            assert_eq!(l["held"], "mp5_gate", "{l}");
            assert_eq!(l["detector"], "rw-mp5-1");
            assert_eq!(l["gate"]["why"], "incomplete");
            assert!(l["decided_ns"].as_i64().is_some());
        }
        assert!(next_notice(&mut frames, 300).await.is_none(), "never sent");
        // Switched off from any device, no restart: held switched_off, still ledgered.
        settings
            .apply(
                DETECTORS_NS,
                "t-off",
                crate::settings::SettingsOp::Set {
                    value: json!({"rw-mp5-1": {"on": false}}),
                    if_absent: false,
                },
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        sink.offer_runner(notice(NoticeKind::SignalsCrossing, "OFF", now_ms()));
        let lines = wait_lines("OFF").await;
        for l in &lines {
            assert_eq!(l["held"], "switched_off", "{l}");
            assert_eq!(l["gate"]["why"], "switched_off");
            assert_eq!(l["gate_facts"]["why"], "incomplete");
        }
        // Other kinds are untouched by the switch.
        sink.offer_runner(notice(NoticeKind::Fast, "FAST", now_ms()));
        let f = next_notice(&mut frames, 2000).await.expect("a fast notice");
        assert_eq!(f["kind"], "runners.fast");
    }

    /// A drawing alert through the real task: the drawing is read from its synced namespace,
    /// a touch fires with its tone (and a sounding phone push would ring on the tone's channel),
    /// and deleting the drawing on some device orphans the alert within the settings feed.
    #[tokio::test]
    async fn a_drawing_alert_follows_its_synced_drawing() {
        let settings = Settings::in_memory();
        let live = Market::idle(market_providers::ClockMode::Live, "idle", None);
        let (job_tx, _jobs) = mpsc::channel(16);
        let (_res_tx, res_rx) = mpsc::channel(4);
        let alerts = Alerts::start_with(
            settings.clone(),
            live.clone(),
            None,
            job_tx,
            res_rx,
            ledger::Ledger::default(),
        );
        let mut frames = alerts.subscribe();
        let now = now_ms();
        settings
            .apply("drawings:XLAB", "op0", SettingsOp::Patch {
                set: json!({"h1": {"z": 0, "d": {"id": "h1", "kind": "hline", "a": {"t": now, "p": 1.85}, "b": {"t": now, "p": 1.85}}}}).as_object().unwrap().clone(),
                remove: vec![],
            })
            .await
            .unwrap();
        settings
            .apply(RULES_NS, "op1", SettingsOp::Patch {
                set: json!({"d1": {"symbol": "XLAB", "rule": {"kind": "drawing", "drawingId": "h1", "part": "line", "tol": {"unit": "ticks", "value": 1}, "tf": "1m"}, "fire": "once", "sessions": ["pre", "regular", "after", "overnight"], "expiry": "never", "createdAt": now, "editedAt": now}}).as_object().unwrap().clone(),
                remove: vec![],
            })
            .await
            .unwrap();
        let state =
            async |settings: &Settings| settings.get(STATE_NS).await.map(|(_, v)| v["d1"].clone());
        let mut armed = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if state(&settings)
                .await
                .is_some_and(|s| s["status"] == "armed")
            {
                armed = true;
                break;
            }
        }
        assert!(armed, "state written");
        let t = now_ms();
        let sess = Some((et(t).date_naive(), market_events::SessionKind::After));
        let sink = live.alert_sink().expect("attached");
        for (p, ts) in [(1.90, t - 1000), (1.86, t)] {
            sink.offer(Input::Trade {
                symbol: "XLAB".into(),
                price: p,
                ts,
                session: sess,
            });
        }
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
        assert_eq!(v["event"]["text"], "Touched line 1.85 at 1.86 from above");
        assert_eq!(v["event"]["tone"], "down");
        // Deleted on another device: orphaned.
        settings
            .apply(
                "drawings:XLAB",
                "op2",
                SettingsOp::Patch {
                    set: serde_json::Map::new(),
                    remove: vec!["h1".into()],
                },
            )
            .await
            .unwrap();
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if state(&settings)
                .await
                .is_some_and(|s| s["orphaned"] == true)
            {
                return;
            }
        }
        panic!("not orphaned: {:?}", state(&settings).await);
    }

    #[test]
    fn drawing_namespaces_and_channels() {
        assert_eq!(
            changed_drawings("{\"type\":\"settings.changed\",\"ns\":\"drawings:XLAB\",\"rev\":3}")
                .as_deref(),
            Some("XLAB")
        );
        assert_eq!(
            changed_drawings("{\"type\":\"settings.changed\",\"ns\":\"alerts\"}"),
            None
        );
        assert_eq!(drawing_channel(eval::Tone::Stop), "drawing-stop-v1");
    }

    /// A sounding drawing alert's push: the tone channel only for phones that reported it, the
    /// plain channel for the rest (an older APK, or a record from before the capability).
    #[test]
    fn tone_channels_only_for_phones_that_have_them() {
        let mut d = Devices::in_memory();
        let opt = |tones: Option<bool>| RunnersOptIn {
            drawing_tones: tones,
            ..RunnersOptIn::default()
        };
        assert!(d.register(
            "ExponentPushToken[new]",
            "android",
            "Fold",
            1,
            opt(Some(true))
        ));
        assert!(d.register("ExponentPushToken[old]", "android", "Tab", 1, opt(None)));
        let mk = |tokens: Vec<String>, channel: Option<&'static str>| Job {
            event_id: "e".into(),
            tokens,
            title: "XLAB".into(),
            body: "b".into(),
            data: json!({}),
            runner: None,
            channel,
            sound: None,
        };
        let jobs = phone_jobs(&d, Some("drawing-stop-v1"), mk);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].tokens, vec!["ExponentPushToken[new]".to_string()]);
        assert_eq!(jobs[0].channel, Some("drawing-stop-v1"));
        assert_eq!(jobs[1].tokens, vec!["ExponentPushToken[old]".to_string()]);
        assert_eq!(jobs[1].channel, None, "the plain alerts channel");
        // Muted (no channel): one job, every phone, the plain channel.
        let jobs = phone_jobs(&d, None, mk);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].tokens.len(), 2);
        // A re-register without the field keeps what the phone said; an explicit false clears it.
        assert!(d.register("ExponentPushToken[new]", "android", "Fold", 2, opt(None)));
        assert!(d.get("ExponentPushToken[new]").unwrap().drawing_tones);
        assert!(d.register(
            "ExponentPushToken[new]",
            "android",
            "Fold",
            3,
            opt(Some(false))
        ));
        let jobs = phone_jobs(&d, Some("drawing-up-v1"), mk);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].channel, None);
        // A stored record from before the field reads as without.
        let old: push::Device =
            serde_json::from_value(json!({"token": "ExponentPushToken[x]", "registeredAt": 1}))
                .unwrap();
        assert!(!old.drawing_tones);
        // The wire: `drawingTones` on push.register.
        let more: push::MoreOptIn = serde_json::from_value(json!({"drawingTones": true})).unwrap();
        assert_eq!(more.with(RunnersOptIn::default()).drawing_tones, Some(true));
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
