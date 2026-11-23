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
//! Policies: `pillars-v5` ([`Policy::current`]: discovery and price on the app's own $0.10–$25 band,
//! 120 s news coverage lag, a float usable for 4 days, shares outstanding below the float maximum
//! passing the float pillar when no usable float is visible), `ross-panel-v1`, and the EXPERIMENTAL `detection-v2` whose
//! micro-pullback (variant MP5) is each row's `setup` cue (`stock_context::cue`, owner decision
//! 2026-10-21: one pullback detector, so the Ross and Detection panels cannot disagree). With
//! `DETECTION_PANEL` off the engine runs that one family for the panel's members only. A cue
//! describes a stage, not an entry signal. Every panel snapshot carries each row's `setup` and the
//! snapshot's `attention`. Both bands are checked against `PRICE_BAND_MIN` / `MAX` at
//! construction.
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
use market_events::conditions::trade_effect;
use market_events::{MarketEvent, Price, Trade, UnixNanos};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};
use stock_context::{
    Adoption, CloseBasis, CorrectedPrint, Coverage, DayHigh, Decision, Engine, Fidelity,
    InstrumentId, LastPrint, OutputTelemetry, PanelPolicy, PanelSnapshot, PillarId, Policy,
    PrevClose, PrintRevision, Qualification, RevisionTarget, RunInfo, Tri, Verdict,
};

/// What one [`ContextBoard::drain_outputs`] took off the engine, and what the engine had to drop
/// before it got there.
///
/// `panels` and `setup_entries` are counts. The setup entries (cue transitions) are also written
/// to the day's transition log by a live market (`crate::oplog`, owner 2026-10-26); the panels
/// are not persisted.
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

/// Cue transitions held between drains at most (a day sees a few thousand; a market that never
/// takes them stops holding more rather than growing).
const SETUP_OUT_MAX: usize = 20_000;

pub(crate) struct ContextBoard {
    engine: Engine,
    /// Symbols whose previous close was offered to the engine, per trading date.
    closes_offered: HashSet<(String, NaiveDate)>,
    /// The panel revision last published (`ross.update` only when it moves).
    published: Option<(u64, i64)>,
    /// The readiness summary last published, by the time it was taken. The panel and the
    /// readiness move independently: with nothing qualifying the panel never changes at all, and
    /// a subscriber that connected before the first summary existed would otherwise never be
    /// told why.
    published_readiness: Option<i64>,
    /// How far this board has taken the market's evidence log (`None`: nothing yet).
    evidence: Option<Cursor>,
    /// Outputs this board has drained since it was built (`ross` viewers do not change it).
    drained: DrainReport,
    /// Shadow measurement capture (`crate::shadow`): a bounded non-blocking handoff, so the
    /// folding and the append happen off this lock. `None` leaves the measurement off entirely.
    shadow: Option<std::sync::Arc<crate::shadow::ShadowSink>>,
    /// The latest barrier this board has decided at: the **run's** clock, which is what the
    /// shadow's session-close rule is settled against. Never the wall clock — a replay's day is
    /// its own, and yesterday's closed sessions must not reopen at this morning's 09:00.
    run_clock: i64,
    /// Transport namespace only; semantic IDs and decisions stay deterministic in the engine.
    detection_epoch: Option<String>,
    /// The members' day lines the last `ross.update` carried (`crate::day_line`): the next
    /// update sends only what changed against them.
    day_lines: crate::day_line::Published,
    /// Ross and Signals push notices decided from the drained journals (`crate::cue_notices`).
    cue: crate::cue_notices::CueNotices,
    /// The drained cue transitions, held for the market to write off the core lock
    /// ([`ContextBoard::take_setup_transitions`], `crate::oplog` "Cue transitions").
    setup_out: Vec<stock_context::DetectionTransition>,
    /// The Runners' flagged symbols and the owner's favorites (`crate::favorites`): together the
    /// engine's watched set, so both kinds of row get micro-pullback levels. Kept apart because
    /// only the Runners list is the pullback notices' context.
    runners_watched: Vec<String>,
    favorites: Vec<String>,
}

/// The `detectors` settings entry that keeps the Ross Panel's news pillar required (owner,
/// 2026-11-22; clients: `packages/shared/src/detectors.ts` `ROSS_REQUIRE_NEWS`). Not an alert:
/// a membership rule, kept in the same synced document as the detector switches so it is
/// written, validated and broadcast the same way (an entry patch, no restart).
pub const ROSS_REQUIRE_NEWS: &str = "ross-require-news";

/// Whether the `detectors` document keeps a news catalyst required: the entry's `on` (or a bare
/// boolean); absent or any other shape is the default, **on** (today's rule, ross-panel-v1).
pub fn news_required(v: Option<&Value>) -> bool {
    match v.and_then(|v| v.get(ROSS_REQUIRE_NEWS)) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Object(e)) => e.get("on").and_then(Value::as_bool).unwrap_or(true),
        _ => true,
    }
}

/// Follows the switch into the markets' context boards (no restart), like `crate::hod::follow`:
/// at start, on every `settings.changed` for the `detectors` namespace, and after a lagged
/// receiver (it may have missed one).
pub fn follow_news_switch(
    settings: crate::settings::Settings,
    markets: Vec<crate::market::Market>,
) {
    const NS: &str = crate::alerts::detectors::NS;
    tokio::spawn(async move {
        let mut rx = settings.subscribe();
        let apply = |v: Option<Value>| {
            let required = news_required(v.as_ref());
            for m in &markets {
                m.set_ross_news_required(required);
            }
        };
        apply(settings.get(NS).await.map(|(_, v)| v));
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    let ours = serde_json::from_str::<Value>(&frame)
                        .is_ok_and(|v| v["type"] == "settings.changed" && v["ns"] == NS);
                    if ours {
                        apply(settings.get(NS).await.map(|(_, v)| v));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    apply(settings.get(NS).await.map(|(_, v)| v));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// A T1 "news pending" halt as news evidence (pillars-v6 source `halt_t1`, owner 2026-11-22): the
/// exchange halted the stock because news is coming, which is evidence of a catalyst before any
/// headline exists. Built from the chart engine's halt update through the news seam
/// (`stock_context::SourceNews`), so the live board and a restore's replay of the same tape
/// (`crate::history`) derive the same item: id `<symbol>:<halt ns>`, published and observed at
/// the halt's exchange time (it reaches us with the halt message), version 1 at the halt and 2
/// once a resume is seen (observed then, carrying the resume time). `None` for any other reason
/// code.
///
/// Not carried, because the stream does not give them per halt: the halt price (the chart has the
/// last print, the halt message does not) and the later codes of the same halt (T2 "news
/// released", T3): the chart engine keeps one reason per halt span.
pub fn t1_news_item(h: &chart_engine::wire::HaltUpdate) -> Option<stock_context::NewsItem> {
    if !h.halt.reason.trim().eq_ignore_ascii_case("T1") {
        return None;
    }
    let ns = |n: i64, ms: i64| if n != 0 { n } else { ms * 1_000_000 };
    let start = ns(h.halt.start_ns, h.halt.start);
    let end = h.halt.end.map(|e| ns(h.halt.end_ns.unwrap_or(0), e));
    Some(stock_context::NewsItem::from_source(
        &stock_context::SourceNews {
            provider: stock_context::NEWS_PROVIDER_HALT_T1.into(),
            source: "T1".into(),
            id: format!("{}:{start}", h.symbol),
            symbol: h.symbol.clone(),
            published_at: start,
            observed_at: end.unwrap_or(start),
            version: if end.is_some() { 2 } else { 1 },
            headline: Some("News pending (T1 halt)".into()),
            url: None,
            kind: stock_context::SourceKind::Pending,
            event_type: None,
            rules: "halt-t1-v1".into(),
            halt: Some(stock_context::HaltFacts {
                halted_at: start,
                reason: h.halt.reason.trim().to_string(),
                resumed_at: end,
            }),
        },
    ))
}

/// The context engine exactly as the live board builds it: the pillar policy, the Ross Panel's
/// policy and the micro-pullback (setup) detector. One constructor, so the ticker page's replay of
/// a past day (`crate::insights`) runs the detector the live market ran, never a lookalike.
pub(crate) fn production_engine(policy: Policy, panel: PanelPolicy, run: RunInfo) -> Engine {
    Engine::new(policy, run)
        .with_panel_policy(panel)
        .with_setup_policy(stock_context::DetectionPolicy::current())
}

impl ContextBoard {
    /// The latest public float held for `symbol`, in shares, whatever its age: the ticker search's
    /// Float chips sort stocks into bands, where a float a few days old is still the right band
    /// (the pillar's own freshness rule stays in `stock_context::evaluate`).
    pub(crate) fn float_shares(&self, symbol: &str) -> Option<u64> {
        self.engine
            .evidence()
            .float(&InstrumentId::symbol(symbol), i64::MAX)
            .map(|f| f.shares)
    }

    /// `None` when the pillar policy's price band or the panel policy's platform band is not the
    /// server's price band: the price pillar and membership would then judge by a band the rest
    /// of the app (the quote board, the collector's universe) does not use.
    pub(crate) fn new(
        live: bool,
        day: Option<NaiveDate>,
        band_min: Price,
        band_max: Price,
    ) -> Option<ContextBoard> {
        Self::new_labeled(live, day, band_min, band_max, false)
    }

    pub(crate) fn new_labeled(
        live: bool,
        day: Option<NaiveDate>,
        band_min: Price,
        band_max: Price,
        fixture: bool,
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
            run_id: if fixture {
                format!("fixture:{}", day.map(|d| d.to_string()).unwrap_or_default())
            } else {
                match (live, day) {
                    (true, _) => "live".to_string(),
                    (false, Some(d)) => format!("replay:{d}"),
                    (false, None) => "replay".to_string(),
                }
            },
            source: if fixture {
                "fixture"
            } else if live {
                "live"
            } else {
                "replay"
            }
            .to_string(),
            // A replayed day may be a live recording or a corrected import; without telling them
            // apart here, claim the weaker one (no latency claims from a replay).
            fidelity: if fixture {
                Fidelity::SyntheticFixture
            } else if live {
                Fidelity::RecordedLive
            } else {
                Fidelity::ReconstructedHistory
            },
            // No coverage records are consulted yet (docs: recording provenance).
            numerator_coverage: if fixture {
                Coverage::Complete
            } else {
                Coverage::Unverified
            },
        };
        Some(ContextBoard {
            engine: production_engine(policy, panel, run),
            closes_offered: HashSet::new(),
            published: None,
            published_readiness: None,
            evidence: None,
            drained: DrainReport::default(),
            shadow: None,
            run_clock: 0,
            detection_epoch: None,
            day_lines: crate::day_line::Published::default(),
            cue: crate::cue_notices::CueNotices::default(),
            setup_out: Vec::new(),
            runners_watched: Vec::new(),
            favorites: Vec::new(),
        })
    }

    /// What the board and its engine hold (entry counts, for the `memory` log line).
    pub(crate) fn cost(&self) -> serde_json::Value {
        serde_json::json!({
            "engine": self.engine.cost(),
            "closes_offered": self.closes_offered.len(),
            "setup_out": self.setup_out.len(),
            "runners_watched": self.runners_watched.len(),
            "favorites": self.favorites.len(),
        })
    }

    /// Selects the Ross Panel policy the owner's switch asks for (`ross-panel-v1` while a news
    /// catalyst is required, `ross-panel-v2` while it is optional). The engine decides every
    /// symbol again at its next barrier under it ([`stock_context::Engine::set_panel_policy`]).
    /// Returns whether the policy changed.
    pub(crate) fn set_news_required(&mut self, required: bool) -> bool {
        self.engine
            .set_panel_policy(PanelPolicy::for_news_required(required))
    }

    /// Whether this board's panel admits without a passing news pillar.
    pub(crate) fn news_optional(&self) -> bool {
        self.engine.panel_policy().news_optional()
    }

    /// The same board deciding under another pillar policy (the completeness audit compares
    /// pillars-v4 with v5 over one recorded day, `crate::ross_audit`).
    #[cfg(test)]
    pub(crate) fn with_pillar_policy(mut self, policy: Policy) -> Self {
        let run = self.engine.run_info().clone();
        let panel = self.engine.panel_policy().clone();
        self.engine = production_engine(policy, panel, run);
        self
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
    /// The engine owns no consumer: it appends panel snapshots and setup-log entries (the
    /// Ross-only cue transitions; with the Detection Panel on those travel in its journal) and its
    /// caller takes them. Draining used to happen only inside [`ContextBoard::take_update`],
    /// which the market skips when nobody is subscribed to `ross`, and the setup log had no
    /// caller at all — so an unattended run grew both for ever. This is called every tick
    /// regardless of viewers. In-memory only (it runs under the core lock): no disk, no provider,
    /// no serialization.
    ///
    /// The drained panels are dropped here. The cue transitions are kept for the market to append
    /// to the day's transition log off the lock ([`ContextBoard::take_setup_transitions`],
    /// owner 2026-10-26); bounded by [`SETUP_OUT_MAX`] so a market that never takes them (a
    /// replay, a test) cannot grow it.
    /// `observe` sees each drained snapshot before it is discarded, so the baseline
    /// observation/latency capture (`crate::observe`) can record its segment times. It must stay
    /// cheap: this runs under the core lock.
    pub(crate) fn drain_outputs(&mut self, observe: &mut dyn FnMut(&PanelSnapshot)) -> DrainReport {
        let panels = self.engine.take_panels();
        for p in &panels {
            observe(p);
        }
        // The Ross-only journal (Detection Panel off) carries the members' crossings; the latest
        // panel their "At trigger" (crate::cue_notices). Read before the entries are dropped.
        let setup = self.engine.take_setup_log();
        self.cue.transitions(&self.engine, &setup);
        if let Some(p) = self.engine.panel() {
            let signals = self.engine.detection_policy().is_some();
            self.cue.ross_panel(p, signals);
        }
        let r = DrainReport {
            panels: panels.len(),
            setup_entries: setup.len(),
        };
        if self.setup_out.len() + setup.len() <= SETUP_OUT_MAX {
            self.setup_out.extend(setup);
        }
        self.drained.panels += r.panels;
        self.drained.setup_entries += r.setup_entries;
        r
    }

    /// The cue transitions drained since the last call (a move, never a copy: it runs under the
    /// core lock).
    pub(crate) fn take_setup_transitions(&mut self) -> Vec<stock_context::DetectionTransition> {
        std::mem::take(&mut self.setup_out)
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
        let news_optional = self.news_optional();
        let mut r = Readiness {
            news_optional,
            evidence_records: self.engine.evidence_len(),
            revisions: self.engine.revision_telemetry(),
            restores: self.engine.restore_telemetry(),
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
            // A reason blocks only a symbol that nothing has ruled out yet. One that already
            // FAILS a pillar cannot reach five whatever arrives, so its UNKNOWNs stop nothing:
            // counted, they buried the real blockers under the whole market. On 2026-10-30 the
            // line led with `float:no_float_evidence x7795`, read as "the float feed is broken",
            // when ~7,400 of those were stocks outside the band (price FAIL) and ETFs, warrants
            // and units FMP publishes no float for; every in-band +10% mover had its float.
            let contender = !d.pillars.iter().any(|p| p.state == Tri::Fail);
            let mut undecided = false;
            for p in &d.pillars {
                let Some(slot) = pillars.iter_mut().find(|x| x.pillar == p.pillar.as_str()) else {
                    continue;
                };
                match p.state {
                    Tri::Pass => slot.pass += 1,
                    Tri::Fail => slot.fail += 1,
                    Tri::Unknown => {
                        slot.unknown += 1;
                        if contender {
                            undecided = true;
                            *reasons.entry((p.pillar.as_str(), p.reason)).or_default() += 1;
                        }
                    }
                }
            }
            if undecided {
                r.contenders += 1;
            }
            if let Some(n) = near_row(d, news_optional) {
                r.near.push(n);
            }
        }
        // Strongest gain first: the near rows are the ones closest to being missed entirely.
        r.near.sort_by(|a, b| {
            b.gain_bp
                .cmp(&a.gain_bp)
                .then_with(|| a.symbol.cmp(&b.symbol))
        });
        r.near_total = r.near.len() as u32;
        r.near.truncate(NEAR_MAX);
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

    /// Symbols with a decision on the session date, skipping the ones `done` already holds, at
    /// most `max`.
    ///
    /// Skipping before the budget is taken, not after: with the skip the other way round a first
    /// batch that never resolves would block every symbol behind it, which is the fairness defect
    /// the warmup scan had.
    pub(crate) fn decided_symbols(&self, max: usize, done: &BTreeSet<String>) -> Vec<String> {
        let mut out = Vec::new();
        for d in self.engine.current() {
            if out.len() >= max {
                break;
            }
            if !done.contains(&d.symbol) {
                out.push(d.symbol.clone());
            }
        }
        out
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

    /// A second engine for restoring one symbol's recorded day on its own, under **this** board's
    /// policies and run identity.
    ///
    /// The restore cannot feed the recorded prints into this engine: its clock has already
    /// advanced with the live tail, so every one of them would be late and none of the day's
    /// barriers would be decided — the numerator would still start at zero after every deploy,
    /// which is what the audit found (F1). It replays them on this engine instead and the result
    /// is adopted ([`ContextBoard::adopt_symbol`]).
    pub(crate) fn restore_engine(&self) -> Engine {
        let engine = Engine::new(self.engine.policy().clone(), self.engine.run_info().clone())
            .with_panel_policy(self.engine.panel_policy().clone())
            .with_setup_policy(self.engine.setup_policy().clone());
        match self.engine.detection_policy() {
            Some(policy) => engine.with_detection_policy(policy.clone()),
            None => engine,
        }
    }

    /// Takes one symbol's restored **facts** into this board, with what is known about how
    /// completely its day was collected and how far back the restored prefix reaches.
    ///
    /// `coverage` is the restore's own answer, not an assumption: a recorder drop means the
    /// recording this process restored from is short while the live numerator was fine, and that
    /// is a different statement from a stream gap. Where nothing is known the run-wide figure
    /// stands, which for production is `Unverified` — never `Complete`, because a restart cannot
    /// prove it saw the whole day.
    ///
    /// Membership, episodes and the latest decision are not adopted
    /// ([`stock_context::Engine::adopt_facts`]): this board keeps whatever it has already
    /// published and decides the symbol again, on the day's full facts, at the next barrier.
    pub(crate) fn adopt_facts(
        &mut self,
        from: &mut Engine,
        symbol: &str,
        coverage: Option<(Coverage, &'static str)>,
        restored_through: Option<i64>,
        at: UnixNanos,
    ) -> Adoption {
        let outcome = self.engine.adopt_facts(from, symbol);
        if matches!(outcome, Adoption::Adopted { .. }) {
            if coverage.is_some() {
                self.engine.set_coverage(symbol, coverage);
            }
            self.engine.set_restored_through(symbol, restored_through);
        } else {
            // A failed restore ends the temporary guard too. It used to leave the symbol
            // history-pending for the life of the process ("terminal, even across a date roll"),
            // and pending makes RVOL UNKNOWN: a symbol whose restore found nothing, or was
            // refused, could never be admitted to the Ross Panel until a restart — and the
            // commonest refusal was a thin premarket print after a 04:00–09:30 deploy, exactly
            // the gappers. What the live tail holds is a lower bound on the day either way, which
            // is what the pillars are built to reason about; the coverage says how much less.
            self.engine.set_history_pending(symbol, false);
            match outcome {
                Adoption::Refused(why) => {
                    let reason = match why {
                        "run_identity" => "restore_refused_run_identity",
                        "pillar_policy" => "restore_refused_pillar_policy",
                        "panel_policy" => "restore_refused_panel_policy",
                        "setup_policy" => "restore_refused_setup_policy",
                        "detection_policy" => "restore_refused_detection_policy",
                        "trading_date" => "restore_refused_trading_date",
                        _ => "restore_refused_unknown",
                    };
                    // The recording had prints this board could not take: the tail is short of
                    // the day by them. Dated, so it ends with the date (`Engine::evaluate`).
                    self.detection_status(Some(symbol), at, false, "history_restore_refused");
                    self.engine
                        .set_coverage(symbol, Some((Coverage::Partial, reason)));
                }
                _ => {
                    // Nothing recorded for the symbol on this date: the live tail is all of it
                    // that anything saw, so there is nothing to protect. A read that came back
                    // short says so through `coverage`, exactly as an adopted one would.
                    self.detection_status(Some(symbol), at, false, "history_restore_empty");
                    if coverage.is_some() {
                        self.engine.set_coverage(symbol, coverage);
                    }
                }
            }
        }
        outcome
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
        for r in b.outstanding {
            self.engine.add_outstanding(r);
        }
        for r in b.intraday {
            // Diagnostic only: the same-time and recent-window measures beside the pillar, never
            // an input to it (`stock_context::evaluate::rvol`).
            self.engine.add_intraday(r);
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
                // Raw on purpose: this record says `CloseBasis::Raw` with no actions check, and the
                // engine grades the share basis from its own corporate-action evidence
                // (crate::evidence). The split-adjusted lookup is for the boards, not for this.
                if let Some((date, close)) = closes.before_raw(&t.symbol, s.date) {
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
        let decided = self.engine.apply(
            t,
            Verdict {
                ohlc: counted.ohlc,
                volume: counted.volume,
            },
            admit,
        );
        self.capture(&decided);
    }

    pub(crate) fn advance(&mut self, now: UnixNanos) {
        let decided = self.engine.advance_to(now);
        self.capture(&decided);
        // The clock marker goes **after** the decisions it produced, because that is what it
        // means: processed through this point. Queued before them it would say the run had
        // settled a barrier it had not yet decided.
        //
        // It is also emitted whether or not anything was decided: a quiet market produces no
        // decisions, and without this nothing would ever advance the run's clock past a
        // session's close.
        self.clock(now.0);
    }

    /// Tells the shadow measurement where the run's clock is, and whether it jumped.
    ///
    /// A jump means the interval it skipped was not monitored, and nothing inside an unmonitored
    /// interval may be read as absent (`crate::shadow`).
    fn clock(&mut self, now: i64) {
        let Some(sink) = self.shadow.as_ref() else {
            self.run_clock = self.run_clock.max(now);
            return;
        };
        let run = &self.engine.run_info().clone();
        let life = |kind: crate::shadow::LifecycleKind, at: i64| {
            sink.capture_lifecycle(crate::shadow::Lifecycle {
                run_id: run.run_id.clone(),
                source: run.source.clone(),
                fidelity: format!("{:?}", run.fidelity),
                kind,
                at,
            });
        };
        if self.run_clock == 0 {
            // The first clock this board sees is when its monitoring began. Not a symbol's first
            // decision, which is its first print and says nothing about when the run started.
            life(crate::shadow::LifecycleKind::Start, now);
        } else if now - self.run_clock > crate::shadow::GAP_SECS * 1_000_000_000 {
            life(
                crate::shadow::LifecycleKind::Gap {
                    from: self.run_clock,
                },
                now,
            );
        } else {
            life(crate::shadow::LifecycleKind::Clock, now);
        }
        self.run_clock = self.run_clock.max(now);
    }

    /// Records that monitoring ended (a clean shutdown), at the point the run had processed
    /// through — never at the wall clock, which for a replay is a different day entirely.
    pub(crate) fn stop_monitoring(&mut self) {
        let at = self.run_clock;
        if let Some(sink) = self.shadow.as_ref() {
            let run = self.engine.run_info().clone();
            sink.capture_lifecycle(crate::shadow::Lifecycle {
                run_id: run.run_id,
                source: run.source,
                fidelity: format!("{:?}", run.fidelity),
                kind: crate::shadow::LifecycleKind::Stop,
                at,
            });
        }
    }

    /// Attaches the shadow measurement's capture. Without one the measurement is off.
    pub(crate) fn with_shadow(mut self, sink: std::sync::Arc<crate::shadow::ShadowSink>) -> Self {
        self.shadow = Some(sink);
        self
    }

    /// Hands decisions to the shadow measurement's bounded queue (`crate::shadow`).
    ///
    /// Here rather than on a drain, because this is where the decisions are: the drained outputs
    /// are panel snapshots, which carry rows and not the per-symbol pillar states the frozen
    /// definition reads. What happens here is a reduction to a small event and a `try_send` —
    /// the folding, the map walk and the append all happen off this lock
    /// ([`crate::shadow::ShadowSink::absorb`]). It changes nothing about the decision itself: a
    /// decision is immutable once made and the shadow only ever reads.
    fn capture(&mut self, decided: &[Decision]) {
        for d in decided {
            self.run_clock = self.run_clock.max(d.cutoff);
        }
        let Some(sink) = self.shadow.as_ref() else {
            return;
        };
        for d in decided {
            sink.capture(crate::shadow::ShadowEvent::of(d));
        }
    }

    /// Applies a correction or cancel, whether or not a chart is loaded for the symbol.
    ///
    /// Nothing used to route a revision here at all: the server gave it to the quote board only,
    /// so an inflated RVOL numerator survived a cancel for the rest of the day and the "missing
    /// prints can only lower RVOL" argument did not hold (the audit's F3). Routing it only on the
    /// loaded path would have left the same hole for every symbol whose history is still loading,
    /// which is exactly the window a restart spends in.
    ///
    /// So the scanner resolves its own: the raw reference the provider published goes to the
    /// context engine, which matches it against the prints **it** admitted
    /// (`stock_context::revision`). `resolved` is the chart engine's answer when there is one,
    /// and contributes only the two things it alone can recompute — the date's last price and its
    /// high. Their absence is counted, never assumed.
    pub(crate) fn apply_revision(
        &mut self,
        e: &MarketEvent,
        resolved: Option<&chart_engine::Revision>,
        admit: UnixNanos,
    ) {
        apply_revision_to(&mut self.engine, e, resolved, admit);
    }

    /// What the engine did with the revisions and duplicates it was given (tests; production
    /// reads it through [`Readiness::revisions`]).
    #[cfg(test)]
    pub(crate) fn revision_telemetry(&self) -> stock_context::RevisionTelemetry {
        self.engine.revision_telemetry()
    }

    pub(crate) fn set_halted(&mut self, symbol: &str, halted: bool) {
        self.engine.set_halted(symbol, halted);
    }

    /// A halt update's news evidence (a T1 halt: [`t1_news_item`]), straight into the engine.
    pub(crate) fn apply_halt_news(&mut self, h: &chart_engine::wire::HaltUpdate) {
        if let Some(n) = t1_news_item(h) {
            self.engine.add_news(n);
        }
    }

    /// The Runners' flagged symbols (`crate::rocket`): the engine also runs the micro-pullback
    /// (MP5) for them, as for Ross members, so their rows get the same levels.
    pub(crate) fn set_watched(&mut self, flagged: Vec<(String, i64)>) {
        // The pullback notices' context too (crate::cue_notices, "Pullback"), with the time each
        // stock was first flagged (its first row today, restored with the rows after a restart).
        self.cue.set_watched(&flagged);
        self.runners_watched = flagged.into_iter().map(|(s, _)| s).collect();
        self.sync_watched();
    }

    /// The run id this board's episodes are numbered under (`live` on every boot of the live
    /// market): what [`crate::notice_memory::read`] looks for in the logs.
    pub(crate) fn run_id(&self) -> &str {
        &self.engine.run_info().run_id
    }

    /// What a previous process of this run announced and numbered today
    /// ([`crate::notice_memory`]): the pullback notices' memory, and the episode numbers the
    /// engine must continue past so a restart never reissues an id the delivery ledger has sent.
    pub(crate) fn restore_notice_memory(&mut self, m: &crate::notice_memory::NoticeMemory) {
        self.cue.restore(m);
        if let Some(date) = m.date {
            self.engine
                .set_episode_floors(date, m.floors.iter().cloned());
        }
    }

    /// The owner's favorites (`crate::favorites`): watched by the engine like Runners rows, so a
    /// favorite's row gets its micro-pullback levels; never the pullback notices' context.
    pub(crate) fn set_favorites(&mut self, symbols: &[String]) {
        self.favorites = symbols.to_vec();
        self.sync_watched();
    }

    pub(crate) fn favorites(&self) -> &[String] {
        &self.favorites
    }

    fn sync_watched(&mut self) {
        let mut all = self.runners_watched.clone();
        for s in &self.favorites {
            if !all.contains(s) {
                all.push(s.clone());
            }
        }
        self.engine.set_watched(all);
    }

    /// A symbol's latest five-pillar decision as the Favorites row's context
    /// (`crate::favorites::Pillars`).
    pub(crate) fn pillars(&self, symbol: &str) -> Option<crate::favorites::Pillars> {
        let d = self.decision(symbol)?;
        let state = |id: stock_context::PillarId| {
            d.pillars
                .iter()
                .find(|p| p.pillar == id)
                .map(|p| (p.state, p.value))
        };
        Some(crate::favorites::Pillars {
            passes: d
                .pillars
                .iter()
                .filter(|p| p.state == stock_context::Tri::Pass)
                .count(),
            of: d.pillars.len(),
            news: state(stock_context::PillarId::News).map(|(s, _)| s),
            news_kind: self.engine.news_kind(symbol).map(str::to_owned),
            rvol_milli: state(stock_context::PillarId::Rvol).and_then(|(_, v)| v),
            member: d.membership.active().is_some(),
        })
    }

    /// A Runners row's micro-pullback levels at `tau` (`rocket.*` row `levels`): the cue's state,
    /// trigger and invalidation (micros), the trigger meter and the distances, exactly as a Ross
    /// row reads them (`stock_context::cue`). `None` before the symbol has a level.
    pub(crate) fn runner_levels(&self, symbol: &str, tau: i64) -> Option<Value> {
        let c = self.engine.watched_cue(symbol, tau)?;
        if c.trigger.is_none() && c.invalidation.is_none() {
            return None;
        }
        Some(json!({
            "state": c.state,
            "trigger": c.trigger.as_ref().map(|l| l.price),
            "invalidation": c.invalidation.as_ref().map(|l| l.price),
            "meter_milli": c.meter_milli,
            "distance_to_trigger_bp": c.distance_to_trigger_bp,
            "to_invalidation_bp": c.to_invalidation_bp,
            "near_trigger": c.near_trigger,
            "episode_id": c.episode_id,
        }))
    }

    pub(crate) fn set_history_pending(&mut self, symbol: &str, pending: bool) {
        self.engine.set_history_pending(symbol, pending);
    }

    pub(crate) fn with_detection(mut self) -> Self {
        self.engine = self
            .engine
            .with_detection_policy(stock_context::DetectionPolicy::current());
        self
    }

    pub(crate) fn detection_policy_tag(&self) -> Option<String> {
        self.engine.detection_policy().map(|p| p.tag())
    }

    pub(crate) fn set_detection_epoch(&mut self, epoch: String) {
        self.detection_epoch = Some(epoch);
    }

    pub(crate) fn add_detection_quiet_reference(
        &mut self,
        reference: stock_context::QuietReference,
    ) {
        self.engine.add_quiet_reference(reference);
    }

    pub(crate) fn detection_status(
        &mut self,
        symbol: Option<&str>,
        at: UnixNanos,
        up: bool,
        reason: &'static str,
    ) {
        // Not only with the Detection Panel on: the Ross rows' cues are the same detector's
        // micro-pullback episodes (stock_context::cue), and a gap, halt or provider error must
        // end a member's episode exactly as it ends the panel's, or the two would disagree.
        let decided = if up {
            self.engine.resume_detection(symbol, at)
        } else {
            self.engine.interrupt_detection(symbol, at, reason)
        };
        self.capture(&decided);
    }

    pub(crate) fn take_detection(&mut self, force: bool) -> Option<crate::detection::CoreCapture> {
        let epoch = self.detection_epoch.as_ref()?;
        let transitions = self.engine.take_detection_transitions();
        // The crossings among them, the moment they are drained (crate::cue_notices).
        self.cue.transitions(&self.engine, &transitions);
        if !force && transitions.is_empty() {
            return None;
        }
        let policy = self.engine.detection_policy()?;
        let policy_hash = policy.hash.clone();
        let research_band = (
            policy.body.universe.research_band_min_micros,
            policy.body.universe.research_band_max_micros,
        );
        // Significant trade-driven transitions drain immediately, but a snapshot walks all
        // symbols/episodes. That walk belongs only to the bounded display cadence.
        let snapshot = force.then(|| self.engine.detection_snapshot()).flatten();
        if let Some(s) = &snapshot {
            self.cue.signals_snapshot(&self.engine, s);
        }
        let symbols: BTreeSet<&str> = snapshot
            .as_ref()
            .into_iter()
            .flat_map(|s| s.active.iter().chain(s.recent.iter()))
            .map(|e| e.symbol.as_str())
            .collect();
        // Clone typed assessments under core; serialization happens after releasing it. Missing
        // symbols have no context entry and remain UNKNOWN in the display join.
        let context = if force {
            self.engine
                .current()
                .filter(|d| symbols.contains(d.symbol.as_str()))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        Some(crate::detection::CoreCapture {
            epoch: epoch.clone(),
            snapshot,
            transitions,
            telemetry: self.engine.detection_telemetry(),
            context,
            policy_hash,
            research_band,
        })
    }

    /// The whole panel as a `ross.snapshot` / `ross.update` frame (empty `rows` before any),
    /// with the cached readiness summary when there is one: an empty panel then says *why*
    /// instead of reading as "nothing qualifies".
    /// Without day lines: the tests' frame (the market always passes its candles).
    #[cfg(test)]
    pub(crate) fn panel_frame(&self, kind: &str, readiness: Option<&Readiness>) -> String {
        self.panel_frame_with(kind, readiness, None)
    }

    /// [`ContextBoard::panel_frame`] with each member's day line (`crate::day_line`) when `lines`
    /// is given: whole lines, as a `ross.snapshot` sends them.
    pub(crate) fn panel_frame_with(
        &self,
        kind: &str,
        readiness: Option<&Readiness>,
        lines: Option<crate::day_line::Source<'_>>,
    ) -> String {
        Self::frame(&self.engine, kind, readiness, lines, None)
    }

    fn frame(
        engine: &Engine,
        kind: &str,
        readiness: Option<&Readiness>,
        lines: Option<crate::day_line::Source<'_>>,
        published: Option<&mut crate::day_line::Published>,
    ) -> String {
        let mut panel = serde_json::to_value(engine.panel()).unwrap_or(Value::Null);
        if let Some(src) = lines {
            crate::day_line::decorate(&mut panel, src, published);
        }
        json!({
            "type": kind,
            "enabled": true,
            "panelPolicy": engine.panel_policy().tag(),
            "pillarPolicy": engine.policy().tag(),
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
    /// Without day lines (tests; the market uses [`ContextBoard::take_update_with`]).
    #[cfg(test)]
    pub(crate) fn take_update(
        &mut self,
        readiness: Option<&Readiness>,
    ) -> Option<(String, Option<PanelSnapshot>)> {
        self.take_update_with(readiness, None)
    }

    /// [`ContextBoard::take_update`] with the members' day lines as deltas against the previous
    /// update's (`crate::day_line`) when `lines` is given.
    pub(crate) fn take_update_with(
        &mut self,
        readiness: Option<&Readiness>,
        lines: Option<crate::day_line::Source<'_>>,
    ) -> Option<(String, Option<PanelSnapshot>)> {
        let at = self.engine.panel().map(|p| (p.revision, p.bar_end));
        let taken = readiness.map(|r| r.observed_at);
        let panel_new = at.is_some() && self.published != at;
        let readiness_new = taken.is_some() && self.published_readiness != taken;
        if !panel_new && !readiness_new {
            return None;
        }
        // The snapshot is returned **only** when the panel itself is new. A frame sent because
        // the readiness summary moved carries the same panel as before: it is not a new
        // detection and must not become a latency sample (`crate::observe`).
        let snap = panel_new.then(|| self.engine.panel().cloned()).flatten();
        if panel_new {
            self.published = at;
        }
        if readiness_new {
            self.published_readiness = taken;
        }
        let frame = Self::frame(
            &self.engine,
            "ross.update",
            readiness,
            lines,
            Some(&mut self.day_lines),
        );
        Some((frame, snap))
    }

    /// The Ross and Signals notices decided since the last call (`crate::cue_notices`).
    pub(crate) fn take_cue_notices(&mut self) -> Vec<crate::rocket::Notice> {
        self.cue.take()
    }

    pub(crate) fn decision(&self, symbol: &str) -> Option<&Decision> {
        self.engine.current().find(|d| d.symbol == symbol)
    }
}

/// Applies a raw correction or cancel to a bare context engine (the restart restore's own
/// replay, `crate::history`), by the same rule [`ContextBoard::apply_revision`] uses.
pub(crate) fn apply_revision_to(
    engine: &mut Engine,
    e: &MarketEvent,
    resolved: Option<&chart_engine::Revision>,
    admit: UnixNanos,
) {
    if let Some(rev) = revision_of(e, resolved) {
        let _ = engine.apply_revision(&rev, admit);
    }
}

/// The provider's revision as the engine's contract names it, with the chart engine's
/// recomputed last price and high when there is one.
fn revision_of(
    e: &MarketEvent,
    resolved: Option<&chart_engine::Revision>,
) -> Option<PrintRevision> {
    let (symbol, target, corrected) = match e {
        MarketEvent::Correction(c) => (
            c.symbol.clone(),
            RevisionTarget {
                id: c.original_trade_id(),
                price: c.original_price.micros(),
                size: c.original_size,
            },
            Some(CorrectedPrint {
                price: c.corrected_price.micros(),
                size: c.corrected_size,
                verdict: {
                    let eff = trade_effect(&c.corrected_conditions, c.tape);
                    Verdict {
                        ohlc: eff.updates_ohlc,
                        volume: eff.updates_volume,
                    }
                },
            }),
        ),
        MarketEvent::Cancel(c) => (
            c.symbol.clone(),
            RevisionTarget {
                id: c.target_trade_id(),
                price: c.price.micros(),
                size: c.size,
            },
            None,
        ),
        _ => return None,
    };
    Some(PrintRevision {
        symbol,
        target,
        corrected,
        // Named, so the price and the execution time cannot be swapped: the engine reads
        // `(exec, price)` and the chart engine gives `(price, ts)`.
        last: resolved.and_then(|r| r.last).map(|(p, ts)| LastPrint {
            exec: ts.nanos(),
            price: p.micros(),
        }),
        day_high: resolved.and_then(|r| r.day_high).map(|h| DayHigh {
            price: h.price.micros(),
            complete: h.complete,
        }),
    })
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
    /// Ross Panel members now (five to enter, four to stay; four of the non-news pillars to
    /// enter and three to stay while news is optional).
    pub(crate) members: u32,
    /// The owner switched "require a news catalyst" off (`ross-panel-v2`): said in the summary
    /// so a reader knows which membership rule the counts are under.
    pub(crate) news_optional: bool,
    pub(crate) pillars: Vec<PillarReadiness>,
    /// Symbols no pillar FAILS that still have a pillar UNKNOWN: the ones evidence could still
    /// put on the panel, and the only ones [`Readiness::blocking`] counts.
    pub(crate) contenders: u32,
    /// The commonest reasons a pillar could not decide, most symbols first, over the
    /// [`Readiness::contenders`] only (a symbol another pillar already fails is not blocked by
    /// missing evidence).
    pub(crate) blocking: Vec<ReasonCount>,
    /// Near-qualifying symbols (owner, 2026-11-22): price, gain and RVOL all PASS, and every
    /// other pillar either passes or is UNKNOWN, at least one UNKNOWN (float or news), and not a
    /// panel member. These are the stocks the Ross rules would show if the missing evidence
    /// arrived; a FAIL anywhere keeps a symbol out. Strongest gain first, at most [`NEAR_MAX`].
    /// Descriptive only: never membership, never a notice.
    pub(crate) near: Vec<NearRow>,
    /// How many symbols were near-qualifying before the cut to [`NEAR_MAX`].
    pub(crate) near_total: u32,
    /// Evidence records the engine holds.
    pub(crate) evidence_records: usize,
    /// What the warmup requester has done (`crate::evidence`).
    pub(crate) warmups: crate::evidence::WarmupStats,
    /// What the engine did with the corrections, cancels and duplicates it was given.
    /// `complete` false means something could not be repaired exactly, and the counts say what.
    pub(crate) revisions: stock_context::RevisionTelemetry,
    /// Symbols whose recorded day was replayed and adopted after a restart
    /// (`stock_context::RestoreTelemetry`).
    pub(crate) restores: stock_context::RestoreTelemetry,
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
        let rev = self.revisions;
        format!(
            "{} symbols, {} qualified, {} members{}, {} restored; {} awaiting evidence: {blocking}; revisions {} applied, {} duplicates dropped{}",
            self.symbols,
            self.qualified,
            self.members,
            if self.news_optional {
                " (news optional)"
            } else {
                ""
            },
            self.restores.adopted,
            self.contenders,
            rev.applied,
            rev.duplicates_dropped,
            if rev.complete() {
                String::new()
            } else {
                format!(
                    " (UNREPAIRED: {} unknown target, {} day high unproven, {} too late)",
                    rev.unknown_target, rev.day_high_unproven, rev.too_late
                )
            }
        )
    }
}

/// Near-qualifying rows the frame carries at most ([`Readiness::near`]).
pub(crate) const NEAR_MAX: usize = 50;

/// One near-qualifying symbol ([`Readiness::near`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct NearRow {
    pub(crate) symbol: String,
    /// Last qualifying price, micros.
    pub(crate) price: Option<i64>,
    pub(crate) gain_bp: Option<i64>,
    /// Daily-denominator RVOL, thousandths.
    pub(crate) rvol_milli: Option<i64>,
    /// The pillars that could not be decided, with the reason (`float` / `news`).
    pub(crate) unknown: Vec<NearPillar>,
    /// The float figure when one is visible (even stale), for the row.
    pub(crate) float_shares: Option<i64>,
    /// The decision's cutoff (UTC ns).
    pub(crate) cutoff: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct NearPillar {
    pub(crate) pillar: &'static str,
    pub(crate) reason: &'static str,
}

/// A decision's near-qualifying row ([`Readiness::near`]), or `None`.
///
/// With news optional (`ross-panel-v2`) the news pillar no longer decides membership, so it
/// neither makes a symbol near nor keeps one out: a symbol blocked only by news is a member then
/// and leaves this list, and a float-only near miss stays whatever its news says.
pub(crate) fn near_row(d: &Decision, news_optional: bool) -> Option<NearRow> {
    if d.membership.active().is_some() {
        return None;
    }
    let state = |p: PillarId| d.pillars.iter().find(|x| x.pillar == p);
    for p in [PillarId::Price, PillarId::Gain, PillarId::Rvol] {
        if state(p)?.state != Tri::Pass {
            return None;
        }
    }
    let mut unknown = Vec::new();
    let open: &[PillarId] = if news_optional {
        &[PillarId::Float]
    } else {
        &[PillarId::Float, PillarId::News]
    };
    for &p in open {
        let r = state(p)?;
        match r.state {
            Tri::Pass => {}
            Tri::Fail => return None,
            Tri::Unknown => unknown.push(NearPillar {
                pillar: p.as_str(),
                reason: r.reason,
            }),
        }
    }
    if unknown.is_empty() {
        // All five pass: a member at the next barrier, not a near row.
        return None;
    }
    let value = |p: PillarId| state(p).and_then(|r| r.value);
    Some(NearRow {
        symbol: d.symbol.clone(),
        price: value(PillarId::Price),
        gain_bp: value(PillarId::Gain),
        rvol_milli: value(PillarId::Rvol),
        unknown,
        float_shares: value(PillarId::Float),
        cutoff: d.cutoff,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ReasonCount {
    pub(crate) pillar: &'static str,
    pub(crate) reason: &'static str,
    pub(crate) symbols: u32,
}

#[cfg(test)]
mod restore_failure_tests {
    use super::*;

    /// A premarket deploy: the restore holds yesterday's prints and one thin print today, with no
    /// later delivery to decide today's first barrier on the restore's own clock. Left there it
    /// is still on yesterday's date and the live board refuses it (`trading_date`) — which used
    /// to leave the symbol pending for good. Advanced to the live clock first, as the loader now
    /// does (`crate::history`), it is today's and adopted.
    #[test]
    fn a_thin_premarket_restore_is_adopted_once_it_reaches_the_live_clock() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        let yesterday = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        let both = Verdict {
            ohlc: true,
            volume: true,
        };
        let print = market_events::session::et(day, 7, 0, 0, 0).unwrap();
        let now = market_events::session::et(day, 7, 5, 0, 0).unwrap();
        let fresh = || {
            let mut board = ContextBoard::new_labeled(
                false,
                Some(day),
                Price::from_cents(10),
                Price::from_cents(2500),
                true,
            )
            .unwrap();
            board.set_history_pending("THIN", true);
            let t = Trade::new("THIN", print, Price::from_cents(300), 500, 2);
            board.apply_trade(&t, Counted::from_effect(&t), &PrevCloses::default());
            board.advance(now);
            let mut restored = board.restore_engine();
            let y = market_events::session::et(yesterday, 19, 59, 0, 0).unwrap();
            restored.apply(
                &Trade::new("THIN", y, Price::from_cents(280), 900, 1),
                both,
                y,
            );
            restored.apply(
                &Trade::new("THIN", print, Price::from_cents(300), 500, 2),
                both,
                UnixNanos(print.0 + 100_000_000),
            );
            (board, restored)
        };

        // (Whether the restore's own clock leaves it on yesterday depends on whether some
        // recheck happens to land after 04:00; the loader no longer depends on that.)
        let (mut board, mut restored) = fresh();
        let _ = restored.advance_to(now);
        assert!(matches!(
            board.adopt_facts(&mut restored, "THIN", None, None, now),
            Adoption::Adopted { .. }
        ));
        assert!(!board.engine.history_pending("THIN"));
        board.advance(UnixNanos(now.0 + 30_000_000_000));
        let d = board.decision("THIN").expect("a decision");
        assert_eq!(d.trading_date, day);
        assert_eq!(
            d.volume.shares, 500,
            "today's facts, from the restore, and not yesterday's 900"
        );
    }

    /// A failed restore names its failure, and then the symbol carries on from its live tail:
    /// pending ends (it used to be terminal, so RVOL stayed UNKNOWN until a restart), a refusal
    /// leaves today's coverage Partial, and the next date starts clean.
    #[test]
    fn refused_and_empty_restores_name_the_failure_and_do_not_block_the_symbol() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        let at = market_events::session::et(day, 9, 31, 0, 0).unwrap();
        for refused in [true, false] {
            let mut board = ContextBoard::new_labeled(
                false,
                Some(day),
                Price::from_cents(10),
                Price::from_cents(2500),
                true,
            )
            .unwrap()
            .with_detection();
            board.set_detection_epoch("fixture-test:1".into());
            board.set_history_pending("ABCD", true);
            let mut restored = if refused {
                let mut run = board.engine.run_info().clone();
                run.run_id = "different-run".into();
                Engine::new(Policy::current(), run)
            } else {
                board.restore_engine()
            };
            if refused {
                let t = Trade::new("ABCD", at, Price::from_cents(300), 100, 1);
                restored.apply(
                    &t,
                    Verdict {
                        ohlc: true,
                        volume: true,
                    },
                    at,
                );
            }
            let outcome = board.adopt_facts(&mut restored, "ABCD", None, None, at);
            let gap_reason = if refused {
                assert_eq!(outcome, Adoption::Refused("run_identity"));
                assert_eq!(
                    board.engine.coverage_of("ABCD"),
                    Some((Coverage::Partial, "restore_refused_run_identity")),
                    "the tail is short of the day by the prints it refused"
                );
                "history_restore_refused"
            } else {
                assert_eq!(outcome, Adoption::Nothing);
                assert_eq!(
                    board.engine.coverage_of("ABCD"),
                    None,
                    "nothing was recorded, so the tail is all anything saw"
                );
                "history_restore_empty"
            };
            assert!(
                !board.engine.history_pending("ABCD"),
                "a failed restore does not leave the symbol pending for the life of the process"
            );
            let snapshot = board.engine.detection_snapshot().unwrap();
            assert!(snapshot.active.is_empty());
            assert!(snapshot.data_status.interrupted_symbols.iter().any(
                |gap| gap.symbol.as_deref() == Some("ABCD")
                    && gap.reason == gap_reason
                    && gap.from == at.0
            ));
            let tomorrow = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
            let next = market_events::session::et(tomorrow, 9, 30, 0, 0).unwrap();
            let trade = Trade::new("ABCD", next, Price::from_cents(300), 200, 2);
            board.apply_trade(&trade, Counted::from_effect(&trade), &PrevCloses::default());
            board.advance(UnixNanos(next.0 + 60_000_000_000));
            assert!(!board.engine.history_pending("ABCD"));
            let decision = board.decision("ABCD").unwrap();
            let rvol = decision
                .pillars
                .iter()
                .find(|p| p.pillar == PillarId::Rvol)
                .unwrap();
            assert_ne!(
                rvol.reason, "history_pending",
                "the next date is measured, not held"
            );
            assert_ne!(
                decision.volume.coverage,
                Coverage::Partial,
                "and yesterday's refusal is not a statement about today: {:?}",
                decision.volume.coverage_reason
            );
            assert!(board.engine.detection_snapshot().unwrap().active.is_empty());
        }
    }
}
