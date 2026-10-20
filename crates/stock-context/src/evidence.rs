//! Evidence records the trades alone cannot give: the previous close and its adjustment coverage,
//! prior sessions' volumes, float, news and news coverage.
//!
//! Records are append-only: a correction or a newer retrieval is a new record (a new `version` or
//! a later `observed_at`), never an edit of an old one. A decision joins the latest record whose
//! `observed_at` is at or before its cutoff, so evidence that arrives later can never reach back
//! into an earlier decision (the leakage rule).
//!
//! Instrument identity: StockWatcher has no instrument master yet, so `instrument` is the symbol
//! (`InstrumentId::symbol`), flagged `symbol_only`. A rename or a reused ticker cannot be told
//! apart until a master exists; records carry the id so that switch changes no logic.

use chrono::NaiveDate;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct InstrumentId(pub String);

impl InstrumentId {
    /// The v1 identity: the display symbol itself.
    pub fn symbol(sym: &str) -> Self {
        InstrumentId(format!("sym:{sym}"))
    }
}

/// What a previous close is expressed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseBasis {
    /// As printed that day (Alpaca `adjustment=raw`): today's `daily-closes.json`.
    Raw,
    /// Restated onto today's share basis for splits.
    SplitAdjusted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PrevClose {
    pub id: String,
    pub version: u32,
    pub instrument: InstrumentId,
    /// The regular session the close belongs to.
    pub date: NaiveDate,
    pub close_micros: i64,
    pub basis: CloseBasis,
    /// Corporate actions were checked (and applied when `SplitAdjusted`) through this trading
    /// date; `None`: not checked. The gain pillar needs it to reach the evaluated date.
    pub actions_checked_through: Option<NaiveDate>,
    pub observed_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionVolume {
    pub date: NaiveDate,
    /// Eligible shares 04:00 → session end, on today's share basis.
    pub volume: u64,
    /// Collection covered the whole window (a covered zero is a valid zero; a gap is not).
    pub complete: bool,
    /// A full trading day comparable to a normal one (an early close is not).
    pub comparable: bool,
}

/// Prior sessions' full-day volumes for the daily-denominator RVOL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DailyBaseline {
    pub id: String,
    pub version: u32,
    pub instrument: InstrumentId,
    pub sessions: Vec<SessionVolume>,
    pub observed_at: i64,
}

/// One prior session's cumulative eligible volume curve from 04:00: `(seconds since 04:00,
/// cumulative shares)`, ascending.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IntradayCurve {
    pub date: NaiveDate,
    pub cum: Vec<(i64, u64)>,
    pub complete: bool,
}

impl IntradayCurve {
    /// Cumulative volume at `offset` seconds since 04:00 (the last point at or before it).
    pub fn at(&self, offset: i64) -> u64 {
        let i = self.cum.partition_point(|&(s, _)| s <= offset);
        if i == 0 { 0 } else { self.cum[i - 1].1 }
    }
}

/// Prior sessions' intraday curves for the same-time and recent-window diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IntradayBaseline {
    pub id: String,
    pub version: u32,
    pub instrument: InstrumentId,
    pub sessions: Vec<IntradayCurve>,
    pub observed_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FloatObservation {
    pub id: String,
    pub version: u32,
    pub instrument: InstrumentId,
    /// Public float as the provider states it (never shares outstanding).
    pub shares: u64,
    pub provider: String,
    pub effective_as_of: Option<NaiveDate>,
    /// When we retrieved it: freshness counts from here (a fresh retrieval does not prove a fresh
    /// provider figure).
    pub observed_at: i64,
    pub quality: Vec<String>,
}

/// What a news record actually establishes about its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Authenticity {
    ReportedUnverified,
    IssuerAnnounced,
    RegulatorConfirmed,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Favorability {
    Favorable,
    Adverse,
    Mixed,
    Undetermined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NewsItem {
    pub id: String,
    pub version: u32,
    pub instrument: InstrumentId,
    /// earnings, guidance, regulatory_decision, clinical_result, commercial_agreement, financing,
    /// corporate_action, other.
    pub event_type: String,
    pub source: String,
    /// The source is identified and credible for what it reports.
    pub credible_source: bool,
    pub relevant: bool,
    pub substantive: bool,
    pub authenticity: Authenticity,
    pub favorability: Favorability,
    pub published_at: Option<i64>,
    /// When StockWatcher first saw this version (the time a forward decision may use).
    pub observed_at: i64,
}

/// Which news sources were searched successfully, over what window, for one instrument.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NewsCoverage {
    pub id: String,
    pub instrument: InstrumentId,
    pub sources: Vec<String>,
    pub covered_from: i64,
    pub covered_through: i64,
    pub observed_at: i64,
}

/// Every evidence record known to a run, per instrument, append-only.
#[derive(Clone, Debug, Default)]
pub struct EvidenceStore {
    prev_close: HashMap<InstrumentId, Vec<PrevClose>>,
    daily: HashMap<InstrumentId, Vec<DailyBaseline>>,
    intraday: HashMap<InstrumentId, Vec<IntradayBaseline>>,
    float: HashMap<InstrumentId, Vec<FloatObservation>>,
    news: HashMap<InstrumentId, Vec<NewsItem>>,
    coverage: HashMap<InstrumentId, Vec<NewsCoverage>>,
}

/// The record with the latest `observed_at` (then the highest version) at or before `cutoff`.
fn latest<T>(rows: Option<&Vec<T>>, cutoff: i64, at: impl Fn(&T) -> (i64, u32)) -> Option<&T> {
    rows?
        .iter()
        .filter(|r| at(r).0 <= cutoff)
        .max_by_key(|r| at(r))
}

impl EvidenceStore {
    pub fn add_prev_close(&mut self, r: PrevClose) {
        self.prev_close
            .entry(r.instrument.clone())
            .or_default()
            .push(r);
    }
    pub fn add_daily(&mut self, r: DailyBaseline) {
        self.daily.entry(r.instrument.clone()).or_default().push(r);
    }
    pub fn add_intraday(&mut self, r: IntradayBaseline) {
        self.intraday
            .entry(r.instrument.clone())
            .or_default()
            .push(r);
    }
    pub fn add_float(&mut self, r: FloatObservation) {
        self.float.entry(r.instrument.clone()).or_default().push(r);
    }
    pub fn add_news(&mut self, r: NewsItem) {
        self.news.entry(r.instrument.clone()).or_default().push(r);
    }
    pub fn add_coverage(&mut self, r: NewsCoverage) {
        self.coverage
            .entry(r.instrument.clone())
            .or_default()
            .push(r);
    }

    pub fn prev_close(&self, i: &InstrumentId, cutoff: i64) -> Option<&PrevClose> {
        latest(self.prev_close.get(i), cutoff, |r| {
            (r.observed_at, r.version)
        })
    }
    pub fn daily(&self, i: &InstrumentId, cutoff: i64) -> Option<&DailyBaseline> {
        latest(self.daily.get(i), cutoff, |r| (r.observed_at, r.version))
    }
    pub fn intraday(&self, i: &InstrumentId, cutoff: i64) -> Option<&IntradayBaseline> {
        latest(self.intraday.get(i), cutoff, |r| (r.observed_at, r.version))
    }
    pub fn float(&self, i: &InstrumentId, cutoff: i64) -> Option<&FloatObservation> {
        latest(self.float.get(i), cutoff, |r| (r.observed_at, r.version))
    }
    /// Every news version first observed at or before `cutoff` (later edits are separate rows).
    pub fn news(&self, i: &InstrumentId, cutoff: i64) -> Vec<&NewsItem> {
        self.news
            .get(i)
            .map(|v| v.iter().filter(|n| n.observed_at <= cutoff).collect())
            .unwrap_or_default()
    }
    /// The earliest `observed_at` after `t` of any record for `i`: when this instrument's visible
    /// evidence next changes.
    pub fn next_after(&self, i: &InstrumentId, t: i64) -> Option<i64> {
        let mut times = Vec::new();
        times.extend(
            self.prev_close
                .get(i)
                .into_iter()
                .flatten()
                .map(|r| r.observed_at),
        );
        times.extend(
            self.daily
                .get(i)
                .into_iter()
                .flatten()
                .map(|r| r.observed_at),
        );
        times.extend(
            self.intraday
                .get(i)
                .into_iter()
                .flatten()
                .map(|r| r.observed_at),
        );
        times.extend(
            self.float
                .get(i)
                .into_iter()
                .flatten()
                .map(|r| r.observed_at),
        );
        times.extend(
            self.news
                .get(i)
                .into_iter()
                .flatten()
                .map(|r| r.observed_at),
        );
        times.extend(
            self.coverage
                .get(i)
                .into_iter()
                .flatten()
                .map(|r| r.observed_at),
        );
        times.into_iter().filter(|&x| x > t).min()
    }

    pub fn coverage(&self, i: &InstrumentId, cutoff: i64) -> Option<&NewsCoverage> {
        latest(self.coverage.get(i), cutoff, |r| (r.observed_at, 0))
    }
}
