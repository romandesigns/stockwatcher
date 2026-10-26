//! Aggregation (protocol sections 7–10): precision with Wilson and symbol-day cluster bootstrap
//! intervals, rates, lead times, the recall proxy, the pre-declared selection, and the overlap
//! analysis. Reads only what `run` wrote.

use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

const SEC: i64 = 1_000_000_000;

#[derive(Clone, Debug, Deserialize)]
pub struct Sig {
    pub day: String,
    pub sym: String,
    pub det: String,
    pub var: String,
    pub kind: String,
    pub t0: i64,
    pub session: String,
    pub p0: Option<i64>,
    pub in_band: bool,
    pub outcome: String,
    pub t_res: Option<i64>,
    pub t_first3: Option<i64>,
    pub mfe_bp: Option<i64>,
    pub mae_bp: Option<i64>,
    pub cue_t: Option<i64>,
    pub armed_t: Option<i64>,
}

impl Sig {
    /// Counts toward precision: pre/regular, in band, resolved or timed out.
    pub fn scored(&self) -> bool {
        self.eligible() && matches!(self.outcome.as_str(), "success" | "failure" | "timeout")
    }
    pub fn eligible(&self) -> bool {
        matches!(self.session.as_str(), "pre" | "regular") && self.in_band && self.p0.is_some()
    }
    fn key(&self) -> String {
        format!("{}:{}:{}", self.det, self.var, self.kind)
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Run {
    t_l: i64,
    t_m: i64,
    t_c: i64,
}

#[derive(Clone, Debug, Deserialize)]
struct SymDay {
    day: String,
    sym: String,
    run: Option<Run>,
}

pub fn wilson(s: u64, n: u64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let (s, n, z) = (s as f64, n as f64, 1.959_963_984_540_054_f64);
    let p = s / n;
    let d = 1.0 + z * z / n;
    let c = (p + z * z / (2.0 * n)) / d;
    let h = z * ((p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt()) / d;
    ((c - h).max(0.0), (c + h).min(1.0))
}

fn quantiles(mut v: Vec<i64>) -> Value {
    if v.is_empty() {
        return json!(null);
    }
    v.sort();
    let q = |f: f64| v[((v.len() - 1) as f64 * f).round() as usize] as f64 / SEC as f64;
    json!({"n": v.len(), "p25_s": q(0.25), "median_s": q(0.5), "p75_s": q(0.75)})
}

fn median_i(mut v: Vec<i64>) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort();
    Some(v[v.len() / 2])
}

/// Symbol-day cluster bootstrap 95 % interval of precision (fixed seed, 2000 resamples).
fn bootstrap(sigs: &[&Sig]) -> (f64, f64) {
    let mut cl: BTreeMap<(&str, &str), (u64, u64)> = BTreeMap::new();
    for s in sigs {
        let e = cl.entry((&s.day, &s.sym)).or_default();
        e.1 += 1;
        e.0 += u64::from(s.outcome == "success");
    }
    let c: Vec<(u64, u64)> = cl.into_values().collect();
    if c.is_empty() {
        return (0.0, 1.0);
    }
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rng = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut ps: Vec<f64> = (0..2000)
        .map(|_| {
            let (mut s, mut n) = (0, 0);
            for _ in 0..c.len() {
                let k = (rng() % c.len() as u64) as usize;
                s += c[k].0;
                n += c[k].1;
            }
            if n == 0 { 0.0 } else { s as f64 / n as f64 }
        })
        .collect();
    ps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (ps[50], ps[1949])
}

pub struct Data {
    pub sigs: Vec<Sig>,
    runs: Vec<SymDay>,
    pub days: Vec<String>,
    pub symdays: usize,
}

pub fn load(dir: &std::path::Path, days: &[String]) -> Data {
    let mut sigs = Vec::new();
    let mut runs = Vec::new();
    let mut present = Vec::new();
    for d in days {
        let p = dir.join(format!("signals-{d}.jsonl"));
        let Ok(text) = std::fs::read_to_string(&p) else {
            eprintln!("{d}: no signals file (missing day)");
            continue;
        };
        present.push(d.clone());
        for l in text.lines().filter(|l| !l.is_empty()) {
            sigs.push(serde_json::from_str::<Sig>(l).expect("signal row"));
        }
        let t = std::fs::read_to_string(dir.join(format!("symdays-{d}.jsonl"))).unwrap();
        for l in t.lines().filter(|l| !l.is_empty()) {
            runs.push(serde_json::from_str::<SymDay>(l).expect("symday row"));
        }
    }
    let symdays = runs.len();
    Data {
        sigs,
        runs,
        days: present,
        symdays,
    }
}

/// Every metric for one `det:var:kind`.
pub fn metrics(d: &Data, key: &str) -> Value {
    let all: Vec<&Sig> = d.sigs.iter().filter(|s| s.key() == key).collect();
    let elig: Vec<&Sig> = all.iter().copied().filter(|s| s.eligible()).collect();
    let scored: Vec<&Sig> = elig.iter().copied().filter(|s| s.scored()).collect();
    let n = scored.len() as u64;
    let cnt = |o: &str| elig.iter().filter(|s| s.outcome == o).count() as u64;
    let (succ, fail, tout, cens) = (
        cnt("success"),
        cnt("failure"),
        cnt("timeout"),
        cnt("censored"),
    );
    let (lo, hi) = wilson(succ, n);
    let (blo, bhi) = bootstrap(&scored);
    let ndays = d.days.len().max(1) as f64;
    let lead_cue: Vec<i64> = scored
        .iter()
        .filter_map(|s| s.cue_t.map(|c| s.t0 - c))
        .collect();
    let lead_arm: Vec<i64> = scored
        .iter()
        .filter_map(|s| s.armed_t.map(|c| s.t0 - c))
        .collect();
    let to3: Vec<i64> = scored
        .iter()
        .filter_map(|s| s.t_first3.map(|t| t - s.t0))
        .collect();
    let tosucc: Vec<i64> = scored
        .iter()
        .filter(|s| s.outcome == "success")
        .filter_map(|s| s.t_res.map(|t| t - s.t0))
        .collect();
    let mfe = median_i(scored.iter().filter_map(|s| s.mfe_bp).collect());
    let mae = median_i(scored.iter().filter_map(|s| s.mae_bp).collect());
    let pre = scored.iter().filter(|s| s.session == "pre").count();
    json!({
        "key": key,
        "signals_raw": all.len(),
        "dropped_session_or_band": all.len() - elig.len(),
        "scored": n, "success": succ, "failure": fail, "timeout": tout, "censored": cens,
        "precision": if n > 0 { succ as f64 / n as f64 } else { 0.0 },
        "wilson95": [lo, hi], "cluster_bootstrap95": [blo, bhi],
        "failure_rate": if n > 0 { fail as f64 / n as f64 } else { 0.0 },
        "timeout_rate": if n > 0 { tout as f64 / n as f64 } else { 0.0 },
        "per_day": n as f64 / ndays,
        "per_eligible_symbol_day": n as f64 / d.symdays.max(1) as f64,
        "symbol_days_with_signal": scored.iter().map(|s| (&s.day, &s.sym)).collect::<BTreeSet<_>>().len(),
        "premarket_share": if n > 0 { pre as f64 / n as f64 } else { 0.0 },
        "share_with_cue_before": if n > 0 { lead_cue.len() as f64 / n as f64 } else { 0.0 },
        "cue_lead": quantiles(lead_cue),
        "armed_lead": quantiles(lead_arm),
        "t0_to_first_plus3": quantiles(to3),
        "t0_to_success": quantiles(tosucc),
        "median_mfe_bp": mfe, "median_mae_bp": mae,
        "recall": recall(d, &elig),
    })
}

/// Recall proxy on the detector-independent +10 % runs.
fn recall(d: &Data, sigs: &[&Sig]) -> Value {
    let mut by: HashMap<(&str, &str), Vec<i64>> = HashMap::new();
    for s in sigs {
        by.entry((&s.day, &s.sym)).or_default().push(s.t0);
    }
    let (mut runs, mut mid, mut comp) = (0u64, 0u64, 0u64);
    let mut early = Vec::new();
    for r in &d.runs {
        let Some(run) = &r.run else { continue };
        runs += 1;
        let ts = by.get(&(r.day.as_str(), r.sym.as_str()));
        let from = run.t_l - 600 * SEC;
        let first = |until: i64| {
            ts.and_then(|v| v.iter().copied().filter(|&t| t >= from && t <= until).min())
        };
        if let Some(t) = first(run.t_m + 2 * SEC) {
            mid += 1;
            early.push(run.t_m - t);
        }
        if first(run.t_c + 2 * SEC).is_some() {
            comp += 1;
        }
    }
    json!({
        "runs": runs,
        "flagged_before_midpoint": mid,
        "share_before_midpoint": if runs > 0 { mid as f64 / runs as f64 } else { 0.0 },
        "flagged_before_completion": comp,
        "share_before_completion": if runs > 0 { comp as f64 / runs as f64 } else { 0.0 },
        "earliness_vs_midpoint": quantiles(early),
    })
}

/// One-to-one nearest matching of the two detectors' TRIGGERs within 2 min (protocol §10).
pub fn overlap(d: &Data, bull: &str, micro: &str) -> Value {
    let pick = |k: &str| -> Vec<&Sig> {
        d.sigs
            .iter()
            .filter(|s| s.key() == k && s.scored())
            .collect()
    };
    let (a, b) = (
        pick(&format!("bull:{bull}:trigger")),
        pick(&format!("micro:{micro}:trigger")),
    );
    type Group<'a> = (Vec<&'a Sig>, Vec<&'a Sig>);
    let mut groups: HashMap<(&str, &str), Group> = HashMap::new();
    for s in &a {
        groups.entry((&s.day, &s.sym)).or_default().0.push(s);
    }
    for s in &b {
        groups.entry((&s.day, &s.sym)).or_default().1.push(s);
    }
    let mut pairs: Vec<(&Sig, &Sig)> = Vec::new();
    let mut only_a = Vec::new();
    let mut only_b = Vec::new();
    for (_, (ga, gb)) in groups {
        let mut cand: Vec<(i64, usize, usize)> = Vec::new();
        for (i, x) in ga.iter().enumerate() {
            for (j, y) in gb.iter().enumerate() {
                let g = (x.t0 - y.t0).abs();
                if g <= 120 * SEC {
                    cand.push((g, i, j));
                }
            }
        }
        cand.sort();
        let (mut ua, mut ub) = (vec![false; ga.len()], vec![false; gb.len()]);
        for (_, i, j) in cand {
            if !ua[i] && !ub[j] {
                ua[i] = true;
                ub[j] = true;
                pairs.push((ga[i], gb[j]));
            }
        }
        only_a.extend(ga.iter().zip(&ua).filter(|(_, u)| !**u).map(|(s, _)| *s));
        only_b.extend(gb.iter().zip(&ub).filter(|(_, u)| !**u).map(|(s, _)| *s));
    }
    let prec = |v: &[&Sig]| {
        let n = v.len() as u64;
        let s = v.iter().filter(|x| x.outcome == "success").count() as u64;
        let (lo, hi) = wilson(s, n);
        json!({"n": n, "success": s, "precision": if n > 0 { s as f64 / n as f64 } else { 0.0 }, "wilson95": [lo, hi]})
    };
    let bull_first = pairs.iter().filter(|(x, y)| x.t0 < y.t0).count();
    let micro_first = pairs.iter().filter(|(x, y)| y.t0 < x.t0).count();
    let gaps: Vec<i64> = pairs.iter().map(|(x, y)| (x.t0 - y.t0).abs()).collect();
    let pa: Vec<&Sig> = pairs.iter().map(|p| p.0).collect();
    let pb: Vec<&Sig> = pairs.iter().map(|p| p.1).collect();
    let earlier: Vec<&Sig> = pairs
        .iter()
        .map(|(x, y)| if x.t0 <= y.t0 { *x } else { *y })
        .collect();
    let agree = pairs.iter().filter(|(x, y)| x.outcome == y.outcome).count();
    json!({
        "bull": bull, "micro": micro,
        "bull_triggers": a.len(), "micro_triggers": b.len(),
        "matched_pairs": pairs.len(),
        "share_of_bull_matched": if a.is_empty() { 0.0 } else { pairs.len() as f64 / a.len() as f64 },
        "share_of_micro_matched": if b.is_empty() { 0.0 } else { pairs.len() as f64 / b.len() as f64 },
        "bull_first": bull_first, "micro_first": micro_first,
        "same_time": pairs.len() - bull_first - micro_first,
        "abs_gap": quantiles(gaps),
        "pairs_same_outcome": agree,
        "pairs_scored_on_bull": prec(&pa), "pairs_scored_on_micro": prec(&pb),
        "pairs_scored_on_earlier": prec(&earlier),
        "bull_only": prec(&only_a), "micro_only": prec(&only_b),
    })
}

fn pre_declared_choice(d: &Data, det: &str, vars: &[String], b0: f64) -> Value {
    let floor = 5.0 * d.days.len() as f64;
    let mut rows = Vec::new();
    for v in vars {
        let m = metrics(d, &format!("{det}:{v}:trigger"));
        let n = m["scored"].as_u64().unwrap() as f64;
        let lb = m["wilson95"][0].as_f64().unwrap();
        rows.push((v.clone(), n, lb));
    }
    let ok: Vec<&(String, f64, f64)> = rows.iter().filter(|r| r.1 >= floor).collect();
    // Highest lower bound; ties: the default (index 0), then the lower id (stable order).
    let best = ok
        .iter()
        .fold(None::<&&(String, f64, f64)>, |a, r| match a {
            Some(x) if x.2 >= r.2 => Some(x),
            _ => Some(r),
        })
        .map(|r| (r.0.clone(), r.2));
    json!({
        "detector": det, "floor_scored_triggers": floor, "b0_precision": b0,
        "candidates": rows.iter().map(|r| json!({"var": r.0, "scored": r.1, "wilson_lb": r.2, "meets_floor": r.1 >= floor})).collect::<Vec<_>>(),
        "chosen": best.as_ref().map(|b| b.0.clone()),
        "beats_b0_on_dev": best.as_ref().is_some_and(|b| b.1 > b0),
    })
}

pub fn main(m: &HashMap<String, String>) {
    let dir = PathBuf::from(m.get("in").expect("--in"));
    let mut avail: Vec<chrono::NaiveDate> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let n = e.ok()?.file_name().into_string().ok()?;
            n.strip_prefix("signals-")?
                .strip_suffix(".jsonl")?
                .parse()
                .ok()
        })
        .collect();
    avail.sort();
    let days: Vec<String> = crate::days_of(m, &avail)
        .iter()
        .map(|d| d.to_string())
        .collect();
    let d = load(&dir, &days);
    let mut keys: BTreeSet<String> = d.sigs.iter().map(|s| s.key()).collect();
    let only: Vec<String> = m
        .get("only")
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    if !only.is_empty() {
        keys.retain(|k| only.iter().any(|o| k.starts_with(o.as_str())));
    }
    let table: Vec<Value> = keys.iter().map(|k| metrics(&d, k)).collect();
    let b0 = table
        .iter()
        .find(|r| r["key"] == "b0:B0:trigger")
        .map_or(0.0, |r| r["precision"].as_f64().unwrap());
    // Every variant the run had, including one that produced no signal at all.
    let ran: Value = std::fs::read_to_string(dir.join("variants.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(json!({}));
    let vars = |det: &str| -> Vec<String> {
        let mut v: Vec<String> = ran[det]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x["id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if !only.is_empty() {
            v.retain(|id| {
                only.iter()
                    .any(|o| format!("{det}:{id}:").starts_with(o.as_str()))
            });
        }
        v.sort();
        v
    };
    let (bv, mv) = (vars("bull"), vars("micro"));
    let selection = json!({
        "bull": pre_declared_choice(&d, "bull", &bv, b0),
        "micro": pre_declared_choice(&d, "micro", &mv, b0),
    });
    let pairs: Vec<(String, String)> = match (m.get("pair"), bv.first(), mv.first()) {
        (Some(p), _, _) => p
            .split(',')
            .map(|x| {
                let (a, b) = x.split_once('+').expect("pair BFx+MPy");
                (a.to_string(), b.to_string())
            })
            .collect(),
        (None, Some(a), Some(b)) => vec![(a.clone(), b.clone())],
        _ => vec![],
    };
    let ov: Vec<Value> = pairs.iter().map(|(a, b)| overlap(&d, a, b)).collect();
    let out = json!({
        "days": d.days, "eligible_symbol_days": d.symdays,
        "signals_loaded": d.sigs.len(),
        "runs_reference_set": d.runs.iter().filter(|r| r.run.is_some()).count(),
        "metrics": table, "selection": selection, "overlap": ov,
    });
    let path = PathBuf::from(m.get("out").expect("--out"));
    std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
    // A compact table on stderr.
    eprintln!(
        "{:<24} {:>7} {:>6} {:>6} {:>6} {:>5} {:>7} {:>15} {:>8} {:>8} {:>8}",
        "key",
        "scored",
        "succ",
        "fail",
        "tout",
        "cens",
        "prec",
        "wilson95",
        "per_day",
        "cueLead",
        "recallM"
    );
    for r in &out["metrics"].as_array().unwrap().clone() {
        eprintln!(
            "{:<24} {:>7} {:>6} {:>6} {:>6} {:>5} {:>7.4} {:>7.4}-{:<7.4} {:>8.1} {:>8} {:>8.4}",
            r["key"].as_str().unwrap(),
            r["scored"],
            r["success"],
            r["failure"],
            r["timeout"],
            r["censored"],
            r["precision"].as_f64().unwrap(),
            r["wilson95"][0].as_f64().unwrap(),
            r["wilson95"][1].as_f64().unwrap(),
            r["per_day"].as_f64().unwrap(),
            r["cue_lead"]["median_s"],
            r["recall"]["share_before_midpoint"].as_f64().unwrap(),
        );
    }
    eprintln!("selection: {}", out["selection"]);
}
