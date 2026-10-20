//! The causal per-symbol context engine.
//!
//! # Timing
//!
//! Decisions sit on a grid of completed bars (`grid.bar_secs`, 15 s in v1). The bar ending at `b`
//! is decided at the barrier `tau = b + settle` (2 s): it sees exactly the prints that executed
//! before `b` AND were admitted (applied) at or before `tau`. A print that executed before `b` but
//! arrived after `tau` is late: it joins the next decision, never the one already made. Evidence
//! records join as of `tau` (`EvidenceStore`). The engine is driven by the run's own clock
//! (`advance_to`, called with each item's admission time before the item applies), never by wall
//! time, so a linear replay and a rebuild after a seek produce the same decisions, and processing
//! delay can never widen what a decision admitted.
//!
//! # Output
//!
//! Every barrier evaluates the symbols that traded on the trading date and need it (new prints,
//! new evidence, or a time-based flip such as a price going stale). A symbol emits a [`Decision`]
//! only when its content changed; decisions are owned by the caller (append-only log), the engine
//! keeps only the latest per symbol. First-seen times live on the per-day [`Timeline`] and are
//! never rewritten.

use crate::evaluate::{self, Coverage, NewsSummary, PriceView, RvolDiagnostics, VolumeView};
use crate::evidence::{
    DailyBaseline, EvidenceStore, FloatObservation, InstrumentId, IntradayBaseline, NewsCoverage,
    NewsItem, PrevClose,
};
use crate::pillar::{Counts, PillarId, PillarResult, Qualification, Tri, aggregate, price_pillar};
use crate::policy::Policy;
use chrono::{Days, NaiveDate};
use market_events::session::{SessionKind, classify, et, trading_date};
use market_events::{Trade, UnixNanos};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, VecDeque};

const SEC: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;

/// What kind of data a run decides on: recorded live arrival order, a reconstruction from
/// history without arrival times (no latency claims), or a synthetic fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fidelity {
    RecordedLive,
    ReconstructedHistory,
    SyntheticFixture,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunInfo {
    pub run_id: String,
    /// `live` or `replay`: runs never share state.
    pub source: String,
    pub fidelity: Fidelity,
    /// How completely this run collects today's prints (the RVOL numerator).
    pub numerator_coverage: Coverage,
}

/// The chart engine's verdict on a print, as the run's conditions policy decided it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub ohlc: bool,
    pub volume: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Timeline {
    /// First decision with a fresh price inside the discovery band.
    pub first_eligible: Option<i64>,
    /// First decision each pillar passed at.
    pub first_pass: BTreeMap<PillarId, i64>,
    pub first_qualified: Option<i64>,
    pub qualified_now: bool,
    /// Qualified at some decision earlier today, whatever it is now.
    pub qualified_earlier: bool,
    pub lost_at: Option<i64>,
    pub requalified: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VolumeFacts {
    pub shares: u64,
    /// Dollar volume, whole dollars (truncated).
    pub dollars: u64,
    pub trades: u64,
    pub coverage: Coverage,
}

/// One frozen decision. Times are UTC nanoseconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub seq: u64,
    pub decision_id: String,
    pub run_id: String,
    pub source: String,
    pub fidelity: Fidelity,
    /// `pillars-v1@<hash>`.
    pub policy: String,
    pub symbol: String,
    pub instrument: InstrumentId,
    pub trading_date: NaiveDate,
    /// End of the completed bar the decision covers (volume endpoint).
    pub bar_end: i64,
    /// The data barrier: nothing admitted after it is in this decision.
    pub cutoff: i64,
    pub pillars: Vec<PillarResult>,
    pub qualification: Qualification,
    pub counts: Counts,
    /// Price fresh and inside the discovery band (research default $0.15–$25).
    pub in_discovery_band: Option<bool>,
    /// Gain against the close as stored, whatever its basis: a named diagnostic only.
    pub gain_vs_stored_close_bp: Option<i64>,
    pub volume: VolumeFacts,
    pub rvol: RvolDiagnostics,
    pub news: NewsSummary,
    pub timeline: Timeline,
    /// Which parts differ from this symbol's previous decision.
    pub changed: Vec<&'static str>,
}

struct Pending {
    exec: i64,
    admit: i64,
    price: i64,
    size: u64,
    verdict: Verdict,
    session: Option<(NaiveDate, SessionKind)>,
}

struct Sym {
    name: String,
    inst: InstrumentId,
    date: Option<NaiveDate>,
    pending: Vec<Pending>,
    /// Had a counted print on `date`: only those symbols emit.
    active: bool,
    cum: u64,
    dollars_micros: u128,
    trades: u64,
    /// (execution, price) of the latest OHLC print admitted.
    last_price: Option<(i64, i64)>,
    /// (bar end, cumulative volume) history for the recent window.
    cums: VecDeque<(i64, u64)>,
    last: Option<Decision>,
    timeline: Timeline,
    recheck_at: i64,
    dirty: bool,
}

pub struct Engine {
    policy: Policy,
    run: RunInfo,
    evidence: EvidenceStore,
    syms: BTreeMap<String, Sym>,
    by_inst: HashMap<InstrumentId, String>,
    next_bar: Option<i64>,
    seq: u64,
}

fn ceil_to(t: i64, step: i64) -> i64 {
    t.div_euclid(step) * step + if t.rem_euclid(step) == 0 { 0 } else { step }
}

impl Engine {
    pub fn new(policy: Policy, run: RunInfo) -> Self {
        Engine {
            policy,
            run,
            evidence: EvidenceStore::default(),
            syms: BTreeMap::new(),
            by_inst: HashMap::new(),
            next_bar: None,
            seq: 0,
        }
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    fn bar(&self) -> i64 {
        self.policy.body.grid.bar_secs * SEC
    }

    fn settle(&self) -> i64 {
        self.policy.body.grid.settle_ms * MS
    }

    fn mark(&mut self, inst: &InstrumentId) {
        if let Some(name) = self.by_inst.get(inst)
            && let Some(s) = self.syms.get_mut(name)
        {
            s.dirty = true;
        }
    }

    pub fn add_prev_close(&mut self, r: PrevClose) {
        let i = r.instrument.clone();
        self.evidence.add_prev_close(r);
        self.mark(&i);
    }
    pub fn add_daily(&mut self, r: DailyBaseline) {
        let i = r.instrument.clone();
        self.evidence.add_daily(r);
        self.mark(&i);
    }
    pub fn add_intraday(&mut self, r: IntradayBaseline) {
        let i = r.instrument.clone();
        self.evidence.add_intraday(r);
        self.mark(&i);
    }
    pub fn add_float(&mut self, r: FloatObservation) {
        let i = r.instrument.clone();
        self.evidence.add_float(r);
        self.mark(&i);
    }
    pub fn add_news(&mut self, r: NewsItem) {
        let i = r.instrument.clone();
        self.evidence.add_news(r);
        self.mark(&i);
    }
    pub fn add_coverage(&mut self, r: NewsCoverage) {
        let i = r.instrument.clone();
        self.evidence.add_coverage(r);
        self.mark(&i);
    }

    /// Applies one print admitted at `admit` (the run clock when it applied). Decides every
    /// barrier passed before it first, so the print can only reach later decisions.
    pub fn apply(&mut self, t: &Trade, verdict: Verdict, admit: UnixNanos) -> Vec<Decision> {
        let out = self.advance_to(admit);
        let name = t.symbol.clone();
        let sym = self.syms.entry(name.clone()).or_insert_with(|| {
            let inst = InstrumentId::symbol(&name);
            Sym {
                name: name.clone(),
                inst,
                date: None,
                pending: Vec::new(),
                active: false,
                cum: 0,
                dollars_micros: 0,
                trades: 0,
                last_price: None,
                cums: VecDeque::new(),
                last: None,
                timeline: Timeline::default(),
                recheck_at: i64::MAX,
                dirty: false,
            }
        });
        self.by_inst.entry(sym.inst.clone()).or_insert(name);
        sym.pending.push(Pending {
            exec: t.event_ts.nanos(),
            admit: admit.nanos(),
            price: t.price.micros(),
            size: t.size,
            verdict,
            session: t.session.map(|s| (s.date, s.kind)),
        });
        out
    }

    /// Decides every barrier `b + settle <= now` not yet decided.
    pub fn advance_to(&mut self, now: UnixNanos) -> Vec<Decision> {
        let now = now.nanos();
        let (bar, settle) = (self.bar(), self.settle());
        let mut b = *self.next_bar.get_or_insert(ceil_to(now, bar));
        // The first barrier `now` cannot decide yet.
        let undecidable = ceil_to(now - settle + 1, bar);
        let mut out = Vec::new();
        while b + settle <= now {
            // Skip barriers where nothing can change (no due print, flip or evidence): the
            // decisions are the same as deciding each of them, at a fraction of the cost.
            let due = self.next_due(b, bar, settle);
            if due > b {
                b = due.min(undecidable);
                continue;
            }
            self.decide(b, b + settle, &mut out);
            b += bar;
        }
        self.next_bar = Some(b);
        out
    }

    /// The earliest barrier at or after `b` where some symbol needs a decision (`i64::MAX` when
    /// none), aligned to the grid.
    fn next_due(&self, b: i64, bar: i64, settle: i64) -> i64 {
        let mut due = i64::MAX;
        for s in self.syms.values() {
            if s.dirty {
                return b;
            }
            for p in &s.pending {
                // Drained at the first barrier past its execution that it was admitted by.
                let by_exec = ceil_to(p.exec + 1, bar);
                let by_admit = ceil_to(p.admit - settle, bar);
                due = due.min(by_exec.max(by_admit));
            }
            if s.recheck_at != i64::MAX {
                // A flip at time t shows at the first barrier whose bar end is past it.
                due = due.min(ceil_to(s.recheck_at, bar));
            }
        }
        due.max(b)
    }

    fn decide(&mut self, b: i64, tau: i64, out: &mut Vec<Decision>) {
        let date = trading_date(UnixNanos(b));
        let names: Vec<String> = self.syms.keys().cloned().collect();
        for name in names {
            let due = {
                let s = &self.syms[&name];
                s.dirty
                    || s.date != Some(date)
                    || b >= s.recheck_at
                    || s.pending.iter().any(|p| p.exec < b && p.admit <= tau)
            };
            if due && let Some(d) = self.evaluate(&name, date, b, tau) {
                out.push(d);
            }
        }
    }

    fn evaluate(&mut self, name: &str, date: NaiveDate, b: i64, tau: i64) -> Option<Decision> {
        let policy = &self.policy;
        let ps = &policy.body.price_staleness;
        let w = policy.body.diagnostics.recent_window_secs * SEC;
        let s = self.syms.get_mut(name).expect("known symbol");
        s.dirty = false;
        if s.date != Some(date) {
            // A new trading date: its own volume window (04:00 →), price and first-seen times.
            s.date = Some(date);
            s.active = false;
            s.cum = 0;
            s.dollars_micros = 0;
            s.trades = 0;
            s.last_price = None;
            s.cums.clear();
            s.timeline = Timeline::default();
            s.last = None;
        }
        let exclude_overnight = policy.body.volume.exclude_overnight_boats;
        let mut keep = Vec::with_capacity(s.pending.len());
        for p in s.pending.drain(..) {
            if !(p.exec < b && p.admit <= tau) {
                keep.push(p);
                continue;
            }
            let Some((d, kind)) = p.session else { continue };
            if d != date || (exclude_overnight && kind == SessionKind::Overnight) {
                continue;
            }
            if p.verdict.volume {
                s.cum += p.size;
                s.dollars_micros += p.price as u128 * p.size as u128;
                s.trades += 1;
                s.active = true;
            }
            if p.verdict.ohlc {
                s.active = true;
                if s.last_price.is_none_or(|(e, _)| p.exec >= e) {
                    s.last_price = Some((p.exec, p.price));
                }
            }
        }
        s.pending = keep;
        s.cums.push_back((b, s.cum));
        while s.cums.len() > 2 && s.cums[1].0 <= b - w {
            s.cums.pop_front();
        }
        if !s.active {
            s.recheck_at = i64::MAX;
            return None;
        }

        // Price: the latest OHLC print before the bar end, stale past the session's limit.
        let regular = classify(UnixNanos(b)).is_some_and(|x| x.kind == SessionKind::Regular);
        let limit = if regular {
            ps.regular_secs
        } else {
            ps.extended_secs
        } * SEC;
        let stale = s.last_price.is_some_and(|(e, _)| b - e > limit);
        let price = PriceView {
            micros: s.last_price.map(|(_, p)| p),
            stale,
        };
        let prof = &policy.body.reference_profile;
        let price_r = price_pillar(
            price.micros,
            stale,
            prof.price_min_micros,
            prof.price_max_micros,
        )
        .as_of(s.last_price.map(|(e, _)| e));
        let band = &policy.body.discovery_band;
        let in_band = match price.micros {
            Some(p) if !stale => Some(p >= band.min_micros && p <= band.max_micros),
            _ => None,
        };

        // Volume endpoints are the bar end; the recent window needs a known cumulative at b - w.
        let day_start = et(date, 4, 0, 0, 0).map(|t| t.nanos()).unwrap_or(b);
        let recent = if b - w <= day_start {
            Some(s.cum)
        } else {
            s.cums
                .iter()
                .rev()
                .find(|(t, _)| *t <= b - w)
                .map(|(_, c)| s.cum - c)
        };
        let vol = VolumeView {
            cum: s.cum,
            recent,
            offset: (b - day_start) / SEC,
            coverage: self.run.numerator_coverage,
        };

        let ev = &self.evidence;
        let g = evaluate::gain(policy, ev, &s.inst, date, tau, price);
        let (rvol_r, rvol_d) = evaluate::rvol(policy, ev, &s.inst, date, tau, vol);
        let float_r = evaluate::float(policy, ev, &s.inst, tau);
        let (news_r, news_s) = evaluate::news(policy, ev, &s.inst, tau);
        let pillars = vec![price_r, g.pillar, rvol_r, float_r, news_r];
        let (qualification, counts) = aggregate(pillars.iter().map(|p| p.state));

        // When could this symbol's result change with no new print? Stale price, stale float,
        // news ageing out, evidence becoming visible, or the next trading date (20:00 ET).
        let mut recheck = i64::MAX;
        if let Some((e, _)) = s.last_price {
            for l in [ps.regular_secs, ps.extended_secs] {
                let t = e + l * SEC + 1;
                if t > b {
                    recheck = recheck.min(t);
                }
            }
        }
        if let Some(f) = ev.float(&s.inst, tau) {
            recheck = recheck.min(f.observed_at + policy.body.float.stale_after_secs * SEC + 1);
        }
        let fresh = prof.news_freshness_secs * SEC;
        for n in ev.news(&s.inst, tau) {
            if let Some(p) = n.published_at
                && p + fresh >= tau
            {
                recheck = recheck.min(p + fresh + 1);
            }
        }
        if let Some(t) = ev.next_after(&s.inst, tau) {
            recheck = recheck.min(t);
        }
        let rollover = [date, date + Days::new(1)]
            .into_iter()
            .filter_map(|d| et(d, 20, 0, 0, 0))
            .map(|t| t.nanos())
            .find(|&t| t > b);
        if let Some(t) = rollover {
            recheck = recheck.min(t);
        }
        s.recheck_at = recheck;

        // Timeline: the cutoff is when a result became known, never the bar end.
        let tl = &mut s.timeline;
        if in_band == Some(true) && tl.first_eligible.is_none() {
            tl.first_eligible = Some(tau);
        }
        for p in &pillars {
            if p.state == Tri::Pass {
                tl.first_pass.entry(p.pillar).or_insert(tau);
            }
        }
        match (qualification == Qualification::Qualified, tl.qualified_now) {
            (true, false) => {
                if tl.first_qualified.is_none() {
                    tl.first_qualified = Some(tau);
                } else {
                    tl.requalified += 1;
                }
                tl.qualified_now = true;
                tl.qualified_earlier = true;
            }
            (false, true) => {
                tl.qualified_now = false;
                tl.lost_at = Some(tau);
            }
            _ => {}
        }

        let volume = VolumeFacts {
            shares: s.cum,
            dollars: (s.dollars_micros / 1_000_000) as u64,
            trades: s.trades,
            coverage: self.run.numerator_coverage,
        };
        let mut changed = Vec::new();
        match &s.last {
            None => changed.push("new"),
            Some(prev) => {
                for (a, p) in prev.pillars.iter().zip(&pillars) {
                    if a != p {
                        changed.push(match p.pillar {
                            PillarId::Price => "pillars.price",
                            PillarId::Gain => "pillars.gain",
                            PillarId::Rvol => "pillars.rvol",
                            PillarId::Float => "pillars.float",
                            PillarId::News => "pillars.news",
                        });
                    }
                }
                if prev.qualification != qualification {
                    changed.push("qualification");
                }
                if prev.in_discovery_band != in_band {
                    changed.push("discovery_band");
                }
                if prev.volume != volume {
                    changed.push("volume");
                }
                if prev.rvol != rvol_d {
                    changed.push("rvol_diagnostics");
                }
                if prev.news != news_s {
                    changed.push("news");
                }
                if prev.gain_vs_stored_close_bp != g.vs_stored_close_bp {
                    changed.push("gain_diagnostic");
                }
            }
        }
        if changed.is_empty() {
            return None;
        }
        self.seq += 1;
        let d = Decision {
            seq: self.seq,
            decision_id: format!("{}:{}:{}", self.run.run_id, s.name, b),
            run_id: self.run.run_id.clone(),
            source: self.run.source.clone(),
            fidelity: self.run.fidelity,
            policy: policy.tag(),
            symbol: s.name.clone(),
            instrument: s.inst.clone(),
            trading_date: date,
            bar_end: b,
            cutoff: tau,
            pillars,
            qualification,
            counts,
            in_discovery_band: in_band,
            gain_vs_stored_close_bp: g.vs_stored_close_bp,
            volume,
            rvol: rvol_d,
            news: news_s,
            timeline: s.timeline.clone(),
            changed,
        };
        s.last = Some(d.clone());
        Some(d)
    }

    /// The latest decision per symbol (what a snapshot sends), in symbol order.
    pub fn current(&self) -> impl Iterator<Item = &Decision> {
        self.syms.values().filter_map(|s| s.last.as_ref())
    }

    /// The last decision sequence number emitted.
    pub fn seq(&self) -> u64 {
        self.seq
    }
}
