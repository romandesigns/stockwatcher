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
//! Policies: `pillars-v2` ([`Policy::current`]: the price pillar on the app's own $0.10–$25 band,
//! 120 s news coverage lag), `ross-panel-v1` and the EXPERIMENTAL `setups-v0`, so every panel
//! snapshot carries each row's `setup` and the snapshot's `attention`. Both bands are checked
//! against `PRICE_BAND_MIN` / `MAX` at construction.
//!
//! Evidence (`crate::evidence`): float, news, market-wide news coverage, and the previous close
//! and daily baseline built from the stored daily bars and corporate-action checks, each with its
//! own `observed_at`, delivered by the market's evidence task ([`ContextBoard::add_evidence`]).
//! The raw previous close from `daily-closes.json` is still offered as a fallback: it carries no
//! corporate-action check, so it can never pass the gain pillar (only feed the
//! `gain_vs_stored_close_bp` diagnostic), and with `observed_at` 0 it loses every join to a
//! baseline-built close of the same day, whose actions coverage is what lets the gain pillar
//! decide at all. Where the maintainer has not built one (a symbol outside the watch list, a day
//! not imported yet) the fallback keeps today's behaviour.
//!
//! The panel goes out as `ross.snapshot` / `ross.update` (the whole panel, at most once per wall
//! second, only when its revision changed), a symbol's latest decision as `context.decision`.

use crate::evidence::{Cursor, EvidenceBatch};
use crate::quotes::Counted;
use chrono::NaiveDate;
use market_archive::PrevCloses;
use market_events::{Price, Trade, UnixNanos};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeSet, HashSet};
use stock_context::{
    CloseBasis, Coverage, Decision, Engine, Fidelity, InstrumentId, OutputTelemetry, PanelPolicy,
    PanelSnapshot, PillarId, Policy, PrevClose, Qualification, RunInfo, SetupPolicy, Tri, Verdict,
};

/// What one [`ContextBoard::drain_outputs`] took off the engine, and what the engine had to drop
/// before it got there.
///
/// `panels` and `setup_entries` are counts, not a claim of persistence: this slice drains so the
/// engine stays bounded and so the counts are honest. Nothing here is written to disk, and
/// `dropped` is what a complete journal would have had to include.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DrainReport {
    pub(crate) panels: usize,
    pub(crate) setup_entries: usize,
}

impl DrainReport {
    pub(crate) fn is_empty(&self) -> bool {
        self.panels == 0 && self.setup_entries == 0
    }
}

pub(crate) struct ContextBoard {
    engine: Engine,
    /// Symbols whose previous close was offered to the engine, per trading date.
    closes_offered: HashSet<(String, NaiveDate)>,
    /// The panel revision last published (`ross.update` only when it moves).
    published: Option<(u64, i64)>,
    /// How far this board has taken the market's evidence log (`None`: nothing yet).
    evidence: Option<Cursor>,
    /// Outputs this board has drained since it was built (`ross` viewers do not change it).
    drained: DrainReport,
}

impl ContextBoard {
    /// `None` when the pillar policy's price band or the panel policy's platform band is not the
    /// server's price band: the price pillar and membership would then judge by a band the rest
    /// of the app (the quote board, the collector's universe) does not use.
    pub(crate) fn new(
        live: bool,
        day: Option<NaiveDate>,
        band_min: Price,
        band_max: Price,
    ) -> Option<ContextBoard> {
        let policy = Policy::current();
        let panel = PanelPolicy::v1();
        let b = &panel.body.membership.platform_band;
        let prof = &policy.body.reference_profile;
        let band = (band_min.micros(), band_max.micros());
        if (prof.price_min_micros, prof.price_max_micros) != band
            || (b.min_micros, b.max_micros) != band
        {
            tracing::error!(
                pillar_policy = %policy.tag(),
                price_min = prof.price_min_micros,
                price_max = prof.price_max_micros,
                platform_min = b.min_micros,
                platform_max = b.max_micros,
                band_min = band_min.micros(),
                band_max = band_max.micros(),
                "STOCK_CONTEXT: the pillar policy's price band or the Ross Panel's platform band is not PRICE_BAND; context disabled"
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
            engine: Engine::new(policy, run)
                .with_panel_policy(panel)
                .with_setup_policy(SetupPolicy::v0()),
            closes_offered: HashSet::new(),
            published: None,
            evidence: None,
            drained: DrainReport::default(),
        })
    }

    /// The same board with other engine output bounds (tests; production keeps the defaults).
    #[cfg(test)]
    pub(crate) fn with_output_bounds(self, panels: usize, setup_entries: usize) -> Self {
        ContextBoard {
            engine: self.engine.with_output_bounds(panels, setup_entries),
            ..self
        }
    }

    /// Takes the engine's bounded output journals, whatever any client is doing.
    ///
    /// The engine owns no consumer: it appends panel snapshots and setup-log entries and its
    /// caller takes them. Draining used to happen only inside [`ContextBoard::take_update`],
    /// which the market skips when nobody is subscribed to `ross`, and the setup log had no
    /// caller at all — so an unattended run grew both for ever. This is called every tick
    /// regardless of viewers. In-memory only (it runs under the core lock): no disk, no provider,
    /// no serialization.
    ///
    /// The drained rows are dropped here. That is deliberate and limited: this slice makes the
    /// engine bounded and the loss counted, it does not persist a decision journal, so nothing
    /// here may be presented as a complete audit trail.
    /// `observe` sees each drained snapshot before it is discarded, so the baseline
    /// observation/latency capture (`crate::observe`) can record its segment times. It must stay
    /// cheap: this runs under the core lock.
    pub(crate) fn drain_outputs(&mut self, observe: &mut dyn FnMut(&PanelSnapshot)) -> DrainReport {
        let panels = self.engine.take_panels();
        for p in &panels {
            observe(p);
        }
        let r = DrainReport {
            panels: panels.len(),
            setup_entries: self.engine.take_setup_log().len(),
        };
        self.drained.panels += r.panels;
        self.drained.setup_entries += r.setup_entries;
        r
    }

    /// Everything this board has drained since it was built.
    pub(crate) fn drained(&self) -> DrainReport {
        self.drained
    }

    /// Per-pillar readiness over the latest decision of every symbol, with the commonest reasons
    /// a pillar could not decide.
    ///
    /// One pass over the decisions the engine already keeps, in memory. It is taken on the
    /// evidence task and cached, not computed per frame: a whole-market pass belongs nowhere near
    /// the publish path.
    pub(crate) fn readiness(&self, observed_at: i64) -> Readiness {
        use std::collections::HashMap;
        let mut pillars: Vec<PillarReadiness> = PillarId::ALL
            .iter()
            .map(|p| PillarReadiness {
                pillar: p.as_str(),
                ..PillarReadiness::default()
            })
            .collect();
        let mut reasons: HashMap<(&'static str, &'static str), u32> = HashMap::new();
        let mut r = Readiness {
            evidence_records: self.engine.evidence_len(),
            observed_at,
            ..Readiness::default()
        };
        for d in self.engine.current() {
            r.symbols += 1;
            if d.qualification == Qualification::Qualified {
                r.qualified += 1;
            }
            if d.membership.active().is_some() {
                r.members += 1;
            }
            for p in &d.pillars {
                let Some(slot) = pillars.iter_mut().find(|x| x.pillar == p.pillar.as_str()) else {
                    continue;
                };
                match p.state {
                    Tri::Pass => slot.pass += 1,
                    Tri::Fail => slot.fail += 1,
                    Tri::Unknown => {
                        slot.unknown += 1;
                        *reasons.entry((p.pillar.as_str(), p.reason)).or_default() += 1;
                    }
                }
            }
        }
        let mut blocking: Vec<ReasonCount> = reasons
            .into_iter()
            .map(|((pillar, reason), symbols)| ReasonCount {
                pillar,
                reason,
                symbols,
            })
            .collect();
        blocking.sort_by(|a, b| {
            b.symbols
                .cmp(&a.symbols)
                .then(a.pillar.cmp(b.pillar))
                .then(a.reason.cmp(b.reason))
        });
        blocking.truncate(TOP_REASONS);
        r.pillars = pillars;
        r.blocking = blocking;
        r
    }

    /// Symbols in the band whose latest decision is missing evidence a warmup could supply, with
    /// the reason, at most `max` ([`warmup_reason`]) — **skipping** the ones `asked` already
    /// holds.
    ///
    /// Skipping before the budget is taken, not after, is the whole point. The engine keeps its
    /// symbols in a stable order, so taking the first `max` candidates and discarding the
    /// already-asked ones afterwards means a first batch that never resolves (a provider outage,
    /// attempts spent, a legitimately unanswerable symbol) blocks every candidate behind it for
    /// the rest of the day. With the skip first, each tick moves on to the next ones.
    ///
    /// In-memory only: it reads the latest decision per symbol, which is what the engine already
    /// keeps, and allocates only the bounded result. Called under the core lock, so the caller
    /// files the requests and writes the inbox afterwards, off the lock and off the market path.
    pub(crate) fn missing_evidence(
        &self,
        max: usize,
        asked: &BTreeSet<String>,
    ) -> Vec<(String, &'static str)> {
        let mut out = Vec::new();
        for d in self.engine.current() {
            if out.len() >= max {
                break;
            }
            if asked.contains(&d.symbol) {
                continue;
            }
            if let Some(r) = warmup_reason(d) {
                out.push((d.symbol.clone(), r));
            }
        }
        out
    }

    /// The engine's output counters, drops included ([`stock_context::OutputTelemetry`]).
    pub(crate) fn output_telemetry(&self) -> OutputTelemetry {
        self.engine.output_telemetry()
    }

    /// Where this board is in the market's evidence log.
    pub(crate) fn evidence_cursor(&self) -> Option<Cursor> {
        self.evidence
    }

    /// Takes evidence records into the engine as they are (each joins decisions from its own
    /// `observed_at` on), then drops what no later decision can read. In-memory only: called
    /// under the core lock.
    pub(crate) fn add_evidence(&mut self, b: EvidenceBatch) {
        self.evidence = Some((b.generation, b.through));
        for r in b.closes {
            self.engine.add_prev_close(r);
        }
        for r in b.baselines {
            self.engine.add_daily(r);
        }
        for r in b.floats {
            self.engine.add_float(r);
        }
        for r in b.news {
            self.engine.add_news(r);
        }
        for r in b.coverage {
            self.engine.add_coverage(r);
        }
        self.engine.prune_evidence();
    }

    /// Evidence records the engine holds (logs).
    pub(crate) fn evidence_len(&self) -> usize {
        self.engine.evidence_len()
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

    /// The whole panel as a `ross.snapshot` / `ross.update` frame (empty `rows` before any),
    /// with the cached readiness summary when there is one: an empty panel then says *why*
    /// instead of reading as "nothing qualifies".
    pub(crate) fn panel_frame(&self, kind: &str, readiness: Option<&Readiness>) -> String {
        let panel = self.engine.panel();
        json!({
            "type": kind,
            "enabled": true,
            "panelPolicy": self.engine.panel_policy().tag(),
            "pillarPolicy": self.engine.policy().tag(),
            "panel": panel,
            "readiness": readiness,
        })
        .to_string()
    }

    /// The `ross.update` frame when the panel changed since the last call, with the snapshot it
    /// speaks for (`crate::observe` records the publication time against it).
    ///
    /// It reads the latest snapshot only; draining the journals is
    /// [`ContextBoard::drain_outputs`]'s job, so what the engine holds never depends on whether
    /// anyone is watching.
    pub(crate) fn take_update(
        &mut self,
        readiness: Option<&Readiness>,
    ) -> Option<(String, PanelSnapshot)> {
        let p = self.engine.panel()?;
        let at = (p.revision, p.bar_end);
        if self.published == Some(at) {
            return None;
        }
        let snap = p.clone();
        self.published = Some(at);
        Some((self.panel_frame("ross.update", readiness), snap))
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

/// Pillar reasons a warmup can actually repair: a symbol that entered the band intraday was not
/// on the day's watch list, so the maintainer never fetched its daily bars, its split snapshot or
/// an announcement check for it.
///
/// Deliberately not here:
/// - `insufficient_baseline` — a genuinely short history. A recent IPO cannot be given 30 prior
///   sessions by asking again, and pretending otherwise would hide a real limit
///   (`docs/stock-context/pillars-v1.md`).
/// - `actions_coverage_expired` — the repair is the maintainer's periodic re-check, not a warmup.
/// - anything about the price or today's volume, which no stored evidence changes.
pub(crate) const WARMUP_REPAIRS: [&str; 5] = [
    "no_previous_close",
    "previous_close_date_mismatch",
    "adjustment_coverage_unknown",
    "adjustment_not_current",
    "no_daily_baseline",
];

/// The first reason on `d` a warmup could repair, when the symbol is in the discovery band now.
pub(crate) fn warmup_reason(d: &Decision) -> Option<&'static str> {
    if d.in_discovery_band != Some(true) {
        return None;
    }
    d.pillars
        .iter()
        .filter(|p| p.state == Tri::Unknown)
        .map(|p| p.reason)
        .find(|r| WARMUP_REPAIRS.contains(r))
}

/// Reasons listed in [`Readiness::blocking`], most symbols first.
pub(crate) const TOP_REASONS: usize = 6;

/// How many symbols each pillar can decide, and what is stopping the rest.
///
/// The panel showing no rows is not the same statement as "nothing qualifies today": it can also
/// mean the evidence a pillar needs has not arrived, or has run out of freshness. Both read as an
/// empty list unless the counts are published, which is what the clients needed to stop saying
/// "No stock at 5/5 yet" over a missing-evidence state (the audit's F10).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Readiness {
    /// Symbols with a decision on the session date.
    pub(crate) symbols: u32,
    /// Symbols at five passes now.
    pub(crate) qualified: u32,
    /// Ross Panel members now (five to enter, four to stay).
    pub(crate) members: u32,
    pub(crate) pillars: Vec<PillarReadiness>,
    /// The commonest reasons a pillar could not decide, most symbols first.
    pub(crate) blocking: Vec<ReasonCount>,
    /// Evidence records the engine holds.
    pub(crate) evidence_records: usize,
    /// What the warmup requester has done (`crate::evidence`).
    pub(crate) warmups: crate::evidence::WarmupStats,
    /// When this summary was taken (UTC ns, wall clock): it is a snapshot, not live.
    pub(crate) observed_at: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct PillarReadiness {
    pub(crate) pillar: &'static str,
    pub(crate) pass: u32,
    pub(crate) fail: u32,
    /// Could not be decided: the evidence is missing, stale or expired.
    pub(crate) unknown: u32,
}

impl Readiness {
    /// One log line: the counts and the reasons, with `no blocking reason` said out loud rather
    /// than left as an empty list a reader could take for "nothing is wrong".
    pub(crate) fn summary_line(&self) -> String {
        let blocking = if self.blocking.is_empty() {
            "no blocking reason".to_string()
        } else {
            self.blocking
                .iter()
                .map(|b| format!("{}:{} x{}", b.pillar, b.reason, b.symbols))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "{} symbols, {} qualified, {} members; {blocking}",
            self.symbols, self.qualified, self.members
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ReasonCount {
    pub(crate) pillar: &'static str,
    pub(crate) reason: &'static str,
    pub(crate) symbols: u32,
}
