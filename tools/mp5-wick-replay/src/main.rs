//! Offline paired replay. Archive readers, replay clock, chart admission and both
//! engines are the real Rust implementations. No provider, push or order client.
mod source;
mod current_glue {
    use stock_context as ctx;
    include!("glue.rs");
}
mod baseline_glue {
    use stock_context_baseline as ctx;
    include!("glue.rs");
}

use chart_engine::ChartEngine;
use chart_engine::wire::ChartUpdate;
use market_archive::{Archive, ArchiveConfig};
use market_events::{MarketEvent, UnixNanos};
use market_replay::{FeedItem, ReplayCore, marks::DEFAULT_MARK_STEP_NS};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use stock_context::{Engine, Family, MicroWick, State, Verdict};
use stock_context_baseline as baseline;

const SEC: i64 = 1_000_000_000;

#[derive(Clone, Deserialize)]
struct Job {
    day: String,
    sym: String,
    first_trigger_ns: i64,
    archive: String,
    live: bool,
    bytes: u64,
}
#[derive(Deserialize)]
struct Population {
    jobs: Vec<Job>,
}

#[derive(Clone)]
struct Print {
    exec: i64,
    admit: i64,
    price: i64,
}
struct Track {
    armed: i64,
    trigger: i64,
    stop: i64,
    price: Option<i64>,
    wick: Option<MicroWick>,
    dip_volume_milli: Option<i64>,
    cross: Option<i64>,
    retracted: bool,
    cross_revised: bool,
}

fn settlement(t: &Track, prints: &[Print], end: i64) -> (&'static str, Option<i64>) {
    let Some(cross) = t.cross else {
        return ("no_cross", None);
    };
    if t.cross_revised {
        return ("retracted", None);
    }
    let horizon = cross + 900 * SEC;
    let mut outcome = if end < horizon { "censored" } else { "timeout" };
    let start = prints.partition_point(|p| p.exec < cross);
    for p in prints[start..].iter().take_while(|p| p.exec <= horizon) {
        if p.admit <= cross || p.admit > horizon {
            continue;
        }
        if p.price as i128 * 100 <= t.trigger as i128 * 97 {
            outcome = "failure";
            return (outcome, Some(p.admit));
        }
        if p.price as i128 * 100 >= t.trigger as i128 * 105 {
            outcome = "continuation";
            return (outcome, Some(p.admit));
        }
    }
    (outcome, None)
}

struct Pipeline {
    job: Job,
    chart: ChartEngine,
    current: Engine,
    baseline: baseline::Engine,
    watched: bool,
    events: u64,
    compared: u64,
    revisions: u64,
    receive_times: u64,
    prints: Vec<Print>,
    tracks: BTreeMap<String, Track>,
}

fn strip_observer(mut v: Value) -> Value {
    if let Some(a) = v.get_mut("anchors").and_then(Value::as_object_mut) {
        a.remove("wick");
    }
    v
}

impl Pipeline {
    fn collect(&mut self) -> Result<(), String> {
        let current = self.current.take_setup_log();
        let control = self.baseline.take_setup_log();
        let a: Vec<_> = current
            .iter()
            .map(|t| strip_observer(serde_json::to_value(t).unwrap()))
            .collect();
        let b: Vec<_> = control
            .iter()
            .map(|t| serde_json::to_value(t).unwrap())
            .collect();
        if a != b {
            return Err(format!(
                "semantic transition mismatch: {} {} current={a:?} baseline={b:?}",
                self.job.day, self.job.sym
            ));
        }
        self.compared += a.len() as u64;
        for t in current {
            if t.family != Family::MicroPullback || t.warmup {
                continue;
            }
            if t.to == State::Armed
                && t.from != Some(t.to)
                && t.observed_at >= self.job.first_trigger_ns
                && t.observed_at - self.job.first_trigger_ns <= 30 * 60 * SEC
            {
                let Some(a) = &t.anchors else {
                    return Err("armed episode has no anchors".into());
                };
                self.tracks.entry(t.episode_id.clone()).or_insert(Track {
                    armed: t.observed_at,
                    trigger: a.h0,
                    stop: a.pullback_low,
                    price: self
                        .current
                        .watched_cue(&self.job.sym, t.observed_at)
                        .and_then(|c| c.price),
                    wick: a.wick.clone(),
                    dip_volume_milli: a.dv_rate_ratio_milli,
                    cross: None,
                    retracted: false,
                    cross_revised: false,
                });
            }
            if let Some(track) = self.tracks.get_mut(&t.episode_id) {
                if t.to == State::CrossingProvisional {
                    track.cross.get_or_insert(t.observed_at);
                }
                if t.event == "crossing_retracted" {
                    track.retracted = true;
                    if t.reason == "crossing_print_revised" {
                        track.cross_revised = true;
                    }
                }
            }
        }
        Ok(())
    }

    fn watch_at(&mut self, at: i64) -> Result<(), String> {
        if !self.watched && at >= self.job.first_trigger_ns {
            self.current
                .advance_to(UnixNanos(self.job.first_trigger_ns));
            self.baseline
                .advance_to(UnixNanos(self.job.first_trigger_ns));
            self.collect()?;
            self.current.set_watched([self.job.sym.clone()]);
            self.baseline.set_watched([self.job.sym.clone()]);
            self.watched = true;
        }
        Ok(())
    }

    fn halts(&mut self, updates: &[ChartUpdate]) {
        for u in updates {
            if let ChartUpdate::Halt(h) = u {
                self.current.set_halted(&h.symbol, h.halt.end.is_none());
                self.baseline.set_halted(&h.symbol, h.halt.end.is_none());
            }
        }
    }

    fn advance(&mut self, at: i64) -> Result<(), String> {
        self.watch_at(at)?;
        let updates = self.chart.advance_clock(UnixNanos(at));
        self.halts(&updates);
        self.current.advance_to(UnixNanos(at));
        self.baseline.advance_to(UnixNanos(at));
        self.collect()
    }

    fn event(&mut self, event: MarketEvent) -> Result<(), String> {
        self.events += 1;
        match &event {
            MarketEvent::Trade(t) => {
                let admit = t.receive_ts.unwrap_or(t.event_ts);
                self.watch_at(admit.0)?;
                let (updates, _) = self.chart.apply_revising(&event);
                if let Some(ChartUpdate::Tape(row)) = updates.first() {
                    let v = Verdict {
                        ohlc: row.counted == "ohlc_and_volume",
                        volume: matches!(row.counted.as_str(), "ohlc_and_volume" | "volume_only"),
                    };
                    self.current.apply(t, v, admit);
                    self.baseline.apply(
                        t,
                        baseline::Verdict {
                            ohlc: v.ohlc,
                            volume: v.volume,
                        },
                        admit,
                    );
                    if t.receive_ts.is_some() {
                        self.receive_times += 1;
                    }
                    if v.ohlc {
                        self.prints.push(Print {
                            exec: t.event_ts.0,
                            admit: admit.0,
                            price: t.price.micros(),
                        });
                    }
                }
                self.halts(&updates);
            }
            MarketEvent::Correction(_) | MarketEvent::Cancel(_) => {
                self.revisions += 1;
                let (updates, revision) = self.chart.apply_revising(&event);
                let at = current_glue::revision_admit(&event);
                if at != baseline_glue::revision_admit(&event) {
                    return Err("revision admission adapters disagree".into());
                }
                self.watch_at(at.0)?;
                if let Some(r) = current_glue::revision_of(&event, revision.as_ref()) {
                    self.current.apply_revision(&r, at);
                }
                if let Some(r) = baseline_glue::revision_of(&event, revision.as_ref()) {
                    self.baseline.apply_revision(&r, at);
                }
                self.halts(&updates);
            }
            _ => {
                let updates = self.chart.apply(&event);
                self.halts(&updates);
            }
        }
        self.collect()
    }

    fn results(&mut self, end: i64) -> Value {
        self.prints.sort_by_key(|p| (p.exec, p.admit));
        let episodes: Vec<_> = self.tracks.iter().map(|(id, t)| {
            let (outcome, settled_at) = settlement(t, &self.prints, end);
            json!({"episode_id":id,"armed_at":t.armed,"trigger":t.trigger,"stop":t.stop,
                "price_at_arm":t.price,"wick":t.wick,"dip_volume_ratio_milli":t.dip_volume_milli,
                "cross_at":t.cross,"crossing_retracted":t.retracted,"crossing_print_revised":t.cross_revised,"outcome":outcome,"settled_at":settled_at})
        }).collect();
        json!({"day":self.job.day,"symbol":self.job.sym,"first_trigger_ns":self.job.first_trigger_ns,
            "archive":self.job.archive,"live":self.job.live,"events":self.events,
            "counted_prints":self.prints.len(),"receive_timestamps":self.receive_times,"revisions":self.revisions,
            "compared_transitions":self.compared,"semantic_mismatches":0,
            "dropped_transitions":self.current.detection_telemetry().transitions_dropped + self.baseline.detection_telemetry().transitions_dropped,
            "episodes":episodes})
    }
}

fn run(job: &Job) -> Result<Value, String> {
    let day = job.day.parse().map_err(|e| format!("date: {e}"))?;
    let archive = Archive::new(ArchiveConfig {
        root: PathBuf::from(&job.archive),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let source = Arc::new(source::OneSymbol {
        archive,
        day,
        symbol: job.sym.clone(),
        live: job.live,
    });
    let mut replay = ReplayCore::new(source, DEFAULT_MARK_STEP_NS)?;
    let run_id = format!("replay:{}", job.day);
    let current = Engine::new(
        stock_context::Policy::current(),
        stock_context::RunInfo {
            run_id: run_id.clone(),
            source: "replay".into(),
            fidelity: stock_context::Fidelity::ReconstructedHistory,
            numerator_coverage: stock_context::Coverage::Unverified,
        },
    );
    let baseline = baseline::Engine::new(
        baseline::Policy::current(),
        baseline::RunInfo {
            run_id,
            source: "replay".into(),
            fidelity: baseline::Fidelity::ReconstructedHistory,
            numerator_coverage: baseline::Coverage::Unverified,
        },
    );
    let mut p = Pipeline {
        job: job.clone(),
        chart: ChartEngine::default(),
        current,
        baseline,
        watched: false,
        events: 0,
        compared: 0,
        revisions: 0,
        receive_times: 0,
        prints: Vec::new(),
        tracks: BTreeMap::new(),
    };
    let mut items = Vec::new();
    loop {
        items.clear();
        let more = replay.step(&mut items)?;
        for item in items.drain(..) {
            match item {
                FeedItem::Advance(m) => p.advance(m.nanos())?,
                FeedItem::Event(e) => p.event(e)?,
                _ => {}
            }
        }
        if !more {
            break;
        }
    }
    p.advance(replay.end().nanos())?;
    let result = p.results(replay.end().nanos());
    if result["dropped_transitions"] != 0 {
        return Err("transition journal dropped records".into());
    }
    Ok(result)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(
        args.len() >= 3,
        "mp5-wick-replay POPULATION.json OUT [THREADS] [LIMIT]"
    );
    let population: Population = serde_json::from_slice(&std::fs::read(&args[1]).unwrap()).unwrap();
    let out = Path::new(&args[2]);
    std::fs::create_dir_all(out).unwrap();
    let threads: usize = args.get(3).map(|x| x.parse().unwrap()).unwrap_or(8);
    let limit: usize = args
        .get(4)
        .map(|x| x.parse().unwrap())
        .unwrap_or(population.jobs.len());
    let jobs = &population.jobs[..limit.min(population.jobs.len())];
    assert!(jobs.iter().all(|j| market_archive::valid_symbol(&j.sym)));
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let start = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    let path = out.join(format!("{}-{}.json", job.day, job.sym));
                    if path.exists() {
                        done.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let mut result = match run(job) {
                        Ok(r) => r,
                        Err(error) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            json!({"day":job.day,"symbol":job.sym,"error":error})
                        }
                    };
                    result["input_bytes"] = json!(job.bytes);
                    let temp = path.with_extension("tmp");
                    std::fs::write(&temp, serde_json::to_vec(&result).unwrap()).unwrap();
                    std::fs::rename(temp, path).unwrap();
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(100) {
                        eprintln!(
                            "replayed {n}/{} symbol-days, errors {}, elapsed {:.1}s",
                            jobs.len(),
                            failed.load(Ordering::Relaxed),
                            start.elapsed().as_secs_f64()
                        );
                    }
                }
            });
        }
    });
    let summary = json!({"jobs":jobs.len(),"completed":done.load(Ordering::Relaxed),"errors":failed.load(Ordering::Relaxed),"seconds":start.elapsed().as_secs_f64()});
    std::fs::write(
        out.join("run-summary.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();
    eprintln!("{summary}");
    if failed.load(Ordering::Relaxed) > 0 {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Track {
        Track {
            armed: 0,
            trigger: 100,
            stop: 90,
            price: Some(100),
            wick: None,
            dip_volume_milli: None,
            cross: Some(10 * SEC),
            retracted: false,
            cross_revised: false,
        }
    }
    fn print(seconds: i64, price: i64) -> Print {
        Print {
            exec: seconds * SEC,
            admit: seconds * SEC,
            price,
        }
    }
    #[test]
    fn target_and_stop_order_and_retracted_crossings() {
        let mut t = track();
        assert_eq!(
            settlement(&t, &[print(11, 105), print(12, 97)], 1000 * SEC).0,
            "continuation"
        );
        assert_eq!(
            settlement(&t, &[print(11, 97), print(12, 105)], 1000 * SEC).0,
            "failure"
        );
        t.cross_revised = true;
        assert_eq!(settlement(&t, &[print(11, 105)], 1000 * SEC).0, "retracted");
        t.cross = None;
        assert_eq!(settlement(&t, &[], 1000 * SEC).0, "no_cross");
    }
    #[test]
    fn pre_crossing_and_late_admission_do_not_settle_the_window() {
        let p = [
            Print {
                exec: 9 * SEC,
                admit: 11 * SEC,
                price: 105,
            },
            Print {
                exec: 20 * SEC,
                admit: 1000 * SEC,
                price: 105,
            },
            print(911, 105),
        ];
        assert_eq!(settlement(&track(), &p, 1000 * SEC).0, "timeout");
        assert_eq!(settlement(&track(), &[], 100 * SEC).0, "censored");
        assert_eq!(
            settlement(&track(), &[print(910, 105)], 1000 * SEC).0,
            "continuation"
        );
    }

    #[test]
    fn an_ordinary_failed_candle_close_is_not_censored() {
        let mut t = track();
        t.retracted = true;
        assert_eq!(settlement(&t, &[print(11, 97)], 1000 * SEC).0, "failure");
        assert_eq!(
            settlement(&t, &[print(11, 105)], 1000 * SEC).0,
            "continuation"
        );
        assert_eq!(settlement(&t, &[], 1000 * SEC).0, "timeout");
        t.cross_revised = true;
        assert_eq!(settlement(&t, &[print(11, 105)], 1000 * SEC).0, "retracted");
    }
}
