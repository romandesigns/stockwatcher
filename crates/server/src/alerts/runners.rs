//! Runners phone pushes (owner, 2026-10-22): the Runners board's new flags and breakouts reach
//! the phone while the app is closed. He missed CJMB (+95 % in ~30 minutes at 08:05 ET) because
//! the only Runners signal was on screen, or a local banner while the dashboard was open.
//!
//! The pipeline is the price alerts' own ([`super::push`]): same registered phones, same Expo
//! sender, batching, retries and token clean-up. What is Runners-specific lives here:
//!
//! - **What:** a [`Notice`] from the LIVE market's board (`crate::rocket`, "Phone pushes"): a new
//!   flag, a flagged row turning `breaking_out`, a new buying burst (`crate::rocket::burst`), a
//!   running entry (`crate::rocket::running`), a "Run +20%" entry (`crate::rocket::run`) or a
//!   "Fast move" entry (`crate::rocket::fast`). The
//!   market offers it to the alerts task with `try_send` after the core lock is released, so
//!   ingest and the board never wait on a push. A replay market never offers one.
//! - **Text** ([`message`]): the evidence in one line, short enough for a lock screen. Title
//!   "SYM · <source> · $price" ("Buying burst", "Running", "Run +20%", "Fast move"); body the %
//!   move and its
//!   window, then for a burst the research's odds ("typ. +13% max in 30 min, 1 in 4 reach +20%
//!   before −5%", the panel's Bursts line). The body always ends with a reference line of its
//!   own, "Alert $2.41 · +5% = $2.53" ([`LIMIT_REF_PCT`]; owner, 2026-10-23), and the push
//!   carries the "runners" category ([`CATEGORY_ID`]) whose "Copy ticker" button the phone
//!   registers. One text for every surface: the phone push, the desktop and web `runners.notice`
//!   and the watch (which mirrors the phone).
//! - **Who:** each registered phone opts in per kind, on the device record
//!   (`push-devices.json`, sent in `push.register`): new flags ON by default, breakouts OFF,
//!   bursts, running, run and fast-move entries ON (owner, 2026-10-22: "I don't wanna lose opportunities
//!   because I'm away from my phone").
//!   Per device rather than synced because it is about which phone makes noise, exactly like the
//!   token itself.
//! - **Every panel event backed by evidence** (owner, 2026-10-23), through this same pipeline
//!   and text: a Runners stock entering a volume shelf ([`NoticeKind::Shelf`], `crate::zones`;
//!   held / lost outcomes never notify), a later Runners path on a stock that already has a row
//!   ([`NoticeKind::FollowUp`], `crate::rocket` "Follow-ups"), a Ross or Signals MP5 episode
//!   reaching "Crossing" (on) or "At trigger" (off) (`crate::cue_notices`, which also explains
//!   the Ross/Signals dedupe: one episode is one notice, the Ross kind, wanted through either
//!   switch) and a Halt watcher gauge appearing ([`NoticeKind::Halt`], off). The data's `kind`
//!   is [`data_kind`] (`ross.crossing`, `signals.trigger`, `halt.watch`, …), every push the
//!   same "runners" category.
//! - **Every distinct event is delivered** (owner, 2026-10-30: "I don't want to keep missing
//!   alerts because they weren't delivered to my phone"). Each notice with an
//!   [`crate::rocket::Notice::event_id`] not sent before goes to every recipient whose switch for
//!   its kind is on. There is no per-stock gap and no merging. TNMG 2026-10-30 lost two: a
//!   Follow-up at 7:49:45 held as `symbol_limit` ("same stock notified moments earlier") 13 s
//!   after its pullback notice, and a breakout ("15s new high since alert") at 7:19:30 held as
//!   `merged`, within 30 s of its flag. This reverses, for phone delivery, the 2026-10-26 WFF
//!   decision (Running, Fast move and Breakout within 14 s merged into one push) and drops the
//!   per-stock gaps of 2026-10-23 (10 minutes, 2 for new evidence). A burst on one stock stacks on
//!   the phone instead: iOS groups a stock's pushes by `threadId` (`super::push`); Android groups
//!   an app's notifications itself (Expo's push API has no Android group field, and its `tag` and
//!   `collapseId` REPLACE a notification, which is exactly the loss this rule forbids).
//! - **Limits**, checked in this order (the first that applies holds it back; logged, never
//!   sent later). These are the only ones:
//!   0. this event was sent already (persisted, so a restart cannot repeat one) → `duplicate`;
//!   1. no recipient opted in for this kind → `no_device`;
//!   2. the price alerts' quiet hours (`alerts.prefs`, ET wall time, the owner's own setting) →
//!      `quiet_hours`;
//!   3. the notice is over [`MAX_AGE_MS`] (3 minutes) old, i.e. a catch-up after a stall →
//!      `stale`.
//!
//!   Ledger lines written before 2026-10-30 may also say `symbol_limit` or `merged`; the
//!   Insights log still words them.
//!
//!   There is NO daily cap (owner, 2026-10-23: "always send out notifications as long as the
//!   evidence of such events supports it"). Volume is bounded by the detectors themselves (one
//!   row per stock per day per path, one follow-up per path per row, one pullback setup per stock
//!   per day); the day's count is still kept (`sent_today`) for the logs and the scorecard.
//!
//! - **Desktop and web** (owner, 2026-10-23: the phone's notifications there too): every notice
//!   is decided a second time for the open desktop and web clients ([`desk_frame`]), with the
//!   same limits but its own memory (`runners-desk.json`). Their "devices" are the connections
//!   that sent `runners.notify` with this kind on (`super::DeskKinds`, the phone's defaults); a
//!   Send goes to every socket as a `runners.notice` frame carrying [`message`]'s title and body,
//!   and each client shows it as a system notification if its own switch is on. Each event is its
//!   own notification there too (the frame's `id` is its tag); nothing updates or replaces an
//!   earlier one.
//!
//! - **Latency** (target: under 5 s from the triggering trade to Expo): a Runners push skips the
//!   push task's batch window ([`super::push`]) and, once Expo took it, logs `runners push:
//!   latency` with `exec_ns` (the triggering print, when the source knows it: a "Run +20%" row),
//!   `decided_ns` (the board's row), `queued_ns` (handed to the push task) and `expo_ns`, and the
//!   three legs in ms. Where the sources decide: a "Run +20%" row within about a second of the
//!   print that makes it (`crate::zones`, "Early fire"; 1–2 s with the 2-print guard), a burst
//!   at 5 s boundaries checked 1 s after (up to ~6 s after the print), a running entry and an R2
//!   flag at 15 s barriers (up to ~15 s after the print), a fast move at the end of its second.
//!   The board decides each ~50 ms after that moment (`crate::market::ROCKET_SETTLE`, the
//!   rocket task; it was 250 ms to 1.25 s on the 1 Hz broadcast tick), so `board_to_queue` is
//!   that plus the alerts task.
//!
//!   The count and the events already sent are kept in `runners-push.json` beside the devices,
//!   so a redeploy cannot send an event twice. They are separate from the price alerts' phone
//!   budget: a busy Runners morning must not silence a price alert the owner set himself.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::model::Prefs;
use crate::rocket::{AlertKind, Notice, NoticeDetail, NoticeKind};

/// A notice older than this when it reaches the task is not pushed (`stale`): it can only be a
/// catch-up after a stall (a blocked task, a restart replaying a queue), and news about a move
/// minutes ago is noise on a lock screen. The slowest normal path decides well inside it: a
/// running entry, an R2 flag or a breakout at a 15 s barrier (up to ~16 s after the print), a
/// burst ~6 s, a Run +20% row ~2 s, a pullback or crossing at its transition (module docs,
/// "Latency").
pub const MAX_AGE_MS: i64 = 3 * 60_000;
const FILE: &str = "runners-push.json";
/// The desktop and web notifications' own memory ([`desk_frame`]): kept apart from the phones'
/// so a notice the phones held (none opted in) still reaches an open desktop, and the reverse;
/// the limits themselves are the same.
pub const DESK_FILE: &str = "runners-desk.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Send,
    Held(Hold),
}

/// Why a notice was not sent (module docs, "Limits"). Older ledger lines also carry
/// `symbol_limit` and `merged`, the per-stock gap and merge removed on 2026-10-30; the Insights
/// log still words them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    NoDevice,
    QuietHours,
    Stale,
    /// This event was already sent (its [`crate::rocket::Notice::event_id`]).
    Duplicate,
}

impl Hold {
    pub fn code(self) -> &'static str {
        match self {
            Hold::NoDevice => "no_device",
            Hold::QuietHours => "quiet_hours",
            Hold::Stale => "stale",
            Hold::Duplicate => "duplicate",
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    /// The ET trading day the count belongs to, `YYYY-MM-DD`.
    #[serde(default)]
    day: String,
    #[serde(default)]
    sent: u32,
    /// Today's events already sent ([`crate::rocket::Notice::event_id`]): never twice. (A file
    /// written before 2026-10-30 also has the gaps' `last_by_symbol`, `last_by_kind` and the
    /// merge's `first_push`; serde skips them.)
    #[serde(default)]
    sent_ids: HashSet<String>,
}

/// The Runners side's memory: today's count and the events already sent, persisted.
#[derive(Debug, Default)]
pub struct RunnersDelivery {
    dir: Option<PathBuf>,
    file: &'static str,
    s: Saved,
}

impl RunnersDelivery {
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// The phones' memory (`runners-push.json`).
    pub fn open(dir: &Path) -> Self {
        Self::open_file(dir, FILE)
    }

    /// The same memory under another file ([`DESK_FILE`] for the desktop and web notices).
    pub fn open_file(dir: &Path, file: &'static str) -> Self {
        let s = std::fs::read(dir.join(file))
            .ok()
            .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
            .unwrap_or_default();
        RunnersDelivery {
            dir: Some(dir.to_owned()),
            file,
            s,
        }
    }

    fn save(&self) {
        let Some(dir) = &self.dir else { return };
        let file = self.file;
        let res = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(dir)?;
            let tmp = dir.join(format!("{file}.tmp"));
            std::fs::write(
                &tmp,
                serde_json::to_vec(&self.s).map_err(std::io::Error::other)?,
            )?;
            std::fs::rename(&tmp, dir.join(file))
        })();
        if let Err(e) = res {
            tracing::warn!(error = %e, "runners push: save failed");
        }
    }

    pub fn sent_today(&self, today: NaiveDate) -> u32 {
        if self.s.day == today.to_string() {
            self.s.sent
        } else {
            0
        }
    }

    /// The decision for one notice (module docs, "Limits"). `devices` = recipients that want its
    /// kind; `at` = when it happened, `now` = now (epoch ms); `today` / `et_minute` = the ET day
    /// and minute of the day at `now`. A Send goes to every one of the `devices`.
    #[allow(clippy::too_many_arguments)]
    pub fn decide(
        &mut self,
        event_id: &str,
        devices: usize,
        prefs: &Prefs,
        at: i64,
        now: i64,
        today: NaiveDate,
        et_minute: u32,
    ) -> Decision {
        let day = today.to_string();
        if self.s.day != day {
            self.s.day = day;
            self.s.sent = 0;
            // An event belongs to its day.
            self.s.sent_ids.clear();
        }
        if self.s.sent_ids.contains(event_id) {
            return Decision::Held(Hold::Duplicate);
        }
        if devices == 0 {
            return Decision::Held(Hold::NoDevice);
        }
        if prefs.quiet.is_some_and(|q| q.contains(et_minute)) {
            return Decision::Held(Hold::QuietHours);
        }
        if now - at > MAX_AGE_MS {
            return Decision::Held(Hold::Stale);
        }
        self.s.sent += 1;
        self.s.sent_ids.insert(event_id.to_owned());
        self.save();
        Decision::Send
    }
}

/// The reference line's step above the alert price, in percent (owner, 2026-10-23). A Fast-move
/// push for YMAT said $2.41 within 1.4 s of the move; a limit at $2.48 never filled while the
/// stock ran to $3.55+ in about a minute. The line ("Alert $2.41 · +5% = $2.53") saves working
/// out, under time pressure, a price that is still marketable a few seconds later. It is a
/// reference, not advice: the text never says "buy" or "limit", and StockSpotter never places
/// orders.
pub const LIMIT_REF_PCT: f64 = 5.0;

/// The longest body a Runners notification carries. Android shows all of it once expanded
/// (BigTextStyle); a lock screen or a watch cuts it anyway. Past this, [`message`] drops the
/// evidence's last clauses, never the reference line.
pub const MAX_BODY_CHARS: usize = 110;

/// The notification category of every Runners push (Expo `categoryId`, set in
/// `super::push::expo_message`): the phone registers it at startup with its "Copy ticker" action
/// (apps/mobile src/notifications/copyTicker.ts `RUNNERS_CATEGORY_ID`; the two must match).
pub const CATEGORY_ID: &str = "runners";

/// `p` on its instrument's tick: $0.0001 below $1, $0.01 from $1 (Reg NMS Rule 612's minimum
/// quoting increments). Half up; the snap to a millionth of a tick first absorbs the binary error
/// that would otherwise round 24.50 × 1.05 = 25.725 down to $25.72.
fn on_tick(p: f64) -> f64 {
    let tick = if p >= 1.0 { 0.01 } else { 0.0001 };
    let ticks = ((p / tick) * 1e6).round() / 1e6;
    ticks.round() * tick
}

/// "Alert $2.41 · +5% = $2.53": the alert price and [`LIMIT_REF_PCT`] above it, both on tick
/// (the second on its own tick: $0.9999 → $1.05).
pub fn price_line(price: f64) -> String {
    let at = on_tick(price);
    let up = on_tick(at * (1.0 + LIMIT_REF_PCT / 100.0));
    format!(
        "Alert {} · +{LIMIT_REF_PCT:.0}% = {}",
        dollars(at),
        dollars(up)
    )
}

/// The evidence, then the reference line on a line of its own (separate in the expanded
/// notification on Android, Windows and the browser). Evidence too long for [`MAX_BODY_CHARS`]
/// loses its last " · " clauses first (a burst's research odds), down to its first.
fn with_price_line(evidence: String, price: f64) -> String {
    // No price known (never expected from the board): no line rather than "$0.0000".
    if !(price.is_finite() && price > 0.0) {
        return evidence;
    }
    let line = price_line(price);
    let room = MAX_BODY_CHARS.saturating_sub(line.chars().count() + 1);
    let mut ev = evidence;
    while ev.chars().count() > room {
        match ev.rfind(" · ") {
            Some(i) => ev.truncate(i),
            None => break,
        }
    }
    if ev.is_empty() {
        line
    } else {
        format!("{ev}\n{line}")
    }
}

/// "$2.45", "$0.4321".
fn dollars(p: f64) -> String {
    if p >= 1.0 {
        format!("${p:.2}")
    } else {
        format!("${p:.4}")
    }
}

/// A price as the Runners notices print it ("$1.86" for 1.855: on its tick, half away from
/// zero, then cents at or above $1 and four places below). The Insights ticker page shows each
/// row's price this way when the notice's title does not carry one, so a row never disagrees with
/// the push it stands for.
pub fn notice_price(p: f64) -> String {
    dollars(on_tick(p))
}

/// The `kind` in the push's data (apps/mobile src/notifications/route.ts reads it).
pub fn data_kind(kind: NoticeKind) -> &'static str {
    match kind {
        NoticeKind::New => "runners.new",
        NoticeKind::Breakout => "runners.breakout",
        NoticeKind::Burst => "runners.burst",
        NoticeKind::Running => "runners.running",
        NoticeKind::Run => "runners.run",
        NoticeKind::Fast => "runners.fast",
        NoticeKind::Shelf => "runners.shelf",
        NoticeKind::FollowUp => "runners.followup",
        NoticeKind::RossCrossing => "ross.crossing",
        NoticeKind::SignalsCrossing => "signals.crossing",
        NoticeKind::RossTrigger => "ross.trigger",
        NoticeKind::SignalsTrigger => "signals.trigger",
        NoticeKind::Halt => "halt.watch",
        // One switch for both stages; the data's `stage` tells them apart.
        NoticeKind::Pullback | NoticeKind::PullbackCrossing => "runners.pullback",
    }
}

/// "+18.2% in 6 min" (under a minute: "in 45 s").
fn moved(p: f64, min: f64) -> String {
    let min = min.max(0.0);
    if min < 1.0 {
        format!("{p:+.1}% in {:.0} s", min * 60.0)
    } else {
        format!("{p:+.1}% in {:.0} min", min.round())
    }
}

/// The research's typical outcome from a burst alert, as the panel's Bursts line says it
/// (packages/shared `rocketBurstSectionView`): "typ. +13% max in 30 min, 1 in 4 reach +20%
/// before −5%".
fn burst_odds() -> String {
    use crate::rocket::burst::{TYPICAL_CONT20_PCT, TYPICAL_MFE30_MED_PCT};
    format!(
        "typ. +{:.0}% max in 30 min, 1 in {:.0} reach +20% before −5%",
        TYPICAL_MFE30_MED_PCT,
        (100.0 / TYPICAL_CONT20_PCT).round().max(1.0)
    )
}

/// Title, body and data of the push for one notice (module docs, "Text"): the ONE place every
/// surface's text comes from (phone, watch, desktop, web). The data's `symbol` is what a tap or
/// click opens the chart for.
pub fn message(n: &Notice) -> (String, String, Value) {
    match n.kind {
        NoticeKind::New
        | NoticeKind::Breakout
        | NoticeKind::Burst
        | NoticeKind::Running
        | NoticeKind::Run
        | NoticeKind::Fast => runner_message(n),
        _ => panel_message(n),
    }
}

/// " · $2.45", or nothing without a usable price.
fn title_price(price: f64) -> String {
    if price.is_finite() && price > 0.0 {
        format!(" · {}", dollars(on_tick(price)))
    } else {
        String::new()
    }
}

/// "3 min ago", "40 s ago".
fn ago(from_ns: i64, to_ns: i64) -> String {
    let s = (to_ns - from_ns).max(0) / 1_000_000_000;
    if s < 60 {
        format!("{s} s ago")
    } else {
        format!("{} min ago", (s + 30) / 60)
    }
}

/// "$0.38–0.42", "$2.10–2.25": a range on its tick, trailing zeros trimmed to cents.
fn range(lo: f64, hi: f64) -> String {
    let fmt = |p: f64| {
        let p = on_tick(p);
        let mut s = if p >= 1.0 {
            format!("{p:.2}")
        } else {
            format!("{p:.4}")
        };
        while s.ends_with('0') && s.len() - s.find('.').unwrap_or(s.len()) > 3 {
            s.pop();
        }
        s
    };
    format!("${}–{}", fmt(lo), fmt(hi))
}

/// The lowest shelf price the shelf research measured ($0.15–$25): below it no historical odds
/// are quoted (review of 75d6418, 2026-10-28).
const SHELF_ODDS_MIN: f64 = 0.15;

/// The volume shelves' honest base rate (`crate::zones::HELD_PCT` / `LOST_PCT`): "about 1 in 3
/// hold, 1 in 2 fail". The bake-off's rate over all entries, never a forecast for this one.
pub fn shelf_odds() -> String {
    use crate::zones::{HELD_PCT, LOST_PCT};
    format!(
        "about 1 in {:.0} hold, 1 in {:.0} fail",
        (100.0 / HELD_PCT).round().max(1.0),
        (100.0 / LOST_PCT).round().max(1.0)
    )
}

/// The newer kinds' text (owner, 2026-10-23): follow-ups, shelf entries, Ross and Signals
/// crossings and at-trigger stages, Halt watcher gauges. Descriptive: a stage or an observation,
/// never "buy" or "sell".
fn panel_message(n: &Notice) -> (String, String, Value) {
    let s = &n.symbol;
    let p = title_price(n.price);
    let mut data = json!({
        "symbol": n.symbol,
        "kind": data_kind(n.kind),
        "alert_id": n.alert_id,
    });
    let (title, body) = match (n.kind, &n.detail) {
        // "YMAT · Run +20% (after Fast move 3 min ago) · $2.71", the path's own evidence below.
        (
            NoticeKind::FollowUp,
            Some(NoticeDetail::FollowUp {
                path,
                first,
                first_at_ns,
            }),
        ) => {
            let title = format!(
                "{s} · {} (after {} {}){p}",
                path.label(),
                first.label(),
                ago(*first_at_ns, n.at_ns)
            );
            let own = match path {
                AlertKind::Running => Some(NoticeKind::Running),
                AlertKind::Run => Some(NoticeKind::Run),
                AlertKind::Fast => Some(NoticeKind::Fast),
                _ => None,
            };
            // The path's own body (its reasons), as its own push would say it.
            let body = own.map_or_else(
                || n.reasons.first().cloned().unwrap_or_default(),
                |k| {
                    let (_, b, _) = runner_message(&Notice {
                        kind: k,
                        price: f64::NAN,
                        detail: None,
                        ..n.clone()
                    });
                    b
                },
            );
            (title, body)
        }
        // "In volume shelf $0.38–0.42 · about 1 in 3 hold, 1 in 2 fail". The odds come from the
        // shelf research on $0.15+ shelves; the floor moved to $0.10 (75d6418) without measuring
        // the added range, so below $0.15 the push says so instead, as Bursts already do.
        (NoticeKind::Shelf, Some(NoticeDetail::Shelf { lo, hi })) => (
            format!("{s} · Volume shelf{p}"),
            if *lo >= SHELF_ODDS_MIN {
                format!("In volume shelf {} · {}", range(*lo, *hi), shelf_odds())
            } else {
                format!(
                    "In volume shelf {} · historical odds unavailable at this price",
                    range(*lo, *hi)
                )
            },
        ),
        (
            NoticeKind::RossCrossing
            | NoticeKind::SignalsCrossing
            | NoticeKind::RossTrigger
            | NoticeKind::SignalsTrigger,
            Some(NoticeDetail::Cue(c)),
        ) => {
            data["also_signals"] = json!(c.also_signals);
            let panel = match n.kind {
                NoticeKind::RossCrossing | NoticeKind::RossTrigger => "Ross",
                _ => "Signals",
            };
            let crossing = matches!(
                n.kind,
                NoticeKind::RossCrossing | NoticeKind::SignalsCrossing
            );
            let stage = if crossing { "Crossing" } else { "At trigger" };
            let trig = c.trigger.filter(|t| t.is_finite() && *t > 0.0);
            let body = match (crossing, trig) {
                (true, Some(t)) => format!(
                    "Above the {} pullback trigger · bar close pending",
                    dollars(on_tick(t))
                ),
                (true, None) => "Above the pullback trigger · bar close pending".into(),
                (false, Some(t)) if n.price.is_finite() && n.price > 0.0 => format!(
                    "{:.1}% below the {} pullback trigger",
                    ((t / n.price - 1.0) * 100.0).max(0.0),
                    dollars(on_tick(t))
                ),
                (false, _) => "Near the pullback trigger".into(),
            };
            let both = if c.also_signals {
                " · also on Signals"
            } else {
                ""
            };
            (
                format!("{s} · {panel} · {stage}{p}"),
                format!("{body}{both}"),
            )
        }
        // "Trigger $3.42 · stop $3.21 · dipped 6.1% (31% of the move) · volume in dip 0.4×":
        // only the episode's own facts, in that order, so the clause trimming drops the dip's
        // volume first and the levels last.
        (NoticeKind::Pullback, Some(NoticeDetail::Pullback(f))) => {
            data["stage"] = json!("setup");
            data["mp5_wick"] = json!(f.wick);
            let mut parts = Vec::new();
            if let Some(t) = f.trigger {
                parts.push(format!("Trigger {}", dollars(on_tick(t))));
            }
            if let Some(st) = f.stop {
                parts.push(format!("stop {}", dollars(on_tick(st))));
            }
            match (f.dip_pct, f.retraced_pct) {
                (Some(d), Some(r)) => parts.push(format!("dipped {d:.1}% ({r:.0}% of the move)")),
                (Some(d), None) => parts.push(format!("dipped {d:.1}%")),
                _ => {}
            }
            if let Some(v) = f.dip_volume_ratio {
                parts.push(format!("volume in dip {v:.1}×"));
            }
            (format!("{s} · Pullback setting up{p}"), parts.join(" · "))
        }
        (NoticeKind::PullbackCrossing, Some(NoticeDetail::Pullback(f))) => {
            data["stage"] = json!("crossing");
            data["mp5_wick"] = json!(f.wick);
            data["also_signals"] = json!(f.also_signals);
            let body = match f.trigger {
                Some(t) => format!(
                    "Above the {} pullback trigger · bar close pending",
                    dollars(on_tick(t))
                ),
                None => "Above the pullback trigger · bar close pending".into(),
            };
            let both = if f.also_signals {
                " · also on Signals"
            } else {
                ""
            };
            (
                format!("{s} · Pullback · Crossing{p}"),
                format!("{body}{both}"),
            )
        }
        // "Buying pressure at the $2.80 band · halt risk 72/100".
        (NoticeKind::Halt, Some(NoticeDetail::Halt { bull, risk, band })) => {
            let side = if *bull { "Buying" } else { "Selling" };
            let at = if band.is_finite() && *band > 0.0 {
                format!(" at the {} LULD band", dollars(on_tick(*band)))
            } else {
                " at its LULD band".into()
            };
            (
                format!("{s} · Halt watcher{p}"),
                format!("{side} pressure building{at} · halt risk {risk}/100"),
            )
        }
        // A notice without its facts (never expected): its name and price only.
        _ => (
            format!("{s} · {}{p}", n.kind.as_str().replace('_', " ")),
            String::new(),
        ),
    };
    (title, with_price_line(body, n.price), data)
}

fn runner_message(n: &Notice) -> (String, String, Value) {
    let p = dollars(on_tick(n.price));
    let s = &n.symbol;
    if let (NoticeKind::Breakout, Some(NoticeDetail::Breakout(e))) = (n.kind, &n.detail) {
        let price = |micros: i64| {
            let mut text = format!("${}.{:06}", micros / 1_000_000, micros % 1_000_000);
            while text.ends_with('0') && text.len() - text.find('.').expect("decimal") > 3 {
                text.pop();
            }
            text
        };
        let pace = match e.volume_ratio {
            Some(ratio) => format!("{ratio:.1}× prior pace"),
            None => "prior pace unavailable".into(),
        };
        let measured = format!(
            "High {} > {} · {:.0} shares/15s · {pace}",
            price(e.high_price_micros),
            price(e.reference_price_micros),
            e.volume
        );
        return (
            format!("{s} · 15s new high since alert · {p}"),
            with_price_line(measured, n.price),
            json!({"symbol":s,"kind":data_kind(n.kind),"alert_id":n.alert_id,
                "event_id":n.event_id(),"breakout":e,"entry_context":n.reasons}),
        );
    }
    let title = match n.kind {
        NoticeKind::New => format!("Runners: {s} {p}"),
        NoticeKind::Breakout => format!("{s} breaking out {p}"),
        NoticeKind::Burst => format!("{s} · Buying burst · {p}"),
        NoticeKind::Running => format!("{s} · Running · {p}"),
        NoticeKind::Run => format!("{s} · Run +20% · {p}"),
        NoticeKind::Fast => format!("{s} · Fast move · {p}"),
        _ => format!("{s} {p}"),
    };
    let data = json!({
        "symbol": n.symbol,
        "kind": data_kind(n.kind),
        "alert_id": n.alert_id,
    });
    let first = n.reasons.first().cloned();
    let body = match n.kind {
        // "+18.2% in 6 min · typ. +13% max in 30 min, 1 in 4 reach +20% before −5%": how far
        // the burst already went, and what typically remains.
        NoticeKind::Burst => {
            let mv = match (n.from_flag_pct, n.since_start_min) {
                (Some(x), Some(m)) => moved(x, m),
                (Some(x), None) => format!("{x:+.1}% in the burst"),
                _ => "Heavy buying on the tape".into(),
            };
            if n.price >= 0.15 {
                format!("{mv} · {}", burst_odds())
            } else {
                format!("{mv} · historical odds unavailable at this price")
            }
        }
        // "+22.0% in 5 min off $1.00 · 25+ trades and $25k+": the leg and its liquidity.
        NoticeKind::Run => {
            let mv = match (n.from_flag_pct, n.since_start_min) {
                (Some(x), Some(m)) => moved(x, m),
                _ => first.clone().unwrap_or_else(|| "+20% leg".into()),
            };
            let off = n
                .reasons
                .first()
                .and_then(|r| r.split(" off ").nth(1))
                .map(|x| format!(" off {x}"))
                .unwrap_or_default();
            format!("{mv}{off} · 25+ trades, $25k+")
        }
        // "+8.4% in 60 s · 52 trades, $71k": the held move, its window and the trading in it
        // (`crate::rocket::fast::FastEntry::reasons`).
        NoticeKind::Fast => {
            let mv = match (n.from_flag_pct, n.since_start_min) {
                (Some(x), Some(m)) => format!("{x:+.1}% in {:.0} s", (m * 60.0).max(0.0)),
                _ => first.clone().unwrap_or_else(|| "Fast move".into()),
            };
            match n.reasons.get(1).and_then(|r| r.split(" in ").next()) {
                Some(t) => format!("{mv} · {t}"),
                None => mv,
            }
        }
        // "+15.3% in 60 min · $120k in 5 min · late in the move": the move off the hour's low,
        // its money, and an honest qualifier (owner, 2026-10-26): a Running entry fires after
        // about 80% of the hour's move with about 10% follow-through, against Burst's ~50% and
        // ~40% (research/post-alert-timing, lateness on 267 days).
        NoticeKind::Running => {
            let mut parts: Vec<String> = Vec::new();
            match n.from_flag_pct {
                Some(x) => parts.push(format!("{x:+.1}% in 60 min")),
                None => parts.extend(first.clone()),
            }
            parts.extend(n.reasons.iter().find(|r| r.contains(" in 5 min")).cloned());
            if parts.is_empty() {
                parts.push("Running on the tape".into());
            }
            parts.push("late in the move".into());
            parts.join(" · ")
        }
        _ => String::new(),
    };
    if !body.is_empty() {
        return (
            title,
            with_crossing(with_price_line(body, n.price), n),
            data,
        );
    }
    let mut parts: Vec<String> = Vec::new();
    if n.kind == NoticeKind::Breakout
        && let Some(p) = n.from_flag_pct
    {
        parts.push(format!("{p:+.1}% from flag"));
    }
    parts.extend(n.reasons.iter().take(2).cloned());
    if parts.is_empty() {
        parts.push("Flagged (experimental)".into());
    }
    (
        title,
        with_crossing(with_price_line(parts.join(" · "), n.price), n),
        data,
    )
}

/// A first entry right after the stock rose through the entry floor carries what the tape saw
/// below it (`crate::rocket::crossing`) on its own line, before the reference line: the line
/// that says where the move came from (WFF, 2026-10-26: four paths fired the moment it crossed
/// $0.10 and none could say). Outside [`MAX_BODY_CHARS`]: the evidence keeps its own room.
fn with_crossing(body: String, n: &Notice) -> String {
    let Some(NoticeDetail::Crossing(line)) = &n.detail else {
        return body;
    };
    match body.rsplit_once('\n') {
        Some((evidence, reference)) => format!("{evidence}\n{line}\n{reference}"),
        None => format!("{body}\n{line}"),
    }
}

/// The `runners.notice` frame for desktop and web (owner, 2026-10-23: the phone's notifications
/// on the desktop and web too). Title, body and data are the phone push's own ([`message`]), so
/// the clients cannot drift from the phone. Sent to every connection once the desk decision
/// (`super::Task::runner`) said Send; each client shows it only when its own switch for `kind`
/// is on and remembers `id`, so a reconnect never shows it twice. `id` is per kind because a
/// row's new flag and its breakout share `alertId`. Never in a snapshot: live news only.
pub fn desk_frame(n: &Notice) -> String {
    let (title, body, data) = message(n);
    json!({
        "type": "runners.notice",
        "id": n.event_id(),
        "alsoSignals": match &n.detail {
            Some(NoticeDetail::Cue(c)) => c.also_signals,
            Some(NoticeDetail::Pullback(p)) => p.also_signals,
            _ => false,
        },
        "alertId": n.alert_id,
        "symbol": n.symbol,
        "kind": data_kind(n.kind),
        "title": title,
        "body": body,
        "data": data,
        "at": n.at_ns.div_euclid(1_000_000),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desk_frame_carries_the_phone_text() {
        for kind in [
            NoticeKind::New,
            NoticeKind::Breakout,
            NoticeKind::Burst,
            NoticeKind::Running,
            NoticeKind::Run,
            NoticeKind::Fast,
        ] {
            let n = Notice {
                kind,
                alert_id: "2026-10-23-CJMB-7".into(),
                symbol: "CJMB".into(),
                at_ns: 1_761_200_000_123_000_000,
                price: 2.41,
                from_flag_pct: Some(8.44),
                reasons: vec!["$120k in 5 min".into(), "52 trades, $71k in 60 s".into()],
                since_start_min: Some(1.0),
                exec_ns: None,
                detail: None,
            };
            let (title, body, data) = message(&n);
            let f: Value = serde_json::from_str(&desk_frame(&n)).unwrap();
            assert_eq!(f["type"], "runners.notice");
            assert_eq!(f["title"], title.as_str());
            assert_eq!(f["body"], body.as_str());
            assert_eq!(f["data"], data);
            assert_eq!(f["kind"], data_kind(kind));
            assert_eq!(f["symbol"], "CJMB");
            assert_eq!(f["alertId"], "2026-10-23-CJMB-7");
            let id = if kind == NoticeKind::Breakout {
                // One row can break out more than once: the barrier's second is in the id.
                "runners-breakout-2026-10-23-CJMB-7-1761200000".to_string()
            } else {
                format!("runners-{}-2026-10-23-CJMB-7", kind.as_str())
            };
            assert_eq!(f["id"], id);
            assert_eq!(f["at"], 1_761_200_000_123_i64);
        }
    }
    use crate::alerts::model::QuietHours;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    const MIN: i64 = 60_000;

    /// A fresh event id per decision (for tests about the other limits, not the dedupe).
    fn uid() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        format!("e{}", N.fetch_add(1, Ordering::Relaxed))
    }

    #[test]
    fn the_only_limits_are_dedupe_devices_quiet_hours_and_staleness() {
        let mut d = RunnersDelivery::in_memory();
        let prefs = Prefs::default();
        let go = |d: &mut RunnersDelivery, at: i64, now: i64| {
            d.decide(&uid(), 1, &prefs, at, now, day(22), 8 * 60)
        };
        assert_eq!(
            d.decide(&uid(), 0, &prefs, 0, 0, day(22), 480),
            Decision::Held(Hold::NoDevice)
        );
        assert_eq!(go(&mut d, 0, 0), Decision::Send);
        // Old news is not pushed: over 3 minutes is a catch-up after a stall. A notice decided at
        // a 15 s barrier and queued behind a busy task (here 2 min 59 s) still goes out.
        assert_eq!(MAX_AGE_MS, 3 * MIN);
        assert_eq!(go(&mut d, 0, 4 * MIN), Decision::Held(Hold::Stale));
        assert_eq!(go(&mut d, 0, MAX_AGE_MS + 1), Decision::Held(Hold::Stale));
        assert_eq!(go(&mut d, MIN, MIN + MAX_AGE_MS), Decision::Send);
        assert_eq!(go(&mut d, 0, 2 * MIN + 59_000), Decision::Send);
        // No daily cap and no per-stock gap: hundreds of events in the same second all go out.
        for _ in 0..400 {
            assert_eq!(go(&mut d, 13 * MIN, 13 * MIN), Decision::Send);
        }
        assert!(d.sent_today(day(22)) > 400, "still counted for the logs");
        // A new ET day.
        assert_eq!(
            d.decide(&uid(), 1, &prefs, 15 * MIN, 15 * MIN, day(23), 480),
            Decision::Send
        );
        assert_eq!(d.sent_today(day(22)), 0);
        // The price alerts' quiet hours hold Runners too.
        let quiet = Prefs {
            quiet: Some(QuietHours {
                start_min: 22 * 60,
                end_min: 7 * 60,
            }),
            ..Prefs::default()
        };
        assert_eq!(
            d.decide(&uid(), 1, &quiet, 20 * MIN, 20 * MIN, day(23), 6 * 60),
            Decision::Held(Hold::QuietHours)
        );
    }

    #[test]
    fn an_event_is_never_sent_twice_even_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = Prefs::default();
        let mut d = RunnersDelivery::open(dir.path());
        let id = "runners-new-2026-10-22-A-1";
        assert_eq!(d.decide(id, 1, &prefs, 0, 0, day(22), 480), Decision::Send);
        assert_eq!(
            d.decide(id, 1, &prefs, MIN, MIN, day(22), 481),
            Decision::Held(Hold::Duplicate)
        );
        let mut back = RunnersDelivery::open(dir.path());
        assert_eq!(back.sent_today(day(22)), 1);
        assert_eq!(
            back.decide(id, 1, &prefs, MIN, MIN, day(22), 481),
            Decision::Held(Hold::Duplicate),
            "a redeploy does not forget what was sent"
        );
        // Another event on the same stock a second later is sent.
        assert_eq!(
            back.decide(
                "runners-breakout-2026-10-22-A-1",
                1,
                &prefs,
                MIN,
                MIN,
                day(22),
                481
            ),
            Decision::Send
        );
    }

    /// A file written before 2026-10-30 (with the removed gaps' and merge's memory) still opens
    /// and keeps its dedupe.
    #[test]
    fn an_old_memory_file_still_opens() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE),
            r##"{"day":"2026-10-30","sent":2,"last_by_symbol":{"TNMG":1},"last_by_kind":{"TNMG new":1},
                "sent_ids":["runners-new-TNMG-1"],"first_push":{"TNMG":{"at":1,"event_id":"runners-new-TNMG-1","notice":"runners.new","recipients":["#0"]}}}"##,
        )
        .unwrap();
        let mut d = RunnersDelivery::open(dir.path());
        assert_eq!(d.sent_today(day(30)), 2);
        let prefs = Prefs::default();
        assert_eq!(
            d.decide("runners-new-TNMG-1", 1, &prefs, 0, 0, day(30), 470),
            Decision::Held(Hold::Duplicate)
        );
        assert_eq!(
            d.decide("runners-breakout-TNMG-1", 1, &prefs, 0, 0, day(30), 470),
            Decision::Send
        );
    }

    #[test]
    fn messages() {
        let mut n = Notice {
            kind: NoticeKind::New,
            alert_id: "2026-10-22-CJMB-260".into(),
            symbol: "CJMB".into(),
            at_ns: 0,
            price: 2.4512,
            from_flag_pct: Some(0.0),
            reasons: vec!["volume 9x pace".into(), "new high".into(), "third".into()],
            since_start_min: None,
            exec_ns: None,
            detail: None,
        };
        let (t, b, data) = message(&n);
        assert_eq!(t, "Runners: CJMB $2.45");
        assert_eq!(
            b,
            "volume 9x pace · new high
Alert $2.45 · +5% = $2.57"
        );
        assert_eq!(
            data,
            json!({"symbol": "CJMB", "kind": "runners.new", "alert_id": "2026-10-22-CJMB-260"})
        );
        n.kind = NoticeKind::Breakout;
        n.price = 0.3;
        n.from_flag_pct = Some(12.34);
        n.reasons.clear();
        let (t, b, data) = message(&n);
        assert_eq!(t, "CJMB breaking out $0.3000");
        assert_eq!(
            b,
            "+12.3% from flag
Alert $0.3000 · +5% = $0.3150"
        );
        assert_eq!(data["kind"], "runners.breakout");
        n.kind = NoticeKind::New;
        assert_eq!(
            message(&n).1,
            "Flagged (experimental)
Alert $0.3000 · +5% = $0.3150"
        );
        // A buying burst: source, price, the move and its window, the research odds; a tap
        // opens the chart (runners.*).
        n.kind = NoticeKind::Burst;
        n.price = 2.1;
        n.from_flag_pct = Some(18.24);
        n.since_start_min = Some(6.4);
        n.reasons = vec!["$ rate 6.4× base".into(), "12 new highs in 15 s".into()];
        let (t, b, data) = message(&n);
        assert_eq!(t, "CJMB · Buying burst · $2.10");
        assert_eq!(
            b,
            "+18.2% in 6 min · typ. +13% max in 30 min, 1 in 4 reach +20% before −5%
Alert $2.10 · +5% = $2.21"
        );
        assert_eq!(data["kind"], "runners.burst");
        assert!(!t.to_lowercase().contains("buy ") && !b.to_lowercase().contains("buy "));
        // A "Run +20%" entry: the leg, its window and start, its liquidity floor.
        n.kind = NoticeKind::Run;
        n.price = 1.22;
        n.from_flag_pct = Some(22.0);
        n.since_start_min = Some(5.0);
        n.reasons = crate::rocket::run::reasons(&crate::zones::Run {
            start_ns: 0,
            start_price: 1.0,
            plus20_ns: 5 * 60_000_000_000,
            close: 1.22,
            exec_ns: 0,
            early: false,
        });
        let (t, b, data) = message(&n);
        assert_eq!(t, "CJMB · Run +20% · $1.22");
        assert_eq!(
            b,
            "+22.0% in 5 min off $1.00 · 25+ trades, $25k+
Alert $1.22 · +5% = $1.28"
        );
        assert_eq!(data["kind"], "runners.run");
        // A running entry: the move off the hour's low and its money.
        n.kind = NoticeKind::Running;
        n.price = 11.49;
        n.from_flag_pct = Some(15.26);
        n.since_start_min = None;
        n.reasons = vec![
            "+15% off the 60-min low (needs 12%)".into(),
            "new 1-hour high".into(),
            "$120k in 5 min".into(),
        ];
        let (t, b, data) = message(&n);
        assert_eq!(t, "CJMB · Running · $11.49");
        assert_eq!(
            b,
            "+15.3% in 60 min · $120k in 5 min · late in the move
Alert $11.49 · +5% = $12.06"
        );
        assert_eq!(data["kind"], "runners.running");
        assert!(t.chars().count() + b.chars().count() < 110, "{t} / {b}");
        // A fast move: the held move, its window, the trading in it.
        n.kind = NoticeKind::Fast;
        n.price = 2.41;
        n.from_flag_pct = Some(8.44);
        n.since_start_min = Some(1.0);
        n.reasons = crate::rocket::fast::FastEntry {
            secs: 60,
            price: 2.41,
            held: 2.40,
            low: 2.213,
            gain: 0.0844,
            trades: 52,
            dollars: 71_200.0,
        }
        .reasons();
        let (t, b, data) = message(&n);
        assert_eq!(t, "CJMB · Fast move · $2.41");
        assert_eq!(
            b,
            "+8.4% in 60 s · 52 trades, $71k
Alert $2.41 · +5% = $2.53"
        );
        assert_eq!(data["kind"], "runners.fast");
    }

    #[test]
    fn sub_fifteen_cent_bursts_do_not_claim_the_historical_odds() {
        let mut n = Notice::test(NoticeKind::Burst, "PENY");
        n.price = 0.12;
        let (_, body, _) = message(&n);
        assert!(body.contains("historical odds unavailable at this price"));
        assert!(!body.contains("typ. "));
        n.price = 0.15;
        let (_, body, _) = message(&n);
        assert!(body.contains("typ. "));
        assert!(!body.contains("odds unavailable"));
    }

    #[test]
    fn the_reference_line_is_on_tick() {
        // The YMAT case (owner, 2026-10-23).
        assert_eq!(price_line(2.41), "Alert $2.41 · +5% = $2.53");
        // Sub-dollar on $0.0001; the +5% crosses $1 and goes to cents.
        assert_eq!(price_line(0.9999), "Alert $0.9999 · +5% = $1.05");
        assert_eq!(price_line(1.0), "Alert $1.00 · +5% = $1.05");
        // 25.725 exactly: half up, not the binary 25.7249… down.
        assert_eq!(price_line(24.5), "Alert $24.50 · +5% = $25.73");
        assert_eq!(price_line(0.4321), "Alert $0.4321 · +5% = $0.4537");
        // An alert price off tick is put on it first: $0.99996 is $1.00.
        assert_eq!(price_line(0.99996), "Alert $1.00 · +5% = $1.05");
        // Neutral wording: a reference, never advice.
        let l = price_line(2.41).to_lowercase();
        assert!(!l.contains("buy") && !l.contains("limit") && !l.contains("sell"));
    }

    #[test]
    fn a_long_body_drops_evidence_not_the_reference_line() {
        let n = Notice {
            kind: NoticeKind::New,
            alert_id: "a".into(),
            symbol: "YMAT".into(),
            at_ns: 0,
            price: 2.41,
            from_flag_pct: None,
            reasons: vec!["x".repeat(60), "y".repeat(60)],
            since_start_min: None,
            exec_ns: None,
            detail: None,
        };
        let (_, b, _) = message(&n);
        assert_eq!(b, format!("{}\nAlert $2.41 · +5% = $2.53", "x".repeat(60)));
        assert!(b.chars().count() <= MAX_BODY_CHARS);
        // Every kind's body ends with the line.
        for kind in [
            NoticeKind::New,
            NoticeKind::Breakout,
            NoticeKind::Burst,
            NoticeKind::Running,
            NoticeKind::Run,
            NoticeKind::Fast,
        ] {
            let n = Notice {
                kind,
                from_flag_pct: Some(8.0),
                since_start_min: Some(1.0),
                reasons: vec!["$120k in 5 min".into(), "52 trades, $71k in 60 s".into()],
                ..n.clone()
            };
            let (_, b, _) = message(&n);
            assert!(b.ends_with("\nAlert $2.41 · +5% = $2.53"), "{kind:?}: {b}");
            assert!(b.chars().count() <= MAX_BODY_CHARS, "{kind:?}: {b}");
        }
    }

    /// A notice of every kind as its source makes it (the newer kinds with their facts).
    fn sample(kind: NoticeKind) -> Notice {
        use crate::rocket::CueNotice;
        let mut n = Notice::test(kind, "YMAT");
        n.at_ns = 1_761_200_000_000_000_000;
        n.price = 2.71;
        n.from_flag_pct = Some(22.0);
        n.since_start_min = Some(5.0);
        n.reasons = vec!["+22% in 5 min off $2.22".into(), "25+ trades".into()];
        n.detail = match kind {
            NoticeKind::FollowUp => Some(NoticeDetail::FollowUp {
                path: AlertKind::Run,
                first: AlertKind::Fast,
                first_at_ns: n.at_ns - 3 * 60_000_000_000,
            }),
            NoticeKind::Shelf => Some(NoticeDetail::Shelf { lo: 0.38, hi: 0.42 }),
            NoticeKind::RossCrossing
            | NoticeKind::SignalsCrossing
            | NoticeKind::RossTrigger
            | NoticeKind::SignalsTrigger => Some(NoticeDetail::Cue(CueNotice {
                episode_id: "run:2026-10-23:YMAT:micro_pullback:3".into(),
                trigger: Some(2.75),
                also_signals: kind == NoticeKind::RossCrossing,
            })),
            NoticeKind::Halt => Some(NoticeDetail::Halt {
                bull: true,
                risk: 72,
                band: 2.8,
            }),
            NoticeKind::Pullback | NoticeKind::PullbackCrossing => {
                Some(NoticeDetail::Pullback(crate::rocket::PullbackNotice {
                    episode_id: "run:2026-10-23:YMAT:micro_pullback:4".into(),
                    trigger: Some(2.75),
                    stop: Some(2.58),
                    dip_pct: Some(6.14),
                    retraced_pct: Some(31.0),
                    dip_volume_ratio: Some(0.4),
                    wick: None,
                    also_signals: kind == NoticeKind::PullbackCrossing,
                }))
            }
            _ => None,
        };
        n
    }

    /// Review of 75d6418: a shelf under $0.15 (outside the shelf research) quotes no odds.
    #[test]
    fn shelf_odds_only_where_they_were_measured() {
        let mut n = sample(NoticeKind::Shelf);
        n.detail = Some(NoticeDetail::Shelf { lo: 0.12, hi: 0.13 });
        let (_, body, _) = message(&n);
        assert!(body.contains("historical odds unavailable"), "{body}");
        n.detail = Some(NoticeDetail::Shelf { lo: 0.38, hi: 0.42 });
        let (_, body, _) = message(&n);
        assert!(body.contains("1 in 3"), "{body}");
    }

    /// Desktop and web carry exactly the phone text for every kind, with the event id, the
    /// copy-ticker data and the reference line; nothing is worded as advice.
    #[test]
    fn every_kind_has_one_text_on_every_surface() {
        for kind in NoticeKind::ALL {
            let n = sample(kind);
            let (title, body, data) = message(&n);
            let f: Value = serde_json::from_str(&desk_frame(&n)).unwrap();
            assert_eq!(f["title"], title.as_str());
            assert_eq!(f["body"], body.as_str());
            assert_eq!(f["data"], data);
            assert_eq!(f["id"], n.event_id());
            assert_eq!(f["kind"], data_kind(kind));
            assert_eq!(data["symbol"], "YMAT");
            assert_eq!(data["alert_id"], n.alert_id.as_str());
            assert!(title.contains("YMAT"), "{kind:?}: {title}");
            assert!(
                body.ends_with("Alert $2.71 · +5% = $2.85"),
                "{kind:?}: {body}"
            );
            assert!(body.chars().count() <= MAX_BODY_CHARS, "{kind:?}: {body}");
            let all = format!("{title} {body}").to_lowercase();
            assert!(
                !all.contains("buy ") && !all.contains("sell ") && !all.contains("enter"),
                "{kind:?}: {all}"
            );
        }
        let t = |k| {
            let (t, b, _) = message(&sample(k));
            (t, b.lines().next().unwrap_or_default().to_string())
        };
        assert_eq!(
            t(NoticeKind::FollowUp),
            (
                "YMAT · Run +20% (after Fast move 3 min ago) · $2.71".into(),
                "+22.0% in 5 min off $2.22 · 25+ trades, $25k+".into()
            )
        );
        assert_eq!(
            t(NoticeKind::Shelf),
            (
                "YMAT · Volume shelf · $2.71".into(),
                "In volume shelf $0.38–0.42 · about 1 in 3 hold, 1 in 2 fail".into()
            )
        );
        assert_eq!(
            t(NoticeKind::RossCrossing),
            (
                "YMAT · Ross · Crossing · $2.71".into(),
                "Above the $2.75 pullback trigger · bar close pending · also on Signals".into()
            )
        );
        assert_eq!(
            t(NoticeKind::SignalsCrossing).0,
            "YMAT · Signals · Crossing · $2.71"
        );
        assert_eq!(
            t(NoticeKind::RossTrigger),
            (
                "YMAT · Ross · At trigger · $2.71".into(),
                "1.5% below the $2.75 pullback trigger".into()
            )
        );
        assert_eq!(
            t(NoticeKind::Halt),
            (
                "YMAT · Halt watcher · $2.71".into(),
                "Buying pressure building at the $2.80 LULD band · halt risk 72/100".into()
            )
        );
        assert_eq!(
            t(NoticeKind::Pullback),
            (
                "YMAT · Pullback setting up · $2.71".into(),
                "Trigger $2.75 · stop $2.58 · dipped 6.1% (31% of the move) · volume in dip 0.4×"
                    .into()
            )
        );
        assert_eq!(
            t(NoticeKind::PullbackCrossing),
            (
                "YMAT · Pullback · Crossing · $2.71".into(),
                "Above the $2.75 pullback trigger · bar close pending · also on Signals".into()
            )
        );
        let (_, _, setup) = message(&sample(NoticeKind::Pullback));
        assert_eq!(
            (setup["kind"].as_str(), setup["stage"].as_str()),
            (Some("runners.pullback"), Some("setup"))
        );
        let f: Value =
            serde_json::from_str(&desk_frame(&sample(NoticeKind::PullbackCrossing))).unwrap();
        assert_eq!(
            (f["alsoSignals"].clone(), f["data"]["stage"].clone()),
            (json!(true), json!("crossing"))
        );
        let f: Value =
            serde_json::from_str(&desk_frame(&sample(NoticeKind::RossCrossing))).unwrap();
        assert_eq!(f["alsoSignals"], true);
        assert_eq!(f["data"]["also_signals"], true);
    }

    #[test]
    fn pullback_wick_data_is_observational_and_identical_on_phone_and_desk() {
        for kind in [NoticeKind::Pullback, NoticeKind::PullbackCrossing] {
            let mut n = sample(kind);
            let (_, original_body, _) = message(&n);
            let Some(NoticeDetail::Pullback(p)) = &mut n.detail else {
                panic!("pullback facts")
            };
            p.wick = Some(stock_context::MicroWick::measure(
                Some([105, 120, 100, 108]),
                15,
                32,
            ));
            let (_, body, data) = message(&n);
            assert_eq!(
                body, original_body,
                "observation does not change the notice"
            );
            assert_eq!(data["mp5_wick"]["status"], "flagged");
            assert_eq!(data["mp5_wick"]["known_at"], 32);
            let desk: Value = serde_json::from_str(&desk_frame(&n)).unwrap();
            assert_eq!(desk["data"], data);
        }
    }

    /// Owner, 2026-10-30 ("I don't want to keep missing alerts because they weren't delivered to
    /// my phone"): every distinct event on one stock is sent, however close together. These are
    /// the sequences the per-stock gap and the merge used to hold back.
    #[test]
    fn every_distinct_event_on_one_stock_is_sent() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = Prefs::default();
        let s = 1_000;
        let t0 = 10 * MIN;
        let mut d = RunnersDelivery::open(dir.path());
        let mut go = |kind: NoticeKind, alert_id: &str, now: i64| {
            let mut n = Notice::test(kind, "TNMG");
            n.alert_id = alert_id.into();
            d.decide(&n.event_id(), 1, &prefs, now, now, day(30), 7 * 60 + 49)
        };
        // TNMG: a pullback notice, then a follow-up 13 s later (7:49:45, held as symbol_limit).
        assert_eq!(go(NoticeKind::Pullback, "ep-7", t0), Decision::Send);
        assert_eq!(
            go(NoticeKind::FollowUp, "2026-10-30-TNMG-1-run", t0 + 13 * s),
            Decision::Send
        );
        // TNMG 7:19: a flag, then its "15s new high since alert" breakout inside 30 s (held as
        // merged that day).
        let t1 = t0 + 5 * MIN;
        assert_eq!(go(NoticeKind::New, "2026-10-30-TNMG-2", t1), Decision::Send);
        assert_eq!(
            go(NoticeKind::Breakout, "2026-10-30-TNMG-2", t1 + 20 * s),
            Decision::Send
        );
        // WFF 2026-10-26: Running, Fast move and Breakout within 14 s: all three.
        let t2 = t1 + 5 * MIN;
        assert_eq!(
            go(NoticeKind::Running, "2026-10-30-TNMG-3", t2),
            Decision::Send
        );
        assert_eq!(
            go(NoticeKind::Fast, "2026-10-30-TNMG-4", t2 + s),
            Decision::Send
        );
        assert_eq!(
            go(NoticeKind::Breakout, "2026-10-30-TNMG-3", t2 + 14 * s),
            Decision::Send
        );
        // New evidence of every kind straight after: a shelf entry, crossings, a halt gauge.
        for (k, id) in [
            (NoticeKind::Shelf, "2026-10-30-TNMG-3-z1"),
            (NoticeKind::RossCrossing, "ep-8"),
            (NoticeKind::PullbackCrossing, "ep-7"),
            (NoticeKind::Halt, "TNMG-h1"),
        ] {
            assert_eq!(go(k, id, t2 + 15 * s), Decision::Send, "{k:?}");
        }
        // The same event again is still a duplicate.
        assert_eq!(
            go(NoticeKind::Breakout, "2026-10-30-TNMG-3", t2 + 16 * s),
            Decision::Held(Hold::Duplicate)
        );
    }
}
