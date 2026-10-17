//! `swarchive`: archive tools.
//!
//! ```text
//! swarchive days
//! swarchive fixture <YYYY-MM-DD> [SYMBOL…] [--seed N]   # deterministic FIXTURE day (not market data)
//! swarchive inspect <YYYY-MM-DD> [--records N] [--symbol S]
//! swarchive verify  <YYYY-MM-DD>
//! swarchive prune   [--today YYYY-MM-DD]               # honours ARCHIVE_RETENTION_DAYS
//! swarchive import alpaca …                            # skeleton only; see below
//! ```
//!
//! Configuration comes from `ARCHIVE_ROOT`, `ARCHIVE_RETENTION_DAYS` and `ARCHIVE_MIN_FREE_MB`.
//! Times are printed in America/New_York via chrono-tz, never the shell's timezone.

use chrono::{NaiveDate, Utc};
use chrono_tz::America::New_York;
use market_archive::fixture::{DEFAULT_SEED, DEFAULT_SYMBOLS, write_fixture_day};
use market_archive::{Archive, ArchiveConfig};
use market_events::{MarketEvent, UnixNanos, classify};
use std::collections::BTreeMap;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  swarchive days\n  swarchive fixture <YYYY-MM-DD> [SYMBOL...] [--seed N]\n  swarchive inspect <YYYY-MM-DD> [--records N] [--symbol S]\n  swarchive verify <YYYY-MM-DD>\n  swarchive prune [--today YYYY-MM-DD]\n  swarchive import alpaca <YYYY-MM-DD> <SYMBOL...>   (skeleton, refuses to run)"
    );
    ExitCode::from(2)
}

fn et_string(ns: i64) -> String {
    UnixNanos(ns)
        .to_utc()
        .with_timezone(&New_York)
        .format("%Y-%m-%d %H:%M:%S%.3f %Z")
        .to_string()
}

fn parse_day(s: Option<&String>) -> Result<NaiveDate, String> {
    let s = s.ok_or("missing <YYYY-MM-DD>")?;
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| format!("invalid day {s:?}"))
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match ArchiveConfig::from_lookup(|k| std::env::var(k).ok()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config: {e}");
            return ExitCode::FAILURE;
        }
    };
    let archive = Archive::new(cfg);
    let result = match args.first().map(String::as_str) {
        Some("days") => days(&archive),
        Some("fixture") => fixture(&archive, &args[1..]),
        Some("inspect") => inspect(&archive, &args[1..]),
        Some("verify") => verify(&archive, &args[1..]),
        Some("prune") => prune(&archive, &args[1..]),
        Some("import") => import(&args[1..]),
        _ => return usage(),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn days(archive: &Archive) -> Result<ExitCode, String> {
    for d in archive.days().map_err(|e| e.to_string())? {
        let m = archive
            .manifest(d)
            .map_err(|e| e.to_string())?
            .expect("listed");
        println!(
            "{d}  {:>3} symbols  {:>9} events  {}",
            m.symbols.len(),
            m.total_events(),
            if m.fixture { "FIXTURE" } else { "" }
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn fixture(archive: &Archive, args: &[String]) -> Result<ExitCode, String> {
    let day = parse_day(args.first())?;
    let seed = match flag(args, "--seed") {
        Some(s) => s.parse().map_err(|_| format!("invalid --seed {s:?}"))?,
        None => DEFAULT_SEED,
    };
    let mut symbols: Vec<String> = Vec::new();
    let mut skip = false;
    for a in &args[1..] {
        if skip {
            skip = false;
        } else if a == "--seed" {
            skip = true;
        } else {
            symbols.push(a.to_ascii_uppercase());
        }
    }
    if symbols.is_empty() {
        symbols = DEFAULT_SYMBOLS.iter().map(|s| s.to_string()).collect();
    }
    let m = write_fixture_day(archive, day, &symbols, seed).map_err(|e| e.to_string())?;
    println!(
        "wrote FIXTURE day {day} (seed {seed}) to {}: {} symbols, {} events. Fixture data is not market data.",
        archive.day_dir(day).display(),
        m.symbols.len(),
        m.total_events()
    );
    Ok(ExitCode::SUCCESS)
}

fn inspect(archive: &Archive, args: &[String]) -> Result<ExitCode, String> {
    let day = parse_day(args.first())?;
    let m = archive
        .manifest(day)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no manifest for {day}"))?;
    println!(
        "day {}  format v{}  contract v{}  sources {:?}{}",
        m.day,
        m.format_version,
        m.contract_version,
        m.sources,
        if m.fixture {
            "  FIXTURE (not market data)"
        } else {
            ""
        }
    );
    let only = flag(args, "--symbol");
    let records: usize = flag(args, "--records")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    for e in m.symbols.values() {
        if only.as_deref().is_some_and(|s| s != e.symbol) {
            continue;
        }
        println!(
            "\n{}  {}  {} B  sha256 {}",
            e.symbol, e.file, e.bytes, e.sha256
        );
        println!(
            "  events {}  trades {}  quotes {}  corrections {}  cancels {}  statuses {}",
            e.events, e.trades, e.quotes, e.corrections, e.cancels, e.statuses
        );
        if let (Some(a), Some(b)) = (e.first_ts, e.last_ts) {
            println!("  first {}  last {}", et_string(a), et_string(b));
        }
        let reader = archive.reader(day, &e.symbol).map_err(|e| e.to_string())?;
        let mut per_session: BTreeMap<String, u64> = BTreeMap::new();
        for (i, ev) in reader.enumerate() {
            let ev = ev.map_err(|e| e.to_string())?;
            if let MarketEvent::Trade(t) = &ev.event {
                let s = classify(t.event_ts).map_or("outside".to_string(), |s| s.to_string());
                *per_session.entry(s).or_default() += 1;
            }
            if i < records {
                println!(
                    "  [{i}] {}  {}",
                    et_string(ev.event.event_ts().nanos()),
                    serde_json::to_string(&ev).unwrap_or_default()
                );
            }
        }
        for (s, n) in per_session {
            println!("  trades in {s}: {n}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn verify(archive: &Archive, args: &[String]) -> Result<ExitCode, String> {
    let day = parse_day(args.first())?;
    let rep = archive.verify(day).map_err(|e| e.to_string())?;
    for s in &rep.symbols {
        if s.problems.is_empty() {
            println!("ok    {}", s.symbol);
        } else {
            for p in &s.problems {
                println!("FAIL  {}: {p}", s.symbol);
            }
        }
    }
    for u in &rep.unlisted {
        println!("note  {u} is not listed in the manifest");
    }
    Ok(if rep.ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn prune(archive: &Archive, args: &[String]) -> Result<ExitCode, String> {
    let today = match flag(args, "--today") {
        Some(s) => parse_day(Some(&s))?,
        None => Utc::now().with_timezone(&New_York).date_naive(),
    };
    if archive.config().retention_days == 0 {
        println!("ARCHIVE_RETENTION_DAYS is 0 (keep forever): nothing pruned");
        return Ok(ExitCode::SUCCESS);
    }
    for d in archive.prune(today).map_err(|e| e.to_string())? {
        println!("deleted {d}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Skeleton for importing Alpaca historical trades (`GET /v2/stocks/{symbol}/trades`, feed=sip)
/// into the archive. Deliberately inert in this build: it needs an explicit opt-in
/// (`SWARCHIVE_IMPORT=alpaca`), entitlement verification and credentials from the environment,
/// none of which this phase provides. It never reads credentials and never calls the network.
fn import(args: &[String]) -> Result<ExitCode, String> {
    if args.first().map(String::as_str) != Some("alpaca") {
        return Err("only `import alpaca` is defined".into());
    }
    if std::env::var("SWARCHIVE_IMPORT").ok().as_deref() != Some("alpaca") {
        return Err(
            "import is disabled: set SWARCHIVE_IMPORT=alpaca to opt in explicitly (and see docs/architecture/simulator-and-chart.md, Appendix B)"
                .into(),
        );
    }
    Err("the Alpaca importer is not implemented yet: it waits for entitlement verification (spec 01, build step 2)".into())
}
