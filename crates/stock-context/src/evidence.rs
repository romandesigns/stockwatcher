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

    /// Every instrument at once, for records that speak for the whole market: a news search
    /// without a symbol filter covered every symbol its source tags. Stored once instead of once
    /// per symbol, that is ~1,000 coverage records a trading day instead of millions
    /// ([`NewsCoverage`], [`EvidenceStore::coverage`]).
    pub fn market() -> Self {
        InstrumentId(MARKET_ID.to_string())
    }

    pub fn is_market(&self) -> bool {
        self.0 == MARKET_ID
    }
}

const MARKET_ID: &str = "market:*";

/// What a previous close is expressed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseBasis {
    /// As printed that day (Alpaca `adjustment=raw`): today's `daily-closes.json`.
    Raw,
    /// Restated onto today's share basis for splits.
    SplitAdjusted,
}

/// What established a record's corporate-action coverage. The kinds differ in strength and are
/// never merged into one flag (docs/stock-context/daily-baseline-plan.md §6):
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionsSource {
    /// Nothing: coverage is unknown.
    #[default]
    Unchecked,
    /// Raw and split-adjusted daily bars are consistent through `actions_checked_through`: the
    /// supplied adjustment is internally consistent (prices × f, volumes ÷ f). This checks the
    /// adjustment Alpaca supplied, it does not prove no other action exists, and it only exists
    /// after the fact: it never certifies an earlier live decision.
    BarVerified,
    /// The evaluated date rests on a confirmed provider announcement (complete, consistent terms),
    /// applied as documented.
    AnnouncedAction,
    /// The evaluated date rests on a fresh, fully paginated provider check that found **no known
    /// relevant action**. Under the coverage policy this permits unchanged-share-basis
    /// calculations; it is an assumption, not verified absence (an empty answer is never proof).
    ProviderCheckedNoKnownAction,
}

/// Per-symbol corporate-action coverage state for the evaluated date (coverage policy
/// `corporate-actions-coverage-v1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageState {
    /// A relevant action with complete, consistent terms: the documented adjustment applies.
    ConfirmedAction,
    /// Fresh, fully paginated check; no relevant known action.
    NoKnownActionProviderChecked,
    /// A relevant action is known but incomplete, undated or conflicting.
    UnresolvedAction,
    /// The latest covering check is older than the policy's freshness limit.
    StaleCoverage,
    /// No usable check covers the symbol (none made, its chunk failed, wrong window).
    UnavailableCoverage,
}

/// A split ratio as announced: `old_rate` shares became `new_rate` (2:1 forward = 1 → 2;
/// 1-for-30 reverse = 30 → 1). Price multiplier `old/new`, volume multiplier `new/old`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SplitRatio {
    pub old_rate: u64,
    pub new_rate: u64,
}

/// Provenance of a record's corporate-action coverage.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ActionsEvidence {
    pub source: ActionsSource,
    /// The share basis the record is expressed on (the evaluated date).
    pub basis_date: Option<NaiveDate>,
    /// Through which date daily bars were consistent (`BarVerified`, or the history under an
    /// announcement-based last day).
    pub bar_verified_through: Option<NaiveDate>,
    /// The announcement coverage state, when announcements were consulted.
    pub coverage: Option<CoverageState>,
    /// `corporate-actions-coverage-v1@<sha256 prefix>`, when announcements were consulted.
    pub coverage_policy: Option<String>,
    /// The announcement retrieval relied on.
    pub announcement: Option<String>,
    /// Splits applied from announcements, with their action ids, oldest first.
    pub applied_splits: Vec<(SplitRatio, String)>,
    /// An explicit assumption the record rests on (e.g. `unchanged_share_basis_assumed`).
    pub assumption: Option<String>,
    /// The ticker the history was read under, when it differs (a rename in the window).
    pub history_symbol: Option<String>,
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
    /// What established `actions_checked_through` (the engine does not grade it; it is shown).
    pub actions: ActionsEvidence,
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
    /// What put the sessions on the evaluated date's share basis.
    pub basis: ActionsEvidence,
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

/// Which news sources were searched successfully, over what window, for one instrument, or for
/// every instrument when `instrument` is [`InstrumentId::market`] (a search without a symbol
/// filter). A lookup for one instrument takes the latest of its own and the market's records
/// ([`EvidenceStore::coverage`]).
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

/// Appends `r` unless an identical record is already there: a record delivered twice (a server
/// re-reading the same files for a new session date) is one record, not two news items.
fn push_new<T: PartialEq>(map: &mut HashMap<InstrumentId, Vec<T>>, i: &InstrumentId, r: T) {
    let rows = map.entry(i.clone()).or_default();
    if !rows.contains(&r) {
        rows.push(r);
    }
}

impl EvidenceStore {
    pub fn add_prev_close(&mut self, r: PrevClose) {
        let i = r.instrument.clone();
        push_new(&mut self.prev_close, &i, r);
    }
    pub fn add_daily(&mut self, r: DailyBaseline) {
        let i = r.instrument.clone();
        push_new(&mut self.daily, &i, r);
    }
    pub fn add_intraday(&mut self, r: IntradayBaseline) {
        let i = r.instrument.clone();
        push_new(&mut self.intraday, &i, r);
    }
    pub fn add_float(&mut self, r: FloatObservation) {
        let i = r.instrument.clone();
        push_new(&mut self.float, &i, r);
    }
    pub fn add_news(&mut self, r: NewsItem) {
        let i = r.instrument.clone();
        push_new(&mut self.news, &i, r);
    }
    pub fn add_coverage(&mut self, r: NewsCoverage) {
        let i = r.instrument.clone();
        push_new(&mut self.coverage, &i, r);
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
                .chain(self.coverage.get(&InstrumentId::market()))
                .flatten()
                .map(|r| r.observed_at),
        );
        times.into_iter().filter(|&x| x > t).min()
    }

    /// The latest coverage record observed by `cutoff`, of the instrument's own and the
    /// market-wide ones. The two are separate runs (a market run is never merged with a
    /// per-symbol one), so taking the latest can only see a shorter run than a merge would:
    /// UNKNOWN where a merge might say FAIL, never the other way round.
    pub fn coverage(&self, i: &InstrumentId, cutoff: i64) -> Option<&NewsCoverage> {
        let own = latest(self.coverage.get(i), cutoff, |r| (r.observed_at, 0));
        let market = latest(self.coverage.get(&InstrumentId::market()), cutoff, |r| {
            (r.observed_at, 0)
        });
        match (own, market) {
            (Some(a), Some(m)) => Some(if m.observed_at > a.observed_at { m } else { a }),
            (a, m) => a.or(m),
        }
    }

    /// Drops every record no decision at a cutoff `>= before` can read; returns how many.
    ///
    /// Exact, not approximate: a join takes the latest record observed by the cutoff, so per
    /// instrument and kind the latest one observed at or before `before` and everything observed
    /// after it are kept. A news item only qualifies while `cutoff - published_at <= fresh`, so
    /// one published before `before - fresh` (or never dated) can never qualify again.
    ///
    /// This is what keeps a long-running live engine bounded: the server re-issues daily
    /// baselines with every announcement check and market news coverage with every news poll.
    pub fn prune(&mut self, before: i64, news_fresh: i64) -> usize {
        fn keep_latest<T>(
            map: &mut HashMap<InstrumentId, Vec<T>>,
            before: i64,
            at: impl Fn(&T) -> (i64, u32),
        ) -> usize {
            let mut dropped = 0;
            for rows in map.values_mut() {
                let keep = rows.iter().map(&at).filter(|k| k.0 <= before).max();
                let n = rows.len();
                let mut kept = false;
                rows.retain(|r| {
                    let k = at(r);
                    if k.0 > before {
                        true
                    } else if Some(k) == keep && !kept {
                        kept = true;
                        true
                    } else {
                        false
                    }
                });
                dropped += n - rows.len();
            }
            map.retain(|_, v| !v.is_empty());
            dropped
        }
        let mut n = 0;
        n += keep_latest(&mut self.prev_close, before, |r| (r.observed_at, r.version));
        n += keep_latest(&mut self.daily, before, |r| (r.observed_at, r.version));
        n += keep_latest(&mut self.intraday, before, |r| (r.observed_at, r.version));
        n += keep_latest(&mut self.float, before, |r| (r.observed_at, r.version));
        n += keep_latest(&mut self.coverage, before, |r| (r.observed_at, 0));
        for rows in self.news.values_mut() {
            let len = rows.len();
            rows.retain(|r| {
                r.observed_at > before || r.published_at.is_some_and(|p| p >= before - news_fresh)
            });
            n += len - rows.len();
        }
        self.news.retain(|_, v| !v.is_empty());
        n
    }

    /// Records held, all kinds (tests and logs).
    pub fn len(&self) -> usize {
        fn count<T>(m: &HashMap<InstrumentId, Vec<T>>) -> usize {
            m.values().map(Vec::len).sum()
        }
        count(&self.prev_close)
            + count(&self.daily)
            + count(&self.intraday)
            + count(&self.float)
            + count(&self.news)
            + count(&self.coverage)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn float(sym: &str, version: u32, observed_at: i64) -> FloatObservation {
        FloatObservation {
            id: format!("float:{sym}"),
            version,
            instrument: InstrumentId::symbol(sym),
            shares: 1_000_000 * u64::from(version),
            provider: "fixture".into(),
            effective_as_of: None,
            observed_at,
            quality: vec![],
        }
    }

    fn cov(id: &str, instrument: InstrumentId, through: i64, observed_at: i64) -> NewsCoverage {
        NewsCoverage {
            id: id.into(),
            instrument,
            sources: vec!["alpaca/benzinga".into()],
            covered_from: 0,
            covered_through: through,
            observed_at,
        }
    }

    fn news(published_at: Option<i64>, observed_at: i64) -> NewsItem {
        NewsItem {
            id: format!("news:{observed_at}"),
            version: 1,
            instrument: InstrumentId::symbol("AAA"),
            event_type: "earnings".into(),
            source: "alpaca/benzinga".into(),
            credible_source: true,
            relevant: true,
            substantive: true,
            authenticity: Authenticity::ReportedUnverified,
            favorability: Favorability::Undetermined,
            published_at,
            observed_at,
        }
    }

    #[test]
    fn market_coverage_serves_every_instrument_and_the_latest_of_the_two_wins() {
        let mut s = EvidenceStore::default();
        let a = InstrumentId::symbol("AAA");
        s.add_coverage(cov("m1", InstrumentId::market(), 100, 110));
        assert_eq!(s.coverage(&a, 109), None, "not observed yet");
        assert_eq!(s.coverage(&a, 110).unwrap().id, "m1");
        assert_eq!(
            s.coverage(&InstrumentId::symbol("ZZZ"), 200).unwrap().id,
            "m1"
        );
        s.add_coverage(cov("own", a.clone(), 150, 160));
        assert_eq!(s.coverage(&a, 200).unwrap().id, "own");
        s.add_coverage(cov("m2", InstrumentId::market(), 190, 200));
        assert_eq!(s.coverage(&a, 200).unwrap().id, "m2");
        assert_eq!(
            s.next_after(&a, 160),
            Some(200),
            "a market record is AAA's too"
        );
    }

    #[test]
    fn prune_keeps_exactly_what_a_later_cutoff_can_read() {
        let mut s = EvidenceStore::default();
        let a = InstrumentId::symbol("AAA");
        for (v, t) in [(1, 10), (2, 20), (3, 30), (4, 50)] {
            s.add_float(float("AAA", v, t));
        }
        s.add_coverage(cov("m1", InstrumentId::market(), 5, 10));
        s.add_coverage(cov("m2", InstrumentId::market(), 15, 20));
        let fresh = 100;
        // Published at 0: still fresh for cutoffs up to 100.
        s.add_news(news(Some(0), 5));
        // Published at -70: too old for any cutoff after 30.
        s.add_news(news(Some(-70), 6));
        // Never dated: never qualifies.
        s.add_news(news(None, 7));
        let joins = |s: &EvidenceStore| -> Vec<_> {
            (41..60)
                .map(|c| (s.float(&a, c).cloned(), s.coverage(&a, c).cloned()))
                .collect()
        };
        let before = joins(&s);
        let dropped = s.prune(40, fresh);
        assert_eq!(dropped, 2 + 1 + 2, "floats v1, v2; coverage m1; two news");
        assert_eq!(
            joins(&s),
            before,
            "every join at a cutoff > 40 is unchanged"
        );
        assert_eq!(s.news(&a, 41).len(), 1);
        assert_eq!(s.float(&a, 49).unwrap().version, 3);
        assert_eq!(s.float(&a, 50).unwrap().version, 4);
        assert_eq!(s.len(), 2 + 1 + 1);
    }
}
