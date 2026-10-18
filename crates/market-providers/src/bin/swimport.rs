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
//! ```
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
  swimport maintain [--once]"
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
