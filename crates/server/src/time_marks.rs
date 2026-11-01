//! Time-based states decided on the server's clock (`clock.v1`): the owner's rule that Rust is
//! the only clock (exchange time, ns), so no client compares a market timestamp with its own
//! `Date.now()` or extrapolates the market clock by wall time.
//!
//! What used to be decided in the clients, and where it is decided now:
//!
//! - **Runners rows** ([`stamp_rows`], on every `rocket.snapshot` / `rocket.update` body): `fresh`
//!   (flagged under [`FRESH_NS`] ago, the tab's count and the row's accent) with `fresh_until_ns`;
//!   on a buying-burst row `burst_live` (outcome pending and alerted under [`BURST_LIVE_NS`] ago)
//!   with `burst_live_until_ns`, and the "look now" `flame {lit, until_ns}` (alerted under
//!   [`FLAME_NS`] ago and the last price still at or above the alert price; packages/shared
//!   rowMarks.ts `flameMark` had the rule). They are in the body the update diff compares, so a
//!   flip sends an update on the next broadcast tick: the mark goes out by itself, at the market
//!   clock, at every client at once. The Ross, Favorites and Movers rows take the flame from the
//!   symbol's Runners row, as they already took the burst.
//! - **Runners toasts** ([`Events`]): `runners.event {kind, id, symbol, at_ns, price, ...}` when a
//!   row is new (a flag or a burst; running, run and fast entries never toast) or its settled
//!   state turns `breaking_out`, judged fresh on the market clock ([`EVENT_FRESH_NS`]) and only on
//!   the LIVE market (a replay never pushes). It replaces each web dashboard diffing successive
//!   panels against its wall clock (apps/web dashboard/runnersToasts.ts).
//! - **Price alerts**: crate::alerts::eval decides expiry, snooze and cooling on its own tick and
//!   writes them into `alerts.state` (`snoozed` there).
//! - **The paper refusal toast**: alpaca_paper's notice carries `fresh`, judged when the frame is
//!   rendered.
//!
//! Ages the clients still show ("3m ago") are measured against the last `replay.clock` frame,
//! which the server pushes at least once a second in live mode (the live provider's 1 s tick) and
//! ~4 times a second while a replay plays, with `nowNs` beside `now`. A paused replay sends none:
//! its clock stands still, and so do the ages.

use std::collections::HashMap;

use serde_json::{Value, json};

/// The feature a client must see in `welcome.features` before it trusts these states over its
/// own clock (an older server sends none of them, and the client keeps its fallbacks).
pub const FEATURE: &str = "clock.v1";

const MS: i64 = 1_000_000;
/// A row flagged this recently is "fresh" (packages/shared rocket.ts: `ageMs < 60_000`).
pub const FRESH_NS: i64 = 60_000 * MS;
/// How long a buying burst keeps the flame lit (owner, 2026-10-26: "look at me right now").
pub const FLAME_NS: i64 = 5 * 60_000 * MS;
/// A burst row's marker stays accent this long after the alert while its outcome is pending.
pub const BURST_LIVE_NS: i64 = 5 * 60_000 * MS;
/// A toast older than this on the market clock is not news (the phone push's MAX_AGE_MS).
pub const EVENT_FRESH_NS: i64 = 3 * 60_000 * MS;
/// At most this many toasts from one tick (a toast replaces the last one anyway).
pub const MAX_EVENTS_PER_TICK: usize = 3;

fn ms_to_ns(v: &Value) -> Option<i64> {
    v.as_i64().map(|ms| ms.saturating_mul(MS))
}

/// Stamps every row of a Runners body (`{rows: [...]}`) with its server-decided time states at
/// `now_ns` (module docs).
pub fn stamp_rows(body: &mut Value, now_ns: i64) {
    let Some(rows) = body.get_mut("rows").and_then(Value::as_array_mut) else {
        return;
    };
    for r in rows {
        let Some(m) = r.as_object_mut() else { continue };
        let Some(at) = m.get("at").and_then(ms_to_ns) else {
            continue;
        };
        let age = now_ns - at;
        m.insert("fresh_until_ns".into(), json!(at + FRESH_NS));
        m.insert("fresh".into(), json!((0..FRESH_NS).contains(&age)));
        if m.get("kind").and_then(Value::as_str) != Some("burst") {
            continue;
        }
        let pending = m
            .get("burst")
            .and_then(|b| b.get("outcome"))
            .and_then(|o| o.get("state"))
            .and_then(Value::as_str)
            == Some("pending");
        m.insert("burst_live_until_ns".into(), json!(at + BURST_LIVE_NS));
        m.insert(
            "burst_live".into(),
            json!(pending && (0..BURST_LIVE_NS).contains(&age)),
        );
        let p = m.get("p").and_then(Value::as_f64).unwrap_or(0.0);
        let last = m.get("last").and_then(Value::as_f64);
        let lit = p > 0.0
            && last.is_some_and(|l| l.is_finite() && l >= p)
            && (0..FLAME_NS).contains(&age);
        m.insert(
            "flame".into(),
            json!({"lit": lit, "until_ns": at + FLAME_NS}),
        );
    }
}

/// The Runners toasts' memory: each row's settled state when last seen, with its flag time (ns).
/// One per market.
#[derive(Debug, Default)]
pub struct Events {
    seen: HashMap<String, (Option<String>, Option<i64>)>,
    primed: bool,
}

impl Events {
    /// Forget everything: the next pass only primes (a rebuild replaced the board's rows).
    pub fn reset(&mut self) {
        self.seen.clear();
        self.primed = false;
    }

    /// The `runners.event` frames one pass over the panel's rows calls for, newest rows first as
    /// the panel lists them. The first pass after start or a [`Events::reset`] only primes, so a
    /// restart or a reconnect never announces the day's earlier rows. `announce` false (a replay)
    /// keeps the memory current without producing anything.
    pub fn take(&mut self, body: &Value, now_ns: i64, announce: bool) -> Vec<String> {
        let fresh =
            |t: Option<i64>| t.is_some_and(|t| (0..=EVENT_FRESH_NS).contains(&(now_ns - t)));
        let mut out = Vec::new();
        let rows = body.get("rows").and_then(Value::as_array);
        for r in rows.into_iter().flatten() {
            let Some(id) = r.get("id").and_then(Value::as_str) else {
                continue;
            };
            let state = r.get("state").and_then(Value::as_str).map(str::to_owned);
            let provisional = r.get("provisional").and_then(Value::as_bool) == Some(true);
            let at = r.get("at").and_then(ms_to_ns);
            let had = self.seen.contains_key(id);
            let before = self.seen.get(id).and_then(|(s, _)| s.clone());
            // A provisional state (a breakout seen in the forming bar) is not a transition yet.
            let settled = if provisional { before.clone() } else { state };
            self.seen.insert(id.to_owned(), (settled.clone(), at));
            if !self.primed || !announce {
                continue;
            }
            let symbol = r.get("s").and_then(Value::as_str).unwrap_or("");
            let kind = r.get("kind").and_then(Value::as_str).unwrap_or("runner");
            if !had {
                // Running, "Run +20%" and "Fast move" entries never toast (owner, 2026-10-22): many
                // a day, and a move under way rather than a forecast.
                if fresh(at) && !matches!(kind, "running" | "run" | "fast") {
                    out.push(event_frame("new_flag", id, symbol, at.unwrap_or(now_ns), r));
                }
            } else if settled.as_deref() == Some("breaking_out")
                && before.as_deref() != Some("breaking_out")
            {
                let since = r.get("state_since").and_then(ms_to_ns);
                if fresh(since) {
                    out.push(event_frame(
                        "breakout",
                        id,
                        symbol,
                        since.unwrap_or(now_ns),
                        r,
                    ));
                }
            }
        }
        self.primed = true;
        // Rows no longer on the panel are forgotten once their flag is too old to toast as new,
        // so the memory stays about the panel's size. Not at once: a row that drops off the panel
        // for a tick (crowded out, a filter edge) and comes back while still fresh would otherwise
        // be "new" again and toast twice (the web's own diff never forgot a row).
        let live: std::collections::HashSet<&str> = rows
            .into_iter()
            .flatten()
            .filter_map(|r| r.get("id")?.as_str())
            .collect();
        self.seen
            .retain(|id, (_, at)| live.contains(id.as_str()) || fresh(*at));
        out.truncate(MAX_EVENTS_PER_TICK);
        out
    }
}

/// `{"type":"runners.event", ...}` (`type` first: the web feed routes on the leading bytes).
fn event_frame(kind: &str, id: &str, symbol: &str, at_ns: i64, r: &Value) -> String {
    #[derive(serde::Serialize)]
    struct Frame<'a> {
        #[serde(rename = "type")]
        t: &'static str,
        kind: &'a str,
        id: &'a str,
        symbol: &'a str,
        at_ns: i64,
        /// The row's source path (`runner`, `burst`, ...).
        path: &'a str,
        /// Dollars: the flag price for a new row, the last price for a breakout.
        price: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_flag_pct: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<&'a str>,
    }
    let price = if kind == "breakout" {
        r.get("last")
            .and_then(Value::as_f64)
            .or_else(|| r.get("p")?.as_f64())
    } else {
        r.get("p").and_then(Value::as_f64)
    };
    serde_json::to_string(&Frame {
        t: "runners.event",
        kind,
        id,
        symbol,
        at_ns,
        path: r.get("kind").and_then(Value::as_str).unwrap_or("runner"),
        price,
        from_flag_pct: (kind == "breakout")
            .then(|| r.get("from_flag_pct").and_then(Value::as_f64))
            .flatten(),
        reason: (kind == "new_flag")
            .then(|| {
                r.get("reasons")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(Value::as_str)
            })
            .flatten(),
    })
    .expect("runners event serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000 * MS;

    fn row(id: &str, kind: &str, at_ms: i64, state: &str) -> Value {
        json!({"id": id, "s": id.to_uppercase(), "kind": kind, "at": at_ms, "p": 2.0, "last": 2.1,
               "state": state, "provisional": false, "state_since": at_ms, "from_flag_pct": 5.0,
               "reasons": ["volume 9x"],
               "burst": {"outcome": {"state": "pending"}}})
    }

    #[test]
    fn rows_carry_fresh_burst_live_and_the_flame_decided_at_the_server_clock() {
        let at = 1_000_000_i64; // ms
        let mut body =
            json!({"rows": [row("a", "burst", at, "running"), row("b", "runner", at, "watching")]});
        stamp_rows(&mut body, at * MS + 30 * 1_000 * MS);
        let a = &body["rows"][0];
        assert_eq!(a["fresh"], true);
        assert_eq!(a["fresh_until_ns"], at * MS + FRESH_NS);
        assert_eq!(a["burst_live"], true);
        assert_eq!(a["flame"]["lit"], true);
        assert_eq!(a["flame"]["until_ns"], at * MS + FLAME_NS);
        let b = &body["rows"][1];
        assert_eq!(b["fresh"], true);
        assert!(b.get("flame").is_none(), "only a burst lights the flame");

        // Five minutes on: everything has gone out by itself.
        let mut body = json!({"rows": [row("a", "burst", at, "running")]});
        stamp_rows(&mut body, at * MS + 5 * MIN);
        assert_eq!(body["rows"][0]["fresh"], false);
        assert_eq!(body["rows"][0]["burst_live"], false);
        assert_eq!(body["rows"][0]["flame"]["lit"], false);

        // Under the alert price: no flame even while young.
        let mut r = row("a", "burst", at, "running");
        r["last"] = json!(1.9);
        let mut body = json!({"rows": [r]});
        stamp_rows(&mut body, at * MS + MIN);
        assert_eq!(body["rows"][0]["flame"]["lit"], false);
    }

    #[test]
    fn events_prime_then_announce_new_rows_and_settled_breakouts_once() {
        let at = 10_000_000_i64;
        let now = at * MS + 10 * 1_000 * MS;
        let mut e = Events::default();
        let body = json!({"rows": [row("a", "runner", at, "watching")]});
        assert!(e.take(&body, now, true).is_empty(), "the first pass primes");
        let body = json!({"rows": [row("b", "runner", at, "watching"), row("a", "runner", at, "watching"),
                                    row("c", "running", at, "running")]});
        let got = e.take(&body, now, true);
        assert_eq!(got.len(), 1, "{got:?}");
        let v: Value = serde_json::from_str(&got[0]).unwrap();
        assert_eq!(
            (v["type"].as_str(), v["kind"].as_str(), v["symbol"].as_str()),
            (Some("runners.event"), Some("new_flag"), Some("B"))
        );
        assert_eq!(v["reason"], "volume 9x");
        assert!(got[0].starts_with(r#"{"type":"runners.event""#));

        // A provisional breakout is not one yet; the settled one is, once.
        let mut a = row("a", "runner", at, "breaking_out");
        a["provisional"] = json!(true);
        assert!(e.take(&json!({"rows": [a.clone()]}), now, true).is_empty());
        a["provisional"] = json!(false);
        let got = e.take(&json!({"rows": [a.clone()]}), now, true);
        assert_eq!(got.len(), 1);
        assert!(got[0].contains(r#""kind":"breakout""#), "{}", got[0]);
        assert!(e.take(&json!({"rows": [a]}), now, true).is_empty());

        // A fresh row that drops off the panel for a pass and comes back is not new again.
        let g = row("g", "runner", at, "watching");
        assert_eq!(e.take(&json!({"rows": [g.clone()]}), now, true).len(), 1);
        assert!(e.take(&json!({"rows": []}), now, true).is_empty());
        assert!(
            e.take(&json!({"rows": [g]}), now, true).is_empty(),
            "announced once"
        );

        // Stale on the market clock: not news. A replay keeps the memory without announcing.
        let old = row("d", "runner", at - 10 * 60_000, "watching");
        assert!(e.take(&json!({"rows": [old]}), now, true).is_empty());
        assert!(
            e.take(
                &json!({"rows": [row("f", "runner", at, "watching")]}),
                now,
                false
            )
            .is_empty()
        );
    }
}
