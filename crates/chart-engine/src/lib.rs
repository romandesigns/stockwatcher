//! # chart-engine
//!
//! Turns normalized [`market_events::MarketEvent`]s into 15s / 30s / 1m / 5m candles, gap markers,
//! a per-session volume profile, a counted trade tape and data-quality counters. The same code
//! runs in replay and live mode; clients only render its output ([`wire`]).
//!
//! ```
//! use chart_engine::{ChartEngine, Timeframe};
//! use market_events::{MarketEvent, Price, Trade, session::et};
//! let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
//! let mut engine = ChartEngine::default();
//! let t = Trade::new("SMPL", et(day, 10, 0, 1, 0).unwrap(), Price::from_cents(318), 100, 1);
//! let updates = engine.apply(&MarketEvent::Trade(t));
//! assert!(!updates.is_empty());
//! let snap = engine.snapshot("SMPL", Timeframe::M1, None).unwrap();
//! assert_eq!(snap.candles[0].v, 100);
//! ```
//!
//! # Policies
//!
//! **Intervals.** Half-open `[start, start + tf)`, aligned to the Unix epoch in UTC (ET offsets are
//! whole hours, so this is also ET-aligned). A trade at exactly `10:00:00.000` belongs to the
//! `10:00:00` interval; `09:59:59.999…` to the one before. Every session boundary is a multiple of
//! 5 minutes, so no interval straddles two sessions.
//!
//! **Candles.** O/H/L/C are the first/highest/lowest/last *price-qualifying* trade in ordering-key
//! order; V is the sum of *volume-qualifying* sizes; `n` counts volume-qualifying trades.
//! Qualification is `market_events::trade_effect` (Alpaca's minute-bar condition table, strictest
//! rule wins). All timeframes are maintained from the same accepted trades, so switching
//! timeframe never changes the underlying data: the 15s volumes of a 5-minute window always sum
//! to that window's 5m volume.
//!
//! **Forming → final.** A candle is final once the symbol's watermark (its newest accepted trade
//! time, or the clock from [`ChartEngine::advance_clock`] if later) reaches the interval end. The
//! final version is emitted exactly once at that moment (`final: true`); a later revision (late
//! trade within the settle window, correction, cancel) re-emits it, still final.
//!
//! **Gaps.** An interval with no trades produces **no candle** — never a zero-volume candle, never
//! an interpolated price. Between two intervals of the same session that have data, the hole is a
//! gap marker `no_trades`. An interval holding only volume-only prints (e.g. only odd lots) has
//! volume but no qualifying price: it is a gap marker `volume_only_trades` carrying its volume.
//! No marker is produced across a session boundary or before a session's first trade.
//!
//! **Ordering.** Trades are ordered by `(event_ts, source_seq, exchange, source)`: same-timestamp
//! trades follow the provider sequence. Arrival order does not matter inside the settle window: a
//! trade whose key is below the newest one is inserted in key order and its interval is recomputed
//! exactly from the retained trades (`outOfOrderReordered`).
//!
//! **Late trades.** A trade is accepted iff `watermark < end of its 15s interval + settle window`
//! (default 2 s). The window is measured on the **finest** configured timeframe and the decision
//! applies to all timeframes at once, because the brief requires that switching timeframe never
//! changes the source data; a per-timeframe window would let a 3-second-late print reach the 5m
//! candle but not the 15s one. An accepted late trade goes to its own interval (re-emitting that
//! candle) and **never** resets or moves the forming candle; only a strictly newer interval opens a
//! new one. A trade past the window is excluded from candles and profile, counted
//! (`lateExcluded`) and shown on the tape as `excluded:late`.
//!
//! **Duplicates.** A print's identity is `(source, exchange, source_seq, event_ts)`
//! (`market_events::PrintId`): the provider id alone is not unique on venue `D`, where FINRA's
//! TRFs reuse ids (docs/architecture/trade-identity.md). A repeat within the retention window is
//! ignored and counted (`duplicates`); beyond retention a repeat would be older than the settle
//! window and is excluded as late anyway.
//!
//! **Corrections and cancels.** Each symbol keeps trade-level records for the retention window
//! (default 10 min, at least the largest timeframe plus the settle window; pruning is aligned down
//! to a 5-minute boundary so every retained interval is complete). A cancel removes the print; a
//! correction replaces it in place (original execution time, `corrected_id`, corrected
//! price/size/conditions re-run through the condition policy). The affected interval of every
//! timeframe is recomputed exactly, and the profile subtracts/adds exact per-price volume. A
//! correction or cancel names its target by provider id plus the original's price and size
//! (`market_events::TradeRef`); it is applied only to the one retained print with that id whose
//! price and size match. A target not in retention is counted (`correctionsUnapplied` /
//! `cancelsUnapplied`); one that fits several retained prints, or none of those holding its id
//! (venue `D`), is counted (`correctionsAmbiguous` / `cancelsAmbiguous`). Either way it changes
//! nothing. Corrections and cancels are not subject to the settle
//! window: they may revise final candles. [`ChartEngine::apply_revising`] also returns the engine's
//! own account of an applied one ([`Revision`]: the print removed, its replacement, what each
//! counted for), so a running tally outside the engine (the server's quote board) can follow the
//! candles exactly instead of re-deriving the rules.
//!
//! **Volume profile.** One per symbol per session id (`"2026-10-15/pre"`, `…/regular`, …), so
//! sessions never mix and the anchor does not depend on the timeframe. Volume is kept per exact
//! price; bins are `floor(price / bin) * bin` anchored at zero, so any bin width is exact. A trade
//! adds to the profile if it qualifies for price (regular sale) **or** its only restriction is the
//! odd-lot flag `I`: odd lots are real executions at the market price, and the profile is "actual
//! traded volume by price". Other volume-only prints (average-price `B`/`W`, prior-reference `P`,
//! derivatively priced `4`, …) count in candle volume but are left out of the profile, because
//! their price is not where the shares traded at that moment. Consequently a session's profile
//! total can be lower than the sum of its candle volumes. POC is the largest bin (lowest price on a
//! tie); the value area is 70%, grown from the POC towards the larger neighbouring bin.
//!
//! **Outside sessions.** A trade whose time is in no session (weekend, holiday, a closed evening)
//! is excluded and counted (`outsideSession`).
//!
//! **Halts.** A symbol's `Halt` status opens a halt at its `event_ts`, `Resume` closes it
//! (`chart.halt` both times; clients upsert by `start`). A second `Halt` while halted and a
//! `Resume` with nothing open are ignored and counted (`haltsUnmatched`); market-wide statuses
//! and every other kind change nothing. Halts are timeframe-independent and are dropped once
//! they ended before every retained interval.
//!
//! **LULD bands.** Modelled, not the exchange's numbers: see [`luld`] for the model and the
//! pure band function. Points are stamped at the time of the change (the closing doubling
//! boundary, the close, a halt, 09:35 without an opening print, the end of a reference's
//! 30-second hold, or the print that moved the reference), whoever notices it first (the clock or
//! a trade), so play, step and a seek rebuild give the same list. The previous close and listing
//! venue the Plan needs come from [`ChartEngine::set_luld_inputs`]. Being published as trades
//! arrive, the bands (unlike candles) depend on arrival order inside the settle window.
//!
//! **Memory.** Per symbol: a ring buffer of `max_intervals_per_timeframe` intervals per timeframe,
//! trade records for the retention window, and profiles/counters for `sessions_retained`
//! sessions. No thread or timer per symbol: `advance_clock` walks a min-heap of pending interval
//! ends and touches only symbols that have something to finalize. Once a symbol's latest trading
//! date reaches its regular open, earlier dates' candles and profiles are dropped
//! ([`EngineConfig::drop_prior_dates_at_open`]): the rings are counted in intervals, and would
//! otherwise keep up to twenty days of 5 m candles. A candle that has left the retention window
//! keeps no trade key. Measured on a synthetic whole-market day (~40M prints, 12k symbols):
//! ~16M candles, ~3.1 GB of heap after the close, 3.7 GB at the peak (`examples/memory.rs`).

mod config;
mod engine;
pub mod indicators;
pub mod levels;
pub mod luld;
pub mod profile;
mod quality;
mod timeframe;
pub mod wire;

pub use config::EngineConfig;
pub use engine::{
    ChartEngine, DayExtremes, DayHigh, DayLine, EngineCost, RevisedPrint, Revision,
    VALUE_AREA_PERCENT, session_schedule,
};
pub use quality::DataQuality;
pub use timeframe::Timeframe;
