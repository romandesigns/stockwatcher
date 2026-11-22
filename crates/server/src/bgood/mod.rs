//! b-good, live (owner, DEPLOY-PLAN.md addendum 2026-11-22 ~01:30 ET: "everything that has proven
//! better than what's live, push it live"): research v2 wave 2's lane B-good, two frozen XGBoost
//! models over the same detector-free market features.
//!
//! - **b-good-1, the confidence tag** (T-B1). Every B0-kind alert (the default-switch Runners
//!   stream: new, burst, running, run, fast, shelf, followup, pullback, pullback_crossing, n1, f1u,
//!   halt_bull) is scored **30 s after it fires**; a score at or above θ tags it "high confidence".
//!   Validation: tagged alerts were GOOD (good-v1: +10 % from the ask 30 s later before −5 %, on
//!   2× time-of-day volume) 29.2 % of the time, against 20.5 % for the rest. The tag never sends
//!   anything on its own: it arrives as an update of an alert already delivered
//!   ([`crate::alerts`], `runners.confidence`), ledgered either way.
//! - **b-good-2, model picks** (T-A2). Every stock at every minute end, scored by the second model;
//!   the online rule keeps at most 2 a stock 10 minutes apart, none within 30 minutes after a B0
//!   alert of the stock, at most 30 a day. Each pick is a new Runners notice
//!   ([`crate::rocket::NoticeKind::BGood`], data kind `model.pick`). Validation: +2.0 points of
//!   recall and +0.65 of precision over B0, about 27 a day.
//!
//! The decisions themselves are [`logic`] (std only, so the parity harness compiles it unchanged)
//! and the boosters are [`xgb`]. Parity on ten Train days (2024-06-11..28) against the research's
//! own rows: `C:/wavystack/outputs/stockwatcher-research-v2/50-ship/bgood/PARITY.md`.
//!
//! # What this module feeds the detector
//!
//! - **15 s bars** from every print the Runners board counts, with the chart engine's verdict, on
//!   the 04:00 ET grid: high / low / close in micro-dollars from OHLC-counted prints (the close is
//!   the latest execution time, the later arrival on a tie), shares, dollars (`px × size`) and the
//!   number of volume-counted prints. A bar is decided at its end + 2 s on the rocket task's clock;
//!   a print arriving after that is dropped and counted (`late`).
//! - **Daily facts** (C1, H1, ADV20, DADV20, MDR20, runs90, runs365, prior_sessions) from the
//!   archive's raw daily bars with the corporate-actions splits, the research rule
//!   ([`logic::day_facts`]), read at 03:30 ET.
//! - **Time-of-day baselines** (5 and 20 prior sessions, per stock and 5-minute bucket), from this
//!   module's own daily files (`<archive>/bgood/tod/<date>.tsv`, written at the end of each day
//!   watched whole: [`TodDay`]).
//! - **Quotes** (b-good-1 only): the last valid NBBO at or before the alert and at the scoring
//!   instant, fetched by the confidence stage (`crate::spread`'s quotes endpoint).
//!
//! # It stays silent unless its inputs are the research's
//!
//! A pick or a tag is sent only on a day watched whole from 04:00 (as N1: every feature runs from
//! the first print of the day) whose inputs match what the models were trained on:
//!
//! - at least [`MIN_HISTORY_DATES`] raw daily-bar dates. `prior_sessions` is "bars since the
//!   research archive began" (2021-01-04), and the models split on it up to 825; a live archive
//!   keeping 63 dates (`DAILY_BARS_KEEP_DAYS` default) would put every listed stock at 63, a value
//!   the models read as a recent listing. `DAILY_BARS_KEEP_DAYS=0` and a backfill are deploy steps;
//! - [`TOD_SESSIONS`] time-of-day sessions on file (the 20-session baseline).
//!
//! Without them it keeps deciding (the counts are in the day summary) and sends nothing, logged
//! once a day with the reason. It is never fed a guess.
//!
//! # Known differences from the research (each counted in PARITY.md or the day summary)
//!
//! - `log1p` / `log` are correctly rounded here and within an ulp on the research's GPU: on the
//!   ten parity days 1 of 1,861 tag margins and 5 of 11,683 pick-sample margins differed, and no
//!   decision changed.
//! - Bars: the research cache stored each bar's VWAP to $0.0001 and dropped canceled prints with
//!   hindsight; live sums the prints as they come.
//! - Universe: the research tape held stocks closing $0.05–$30 that day; live sees all.
//! - Quotes: research quote files covered about 87 % of B0 alerts (`q_flags` 1 = no file on the
//!   rest, which the model learned); live always fetches one, so a tag is decided on the
//!   quote-present branches.
//! - Picks at the same minute: the research's order among them was unspecified (a sort by time);
//!   live takes them by ticker, which matters only when the day's 30th pick falls inside a tie.

pub mod logic;
pub mod stage;
pub mod xgb;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use chrono::NaiveDate;
use market_events::session::{SessionKind, sessions_for, trading_date};
use market_events::{Trade, UnixNanos};
use serde_json::{Value, json};

use crate::n1_live::SecType;
use crate::quotes::Counted;
use crate::rocket::{Notice, NoticeDetail, NoticeKind};
use logic::{Bar, Ctx, DayFacts, NF, NSLOT, Picker, SLOT_NS, SymFeat, TodRow, TodSym};
pub use xgb::XgbModel;

/// Detector ids, as the finalist records and the per-alert ledger name them.
pub const TAG_ID: &str = "b-good-1";
pub const PICK_ID: &str = "b-good-2";
/// Raw daily-bar dates needed before anything is sent (module docs).
pub const MIN_HISTORY_DATES: usize = 826;
/// Raw daily-bar dates read at most.
pub const HISTORY_LOOKBACK: usize = 900;
/// Time-of-day sessions the baseline needs.
pub const TOD_SESSIONS: usize = 20;
/// When the day's inputs are read: 30 minutes before 04:00 ET.
pub const FACTS_LEAD_NS: i64 = 30 * 60 * logic::SEC;
const START_GRACE_NS: i64 = 60 * logic::SEC;
const MAX_NOTICES: usize = 256;
const MAX_PENDING: usize = 4096;

static M1: OnceLock<XgbModel> = OnceLock::new();
static M2: OnceLock<XgbModel> = OnceLock::new();

/// The frozen b-good-1 booster (embedded `b-good-1.xgb.txt`).
pub fn model1() -> &'static XgbModel {
    M1.get_or_init(|| XgbModel::parse(include_str!("b-good-1.xgb.txt")).expect("b-good-1.xgb.txt"))
}

/// The frozen b-good-2 booster (embedded `b-good-2.xgb.txt`).
pub fn model2() -> &'static XgbModel {
    M2.get_or_init(|| XgbModel::parse(include_str!("b-good-2.xgb.txt")).expect("b-good-2.xgb.txt"))
}

/// The version every ledger line carries: the finalist record's sha256 (`b-good-N.json`, sealed in
/// `SEALS.sha256`), first 12 hex digits.
pub fn version(m: &XgbModel) -> String {
    m.finalist_sha256.chars().take(12).collect()
}

/// The day's inputs ([`load_inputs`]).
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub date: Option<NaiveDate>,
    pub facts: HashMap<String, DayFacts>,
    pub types: HashMap<String, SecType>,
    pub tod: HashMap<String, TodSym>,
    pub raw_dates: usize,
    pub tod_sessions: usize,
}

impl Inputs {
    /// Why nothing may be sent today, `None` when the inputs are complete (module docs).
    pub fn missing(&self) -> Option<String> {
        let mut why = Vec::new();
        if self.raw_dates < MIN_HISTORY_DATES {
            why.push(format!(
                "{} raw daily-bar dates (needs {MIN_HISTORY_DATES}: DAILY_BARS_KEEP_DAYS=0 and a backfill)",
                self.raw_dates
            ));
        }
        if self.tod_sessions < TOD_SESSIONS {
            why.push(format!(
                "{} time-of-day sessions on file (needs {TOD_SESSIONS})",
                self.tod_sessions
            ));
        }
        (!why.is_empty()).then(|| why.join("; "))
    }

    fn excluded(&self, sym: &str) -> bool {
        self.types
            .get(sym)
            .copied()
            .unwrap_or_else(|| crate::n1_live::logic::sec_type(sym, None))
            == SecType::Excluded
    }
}

/// The time-of-day file of one finished day: per stock and 5-minute bucket, volume-counted prints,
/// shares and dollars (the research's `tod_agg`), and the stocks present (closing $0.05–$30, the
/// research archive's membership, `tod_present`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TodDay {
    pub date: Option<NaiveDate>,
    pub present: Vec<String>,
    /// `(symbol, bucket, n, shares, dollars)`.
    pub rows: Vec<(String, u8, u64, u64, f32)>,
}

impl TodDay {
    /// `P SYM` lines for the present stocks, then `SYM bucket n shares dollars`.
    pub fn to_tsv(&self) -> String {
        let mut s = String::new();
        for p in &self.present {
            s.push_str("P\t");
            s.push_str(p);
            s.push('\n');
        }
        for (sym, b, n, v, d) in &self.rows {
            s.push_str(&format!("{sym}\t{b}\t{n}\t{v}\t{d:?}\n"));
        }
        s
    }

    pub fn parse(txt: &str) -> TodDay {
        let mut t = TodDay::default();
        for l in txt.lines() {
            let p: Vec<&str> = l.split('\t').collect();
            match p.as_slice() {
                ["P", s] => t.present.push((*s).to_string()),
                [s, b, n, v, d] => {
                    if let (Ok(b), Ok(n), Ok(v), Ok(d)) =
                        (b.parse(), n.parse(), v.parse(), d.parse())
                    {
                        t.rows.push(((*s).to_string(), b, n, v, d));
                    }
                }
                _ => {}
            }
        }
        t
    }
}

/// `Cache.tod_baseline` over the given prior days (newest last): per (stock, bucket) the mean over
/// k = 5 and k = 20 sessions of shares and dollars (a session without the stock counts 0; the sum
/// divided by k even when fewer days exist, as the research), and `present{k}` = sessions among the
/// k where the stock was present.
pub fn tod_baseline(prior: &[TodDay]) -> HashMap<String, TodSym> {
    let mut out: HashMap<String, TodSym> = HashMap::new();
    for (k, is5) in [(20usize, false), (5usize, true)] {
        let days = &prior[prior.len().saturating_sub(k)..];
        let mut sums: HashMap<(&str, u8), (f64, f64)> = HashMap::new();
        let mut pres: HashMap<&str, u32> = HashMap::new();
        for d in days {
            for (s, b, _n, v, dol) in &d.rows {
                let e = sums.entry((s.as_str(), *b)).or_default();
                e.0 += *v as f64;
                e.1 += *dol as f64;
            }
            for p in &d.present {
                *pres.entry(p.as_str()).or_default() += 1;
            }
        }
        for ((s, b), (v, dol)) in sums {
            let e = out.entry(s.to_string()).or_default();
            let r = e.rows[b as usize].get_or_insert(TodRow::default());
            let (mv, md) = ((v / k as f64) as f32, (dol / k as f64) as f32);
            if is5 {
                r.b5_vol = mv;
                r.b5_dol = md;
            } else {
                r.b20_vol = mv;
                r.b20_dol = md;
            }
        }
        for (s, n) in pres {
            if let Some(e) = out.get_mut(s) {
                for r in e.rows.iter_mut().flatten() {
                    if is5 {
                        r.present5 = n;
                    } else {
                        r.present20 = n;
                    }
                }
            }
        }
    }
    out
}

/// Reads `date`'s inputs from the archive (off the ingest path: the market runs it on a blocking
/// thread): daily facts over every raw daily-bar date before the day (at most
/// [`HISTORY_LOOKBACK`]), split-restated with `splits`; security types from `assets.json`; the
/// time-of-day files of the [`TOD_SESSIONS`] newest dates before the day.
pub fn load_inputs(
    archive: &market_archive::Archive,
    date: NaiveDate,
    splits: &[market_archive::SplitFact],
) -> Inputs {
    let dates: Vec<NaiveDate> = archive
        .raw_bar_dates()
        .unwrap_or_default()
        .into_iter()
        .filter(|d| *d < date)
        .collect();
    let tail = &dates[dates.len().saturating_sub(HISTORY_LOOKBACK)..];
    let ord = |d: NaiveDate| d.signed_duration_since(NaiveDate::MIN).num_days();
    let mut by_sym: HashMap<String, Vec<logic::DailyIn>> = HashMap::new();
    let mut read = 0;
    for d in tail {
        let Some(b) = archive.read_raw_bars(*d).ok().flatten() else {
            continue;
        };
        read += 1;
        for (s, bar) in &b.bars {
            if let (Some(h), Some(l)) = (bar.h, bar.l) {
                by_sym.entry(s.clone()).or_default().push(logic::DailyIn {
                    day: ord(*d),
                    h,
                    l,
                    c: bar.c,
                    v: bar.v.unwrap_or(0) as f64,
                    vw: bar.vw.unwrap_or(bar.c),
                });
            }
        }
    }
    let mut sp: HashMap<&str, Vec<(i64, f64)>> = HashMap::new();
    for s in splits {
        let v = sp.entry(s.symbol.as_str()).or_default();
        let e = ord(s.effective);
        if !v.iter().any(|(d, _)| *d == e) {
            v.push((e, s.factor));
        }
    }
    let day = ord(date);
    let facts = by_sym
        .into_iter()
        .map(|(s, v)| {
            let f = {
                let sv = sp.get(s.as_str());
                logic::day_facts(
                    &v,
                    |from, to| {
                        sv.map(|x| {
                            x.iter()
                                .filter(|(d, _)| *d > from && *d <= to)
                                .map(|(_, f)| *f)
                                .product()
                        })
                        .unwrap_or(1.0)
                    },
                    day,
                )
            };
            (s, f)
        })
        .collect();
    let assets = archive.read_assets().ok().flatten().unwrap_or_default();
    let types = assets
        .iter()
        .map(|(s, a)| (s.clone(), crate::n1_live::logic::sec_type(s, Some(&a.name))))
        .collect();
    let dir = archive.root().join("bgood").join("tod");
    let mut tod_files: Vec<NaiveDate> = std::fs::read_dir(&dir)
        .map(|r| {
            r.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    NaiveDate::parse_from_str(n.strip_suffix(".tsv")?, "%Y-%m-%d").ok()
                })
                .filter(|d| *d < date)
                .collect()
        })
        .unwrap_or_default();
    tod_files.sort();
    let prior: Vec<TodDay> = tod_files[tod_files.len().saturating_sub(TOD_SESSIONS)..]
        .iter()
        .filter_map(|d| std::fs::read_to_string(dir.join(format!("{d}.tsv"))).ok())
        .map(|t| TodDay::parse(&t))
        .collect();
    Inputs {
        date: Some(date),
        facts,
        types,
        tod_sessions: prior.len(),
        tod: tod_baseline(&prior),
        raw_dates: read,
    }
}

/// Writes a finished day's time-of-day file (module docs). Off the ingest path.
pub fn write_tod(archive_root: &std::path::Path, day: &TodDay) -> std::io::Result<()> {
    let Some(d) = day.date else { return Ok(()) };
    let dir = archive_root.join("bgood").join("tod");
    std::fs::create_dir_all(&dir)?;
    let p = dir.join(format!("{d}.tsv"));
    let tmp = dir.join(format!("{d}.tsv.tmp"));
    std::fs::write(&tmp, day.to_tsv())?;
    std::fs::rename(tmp, p)
}

/// A pick's facts ([`NoticeDetail::BGood`]).
#[derive(Clone, Debug, PartialEq)]
pub struct BGoodNotice {
    /// The model's float32 margin and its probability (display only: the decision is the margin).
    pub margin: f32,
    pub prob: f32,
    /// The day's picks so far, this one included.
    pub nth: u32,
    pub version: String,
    /// When the server decided it (the rocket task's clock, epoch ns).
    pub emitted_ns: i64,
}

/// A b-good-1 tag due for its quotes and score ([`score_tag`]): everything but the quote group.
#[derive(Clone, Debug, PartialEq)]
pub struct TagRequest {
    pub event_id: String,
    /// The notice's `alert_id` (a Runners row's id for a row's own entry).
    pub alert_id: String,
    pub symbol: String,
    pub kind: NoticeKind,
    pub kind_code: usize,
    pub tier: u8,
    /// The alert's decision instant `a` and the scoring instant `t = a + 30 s` (epoch ns).
    pub a_ns: i64,
    pub t_ns: i64,
    pub ctx: (f32, f32, f32),
    pub ts: [f32; NF],
}

/// A scored tag: the alerts task ledgers it and, when tagged, updates the alert on every surface.
#[derive(Clone, Debug, PartialEq)]
pub struct Confidence {
    pub event_id: String,
    pub alert_id: String,
    pub symbol: String,
    pub kind: NoticeKind,
    pub a_ns: i64,
    pub t_ns: i64,
    pub margin: f32,
    pub prob: f32,
    pub tagged: bool,
    pub q_a: logic::QuoteAt,
    pub q_t: logic::QuoteAt,
    /// When the stage scored it (epoch ns).
    pub decided_ns: i64,
}

impl Confidence {
    /// `"high"` when tagged, else `"none"`: the only tiers there are.
    pub fn tier(&self) -> &'static str {
        if self.tagged { "high" } else { "none" }
    }
}

/// b-good-1's decision for a request with its two quotes ([`logic::QuoteAt::at`]).
pub fn score_tag(
    r: &TagRequest,
    q_a: logic::QuoteAt,
    q_t: logic::QuoteAt,
    now_ns: i64,
) -> Confidence {
    let m = model1();
    let x = logic::u1_row(&r.ts, r.kind_code, r.tier, r.ctx, &q_a, &q_t);
    let margin = m.margin(&x);
    Confidence {
        event_id: r.event_id.clone(),
        alert_id: r.alert_id.clone(),
        symbol: r.symbol.clone(),
        kind: r.kind,
        a_ns: r.a_ns,
        t_ns: r.t_ns,
        margin,
        prob: XgbModel::prob(margin),
        tagged: m.passes(margin),
        q_a,
        q_t,
        decided_ns: now_ns,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct BarAcc {
    has_px: bool,
    h: i64,
    l: i64,
    c: i64,
    c_ns: i64,
    vol: u64,
    dollars: f64,
    n: u32,
}

impl BarAcc {
    fn bar(&self) -> Bar {
        Bar {
            c_u: if self.has_px { self.c } else { 0 },
            h_u: if self.has_px { self.h } else { 0 },
            l_u: if self.has_px { self.l } else { 0 },
            vol: self.vol as f64,
            dollars: self.dollars,
            n: self.n as f64,
        }
    }
}

struct SymLive {
    feat: SymFeat,
    /// Bars still open (slot, accumulator), oldest first; at most a few.
    open: Vec<(i64, BarAcc)>,
    /// The latest-executed OHLC print `(exec ns, micro-dollars)`.
    last_ohlc: Option<(i64, i64)>,
    /// Per 5-minute bucket of the day: prints, shares, dollars (the time-of-day file).
    buckets: Vec<(u8, u64, u64, f64)>,
}

impl SymLive {
    fn new() -> Self {
        SymLive {
            feat: SymFeat::new(),
            open: Vec::new(),
            last_ohlc: None,
            buckets: Vec::new(),
        }
    }
}

struct Pending {
    event_id: String,
    alert_id: String,
    symbol: Arc<str>,
    kind: NoticeKind,
    kind_code: usize,
    tier: u8,
    a_ns: i64,
    t_ns: i64,
    slot: i64,
    ts: Option<[f32; NF]>,
}

#[derive(Clone, Copy, Debug)]
struct Day {
    date: NaiveDate,
    t0: i64,
    end: i64,
}

fn day_of(date: NaiveDate) -> Option<Day> {
    let s = sessions_for(date);
    let t0 = s.iter().find(|(id, _, _)| id.kind == SessionKind::Pre)?.1.0;
    let end = s.iter().map(|(_, _, e)| e.0).max()?;
    Some(Day { date, t0, end })
}

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    alerts_seen: usize,
    alerts_scored: usize,
    tags_requested: usize,
    tags_withheld: usize,
    candidates: usize,
    above_theta: usize,
    picks: usize,
    picks_sent: usize,
    late_prints: usize,
}

/// The two detectors' live state (module docs). Fed under the core lock with the Runners board:
/// O(1) per print; per decided slot one step of every stock's features (a few microseconds each).
#[derive(Default)]
pub struct BGoodLive {
    day: Option<Day>,
    started_at: Option<i64>,
    now: i64,
    inputs: Option<Arc<Inputs>>,
    inputs_wanted: Option<NaiveDate>,
    inputs_asked: Option<NaiveDate>,
    inputs_late: bool,
    syms: HashMap<Arc<str>, SymLive>,
    /// The last slot every stock was advanced through.
    decided: i64,
    ctx: Ctx,
    seen: HashSet<(NoticeKind, Arc<str>, i64)>,
    picker: Picker,
    pending: Vec<Pending>,
    ready: Vec<TagRequest>,
    notices: Vec<Notice>,
    tod_done: Option<TodDay>,
    counts: Counts,
    warned_missing: Option<NaiveDate>,
}

impl BGoodLive {
    pub fn new() -> Self {
        BGoodLive {
            decided: -1,
            ..Default::default()
        }
    }

    /// Watched from 04:00 ET with the day's inputs in time (module docs).
    pub fn complete(&self) -> bool {
        match (self.day, self.started_at) {
            (Some(d), Some(s)) => s <= d.t0 + START_GRACE_NS && !self.inputs_late,
            _ => false,
        }
    }

    fn inputs_today(&self) -> Option<&Inputs> {
        let d = self.day?;
        self.inputs.as_deref().filter(|i| i.date == Some(d.date))
    }

    /// Whether today's decisions may be sent: watched whole and the inputs complete.
    pub fn may_send(&self) -> bool {
        self.complete() && self.inputs_today().is_some_and(|i| i.missing().is_none())
    }

    fn see(&mut self, t: i64) {
        if self.started_at.is_none() {
            self.started_at = Some(t);
        }
        self.now = self.now.max(t);
    }

    fn roll(&mut self, date: NaiveDate) -> bool {
        if self.day.is_some_and(|d| d.date == date) {
            return true;
        }
        if self.day.is_some_and(|d| d.date > date) {
            return false;
        }
        let Some(d) = day_of(date) else { return false };
        if let Some(prev) = self.day {
            self.finish(prev);
        }
        self.day = Some(d);
        self.syms.clear();
        self.decided = -1;
        self.ctx = Ctx::default();
        self.seen.clear();
        self.picker = Picker::default();
        self.pending.clear();
        self.ready.clear();
        self.counts = Counts::default();
        self.inputs_late = false;
        true
    }

    /// The day summary, and its time-of-day file when the day was watched whole.
    fn finish(&mut self, prev: Day) {
        let c = self.counts;
        tracing::info!(
            day = %prev.date,
            alerts_seen = c.alerts_seen, alerts_scored = c.alerts_scored,
            tags_requested = c.tags_requested, tags_withheld = c.tags_withheld,
            candidates = c.candidates, above_theta = c.above_theta,
            picks = c.picks, picks_sent = c.picks_sent, late_prints = c.late_prints,
            watched_whole_day = self.complete(),
            "b-good: day summary"
        );
        if !self.complete() {
            return;
        }
        let mut t = TodDay {
            date: Some(prev.date),
            ..Default::default()
        };
        for (k, s) in &self.syms {
            if let Some((_, px)) = s.last_ohlc
                && (50_000..=30_000_000).contains(&px)
            {
                t.present.push(k.to_string());
            }
            for (b, n, v, d) in &s.buckets {
                t.rows.push((k.to_string(), *b, *n, *v, *d as f32));
            }
        }
        t.present.sort();
        t.rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        self.tod_done = Some(t);
    }

    /// One print the Runners board counted. O(1).
    pub fn apply_trade(&mut self, t: &Trade, counted: Counted) {
        let ts = t.event_ts.0;
        self.see(ts);
        let Some(s) = t.session else { return };
        if s.kind == SessionKind::Overnight || !(counted.ohlc || counted.volume) {
            return;
        }
        if !self.roll(s.date) {
            return;
        }
        let d = self.day.expect("rolled");
        if ts < d.t0 {
            return;
        }
        let slot = (ts - d.t0) / SLOT_NS;
        if slot >= NSLOT {
            return;
        }
        if slot <= self.decided {
            self.counts.late_prints += 1;
            return;
        }
        if self.inputs_today().is_some_and(|i| i.excluded(&t.symbol)) {
            return;
        }
        let key: Arc<str> = match self.syms.get_key_value(t.symbol.as_str()) {
            Some((k, _)) => k.clone(),
            None => {
                let k: Arc<str> = Arc::from(t.symbol.as_str());
                self.syms.insert(k.clone(), SymLive::new());
                k
            }
        };
        let sym = self.syms.get_mut(&key).expect("inserted");
        let i = match sym.open.iter().position(|(s, _)| *s == slot) {
            Some(i) => i,
            None => {
                sym.open.push((slot, BarAcc::default()));
                sym.open.sort_by_key(|(s, _)| *s);
                sym.open
                    .iter()
                    .position(|(s, _)| *s == slot)
                    .expect("pushed")
            }
        };
        let b = &mut sym.open[i].1;
        let px = t.price.micros();
        if counted.ohlc {
            if !b.has_px {
                b.has_px = true;
                b.h = px;
                b.l = px;
                b.c = px;
                b.c_ns = ts;
            } else {
                b.h = b.h.max(px);
                b.l = b.l.min(px);
                if ts >= b.c_ns {
                    b.c = px;
                    b.c_ns = ts;
                }
            }
            if sym.last_ohlc.is_none_or(|(n, _)| ts >= n) {
                sym.last_ohlc = Some((ts, px));
            }
        }
        if counted.volume {
            b.vol += t.size;
            b.dollars += px as f64 / 1e6 * t.size as f64;
            b.n += 1;
        }
    }

    /// A Runners-pipeline notice the market is offering (every source: the board, the MP5 cues,
    /// the HALT add-on). A B0-kind one inside `universe-v1` at its decision instant becomes a row
    /// of the day's alert context and a b-good-1 tag due 30 s later.
    pub fn record_alert(&mut self, n: &Notice) {
        let Some(kc) = logic::kind_code(n.kind.as_str()) else {
            return;
        };
        let Some(d) = self.day else { return };
        let a = n.at_ns.max(self.now);
        self.counts.alerts_seen += 1;
        if a < d.t0 || a >= d.t0 + NSLOT * SLOT_NS {
            return;
        }
        if self.inputs_today().is_some_and(|i| i.excluded(&n.symbol)) {
            return;
        }
        let Some((key, sym)) = self.syms.get_key_value(n.symbol.as_str()) else {
            return;
        };
        let last = sym.last_ohlc.filter(|(ns, _)| *ns < a);
        let Some(px) = logic::universe_ok(last, a) else {
            return;
        };
        let key = key.clone();
        if !self.seen.insert((n.kind, key.clone(), a)) {
            return;
        }
        self.counts.alerts_scored += 1;
        self.ctx.record(&key, a);
        if self.pending.len() >= MAX_PENDING {
            return;
        }
        let t_ns = a + logic::TAG_DELAY_NS;
        self.pending.push(Pending {
            event_id: n.event_id(),
            alert_id: n.alert_id.clone(),
            symbol: key,
            kind: n.kind,
            kind_code: kc,
            tier: logic::price_tier(px),
            a_ns: a,
            t_ns,
            slot: logic::slot_for_ns(t_ns, d.t0).min(NSLOT - 1),
            ts: None,
        });
    }

    /// The clock reached `now`: every slot that ended 2 s ago or more is decided for every stock,
    /// the minute-end ones screened and scored for picks; tags whose scoring instant has come are
    /// handed over ([`Self::take_tag_requests`]).
    pub fn tick(&mut self, now: i64) {
        self.see(now);
        let date = trading_date(UnixNanos(now));
        if !self.roll(date) {
            return;
        }
        let d = self.day.expect("rolled");
        if self.inputs_today().is_none()
            && self.inputs_asked != Some(d.date)
            && now >= d.t0 - FACTS_LEAD_NS
        {
            self.inputs_asked = Some(d.date);
            self.inputs_wanted = Some(d.date);
        }
        let target = logic::slot_for_ns(now, d.t0).min(NSLOT - 1);
        while self.decided < target {
            let s = self.decided + 1;
            if self.inputs_today().is_none() && !self.inputs_late && s >= 0 {
                self.inputs_late = true;
                tracing::warn!(day = %d.date, "b-good: a slot was due before the day's inputs were loaded; nothing is sent today");
            }
            self.decide_slot(s, d);
            self.decided = s;
        }
        let may = self.may_send();
        if !may
            && self.warned_missing != Some(d.date)
            && self.inputs_today().is_some()
            && now >= d.t0
        {
            self.warned_missing = Some(d.date);
            let why = self
                .inputs_today()
                .and_then(Inputs::missing)
                .unwrap_or_else(|| "the day was not watched from 04:00 ET".into());
            tracing::warn!(day = %d.date, why, "b-good: deciding but sending nothing today");
        }
        let mut keep = Vec::with_capacity(self.pending.len());
        for p in std::mem::take(&mut self.pending) {
            match p.ts {
                Some(ts) if now >= p.t_ns => {
                    if may {
                        self.counts.tags_requested += 1;
                        self.ready.push(TagRequest {
                            event_id: p.event_id,
                            alert_id: p.alert_id,
                            symbol: p.symbol.to_string(),
                            kind: p.kind,
                            kind_code: p.kind_code,
                            tier: p.tier,
                            a_ns: p.a_ns,
                            t_ns: p.t_ns,
                            ctx: self.ctx.of(&p.symbol, p.a_ns),
                            ts,
                        });
                    } else {
                        self.counts.tags_withheld += 1;
                    }
                }
                _ => keep.push(p),
            }
        }
        self.pending = keep;
    }

    fn decide_slot(&mut self, s: i64, d: Day) {
        let inputs = self.inputs.clone().filter(|i| i.date == Some(d.date));
        let last_bar = (d.end - d.t0) / SLOT_NS - 1;
        let minute = s % 4 == 3 && s >= 19 && s <= logic::U2_LAST_SLOT.min(last_bar);
        let m2 = model2();
        let nan_facts = DayFacts::default();
        let mut cands: Vec<(Arc<str>, f32, f64)> = Vec::new();
        for (k, sym) in self.syms.iter_mut() {
            let bar = match sym.open.first() {
                Some((bs, _)) if *bs == s => Some(sym.open.remove(0).1),
                _ => None,
            };
            if let Some(b) = &bar
                && b.vol > 0
            {
                let bk = (s / 20) as u8;
                match sym.buckets.last_mut() {
                    Some(x) if x.0 == bk => {
                        x.1 += b.n as u64;
                        x.2 += b.vol;
                        x.3 += b.dollars;
                    }
                    _ => sym.buckets.push((bk, b.n as u64, b.vol, b.dollars)),
                }
            }
            sym.feat.advance(s, bar.map(|b| b.bar()).as_ref());
            if !minute || !logic::screen(&sym.feat, last_bar) {
                continue;
            }
            let Some(inp) = inputs.as_deref() else {
                continue;
            };
            self.counts.candidates += 1;
            let f = sym.feat.features(
                inp.facts.get(&**k).unwrap_or(&nan_facts),
                Some(inp.tod.get(&**k)),
            );
            let m = m2.margin(&f);
            if m2.passes(m) {
                cands.push((k.clone(), m, sym.feat.px() as f64));
            }
        }
        // Tags due at this slot take its features now (the state is exactly at `s`).
        for p in self
            .pending
            .iter_mut()
            .filter(|p| p.slot == s && p.ts.is_none())
        {
            let f = match (self.syms.get(&p.symbol), inputs.as_deref()) {
                (Some(sym), Some(inp)) => sym.feat.features(
                    inp.facts.get(&*p.symbol).unwrap_or(&nan_facts),
                    Some(inp.tod.get(&*p.symbol)),
                ),
                _ => [f32::NAN; NF],
            };
            p.ts = Some(f);
        }
        if cands.is_empty() {
            return;
        }
        cands.sort_by(|a, b| a.0.cmp(&b.0));
        let ns = logic::cand_ns(d.t0, s);
        let may = self.may_send();
        for (k, m, px) in cands {
            self.counts.above_theta += 1;
            if self.picker.offer(&k, ns, self.ctx.alerts_of(&k)).is_err() {
                continue;
            }
            self.counts.picks += 1;
            let detail = BGoodNotice {
                margin: m,
                prob: XgbModel::prob(m),
                nth: self.picker.taken() as u32,
                version: version(m2),
                emitted_ns: self.now,
            };
            tracing::info!(symbol = %k, t_ns = ns, price = px, margin = m, nth = detail.nth, sent = may, "b-good-2: pick");
            if !may || self.notices.len() >= MAX_NOTICES {
                continue;
            }
            self.counts.picks_sent += 1;
            self.notices.push(Notice {
                spread_bp: None,
                kind: NoticeKind::BGood,
                alert_id: format!("{}-{k}-bg-{}", d.date, ns.div_euclid(logic::SEC)),
                symbol: k.to_string(),
                at_ns: ns,
                price: px,
                from_flag_pct: None,
                reasons: Vec::new(),
                since_start_min: None,
                exec_ns: None,
                detail: Some(NoticeDetail::BGood(detail)),
            });
        }
    }

    /// The picks decided since the last call.
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// The tags whose scoring instant has come (the confidence stage fetches their quotes).
    pub fn take_tag_requests(&mut self) -> Vec<TagRequest> {
        std::mem::take(&mut self.ready)
    }

    /// The date whose inputs the market should load now, once ([`load_inputs`]).
    pub fn take_inputs_request(&mut self) -> Option<NaiveDate> {
        self.inputs_wanted.take()
    }

    /// A finished day's time-of-day file to write ([`write_tod`]), once.
    pub fn take_tod_day(&mut self) -> Option<TodDay> {
        self.tod_done.take()
    }

    pub fn set_inputs(&mut self, inputs: Inputs) {
        tracing::info!(
            date = ?inputs.date, symbols = inputs.facts.len(), raw_dates = inputs.raw_dates,
            tod_sessions = inputs.tod_sessions, missing = ?inputs.missing(),
            "b-good: inputs read"
        );
        self.inputs = Some(Arc::new(inputs));
    }

    /// The day so far, for the board's cost line.
    pub fn cost(&self) -> Value {
        let c = self.counts;
        json!({
            "stocks": self.syms.len(), "decided_slot": self.decided,
            "alerts_scored": c.alerts_scored, "tags_requested": c.tags_requested,
            "tags_withheld": c.tags_withheld, "candidates": c.candidates,
            "above_theta": c.above_theta, "picks": c.picks, "picks_sent": c.picks_sent,
            "late": c.late_prints, "may_send": self.may_send(),
        })
    }
}

#[cfg(test)]
mod tests;
