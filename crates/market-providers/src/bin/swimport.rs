//! `swimport`: provider history into the archive (feature `import`).
//!
//! ```text
//! swimport credentials                                   # which variables are set (never values)
//! swimport alpaca-trades <SYMBOL> <YYYY-MM-DD> [--feed sip] [--limit 10000] [--force]
//! swimport fmp-bars <SYMBOL> <YYYY-MM-DD> [--force]
//! swimport alpaca-batch --from <YYYY-MM-DD> --to <YYYY-MM-DD> [--jobs 12] [--per-minute 9000] [--plan-only]
//! swimport check <SYMBOL> <YYYY-MM-DD>                   # verify, replay twice, compare with FMP
//! swimport alpaca-assets                                 # names + exchanges -> $ARCHIVE_ROOT/assets.json
//! swimport alpaca-daily-closes --from <YYYY-MM-DD> --to <YYYY-MM-DD>  # -> $ARCHIVE_ROOT/daily-closes.json
//! swimport maintain [--once]                             # keep the rolling 21-trading-day window (runs forever)
//! swimport research-universe --out <DIR> --from <YYYY-MM-DD> --to <YYYY-MM-DD> [--per-minute 100]
//! swimport research-pilot --out <DIR> --selection <FILE> [--per-minute 100] [--budget-minutes 30] [--budget-gb 5] [--warmup 30]
//! swimport research-manifest --out <DIR> [--meta <FILE>]
//! ```
//!
//! The `research-*` commands are a bounded research acquisition (import::pilot), never used by
//! production: they write raw reference data, SIP and BOATS trades and SIP quotes under `<DIR>`,
//! one process and one paced client at a time (import::pacing; default 100 requests/minute,
//! lowered further when the provider's `X-RateLimit-*` headers ask for it). Every run appends
//! its measurements to `<DIR>/runs.jsonl` and the time budget is cumulative over runs.
//!
//! `maintain` is what the VPS runs as its `archiver` service (import::maintain has the window,
//! schedule and safety rules, and its environment variables). It is a subcommand of this binary
//! rather than a binary of its own because it is the same importer, credentials and HTTP client
//! on a timer: one more file in the image and one more code path to keep in step would buy
//! nothing.
//!
//! Credentials come only from the environment (a local `.env` or the Pi's secret store, loaded by
//! the shell): `ALPACA_API_KEY` / `ALPACA_API_SECRET` (or `APCA_API_KEY_ID` /
//! `APCA_API_SECRET_KEY`) and `FMP_API_KEY`. Values are never printed. Base URLs:
//! `ALPACA_DATA_BASE` (default `https://data.alpaca.markets`), `FMP_BASE` (default
//! `https://financialmodelingprep.com`), `ALPACA_TRADING_BASE` for the assets list (default
//! `https://paper-api.alpaca.markets`). The archive is `ARCHIVE_ROOT` (see swarchive).
//!
//! The SIP range defaults to the trading date's premarket open through the end of after hours,
//! from the session calendar (early closes included). Only completed trading dates are accepted.

use chrono::{NaiveDate, Utc};
use chrono_tz::America::New_York;
use market_archive::{Archive, ArchiveConfig, BarEntry};
use market_events::session::sessions_for;
use market_events::{Price, PriceBand, SessionKind, UnixNanos, is_trading_day};
use market_providers::import::alpaca::{
    self, AlpacaCredentials, Feed, ImportOptions, TradesQuery, import_trades,
};
use market_providers::import::check::{compare_minutes, replay_day};
use market_providers::import::client::ReqwestHttp;
use market_providers::import::fmp::{self, FmpKey, fetch_minute_bars};
use market_providers::import::http::RetryPolicy;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  swimport credentials\n  swimport alpaca-trades <SYMBOL> <YYYY-MM-DD> [--feed sip|iex|boats] [--limit N] [--force]\n  swimport fmp-bars <SYMBOL> <YYYY-MM-DD> [--force]\n  swimport check <SYMBOL> <YYYY-MM-DD>\n  swimport alpaca-batch --from <YYYY-MM-DD> --to <YYYY-MM-DD> [--jobs N] [--per-minute N] [--plan-only]\n  swimport alpaca-assets
  swimport alpaca-daily-closes --from <YYYY-MM-DD> --to <YYYY-MM-DD>
  swimport maintain [--once]
  swimport research-universe --out <DIR> --from <YYYY-MM-DD> --to <YYYY-MM-DD> [--per-minute N]
  swimport research-pilot --out <DIR> --selection <FILE> [--per-minute N] [--budget-minutes N] [--budget-gb N] [--warmup N]
  swimport research-manifest --out <DIR> [--meta <FILE>]"
    );
    ExitCode::from(2)
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn et(ns: i64) -> String {
    UnixNanos(ns)
        .to_utc()
        .with_timezone(&New_York)
        .format("%Y-%m-%d %H:%M:%S%.3f %Z")
        .to_string()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let run = || -> Result<ExitCode, String> {
        match args.first().map(String::as_str) {
            Some("credentials") => Ok(credentials()),
            Some("alpaca-trades") => alpaca_trades(&args[1..]),
            Some("fmp-bars") => fmp_bars(&args[1..]),
            Some("check") => check(&args[1..]),
            Some("alpaca-batch") => alpaca_batch(&args[1..]),
            Some("alpaca-assets") => alpaca_assets(),
            Some("alpaca-daily-closes") => alpaca_daily_closes(&args[1..]),
            Some("maintain") => maintain(&args[1..]),
            Some("research-universe") => research::universe(&args[1..]),
            Some("research-pilot") => research::pilot(&args[1..]),
            Some("research-manifest") => research::manifest(&args[1..]),
            _ => Ok(usage()),
        }
    };
    match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn archive() -> Result<Archive, String> {
    ArchiveConfig::from_lookup(|k| std::env::var(k).ok())
        .map(Archive::new)
        .map_err(|e| e.to_string())
}

fn symbol_and_day(args: &[String]) -> Result<(String, NaiveDate), String> {
    let symbol = args
        .first()
        .ok_or("missing <SYMBOL>")?
        .trim()
        .to_ascii_uppercase();
    if !market_archive::valid_symbol(&symbol) {
        return Err(format!("invalid symbol {symbol:?}"));
    }
    let day = args.get(1).ok_or("missing <YYYY-MM-DD>")?;
    let day =
        NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|_| format!("invalid day {day:?}"))?;
    Ok((symbol, day))
}

/// A completed trading date: a trading day strictly before today in New York.
fn completed_trading_day(day: NaiveDate) -> Result<(), String> {
    let today = Utc::now().with_timezone(&New_York).date_naive();
    if day >= today {
        return Err(format!(
            "{day} is not a completed session (today in New York is {today})"
        ));
    }
    if !is_trading_day(day) {
        return Err(format!("{day} is not a trading day"));
    }
    Ok(())
}

fn credentials() -> ExitCode {
    let show = |name: &str| {
        println!(
            "{name:<22} {}",
            if env(name).is_some() {
                "set"
            } else {
                "not set"
            }
        )
    };
    for n in [
        "ALPACA_API_KEY",
        "ALPACA_API_SECRET",
        "APCA_API_KEY_ID",
        "APCA_API_SECRET_KEY",
        "ALPACA_DATA_BASE",
        "ALPACA_TRADING_BASE",
        "FMP_API_KEY",
        "FMP_BASE",
        "ARCHIVE_ROOT",
        "PRICE_BAND_MIN",
        "PRICE_BAND_MAX",
    ] {
        show(n);
    }
    println!("(values are never printed)");
    ExitCode::SUCCESS
}

fn alpaca_creds() -> Result<AlpacaCredentials, String> {
    AlpacaCredentials::from_lookup(env).ok_or_else(|| {
        "ALPACA_API_KEY and ALPACA_API_SECRET are not set in this environment".into()
    })
}

fn price_band() -> Result<PriceBand, String> {
    market_providers::import::maintain::price_band_from_lookup(env)
}

fn alpaca_trades(args: &[String]) -> Result<ExitCode, String> {
    let (symbol, day) = symbol_and_day(args)?;
    completed_trading_day(day)?;
    let feed = Feed::parse(&flag(args, "--feed").unwrap_or_else(|| "sip".into()))?;
    let limit: u32 = flag(args, "--limit")
        .map(|s| s.parse().map_err(|_| format!("invalid --limit {s:?}")))
        .transpose()?
        .unwrap_or(alpaca::MAX_LIMIT);
    let sessions = sessions_for(day);
    let kinds: &[SessionKind] = match feed {
        Feed::Boats | Feed::Overnight => &[SessionKind::Overnight],
        _ => &[SessionKind::Pre, SessionKind::Regular, SessionKind::After],
    };
    let chosen: Vec<_> = sessions
        .iter()
        .filter(|(id, ..)| kinds.contains(&id.kind))
        .collect();
    let (start, end) = match (chosen.first(), chosen.last()) {
        (Some(a), Some(b)) => (a.1.to_utc(), b.2.to_utc()),
        _ => return Err(format!("{day} has no {feed:?} sessions in the calendar")),
    };
    let creds = alpaca_creds()?;
    let base = env("ALPACA_DATA_BASE").unwrap_or_else(|| "https://data.alpaca.markets".into());
    let archive = archive()?;
    let query = TradesQuery {
        symbol: symbol.clone(),
        day,
        start,
        end,
        feed,
        limit,
    };
    println!(
        "Alpaca {} trades {symbol} {day}: {} .. {} ({} .. {} ET), limit {limit}/page, into {}",
        feed.as_str(),
        alpaca::rfc3339(start),
        alpaca::rfc3339(end),
        start.with_timezone(&New_York).format("%H:%M"),
        end.with_timezone(&New_York).format("%H:%M"),
        archive.day_dir(day).display()
    );
    let mut http = ReqwestHttp::new()?;
    let opts = ImportOptions {
        retry: RetryPolicy::default(),
        force: args.iter().any(|a| a == "--force"),
    };
    let report = import_trades(
        &mut http,
        &base,
        &creds,
        &query,
        &archive,
        &opts,
        &mut |d: Duration| std::thread::sleep(d),
        &mut |line: &str| eprintln!("  {line}"),
    )
    .map_err(|e| e.to_string())?;
    if report.skipped {
        println!("already imported (complete, same request): nothing fetched");
    }
    println!(
        "pages {}  (empty {})  events {}  out-of-order {}",
        report.pages, report.empty_pages, report.events, report.out_of_order
    );
    if let (Some(a), Some(b)) = (report.first_ts, report.last_ts) {
        println!("first {}  last {}", et(a), et(b));
    }
    if !report.updates.is_empty() {
        println!(
            "provider update markers (kept raw; canceled/incorrect excluded from candles): {:?}",
            report.updates
        );
    }
    if let Some(e) = &report.entry {
        println!("file {}  {} B  sha256 {}", e.file, e.bytes, e.sha256);
    }
    Ok(ExitCode::SUCCESS)
}

fn fmp_bars(args: &[String]) -> Result<ExitCode, String> {
    let (symbol, day) = symbol_and_day(args)?;
    completed_trading_day(day)?;
    let key = env("FMP_API_KEY").ok_or("FMP_API_KEY is not set in this environment")?;
    let base = env("FMP_BASE").unwrap_or_else(|| "https://financialmodelingprep.com".into());
    let archive = archive()?;
    let force = args.iter().any(|a| a == "--force");
    if !force
        && archive
            .manifest(day)
            .map_err(|e| e.to_string())?
            .is_some_and(|m| {
                m.bars
                    .contains_key(&BarEntry::key(fmp::PROVIDER, fmp::GRANULARITY, &symbol))
            })
    {
        println!(
            "FMP 1-minute bars for {symbol} {day} already stored: nothing fetched (--force to refetch)"
        );
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "FMP 1-minute bars {symbol} {day} ({})",
        fmp::public_query(&symbol, day)
    );
    let mut http = ReqwestHttp::new()?;
    let ds = fetch_minute_bars(
        &mut http,
        &base,
        &FmpKey(key),
        &symbol,
        day,
        RetryPolicy::default(),
        &mut |d: Duration| std::thread::sleep(d),
        &mut |line: &str| eprintln!("  {line}"),
    )
    .map_err(|e| e.to_string())?;
    let entry = archive.write_bars(day, &ds).map_err(|e| e.to_string())?;
    println!(
        "{} bars ({}) stored at {}  sha256 {}. Minute bars: comparison only, never trades.",
        entry.bars,
        match (entry.first_ts, entry.last_ts) {
            (Some(a), Some(b)) => format!("{} .. {}", et(a), et(b)),
            _ => "none".into(),
        },
        entry.file,
        entry.sha256
    );
    Ok(ExitCode::SUCCESS)
}

fn check(args: &[String]) -> Result<ExitCode, String> {
    let (symbol, day) = symbol_and_day(args)?;
    let archive = archive()?;
    let verify = archive.verify(day).map_err(|e| e.to_string())?;
    let mut ok = verify.ok();
    for s in &verify.symbols {
        if s.problems.is_empty() {
            println!("verify ok    {}", s.symbol);
        } else {
            for p in &s.problems {
                println!("verify FAIL  {}: {p}", s.symbol);
            }
        }
    }
    let m = archive
        .manifest(day)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no manifest for {day}"))?;
    let entry = m
        .symbols
        .get(&symbol)
        .ok_or_else(|| format!("{symbol} is not in {day}"))?;
    match &entry.import {
        Some(p) => println!(
            "import       {} {} feed={} {} .. {}  pages {}  complete {}  imported {}",
            p.provider, p.dataset, p.feed, p.start, p.end, p.pages, p.complete, p.imported_at
        ),
        None => println!("import       none recorded (fixture or recording)"),
    }
    let (summary, snaps) = replay_day(&archive, day, &symbol).map_err(|e| e.to_string())?;
    println!(
        "replay       {} events, deterministic across two runs: {}",
        summary.events, summary.deterministic
    );
    ok &= summary.deterministic;
    for t in &summary.timeframes {
        println!(
            "  {:<4} {:>5} candles  {:>4} gap markers  volume {}",
            serde_json::to_value(t.tf)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default(),
            t.candles,
            t.gaps,
            t.volume
        );
    }
    println!(
        "profile      session {}  total {}  POC {}",
        summary.profile_anchor.as_deref().unwrap_or("-"),
        summary.profile_total,
        summary
            .profile_poc
            .map_or("-".into(), |p| format!("{p:.4}"))
    );
    println!(
        "statuses     {} in the log, {} halts derived. Historical trades carry no halt/resume events: halts are not covered; LULD bands shown are modelled from trades.",
        summary.statuses, summary.halts
    );
    println!(
        "excluded     {} prints by condition or provider update",
        summary.condition_excluded
    );
    let band = price_band()?;
    if let Some(m1) = snaps
        .iter()
        .find(|s| s.timeframe == chart_engine::Timeframe::M1)
        && let Some(last) = m1.candles.last()
    {
        let p = Price::from_f64_rounded(last.c).unwrap_or(Price::from_cents(0));
        println!(
            "price band   configured {}..={} (unchanged); last 1m close {} is {}",
            band.min,
            band.max,
            p,
            if band.contains(p) {
                "inside"
            } else {
                "OUTSIDE"
            }
        );
        if let Some(ds) = archive
            .read_bars(day, fmp::PROVIDER, fmp::GRANULARITY, &symbol)
            .map_err(|e| e.to_string())?
        {
            let c = compare_minutes(m1, &ds.bars);
            println!(
                "FMP vs Alpaca 1m (diagnostic): alpaca {} min, fmp {} min, both {}, only alpaca {}, only fmp {}",
                c.alpaca_minutes, c.fmp_minutes, c.both, c.only_alpaca, c.only_fmp
            );
            println!(
                "  shared minutes: OHLC equal {}, close equal {}, volume equal {}; volume alpaca {} vs fmp {}",
                c.ohlc_equal, c.close_equal, c.volume_equal, c.alpaca_volume, c.fmp_volume
            );
            for d in &c.examples {
                println!(
                    "  {}  alpaca {:?}  fmp {:?}",
                    et(d.t_ms * 1_000_000),
                    d.alpaca,
                    d.fmp
                );
            }
        } else {
            println!("FMP bars     none stored for {symbol} {day} (run `swimport fmp-bars`)");
        }
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Name and listing exchange of every active US equity into `$ARCHIVE_ROOT/assets.json`, which the
/// server reads for the ticker search (and reloads when it changes). Replaces the file
/// atomically; safe to re-run.
fn alpaca_assets() -> Result<ExitCode, String> {
    use market_providers::import::batch::asset_metadata;
    let creds = alpaca_creds()?;
    let trading_base =
        env("ALPACA_TRADING_BASE").unwrap_or_else(|| "https://paper-api.alpaca.markets".into());
    let archive = archive()?;
    let mut http = ReqwestHttp::new()?;
    let assets = asset_metadata(&mut http, &trading_base, &creds, &mut |l: &str| {
        eprintln!("  {l}")
    })?;
    if assets.is_empty() {
        // An empty answer is far more likely a wrong endpoint than a market with no stocks; keep
        // whatever file is there rather than wipe every name.
        return Err("the assets endpoint returned no assets; assets.json left unchanged".into());
    }
    let path = archive.write_assets(&assets).map_err(|e| e.to_string())?;
    let mut by_exchange: std::collections::BTreeMap<&str, usize> = Default::default();
    for m in assets.values() {
        *by_exchange.entry(m.exchange.as_str()).or_default() += 1;
    }
    println!(
        "{} assets written to {} (by exchange: {by_exchange:?}). A running server reloads it within ARCHIVE_META_CHECK_SECS (60 s).",
        assets.len(),
        path.display()
    );
    Ok(ExitCode::SUCCESS)
}

/// `--from` / `--to` as completed trading dates (`--to` strictly before today in New York): a
/// daily bar of a session still running is a partial bar, and its "close" is just the last print.
fn completed_range(args: &[String]) -> Result<(NaiveDate, NaiveDate), String> {
    let day = |name: &str| -> Result<NaiveDate, String> {
        let v = flag(args, name).ok_or_else(|| format!("missing {name} <YYYY-MM-DD>"))?;
        NaiveDate::parse_from_str(&v, "%Y-%m-%d").map_err(|_| format!("invalid {name} {v:?}"))
    };
    let (from, to) = (day("--from")?, day("--to")?);
    if from > to {
        return Err("--from is after --to".into());
    }
    let today = Utc::now().with_timezone(&New_York).date_naive();
    if to >= today {
        return Err(format!(
            "--to {to} is not a completed session (today in New York is {today})"
        ));
    }
    Ok((from, to))
}

/// Official daily closes of every active US equity over a range, merged into
/// `$ARCHIVE_ROOT/daily-closes.json` (market_archive::closes), which the server reads (and reloads)
/// to quote change % against the previous close. To replay day D, the range must include the
/// trading day before D; for live mode it must be re-run after every session (`swimport maintain`
/// does that daily). Re-running a range refetches and overwrites those dates; other dates in the
/// file are kept.
fn alpaca_daily_closes(args: &[String]) -> Result<ExitCode, String> {
    use market_archive::merge_daily_closes;
    use market_providers::import::batch::daily_closes;
    let (from, to) = completed_range(args)?;
    let creds = alpaca_creds()?;
    let data_base = env("ALPACA_DATA_BASE").unwrap_or_else(|| "https://data.alpaca.markets".into());
    let trading_base =
        env("ALPACA_TRADING_BASE").unwrap_or_else(|| "https://paper-api.alpaca.markets".into());
    let archive = archive()?;
    // Read before fetching: an unreadable existing file stops the run instead of being replaced
    // by just this range (which would silently drop every other date).
    let mut all = archive
        .read_daily_closes()
        .map_err(|e| format!("{e} (fix or remove it; it is not overwritten)"))?
        .unwrap_or_default();
    println!("daily closes {from} .. {to} (SIP 1Day bars, raw)");
    let mut http = ReqwestHttp::new()?;
    let new = daily_closes(
        &mut http,
        &trading_base,
        &data_base,
        &creds,
        from,
        to,
        &mut |l: &str| eprintln!("  {l}"),
    )?;
    if new.is_empty() {
        // No trading date in range, or an endpoint answering nothing: keep the file as it is.
        return Err(format!(
            "no daily bars for {from} .. {to}; daily-closes.json left unchanged"
        ));
    }
    let fetched_days = new.len();
    let changed = merge_daily_closes(&mut all, new);
    let path = archive
        .write_daily_closes(&all)
        .map_err(|e| e.to_string())?;
    println!(
        "{fetched_days} trading dates fetched, {changed} closes added or changed; {} dates in {} ({} .. {}). A running server reloads it within ARCHIVE_META_CHECK_SECS (60 s).",
        all.len(),
        path.display(),
        all.keys().next().map_or("-".into(), |d| d.to_string()),
        all.keys().next_back().map_or("-".into(), |d| d.to_string()),
    );
    Ok(ExitCode::SUCCESS)
}

/// Every symbol inside the configured price band on each completed trading day of a range.
fn alpaca_batch(args: &[String]) -> Result<ExitCode, String> {
    use market_providers::import::batch::{plan_universe, run_batch};
    let (from, to) = completed_range(args)?;
    let jobs: usize = flag(args, "--jobs").map_or(Ok(12), |v| {
        v.parse().map_err(|_| format!("invalid --jobs {v:?}"))
    })?;
    let per_minute: u32 = flag(args, "--per-minute").map_or(Ok(9000), |v| {
        v.parse().map_err(|_| format!("invalid --per-minute {v:?}"))
    })?;
    let creds = alpaca_creds()?;
    let data_base = env("ALPACA_DATA_BASE").unwrap_or_else(|| "https://data.alpaca.markets".into());
    let trading_base =
        env("ALPACA_TRADING_BASE").unwrap_or_else(|| "https://paper-api.alpaca.markets".into());
    let band = price_band()?;
    let archive = archive()?;
    let plans = archive.root().join("plans");
    std::fs::create_dir_all(&plans).map_err(|e| format!("{}: {e}", plans.display()))?;

    println!(
        "planning {from} .. {to}, band {}..={} (daily close), SIP",
        band.min, band.max
    );
    let mut http = ReqwestHttp::new()?;
    let plan = plan_universe(
        &mut http,
        &trading_base,
        &data_base,
        &creds,
        from,
        to,
        band,
        &mut |l: &str| eprintln!("  {l}"),
    )?;
    let plan_path = plans.join(format!("universe-{from}-{to}.json"));
    std::fs::write(
        &plan_path,
        serde_json::to_vec_pretty(&plan).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", plan_path.display()))?;
    let mut days: Vec<NaiveDate> = plan.tasks.iter().map(|t| t.day).collect();
    days.sort();
    days.dedup();
    let symbols: std::collections::BTreeSet<&str> =
        plan.tasks.iter().map(|t| t.symbol.as_str()).collect();
    println!(
        "plan: {} assets, {} trading days, {} symbol-days across {} symbols, {} trades, ~{} requests; saved {}",
        plan.assets,
        days.len(),
        plan.tasks.len(),
        symbols.len(),
        plan.trades(),
        plan.requests(alpaca::MAX_LIMIT),
        plan_path.display()
    );
    if !plan.skipped_symbols.is_empty() {
        println!(
            "not storable as archive file names (skipped): {:?}",
            plan.skipped_symbols
        );
    }
    if args.iter().any(|a| a == "--plan-only") {
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "importing with {jobs} workers, at most {per_minute} requests/min, into {}",
        archive.root().display()
    );
    let report = run_batch(
        &plan,
        &ReqwestHttp::new,
        &data_base,
        &creds,
        &archive,
        jobs,
        per_minute,
        Duration::from_secs(30),
        &|line: &str| println!("{line}"),
        &|line: &str| eprintln!("  {line}"),
    );
    let failures_path = plans.join(format!("failures-{from}-{to}.json"));
    std::fs::write(
        &failures_path,
        serde_json::to_vec_pretty(&report.failed).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", failures_path.display()))?;
    println!(
        "done in {:.0}s: {} tasks, {} imported, {} already complete, {} failed ({}), {} events, {} requests",
        report.elapsed.as_secs_f64(),
        report.tasks,
        report.imported,
        report.skipped,
        report.failed.len(),
        failures_path.display(),
        report.events,
        report.requests
    );
    if !report.failed.is_empty() {
        println!(
            "re-run the same command to retry the failed symbol-days; completed ones are skipped"
        );
    }
    Ok(if report.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Sets `stop` on SIGTERM (compose stop) or Ctrl-C. A current-thread runtime on its own thread:
/// the importer is blocking code, and this is the one place it needs async.
fn watch_signals(stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("{} signal handling unavailable: {e}", stamp());
                return;
            }
        };
        rt.block_on(async {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                match signal(SignalKind::terminate()) {
                    Ok(mut term) => {
                        tokio::select! {
                            _ = tokio::signal::ctrl_c() => {}
                            _ = term.recv() => {}
                        }
                    }
                    Err(_) => {
                        let _ = tokio::signal::ctrl_c().await;
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        });
        stop.store(true, Ordering::Relaxed);
        println!(
            "{} stop requested: running symbol-days finish and commit, no new ones start",
            stamp()
        );
    });
}

fn stamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Sleeps `d` in short slices; false as soon as `stop` is set.
fn sleep_or_stop(stop: &AtomicBool, d: Duration) -> bool {
    let until = std::time::Instant::now() + d;
    while !stop.load(Ordering::Relaxed) {
        let now = std::time::Instant::now();
        if now >= until {
            return true;
        }
        std::thread::sleep((until - now).min(Duration::from_millis(500)));
    }
    false
}

/// The archive maintainer (import::maintain): a cycle now, then one daily at `MAINTAIN_AT` ET,
/// until SIGTERM. `--once` runs one cycle and exits (non-zero unless it was clean), for a manual
/// catch-up. Missing Alpaca keys are not fatal: the process keeps running (so compose does not
/// restart it in a loop), imports nothing, deletes nothing, and says so every cycle.
fn maintain(args: &[String]) -> Result<ExitCode, String> {
    use market_providers::import::maintain::{
        MaintainConfig, Maintainer, next_run, status_path, write_status,
    };
    let cfg = MaintainConfig::from_lookup(env)?;
    let creds = AlpacaCredentials::from_lookup(env);
    let archive = archive()?;
    let once = args.iter().any(|a| a == "--once");
    let stop = Arc::new(AtomicBool::new(false));
    watch_signals(stop.clone());
    println!(
        "{} maintain: archive {}, window {} trading days, daily at {} ET, {} workers, <= {}/min, band {}..={}, server {} (token {}), Alpaca keys {}, free-space floor {} MB, status {}",
        stamp(),
        archive.root().display(),
        cfg.window_days,
        cfg.run_at.format("%H:%M"),
        cfg.jobs,
        cfg.per_minute,
        cfg.band.min,
        cfg.band.max,
        cfg.server_url,
        if cfg.api_token.is_some() {
            "set"
        } else {
            "not set"
        },
        if creds.is_some() { "set" } else { "NOT SET" },
        archive.config().min_free_bytes / (1024 * 1024),
        status_path(&archive).display(),
    );
    let make = ReqwestHttp::new;
    let wait = |d: Duration| sleep_or_stop(&stop, d);
    let log = |l: &str| println!("{} {l}", stamp());
    loop {
        let m = Maintainer {
            cfg: &cfg,
            creds: creds.as_ref(),
            archive: &archive,
            make_http: &make,
            stop: &stop,
            wait: &wait,
            log: &log,
        };
        let mut report = m.run_cycle(Utc::now());
        let next = next_run(Utc::now(), cfg.run_at);
        report.next_run = Some(
            next.with_timezone(&New_York)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        );
        if let Err(e) = write_status(&archive, &report) {
            report.errors.push(format!("status file: {e}"));
        }
        println!("{} {}", stamp(), report.summary());
        if once {
            return Ok(if report.outcome == "ok" {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let ahead = (next - Utc::now()).to_std().unwrap_or(Duration::ZERO);
        if !sleep_or_stop(&stop, ahead) {
            break;
        }
    }
    println!("{} maintain stopped", stamp());
    Ok(ExitCode::SUCCESS)
}

/// The research acquisition commands (see the module docs and `import::pilot`).
mod research {
    use super::{alpaca_creds, env, flag};
    use chrono::{NaiveDate, Utc};
    use market_providers::import::alpaca::rfc3339;
    use market_providers::import::http::{Http, RetryPolicy};
    use market_providers::import::paced_client::PacedHttp;
    use market_providers::import::pilot::{
        Budget, Kind, Metered, PilotConfig, SymbolDay, append_run, read_runs, run_pilot,
        write_atomic, write_manifest,
    };
    use market_providers::import::reference::{
        STRUCTURAL_ACTION_TYPES, assets_url, bar_rows, corporate_actions_url, daily_bars_url,
        et_date_range, fetch_pages, invalid_symbol_in, plausible_ticker,
        summarize_corporate_actions,
    };
    use serde_json::{Value, json};
    use std::collections::{BTreeMap, HashSet};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;
    use std::time::{Duration, Instant};

    fn out_dir(args: &[String]) -> Result<PathBuf, String> {
        let out = PathBuf::from(flag(args, "--out").ok_or("missing --out <DIR>")?);
        std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        Ok(out)
    }

    fn num<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> Result<T, String> {
        flag(args, name)
            .map(|s| s.parse().map_err(|_| format!("invalid {name} {s:?}")))
            .transpose()
            .map(|v| v.unwrap_or(default))
    }

    fn date(args: &[String], name: &str) -> Result<NaiveDate, String> {
        let s = flag(args, name).ok_or_else(|| format!("missing {name} <YYYY-MM-DD>"))?;
        NaiveDate::parse_from_str(&s, "%Y-%m-%d").map_err(|_| format!("invalid {name} {s:?}"))
    }

    fn data_base() -> String {
        env("ALPACA_DATA_BASE").unwrap_or_else(|| "https://data.alpaca.markets".into())
    }

    fn trading_base() -> String {
        env("ALPACA_TRADING_BASE").unwrap_or_else(|| "https://paper-api.alpaca.markets".into())
    }

    /// Active download time already spent by earlier runs (the budget is cumulative).
    fn used_active_ms(out: &Path) -> u64 {
        read_runs(out)
            .iter()
            .filter_map(|r| r.get("active_ms").and_then(Value::as_u64))
            .sum()
    }

    fn log(line: &str) {
        eprintln!("  {line}");
    }

    fn to_json_bytes(v: &Value) -> Result<Vec<u8>, String> {
        serde_json::to_vec(v).map_err(|e| e.to_string())
    }

    /// Active and inactive asset snapshots, raw daily bars of every one of them over the range
    /// (for the in-band universe estimate and the prior-day selection), and the structural
    /// corporate actions of the whole market over the range.
    pub fn universe(args: &[String]) -> Result<ExitCode, String> {
        let out = out_dir(args)?;
        let (from, to) = (date(args, "--from")?, date(args, "--to")?);
        let per_minute: u32 = num(args, "--per-minute", 100)?;
        let creds = alpaca_creds()?;
        let (dbase, tbase) = (data_base(), trading_base());
        // Snapshot files are named by the ET date they were taken on.
        let today = Utc::now()
            .with_timezone(&chrono_tz::America::New_York)
            .date_naive();
        let started = Instant::now();
        let started_at = rfc3339(Utc::now());
        let mut paced = PacedHttp::new(per_minute)?;
        let mut sleep = |d: Duration| std::thread::sleep(d);
        let mut lg = |l: &str| log(l);
        let mut m = Metered::new(&mut paced, None);

        let mut lists: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for status in ["active", "inactive"] {
            m.set_kind(&format!("assets-{status}"));
            let url = assets_url(&tbase, status);
            let pages = fetch_pages(&mut m, &creds, &|_| url.clone(), &mut sleep, &mut lg)?;
            let mut syms: Vec<String> = pages
                .iter()
                .flat_map(|p| p.as_array().cloned().unwrap_or_default())
                .filter_map(|a| a.get("symbol").and_then(Value::as_str).map(str::to_string))
                .collect();
            syms.sort();
            syms.dedup();
            println!("{status} assets: {}", syms.len());
            let path = out
                .join("raw")
                .join("assets")
                .join(format!("{status}-{today}.json"));
            let doc = json!({
                "request": { "endpoint": "/v2/assets", "status": status, "asset_class": "us_equity" },
                "fetched_at": rfc3339(Utc::now()),
                "note": "current snapshot of the asset list; Alpaca keeps no asset-list history",
                "pages": pages,
            });
            write_atomic(&path, &to_json_bytes(&doc)?)?;
            lists.insert(status, syms);
        }
        let active: HashSet<&String> = lists["active"].iter().collect();
        let mut all: Vec<String> = lists.values().flatten().cloned().collect();
        all.sort();
        all.dedup();
        let (all, not_tickers): (Vec<String>, Vec<String>) =
            all.into_iter().partition(|s| plausible_ticker(s));
        let mut refused: Vec<String> = Vec::new();

        let (start, end) = et_date_range(from, to).ok_or("calendar does not cover the range")?;
        let bars_path = out
            .join("universe")
            .join(format!("daily-bars_{from}_{to}.jsonl"));
        std::fs::create_dir_all(out.join("universe")).map_err(|e| e.to_string())?;
        let mut f =
            std::io::BufWriter::new(std::fs::File::create(&bars_path).map_err(|e| e.to_string())?);
        m.set_kind("universe-daily-bars");
        let mut n_bars = 0u64;
        for (i, chunk) in all.chunks(200).enumerate() {
            // A whole request fails on one symbol the API refuses: drop it, record it, retry.
            let mut chunk = chunk.to_vec();
            let pages = loop {
                match fetch_pages(
                    &mut m,
                    &creds,
                    &|t| daily_bars_url(&dbase, &chunk, start, end, t),
                    &mut sleep,
                    &mut lg,
                ) {
                    Ok(p) => break p,
                    Err(e) => match invalid_symbol_in(&e) {
                        Some(bad) if chunk.contains(&bad) && refused.len() < 500 => {
                            chunk.retain(|s| *s != bad);
                            refused.push(bad);
                            if chunk.is_empty() {
                                break Vec::new();
                            }
                        }
                        _ => return Err(e),
                    },
                }
            };
            for (sym, bar) in bar_rows(&pages) {
                let mut row = json!({
                    "symbol": sym,
                    "listed": if active.contains(&sym) { "active" } else { "inactive" },
                });
                if let (Some(r), Some(b)) = (row.as_object_mut(), bar.as_object()) {
                    for (k, v) in b {
                        r.insert(k.clone(), v.clone());
                    }
                }
                writeln!(f, "{row}").map_err(|e| e.to_string())?;
                n_bars += 1;
            }
            if i % 10 == 0 {
                log(&format!(
                    "bars: {} / {} symbols, {n_bars} bars",
                    ((i + 1) * 200).min(all.len()),
                    all.len()
                ));
            }
        }
        f.flush().map_err(|e| e.to_string())?;
        drop(f);

        m.set_kind("universe-corporate-actions");
        let pages = fetch_pages(
            &mut m,
            &creds,
            &|t| corporate_actions_url(&dbase, None, STRUCTURAL_ACTION_TYPES, from, to, t),
            &mut sleep,
            &mut lg,
        )?;
        let ca = summarize_corporate_actions(&pages);
        let doc = json!({
            "request": { "endpoint": "/v1/corporate-actions", "types": STRUCTURAL_ACTION_TYPES, "start": from, "end": to },
            "fetched_at": rfc3339(Utc::now()),
            "pages": pages,
        });
        write_atomic(
            &out.join("raw")
                .join("corporate-actions")
                .join(format!("universe_{from}_{to}.json")),
            &to_json_bytes(&doc)?,
        )?;
        println!("corporate actions {from}..{to}: {:?}", ca.counts);
        let stats = m.stats.clone();
        drop(m);

        let run = json!({
            "command": "research-universe",
            "started_at": started_at,
            "finished_at": rfc3339(Utc::now()),
            "active_ms": started.elapsed().as_millis() as u64,
            "per_minute_cap": per_minute,
            "concurrency": 1,
            "stats": stats,
            "pacer": paced.pacer_report(),
            "wire_bytes": paced.wire_bytes,
            "summary": {
                "from": from, "to": to,
                "assets_active": lists["active"].len(),
                "assets_inactive": lists["inactive"].len(),
                "symbols_queried": all.len() - refused.len(),
                "symbols_not_tickers_skipped": not_tickers,
                "symbols_refused_by_api": refused,
                "daily_bars": n_bars,
                "corporate_actions": ca.counts,
            },
        });
        write_atomic(
            &out.join("universe").join("universe.json"),
            &serde_json::to_vec_pretty(&run).map_err(|e| e.to_string())?,
        )?;
        append_run(&out, &run)?;
        println!("{}", serde_json::to_string_pretty(&run).unwrap_or_default());
        Ok(ExitCode::SUCCESS)
    }

    /// Runs the pilot over the selection file's `symbol_days` (at most 12).
    pub fn pilot(args: &[String]) -> Result<ExitCode, String> {
        let out = out_dir(args)?;
        let sel_path = flag(args, "--selection").ok_or("missing --selection <FILE>")?;
        let sel: Value = serde_json::from_slice(
            &std::fs::read(&sel_path).map_err(|e| format!("{sel_path}: {e}"))?,
        )
        .map_err(|e| format!("{sel_path}: {e}"))?;
        let tasks: Vec<SymbolDay> = serde_json::from_value(
            sel.get("symbol_days")
                .cloned()
                .ok_or("selection has no symbol_days")?,
        )
        .map_err(|e| format!("symbol_days: {e}"))?;
        if tasks.len() > 12 {
            return Err(format!(
                "{} symbol-days: the pilot is capped at 12",
                tasks.len()
            ));
        }
        let per_minute: u32 = num(args, "--per-minute", 100)?;
        let budget_minutes: u64 = num(args, "--budget-minutes", 30)?;
        let budget_gb: f64 = num(args, "--budget-gb", 5.0)?;
        let warmup: usize = num(args, "--warmup", 30)?;
        let creds = alpaca_creds()?;
        let dbase = data_base();
        let used = Duration::from_millis(used_active_ms(&out));
        let cfg = PilotConfig {
            out: &out,
            data_base: &dbase,
            creds: &creds,
            budget: Budget {
                active: Duration::from_secs(budget_minutes * 60),
                bytes: (budget_gb * 1e9) as u64,
            },
            used_active: used,
            baseline_bytes: 0,
            warmup_sessions: warmup,
            kinds: Kind::ALL.to_vec(),
            retry: RetryPolicy::default(),
        };
        println!(
            "pilot: {} symbol-days, cap {per_minute}/min, concurrency 1, budget {budget_minutes} min ({:.1} used) / {budget_gb} GB",
            tasks.len(),
            used.as_secs_f64() / 60.0
        );
        let mut paced = PacedHttp::new(per_minute)?;
        let run = run_pilot(
            &mut paced as &mut dyn Http,
            &cfg,
            &tasks,
            &mut |d| std::thread::sleep(d),
            &mut |l| log(l),
        )?;
        let mut v = serde_json::to_value(&run).map_err(|e| e.to_string())?;
        v["command"] = json!("research-pilot");
        v["per_minute_cap"] = json!(per_minute);
        v["concurrency"] = json!(1);
        v["pacer"] = serde_json::to_value(paced.pacer_report()).unwrap_or_default();
        v["wire_bytes"] = json!(paced.wire_bytes);
        append_run(&out, &v)?;
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        Ok(ExitCode::SUCCESS)
    }

    /// (Re)writes `<DIR>/MANIFEST.json`, merging the JSON object in `--meta` if given.
    pub fn manifest(args: &[String]) -> Result<ExitCode, String> {
        let out = out_dir(args)?;
        let meta: Value = match flag(args, "--meta") {
            Some(p) => serde_json::from_slice(&std::fs::read(&p).map_err(|e| format!("{p}: {e}"))?)
                .map_err(|e| format!("{p}: {e}"))?,
            None => json!({}),
        };
        let m = write_manifest(&out, meta)?;
        println!(
            "MANIFEST.json revision {} ({} files)",
            m["revision"],
            m["files"].as_array().map_or(0, |a| a.len())
        );
        Ok(ExitCode::SUCCESS)
    }
}
