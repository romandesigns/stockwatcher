//! The alert rules as the clients store them in the synced settings, parsed defensively.
//!
//! Three namespaces (docs/architecture/app-state.md "Alerts"; the TypeScript twin is
//! packages/shared/src/alerts.ts, change both together):
//!
//! - `alerts` (client-owned): a map `alertId → rule`, written per entry with `settings.patch`
//!   so two devices editing different alerts never overwrite each other (the drawings rule).
//! - `alerts.prefs` (client-owned): the phone budget, quiet hours, the sessions picked last.
//! - `alerts.state` (server-owned, [`STATE_NS`]): what the evaluator decided per alert (armed,
//!   cooling until t, done, expired). Kept apart from the rules so a rule edit from a phone and a
//!   state change from the server can never overwrite each other (last writer wins per entry,
//!   and the two are different entries in different namespaces). Clients cannot write it.
//!
//! The settings store keeps every value opaque, so everything read here may be junk from an
//! older or newer client: a rule that does not parse is skipped (logged once), a field that
//! does not parse falls back to its default, exactly as the clients' own parsers do.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::geometry::{Part, Tolerance};

pub const RULES_NS: &str = "alerts";
pub const PREFS_NS: &str = "alerts.prefs";
/// Written only by the server (ws.rs refuses client writes to it).
pub const STATE_NS: &str = "alerts.state";

/// "Every time" gap bounds and default (Strategy board: 5 min to 1 h, default 15).
pub const MIN_GAP_MIN: u32 = 5;
pub const MAX_GAP_MIN: u32 = 60;
pub const DEFAULT_GAP_MIN: u32 = 15;
/// The daily phone budgets offered, and the default.
pub const PHONE_BUDGETS: [u32; 4] = [5, 10, 20, 50];
pub const DEFAULT_PHONE_BUDGET: u32 = 20;

/// True for a namespace only the server may write.
pub fn server_owned(ns: &str) -> bool {
    ns == STATE_NS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Session {
    Pre,
    Regular,
    After,
    Overnight,
}

impl Session {
    pub fn of(kind: market_events::SessionKind) -> Session {
        match kind {
            market_events::SessionKind::Pre => Session::Pre,
            market_events::SessionKind::Regular => Session::Regular,
            market_events::SessionKind::After => Session::After,
            market_events::SessionKind::Overnight => Session::Overnight,
        }
    }

    fn parse(v: &Value) -> Option<Session> {
        match v.as_str()? {
            "pre" => Some(Session::Pre),
            "regular" => Some(Session::Regular),
            "after" => Some(Session::After),
            "overnight" => Some(Session::Overnight),
            _ => None,
        }
    }
}

/// The default sessions of a new alert (the design's chips): pre, regular and after, not overnight.
pub const DEFAULT_SESSIONS: [Session; 3] = [Session::Pre, Session::Regular, Session::After];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossDir {
    Above,
    Below,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveDir {
    Up,
    Down,
    Either,
}

/// The reference a % move is measured against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveWindow {
    /// Within the last N minutes (1, 5 or 15), from the 15 s buckets' lows/highs.
    Minutes(u32),
    /// Since today's regular open (the first eligible regular-session trade).
    Open,
    /// Since the previous trading day's official close (daily-closes.json, split-adjusted).
    PrevClose,
}

/// What an alert watches.
#[derive(Clone, Debug, PartialEq)]
pub enum Condition {
    /// The price crosses a level: an eligible trade on the other side from the previous one.
    Price { dir: CrossDir, price: f64 },
    /// A % move up/down/either way within a window.
    Move {
        dir: MoveDir,
        pct: f64,
        window: MoveWindow,
    },
    /// The stock is halted or resumes (the live status feed).
    Halt,
    /// Within `pct`% of the MODELLED limit-up/down band (chart-engine luld.rs: our reading of the
    /// LULD plan, not the exchange's numbers; every text that shows it says "modelled").
    Luld { side: MoveDir, pct: f64 },
    /// A new high of the trading day (an eligible trade above every earlier one since 04:00).
    HighOfDay,
    /// The Halt watcher's pressure (crate::pressure, 0–100) reaches `min` on the buy side, the
    /// sell side, or either. Scored once a second for the symbols such an alert watches.
    Pressure { side: MoveDir, min: u8 },
    /// An eligible trade touches a user drawing (a drawing alert, 2026-11-09): within
    /// `tolerance` of the level `part` of drawing `drawing_id` in the symbol's synced
    /// `drawings:<SYMBOL>` namespace, at the trade's candle, on the chart of timeframe `tf_ms`
    /// (super::geometry). The geometry is read live from the drawings, never copied here: an
    /// edited drawing moves the alert with it, a deleted one leaves it orphaned (eval.rs).
    Drawing {
        drawing_id: String,
        part: Part,
        tolerance: Tolerance,
        tf_ms: i64,
    },
}

impl Condition {
    pub fn is_halt(&self) -> bool {
        matches!(self, Condition::Halt)
    }

    /// Re-arms on the gap alone: there is no fire price for "moved back 0.5%" (a halt, a
    /// pressure reading).
    pub fn rearms_on_gap(&self) -> bool {
        matches!(self, Condition::Halt | Condition::Pressure { .. })
    }

    pub fn is_drawing(&self) -> bool {
        matches!(self, Condition::Drawing { .. })
    }
}

/// A drawing alert's timeframe (`tf`): the chart it was set on, whose slots place a sloped line
/// (super::geometry). 1 minute when absent or unknown.
fn tf_ms(v: Option<&Value>) -> i64 {
    match v.and_then(Value::as_str) {
        Some("10s") => 10_000,
        Some("15s") => 15_000,
        Some("30s") => 30_000,
        Some("5m") => 300_000,
        Some("1h") => 3_600_000,
        _ => 60_000,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fire {
    Once,
    /// Every time, at least this many minutes apart (re-arming also needs a 0.5% move back).
    Every {
        gap_min: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expiry {
    EndOfDay,
    EndOfWeek,
    Never,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub id: String,
    pub symbol: String,
    pub condition: Condition,
    pub fire: Fire,
    pub sessions: Vec<Session>,
    pub expiry: Expiry,
    /// Epoch ms the alert expires at (computed by the client from `expiry` when it is created or
    /// edited, so every device shows the same "expires Fri"); None = never.
    pub expires_at: Option<i64>,
    pub notify_app: bool,
    pub notify_phone: bool,
    /// The alert's sound chip (drawing alerts; default on). The server only uses it to pick the
    /// phone's notification channel (a sounding drawing channel or the plain alerts one); the
    /// clients play the in-app tone.
    pub sound: bool,
    /// Epoch ms; while in the future the alert neither fires nor records anything.
    pub snooze_until: Option<i64>,
    pub created_at: i64,
    /// Bumped by the client on every edit (a drag, the form). A rule whose `edited_at` is newer
    /// than its state's `basis` starts over armed: moving a finished alert re-arms it.
    pub edited_at: i64,
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn int(v: Option<&Value>) -> Option<i64> {
    v.and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
    })
}

/// A ticker as the wire spells it (app-state parseSymbol's rule).
pub fn parse_symbol(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?.trim().to_ascii_uppercase();
    let mut chars = s.chars();
    let first = chars.next()?;
    (first.is_ascii_uppercase()
        && s.len() <= 12
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '.' || c == '-'))
    .then_some(s)
}

fn move_dir(v: Option<&Value>) -> MoveDir {
    match v.and_then(Value::as_str) {
        Some("down") => MoveDir::Down,
        Some("either") => MoveDir::Either,
        _ => MoveDir::Up,
    }
}

fn parse_condition(v: &Value) -> Option<Condition> {
    let o = v.as_object()?;
    match o.get("kind")?.as_str()? {
        "price" => {
            let price = num(o.get("price")).filter(|p| *p > 0.0)?;
            let dir = match o.get("dir").and_then(Value::as_str) {
                Some("below") => CrossDir::Below,
                _ => CrossDir::Above,
            };
            Some(Condition::Price { dir, price })
        }
        "move" => {
            let pct = num(o.get("pct")).filter(|p| *p > 0.0 && *p <= 1000.0)?;
            let window = match o.get("window").and_then(Value::as_str) {
                Some("1m") => MoveWindow::Minutes(1),
                Some("15m") => MoveWindow::Minutes(15),
                Some("open") => MoveWindow::Open,
                Some("prevClose") => MoveWindow::PrevClose,
                _ => MoveWindow::Minutes(5),
            };
            Some(Condition::Move {
                dir: move_dir(o.get("dir")),
                pct,
                window,
            })
        }
        "halt" => Some(Condition::Halt),
        "luld" => {
            let pct = num(o.get("pct")).filter(|p| *p > 0.0 && *p <= 100.0)?;
            Some(Condition::Luld {
                side: move_dir(o.get("side")),
                pct,
            })
        }
        "hod" => Some(Condition::HighOfDay),
        "pressure" => {
            let min = int(o.get("min")).filter(|m| (1..=100).contains(m))? as u8;
            let side = match o.get("side").and_then(Value::as_str) {
                Some("sell") => MoveDir::Down,
                Some("either") => MoveDir::Either,
                _ => MoveDir::Up,
            };
            Some(Condition::Pressure { side, min })
        }
        "drawing" => {
            let drawing_id = o
                .get("drawingId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty() && s.len() <= 128)?
                .to_owned();
            Some(Condition::Drawing {
                drawing_id,
                part: Part::parse(o.get("part")),
                tolerance: Tolerance::parse(o.get("tol")),
                tf_ms: tf_ms(o.get("tf")),
            })
        }
        _ => None,
    }
}

/// One rule from its map entry. None = skip it (no symbol, no condition the server knows).
pub fn parse_rule(id: &str, v: &Value) -> Option<Rule> {
    let o = v.as_object()?;
    let symbol = parse_symbol(o.get("symbol"))?;
    let condition = parse_condition(o.get("rule")?)?;
    let fire = match o.get("fire").and_then(Value::as_str) {
        Some("every") => Fire::Every {
            gap_min: int(o.get("gapMin"))
                .map(|g| g.clamp(MIN_GAP_MIN as i64, MAX_GAP_MIN as i64) as u32)
                .unwrap_or(DEFAULT_GAP_MIN),
        },
        _ => Fire::Once,
    };
    let sessions: Vec<Session> = match o.get("sessions").and_then(Value::as_array) {
        Some(a) => {
            let mut s: Vec<Session> = a.iter().filter_map(Session::parse).collect();
            s.dedup();
            s
        }
        None => DEFAULT_SESSIONS.to_vec(),
    };
    let expiry = match o.get("expiry").and_then(Value::as_str) {
        Some("day") => Expiry::EndOfDay,
        Some("never") => Expiry::Never,
        _ => Expiry::EndOfWeek,
    };
    let notify = o.get("notify").and_then(Value::as_object);
    let flag = |k: &str, d: bool| {
        notify
            .and_then(|n| n.get(k))
            .and_then(Value::as_bool)
            .unwrap_or(d)
    };
    let created_at = int(o.get("createdAt")).unwrap_or(0);
    Some(Rule {
        id: id.to_owned(),
        symbol,
        condition,
        fire,
        sessions,
        expiry,
        expires_at: if expiry == Expiry::Never {
            None
        } else {
            int(o.get("expiresAt"))
        },
        notify_app: flag("app", true),
        notify_phone: flag("phone", true),
        sound: o.get("sound").and_then(Value::as_bool).unwrap_or(true),
        snooze_until: int(o.get("snoozeUntil")),
        created_at,
        edited_at: int(o.get("editedAt")).unwrap_or(created_at),
    })
}

/// Every rule in the `alerts` namespace's value (absent or not a map = none).
pub fn parse_rules(v: Option<&Value>) -> Vec<Rule> {
    let Some(map) = v.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<Rule> = map.iter().filter_map(|(id, r)| parse_rule(id, r)).collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Quiet hours, ET wall time, minutes after midnight. `start > end` wraps midnight (22:00–07:00).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuietHours {
    pub start_min: u32,
    pub end_min: u32,
}

impl QuietHours {
    pub fn contains(&self, minute_of_day: u32) -> bool {
        if self.start_min == self.end_min {
            return false;
        }
        if self.start_min < self.end_min {
            minute_of_day >= self.start_min && minute_of_day < self.end_min
        } else {
            minute_of_day >= self.start_min || minute_of_day < self.end_min
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub phone_budget: u32,
    pub quiet: Option<QuietHours>,
    /// The global sound switch (`sound.on`, default on): off sends drawing alerts' pushes on the
    /// plain alerts channel instead of a sounding drawing channel. The volume is the clients'.
    pub sound_on: bool,
    /// "Hide repeat alerts" (`hideRepeats`, owner 2026-11-22, **default off**): a Runners notice
    /// for a stock that was already sent on the same path within the last 5 minutes is held
    /// `repeat` (`super::runners`, "Limits"). A comfort choice, not a quality gain: research v2
    /// (W2-REPORT §3, Engine A) found it cuts alerts by 38 % but *lowers* precision, because
    /// repeats land on real runners. So it changes nothing unless the owner turns it on.
    pub hide_repeats: bool,
    /// "Group repeat alerts" (`groupRepeats`, owner 2026-11-25 after AKAN 11-24, **default on**):
    /// a stock's later notices of the same family (breakouts, follow-ups, the MP5 trigger /
    /// crossing / pullback notices, shelves, the Halt watcher gauge) in the same session part
    /// (pre-market, regular, after hours) are held `grouped` instead of pushed: the first one of
    /// each family per part rings, the rest update that stock's card (`super::runners`,
    /// "Limits" 4a). AKAN 11-24 sent 47 pushes; the one that mattered (the 09:33 breakout, +70 %
    /// still ahead) was the first regular-session breakout, which this keeps. `false` restores the
    /// 2026-10-30 rule (every distinct event pushed).
    pub group_repeats: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            phone_budget: DEFAULT_PHONE_BUDGET,
            quiet: None,
            sound_on: true,
            hide_repeats: false,
            group_repeats: true,
        }
    }
}

fn hhmm(v: Option<&Value>) -> Option<u32> {
    let s = v?.as_str()?;
    let (h, m) = s.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn parse_prefs(v: Option<&Value>) -> Prefs {
    let Some(o) = v.and_then(Value::as_object) else {
        return Prefs::default();
    };
    let phone_budget = int(o.get("phoneBudget"))
        .and_then(|b| PHONE_BUDGETS.iter().copied().find(|x| *x as i64 == b))
        .unwrap_or(DEFAULT_PHONE_BUDGET);
    let quiet = o
        .get("quietHours")
        .and_then(Value::as_object)
        .and_then(|q| {
            if q.get("on").and_then(Value::as_bool) != Some(true) {
                return None;
            }
            Some(QuietHours {
                start_min: hhmm(q.get("start"))?,
                end_min: hhmm(q.get("end"))?,
            })
        });
    let sound_on = o
        .get("sound")
        .and_then(Value::as_object)
        .and_then(|s| s.get("on"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    // Only an explicit `true` turns it on (absent, from every client before 2026-11-22: off).
    let hide_repeats = o.get("hideRepeats").and_then(Value::as_bool) == Some(true);
    // Only an explicit `false` turns it off (absent, from every client before 2026-11-25: on).
    let group_repeats = o.get("groupRepeats").and_then(Value::as_bool) != Some(false);
    Prefs {
        phone_budget,
        quiet,
        sound_on,
        hide_repeats,
        group_repeats,
    }
}

/// The evaluator's verdict for one alert, as stored in `alerts.state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Armed,
    Cooling,
    Done,
    Expired,
}

/// One `alerts.state` entry. Field names are the wire's (camelCase).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertState {
    pub status: Status,
    /// Cooling: the gap ends at this epoch ms (re-arming also needs the 0.5% move back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
    /// Cooling: the price has already moved back 0.5% (only the gap is left).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub moved_back: bool,
    /// The rule's `editedAt` this state belongs to (an edit starts it over).
    pub basis: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_price: Option<f64>,
    /// Fired upward (true) or downward: which way "moved back" is measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_up: Option<bool>,
    #[serde(default)]
    pub count: u32,
    /// The rule's snooze is running at the evaluator's clock (crate::alerts::eval `tick`). This
    /// server writes it on every entry, `false` included, so its presence tells a client that
    /// expiry, snooze and the cooling gap are decided here on the server's clock (`clock.v1`,
    /// crate::time_marks) and that it must not judge them against its own `Date.now()`. Absent
    /// in entries an older server wrote (the client then falls back to its own clock).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed: Option<bool>,
    /// A drawing alert whose drawing is gone (deleted on some device): it watches nothing until
    /// the drawing comes back (an undo), and the clients say so. Absent = false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub orphaned: bool,
}

impl AlertState {
    pub fn armed(basis: i64) -> Self {
        AlertState {
            status: Status::Armed,
            until: None,
            moved_back: false,
            basis,
            fired_at: None,
            fired_price: None,
            fired_up: None,
            count: 0,
            snoozed: None,
            orphaned: false,
        }
    }
}

/// The `alerts.state` map as stored (unparseable entries dropped: they are rewritten on the
/// next change of that alert).
pub fn parse_states(v: Option<&Value>) -> Vec<(String, AlertState)> {
    let Some(map) = v.and_then(Value::as_object) else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(id, s)| {
            serde_json::from_value(s.clone())
                .ok()
                .map(|s| (id.clone(), s))
        })
        .collect()
}

pub fn state_value(s: &AlertState) -> Value {
    serde_json::to_value(s).unwrap_or(Value::Null)
}

/// A map of entries for a settings patch.
pub fn patch_set(entries: impl IntoIterator<Item = (String, AlertState)>) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(k, s)| (k, state_value(&s)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rules_parse_defensively() {
        let v = json!({
            "a1": {"symbol": "xlab", "rule": {"kind": "price", "dir": "above", "price": 2.5}, "fire": "every", "gapMin": 2, "sessions": ["regular", "nope"], "expiry": "day", "expiresAt": 10, "notify": {"phone": false}, "createdAt": 5},
            "a2": {"symbol": "SAFX", "rule": {"kind": "move", "dir": "either", "pct": 5, "window": "15m"}},
            "bad1": {"symbol": "1BAD", "rule": {"kind": "price", "price": 2}},
            "bad2": {"symbol": "OK", "rule": {"kind": "price", "price": -1}},
            "bad3": {"symbol": "OK", "rule": {"kind": "pressure", "min": 0}},
            "bad4": "junk",
        });
        let rules = parse_rules(Some(&v));
        assert_eq!(rules.len(), 2);
        let a1 = &rules[0];
        assert_eq!(a1.symbol, "XLAB");
        assert_eq!(a1.fire, Fire::Every { gap_min: 5 }, "gap clamped to 5..=60");
        assert_eq!(a1.sessions, vec![Session::Regular]);
        assert_eq!(a1.expires_at, Some(10));
        assert!(a1.notify_app && !a1.notify_phone);
        assert_eq!(a1.edited_at, 5, "editedAt falls back to createdAt");
        let a2 = &rules[1];
        assert_eq!(
            a2.condition,
            Condition::Move {
                dir: MoveDir::Either,
                pct: 5.0,
                window: MoveWindow::Minutes(15)
            }
        );
        assert_eq!(a2.fire, Fire::Once);
        assert_eq!(a2.sessions, DEFAULT_SESSIONS.to_vec());
        assert_eq!(a2.expiry, Expiry::EndOfWeek);
        assert!(parse_rules(Some(&json!([1, 2]))).is_empty());
        assert!(parse_rules(None).is_empty());
    }

    #[test]
    fn drawing_rules_parse_with_defaults() {
        let v = json!({
            "d1": {"symbol": "XLAB", "rule": {"kind": "drawing", "drawingId": "dr-1", "part": "stop", "tol": {"unit": "pct", "value": 0.5}, "tf": "5m"}, "sound": false},
            "d2": {"symbol": "XLAB", "rule": {"kind": "drawing", "drawingId": "dr-2"}},
            "bad": {"symbol": "XLAB", "rule": {"kind": "drawing", "drawingId": ""}},
        });
        let rules = parse_rules(Some(&v));
        assert_eq!(rules.len(), 2);
        assert_eq!(
            rules[0].condition,
            Condition::Drawing {
                drawing_id: "dr-1".into(),
                part: Part::Stop,
                tolerance: Tolerance::Pct(0.5),
                tf_ms: 300_000
            }
        );
        assert!(!rules[0].sound);
        assert_eq!(
            rules[1].condition,
            Condition::Drawing {
                drawing_id: "dr-2".into(),
                part: Part::Line,
                tolerance: Tolerance::Ticks(1),
                tf_ms: 60_000
            },
            "a line, one tick, the 1 minute chart"
        );
        assert!(rules[1].sound, "sound defaults on");
        let p = parse_prefs(Some(&json!({"sound": {"on": false, "volume": 0.3}})));
        assert!(!p.sound_on);
        assert!(Prefs::default().sound_on);
    }

    #[test]
    fn prefs_and_quiet_hours() {
        assert_eq!(parse_prefs(None), Prefs::default());
        let p = parse_prefs(Some(
            &json!({"phoneBudget": 7, "quietHours": {"on": true, "start": "22:00", "end": "07:30"}}),
        ));
        assert_eq!(
            p.phone_budget, DEFAULT_PHONE_BUDGET,
            "only the offered budgets"
        );
        let q = p.quiet.unwrap();
        assert!(q.contains(23 * 60) && q.contains(60) && !q.contains(8 * 60));
        let p = parse_prefs(Some(
            &json!({"phoneBudget": 50, "quietHours": {"on": false, "start": "22:00", "end": "07:00"}}),
        ));
        assert_eq!(p.phone_budget, 50);
        assert!(p.quiet.is_none());
    }

    #[test]
    fn state_round_trips() {
        let mut s = AlertState::armed(3);
        s.status = Status::Cooling;
        s.until = Some(99);
        let v = state_value(&s);
        assert_eq!(v["status"], "cooling");
        assert_eq!(v["until"], 99);
        assert!(v.get("orphaned").is_none(), "absent while false");
        let back = parse_states(Some(&json!({"x": v, "y": "junk"})));
        assert_eq!(back, vec![("x".to_string(), s.clone())]);
        s.orphaned = true;
        assert_eq!(state_value(&s)["orphaned"], true);
    }
}
