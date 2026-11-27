//! The async replay driver: one tokio task, wall-clock pacing, commands, bounded output.
//!
//! No per-symbol tasks: one task merges every symbol. Output goes through a bounded channel, so a
//! slow consumer slows replay down (back-pressure) instead of growing memory; at max speed the
//! consumer's pace *is* the replay speed.
//!
//! Play, pause and speed are answered even while that back-pressure holds a paced flush (the
//! Practice-page freeze, 2026-11-26): at the open a whole-market replay's consumer can fall
//! seconds behind, and a command read only between ticks then waited for the whole backlog, which
//! made the page's Play button look dead. Those three only change the pacing, so they apply at
//! once, end the paced catch-up in progress, and are confirmed with a `Clock` as soon as the
//! output has room. Every other command (seek, day, step) still waits for the flush, in order.
//!
//! A replay that falls behind wall time (the consumer slower than the speed asked for) no longer
//! sprints to make it up: each tick runs at most [`MAX_CATCH_UP`] of wall time's worth, and the
//! rest is dropped, so the clock runs slower than asked but steadily, with a `Clock` every
//! `clock_every` meanwhile. Before, the clock froze for the length of the catch-up and then jumped
//! (09:35:34 to 09:35:44 in the owner's recording), and the burst made the consumer later still.

use crate::core::ReplayCore;
use crate::marks::DEFAULT_MARK_STEP_NS;
use crate::pacing::{Speed, paced_target, virtual_elapsed};
use crate::source::EventSource;
use crate::{ClockMode, ClockState, FeedItem};
use chrono::{NaiveDate, NaiveTime};
use market_events::UnixNanos;
use market_events::session::et_instant;
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

/// The most wall time's worth of replay one paced tick runs; a replay further behind than this
/// drops the rest instead of catching up in one burst.
pub const MAX_CATCH_UP: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct ReplayOptions {
    /// Where the clock starts (clamped to the day); `None` = the day's first session start.
    pub start: Option<UnixNanos>,
    /// ET wall time a day switch ([`Command::Day`]) starts the new day at (`REPLAY_START`), so
    /// every day opens at the same time of day the server was configured to start at. `None` = the
    /// new day's first session start. A time that does not exist on the new day (a DST gap) falls
    /// back to that too.
    pub start_time: Option<NaiveTime>,
    pub speed: Speed,
    pub autoplay: bool,
    pub mark_step_ns: i64,
    /// Pacing tick while playing at a multiplier.
    pub tick: Duration,
    /// Display-clock period while playing.
    pub clock_every: Duration,
    /// Events per batch (max speed, catch-up); bounds the work between command checks.
    pub batch: usize,
    /// Output channel capacity.
    pub channel: usize,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        ReplayOptions {
            start: None,
            start_time: None,
            speed: Speed::Times(10),
            autoplay: true,
            mark_step_ns: DEFAULT_MARK_STEP_NS,
            tick: Duration::from_millis(50),
            clock_every: Duration::from_millis(250),
            batch: 2048,
            channel: 8192,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Play,
    Pause,
    /// Pause and emit exactly one event.
    Step,
    /// Pause and move to the next multiple of this interval (ns).
    StepInterval(i64),
    /// Rebuild from the start of the day up to this instant (clamped to the day).
    Seek(UnixNanos),
    Speed(Speed),
    /// Replay another day of the same source ([`EventSource::for_day`]): `Source`, then a rebuild
    /// exactly like a seek, to [`ReplayOptions::start_time`] on the new day. Playing/paused and
    /// the speed carry over. Refused (nothing changes) when the day cannot be opened.
    Day(NaiveDate),
}

/// Why [`ReplayHandle::command`] did not apply a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// The replay task has ended (its consumer went away).
    Stopped,
    /// The command was refused and nothing changed (e.g. a day that is not in the archive).
    Rejected(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::Stopped => f.write_str("replay stopped"),
            CommandError::Rejected(m) => write!(f, "rejected: {m}"),
        }
    }
}

impl std::error::Error for CommandError {}

type Reply = oneshot::Sender<Result<ClockState, String>>;

/// Control and state handle; cheap to clone.
#[derive(Clone)]
pub struct ReplayHandle {
    tx: mpsc::Sender<(Command, Option<Reply>)>,
    state: watch::Receiver<ClockState>,
}

impl ReplayHandle {
    /// Sends a command and waits until it has been applied; returns the resulting state.
    pub async fn command(&self, cmd: Command) -> Result<ClockState, CommandError> {
        self.enqueue(cmd).await?.await
    }

    /// Queues a command (commands apply in the order they were queued) and returns, without
    /// waiting for it to apply, a future that resolves once it has. A WebSocket session awaits
    /// only the queueing, so a replay busy flushing never stalls the connection that sent it.
    pub async fn enqueue(&self, cmd: Command) -> Result<Applied, CommandError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send((cmd, Some(tx)))
            .await
            .map_err(|_| CommandError::Stopped)?;
        Ok(Applied(rx))
    }

    pub fn state(&self) -> ClockState {
        self.state.borrow().clone()
    }

    pub fn watch(&self) -> watch::Receiver<ClockState> {
        self.state.clone()
    }
}

/// A queued command's outcome ([`ReplayHandle::enqueue`]): the state right after it applied.
pub struct Applied(oneshot::Receiver<Result<ClockState, String>>);

impl std::future::Future for Applied {
    type Output = Result<ClockState, CommandError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.0).poll(cx).map(|r| {
            r.map_err(|_| CommandError::Stopped)?
                .map_err(CommandError::Rejected)
        })
    }
}

struct Driver {
    core: ReplayCore,
    opts: ReplayOptions,
    speed: Speed,
    playing: bool,
    anchor_wall: Instant,
    anchor_virtual: i64,
    last_clock: Instant,
    out: mpsc::Sender<FeedItem>,
    state: watch::Sender<ClockState>,
    buf: Vec<FeedItem>,
    cmds: mpsc::Receiver<(Command, Option<Reply>)>,
    cmds_open: bool,
    /// Commands read during a paced flush that must wait for it (seek, day, step), in order;
    /// once one waits, every later command waits behind it, so none overtakes another.
    deferred: VecDeque<(Command, Option<Reply>)>,
    /// A paced tick is running: play, pause and speed apply during its flushes.
    pacing: bool,
    /// Play, pause or speed applied during the paced tick: it stops its catch-up (the target it
    /// was running to belongs to the old pacing) and confirms with a `Clock`.
    repaced: bool,
}

struct Closed;

/// What a flush waiting for room in the output got first.
enum Flushed<'a> {
    Room(Result<mpsc::Permit<'a, FeedItem>, mpsc::error::SendError<()>>),
    Command(Option<(Command, Option<Reply>)>),
}

impl Driver {
    fn clock(&self) -> ClockState {
        ClockState {
            mode: ClockMode::Replay,
            now: self.core.now(),
            speed: self.speed,
            playing: self.playing,
            day: Some(self.core.source().day()),
            start: Some(self.core.start()),
            end: Some(self.core.end()),
            finished: self.core.finished(),
            events: self.core.emitted(),
        }
    }

    async fn flush(&mut self) -> Result<(), Closed> {
        let mut items = std::mem::take(&mut self.buf);
        let out = self.out.clone();
        let mut result = Ok(());
        for item in items.drain(..) {
            let room = loop {
                let got = tokio::select! {
                    biased;
                    room = out.reserve() => Flushed::Room(room),
                    cmd = self.cmds.recv(), if self.pacing && self.cmds_open => Flushed::Command(cmd),
                };
                match got {
                    Flushed::Room(room) => break room,
                    Flushed::Command(cmd) => self.during_flush(cmd),
                }
            };
            match room {
                Ok(permit) => permit.send(item),
                Err(_) => {
                    result = Err(Closed);
                    break;
                }
            }
        }
        // Keep the buffer's capacity (anything left after a close is dropped here).
        items.clear();
        if self.buf.is_empty() {
            self.buf = items;
        }
        result
    }

    /// A command read while a paced flush waits for room. Play, pause and speed apply and are
    /// answered now; anything else (and anything behind a deferred command) waits for the flush.
    fn during_flush(&mut self, cmd: Option<(Command, Option<Reply>)>) {
        let Some((cmd, reply)) = cmd else {
            self.cmds_open = false;
            return;
        };
        if !self.deferred.is_empty() {
            self.deferred.push_back((cmd, reply));
            return;
        }
        match cmd {
            Command::Play => self.playing = !self.core.finished(),
            Command::Pause => self.playing = false,
            Command::Speed(s) => self.speed = s,
            other => {
                self.deferred.push_back((other, reply));
                return;
            }
        }
        self.reanchor();
        self.repaced = true;
        // Readers of the state (`/api/replay/state`, the reply) see it at once; clients get the
        // `Clock` when the tick ends, behind the events already queued.
        let c = self.clock();
        self.state.send_replace(c.clone());
        if let Some(reply) = reply {
            let _ = reply.send(Ok(c));
        }
    }

    async fn emit_clock(&mut self) -> Result<(), Closed> {
        let c = self.clock();
        self.state.send_replace(c.clone());
        self.last_clock = Instant::now();
        self.out.send(FeedItem::Clock(c)).await.map_err(|_| Closed)
    }

    fn reanchor(&mut self) {
        self.anchor_wall = Instant::now();
        self.anchor_virtual = self.core.now().nanos();
    }

    async fn fail(&mut self, err: String) -> Result<(), Closed> {
        tracing::error!(error = %err, "replay source failed; pausing");
        self.playing = false;
        self.buf.clear();
        self.out
            .send(FeedItem::Error(err))
            .await
            .map_err(|_| Closed)?;
        self.emit_clock().await
    }

    /// Advances to `until` in batches, flushing each. A paced run (a tick) stops early once play,
    /// pause or speed changed during one of its flushes.
    async fn run_to(&mut self, until: UnixNanos) -> Result<(), Closed> {
        loop {
            match self.core.advance(until, self.opts.batch, &mut self.buf) {
                Ok(reached) => {
                    self.flush().await?;
                    if reached || (self.pacing && self.repaced) {
                        return Ok(());
                    }
                    // A long paced run still shows the clock moving.
                    if self.pacing && self.last_clock.elapsed() >= self.opts.clock_every {
                        self.emit_clock().await?;
                    }
                }
                Err(e) => return self.fail(e).await,
            }
        }
    }

    async fn seek(&mut self, to: UnixNanos) -> Result<(), Closed> {
        if let Err(e) = self.core.reset() {
            return self.fail(e).await;
        }
        self.rebuild(to).await
    }

    /// The rebuild half of a seek, on a core already at the start of its day: `Reset`, the
    /// catch-up to `to` (clamped to the day), then the closing `Clock`.
    async fn rebuild(&mut self, to: UnixNanos) -> Result<(), Closed> {
        let to = UnixNanos(
            to.nanos()
                .clamp(self.core.start().nanos(), self.core.end().nanos()),
        );
        let mut target = self.clock();
        target.now = to;
        self.out
            .send(FeedItem::Reset(target))
            .await
            .map_err(|_| Closed)?;
        self.run_to(to).await?;
        self.reanchor();
        self.emit_clock().await
    }

    /// Switches to `day`. Everything that can fail (manifest, trading day, opening every symbol
    /// file) happens on the new core before the swap, so a refusal leaves the running day exactly
    /// as it was. Assigning `self.core` drops the old core: its merger, its open readers and the
    /// last `Arc` this task holds on the old source.
    async fn switch_day(&mut self, day: NaiveDate) -> Result<Result<(), String>, Closed> {
        let core = match self
            .core
            .source()
            .for_day(day)
            .and_then(|src| ReplayCore::new(src, self.opts.mark_step_ns))
        {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        self.core = core;
        let to = self
            .opts
            .start_time
            .and_then(|t| et_instant(day, t))
            .unwrap_or(self.core.start());
        self.out
            .send(FeedItem::Source(Arc::new(self.core.source().info())))
            .await
            .map_err(|_| Closed)?;
        // `playing` and `speed` are left alone: the new day plays if the old one was playing. A day
        // that ran to its end had already paused itself, so switching from it lands paused.
        self.rebuild(to).await.map(Ok)
    }

    /// Applies one command. The outer error means the consumer is gone (the task ends); the
    /// inner one is a refusal that changed nothing and goes back to the caller.
    async fn apply(&mut self, cmd: Command) -> Result<Result<(), String>, Closed> {
        match cmd {
            Command::Play => {
                self.playing = !self.core.finished();
                self.reanchor();
            }
            Command::Pause => self.playing = false,
            Command::Speed(s) => {
                self.speed = s;
                self.reanchor();
            }
            Command::Step => {
                self.playing = false;
                if let Err(e) = self.core.step(&mut self.buf) {
                    return self.fail(e).await.map(Ok);
                }
                self.flush().await?;
                self.reanchor();
            }
            Command::StepInterval(len) => {
                self.playing = false;
                let len = len.max(1);
                let next = (self.core.now().nanos().div_euclid(len) + 1) * len;
                self.run_to(UnixNanos(next)).await?;
                self.reanchor();
            }
            Command::Seek(t) => return self.seek(t).await.map(Ok),
            Command::Day(day) => return self.switch_day(day).await,
        }
        self.emit_clock().await.map(Ok)
    }

    async fn tick(&mut self) -> Result<(), Closed> {
        self.pacing = true;
        self.repaced = false;
        let ran = self.paced_step().await;
        self.pacing = false;
        ran?;
        if self.core.finished() {
            self.playing = false;
            return self.emit_clock().await;
        }
        if std::mem::take(&mut self.repaced) || self.last_clock.elapsed() >= self.opts.clock_every {
            self.emit_clock().await?;
        }
        Ok(())
    }

    async fn paced_step(&mut self) -> Result<(), Closed> {
        match self.speed {
            Speed::Max => match self
                .core
                .advance(self.core.end(), self.opts.batch, &mut self.buf)
            {
                Ok(_) => self.flush().await,
                Err(e) => self.fail(e).await,
            },
            Speed::Times(_) => {
                let target = paced_target(
                    self.anchor_virtual,
                    self.anchor_wall.elapsed(),
                    self.speed,
                    self.core.end().nanos(),
                );
                let cap = virtual_elapsed(MAX_CATCH_UP, self.speed)
                    .map_or(target, |v| self.core.now().nanos().saturating_add(v));
                if target <= cap {
                    return self.run_to(UnixNanos(target)).await;
                }
                // Behind wall time by more than MAX_CATCH_UP: run that much, and pace on from here.
                self.run_to(UnixNanos(cap)).await?;
                self.reanchor();
                Ok(())
            }
        }
    }
}

/// What the driver's loop does next.
enum Next {
    Command(Option<(Command, Option<Reply>)>),
    Tick,
    Closed,
}

/// Starts the replay task. The first items are a `Reset`, the catch-up to `opts.start`, and a
/// `Clock`.
pub fn spawn(
    source: Arc<dyn EventSource>,
    opts: ReplayOptions,
) -> Result<(ReplayHandle, mpsc::Receiver<FeedItem>), String> {
    let core = ReplayCore::new(source, opts.mark_step_ns)?;
    let (out, rx) = mpsc::channel(opts.channel.max(16));
    let (cmd_tx, cmd_rx) = mpsc::channel::<(Command, Option<Reply>)>(64);
    let now = Instant::now();
    let mut d = Driver {
        speed: opts.speed,
        playing: opts.autoplay,
        anchor_wall: now,
        anchor_virtual: core.now().nanos(),
        last_clock: now,
        core,
        out,
        state: watch::Sender::new(ClockState {
            mode: ClockMode::Replay,
            now: UnixNanos(0),
            speed: opts.speed,
            playing: false,
            day: None,
            start: None,
            end: None,
            finished: false,
            events: 0,
        }),
        buf: Vec::with_capacity(opts.batch * 2 + 2),
        cmds: cmd_rx,
        cmds_open: true,
        deferred: VecDeque::new(),
        pacing: false,
        repaced: false,
        opts,
    };
    d.state.send_replace(d.clock());
    let state = d.state.subscribe();
    tokio::spawn(async move {
        let start = d.opts.start.unwrap_or(d.core.start());
        if d.seek(start).await.is_err() {
            return;
        }
        loop {
            let wait = match d.speed {
                Speed::Max => Duration::ZERO,
                Speed::Times(_) => d.opts.tick,
            };
            // Commands a paced flush put off come first, in the order they arrived.
            let next = match d.deferred.pop_front() {
                Some(cmd) => Next::Command(Some(cmd)),
                None => tokio::select! {
                    biased;
                    cmd = d.cmds.recv(), if d.cmds_open => Next::Command(cmd),
                    _ = tokio::time::sleep(wait), if d.playing => Next::Tick,
                    _ = d.out.closed() => Next::Closed,
                },
            };
            let res = match next {
                Next::Command(Some((cmd, reply))) => match d.apply(cmd).await {
                    Ok(applied) => {
                        if let Some(reply) = reply {
                            let _ = reply.send(applied.map(|()| d.clock()));
                        }
                        Ok(())
                    }
                    Err(Closed) => Err(Closed),
                },
                Next::Command(None) => {
                    d.cmds_open = false;
                    Ok(())
                }
                Next::Tick => d.tick().await,
                Next::Closed => Err(Closed),
            };
            if res.is_err() {
                break;
            }
        }
        tracing::debug!("replay task stopped (consumer gone)");
    });
    Ok((ReplayHandle { tx: cmd_tx, state }, rx))
}
