use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use stockwatcher_server::{AppState, COMMIT, Config, Markets, Settings, VERSION, router};
use tracing_subscriber::EnvFilter;

/// mimalloc instead of glibc's malloc, for the server binary only (tests keep the system one).
///
/// Why (2026-10-22): the server's RSS climbed from ~7 GB after a restart to ~24 GB by the close
/// and never came back down, also on days its live heap could not account for that. Its heap is
/// many small, mixed-lifetime allocations made and freed across a dozen threads: a whole-market
/// open fills the trade-retention windows (chart engine, scanner ledger, held events of symbols
/// still loading) several times over what they hold an hour later, interleaved with candle
/// buckets that live all day. glibc keeps per-thread arenas and only returns memory at an
/// arena's top, so the open's high-water mark stayed resident for the rest of the day — the
/// "+7 GB in the first half hour, then never down" shape production showed. mimalloc frees
/// whole pages back to the OS as they empty and does not strand freed memory in another
/// thread's arena. Measured live heap and production RSS are compared in the commit that added
/// this; `crates/server/tests/memory_bench.rs` reproduces the heap side.
#[global_allocator]
static GLOBAL: Counted = Counted;

/// mimalloc, counting the bytes the program holds (`live_heap_mb` in the `memory` line).
///
/// Why (2026-10-28): RSS grew 1–3 GB an hour all day and all night until the kernel killed the
/// process (twice in 24 h), while every structure `memory_report` names stayed flat and the
/// benches' live heap stayed bounded. mimalloc's commit cannot tell a leak from memory the
/// allocator keeps, so production had no way to say which one it was. The count is sharded so
/// the dozen threads allocating at the open do not all bounce one cache line.
struct Counted;

#[repr(align(128))]
struct Shard(std::sync::atomic::AtomicIsize);

const SHARDS: usize = 32;
static HELD: [Shard; SHARDS] = [const { Shard(std::sync::atomic::AtomicIsize::new(0)) }; SHARDS];
static NEXT_SHARD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

thread_local! {
    // `const`: initializing it must not allocate (this runs inside the allocator).
    static SHARD: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

/// Live bytes by allocation size class, per shard (`heap_by_size` in the `memory` line).
///
/// Why (2026-10-28): with the new counters production's live heap grew ~2 GB an hour after
/// the preload while every structure the line names grew by tens of MB. The size of what
/// accumulates narrows down the type that holds it (a 48-byte node, a 4 KiB buffer, ...)
/// without a profiler in the distroless image.
const CLASSES: usize = 128;
static BY_SIZE: [[std::sync::atomic::AtomicIsize; CLASSES]; SHARDS] =
    [const { [const { std::sync::atomic::AtomicIsize::new(0) }; CLASSES] }; SHARDS];

/// 16-byte steps up to 512 bytes, then four steps per power of two.
fn size_class(n: usize) -> usize {
    if n <= 512 {
        return n.div_ceil(16);
    }
    let p = (usize::BITS - 1 - n.leading_zeros()) as usize;
    let sub = (n >> (p - 2)) & 3;
    (33 + (p - 9) * 4 + sub).min(CLASSES - 1)
}

/// The smallest size in a class (the label `heap_by_size` reports it under).
fn class_floor(c: usize) -> usize {
    if c <= 32 {
        return c.saturating_sub(1) * 16 + 1;
    }
    let p = (c - 33) / 4 + 9;
    // 512 itself is the last 16-byte class.
    ((1 << p) + ((c - 33) % 4) * (1 << (p - 2))).max(513)
}

fn shard() -> usize {
    use std::sync::atomic::Ordering::Relaxed;
    SHARD
        .try_with(|s| {
            if s.get() == usize::MAX {
                s.set(NEXT_SHARD.fetch_add(1, Relaxed) % SHARDS);
            }
            s.get()
        })
        // A thread being torn down has no thread-locals left; shard 0 still counts it.
        .unwrap_or(0)
}

/// Live allocations by **exact** size from 4 KiB to 64 KiB (`heap_exact` in the `memory` line).
///
/// Why (2026-10-29): `heap_by_size` put all of production's post-preload growth (~16 MB a
/// minute, ~800 allocations never freed) in the 12-32 KiB classes. A Vec or VecDeque buffer is
/// capacity x element size, so the exact sizes that pile up give the element size, and that
/// names the type. The image has no symbols for a sampled backtrace.
const EXACT_MIN: usize = 4096;
const EXACT_MAX: usize = 65536;
static EXACT: [std::sync::atomic::AtomicI32; EXACT_MAX - EXACT_MIN] =
    [const { std::sync::atomic::AtomicI32::new(0) }; EXACT_MAX - EXACT_MIN];

fn count(size: usize, sign: isize) {
    use std::sync::atomic::Ordering::Relaxed;
    let i = shard();
    HELD[i].0.fetch_add(sign * size as isize, Relaxed);
    BY_SIZE[i][size_class(size)].fetch_add(sign * size as isize, Relaxed);
    if (EXACT_MIN..EXACT_MAX).contains(&size) {
        EXACT[size - EXACT_MIN].fetch_add(sign as i32, Relaxed);
    }
}

/// Who holds the 12-32 KiB allocations (`memory sites` log line).
///
/// Why (2026-10-29): production's growth after the preload is all in those sizes, about 800
/// blocks a minute, spread over many exact sizes (`heap_exact` stayed flat). So these are
/// exact-length copies of variable payloads, and their size cannot name the owner. Every
/// `MEM_SITES_EVERY`-th allocation in the range (default 256; 0 = off) records its call stack
/// until it is freed. Every 10th `memory` line then names the stacks whose samples have lived
/// longest. The table is capped, so a leak cannot make the profiler itself grow without limit.
/// The server binary keeps its symbol table (`[profile.release.package.stockwatcher-server]`)
/// so the stacks resolve in the distroless image.
mod sites {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    pub const SIZES: std::ops::Range<usize> = 12_288..32_768;
    const CAP: usize = 16_384;
    pub static EVERY: AtomicUsize = AtomicUsize::new(0);
    static TICK: AtomicUsize = AtomicUsize::new(0);
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    type Table = HashMap<usize, (Instant, usize, Arc<std::backtrace::Backtrace>)>;
    static SAMPLED: Mutex<Option<Table>> = Mutex::new(None);

    thread_local! {
        static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Runs `f` unless this thread is already inside the profiler (its own allocations).
    fn guarded(f: impl FnOnce()) {
        if BUSY.try_with(|b| b.replace(true)).unwrap_or(true) {
            return;
        }
        f();
        let _ = BUSY.try_with(|b| b.set(false));
    }

    pub fn on_alloc(p: *mut u8, size: usize) {
        let every = EVERY.load(Relaxed);
        if every == 0
            || !SIZES.contains(&size)
            || !TICK.fetch_add(1, Relaxed).is_multiple_of(every)
            || LIVE.load(Relaxed) >= CAP
        {
            return;
        }
        guarded(|| {
            let bt = Arc::new(std::backtrace::Backtrace::force_capture());
            if let Ok(mut t) = SAMPLED.lock() {
                t.get_or_insert_with(HashMap::new)
                    .insert(p as usize, (Instant::now(), size, bt));
                LIVE.store(t.as_ref().map_or(0, HashMap::len), Relaxed);
            }
        });
    }

    pub fn on_free(p: *mut u8, size: usize) {
        if LIVE.load(Relaxed) == 0 || !SIZES.contains(&size) {
            return;
        }
        guarded(|| {
            if let Ok(mut t) = SAMPLED.lock()
                && let Some(m) = t.as_mut()
            {
                m.remove(&(p as usize));
                LIVE.store(m.len(), Relaxed);
            }
        });
    }

    /// The `top` call stacks holding the most sampled allocations older than `age`:
    /// `(samples, bytes, frames)`, the frames cut to this workspace's own functions.
    pub fn report(age: std::time::Duration, top: usize) -> (usize, Vec<(usize, usize, String)>) {
        let mut out = (0, Vec::new());
        guarded(|| {
            let held: Vec<(usize, Arc<std::backtrace::Backtrace>)> = SAMPLED
                .lock()
                .ok()
                .and_then(|t| {
                    t.as_ref().map(|m| {
                        m.values()
                            .filter(|(at, _, _)| at.elapsed() >= age)
                            .map(|(_, size, bt)| (*size, bt.clone()))
                            .collect()
                    })
                })
                .unwrap_or_default();
            out.0 = LIVE.load(Relaxed);
            let mut by: HashMap<String, (usize, usize)> = HashMap::new();
            for (size, bt) in held {
                let frames = bt
                    .to_string()
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.starts_with("at "))
                    .filter_map(|l| l.split_once(": ").map(|(_, f)| f))
                    .filter(|f| {
                        [
                            "stockwatcher",
                            "chart_engine",
                            "stock_context",
                            "market_",
                            "provider_sim",
                        ]
                        .iter()
                        .any(|c| f.contains(c))
                            && !f.contains("::sites::")
                            && !f.contains("Counted")
                    })
                    .take(8)
                    .collect::<Vec<_>>()
                    .join(" < ");
                let e = by.entry(frames).or_default();
                e.0 += 1;
                e.1 += size;
            }
            let mut v: Vec<_> = by.into_iter().map(|(f, (n, b))| (n, b, f)).collect();
            v.sort_by_key(|&(n, _, _)| std::cmp::Reverse(n));
            v.truncate(top);
            out.1 = v;
        });
        out
    }
}

/// The exact sizes (4-64 KiB) with the most live allocations: `[size, count]`, most first.
fn heap_exact(top: usize) -> Vec<(usize, i32)> {
    let mut held: Vec<(usize, i32)> = EXACT
        .iter()
        .enumerate()
        .map(|(i, n)| (i + EXACT_MIN, n.load(std::sync::atomic::Ordering::Relaxed)))
        .filter(|&(_, n)| n > 0)
        .collect();
    held.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    held.truncate(top);
    held
}

/// Bytes the program holds right now (allocated and not yet freed), across every thread.
fn live_heap() -> isize {
    HELD.iter()
        .map(|s| s.0.load(std::sync::atomic::Ordering::Relaxed))
        .sum()
}

/// The size classes holding the most, largest first: `[smallest size in class, MB held]`.
fn heap_by_size(top: usize) -> Vec<(usize, isize)> {
    let mut held: Vec<(usize, isize)> = (0..CLASSES)
        .map(|c| {
            let bytes: isize = BY_SIZE
                .iter()
                .map(|s| s[c].load(std::sync::atomic::Ordering::Relaxed))
                .sum();
            (class_floor(c), bytes / 1_000_000)
        })
        .collect();
    held.sort_by_key(|&(_, mb)| std::cmp::Reverse(mb));
    held.truncate(top);
    held
}

// SAFETY: every call is forwarded unchanged to mimalloc; the counting only reads the layout.
unsafe impl std::alloc::GlobalAlloc for Counted {
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        count(l.size(), 1);
        let p = unsafe { mimalloc::MiMalloc.alloc(l) };
        sites::on_alloc(p, l.size());
        p
    }
    unsafe fn alloc_zeroed(&self, l: std::alloc::Layout) -> *mut u8 {
        count(l.size(), 1);
        let p = unsafe { mimalloc::MiMalloc.alloc_zeroed(l) };
        sites::on_alloc(p, l.size());
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        count(l.size(), -1);
        sites::on_free(p, l.size());
        unsafe { mimalloc::MiMalloc.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, new: usize) -> *mut u8 {
        count(l.size(), -1);
        count(new, 1);
        sites::on_free(p, l.size());
        let q = unsafe { mimalloc::MiMalloc.realloc(p, l, new) };
        sites::on_alloc(q, new);
        q
    }
}

fn main() -> ExitCode {
    // The runtime image is distroless (no shell, no curl), so the Docker
    // HEALTHCHECK runs this binary again with --healthcheck.
    if std::env::args().any(|a| a == "--healthcheck") {
        return healthcheck();
    }
    // Any other argument is a mistake, never a reason to start: on 2026-10-26 a `--help` typed
    // into the live container started a second full server against the same data volume.
    if let Some(arg) = std::env::args().nth(1) {
        eprintln!(
            "stockwatcher-server takes no arguments (got {arg:?}); configuration is by environment \
             (crates/server/src/config.rs). --healthcheck probes a running server."
        );
        return ExitCode::from(2);
    }

    // One server per DATA_DIR, taken before anything opens a file there (logs included). The lock
    // goes with the process, so a deploy's stop-then-start is unaffected.
    let data = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".into());
    let _data_lock = match stockwatcher_server::lock_data_dir(std::path::Path::new(&data)) {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("stockwatcher-server: {e}");
            return ExitCode::FAILURE;
        }
    };

    init_tracing();
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("invalid configuration: {e}");
            return ExitCode::FAILURE;
        }
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    match runtime.block_on(serve(config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("server error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// stdout as before (`LOG_FORMAT=json` in the image), plus the same events as JSON lines in
/// `DATA_DIR/logs/server-<ET date>.jsonl` (crate::oplog), which survive deploys. `SERVER_LOG=0`
/// turns the file off. DATA_DIR is read here directly: tracing starts before the configuration is
/// parsed, so a configuration error is logged to the file too.
fn init_tracing() {
    use stockwatcher_server::oplog;
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let json = std::env::var("LOG_FORMAT").is_ok_and(|f| f == "json");
    let stdout = if json {
        tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer().boxed()
    };
    let file = (std::env::var("SERVER_LOG").as_deref() != Ok("0")).then(|| {
        let data = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".into());
        let log = oplog::DailyLog::start(
            "server",
            oplog::DailyLogConfig {
                dir: std::path::PathBuf::from(data).join(oplog::SERVER_LOG_DIR),
                prefix: "server-",
                retention_days: oplog::SERVER_LOG_DAYS,
                cap_bytes: oplog::SERVER_LOG_CAP,
            },
        );
        tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_ansi(false)
            .with_writer(oplog::ServerLogWriter(log))
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(stdout)
        .with(file)
        .init();
}

async fn serve(config: Config) -> std::io::Result<()> {
    std::fs::create_dir_all(&config.data_dir)?;
    if config.api_token.is_none() {
        tracing::warn!("STOCKWATCHER_API_TOKEN is not set: auth is OFF (local dev only)");
    }
    if !config.static_dir.join("index.html").is_file() {
        tracing::warn!(static_dir = %config.static_dir.display(), "no index.html; web app will 404 (run the web build)");
    }

    // Synced settings live on the persistent volume (DATA_DIR = /app/data, the stockwatcher_data
    // volume, in the image), beside the archive but not in it. An unusable directory is fatal,
    // like an unusable DATA_DIR: running on would accept writes it cannot keep.
    let settings = Settings::open(config.data_dir.join("settings"))?;
    // Live and replay side by side (the dashboard reads live, the chart page practises on replay).
    let (markets, sim) = Markets::start(&config.market);
    spawn_memory_stats(&markets);
    // Price alerts watch the live market (never replay), with their rules in the same store and
    // Activity + the registered phones under DATA_DIR/alerts.
    let alerts = stockwatcher_server::alerts::Alerts::start(
        settings.clone(),
        markets.live.clone(),
        Some(config.data_dir.join("alerts")),
        stockwatcher_server::alerts::push::PushConfig::from_lookup(|k| std::env::var(k).ok()),
        // Every decided notice, beside rocket-watch on the archive (alerts::ledger).
        stockwatcher_server::alerts::ledger::Ledger::start(&config.market.provider.archive.root),
    );
    // The Favorites panel's list is a synced setting the server also reads (crate::favorites).
    stockwatcher_server::favorites::follow(
        settings.clone(),
        vec![markets.live.clone(), markets.replay.clone()],
    );
    // The finished days of every operational log, copied to the Pi beside the preserved
    // originals (crate::oplog, "Durable copy").
    {
        use stockwatcher_server::oplog::{self, DurableSource};
        let archive = &config.market.provider.archive.root;
        stockwatcher_server::oplog::spawn_durable_copy(
            vec![
                DurableSource {
                    name: "rocket-watch",
                    dir: archive.join("rocket-watch"),
                },
                DurableSource {
                    name: "zones",
                    dir: archive.join("zones"),
                },
                DurableSource {
                    name: oplog::NOTICES_DIR,
                    dir: archive.join(oplog::NOTICES_DIR),
                },
                DurableSource {
                    name: oplog::TRANSITIONS_DIR,
                    dir: archive.join(oplog::TRANSITIONS_DIR),
                },
                DurableSource {
                    name: "server",
                    dir: config.data_dir.join(oplog::SERVER_LOG_DIR),
                },
            ],
            std::env::var("LOG_DURABLE_DIR").ok(),
            std::env::var("LIVE_ORIGINAL_DURABLE_DIR").ok(),
        );
    }
    let state = AppState::with_alerts(config.api_token.clone(), markets, settings, alerts);
    // The Insights page reads the same ledger back (GET /api/notices, crate::notices).
    state.set_notices_dir(
        config
            .market
            .provider
            .archive
            .root
            .join(stockwatcher_server::oplog::NOTICES_DIR),
    );
    if let Some(sim) = sim {
        // The provider-compatible simulator on its own port (loopback by default), so Alpaca/FMP
        // base URLs can point at it by configuration. Dummy credentials only.
        let addr = SocketAddr::new(sim.config().host, sim.config().port);
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                tracing::info!(%addr, "provider simulator listening (Alpaca/FMP subset, dummy auth)");
                let mut stop = state.shutdown_signal();
                tokio::spawn(async move {
                    let res = axum::serve(listener, sim.router())
                        .with_graceful_shutdown(async move {
                            let _ = stop.changed().await;
                        })
                        .await;
                    if let Err(e) = res {
                        tracing::error!(error = %e, "provider simulator stopped");
                    }
                });
            }
            Err(e) => {
                tracing::error!(%addr, error = %e, "provider simulator could not bind; continuing without it")
            }
        }
    }
    let app = router(state.clone(), &config.static_dir, &config.data_dir);
    let addr = SocketAddr::new(config.host, config.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, version = VERSION, commit = COMMIT, data_dir = %config.data_dir.display(), "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_requested().await;
            tracing::info!("shutdown requested; draining connections");
            state.close_sessions(Duration::from_secs(5)).await;
            // Inside compose's 15 s stop grace: sessions (≤5 s), then the recording (≤5 s), so
            // a deploy loses none of the day's recorded events.
            state.close_recording(Duration::from_secs(5)).await;
        })
        .await?;
    tracing::info!("stopped");
    Ok(())
}

/// `MEM_STATS_SECS=<n>` (unset or 0 = off): every `n` seconds, one `memory` log line with
/// mimalloc's view of the process (RSS, committed memory and their peaks) and what the live
/// market's per-symbol structures hold (`Market::memory_report`).
///
/// Why (2026-10-23): production RSS reached 19 GB with no way to tell the heap from what the
/// kernel had attached to it. The host's transparent huge pages had collapsed ~11 GB of the
/// process into 2 MiB pages, which is invisible from inside without this. The process cannot
/// read its own `smaps_rollup` in the distroless image either, so `AnonHugePages` is read from
/// `/proc/self/smaps_rollup` when it can be and left out when it cannot.
fn spawn_memory_stats(markets: &Markets) {
    let secs: u64 = std::env::var("MEM_STATS_SECS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    if secs == 0 {
        return;
    }
    let live = markets.live.clone();
    let guard = MemoryGuard::from_env();
    // The replay market shares the process (and the owner practises on it): growth there would
    // otherwise be invisible in a line that only described the live one.
    let replay = markets.replay.clone();
    sites::EVERY.store(
        std::env::var("MEM_SITES_EVERY")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(256),
        std::sync::atomic::Ordering::Relaxed,
    );
    let spawned = std::thread::Builder::new()
        .name("memory-stats".into())
        .spawn(move || {
            let mut lines = 0u64;
            loop {
                lines += 1;
                if lines.is_multiple_of(10) && sites::EVERY.load(std::sync::atomic::Ordering::Relaxed) > 0 {
                    let (sampled, top) = sites::report(Duration::from_secs(600), 5);
                    for (n, bytes, frames) in top {
                        tracing::info!(sampled, samples = n, bytes, frames = %frames, "memory sites");
                    }
                }
                let (mut rss, mut peak_rss, mut commit, mut peak_commit) = (0, 0, 0, 0);
                // SAFETY: every out-pointer is a valid, writable usize or null.
                unsafe {
                    libmimalloc_sys::mi_process_info(
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        &mut rss,
                        &mut peak_rss,
                        &mut commit,
                        &mut peak_commit,
                        std::ptr::null_mut(),
                    );
                }
                let huge_kb = std::fs::read_to_string("/proc/self/smaps_rollup")
                    .ok()
                    .and_then(|s| {
                        s.lines()
                            .find(|l| l.starts_with("AnonHugePages:"))
                            .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
                    });
                if let Some(reason) = memory_guard(rss / 1_000_000, et_minute_now(), &guard) {
                    tracing::warn!(
                        rss_mb = rss / 1_000_000,
                        reason,
                        "memory guard: restarting cleanly"
                    );
                    memory_restart().notify_one();
                }
                // The kernel's current resident set (mimalloc's `rss` is its commit on Linux, and
                // `peak_rss` only ever rises): the second field of statm, in pages.
                let vm_rss_mb = std::fs::read_to_string("/proc/self/statm")
                    .ok()
                    .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
                    .map(|pages| pages * 4096 / 1_000_000);
                tracing::info!(
                    live_heap_mb = live_heap() / 1_000_000,
                    heap_by_size = ?heap_by_size(12),
                    heap_exact = ?heap_exact(20),
                    vm_rss_mb,
                    rss_mb = rss / 1_000_000,
                    peak_rss_mb = peak_rss / 1_000_000,
                    commit_mb = commit / 1_000_000,
                    peak_commit_mb = peak_commit / 1_000_000,
                    anon_huge_mb = huge_kb.map(|k| k / 1000),
                    live = %live.memory_report(),
                    replay = %replay.memory_report(),
                    "memory"
                );
                std::thread::sleep(Duration::from_secs(secs));
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "MEM_STATS_SECS: could not start the stats thread");
    }
}

/// The live server's memory grows without bound until the cause is found (2026-10-28: ~7.5 GB after
/// start, +1–5 GB/h, killed at 38.7 GB on the 31 GB host twice, the second time at 10:51 ET in
/// session, dropping 22,945 events). Until then the server restarts itself cleanly rather than be
/// killed: the same graceful path as a deploy (sessions drained, recording written), and compose's
/// `restart: unless-stopped` brings it straight back.
///
/// - Before the pre-market (03:30–03:55 ET) above `MEM_GUARD_PREOPEN_MB` (default 16,000): a fresh
///   process for the session, at the quietest time of the day.
/// - At any time above `MEM_GUARD_HARD_MB` (default 30,000): a clean stop beats an OOM kill.
///
/// `MEM_GUARD=0` turns both off. It rides the `MEM_STATS_SECS` thread, so it runs only where the
/// memory line is on (production). Remove once the growth is fixed.
#[derive(Clone, Copy, Debug)]
struct MemoryGuard {
    on: bool,
    preopen_mb: usize,
    hard_mb: usize,
}

impl MemoryGuard {
    fn from_env() -> Self {
        let mb = |k: &str, d: usize| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(d)
        };
        Self {
            on: std::env::var("MEM_GUARD").as_deref() != Ok("0"),
            preopen_mb: mb("MEM_GUARD_PREOPEN_MB", 16_000),
            hard_mb: mb("MEM_GUARD_HARD_MB", 30_000),
        }
    }
}

/// Minutes after ET midnight.
fn et_minute_now() -> u32 {
    use chrono::Timelike;
    let t = chrono::Utc::now().with_timezone(&chrono_tz::America::New_York);
    t.hour() * 60 + t.minute()
}

/// Why the process should restart now, if it should (see [`MemoryGuard`]).
fn memory_guard(rss_mb: usize, et_minute: u32, g: &MemoryGuard) -> Option<&'static str> {
    const PREOPEN: std::ops::Range<u32> = (3 * 60 + 30)..(3 * 60 + 55);
    if !g.on {
        return None;
    }
    if rss_mb >= g.hard_mb {
        return Some("hard_limit");
    }
    if PREOPEN.contains(&et_minute) && rss_mb >= g.preopen_mb {
        return Some("before_premarket");
    }
    None
}

fn memory_restart() -> &'static tokio::sync::Notify {
    static N: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

async fn shutdown_requested() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
        _ = memory_restart().notified() => {}
    }
}

fn healthcheck() -> ExitCode {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8790);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let check = || -> std::io::Result<bool> {
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.write_all(
            b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )?;
        let mut body = String::new();
        stream.read_to_string(&mut body)?;
        Ok(body.starts_with("HTTP/1.1 200") && body.contains(r#""status":"ok""#))
    };
    match check() {
        Ok(true) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampled_sites_name_who_holds_memory_until_it_is_freed() {
        sites::EVERY.store(1, std::sync::atomic::Ordering::Relaxed);
        #[inline(never)]
        fn hold_buffers() -> Vec<Vec<u8>> {
            (0..20).map(|i| vec![1u8; 16_000 + i]).collect()
        }
        let held = hold_buffers();
        let (_, top) = sites::report(Duration::ZERO, 5);
        assert!(top.iter().map(|t| t.0).sum::<usize>() >= 20, "{top:?}");
        // Unoptimized, the frames resolve to the holder itself (under LTO this test inlines
        // into the harness; the server's real call chains keep their names).
        if cfg!(debug_assertions) {
            let mine = top.iter().find(|(_, _, f)| f.contains("hold_buffers"));
            assert!(mine.is_some_and(|m| m.0 >= 20), "{top:?}");
        }
        drop(held);
        let (sampled, top) = sites::report(Duration::ZERO, 5);
        assert_eq!(top.iter().map(|t| t.0).sum::<usize>(), sampled, "{top:?}");
        assert!(sampled < 20, "freed samples leave the table: {sampled}");
        sites::EVERY.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn a_size_lands_in_the_class_its_floor_names() {
        for n in (1..5000).chain([1 << 20, (1 << 20) + 1, 3 << 20, 1 << 30]) {
            let c = size_class(n);
            assert!(
                class_floor(c) <= n,
                "{n}: class {c} floor {}",
                class_floor(c)
            );
            if c + 1 < CLASSES {
                assert!(
                    class_floor(c + 1) > n,
                    "{n}: next class {}",
                    class_floor(c + 1)
                );
            }
        }
    }

    const G: MemoryGuard = MemoryGuard {
        on: true,
        preopen_mb: 16_000,
        hard_mb: 30_000,
    };

    #[test]
    fn memory_guard_restarts_before_the_premarket_or_at_the_hard_limit() {
        let at = |h: u32, m: u32| h * 60 + m;
        assert_eq!(
            memory_guard(17_000, at(3, 40), &G),
            Some("before_premarket")
        );
        assert_eq!(memory_guard(15_000, at(3, 40), &G), None);
        assert_eq!(memory_guard(17_000, at(3, 55), &G), None);
        assert_eq!(memory_guard(25_000, at(11, 0), &G), None);
        assert_eq!(memory_guard(30_000, at(11, 0), &G), Some("hard_limit"));
        let off = MemoryGuard { on: false, ..G };
        assert_eq!(memory_guard(40_000, at(3, 40), &off), None);
    }
}
