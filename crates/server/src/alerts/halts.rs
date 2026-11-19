//! Exchange halts and resumptions as notifications (owner-approved plan "halt sounds", release 1,
//! 2026-11-16): two events per stock, **Halted** and **Resumed**, from the exchange's own
//! trading-status feed (Alpaca SIP `s` messages, `market_providers::alpaca_stream::protocol`
//! `status_kind`: `2`/`H`/`P` halt, `3`/`T` resume). This is NOT the Halt watcher
//! (`crate::pressure`, `halt.watch`), which forecasts a possible LULD pause; that is unchanged.
//!
//! - **Input:** the alerts task already receives every live symbol's eligible prints and its
//!   halt/resume statuses ([`super::eval::Input`], offered by the market for loaded and held
//!   symbols alike). This tracker reads them before the evaluator does: the evaluator only
//!   keeps symbols a rule watches, and a halt notice is about symbols no rule may watch.
//! - **Coverage** (decided by the task at each status, `super::Task::halt_covered`): a stock on
//!   today's Runners board, a symbol an open live client has on a chart (open or linked, one of
//!   its `chart.subscribe` slots), or a symbol with a per-stock halt rule. Everything else is
//!   tracked (so a later resume knows its halt) but never notified.
//! - **One notification per event**, through the Runners notices' own delivery
//!   (`super::runners`: dedupe persisted per day, the devices that want the kind, quiet hours,
//!   the 3-minute stale drop; no daily cap), so the ledger and the Insights log record every
//!   decision, held ones included. A repeated halt status while halted, or a repeated resume,
//!   is not news. The event id is the symbol and the halt's start second, the feed's own clock,
//!   so a stream reconnect or a restart that replays the same status is a `duplicate`.
//! - **Resumed waits for the reopening print**, up to [`RESUME_WAIT_MS`]: the status arrives
//!   before the reopening cross prints, so the price at the status is still the halt price and
//!   "the change from the halt price" would always read 0. Without a print in time it goes out
//!   with the halt price and says "no print yet". Its age for the stale rule is the status time.
//! - **A resumption the tracker never saw halt** (a restart during the halt) is not notified:
//!   with no halt start or price there is nothing true to say about it, and stray resumption
//!   codes (an IPO opening, a start-of-day status) would ring. Deploys stay outside market hours
//!   (docs/CONVENTIONS.md "Deploy window"), so this is rare.
//! - **Sounds:** [`SOUNDS`] are the keys of the built-in library (packages/app-state
//!   `HALT_SOUNDS`, the score the web plays and apps/mobile/scripts/tones.ts renders). The notice
//!   carries its event's default key ([`DEFAULT_HALTED_SOUND`], [`DEFAULT_RESUMED_SOUND`]); a
//!   phone's push carries the phone's own choice (`super::push::Device`), and a desktop or web
//!   client plays its own.

use std::collections::HashMap;

use crate::rocket::{Notice, NoticeDetail, NoticeKind, TradingNotice};

/// The built-in sounds' keys (packages/app-state `HALT_SOUND_KEYS`; the two must match), "none"
/// last. Each is a short synthesized sound, original and licence-clean.
pub const SOUNDS: [&str; 9] = [
    "siren", "klaxon", "triple", "alarm", "chime", "ding", "rise", "blip", "none",
];
/// Halted: an urgent two-tone sweep.
pub const DEFAULT_HALTED_SOUND: &str = "siren";
/// Resumed: a lighter two-note chime.
pub const DEFAULT_RESUMED_SOUND: &str = "chime";

/// A sound key as the library spells it, or None for anything else (an unknown key from a newer
/// or broken client is ignored, never stored).
pub fn sound_key(s: &str) -> Option<&'static str> {
    SOUNDS.iter().copied().find(|k| *k == s)
}

/// How long a Resumed notice waits for the reopening print (module docs).
pub const RESUME_WAIT_MS: i64 = 15_000;
/// A print up to this long before the resume status still counts as the reopening print: the
/// status and the print carry different clocks (SIP vs the participant's).
const PRINT_SKEW_MS: i64 = 5_000;

/// The reason as the text says it. Alpaca's `rc` is the exchanges' reason code (Nasdaq Trader's
/// halt codes; `Input::Status` falls back to the status code when there is none); a CTA halt may
/// carry only `rm`, a phrase, which is matched by its words.
pub fn reason_label(reason: &str) -> String {
    let r = reason.trim();
    let known = match r.to_ascii_uppercase().as_str() {
        "LUDP" | "LUDS" | "P" => Some("LULD pause"),
        "T1" => Some("News pending"),
        "T2" => Some("News released"),
        "T5" => Some("Single-stock pause"),
        "T6" => Some("Extraordinary activity"),
        "T8" => Some("ETF halt"),
        "T12" => Some("Info requested"),
        "H4" => Some("Non-compliance"),
        "H9" => Some("Filings not current"),
        "H10" => Some("SEC suspension"),
        "H11" => Some("Regulatory concern"),
        "O1" => Some("Operations halt"),
        "IPO1" => Some("IPO not yet trading"),
        "M1" => Some("Corporate action"),
        "M2" => Some("Quote unavailable"),
        "MWC0" => Some("Circuit breaker carry-over"),
        "MWC1" => Some("Circuit breaker level 1"),
        "MWC2" => Some("Circuit breaker level 2"),
        "MWC3" => Some("Circuit breaker level 3"),
        "" | "H" | "2" => Some("Trading halt"),
        _ => None,
    };
    if let Some(k) = known {
        return k.to_owned();
    }
    let lower = r.to_ascii_lowercase();
    if lower.contains("volatility") || lower.contains("luld") {
        return "LULD pause".to_owned();
    }
    if lower.contains("news pending") {
        return "News pending".to_owned();
    }
    // Anything else as the feed said it, kept short for a lock screen.
    let mut s: String = r.chars().filter(|c| !c.is_control()).take(40).collect();
    if s.is_empty() {
        s = "Trading halt".to_owned();
    }
    s
}

#[derive(Clone, Debug)]
struct Open {
    start_ms: i64,
    price: Option<f64>,
    reason: String,
}

#[derive(Clone, Debug)]
struct Pending {
    resume_ms: i64,
    /// Wall clock when the status arrived: the wait runs on it.
    since_ms: i64,
    open: Open,
}

/// The halt state of every live symbol, and the Resumed notices waiting for their print.
#[derive(Debug, Default)]
pub struct HaltTracker {
    last: HashMap<String, f64>,
    open: HashMap<String, Open>,
    pending: HashMap<String, Pending>,
}

impl HaltTracker {
    pub fn halted(&self, symbol: &str) -> bool {
        self.open.contains_key(symbol)
    }

    /// An eligible print: the stock's last price, and a waiting Resumed notice's reopening print.
    pub fn on_trade(&mut self, symbol: &str, price: f64, ts: i64) -> Option<Notice> {
        if !(price.is_finite() && price > 0.0) {
            return None;
        }
        match self.last.get_mut(symbol) {
            Some(p) => *p = price,
            None => {
                self.last.insert(symbol.to_owned(), price);
            }
        }
        let p = self.pending.get(symbol)?;
        if ts < p.resume_ms - PRINT_SKEW_MS || ts <= p.open.start_ms {
            return None;
        }
        let p = self.pending.remove(symbol)?;
        Some(resumed(symbol, &p, price, false, Some(ts)))
    }

    /// A halt (`halted`) or resume status at `ts` (feed clock, ms). `covered`: whether this
    /// stock's events are notified now (module docs, "Coverage"); `prev_close` for the day's %.
    /// `now` is the wall clock (ms). A halt that arrives while a Resumed still waits for its
    /// print sends that Resumed first (with no print), then the new halt.
    #[allow(clippy::too_many_arguments)]
    pub fn on_status(
        &mut self,
        symbol: &str,
        halted: bool,
        ts: i64,
        reason: &str,
        covered: bool,
        prev_close: Option<f64>,
        now: i64,
    ) -> Vec<Notice> {
        let mut out = Vec::new();
        if halted {
            if self.open.contains_key(symbol) {
                return out;
            }
            if let Some(p) = self.pending.remove(symbol) {
                let price = self.last.get(symbol).copied().or(p.open.price);
                if let Some(price) = price {
                    out.push(resumed(symbol, &p, price, true, None));
                }
            }
            let open = Open {
                start_ms: ts,
                price: self.last.get(symbol).copied(),
                reason: reason_label(reason),
            };
            self.open.insert(symbol.to_owned(), open.clone());
            if covered {
                out.push(halted_notice(symbol, &open, prev_close));
            }
            return out;
        }
        // A resume with no halt known: nothing true to say (module docs).
        let Some(open) = self.open.remove(symbol) else {
            return out;
        };
        if covered {
            self.pending.insert(
                symbol.to_owned(),
                Pending {
                    resume_ms: ts,
                    since_ms: now,
                    open,
                },
            );
        }
        out
    }

    /// The Resumed notices whose print did not come within [`RESUME_WAIT_MS`]: sent with the
    /// last price known (the halt price).
    pub fn tick(&mut self, now: i64) -> Vec<Notice> {
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| now - p.since_ms >= RESUME_WAIT_MS)
            .map(|(s, _)| s.clone())
            .collect();
        let mut out = Vec::new();
        for s in due {
            let Some(p) = self.pending.remove(&s) else {
                continue;
            };
            let price = self.last.get(&s).copied().or(p.open.price).unwrap_or(0.0);
            out.push(resumed(&s, &p, price, true, None));
        }
        out
    }
}

/// The halt's id: symbol and start second, the feed's clock (the dedupe key's tail).
fn halt_id(symbol: &str, start_ms: i64) -> String {
    format!("{symbol}-halt-{}", start_ms.div_euclid(1000))
}

fn halted_notice(symbol: &str, open: &Open, prev_close: Option<f64>) -> Notice {
    let day_pct = open
        .price
        .zip(prev_close.filter(|c| c.is_finite() && *c > 0.0))
        .map(|(p, c)| (p / c - 1.0) * 100.0);
    Notice {
        spread_bp: None,
        kind: NoticeKind::Halted,
        alert_id: halt_id(symbol, open.start_ms),
        symbol: symbol.to_owned(),
        at_ns: open.start_ms.saturating_mul(1_000_000),
        price: open.price.unwrap_or(0.0),
        from_flag_pct: None,
        reasons: vec![open.reason.clone()],
        since_start_min: None,
        exec_ns: None,
        detail: Some(NoticeDetail::Trading(TradingNotice {
            reason: open.reason.clone(),
            day_pct,
            halt_price: open.price,
            halted_ms: None,
            no_print: false,
            sound: DEFAULT_HALTED_SOUND,
            test: false,
        })),
    }
}

fn resumed(symbol: &str, p: &Pending, price: f64, no_print: bool, exec_ms: Option<i64>) -> Notice {
    Notice {
        spread_bp: None,
        kind: NoticeKind::Resumed,
        alert_id: halt_id(symbol, p.open.start_ms),
        symbol: symbol.to_owned(),
        at_ns: p.resume_ms.saturating_mul(1_000_000),
        price,
        from_flag_pct: None,
        reasons: vec![p.open.reason.clone()],
        since_start_min: None,
        exec_ns: exec_ms.map(|t| t.saturating_mul(1_000_000)),
        detail: Some(NoticeDetail::Trading(TradingNotice {
            reason: p.open.reason.clone(),
            day_pct: None,
            halt_price: p.open.price,
            halted_ms: Some((p.resume_ms - p.open.start_ms).max(0)),
            no_print,
            sound: DEFAULT_RESUMED_SOUND,
            test: false,
        })),
    }
}

/// A Settings "Send test" notice (`halts.test`, `crate::ws`): the real text and frame for a made-up
/// stock, a new id each time so no client's dedupe swallows it. Never decided or logged: it goes
/// only to the connection that asked.
pub fn test_notice(resumed_kind: bool, now_ms: i64) -> Notice {
    let open = Open {
        start_ms: now_ms - 4 * 60_000 - 30_000,
        price: Some(2.76),
        reason: "LULD pause".into(),
    };
    let mut n = if resumed_kind {
        resumed(
            "TEST",
            &Pending {
                resume_ms: now_ms,
                since_ms: now_ms,
                open,
            },
            3.10,
            false,
            None,
        )
    } else {
        let mut n = halted_notice("TEST", &open, Some(1.94));
        n.at_ns = now_ms.saturating_mul(1_000_000);
        n
    };
    n.alert_id = format!("test-{now_ms}");
    if let Some(NoticeDetail::Trading(t)) = &mut n.detail {
        t.test = true;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(v: &[Notice]) -> Vec<NoticeKind> {
        v.iter().map(|n| n.kind).collect()
    }

    #[test]
    fn halt_then_resume_is_one_halted_and_one_resumed() {
        let mut t = HaltTracker::default();
        assert!(t.on_trade("XLAB", 2.76, 1_000).is_none());
        let h = t.on_status("XLAB", true, 2_000, "LUDP", true, Some(1.94), 2_100);
        assert_eq!(kinds(&h), vec![NoticeKind::Halted]);
        assert_eq!(h[0].price, 2.76);
        assert_eq!(h[0].alert_id, "XLAB-halt-2");
        // A repeated halt status (UTP sends H, then P; a reconnect resends) is not news.
        assert!(
            t.on_status("XLAB", true, 2_500, "LUDP", true, None, 2_600)
                .is_empty()
        );
        // The resume waits for the reopening print.
        let r = t.on_status("XLAB", false, 302_000, "", true, None, 302_050);
        assert!(r.is_empty());
        assert!(t.tick(302_060).is_empty());
        let r = t
            .on_trade("XLAB", 3.10, 302_400)
            .expect("the reopening print sends it");
        assert_eq!(r.kind, NoticeKind::Resumed);
        assert_eq!(r.price, 3.10);
        assert_eq!(r.alert_id, "XLAB-halt-2");
        assert_eq!(r.at_ns, 302_000 * 1_000_000);
        match &r.detail {
            Some(NoticeDetail::Trading(d)) => {
                assert_eq!(d.halted_ms, Some(300_000));
                assert_eq!(d.halt_price, Some(2.76));
                assert!(!d.no_print);
            }
            other => panic!("{other:?}"),
        }
        // Nothing more: a repeated resume, later prints, the tick.
        assert!(
            t.on_status("XLAB", false, 302_500, "", true, None, 302_600)
                .is_empty()
        );
        assert!(t.on_trade("XLAB", 3.2, 303_000).is_none());
        assert!(t.tick(400_000).is_empty());
    }

    #[test]
    fn an_uncovered_symbol_never_notifies_but_is_tracked() {
        let mut t = HaltTracker::default();
        t.on_trade("NOPE", 5.0, 0);
        assert!(
            t.on_status("NOPE", true, 1_000, "T1", false, None, 1_000)
                .is_empty()
        );
        assert!(t.halted("NOPE"));
        assert!(
            t.on_status("NOPE", false, 9_000, "", false, None, 9_000)
                .is_empty()
        );
        assert!(t.on_trade("NOPE", 5.5, 9_100).is_none());
        assert!(t.tick(60_000).is_empty());
        assert!(!t.halted("NOPE"));
    }

    #[test]
    fn a_resume_without_a_print_goes_out_after_the_wait() {
        let mut t = HaltTracker::default();
        t.on_trade("AAA", 1.5, 0);
        t.on_status("AAA", true, 1_000, "T1", true, None, 1_000);
        t.on_status("AAA", false, 61_000, "", true, None, 61_000);
        assert!(t.tick(61_000 + RESUME_WAIT_MS - 1).is_empty());
        let r = t.tick(61_000 + RESUME_WAIT_MS);
        assert_eq!(kinds(&r), vec![NoticeKind::Resumed]);
        assert_eq!(r[0].price, 1.5);
        assert!(matches!(&r[0].detail, Some(NoticeDetail::Trading(d)) if d.no_print));
        // A print from before the halt does not count as the reopening print.
        t.on_status("AAA", true, 100_000, "LUDP", true, None, 100_000);
        t.on_status("AAA", false, 400_000, "", true, None, 400_000);
        assert!(t.on_trade("AAA", 1.6, 90_000).is_none());
    }

    #[test]
    fn a_resume_never_seen_halting_is_not_notified() {
        let mut t = HaltTracker::default();
        assert!(
            t.on_status("AAA", false, 1_000, "", true, None, 1_000)
                .is_empty()
        );
        assert!(t.tick(100_000).is_empty());
    }

    #[test]
    fn a_new_halt_flushes_a_waiting_resume_first() {
        let mut t = HaltTracker::default();
        t.on_trade("AAA", 2.0, 0);
        t.on_status("AAA", true, 1_000, "LUDP", true, None, 1_000);
        t.on_status("AAA", false, 301_000, "", true, None, 301_000);
        let o = t.on_status("AAA", true, 305_000, "LUDP", true, None, 305_000);
        assert_eq!(kinds(&o), vec![NoticeKind::Resumed, NoticeKind::Halted]);
        assert_ne!(o[0].event_id(), o[1].event_id());
        assert_eq!(o[1].alert_id, "AAA-halt-305");
    }

    #[test]
    fn reasons_read_as_words() {
        assert_eq!(reason_label("LUDP"), "LULD pause");
        assert_eq!(reason_label("T1"), "News pending");
        assert_eq!(reason_label("Volatility Trading Pause"), "LULD pause");
        assert_eq!(reason_label(""), "Trading halt");
        assert_eq!(reason_label("H"), "Trading halt");
        assert_eq!(reason_label("H10"), "SEC suspension");
        assert_eq!(reason_label("Something New"), "Something New");
    }

    #[test]
    fn sound_keys() {
        assert_eq!(sound_key("siren"), Some("siren"));
        assert_eq!(sound_key("none"), Some("none"));
        assert_eq!(sound_key("bogus"), None);
        assert!(SOUNDS.contains(&DEFAULT_HALTED_SOUND));
        assert!(SOUNDS.contains(&DEFAULT_RESUMED_SOUND));
    }
}
