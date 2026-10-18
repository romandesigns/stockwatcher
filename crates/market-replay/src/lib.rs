//! # market-replay
//!
//! Deterministic replay of archived [`market_events::MarketEvent`]s on a controllable virtual
//! clock: play, pause, step (one event, or to the next interval boundary), seek, and speed
//! (1×, 2×, 10×, 60×, … or max).
//!
//! # Ordering (what "deterministic" means)
//!
//! Each symbol's file is read in **arrival order** (the archive never re-sorts). Every event gets a
//! *delivery time*: the running maximum, within its symbol stream, of `receive_ts` (or `event_ts`
//! when the provider path did not record a receive time). The streams are merged k-way by
//!
//! `(delivery, event_ts, source_seq, exchange, source, symbol, stream index)`
//!
//! For a stream with no `receive_ts` and time-sorted records (a historical import) this is exactly
//! `(event_ts, source_seq, exchange, source)`. For a live recording it reproduces the arrival
//! order, so a print that arrived late is replayed late and the chart engine makes the same
//! late/out-of-order decision it made live. The merge is a pure function of the files: the same
//! archive always yields the same event sequence, independent of speed, pauses or wall time.
//!
//! # The engine clock (determinism of derived state)
//!
//! Besides events, replay emits [`FeedItem::Advance`] marks: the engine clock
//! (`ChartEngine::advance_clock`), quantized to whole seconds ([`marks::MarkClock`]). Before an
//! event delivered at `D`, the engine has always been advanced to exactly `floor_1s(D)`; at the end
//! of a step to virtual time `T`, to `floor_1s(T)`. Wall pacing only decides *when* items are
//! released and how consecutive marks coalesce, never their values relative to events, so the
//! engine state at any virtual time `T` is identical whether `T` was reached by playing at any
//! speed, stepping, or seeking. [`FeedItem::Clock`] (the display clock, wall-paced, ~4 Hz) never
//! touches the engine.
//!
//! # Virtual time
//!
//! "Now" means: every event with delivery ≤ now has been emitted. Playing at speed `m` moves now by
//! `m ×` wall time from the last anchor (set on play, speed change, step and seek). Max speed
//! emits as fast as the consumer accepts (bounded channel), in batches, and sets now to the last
//! delivered event. Seek to `T` emits [`FeedItem::Reset`], then re-reads the day from the start and
//! emits every item up to `T` (consumers rebuild their state silently and resume on the next
//! [`FeedItem::Clock`]); that is how a seek rebuilds the engine deterministically from the session
//! start rather than patching state. Step-interval moves now to the next multiple of the interval
//! (inclusive, like everything else: events delivered exactly at the boundary are emitted).
//!
//! # Switching days
//!
//! [`Command::Day`] swaps the driver's [`EventSource`] for the same origin's source of another day
//! ([`EventSource::for_day`]) and then runs exactly the seek path above: [`FeedItem::Source`]
//! (what the new day is: label, symbols, fixture), [`FeedItem::Reset`], the catch-up to the
//! configured ET start time-of-day ([`ReplayOptions::start_time`]) on the new day, and the closing
//! [`FeedItem::Clock`]. So a consumer that already handles a seek handles a day switch with one
//! extra item, and one provider session (one channel, one handle) serves every day. The new source
//! is opened before anything is emitted: a day that cannot be opened is refused with nothing
//! changed. The old source, its open readers and its merger are dropped at the swap; nothing of the
//! old day stays in the driver.

pub mod core;
pub mod driver;
pub mod marks;
pub mod merge;
pub mod pacing;
pub mod source;

pub use crate::core::ReplayCore;
pub use driver::{Command, CommandError, ReplayHandle, ReplayOptions, spawn};
pub use marks::MarkClock;
pub use merge::Merger;
pub use pacing::Speed;
pub use source::{ArchiveDaySource, EventSource, EventStream, SourceInfo, VecSource, day_bounds};

use chrono::NaiveDate;
use market_events::{MarketEvent, UnixNanos};
use serde::Serialize;

/// Who drives the clock. Shared with `market-providers` (live mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClockMode {
    Replay,
    Live,
}

impl ClockMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ClockMode::Replay => "replay",
            ClockMode::Live => "live",
        }
    }
}

/// Clock state, published on every change and periodically while playing.
#[derive(Clone, Debug, PartialEq)]
pub struct ClockState {
    pub mode: ClockMode,
    /// Virtual now (replay) or wall now (live).
    pub now: UnixNanos,
    pub speed: Speed,
    pub playing: bool,
    pub day: Option<NaiveDate>,
    pub start: Option<UnixNanos>,
    pub end: Option<UnixNanos>,
    /// Replay reached the end of the day.
    pub finished: bool,
    /// Events emitted since the last reset.
    pub events: u64,
}

/// What a replay (or any provider) emits, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum FeedItem {
    /// One normalized event.
    Event(MarketEvent),
    /// Advance the engine clock to this instant (deterministic, see the crate docs).
    Advance(UnixNanos),
    /// Display clock / state tick. Never affects derived state.
    Clock(ClockState),
    /// Drop all derived state: the items that follow replay the day from its start up to
    /// `clock.now`, and the next `Clock` ends the rebuild.
    Reset(ClockState),
    /// The source failed (e.g. a corrupt record); replay paused.
    Error(String),
    /// The replay switched to another day ([`Command::Day`]). Always immediately followed by the
    /// `Reset` that rebuilds on it, so a consumer replaces whatever it knows about the day (label,
    /// symbol list, fixture flag) here and drops its derived state on the `Reset` as for a seek.
    /// Not emitted at startup: the session's initial description is the provider's own.
    Source(std::sync::Arc<SourceInfo>),
    /// A live source's connection changed (never emitted by replay). Never affects derived
    /// state: it only tells consumers whether silence means "quiet market" or "no connection".
    Link(Link),
}

/// Connection state of a live source, as reported by [`FeedItem::Link`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkState {
    /// Connecting or authenticating (first connect, or a reconnect after a backoff wait).
    Connecting,
    /// Authenticated and subscribed: events flow (or the market is genuinely quiet).
    Up,
    /// Disconnected or refused; the source is waiting to reconnect. Events in the gap are lost.
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Link {
    pub state: LinkState,
    /// Why, in words an operator can act on (never credentials).
    pub detail: Option<String>,
}
