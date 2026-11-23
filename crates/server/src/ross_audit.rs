//! Ross Panel completeness audit (owner, 2026-11-22): over one recorded day, which symbols passed
//! price + gain + RVOL at some decision, which reached the panel, and which pillar kept the rest
//! off. Operational completeness only: it reads the tape and the evidence archive, never a trade
//! outcome or a research label.
//!
//! An `#[ignore]` test driven by environment variables, run against a **local copy** of the
//! archive (never the production volume, never the production binary):
//!
//! ```text
//! ROSS_AUDIT_ROOT=C:\swt\ross-audit\archive ROSS_AUDIT_DAY=2026-11-20 ROSS_AUDIT_POLICY=v4 \
//! ROSS_AUDIT_OUT=C:\swt\ross-audit\v4.json \
//!   cargo test --release -p stockwatcher-server --lib ross_audit -- --ignored --nocapture
//! ```
//!
//! How it differs from the live run, so the numbers are read correctly:
//! - Candidates are pre-filtered from the tape (max in-band price at least 8 % over the previous
//!   close, day volume at least 3x the prior 30 raw daily volumes' mean): a superset of what the
//!   strict pillars can pass, so the filter drops nobody the engine could have passed.
//! - Prints are fed in recorded arrival order with the conditions policy's own verdict
//!   (`Counted::from_effect`), not the chart engine's tape verdict; corrections and cancels are
//!   not applied. Both can move RVOL slightly; neither touches float or news.
//! - Evidence comes through the server's own [`EvidenceLoader`] in replay mode, refreshed every
//!   15 s of run clock exactly as the live check is, so every record joins at its own
//!   `observed_at` (causal).
//! - No restarts, no restore: one continuous engine from the first print.
//! - **The archive is not replay-faithful for daily bars.** Every nightly maintain re-fetches the
//!   prior sessions' raw bars and appends its retrieval to each file, and the split snapshots of
//!   past nights are rotated away, so a later copy stamps every baseline and previous close with
//!   a retrieval after the day and the replay sees none (gain and RVOL UNKNOWN all day). The
//!   2026-11-20 audit copy had each raw file's fetches cut to those before 04:00 ET and the next
//!   night's split set renamed to the night before: the completed sessions' numbers are the same,
//!   only their visibility time is restored. Float and news records are append-only and causal
//!   as stored.

use crate::context::ContextBoard;
use crate::evidence::{EvidenceLoader, EvidenceLog};
use crate::quotes::Counted;
use chrono::{DateTime, NaiveDate};
use market_archive::{Archive, ArchiveConfig, PrevCloses};
use market_events::{MarketEvent, Price, UnixNanos};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use stock_context::{PillarId, Policy, Tri};

const SEC: i64 = 1_000_000_000;

#[derive(Default, Serialize)]
struct SymbolAudit {
    first_ppr: Option<i64>,
    ppr_decisions: u32,
    member: bool,
    /// Float and news (state, reason) seen at the price+gain+RVOL decisions, with counts.
    float: BTreeMap<String, u32>,
    news: BTreeMap<String, u32>,
    float_ever_pass: bool,
    news_ever_pass: bool,
    both_pass: bool,
    max_gain_bp: i64,
    max_rvol_milli: i64,
}

fn mean30(bars: &[serde_json::Value], sym: &str) -> Option<f64> {
    let v: Vec<f64> = bars
        .iter()
        .filter_map(|b| b["bars"][sym]["v"].as_f64())
        .collect();
    (v.len() >= 10).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

#[test]
#[ignore]
fn ross_completeness_audit() {
    let root = std::env::var("ROSS_AUDIT_ROOT").expect("ROSS_AUDIT_ROOT");
    let day: NaiveDate = std::env::var("ROSS_AUDIT_DAY")
        .expect("ROSS_AUDIT_DAY")
        .parse()
        .unwrap();
    let policy = match std::env::var("ROSS_AUDIT_POLICY").as_deref() {
        Ok("v4") => Policy::v4(),
        _ => Policy::v5(),
    };
    let out = std::env::var("ROSS_AUDIT_OUT").expect("ROSS_AUDIT_OUT");
    let archive = Archive::new(ArchiveConfig {
        root: root.clone().into(),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let closes = {
        let m = archive.read_daily_closes().unwrap().unwrap_or_default();
        PrevCloses::new(&m).0
    };
    // The prior 30 raw daily bar files.
    let mut files: Vec<_> = std::fs::read_dir(std::path::Path::new(&root).join("daily-bars/raw"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".json") && n.as_str() < format!("{day}.json").as_str())
        .collect();
    files.sort();
    let files = &files[files.len().saturating_sub(30)..];
    let bars: Vec<serde_json::Value> = files
        .iter()
        .map(|n| {
            serde_json::from_slice(
                &std::fs::read(std::path::Path::new(&root).join("daily-bars/raw").join(n)).unwrap(),
            )
            .unwrap()
        })
        .collect();

    // Pass 1: the candidate superset.
    let syms: Vec<String> = std::fs::read_dir(archive.day_dir(day))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_suffix(".swa")
                .map(String::from)
        })
        .collect();
    let (lo, hi) = (90_000i64, 25_500_000i64);
    let mut cands: Vec<(String, Vec<MarketEvent>)> = Vec::new();
    for s in &syms {
        let Ok(r) = archive.reader(day, s) else {
            continue;
        };
        let events: Vec<MarketEvent> = r.filter_map(|e| e.ok()).map(|v| v.event).collect();
        let (mut max_p, mut vol) = (0i64, 0u64);
        for e in &events {
            if let MarketEvent::Trade(t) = e {
                let c = Counted::from_effect(t);
                let p = t.price.micros();
                if c.ohlc && (lo..=hi).contains(&p) {
                    max_p = max_p.max(p);
                }
                if c.volume {
                    vol += t.size;
                }
            }
        }
        let Some((_, pc)) = closes.before_raw(s, day) else {
            continue;
        };
        let pc = pc.micros();
        let Some(m) = mean30(&bars, s) else {
            continue;
        };
        if pc > 0 && max_p * 100 >= pc * 108 && vol as f64 >= 3.0 * m {
            cands.push((s.clone(), events));
        }
    }
    eprintln!("{} symbols, {} candidates", syms.len(), cands.len());

    // Pass 2: one engine, the candidates' prints merged in arrival order.
    let mut merged: Vec<(i64, MarketEvent)> = Vec::new();
    for (_, ev) in cands.iter_mut() {
        for e in ev.drain(..) {
            if let MarketEvent::Trade(t) = &e {
                let at = t.receive_ts.unwrap_or(t.event_ts).0;
                merged.push((at, e));
            }
        }
    }
    merged.sort_by_key(|(at, _)| *at);
    let names: BTreeSet<String> = cands.iter().map(|(s, _)| s.clone()).collect();
    let mut cx = ContextBoard::new(
        false,
        Some(day),
        Price::from_cents(10),
        Price::from_cents(2500),
    )
    .expect("band")
    .with_pillar_policy(policy.clone());
    let mut loader = EvidenceLoader::new(archive.clone(), Price::from_cents(2500), false);
    let log = Mutex::new(EvidenceLog::default());
    let refresh = |loader: &mut EvidenceLoader, cx: &mut ContextBoard, now: i64| {
        let rep = loader.refresh(day, DateTime::from_timestamp_nanos(now), &log);
        if !rep.errors.is_empty() {
            eprintln!(
                "refresh at {now}: floats {} outstanding {} news {} coverage {} closes {} baselines {} errors {:?}",
                rep.floats,
                rep.outstanding,
                rep.news,
                rep.coverage,
                rep.closes,
                rep.baselines,
                rep.errors.iter().take(3).collect::<Vec<_>>()
            );
        }
        if let Some(b) = log.lock().unwrap().since(cx.evidence_cursor()) {
            cx.add_evidence(b);
        }
    };
    let mut audit: BTreeMap<String, SymbolAudit> = BTreeMap::new();
    let observe = |cx: &ContextBoard, audit: &mut BTreeMap<String, SymbolAudit>| {
        for s in &names {
            let Some(d) = cx.decision(s) else { continue };
            let a = audit.entry(s.clone()).or_default();
            if d.membership.active().is_some() {
                a.member = true;
            }
            let p = |id: PillarId| d.pillars.iter().find(|x| x.pillar == id).unwrap();
            if [PillarId::Price, PillarId::Gain, PillarId::Rvol]
                .iter()
                .all(|&id| p(id).state == Tri::Pass)
            {
                a.first_ppr.get_or_insert(d.cutoff);
                a.ppr_decisions += 1;
                let (f, n) = (p(PillarId::Float), p(PillarId::News));
                *a.float
                    .entry(format!("{:?}:{}", f.state, f.reason))
                    .or_default() += 1;
                *a.news
                    .entry(format!("{:?}:{}", n.state, n.reason))
                    .or_default() += 1;
                a.float_ever_pass |= f.state == Tri::Pass;
                a.news_ever_pass |= n.state == Tri::Pass;
                a.both_pass |= f.state == Tri::Pass && n.state == Tri::Pass;
                a.max_gain_bp = a.max_gain_bp.max(p(PillarId::Gain).value.unwrap_or(0));
                a.max_rvol_milli = a.max_rvol_milli.max(p(PillarId::Rvol).value.unwrap_or(0));
            }
        }
    };
    let mut clock = merged.first().map(|(a, _)| a - 60 * SEC).unwrap_or(0);
    let mut next_refresh = clock;
    let mut next_observe = clock;
    let closes_ref = &closes;
    for (at, e) in &merged {
        while clock < *at {
            clock = (clock.div_euclid(SEC) + 1) * SEC;
            if clock >= next_refresh {
                refresh(&mut loader, &mut cx, clock);
                next_refresh = clock + 15 * SEC;
            }
            cx.advance(UnixNanos(clock));
            if clock >= next_observe {
                observe(&cx, &mut audit);
                next_observe = clock + 15 * SEC;
            }
        }
        if let MarketEvent::Trade(t) = e {
            cx.apply_trade(t, Counted::from_effect(t), closes_ref);
        }
    }
    let end = clock + 600 * SEC;
    while clock < end {
        clock += SEC;
        if clock >= next_refresh {
            refresh(&mut loader, &mut cx, clock);
            next_refresh = clock + 15 * SEC;
        }
        cx.advance(UnixNanos(clock));
        if clock >= next_observe {
            observe(&cx, &mut audit);
            next_observe = clock + 15 * SEC;
        }
    }
    // Why the candidates did not pass, at their last decision (a check on the harness itself:
    // a run where nothing passes is more likely missing evidence than a quiet day).
    let mut last: BTreeMap<String, u32> = BTreeMap::new();
    for s in &names {
        if let Some(d) = cx.decision(s) {
            for p in &d.pillars {
                *last
                    .entry(format!("{}:{:?}:{}", p.pillar.as_str(), p.state, p.reason))
                    .or_default() += 1;
            }
        }
    }
    eprintln!("last decisions: {last:#?}");
    audit.retain(|_, a| a.ppr_decisions > 0 || a.member);
    let ppr = audit.values().filter(|a| a.ppr_decisions > 0).count();
    let members = audit.values().filter(|a| a.member).count();
    eprintln!(
        "policy {}: {ppr} passed price+gain+RVOL, {members} reached the panel",
        policy.tag()
    );
    let body = serde_json::json!({
        "policy": policy.tag(),
        "day": day.to_string(),
        "symbols": syms.len(),
        "candidates": names.len(),
        "ppr": ppr,
        "members": members,
        "last_states": last,
        "rows": audit,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&body).unwrap()).unwrap();
}
