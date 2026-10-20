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
//! Pure and synchronous: no I/O, no clocks, no providers. The server feeds prints and evidence
//! records; a provider outage only means fewer records (UNKNOWN), never a blocked print.

pub mod engine;
pub mod evaluate;
pub mod evidence;
pub mod panel;
pub mod pillar;
pub mod policy;

pub use engine::{
    Decision, Engine, Fidelity, PanelRow, PanelSnapshot, RunInfo, Timeline, Verdict, VolumeFacts,
};
pub use evaluate::{Coverage, NewsSummary, RvolDiagnostics};
pub use evidence::{
    ActionsEvidence, ActionsSource, Authenticity, CloseBasis, CoverageState, DailyBaseline,
    EvidenceStore, Favorability, FloatObservation, InstrumentId, IntradayBaseline, IntradayCurve,
    NewsCoverage, NewsItem, PrevClose, SessionVolume, SplitRatio,
};
pub use panel::{
    Episode, ExitReason, MemberStatus, Membership, Momentum, MomentumStatus, PanelPolicy,
    ROSS_PANEL_V1,
};
pub use pillar::{Counts, PillarId, PillarResult, Qualification, Tri, aggregate};
pub use policy::{PILLARS_V1, Policy};
