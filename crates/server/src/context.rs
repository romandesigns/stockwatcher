//! Stock context and the Ross Panel on the server (`STOCK_CONTEXT=1`; off by default).
//!
//! One [`stock_context::Engine`] per market `Core`, so the live and the replay market never share
//! state, and a replay seek (which replaces the `Core`) rebuilds it from the re-read day. It is fed
//! every print ONCE, on arrival, with the verdict the market has for it: the chart engine's when
//! the symbol is loaded, the print's own conditions (`Counted::from_effect`) when its history is
//! not loaded yet (`crate::history` holds it back from the chart engine; the context engine must
//! not lose it, and must not count it again when it is applied later). The admission time is the
//! print's receive time when the feed knows it (live, recorded live), else its execution time.
//!
//! Evidence today: the raw previous close from `daily-closes.json` (basis raw, corporate actions
//! unchecked), so the gain pillar is UNKNOWN by design; RVOL, float and news are UNKNOWN until
//! their sources are wired (docs/stock-context/pillars-v1.md). The panel will therefore be empty
//! in production until then: an honest empty state, not a bug.
//!
//! The panel goes out as `ross.snapshot` / `ross.update` (the whole panel, at most once per wall
//! second, only when its revision changed), a symbol's latest decision as `context.decision`.

use crate::quotes::Counted;
use chrono::NaiveDate;
use market_archive::PrevCloses;
use market_events::{Price, Trade, UnixNanos};
use serde_json::json;
use std::collections::HashSet;
use stock_context::{
    CloseBasis, Coverage, Decision, Engine, Fidelity, InstrumentId, PanelPolicy, Policy, PrevClose,
    RunInfo, Verdict,
};

pub(crate) struct ContextBoard {
    engine: Engine,
    /// Symbols whose previous close was offered to the engine, per trading date.
    closes_offered: HashSet<(String, NaiveDate)>,
    /// The panel revision last published (`ross.update` only when it moves).
    published: Option<(u64, i64)>,
}

impl ContextBoard {
    /// `None` when the panel policy's platform band is not the server's price band: membership
    /// would then judge eligibility by a band the rest of the app does not use.
    pub(crate) fn new(
        live: bool,
        day: Option<NaiveDate>,
        band_min: Price,
        band_max: Price,
    ) -> Option<ContextBoard> {
        let panel = PanelPolicy::v1();
        let b = &panel.body.membership.platform_band;
        if b.min_micros != band_min.micros() || b.max_micros != band_max.micros() {
            tracing::error!(
                policy_min = b.min_micros,
                policy_max = b.max_micros,
                band_min = band_min.micros(),
                band_max = band_max.micros(),
                "STOCK_CONTEXT: the Ross Panel policy's platform band is not PRICE_BAND; context disabled"
            );
            return None;
        }
        let run = RunInfo {
            run_id: match (live, day) {
                (true, _) => "live".to_string(),
                (false, Some(d)) => format!("replay:{d}"),
                (false, None) => "replay".to_string(),
            },
            source: if live { "live" } else { "replay" }.to_string(),
            // A replayed day may be a live recording or a corrected import; without telling them
            // apart here, claim the weaker one (no latency claims from a replay).
            fidelity: if live {
                Fidelity::RecordedLive
            } else {
                Fidelity::ReconstructedHistory
            },
            // No coverage records are consulted yet (docs: recording provenance).
            numerator_coverage: Coverage::Unverified,
        };
        Some(ContextBoard {
            engine: Engine::new(Policy::v1(), run).with_panel_policy(panel),
            closes_offered: HashSet::new(),
            published: None,
        })
    }

    pub(crate) fn apply_trade(&mut self, t: &Trade, counted: Counted, closes: &PrevCloses) {
        if let Some(s) = t.session {
            let key = (t.symbol.clone(), s.date);
            if !self.closes_offered.contains(&key) {
                if let Some((date, close)) = closes.before(&t.symbol, s.date) {
                    self.engine.add_prev_close(PrevClose {
                        id: format!("daily-closes:{}:{date}", t.symbol),
                        version: 1,
                        instrument: InstrumentId::symbol(&t.symbol),
                        date,
                        close_micros: close.micros(),
                        basis: CloseBasis::Raw,
                        actions_checked_through: None,
                        // Not checked, so no evidence of a check either (#53's field).
                        actions: Default::default(),
                        // Loaded before the session: visible to every decision of the day.
                        observed_at: 0,
                    });
                }
                self.closes_offered.insert(key);
            }
        }
        let admit = t.receive_ts.unwrap_or(t.event_ts);
        // Decisions stay in the engine (latest per symbol); the append-only log is a follow-up.
        let _ = self.engine.apply(
            t,
            Verdict {
                ohlc: counted.ohlc,
                volume: counted.volume,
            },
            admit,
        );
    }

    pub(crate) fn advance(&mut self, now: UnixNanos) {
        let _ = self.engine.advance_to(now);
    }

    pub(crate) fn set_halted(&mut self, symbol: &str, halted: bool) {
        self.engine.set_halted(symbol, halted);
    }

    /// The whole panel as a `ross.snapshot` / `ross.update` frame (empty `rows` before any).
    pub(crate) fn panel_frame(&self, kind: &str) -> String {
        let panel = self.engine.panel();
        json!({
            "type": kind,
            "enabled": true,
            "panelPolicy": self.engine.panel_policy().tag(),
            "pillarPolicy": self.engine.policy().tag(),
            "panel": panel,
        })
        .to_string()
    }

    /// The `ross.update` frame when the panel changed since the last call.
    pub(crate) fn take_update(&mut self) -> Option<String> {
        let _ = self.engine.take_panels();
        let p = self.engine.panel()?;
        let at = (p.revision, p.bar_end);
        if self.published == Some(at) {
            return None;
        }
        self.published = Some(at);
        Some(self.panel_frame("ross.update"))
    }

    pub(crate) fn decision(&self, symbol: &str) -> Option<&Decision> {
        self.engine.current().find(|d| d.symbol == symbol)
    }
}

/// `ross.snapshot` while the feature is off: the client shows "not enabled", never an empty panel
/// that would read as "nothing qualifies".
pub(crate) fn disabled_frame() -> String {
    json!({ "type": "ross.snapshot", "enabled": false, "panel": null }).to_string()
}

pub(crate) fn decision_frame(symbol: &str, d: Option<&Decision>, enabled: bool) -> String {
    json!({ "type": "context.decision", "symbol": symbol, "enabled": enabled, "decision": d })
        .to_string()
}
