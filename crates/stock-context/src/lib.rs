//! # stock-context
//!
//! Causal per-symbol stock context for StockWatcher: the five-pillar measurements (price, day
//! gain, relative volume, float, news) of `policy/pillars-v1.json`, each PASS / FAIL / UNKNOWN
//! with its value, rule, reason and evidence, aggregated into QUALIFIED / NOT_QUALIFIED /
//! UNDETERMINED, decided on a grid of completed bars with an explicit data barrier and emitted as
//! append-only decisions. Design and sources: `docs/stock-context/pillars-v1.md`.
//!
//! The pillars are a published heuristic StockWatcher measures, not a validated trading rule and
//! not Warrior Trading's scanner. Nothing here ranks, scores or predicts.
//!
//! `detect` holds the EXPERIMENTAL detection families (`policy/detection-v2.json`,
//! `docs/stock-context/detection-core.md`). The Detection Panel runs all of them, off unless the
//! caller enables it; without it the engine still runs the micro-pullback for Ross Panel members,
//! because `cue` reads each row's pullback cue from those episodes (owner decision 2026-10-21:
//! one pullback detector everywhere). A cue describes a stage, not an entry signal.
//!
//! `setup` keeps the earlier Ross cue family `bull_flag_v0` (`policy/setups-v0.json`,
//! `docs/stock-context/setups-v0.md`) for research reproducibility only; no row uses it.
//!
//! Pure and synchronous: no I/O, no clocks, no providers. The server feeds prints and evidence
//! records; a provider outage only means fewer records (UNKNOWN), never a blocked print.

pub mod cue;
pub use cue::{CueLevel, CueState, RossCue};
pub mod detect;
pub mod engine;
pub mod evaluate;
pub mod evidence;
pub mod output;
pub mod panel;
pub mod pillar;
pub mod policy;
pub mod revision;
pub mod setup;

pub use detect::{
    Assessment, Certainty, Check, ContextClass, DETECTION_V0, DETECTION_V1, DETECTION_V2,
    DataStatus, DetectionPolicy, DetectionSnapshot, DetectionTelemetry, DetectionTransition,
    Episode as DetectionEpisode, Family, FeatureSnapshot, Gap, LevelVersion, MP5_WICK_RULE,
    MicroAnchors, MicroWick, Monitored, QuietReference, Resolution, State, SummaryFact,
    SymbolDetections, TradeEvidence, WickStatus,
};
pub use engine::{
    Adoption, Decision, Engine, Fidelity, MissingPillar, PanelRow, PanelSnapshot, RestoreTelemetry,
    RunInfo, SPARK_BARS, Timeline, Verdict, VolumeFacts,
};
pub use evaluate::{Coverage, Integrity, NewsSummary, RvolDiagnostics};
pub use evidence::{
    ActionsEvidence, ActionsSource, Authenticity, CloseBasis, CoverageState, DailyBaseline,
    EvidenceStore, Favorability, FloatObservation, InstrumentId, IntradayBaseline, IntradayCurve,
    NewsCoverage, NewsItem, OutstandingObservation, PrevClose, SessionVolume, SplitRatio, Tag,
};
pub use output::{BoundedLog, OutputTelemetry, PANEL_LOG_MAX, SETUP_LOG_MAX};
pub use panel::{
    Episode, ExitReason, MemberStatus, Membership, Momentum, MomentumStatus, PanelPolicy,
    ROSS_PANEL_V1,
};
pub use pillar::{Counts, PillarId, PillarResult, Qualification, Tri, aggregate};
pub use policy::{PILLARS_V1, PILLARS_V2, PILLARS_V5, Policy};
pub use revision::{
    CorrectedPrint, DEDUP_WINDOW_SECS, DayHigh, LastPrint, PrintRevision, REVISION_WINDOW_SECS,
    RevisionTarget, RevisionTelemetry,
};
pub use setup::{
    Level, LevelRecord, Outcome, Pending, Pole, SETUPS_V0, SetupLogEntry, SetupPolicy, SetupState,
    SetupTransition, SetupView,
};
