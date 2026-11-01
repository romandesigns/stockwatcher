//! The notification log on the Insights page (owner, 2026-10-26: "create an insight page where we
//! can display a notification log"): the ledger's day file read back and grouped, one item per
//! notice (`GET /api/notices?day=YYYY-MM-DD`).
//!
//! The ledger (`crate::alerts::ledger`) writes several lines per notice: a `decision` per path
//! (desk, phone), a `pushed` when Expo took it, a `failed` when it refused. The page wants one row
//! per notice, so lines are grouped by `event_id`:
//!
//! - `decision` sets the notice's facts (first seen wins: both paths carry the same) and its
//!   `desk` / `phone` outcome `{decision, held, devices}`;
//! - `alert` (a fired price alert) is an item of its own, notice `price.alert`, with `phone`;
//! - `pushed` adds `latency` (the four latency figures it carries);
//! - `failed` adds `failed` (Expo's detail, already token-redacted by the ledger).
//!
//! Items come newest first, at most [`MAX_ITEMS`] (the newest; `truncated` says so). The file is
//! append-only and capped by its writer (32 MB), and a crash can leave a torn last line, which is
//! skipped like any malformed line. A day without a file (before the ledger existed on
//! 2026-10-26, a day the live server was down) is an empty list, not an error.
//!
//! **Pushed** (`notices.v1`, [`NoticeFeed`]; owner rule: realtime is pushed, clients only
//! render). The open log used to refetch the whole day every 15 s on every client. Now:
//!
//! - client `notices.subscribe {day?}` (absent = today, the server's ET date) -> server
//!   `notices.snapshot {day, days, items, truncated}`: the same body as `GET /api/notices`.
//! - Then, for today, `notices.append {day, items: [item]}` each time the ledger writes a line:
//!   that notice's item regrouped from all of its lines so far (a desk decision, then the phone
//!   one, then `pushed` with its latency), which the client upserts by `event_id` (new ones go
//!   on top). Subscribed to today without a `day`, the subscription follows the date: at ET
//!   midnight ([`NoticeFeed::spawn_rollover`]) it gets the new day's snapshot (empty), so the
//!   log changes day when the date does, not when the next day's first notice happens to come
//!   (on a weekend, days later). A line of the next ET day arriving before the rollover tick
//!   moves it the same way.
//! - A past day gets its snapshot and nothing else (its file no longer changes). A lag resends
//!   the snapshot. `notices.unsubscribe` stops it.
//!
//! The feed keeps today's lines in memory, primed from today's file at start, so a snapshot of
//! today never races the ledger's background writer (a line can be broadcast before it is on
//! disk) and a restart mid-day still regroups an event whose first lines came before it. Bounded
//! like the file: past [`crate::oplog::NOTICES_CAP`] bytes in a day, lines stop being kept (the
//! writer stops writing them too). Every append carries a `seq` taken under the same lock as the
//! snapshot, so a connection forwards only appends newer than its snapshot and an item never goes
//! back to an older state.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use serde_json::{Map, Value, json};
use tokio::sync::broadcast;

/// The feature a client must see in `welcome.features` before it sends `notices.subscribe`.
pub const FEATURE: &str = "notices.v1";

/// Rows assembled here (owner rule: Rust computes, clients only render). With this feature in
/// `welcome.features` every item carries `status` (`sent` | `held`), `reason` (the hold's code or
/// null) and `reason_text` (the same in plain words), and every body (`GET /api/notices`,
/// `notices.snapshot`, each `notices.append`) carries `summary` `{sent, held, reasons: [{reason,
/// text, n}]}` over the WHOLE day (also past [`MAX_ITEMS`]). The fields are additive, so a client
/// that predates them reads the same body as before and keeps deriving them itself
/// (packages/shared noticeLog.ts `noticeStatus` / `noticeSummary`). Filtering the delivered rows
/// by ticker or kind stays on the client: it is presentation over rows already sent, no counting.
pub const FEATURE_ROWS: &str = "notices.rows.v1";

/// Rows returned for one day at most (the newest kept).
pub const MAX_ITEMS: usize = 2_000;

/// The dates that have a ledger file, newest first.
pub fn days(dir: &Path) -> Vec<NaiveDate> {
    let mut out: Vec<NaiveDate> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    // Exactly `<date>.jsonl`: never a partial copy or another log's file.
                    let name = p.file_name()?.to_str()?;
                    (name.len() == 16).then_some(())?;
                    crate::oplog::day_file_date(&p)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_unstable_by(|a, b| b.cmp(a));
    out
}

/// One day's items, grouped and newest first, plus whether older ones were cut.
pub fn read_day(dir: &Path, day: NaiveDate) -> (Vec<Value>, bool) {
    let (items, truncated, _) = read_day_summarized(dir, day);
    (items, truncated)
}

/// [`read_day`] plus the day's [`summary`].
pub fn read_day_summarized(dir: &Path, day: NaiveDate) -> (Vec<Value>, bool, Value) {
    let text = std::fs::read_to_string(day_file(dir, day)).unwrap_or_default();
    group_summarized(&text)
}

/// The ledger file of `day` under `dir`.
pub fn day_file(dir: &Path, day: NaiveDate) -> PathBuf {
    dir.join(format!("{}.jsonl", day.format("%Y-%m-%d")))
}

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

/// Groups ledger lines into items (module docs). Pure, so it is tested without files.
pub fn group(text: &str) -> (Vec<Value>, bool) {
    let (items, truncated, _) = group_summarized(text);
    (items, truncated)
}

/// [`group`] plus the day's [`summary`] over every item, the ones past [`MAX_ITEMS`] included.
pub fn group_summarized(text: &str) -> (Vec<Value>, bool, Value) {
    let mut order: Vec<String> = Vec::new();
    let mut items: HashMap<String, Map<String, Value>> = HashMap::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(id) = v.get("event_id").and_then(Value::as_str) else {
            continue;
        };
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
        let item = items.entry(id.to_string()).or_insert_with(|| {
            order.push(id.to_string());
            let mut m = Map::new();
            m.insert("event_id".into(), json!(id));
            m
        });
        let set_if_absent = |m: &mut Map<String, Value>, k: &str, val: Option<Value>| {
            if let Some(val) = val.filter(|x| !x.is_null())
                && !m.contains_key(k)
            {
                m.insert(k.into(), val);
            }
        };
        match kind {
            "decision" => {
                for k in [
                    "symbol",
                    "price",
                    "notice",
                    "title",
                    "body",
                    "mp5_wick",
                    "breakout",
                    "entry_context",
                    // The event's own clocks (the ticker page measures outcomes from them).
                    "exec_ns",
                    "at_ns",
                ] {
                    set_if_absent(item, k, v.get(k).cloned());
                }
                if let Some(at) = num(&v, "decided_at") {
                    let earlier = item
                        .get("at")
                        .and_then(Value::as_f64)
                        .is_none_or(|a| at < a);
                    if earlier {
                        item.insert("at".into(), json!(at as i64));
                    }
                }
                if let Some(path @ ("desk" | "phone")) = v.get("path").and_then(Value::as_str) {
                    let mut outcome = json!({
                        "decision": v.get("decision").cloned().unwrap_or(Value::Null),
                        "held": v.get("held").cloned().unwrap_or(Value::Null),
                        "devices": v.get("devices").cloned().unwrap_or(Value::Null),
                    });
                    // A merged Runners path (the merge removed 2026-10-30; old days still have it): what it was folded into.
                    for k in ["merged_into", "merged_devices"] {
                        if let Some(x) = v.get(k) {
                            outcome[k] = x.clone();
                        }
                    }
                    item.insert(path.into(), outcome);
                }
            }
            "alert" => {
                item.insert("notice".into(), json!("price.alert"));
                for k in ["symbol", "price"] {
                    set_if_absent(item, k, v.get(k).cloned());
                }
                set_if_absent(item, "title", v.get("text").cloned());
                if let Some(at) = num(&v, "decided_at") {
                    item.insert("at".into(), json!(at as i64));
                }
                item.insert(
                    "phone".into(),
                    json!({
                        "decision": v.get("phone").cloned().unwrap_or(Value::Null),
                        "held": v.get("held").cloned().unwrap_or(Value::Null),
                        "devices": v.get("devices").cloned().unwrap_or(Value::Null),
                    }),
                );
            }
            "pushed" => {
                for k in ["symbol", "notice"] {
                    set_if_absent(item, k, v.get(k).cloned());
                }
                if !item.contains_key("at")
                    && let Some(ns) = num(&v, "expo_ns")
                {
                    item.insert("at".into(), json!((ns / 1e6) as i64));
                }
                let mut lat = Map::new();
                for k in [
                    "trade_to_board_ms",
                    "board_to_queue_ms",
                    "queue_to_expo_ms",
                    "trade_to_expo_ms",
                ] {
                    if let Some(x) = v.get(k).filter(|x| !x.is_null()) {
                        lat.insert(k.into(), x.clone());
                    }
                }
                item.insert("latency".into(), Value::Object(lat));
            }
            "failed" => {
                item.insert(
                    "failed".into(),
                    v.get("detail").cloned().unwrap_or(Value::Null),
                );
                if !item.contains_key("at")
                    && let Some(at) = num(&v, "at")
                {
                    item.insert("at".into(), json!(at as i64));
                }
            }
            _ => {}
        }
    }
    // An item never decided here (a `pushed` or `failed` whose decision is in another day's file,
    // around midnight) still shows, at the time it has.
    let mut out: Vec<(i64, usize, Value)> = order
        .into_iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let m = items.remove(&id)?;
            let at = m.get("at").and_then(Value::as_i64).unwrap_or(0);
            Some((at, i, Value::Object(m)))
        })
        .collect();
    // Newest first; ties keep file order reversed (the later line first).
    out.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut tally = Tally::default();
    for (_, _, v) in &mut out {
        if let Value::Object(m) = v {
            let s = annotate(m);
            tally.add(&s, 1);
        }
    }
    let truncated = out.len() > MAX_ITEMS;
    out.truncate(MAX_ITEMS);
    (
        out.into_iter().map(|(_, _, v)| v).collect(),
        truncated,
        tally.summary(),
    )
}

/// A row's delivery as the page shows it ([`FEATURE_ROWS`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowStatus {
    pub sent: bool,
    /// The hold's code (`no_device`, `quiet_hours`, …; `off` for a price alert the phone had
    /// switched off); `None` when sent, or held without a code.
    pub reason: Option<String>,
}

impl RowStatus {
    /// The summary's bucket: the code, or `held` when there is none; `None` when sent.
    fn bucket(&self) -> Option<&str> {
        (!self.sent).then(|| self.reason.as_deref().unwrap_or("held"))
    }
}

fn decision(p: Option<&Value>) -> Option<&str> {
    p?.get("decision")?.as_str().filter(|d| !d.is_empty())
}

/// The overall status of a notice: the phone's when the phones saw it (the question the log
/// answers is "why didn't my phone buzz?": SRTM's pullback on 2026-10-26 reached the desktop but
/// was held for the phone), else the desk's. The hold reason that matters is the phone's if held
/// (or switched off), else the desk's. Moved here from packages/shared noticeLog.ts
/// (`noticeStatus`, `noticeHeldReason`), which clients without [`FEATURE_ROWS`] still run.
pub fn row_status(item: &Map<String, Value>) -> RowStatus {
    let phone = item.get("phone");
    let desk = item.get("desk");
    let main = if decision(phone).is_some() {
        phone
    } else {
        desk
    };
    let sent = decision(main) == Some("sent");
    let held = |p: Option<&Value>| {
        p.and_then(|p| p.get("held"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let reason = match (decision(phone), decision(desk)) {
        (Some("held"), _) => held(phone),
        (Some("off"), _) => held(phone).or(Some("off".into())),
        (_, Some("held")) => held(desk),
        _ => None,
    };
    RowStatus {
        sent,
        reason: if sent { None } else { reason },
    }
}

/// What the held path's notice was merged into (the phone's if held, else the desk's).
fn merged_into(item: &Map<String, Value>) -> Option<&Value> {
    for k in ["phone", "desk"] {
        let p = item.get(k);
        if decision(p) == Some("held") {
            return p?.get("merged_into").filter(|m| m.is_object());
        }
    }
    None
}

/// Writes `status`, `reason` and `reason_text` into a grouped item and returns its status.
fn annotate(m: &mut Map<String, Value>) -> RowStatus {
    let s = row_status(m);
    let text = (!s.sent).then(|| held_text(s.reason.as_deref(), merged_into(m)));
    m.insert("status".into(), json!(if s.sent { "sent" } else { "held" }));
    m.insert("reason".into(), json!(s.reason));
    m.insert("reason_text".into(), json!(text));
    s
}

/// A notice kind's name in the log ("Buying burst"); an unknown kind is its raw name.
pub fn kind_label(notice: &str) -> &str {
    match notice {
        "runners.new" => "Runners flag",
        "runners.burst" => "Buying burst",
        "runners.fast" => "Fast move",
        "runners.run" => "Run +20%",
        "runners.running" => "Running",
        "runners.breakout" => "Breakout",
        "runners.pullback" => "Pullback",
        "ross.crossing" => "Ross crossing",
        "ross.trigger" => "Ross at trigger",
        "signals.crossing" => "Signals crossing",
        "signals.trigger" => "Signals at trigger",
        "runners.shelf" => "Volume shelf",
        "runners.followup" => "Follow-up",
        "halt.watch" => "Halt watcher",
        "price.alert" => "Price alert",
        other => other,
    }
}

/// A hold reason in plain words ("no device wanted it"). A merged one names what it was folded
/// into when known: "merged into the Running alert 1 s earlier". Old days keep their old codes
/// (`symbol_limit`, `merged`: the per-stock gap and merge removed on 2026-10-30), still worded.
pub fn held_text(reason: Option<&str>, merged: Option<&Value>) -> String {
    let Some(reason) = reason else {
        return "held".into();
    };
    if reason == "merged"
        && let Some(m) = merged
    {
        let kind = m.get("notice").and_then(Value::as_str).unwrap_or("");
        let ago = match m.get("secs").and_then(Value::as_f64) {
            Some(s) if s.is_finite() && s >= 1.0 => format!("{} s earlier", s.round() as i64),
            _ => "moments earlier".into(),
        };
        return format!("merged into the {} alert {ago}", kind_label(kind));
    }
    match reason {
        "symbol_limit" => "same stock notified moments earlier".into(),
        "no_device" => "no device wanted it".into(),
        "quiet_hours" => "quiet hours".into(),
        "stale" => "too old by the time it was decided".into(),
        "duplicate" => "already sent".into(),
        "off" => "phone alerts off for it".into(),
        "merged" => "merged into an earlier alert".into(),
        other => other.replace('_', " "),
    }
}

/// Sent / held counts and the held ones by reason (the feed keeps one incrementally).
#[derive(Clone, Debug, Default)]
struct Tally {
    sent: i64,
    held: BTreeMap<String, i64>,
}

impl Tally {
    /// Counts `s` `n` times (`n = -1` takes back a row's previous status).
    fn add(&mut self, s: &RowStatus, n: i64) {
        match s.bucket() {
            None => self.sent += n,
            Some(r) => {
                let e = self.held.entry(r.to_string()).or_default();
                *e += n;
                if *e <= 0 {
                    self.held.remove(r);
                }
            }
        }
    }

    /// `{sent, held, reasons: [{reason, text, n}]}`, most common reason first (ties by code).
    fn summary(&self) -> Value {
        let mut reasons: Vec<(&String, &i64)> = self.held.iter().collect();
        reasons.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        json!({
            "sent": self.sent,
            "held": reasons.iter().map(|(_, n)| **n).sum::<i64>(),
            "reasons": reasons.iter().map(|(r, n)| json!({
                "reason": r,
                "text": held_text(Some(r.as_str()), None),
                "n": n,
            })).collect::<Vec<_>>(),
        })
    }
}

/// The summary line over `items` (grouped rows).
pub fn summary(items: &[Value]) -> Value {
    let mut t = Tally::default();
    for i in items {
        if let Value::Object(m) = i {
            t.add(&row_status(m), 1);
        }
    }
    t.summary()
}

/// One `notices.append` on the feed's channel, or the start of a new ET day.
#[derive(Debug)]
pub struct Append {
    pub day: NaiveDate,
    /// Taken under the feed's lock: compare with [`Snapshot::seq`].
    pub seq: u64,
    /// The notice's symbol, when its lines name one (the ticker page follows its own); `None`
    /// for a rollover.
    pub symbol: Option<String>,
    /// The append frame; empty for a rollover ([`Append::roll`]).
    pub frame: String,
    /// `day` began ([`NoticeFeed::roll`]): nothing to send as such; a subscription following
    /// today answers it with `day`'s snapshot, one showing a fixed day ignores it.
    pub roll: bool,
}

/// A `notices.snapshot` and the append `seq` it already includes.
#[derive(Debug)]
pub struct Snapshot {
    pub day: NaiveDate,
    pub seq: u64,
    pub frame: String,
}

/// Today's ledger lines in memory, per event, in file order (module docs, "Pushed").
#[derive(Default)]
struct DayLines {
    day: Option<NaiveDate>,
    /// Event ids in order of first appearance, and each event's lines in order.
    order: Vec<String>,
    events: HashMap<String, Vec<String>>,
    /// Each event's current row status and the day's tally of them ([`FEATURE_ROWS`]): kept
    /// as lines arrive, so an append carries the day's summary without regrouping the day.
    statuses: HashMap<String, RowStatus>,
    tally: Tally,
    bytes: u64,
    seq: u64,
}

impl DayLines {
    /// Starts `day` empty. `seq` keeps counting: a connection compares it across days.
    fn reset(&mut self, day: NaiveDate) {
        let seq = self.seq;
        *self = DayLines {
            day: Some(day),
            seq,
            ..Default::default()
        };
    }

    /// Keeps `line` under `id`; false once the day's cap is reached.
    fn keep(&mut self, id: &str, line: String) -> bool {
        if self.bytes + line.len() as u64 > crate::oplog::NOTICES_CAP {
            return false;
        }
        self.bytes += line.len() as u64 + 1;
        match self.events.get_mut(id) {
            Some(v) => v.push(line),
            None => {
                self.order.push(id.to_string());
                self.events.insert(id.to_string(), vec![line]);
            }
        }
        true
    }

    /// Regroups event `id` from its lines so far, recounting its status in the tally. Returns
    /// its item (one, or none when its lines do not make one).
    fn regroup(&mut self, id: &str) -> Vec<Value> {
        let (items, _) = group(&self.events[id].join("\n"));
        if let Some(Value::Object(m)) = items.first() {
            let now = row_status(m);
            if let Some(old) = self.statuses.insert(id.to_string(), now.clone()) {
                self.tally.add(&old, -1);
            }
            self.tally.add(&now, 1);
        }
        items
    }

    /// The whole day as ledger text, grouped per event. [`group`] reads it exactly as it reads
    /// the file: its result depends only on each event's line order and the order in which
    /// events first appear, both kept here.
    fn text(&self) -> String {
        let mut out = String::with_capacity(self.bytes as usize);
        for id in &self.order {
            for l in &self.events[id] {
                out.push_str(l);
                out.push('\n');
            }
        }
        out
    }
}

struct FeedInner {
    dir: Option<PathBuf>,
    lines: Mutex<DayLines>,
    tx: broadcast::Sender<Arc<Append>>,
}

/// The pushed notification log (module docs, "Pushed"). Cheap to clone. The ledger feeds it
/// every line it writes (`crate::alerts::ledger`); `/ws` sessions subscribe to it.
#[derive(Clone)]
pub struct NoticeFeed(Arc<FeedInner>);

impl std::fmt::Debug for NoticeFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoticeFeed")
            .field("dir", &self.0.dir)
            .finish()
    }
}

impl NoticeFeed {
    /// No ledger (tests, a replay-only server): empty snapshots, never an append.
    pub fn idle() -> Self {
        Self::open(None, crate::oplog::et_date_ms(crate::oplog::now_ms()))
    }

    /// The feed over the ledger directory `dir`, primed with `today`'s file if there is one.
    pub fn open(dir: Option<PathBuf>, today: NaiveDate) -> Self {
        let mut lines = DayLines::default();
        lines.reset(today);
        if let Some(d) = &dir {
            let file = d.join(format!("{}.jsonl", today.format("%Y-%m-%d")));
            for line in std::fs::read_to_string(file).unwrap_or_default().lines() {
                // A torn or malformed line is skipped, as `group` skips it.
                if let Some(id) = event_id(line) {
                    lines.keep(&id, line.to_string());
                }
            }
            for id in lines.order.clone() {
                lines.regroup(&id);
            }
        }
        NoticeFeed(Arc::new(FeedInner {
            dir,
            lines: Mutex::new(lines),
            tx: broadcast::channel(256).0,
        }))
    }

    pub fn dir(&self) -> Option<&Path> {
        self.0.dir.as_deref()
    }

    /// Subscribe BEFORE taking [`NoticeFeed::snapshot`].
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Append>> {
        self.0.tx.subscribe()
    }

    /// A ledger line, written for ET date `day`: kept, and its notice's regrouped item broadcast.
    /// A line for an earlier day than the one held (a `pushed` stamped just before midnight,
    /// arriving after it) is in that day's file only; a later day starts a new day.
    pub fn push(&self, day: NaiveDate, line: &Value) {
        let Some(id) = line.get("event_id").and_then(Value::as_str) else {
            return;
        };
        let mut l = self.0.lines.lock().expect("notice feed lock");
        match l.day {
            Some(d) if day < d => return,
            Some(d) if day == d => {}
            _ => l.reset(day),
        }
        if !l.keep(id, line.to_string()) {
            return;
        }
        let items = l.regroup(id);
        l.seq += 1;
        let symbol = items
            .first()
            .and_then(|i| i.get("symbol"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let frame = typed_frame(
            "notices.append",
            json!({
                "day": day.format("%Y-%m-%d").to_string(),
                "items": items,
                "summary": l.tally.summary(),
            }),
        );
        // Under the lock, so appends reach the channel in `seq` order.
        let _ = self.0.tx.send(Arc::new(Append {
            day,
            seq: l.seq,
            symbol,
            frame,
            roll: false,
        }));
    }

    /// One day's grouped items, whether older ones were cut, and the day's summary: today's
    /// from memory (what [`NoticeFeed::snapshot`] serves), any other day's from its file. Reads
    /// files, so call it off the async workers.
    pub fn items(&self, day: NaiveDate) -> (Vec<Value>, bool, Value) {
        let held = {
            let l = self.0.lines.lock().expect("notice feed lock");
            (l.day == Some(day)).then(|| l.text())
        };
        match (held, &self.0.dir) {
            (Some(text), _) => group_summarized(&text),
            (None, Some(dir)) => read_day_summarized(dir, day),
            (None, None) => (Vec::new(), false, summary(&[])),
        }
    }

    /// ET date `today` has begun: when it is later than the day held, the feed starts it empty
    /// and tells the subscribers (an [`Append`] with `roll`). Idempotent; an earlier or equal
    /// date does nothing.
    pub fn roll(&self, today: NaiveDate) {
        let mut l = self.0.lines.lock().expect("notice feed lock");
        if l.day.is_some_and(|d| d >= today) {
            return;
        }
        l.reset(today);
        l.seq += 1;
        let _ = self.0.tx.send(Arc::new(Append {
            day: today,
            seq: l.seq,
            symbol: None,
            frame: String::new(),
            roll: true,
        }));
    }

    /// Rolls the feed over at every ET midnight, on the server's wall clock (the ledger dates
    /// its lines the same way, `crate::oplog::et_date_ms`). It sleeps until just past the next
    /// midnight, but never longer than [`ROLL_CHECK`], so a clock change or a suspended host
    /// costs at most that much lateness. Needs a tokio runtime.
    pub fn spawn_rollover(&self) {
        let feed = self.clone();
        tokio::spawn(async move {
            loop {
                let now = crate::oplog::now_ms();
                let wait = (next_et_midnight_ms(now) - now).max(0) as u64 + ROLL_SLACK_MS;
                tokio::time::sleep(std::time::Duration::from_millis(wait).min(ROLL_CHECK)).await;
                feed.roll(crate::oplog::et_date_ms(crate::oplog::now_ms()));
            }
        });
    }

    /// The `notices.snapshot` of `day` (None = today in ET). Reads files (the day list, and a
    /// day not held in memory), so call it off the async workers.
    pub fn snapshot(&self, day: Option<NaiveDate>) -> Snapshot {
        let day = day.unwrap_or_else(|| crate::oplog::et_date_ms(crate::oplog::now_ms()));
        let days = self.0.dir.as_deref().map(days).unwrap_or_default();
        let (held, seq) = {
            let l = self.0.lines.lock().expect("notice feed lock");
            ((l.day == Some(day)).then(|| l.text()), l.seq)
        };
        let (items, truncated, summary) = match (held, &self.0.dir) {
            (Some(text), _) => group_summarized(&text),
            (None, Some(dir)) => read_day_summarized(dir, day),
            (None, None) => (Vec::new(), false, summary(&[])),
        };
        let frame = typed_frame(
            "notices.snapshot",
            json!({
                "day": day.format("%Y-%m-%d").to_string(),
                "days": days.iter().map(|d| d.format("%Y-%m-%d").to_string()).collect::<Vec<_>>(),
                "items": items,
                "truncated": truncated,
                "summary": summary,
            }),
        );
        Snapshot { day, seq, frame }
    }
}

/// The longest the rollover task sleeps between looks at the clock.
const ROLL_CHECK: std::time::Duration = std::time::Duration::from_secs(60);

/// Wakes this far past midnight, so the date it then reads is certainly the new one.
const ROLL_SLACK_MS: u64 = 50;

/// Epoch ms of the first ET midnight after `now_ms`. Midnight always exists in New York (the
/// clocks change at 02:00), so DST only changes how far away it is: 23 or 25 hours on those days.
pub fn next_et_midnight_ms(now_ms: i64) -> i64 {
    use chrono::TimeZone;
    let tomorrow = crate::oplog::et_date_ms(now_ms)
        .succ_opt()
        .expect("a date after today");
    chrono_tz::America::New_York
        .from_local_datetime(&tomorrow.and_time(chrono::NaiveTime::MIN))
        .earliest()
        .map_or(now_ms + 86_400_000, |t| t.timestamp_millis())
}

/// `rest` (an object) as a frame with `type` first. The web and phone feeds route a frame by its
/// leading `{"type":"notices.` without parsing it (apps/*/src/feed/serverFeed.ts); serde_json's
/// map sorts keys, so `json!` would put `type` after `day`, and a snapshot of a whole day (MBs on
/// a busy one) would be parsed twice more as a chart and a paper frame before reaching its handler.
fn typed_frame(kind: &str, rest: Value) -> String {
    let rest = rest.to_string();
    debug_assert!(
        rest.len() > 2 && rest.starts_with('{'),
        "a non-empty object"
    );
    format!("{{\"type\":{},{}", json!(kind), &rest[1..])
}

fn event_id(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .ok()?
        .get("event_id")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: Value) -> String {
        v.to_string()
    }

    #[test]
    fn one_item_per_notice_newest_first() {
        let text = [
            line(json!({"kind":"decision","v":1,"decided_at":1000,"event_id":"a","notice":"runners.burst","symbol":"SRTM","price":0.55,"path":"desk","decision":"sent","held":null,"devices":2,"title":"SRTM · Buying burst","body":"b"})),
            line(json!({"kind":"decision","v":1,"decided_at":1001,"event_id":"a","notice":"runners.burst","symbol":"SRTM","price":0.55,"path":"phone","decision":"held","held":"symbol_limit","devices":1,"title":"SRTM · Buying burst","body":"b"})),
            line(json!({"kind":"pushed","v":1,"event_id":"a","notice":"runners.burst","symbol":"SRTM","expo_ns":1_500_000_000i64,"trade_to_expo_ms":1044.6,"board_to_queue_ms":838.4,"queue_to_expo_ms":105.1,"trade_to_board_ms":100.9})),
            line(json!({"kind":"alert","v":1,"decided_at":2000,"event_id":"p","alert_id":"x","symbol":"AAPL","price":200.0,"text":"AAPL above 200","phone":"sent","held":null,"devices":1})),
            line(json!({"kind":"failed","v":1,"at":2500,"event_id":"p","detail":"DeviceNotRegistered"})),
            "{\"kind\":\"decision\",\"event_id\":\"torn".to_string(),
        ]
        .join("\n");
        let (items, truncated) = group(&text);
        assert!(!truncated);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["notice"], "price.alert");
        assert_eq!(items[0]["phone"]["decision"], "sent");
        assert_eq!(items[0]["failed"], "DeviceNotRegistered");
        assert_eq!(items[1]["symbol"], "SRTM");
        assert_eq!(items[1]["at"], 1000);
        assert_eq!(items[1]["desk"]["decision"], "sent");
        assert_eq!(items[1]["phone"]["held"], "symbol_limit");
        assert_eq!(items[1]["latency"]["trade_to_expo_ms"], 1044.6);
    }

    /// A merged Runners path (old days, before the 2026-10-30 removal) keeps what it was folded into.
    #[test]
    fn a_merged_path_says_what_it_was_folded_into() {
        let text = line(
            json!({"kind":"decision","v":1,"decided_at":3000,"event_id":"f",
            "notice":"runners.fast","symbol":"WFF","path":"phone","decision":"held","held":"merged",
            "devices":2,"merged_into":{"event_id":"r","notice":"runners.running","secs":1.0},
            "merged_devices":2}),
        );
        let (items, _) = group(&text);
        assert_eq!(items[0]["phone"]["held"], "merged");
        assert_eq!(
            items[0]["phone"]["merged_into"]["notice"],
            "runners.running"
        );
        assert_eq!(items[0]["phone"]["merged_devices"], 2);
        assert!(items[0].get("desk").is_none());
    }

    #[test]
    fn caps_to_the_newest_and_lists_days() {
        let text: String = (0..MAX_ITEMS + 5)
            .map(|i| line(json!({"kind":"decision","decided_at":i,"event_id":format!("e{i}"),"path":"desk","decision":"sent"})))
            .collect::<Vec<_>>()
            .join("\n");
        let (items, truncated) = group(&text);
        assert!(truncated);
        assert_eq!(items.len(), MAX_ITEMS);
        assert_eq!(items[0]["event_id"], format!("e{}", MAX_ITEMS + 4));

        let dir = tempfile::tempdir().unwrap();
        for f in [
            "2026-10-26.jsonl",
            "2026-10-27.jsonl",
            ".incoming-2026-10-28.jsonl",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        let d = days(dir.path());
        assert_eq!(
            d.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
            vec!["2026-10-27", "2026-10-26"]
        );
        // A day without a file is empty, not an error.
        assert_eq!(
            read_day(dir.path(), "2026-10-20".parse().unwrap()).0.len(),
            0
        );
    }

    #[test]
    fn frozen_wick_observations_reach_the_read_api_unchanged() {
        let wick = serde_json::to_value(stock_context::MicroWick::measure(
            Some([105, 120, 100, 108]),
            15,
            32,
        ))
        .unwrap();
        let text = line(json!({"kind":"decision","event_id":"w","decided_at":1000,
            "symbol":"WFF","notice":"runners.pullback","path":"phone","decision":"held",
            "held":"symbol_limit","mp5_wick":wick}));
        let (items, _) = group(&text);
        assert_eq!(items[0]["mp5_wick"], wick);
        assert_eq!(items[0]["phone"]["decision"], "held");
    }

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn take(rx: &mut broadcast::Receiver<Arc<Append>>) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(a) = rx.try_recv() {
            // `type` first: the clients route on the frame's prefix (`typed_frame`).
            assert!(a.frame.starts_with(r#"{"type":"notices.append","#));
            out.push(serde_json::from_str(&a.frame).unwrap());
        }
        out
    }

    /// The pushed log: each ledger line re-sends its notice's whole item; the snapshot of today
    /// comes from memory (primed from the file at start) and groups as the file does.
    #[test]
    fn the_feed_appends_regrouped_items_and_snapshots_today_from_memory() {
        let dir = tempfile::tempdir().unwrap();
        let before = line(
            json!({"kind":"decision","decided_at":100,"event_id":"old","notice":"runners.fast",
            "symbol":"AAA","path":"desk","decision":"sent"}),
        );
        std::fs::write(dir.path().join("2026-10-30.jsonl"), format!("{before}\n")).unwrap();
        let feed = NoticeFeed::open(Some(dir.path().to_path_buf()), d("2026-10-30"));
        let mut rx = feed.subscribe();
        let snap = feed.snapshot(Some(d("2026-10-30")));
        assert!(snap.frame.starts_with(r#"{"type":"notices.snapshot","#));
        let v: Value = serde_json::from_str(&snap.frame).unwrap();
        assert_eq!(v["type"], "notices.snapshot");
        assert_eq!(v["items"][0]["event_id"], "old", "primed from today's file");
        assert_eq!(v["days"], json!(["2026-10-30"]));

        // The restart's earlier event gets its phone decision now: the append carries both paths.
        feed.push(
            d("2026-10-30"),
            &json!({"kind":"decision","decided_at":101,"event_id":"old","notice":"runners.fast",
            "symbol":"AAA","path":"phone","decision":"held","held":"quiet_hours"}),
        );
        feed.push(
            d("2026-10-30"),
            &json!({"kind":"decision","decided_at":200,"event_id":"new","notice":"runners.burst",
            "symbol":"BBB","path":"desk","decision":"sent"}),
        );
        feed.push(d("2026-10-30"), &json!({"no_event_id": true}));
        let got = take(&mut rx);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["type"], "notices.append");
        assert_eq!(got[0]["items"][0]["desk"]["decision"], "sent");
        assert_eq!(got[0]["items"][0]["phone"]["held"], "quiet_hours");
        assert_eq!(got[1]["items"][0]["symbol"], "BBB");
        let snap2 = feed.snapshot(Some(d("2026-10-30")));
        assert!(snap2.seq > snap.seq);
        let v: Value = serde_json::from_str(&snap2.frame).unwrap();
        assert_eq!(v["items"][0]["event_id"], "new", "newest first");
        assert_eq!(v["items"][1]["phone"]["decision"], "held");

        // A line for the day before is that day's file's business; the next day starts afresh.
        feed.push(
            d("2026-10-29"),
            &json!({"kind":"failed","at":5,"event_id":"x","detail":"d"}),
        );
        assert!(take(&mut rx).is_empty());
        feed.push(
            d("2026-10-31"),
            &json!({"kind":"decision","decided_at":300,"event_id":"n2","path":"desk","decision":"sent"}),
        );
        assert_eq!(take(&mut rx)[0]["day"], "2026-10-31");
        let v: Value = serde_json::from_str(&feed.snapshot(Some(d("2026-10-31"))).frame).unwrap();
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        // A day no longer held is read from its file.
        let v: Value = serde_json::from_str(&feed.snapshot(Some(d("2026-10-30"))).frame).unwrap();
        assert_eq!(v["items"][0]["event_id"], "old");
    }

    /// The rows the server assembles (`notices.rows.v1`): the phone's decision wins over the
    /// desk's, a switched-off price alert is held as `off`, a merged path names what it joined,
    /// and the summary counts the whole day by reason (also past the item cap).
    #[test]
    fn rows_carry_status_reason_and_the_day_summary() {
        let text = [
            line(json!({"kind":"decision","decided_at":1,"event_id":"a","path":"desk","decision":"sent"})),
            line(json!({"kind":"decision","decided_at":1,"event_id":"a","path":"phone","decision":"held","held":"no_device"})),
            line(json!({"kind":"decision","decided_at":2,"event_id":"b","path":"desk","decision":"sent"})),
            line(json!({"kind":"alert","decided_at":3,"event_id":"c","phone":"off","held":null})),
            line(json!({"kind":"decision","decided_at":4,"event_id":"d","path":"phone","decision":"held",
                "held":"merged","merged_into":{"event_id":"r","notice":"runners.running","secs":1.2}})),
            line(json!({"kind":"decision","decided_at":5,"event_id":"e","path":"phone","decision":"held","held":"no_device"})),
        ]
        .join("\n");
        let (items, _, summary) = group_summarized(&text);
        let by: HashMap<&str, &Value> = items
            .iter()
            .map(|i| (i["event_id"].as_str().unwrap(), i))
            .collect();
        assert_eq!(by["a"]["status"], "held");
        assert_eq!(by["a"]["reason"], "no_device");
        assert_eq!(by["b"]["status"], "sent");
        assert_eq!(by["b"]["reason_text"], Value::Null);
        assert_eq!(by["c"]["reason"], "off");
        assert_eq!(by["c"]["reason_text"], "phone alerts off for it");
        assert_eq!(
            by["d"]["reason_text"],
            "merged into the Running alert 1 s earlier"
        );
        assert_eq!(
            summary,
            json!({"sent": 1, "held": 4, "reasons": [
                {"reason": "no_device", "text": "no device wanted it", "n": 2},
                {"reason": "merged", "text": "merged into an earlier alert", "n": 1},
                {"reason": "off", "text": "phone alerts off for it", "n": 1},
            ]})
        );
        assert_eq!(summary, super::summary(&items));
    }

    /// The feed keeps the summary as lines change a row's status, and tags appends by symbol.
    #[test]
    fn the_feed_keeps_the_summary_and_names_the_symbol() {
        let feed = NoticeFeed::open(None, d("2026-10-30"));
        let mut rx = feed.subscribe();
        feed.push(
            d("2026-10-30"),
            &json!({"kind":"decision","decided_at":1,"event_id":"a","symbol":"INO","path":"desk","decision":"sent"}),
        );
        feed.push(
            d("2026-10-30"),
            &json!({"kind":"decision","decided_at":1,"event_id":"a","symbol":"INO","path":"phone","decision":"held","held":"quiet_hours"}),
        );
        let a1 = rx.try_recv().unwrap();
        let a2 = rx.try_recv().unwrap();
        assert_eq!(a1.symbol.as_deref(), Some("INO"));
        let v1: Value = serde_json::from_str(&a1.frame).unwrap();
        let v2: Value = serde_json::from_str(&a2.frame).unwrap();
        assert_eq!(v1["summary"]["sent"], 1);
        assert_eq!(v2["summary"]["sent"], 0);
        assert_eq!(v2["summary"]["reasons"][0]["reason"], "quiet_hours");
        let (_, _, s) = feed.items(d("2026-10-30"));
        assert_eq!(s, v2["summary"]);
    }

    /// Midnight starts the next day empty and says so once; an older or the same date changes
    /// nothing; a line for the day before is still that day's file's alone.
    #[test]
    fn the_feed_rolls_over_to_an_empty_day() {
        let feed = NoticeFeed::open(None, d("2026-10-30"));
        feed.push(
            d("2026-10-30"),
            &json!({"kind":"decision","decided_at":1,"event_id":"a","path":"desk","decision":"sent"}),
        );
        let mut rx = feed.subscribe();
        let before = feed.snapshot(Some(d("2026-10-30"))).seq;
        feed.roll(d("2026-10-30"));
        feed.roll(d("2026-10-29"));
        assert!(rx.try_recv().is_err(), "not a new day");
        feed.roll(d("2026-10-31"));
        let r = rx.try_recv().unwrap();
        assert!(r.roll && r.frame.is_empty());
        assert_eq!((r.day, r.seq > before), (d("2026-10-31"), true));
        feed.roll(d("2026-10-31"));
        assert!(rx.try_recv().is_err(), "once");
        let v: Value = serde_json::from_str(&feed.snapshot(Some(d("2026-10-31"))).frame).unwrap();
        assert_eq!(v["items"], json!([]));
        assert!(feed.snapshot(Some(d("2026-10-31"))).seq >= r.seq);
        feed.push(
            d("2026-10-30"),
            &json!({"kind":"failed","at":5,"event_id":"a","detail":"late"}),
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn the_next_midnight_follows_new_york_across_dst() {
        use chrono::TimeZone;
        let utc = |y, mo, d, h, mi| {
            chrono::Utc
                .with_ymd_and_hms(y, mo, d, h, mi, 0)
                .unwrap()
                .timestamp_millis()
        };
        // 2026-10-31 23:59 EDT -> 2026-11-01 00:00 EDT (04:00 UTC), a minute away.
        assert_eq!(
            next_et_midnight_ms(utc(2026, 11, 1, 3, 59)),
            utc(2026, 11, 1, 4, 0)
        );
        // The fall-back day is 25 hours long: 2026-11-01 00:00 EDT -> 2026-11-02 00:00 EST.
        assert_eq!(
            next_et_midnight_ms(utc(2026, 11, 1, 4, 0)),
            utc(2026, 11, 2, 5, 0)
        );
        // The spring-forward day is 23: 2026-03-08 00:00 EST -> 2026-03-09 00:00 EDT.
        assert_eq!(
            next_et_midnight_ms(utc(2026, 3, 8, 5, 0)),
            utc(2026, 3, 9, 4, 0)
        );
    }

    #[test]
    fn an_idle_feed_is_empty() {
        let feed = NoticeFeed::idle();
        let v: Value = serde_json::from_str(&feed.snapshot(Some(d("2026-10-30"))).frame).unwrap();
        assert_eq!(
            (v["items"].clone(), v["days"].clone()),
            (json!([]), json!([]))
        );
    }
}
