//! `rf`: research-features command line.
//!
//! ```text
//! rf audit-table                                  # the condition audit as Markdown
//! rf conditions <archive-root> [--day YYYY-MM-DD]... [--symbol SYM]...
//!                                                 # condition-impact report (JSON) over an archive
//! rf candles <archive-root> <day> <symbol> <out-dir>
//!                                                 # chart-engine candles+gaps per timeframe (JSON)
//! rf parity <candles.json> <ts-indicators.json> [--tolerance 1e-9]
//!                                                 # Rust chart_* vs the TS output, max |diff| per line
//! ```

use chart_engine::{ChartEngine, Timeframe};
use chrono::NaiveDate;
use market_archive::{Archive, ArchiveConfig};
use market_events::{MarketEvent, UnixNanos};
use research_features::condition_impact::ImpactReport;
use research_features::conditions::audit_markdown;
use research_features::parity::{CandleFile, TsOut, compare};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let res = match args.first().map(String::as_str) {
        Some("audit-table") => {
            print!("{}", audit_markdown());
            Ok(())
        }
        Some("conditions") => conditions(&args[1..]),
        Some("candles") => candles(&args[1..]),
        Some("parity") => parity(&args[1..]),
        _ => Err(
            "usage: rf audit-table | conditions <root> [--day D]... [--symbol S]... | \
                  candles <root> <day> <symbol> <out-dir> | parity <candles.json> <ts.json> \
                  [--tolerance T]"
                .to_string(),
        ),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rf: {e}");
            ExitCode::FAILURE
        }
    }
}

fn open(root: &str) -> Archive {
    Archive::new(ArchiveConfig {
        root: PathBuf::from(root),
        ..ArchiveConfig::default()
    })
}

fn parse_day(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| format!("bad day {s}: {e}"))
}

fn events(archive: &Archive, day: NaiveDate, symbol: &str) -> Result<Vec<MarketEvent>, String> {
    archive
        .reader(day, symbol)
        .map_err(|e| e.to_string())?
        .map(|r| r.map(|v| v.event).map_err(|e| e.to_string()))
        .collect()
}

#[derive(Serialize)]
struct ConditionsOut {
    archive: String,
    days: Vec<String>,
    symbol_days: u64,
    report: ImpactReport,
}

fn conditions(args: &[String]) -> Result<(), String> {
    let root = args.first().ok_or("conditions: missing archive root")?;
    let (mut days, mut symbols) = (Vec::new(), Vec::new());
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--day" => days.push(parse_day(it.next().ok_or("--day needs a value")?)?),
            "--symbol" => symbols.push(it.next().ok_or("--symbol needs a value")?.clone()),
            x => return Err(format!("unknown argument {x}")),
        }
    }
    let archive = open(root);
    if days.is_empty() {
        days = archive.days().map_err(|e| e.to_string())?;
    }
    let mut report = ImpactReport::new();
    let mut symbol_days = 0;
    for &day in &days {
        let Some(m) = archive.manifest(day).map_err(|e| e.to_string())? else {
            continue;
        };
        for sym in m.symbols.keys() {
            if !symbols.is_empty() && !symbols.contains(sym) {
                continue;
            }
            symbol_days += 1;
            for e in events(&archive, day, sym)? {
                if let MarketEvent::Trade(t) = e {
                    report.add(&t);
                }
            }
        }
    }
    let out = ConditionsOut {
        archive: root.clone(),
        days: days.iter().map(|d| d.to_string()).collect(),
        symbol_days,
        report,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn candles(args: &[String]) -> Result<(), String> {
    let [root, day, symbol, out] = args else {
        return Err("candles <archive-root> <day> <symbol> <out-dir>".into());
    };
    let archive = open(root);
    let day_d = parse_day(day)?;
    let evs = events(&archive, day_d, symbol)?;
    // As market-providers' `replay_day`: apply every event, then move the clock past the last
    // print's 5-minute interval so every candle is final.
    let mut engine = ChartEngine::default();
    let mut last: Option<i64> = None;
    for e in &evs {
        engine.apply(e);
        last = Some(last.map_or(e.event_ts().0, |l| l.max(e.event_ts().0)));
    }
    if let Some(l) = last {
        engine.advance_clock(UnixNanos(l + 10 * 60 * 1_000_000_000));
    }
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    for tf in Timeframe::ALL {
        let snap = engine
            .snapshot(symbol, tf, None)
            .ok_or_else(|| format!("no snapshot for {symbol} {tf}"))?;
        let file = CandleFile {
            symbol: symbol.clone(),
            day: day.clone(),
            timeframe: tf,
            source: format!("chart-engine default config, archive {root}"),
            candles: snap.candles,
            gaps: snap.gaps,
        };
        let path = Path::new(out).join(format!("{symbol}-{day}-{tf}.json"));
        std::fs::write(
            &path,
            serde_json::to_string(&file).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        eprintln!(
            "{}: {} candles, {} gaps",
            path.display(),
            file.candles.len(),
            file.gaps.len()
        );
    }
    Ok(())
}

fn parity(args: &[String]) -> Result<(), String> {
    let (Some(cpath), Some(tpath)) = (args.first(), args.get(1)) else {
        return Err("parity <candles.json> <ts.json> [--tolerance T]".into());
    };
    let tol: f64 = match args.get(2).map(String::as_str) {
        Some("--tolerance") => args
            .get(3)
            .ok_or("--tolerance needs a value")?
            .parse()
            .map_err(|e| format!("bad tolerance: {e}"))?,
        _ => 1e-9,
    };
    let read = |p: &str| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"));
    let candles: CandleFile =
        serde_json::from_str(&read(cpath)?).map_err(|e| format!("{cpath}: {e}"))?;
    let ts: TsOut = serde_json::from_str(&read(tpath)?).map_err(|e| format!("{tpath}: {e}"))?;
    let diffs = compare(&candles, &ts)?;
    let worst = diffs.values().map(|d| d.max_abs_diff).fold(0.0, f64::max);
    let nan = diffs.values().map(|d| d.nan_mismatches).sum::<usize>();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "candles": cpath,
            "symbol": candles.symbol,
            "day": candles.day,
            "timeframe": candles.timeframe,
            "slots": candles.candles.len() + candles.gaps.len(),
            "tolerance_abs": tol,
            "max_abs_diff": worst,
            "nan_mismatches": nan,
            "lines": diffs,
        }))
        .map_err(|e| e.to_string())?
    );
    if worst > tol || nan > 0 {
        return Err(format!(
            "parity FAILED: max |diff| {worst:e} (tolerance {tol:e}), {nan} NaN mismatches"
        ));
    }
    Ok(())
}
