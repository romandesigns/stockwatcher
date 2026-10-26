//! RESEARCH ONLY (docs/research/setup-compare/protocol.md): replays archived days through the
//! production pipeline and scores `bull_flag_v0`, `micro_pullback_v0` and the B0 baseline.
//! Reads an archive copy, writes JSONL/JSON outside the repo, never touches a provider or a
//! production path.
//!
//! ```text
//! setup-research run   --archive DIR --out DIR [--days D,D|--from D --to D] [--bull BF0,..]
//!                      [--micro MP0,..] [--threads N] [--symbols A,B]
//! setup-research score --in DIR --days D,D|--from D --to D --out FILE [--only det:var,..]
//! ```

mod outcome;
mod replay;
mod score;
mod variants;

use chrono::NaiveDate;
use market_archive::{Archive, ArchiveConfig};
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

fn args() -> HashMap<String, String> {
    let a: Vec<String> = std::env::args().collect();
    let mut m = HashMap::new();
    m.insert("cmd".to_string(), a.get(1).cloned().unwrap_or_default());
    let mut i = 2;
    while i < a.len() {
        if let Some(k) = a[i].strip_prefix("--") {
            m.insert(k.to_string(), a.get(i + 1).cloned().unwrap_or_default());
            i += 2;
        } else {
            i += 1;
        }
    }
    m
}

fn list(m: &HashMap<String, String>, k: &str) -> Vec<String> {
    m.get(k)
        .map(|s| {
            s.split(',')
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn days_of(m: &HashMap<String, String>, available: &[NaiveDate]) -> Vec<NaiveDate> {
    let explicit: Vec<NaiveDate> = list(m, "days")
        .iter()
        .map(|d| d.parse().expect("day YYYY-MM-DD"))
        .collect();
    if !explicit.is_empty() {
        return explicit;
    }
    let from: Option<NaiveDate> = m.get("from").map(|d| d.parse().expect("from"));
    let to: Option<NaiveDate> = m.get("to").map(|d| d.parse().expect("to"));
    available
        .iter()
        .copied()
        .filter(|d| from.is_none_or(|f| *d >= f) && to.is_none_or(|t| *d <= t))
        .collect()
}

fn main() {
    let m = args();
    match m["cmd"].as_str() {
        "run" => run(&m),
        "score" => score::main(&m),
        _ => {
            eprintln!("usage: setup-research run|score ... (see the crate docs)");
            std::process::exit(2);
        }
    }
}

/// Early skip (protocol section 2): only a stored daily bar proving the day never came within
/// 20 % of the band. No bar, no skip.
fn prefilter_out(bars: Option<&market_archive::RawDayBars>, sym: &str) -> bool {
    let Some(Some(Some(b))) = bars.map(|d| d.coverage(sym)) else {
        return false;
    };
    let (lo, hi) = (b.l.unwrap_or(b.c), b.h.unwrap_or(b.c));
    lo > 30.0 || hi < 0.12
}

fn run(m: &HashMap<String, String>) {
    let root = PathBuf::from(m.get("archive").expect("--archive"));
    let out = PathBuf::from(m.get("out").expect("--out"));
    std::fs::create_dir_all(&out).expect("out dir");
    let archive = Archive::new(ArchiveConfig {
        root,
        retention_days: 0,
        min_free_bytes: 0,
    });
    let available = archive.days().expect("archive days");
    let days = days_of(m, &available);
    let v = replay::Variants {
        bulls: variants::bull(&list(m, "bull")),
        micros: variants::micro(&list(m, "micro")),
    };
    let threads: usize = m
        .get("threads")
        .map(|t| t.parse().expect("threads"))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });
    let only_syms = list(m, "symbols");
    let policies = json!({
        "bull": v.bulls.iter().map(|(id, p)| json!({"id": id, "tag": p.tag(), "rules": format!("{:?}", p.body.bull_flag)})).collect::<Vec<_>>(),
        "micro": v.micros.iter().map(|(id, p)| json!({"id": id, "tag": p.tag(), "rules": format!("{:?}", p.body.micro)})).collect::<Vec<_>>(),
    });
    std::fs::write(
        out.join("variants.json"),
        serde_json::to_string_pretty(&policies).unwrap(),
    )
    .expect("write variants");
    for day in days {
        let done = out.join(format!("day-{day}.json"));
        if done.exists() {
            eprintln!("{day}: already done, skipped");
            continue;
        }
        run_day(&archive, day, &v, threads, &only_syms, &out);
    }
}

fn run_day(
    archive: &Archive,
    day: NaiveDate,
    v: &replay::Variants,
    threads: usize,
    only: &[String],
    out: &Path,
) {
    let t = Instant::now();
    let Some(live) = replay::day_version(archive, day) else {
        eprintln!("{day}: no replayable version, skipped");
        std::fs::write(
            out.join(format!("day-{day}.json")),
            json!({"day": day.to_string(), "missing": true}).to_string(),
        )
        .unwrap();
        return;
    };
    let manifest = if live {
        archive.live_manifest(day)
    } else {
        archive.manifest(day)
    }
    .expect("manifest")
    .expect("manifest present");
    let raw = archive.read_raw_bars(day).ok().flatten();
    let mut syms: Vec<(String, u64)> = manifest
        .symbols
        .iter()
        .filter(|(s, _)| only.is_empty() || only.contains(s))
        .map(|(s, e)| (s.clone(), e.bytes))
        .collect();
    let total_syms = syms.len();
    syms.retain(|(s, _)| !prefilter_out(raw.as_ref(), s));
    let prefiltered = total_syms - syms.len();
    // Largest first, so one big file does not finish last on its own thread.
    syms.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let next = AtomicUsize::new(0);
    let tmp_sig = out.join(format!("signals-{day}.jsonl.tmp"));
    let tmp_sd = out.join(format!("symdays-{day}.jsonl.tmp"));
    let sig_w = Mutex::new(std::io::BufWriter::new(
        std::fs::File::create(&tmp_sig).unwrap(),
    ));
    let sd_w = Mutex::new(std::io::BufWriter::new(
        std::fs::File::create(&tmp_sd).unwrap(),
    ));
    let stats = Mutex::new((0u64, 0u64, 0u64, 0u64, 0u64, Vec::<String>::new())); // (events, trades, eligible, dropped, ohlc prints, errors)
    let dstr = day.to_string();
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((sym, _)) = syms.get(i) else {
                        break;
                    };
                    match replay::run_symbol_day(archive, day, sym, live, v) {
                        Err(e) => stats.lock().unwrap().5.push(format!("{sym}: {e}")),
                        Ok(sd) => {
                            {
                                let mut s = stats.lock().unwrap();
                                s.0 += sd.events;
                                s.1 += sd.trades;
                                s.2 += u64::from(sd.eligible);
                                s.3 += sd.dropped;
                                if sd.eligible {
                                    s.4 += sd.prints.len() as u64;
                                }
                            }
                            if !sd.eligible {
                                continue;
                            }
                            let run = outcome::first_run(&sd.bars);
                            let mut lines = String::new();
                            for s in &sd.signals {
                                let sc = outcome::score(&sd.prints, &sd.bars, s, &dstr, sym);
                                lines.push_str(&serde_json::to_string(&sc).unwrap());
                                lines.push('\n');
                            }
                            sig_w.lock().unwrap().write_all(lines.as_bytes()).unwrap();
                            let row = json!({
                                "day": dstr, "sym": sym, "trades": sd.trades,
                                "ohlc_prints": sd.prints.len(), "bars": sd.bars.len(), "run": run,
                            });
                            writeln!(sd_w.lock().unwrap(), "{row}").unwrap();
                        }
                    }
                }
            });
        }
    });
    sig_w.into_inner().unwrap().flush().unwrap();
    sd_w.into_inner().unwrap().flush().unwrap();
    std::fs::rename(&tmp_sig, out.join(format!("signals-{day}.jsonl"))).unwrap();
    std::fs::rename(&tmp_sd, out.join(format!("symdays-{day}.jsonl"))).unwrap();
    let s = stats.into_inner().unwrap();
    let summary = json!({
        "day": dstr, "live_recording": live, "symbols_in_manifest": total_syms,
        "prefiltered_by_daily_bar": prefiltered, "replayed": syms.len(),
        "events": s.0, "trades": s.1, "eligible_symbol_days": s.2,
        "eligible_ohlc_prints": s.4, "journal_dropped": s.3,
        "errors": s.5, "secs": t.elapsed().as_secs_f64(),
    });
    std::fs::write(
        out.join(format!("day-{day}.json")),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();
    eprintln!("{summary}");
}
