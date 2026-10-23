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
//! - **Limits**, checked in this order (the first that applies holds it back; logged, never
//!   sent later):
//!   1. no phone opted in for this kind → `no_device`;
//!   2. the price alerts' quiet hours (`alerts.prefs`, ET wall time) → `quiet_hours`;
//!   3. the notice is over [`MAX_AGE_MS`] old (a catch-up after a stall) → `stale`;
//!   4. the same stock got a Runners push less than [`SYMBOL_GAP_MS`] ago, whatever the kind
//!      (a burst: [`BURST_GAP_MS`], so a burst right after a run or running push still goes
//!      out) → `symbol_limit`;
//!   5. [`DAILY_CAP`] Runners pushes already went out this ET day → `cap`, by priority: bursts
//!      and new flags are never dropped by the cap (they still count); breakouts, running, run
//!      and fast-move entries are. With ~50 running, ~25 run, ~20 burst and up to ~100 fast
//!      pushes on a typical day the cap
//!      is a runaway guard, not a budget.
//!
//! - **Desktop and web** (owner, 2026-10-23: the phone's notifications there too): every notice
//!   is decided a second time for the open desktop and web clients ([`desk_frame`]), with the
//!   same limits but its own count and gaps (`runners-desk.json`). Their "devices" are the
//!   connections that sent `runners.notify` with this kind on (`super::DeskKinds`, the phone's
//!   defaults); a Send goes to every socket as a `runners.notice` frame carrying [`message`]'s
//!   title and body, and each client shows it as a system notification if its own switch is on.
//!   Separate counts so that a kind only the phone wants never spends a desk gap, and the reverse.
//!
//! - **Latency** (target: under 5 s from the triggering trade to Expo): a Runners push skips the
//!   push task's batch window ([`super::push`]) and, once Expo took it, logs `runners push:
//!   latency` with `exec_ns` (the triggering print, when the source knows it: a "Run +20%" row),
//!   `decided_ns` (the board's row), `queued_ns` (handed to the push task) and `expo_ns`, and the
//!   three legs in ms. Where the sources decide: a "Run +20%" row within about a second of the
//!   print that makes it (`crate::zones`, "Early fire"; 1–2 s with the 2-print guard), a burst
//!   at 5 s boundaries checked 1 s after (up to ~6 s after the print), a running entry and an R2
//!   flag at 15 s barriers (up to ~15 s after the print, plus the ≤ 1 s tick).
//!
//!   The count and each stock's last push are kept in `runners-push.json` beside the devices, so
//!   a redeploy at 09:00 does not hand out a fresh cap. They are separate from the price alerts'
//!   phone budget: a busy Runners morning must not silence a price alert the owner set himself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::model::Prefs;
use crate::rocket::{Notice, NoticeKind};

/// At most one Runners push per stock per 10 minutes (every kind but a burst).
pub const SYMBOL_GAP_MS: i64 = 10 * 60_000;
/// A burst's gap after the same stock's last Runners push: shorter, so heavy buying confirmed
/// minutes after a run or running push still reaches the phone.
pub const BURST_GAP_MS: i64 = 2 * 60_000;
/// Breakout, running and run pushes stop once this many Runners pushes went out in the ET
/// calendar day; bursts and new flags never do (module docs, "Limits").
pub const DAILY_CAP: u32 = 120;
/// A notice older than this when it reaches the task is not pushed: news about a move minutes
/// ago is noise on a lock screen.
pub const MAX_AGE_MS: i64 = 3 * 60_000;
const FILE: &str = "runners-push.json";
/// The desktop and web notifications' own count and gaps ([`desk_frame`]): kept apart from the
/// phones' so a notice the phones held (none opted in) still reaches an open desktop, and the
/// reverse; the limits themselves are the same.
pub const DESK_FILE: &str = "runners-desk.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Send,
    Held(Hold),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    NoDevice,
    QuietHours,
    Stale,
    SymbolLimit,
    Cap,
}

impl Hold {
    pub fn code(self) -> &'static str {
        match self {
            Hold::NoDevice => "no_device",
            Hold::QuietHours => "quiet_hours",
            Hold::Stale => "stale",
            Hold::SymbolLimit => "symbol_limit",
            Hold::Cap => "cap",
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
    #[serde(default)]
    last_by_symbol: HashMap<String, i64>,
}

/// The Runners side's memory: today's count and each stock's last push, persisted.
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

    /// The decision for one notice of `kind` (module docs, "Limits"). `devices` = phones opted in
    /// for its kind; `at` = when it happened, `now` = now (epoch ms); `today` / `et_minute` = the
    /// ET day and minute of the day at `now`.
    #[allow(clippy::too_many_arguments)]
    pub fn decide(
        &mut self,
        kind: NoticeKind,
        symbol: &str,
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
            // Yesterday's stocks cannot hold anything back today (10 minutes < a night).
            self.s.last_by_symbol.clear();
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
        let gap = if kind == NoticeKind::Burst {
            BURST_GAP_MS
        } else {
            SYMBOL_GAP_MS
        };
        if self
            .s
            .last_by_symbol
            .get(symbol)
            .is_some_and(|t| now - t < gap)
        {
            return Decision::Held(Hold::SymbolLimit);
        }
        let never_capped = matches!(kind, NoticeKind::Burst | NoticeKind::New);
        if self.s.sent >= DAILY_CAP && !never_capped {
            return Decision::Held(Hold::Cap);
        }
        self.s.sent += 1;
        self.s.last_by_symbol.insert(symbol.to_owned(), now);
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

/// The `kind` in the push's data (apps/mobile src/notifications/route.ts reads it).
pub fn data_kind(kind: NoticeKind) -> &'static str {
    match kind {
        NoticeKind::New => "runners.new",
        NoticeKind::Breakout => "runners.breakout",
        NoticeKind::Burst => "runners.burst",
        NoticeKind::Running => "runners.running",
        NoticeKind::Run => "runners.run",
        NoticeKind::Fast => "runners.fast",
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

/// Title, body and data of the push for one notice (module docs, "Text"). The data's `symbol` is
/// what the phone opens the chart for on a tap.
pub fn message(n: &Notice) -> (String, String, Value) {
    let p = dollars(on_tick(n.price));
    let s = &n.symbol;
    let title = match n.kind {
        NoticeKind::New => format!("Runners: {s} {p}"),
        NoticeKind::Breakout => format!("{s} breaking out {p}"),
        NoticeKind::Burst => format!("{s} · Buying burst · {p}"),
        NoticeKind::Running => format!("{s} · Running · {p}"),
        NoticeKind::Run => format!("{s} · Run +20% · {p}"),
        NoticeKind::Fast => format!("{s} · Fast move · {p}"),
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
            format!("{mv} · {}", burst_odds())
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
        // "+15.3% in 60 min · $120k in 5 min": the move off the hour's low and its money.
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
            parts.join(" · ")
        }
        NoticeKind::New | NoticeKind::Breakout => String::new(),
    };
    if !body.is_empty() {
        return (title, with_price_line(body, n.price), data);
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
    (title, with_price_line(parts.join(" · "), n.price), data)
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
        "id": format!("runners-{}-{}", n.kind.as_str(), n.alert_id),
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
            assert_eq!(
                f["id"],
                format!("runners-{}-2026-10-23-CJMB-7", kind.as_str())
            );
            assert_eq!(f["at"], 1_761_200_000_123_i64);
        }
    }
    use crate::alerts::model::QuietHours;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    const MIN: i64 = 60_000;

    #[test]
    fn cooldown_cap_quiet_hours_and_staleness() {
        let mut d = RunnersDelivery::in_memory();
        let prefs = Prefs::default();
        let run = NoticeKind::Run;
        let go = |d: &mut RunnersDelivery, s: &str, now: i64| {
            d.decide(run, s, 1, &prefs, now, now, day(22), 8 * 60)
        };
        assert_eq!(
            d.decide(run, "CJMB", 0, &prefs, 0, 0, day(22), 480),
            Decision::Held(Hold::NoDevice)
        );
        assert_eq!(go(&mut d, "CJMB", 0), Decision::Send);
        // The same stock within 10 minutes (a breakout right after its flag): held.
        assert_eq!(
            go(&mut d, "CJMB", 9 * MIN),
            Decision::Held(Hold::SymbolLimit)
        );
        assert_eq!(go(&mut d, "CJMB", 10 * MIN), Decision::Send);
        // A burst 2 minutes after the same stock's run push still goes out; 1 minute does not.
        let burst = |d: &mut RunnersDelivery, s: &str, now: i64| {
            d.decide(NoticeKind::Burst, s, 1, &prefs, now, now, day(22), 8 * 60)
        };
        assert_eq!(
            burst(&mut d, "CJMB", 11 * MIN),
            Decision::Held(Hold::SymbolLimit)
        );
        assert_eq!(burst(&mut d, "CJMB", 12 * MIN), Decision::Send);
        // Old news is not pushed.
        assert_eq!(
            d.decide(run, "OLD", 1, &prefs, 0, 4 * MIN, day(22), 480),
            Decision::Held(Hold::Stale)
        );
        // The daily cap of 120: running and run pushes stop there, bursts and new flags do not.
        for i in 0..(DAILY_CAP - 3) {
            assert_eq!(go(&mut d, &format!("S{i}"), 13 * MIN), Decision::Send);
        }
        assert_eq!(d.sent_today(day(22)), DAILY_CAP);
        assert_eq!(go(&mut d, "LATE", 14 * MIN), Decision::Held(Hold::Cap));
        let running = d.decide(
            NoticeKind::Running,
            "LATE",
            1,
            &prefs,
            14 * MIN,
            14 * MIN,
            day(22),
            480,
        );
        assert_eq!(running, Decision::Held(Hold::Cap));
        let breakout = d.decide(
            NoticeKind::Breakout,
            "LATE",
            1,
            &prefs,
            14 * MIN,
            14 * MIN,
            day(22),
            480,
        );
        assert_eq!(breakout, Decision::Held(Hold::Cap));
        // A fast move is droppable first too, with running and run.
        let fast = d.decide(
            NoticeKind::Fast,
            "FAST",
            1,
            &prefs,
            14 * MIN,
            14 * MIN,
            day(22),
            480,
        );
        assert_eq!(fast, Decision::Held(Hold::Cap));
        assert_eq!(burst(&mut d, "BURST", 14 * MIN), Decision::Send);
        let new = d.decide(
            NoticeKind::New,
            "FLAG",
            1,
            &prefs,
            14 * MIN,
            14 * MIN,
            day(22),
            480,
        );
        assert_eq!(new, Decision::Send);
        assert_eq!(d.sent_today(day(22)), DAILY_CAP + 2, "they still count");
        // A new ET day.
        assert_eq!(
            d.decide(run, "LATE", 1, &prefs, 15 * MIN, 15 * MIN, day(23), 480),
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
            d.decide(run, "Q", 1, &quiet, 20 * MIN, 20 * MIN, day(23), 6 * 60),
            Decision::Held(Hold::QuietHours)
        );
    }

    #[test]
    fn the_count_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = Prefs::default();
        let mut d = RunnersDelivery::open(dir.path());
        let k = NoticeKind::New;
        assert_eq!(
            d.decide(k, "A", 1, &prefs, 0, 0, day(22), 480),
            Decision::Send
        );
        let mut back = RunnersDelivery::open(dir.path());
        assert_eq!(back.sent_today(day(22)), 1);
        assert_eq!(
            back.decide(k, "A", 1, &prefs, MIN, MIN, day(22), 481),
            Decision::Held(Hold::SymbolLimit),
            "a redeploy does not forget the cooldown"
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
            "+15.3% in 60 min · $120k in 5 min
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
}
