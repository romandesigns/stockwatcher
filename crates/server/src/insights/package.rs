//! The ticker-day package from the replay's facts and the day's records: day stats, the notice
//! rows with their outcomes, the scorecard, halts and LULD, and the missed setups. Pure: every
//! input is handed in, so it is tested without files.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;
use market_events::session::SessionKind;
use serde_json::{Map, Value, json};
use stock_context::{DetectionTransition, State};

use chart_engine::wire::CandleWire;

use super::replay::{Print, ReplayOut};
use crate::cue_notices::{SetupDecision, SetupVerdict};

const NS_PER_MS: i64 = 1_000_000;

/// Micro-dollars to dollars for the wire.
fn dollars(m: i64) -> f64 {
    m as f64 / 1_000_000.0
}

/// A gain in percent, one decimal (the page prints it as is).
fn pct(to: i64, from: i64) -> Option<f64> {
    (from > 0).then(|| ((to as f64 / from as f64 - 1.0) * 1000.0).round() / 10.0)
}

fn ms(ns: i64) -> i64 {
    ns.div_euclid(NS_PER_MS)
}

/// One float figure the server held, as of the day.
#[derive(Clone, Debug, PartialEq)]
pub struct FloatFact {
    pub shares: u64,
    /// When it was retrieved (UTC ns): the "as of" the server can actually claim.
    pub retrieved_at: i64,
    pub provider: String,
    pub quality: Vec<String>,
}

/// What a live episode recorded in the day's cue transition log.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveEpisode {
    pub warm: bool,
    pub trigger: Option<i64>,
    pub stop: Option<i64>,
    pub armed_at: Option<i64>,
}

/// Everything besides the replay that the package is made of.
#[derive(Clone, Debug, Default)]
pub struct DayRecords {
    pub symbol: String,
    pub day: NaiveDate,
    pub name: Option<String>,
    /// The previous close (its date, micros), split-adjusted like the chart header's.
    pub prev_close: Option<(NaiveDate, i64)>,
    pub float: Option<FloatFact>,
    /// The symbol's notice rows that day (grouped ledger items with their status), any order.
    pub notices: Vec<Value>,
    /// The ledger file of that day exists (or today's is held in memory): an armed setup absent
    /// from it was really not notified.
    pub ledger_known: bool,
    pub live: HashMap<String, LiveEpisode>,
    pub boots: Vec<i64>,
    /// Where the tape came from (`live-original`, `live`, `final`).
    pub tape: &'static str,
    /// The day is still running (today): outcomes are so far.
    pub ongoing: bool,
}

/// The episode an event id names, for the cue notices (`runners-<kind>-<episode id>`).
pub fn episode_of(event_id: &str) -> Option<(&str, &str)> {
    let rest = event_id.strip_prefix("runners-")?;
    let (kind, ep) = rest.split_once('-')?;
    matches!(
        kind,
        "pullback"
            | "pullback_crossing"
            | "ross_crossing"
            | "signals_crossing"
            | "ross_trigger"
            | "signals_trigger"
    )
    .then_some((kind, ep))
}

/// How a price moved after a moment, measured on OHLC-qualifying prints only (the chart's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub ref_price: i64,
    pub ref_at: i64,
    pub best: Option<(i64, i64)>,
    pub plus5_at: Option<i64>,
    pub stop: Option<i64>,
    pub stop_hit_at: Option<i64>,
    /// The first print through the trigger after the start, before any stop, and from it.
    pub crossing: Option<Crossing>,
    /// The first print through the trigger came only after the stop.
    pub crossed_after_stop: Option<i64>,
    /// The last print measured.
    pub through: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Crossing {
    pub price: i64,
    pub at: i64,
    pub best: Option<(i64, i64)>,
    pub plus5_at: Option<i64>,
}

fn reached_5pct(p: i64, from: i64) -> bool {
    p as i128 * 100 >= from as i128 * 105
}

/// Measures from `start` (exclusive; exec time): the best print before the stop is hit (or to the
/// end), when +5% over `ref_price` was first reached (before the stop), and, given a trigger, the
/// same measured from the first print above it (reference analysis: `outcome.py`).
pub fn measure(
    prints: &[(i64, i64)],
    start: i64,
    ref_price: i64,
    stop: Option<i64>,
    trigger: Option<i64>,
) -> Outcome {
    let from = prints.partition_point(|(x, _)| *x <= start);
    let after = &prints[from..];
    let stop_idx = stop.and_then(|s| after.iter().position(|(_, p)| *p < s));
    let seg = &after[..stop_idx.unwrap_or(after.len())];
    let best = |seg: &[(i64, i64)]| {
        let mut b: Option<(i64, i64)> = None;
        for &(x, p) in seg {
            if b.is_none_or(|(_, bp)| p > bp) {
                b = Some((x, p));
            }
        }
        b
    };
    let mut out = Outcome {
        ref_price,
        ref_at: start,
        best: best(seg),
        plus5_at: seg
            .iter()
            .find(|(_, p)| reached_5pct(*p, ref_price))
            .map(|(x, _)| *x),
        stop,
        stop_hit_at: stop_idx.map(|i| after[i].0),
        through: prints.last().map(|(x, _)| *x),
        ..Outcome::default()
    };
    if let Some(trig) = trigger
        && let Some(ci) = after.iter().position(|(_, p)| *p > trig)
    {
        if stop_idx.is_none_or(|s| ci < s) {
            let (cx, cp) = after[ci];
            let seg2 = &after[ci..stop_idx.unwrap_or(after.len())];
            out.crossing = Some(Crossing {
                price: cp,
                at: cx,
                best: best(seg2),
                plus5_at: seg2
                    .iter()
                    .find(|(_, p)| reached_5pct(*p, cp))
                    .map(|(x, _)| *x),
            });
        } else {
            out.crossed_after_stop = Some(after[ci].0);
        }
    }
    out
}

fn outcome_json(o: &Outcome) -> Value {
    json!({
        "ref_price": dollars(o.ref_price),
        "ref_at": ms(o.ref_at),
        "best_price": o.best.map(|(_, p)| dollars(p)),
        "best_at": o.best.map(|(x, _)| ms(x)),
        "best_pct": o.best.and_then(|(_, p)| pct(p, o.ref_price)),
        "plus5_price": dollars((o.ref_price as i128 * 105 / 100) as i64),
        "plus5_at": o.plus5_at.map(ms),
        "stop": o.stop.map(dollars),
        "stop_hit_at": o.stop_hit_at.map(ms),
        "crossing": o.crossing.as_ref().map(|c| json!({
            "price": dollars(c.price),
            "at": ms(c.at),
            "best_price": c.best.map(|(_, p)| dollars(p)),
            "best_at": c.best.map(|(x, _)| ms(x)),
            "best_pct": c.best.and_then(|(_, p)| pct(p, c.price)),
            "plus5_at": c.plus5_at.map(ms),
        })),
        "crossed_after_stop_at": o.crossed_after_stop.map(ms),
        "through": o.through.map(ms),
    })
}

/// The OHLC-qualifying prints as `(exec, price)`, in exec order.
fn ohlc_prints(prints: &[Print]) -> Vec<(i64, i64)> {
    prints
        .iter()
        .filter(|p| p.ohlc)
        .map(|p| (p.exec, p.price))
        .collect()
}

#[derive(Default)]
struct Acc {
    open: Option<(i64, i64)>,
    high: Option<(i64, i64)>,
    low: Option<(i64, i64)>,
    close: Option<(i64, i64)>,
    volume: u64,
    dollars: f64,
}

impl Acc {
    fn take(&mut self, p: &Print) {
        if p.ohlc {
            let x = (p.exec, p.price);
            self.open.get_or_insert(x);
            if self.high.is_none_or(|(_, h)| p.price > h) {
                self.high = Some(x);
            }
            if self.low.is_none_or(|(_, l)| p.price < l) {
                self.low = Some(x);
            }
            self.close = Some(x);
        }
        if p.volume {
            self.volume += p.size;
            self.dollars += dollars(p.price) * p.size as f64;
        }
    }

    fn json(&self) -> Value {
        if self.open.is_none() && self.volume == 0 {
            return Value::Null;
        }
        let px = |v: Option<(i64, i64)>| v.map(|(_, p)| dollars(p));
        json!({
            "open": px(self.open),
            "high": px(self.high),
            "high_at": self.high.map(|(x, _)| ms(x)),
            "low": px(self.low),
            "low_at": self.low.map(|(x, _)| ms(x)),
            "close": px(self.close),
            "volume": self.volume,
            "dollar_volume": self.dollars.round(),
        })
    }
}

fn session_name(k: Option<SessionKind>) -> &'static str {
    k.map_or("none", |k| k.as_str())
}

fn stats(r: &DayRecords, prints: &[Print]) -> Value {
    let mut by: BTreeMap<SessionKind, Acc> = BTreeMap::new();
    let mut all = Acc::default();
    let mut high: Option<&Print> = None;
    for p in prints {
        all.take(p);
        if let Some(k) = p.session {
            by.entry(k).or_default().take(p);
        }
        if p.ohlc && high.is_none_or(|h| p.price > h.price) {
            high = Some(p);
        }
    }
    let regular_close = by
        .get(&SessionKind::Regular)
        .and_then(|a| a.close)
        .map(|(_, p)| p);
    let prev = r.prev_close.map(|(_, c)| c);
    let change = match (regular_close, prev) {
        (Some(c), Some(p)) => json!({"abs": dollars(c - p), "pct": pct(c, p)}),
        _ => Value::Null,
    };
    let sess = |k| by.get(&k).map_or(Value::Null, Acc::json);
    json!({
        "prev_close": r.prev_close.map(|(d, c)| json!({"day": d.to_string(), "price": dollars(c)})),
        "close": regular_close.map(dollars),
        "change": change,
        "last": all.close.map(|(_, p)| dollars(p)),
        "sessions": {
            "pre": sess(SessionKind::Pre),
            "regular": sess(SessionKind::Regular),
            "after": sess(SessionKind::After),
        },
        "volume": all.volume,
        "dollar_volume": all.dollars.round(),
        "high": high.map(|h| json!({
            "price": dollars(h.price),
            "at": ms(h.exec),
            "session": session_name(h.session),
        })),
        "float": match &r.float {
            Some(f) => json!({
                "available": true,
                "shares": f.shares,
                "as_of": ms(f.retrieved_at),
                "provider": f.provider,
                "quality": f.quality,
            }),
            None => json!({"available": false, "reason": "no_float_record_on_or_before_day"}),
        },
        // Nothing in the server collects short interest (no provider, no evidence record):
        // stated rather than left blank, never estimated.
        "short_interest": {"available": false, "reason": "not_collected"},
    })
}

/// A notice row's alert start, reference price, stop and trigger.
struct AlertRef {
    start: i64,
    ref_price: i64,
    stop: Option<i64>,
    trigger: Option<i64>,
}

fn row_num(m: &Map<String, Value>, k: &str) -> Option<i64> {
    m.get(k)
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
}

fn alert_ref(
    m: &Map<String, Value>,
    prints: &[(i64, i64)],
    episodes: &HashMap<String, (Option<i64>, Option<i64>)>,
) -> Option<AlertRef> {
    let exec = row_num(m, "exec_ns");
    let start = exec
        .or_else(|| row_num(m, "at_ns"))
        .or_else(|| row_num(m, "at").map(|a| a * NS_PER_MS))?;
    let price = m
        .get("price")
        .and_then(Value::as_f64)
        .filter(|p| p.is_finite() && *p > 0.0)
        .map(|p| (p * 1_000_000.0).round() as i64);
    let id = m.get("event_id").and_then(Value::as_str).unwrap_or("");
    let ep = episode_of(id);
    let crossing = ep.is_some_and(|(k, _)| k.ends_with("crossing"));
    // A crossing is measured from the crossing print itself: its notice carries that print's
    // price and execution time; a row without a price falls back to the print at that time.
    let ref_price = match (crossing, price, exec) {
        (true, None, Some(x)) => prints.iter().rev().find(|(t, _)| *t <= x).map(|(_, p)| *p),
        _ => price,
    }?;
    let (trigger, stop) = ep
        .and_then(|(_, e)| episodes.get(e).copied())
        .unwrap_or((None, None));
    Some(AlertRef {
        start,
        ref_price,
        stop,
        trigger: if crossing { None } else { trigger },
    })
}

const MINUTE_MS: i64 = 60_000;
/// How far before the move the chart opens: enough flat tape to see where it started from.
pub const FOCUS_LEAD_MS: i64 = 30 * MINUTE_MS;
/// How far after the last alert: enough to see how the last one played out.
pub const FOCUS_TAIL_MS: i64 = 40 * MINUTE_MS;
/// How far back from the first alert the move's base (its lowest bar) is looked for.
const BASE_LOOKBACK_MS: i64 = 60 * MINUTE_MS;
/// With no alert and no setup: how far back from the day's high its base is looked for.
const HIGH_LOOKBACK_MS: i64 = 120 * MINUTE_MS;
/// The day's high extends the window only when it came this soon after the last alert: a peak
/// hours later is another move, and pulling it in would shrink the one the alerts were about.
const HIGH_REACH_MS: i64 = 60 * MINUTE_MS;

/// The window the chart OPENS on (epoch ms, both ends bar starts inside the day's bars), so a
/// day whose move came after hours does not open as a whole 4 AM to 8 PM day with every marker
/// bunched at one edge. Clients use it for the initial view only; zooming out shows the day.
///
/// From [`FOCUS_LEAD_MS`] before the earliest of the first sent alert, the first armed setup
/// and the move's base (the lowest bar in the hour before the first of them), to
/// [`FOCUS_TAIL_MS`] after the last sent alert (the last setup when nothing was sent: the late
/// missed setups the page folds away would otherwise stretch it), or after the day's high when
/// that came within [`HIGH_REACH_MS`] of it. With neither alerts nor setups: the day's biggest
/// run, its base (lowest bar in the two hours before the high) to the high, padded the same.
/// Clamped to the bars; `None` only without bars.
pub fn focus_window(
    bars: &[CandleWire],
    sent: &[i64],
    setups: &[i64],
) -> Option<(i64, i64, &'static str)> {
    let first_t = bars.first()?.t;
    let last_t = bars.last()?.t;
    let high = bars
        .iter()
        .max_by(|a, b| a.h.total_cmp(&b.h).then(b.t.cmp(&a.t)))?;
    let lowest_in = |from: i64, to: i64| {
        bars.iter()
            .filter(|b| b.t >= from && b.t <= to)
            .min_by(|a, b| a.l.total_cmp(&b.l).then(b.t.cmp(&a.t)))
            .map(|b| b.t)
    };
    let first = sent.iter().chain(setups).copied().min();
    let (start, end, reason) = match first {
        Some(first) => {
            let base = lowest_in(first - BASE_LOOKBACK_MS, first).unwrap_or(first);
            let tail = sent
                .iter()
                .copied()
                .max()
                .or_else(|| setups.iter().copied().max())
                .unwrap_or(first);
            let peak = if high.t > first && high.t <= tail + HIGH_REACH_MS {
                high.t
            } else {
                tail
            };
            let reason = if sent.is_empty() { "setups" } else { "alerts" };
            (base.min(first), tail.max(peak), reason)
        }
        None => {
            let base = lowest_in(high.t - HIGH_LOOKBACK_MS, high.t).unwrap_or(high.t);
            (base, high.t, "move")
        }
    };
    let from = (start - FOCUS_LEAD_MS).clamp(first_t, last_t);
    let to = (end + FOCUS_TAIL_MS).clamp(from, last_t);
    Some((from, to, reason))
}

/// The ticker-day package (module docs of `crate::insights`).
pub fn build(r: &DayRecords, out: &ReplayOut) -> Value {
    let prints = ohlc_prints(&out.prints);

    // Each episode's trigger and stop: the live journal's when it has them, else the replay's.
    let mut episodes: HashMap<String, (Option<i64>, Option<i64>)> = HashMap::new();
    let mut armed: Vec<&DetectionTransition> = Vec::new();
    for t in &out.transitions {
        if let Some(a) = &t.anchors {
            episodes.insert(t.episode_id.clone(), (Some(a.h0), Some(a.pullback_low)));
        }
        if t.to == State::Armed && t.from == Some(State::PullbackForming) {
            armed.push(t);
        }
    }
    for (id, e) in &r.live {
        let slot = episodes.entry(id.clone()).or_default();
        if e.trigger.is_some() {
            slot.0 = e.trigger;
        }
        if e.stop.is_some() {
            slot.1 = e.stop;
        }
    }

    // Rows oldest first; sent ones numbered as the chart's markers.
    let mut rows: Vec<Map<String, Value>> = r
        .notices
        .iter()
        .filter_map(|v| v.as_object().cloned())
        .collect();
    rows.sort_by_key(|m| row_num(m, "at").unwrap_or(0));
    let mut notified_episodes: HashSet<String> = HashSet::new();
    let mut marker = 0u32;
    let mut markers: Vec<Value> = Vec::new();
    let mut by_kind: BTreeMap<String, (u32, u32, u32, Option<f64>)> = BTreeMap::new();
    let (mut sent, mut held, mut reached) = (0u32, 0u32, 0u32);
    let mut best_entry: Option<(f64, Value)> = None;
    for (i, m) in rows.iter_mut().enumerate() {
        if let Some((_, ep)) = m
            .get("event_id")
            .and_then(Value::as_str)
            .and_then(episode_of)
        {
            notified_episodes.insert(ep.to_string());
        }
        let is_sent = m.get("status").and_then(Value::as_str) == Some("sent");
        let o = alert_ref(m, &prints, &episodes)
            .map(|a| measure(&prints, a.start, a.ref_price, a.stop, a.trigger));
        let notice = m
            .get("notice")
            .and_then(Value::as_str)
            .unwrap_or("notice")
            .to_string();
        let hit = o.as_ref().is_some_and(|o| o.plus5_at.is_some());
        let best_pct = o
            .as_ref()
            .and_then(|o| o.best.and_then(|(_, p)| pct(p, o.ref_price)));
        let k = by_kind.entry(notice.clone()).or_default();
        k.0 += 1;
        if is_sent {
            sent += 1;
            k.1 += 1;
            marker += 1;
            m.insert("marker".into(), json!(marker));
            if let Some(o) = &o {
                markers.push(json!({
                    "kind": "alert",
                    "n": marker,
                    "event_id": m.get("event_id"),
                    "at": ms(o.ref_at),
                    "price": dollars(o.ref_price),
                }));
            }
        } else {
            held += 1;
        }
        if hit {
            reached += 1;
            k.2 += 1;
        }
        if let Some(b) = best_pct {
            k.3 = Some(k.3.map_or(b, |x: f64| x.max(b)));
            if best_entry.as_ref().is_none_or(|(x, _)| b > *x) {
                best_entry = Some((
                    b,
                    json!({
                        "event_id": m.get("event_id"),
                        "notice": notice,
                        "marker": m.get("marker"),
                        "price": o.as_ref().map(|o| dollars(o.ref_price)),
                        "best_pct": b,
                    }),
                ));
            }
        }
        m.insert("n".into(), json!(i + 1));
        m.insert("label".into(), json!(crate::notices::kind_label(&notice)));
        m.insert(
            "outcome".into(),
            o.as_ref().map_or(Value::Null, outcome_json),
        );
    }

    // Missed setups: armed, and not notified (the ledger's word when it has the day).
    let verdicts: HashMap<&str, &SetupDecision> = out
        .decisions
        .iter()
        .map(|d| (d.episode_id.as_str(), d))
        .collect();
    let first_notified = out
        .decisions
        .iter()
        .find(|d| d.verdict == SetupVerdict::Notified)
        .map(|d| d.episode_id.clone());
    let mut missed: Vec<Value> = Vec::new();
    let mut armed_n = 0u32;
    for t in &armed {
        armed_n += 1;
        let in_ledger = notified_episodes.contains(&t.episode_id);
        let v = verdicts.get(t.episode_id.as_str()).map(|d| d.verdict);
        let (code, text): (&str, String) = match (v, in_ledger) {
            (_, true) => continue,
            (Some(SetupVerdict::Notified), false) if r.ledger_known && !r.ongoing => (
                "not_recorded",
                "not in the notification log: the live server did not send it (no rule here explains why)".into(),
            ),
            (Some(SetupVerdict::Notified), _) => continue,
            (Some(SetupVerdict::Warmup), _) => (
                "warmup",
                "muted: warm-up after a server restart (its pattern began before the restart)".into(),
            ),
            (Some(SetupVerdict::OnePerDay), _) => (
                "one_per_day",
                "held: one setup notice per stock per day, already sent".into(),
            ),
            (Some(SetupVerdict::WindowClosed { .. }), _) => (
                "window_closed",
                "held: armed more than 30 min after the Runners flag, and not a Ross member".into(),
            ),
            (Some(SetupVerdict::NoContext), _) | (None, _) => (
                "no_context",
                "held: not a Runners row that day and not a Ross member".into(),
            ),
        };
        let (trigger, stop) = episodes.get(&t.episode_id).copied().unwrap_or_default();
        let at = t.observed_at;
        let ref_price = prints
            .iter()
            .rev()
            .find(|(x, _)| *x <= at)
            .map(|(_, p)| *p)
            .unwrap_or(0);
        let o = measure(&prints, at, ref_price, stop, trigger);
        let restart_at = r.boots.iter().rev().find(|b| **b <= at).copied();
        let since = match v {
            Some(SetupVerdict::WindowClosed { since }) => Some(ms(since)),
            _ => None,
        };
        let evidence = if r.live.contains_key(&t.episode_id) {
            "journal"
        } else {
            "replay"
        };
        markers.push(json!({
            "kind": "missed",
            "episode_id": t.episode_id,
            "at": ms(at),
            "price": dollars(ref_price),
        }));
        missed.push(json!({
            "episode_id": t.episode_id,
            "armed_at": ms(at),
            "trigger": trigger.map(dollars),
            "stop": stop.map(dollars),
            "reason": code,
            "reason_text": text,
            "restart_at": (code == "warmup").then_some(restart_at.map(ms)).flatten(),
            "flagged_at": since,
            "first_notified_episode": (code == "one_per_day").then(|| first_notified.clone()).flatten(),
            "evidence": evidence,
            "outcome": outcome_json(&o),
        }));
    }

    let regular: Vec<&Print> = out
        .prints
        .iter()
        .filter(|p| p.ohlc && p.session == Some(SessionKind::Regular))
        .collect();
    let high = out.prints.iter().filter(|p| p.ohlc).max_by_key(|p| p.price);
    let outside = high.is_some_and(|h| h.session != Some(SessionKind::Regular));

    let at_of = |kind: &str| -> Vec<i64> {
        markers
            .iter()
            .filter(|m| m["kind"] == kind)
            .filter_map(|m| m["at"].as_i64())
            .collect()
    };
    let focus = focus_window(&out.bars, &at_of("alert"), &at_of("missed"))
        .map(|(from, to, reason)| json!({"from": from, "to": to, "reason": reason}));

    json!({
        "available": true,
        "stats": stats(r, &out.prints),
        "bars": out.bars,
        "focus": focus,
        "alerts": rows,
        "markers": markers,
        "scorecard": {
            "alerts": sent + held,
            "sent": sent,
            "held": held,
            "reached_5pct": reached,
            "best_entry": best_entry.map(|(_, v)| v),
            "setups_armed": armed_n,
            "missed": missed.len(),
            "by_kind": by_kind.iter().map(|(k, (n, s, h, b))| json!({
                "notice": k,
                "label": crate::notices::kind_label(k),
                "alerts": n,
                "sent": s,
                "reached_5pct": h,
                "best_pct": b,
            })).collect::<Vec<_>>(),
        },
        "halts": {
            "halts": out.halts,
            "move_session": high.map(|h| session_name(h.session)),
            "outside_regular": outside,
            // LULD bands exist only 09:30–16:00 ET: a move outside them had no band to approach.
            "luld_applies": !outside,
            "closest": closest_approach(&regular, &out.luld),
        },
        "missed": missed,
    })
}

/// The regular session's closest approach to a modelled LULD band (production's `luld.rs`, via
/// the chart engine's band points): per side, the print nearest its band, in basis points.
fn closest_approach(regular: &[&Print], luld: &[chart_engine::wire::LuldWire]) -> Value {
    let mut pts: Vec<&chart_engine::wire::LuldWire> = luld.iter().collect();
    pts.sort_by_key(|p| p.t);
    let micros = |v: Option<f64>| v.map(|d| (d * 1_000_000.0).round() as i64);
    let mut up: Option<(i64, &Print, i64)> = None;
    let mut down: Option<(i64, &Print, i64)> = None;
    let mut j = 0usize;
    let mut cur: Option<&chart_engine::wire::LuldWire> = None;
    for p in regular {
        let t = ms(p.exec);
        while j < pts.len() && pts[j].t <= t {
            cur = Some(pts[j]);
            j += 1;
        }
        let Some(b) = cur else { continue };
        if p.price <= 0 {
            continue;
        }
        if let Some(u) = micros(b.up) {
            let bp = (u - p.price) * 10_000 / p.price;
            if up.is_none_or(|(x, _, _)| bp < x) {
                up = Some((bp, p, u));
            }
        }
        if let Some(d) = micros(b.down) {
            let bp = (p.price - d) * 10_000 / p.price;
            if down.is_none_or(|(x, _, _)| bp < x) {
                down = Some((bp, p, d));
            }
        }
    }
    let side = |s: Option<(i64, &Print, i64)>| {
        s.map(|(bp, p, band)| {
            json!({"bp": bp, "at": ms(p.exec), "price": dollars(p.price), "band": dollars(band)})
        })
    };
    json!({"up": side(up), "down": side(down), "regular_prints": regular.len()})
}
