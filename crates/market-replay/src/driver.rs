//! The async replay driver: one tokio task, wall-clock pacing, commands, bounded output.
//!
//! No per-symbol tasks: one task merges every symbol. Output goes through a bounded channel, so a
//! slow consumer slows replay down (back-pressure) instead of growing memory; at max speed the
//! consumer's pace *is* the replay speed.

use crate::core::ReplayCore;
use crate::marks::DEFAULT_MARK_STEP_NS;
use crate::pacing::{Speed, paced_target};
use crate::source::EventSource;
use crate::{ClockMode, ClockState, FeedItem};
use chrono::{NaiveDate, NaiveTime};
use market_events::UnixNanos;
use market_events::session::et_instant;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

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
        let (tx, rx) = oneshot::channel();
        self.tx
            .send((cmd, Some(tx)))
            .await
            .map_err(|_| CommandError::Stopped)?;
        rx.await
            .map_err(|_| CommandError::Stopped)?
            .map_err(CommandError::Rejected)
    }

    pub fn state(&self) -> ClockState {
        self.state.borrow().clone()
    }

    pub fn watch(&self) -> watch::Receiver<ClockState> {
        self.state.clone()
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
}

struct Closed;

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
        for item in self.buf.drain(..) {
            self.out.send(item).await.map_err(|_| Closed)?;
        }
        Ok(())
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

    /// Advances to `until` in batches, flushing each.
    async fn run_to(&mut self, until: UnixNanos) -> Result<(), Closed> {
        loop {
            match self.core.advance(until, self.opts.batch, &mut self.buf) {
                Ok(reached) => {
                    self.flush().await?;
                    if reached {
                        return Ok(());
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
        match self.speed {
            Speed::Max => match self
                .core
                .advance(self.core.end(), self.opts.batch, &mut self.buf)
            {
                Ok(_) => self.flush().await?,
                Err(e) => return self.fail(e).await,
            },
            Speed::Times(_) => {
                let target = paced_target(
                    self.anchor_virtual,
                    self.anchor_wall.elapsed(),
                    self.speed,
                    self.core.end().nanos(),
                );
                self.run_to(UnixNanos(target)).await?;
            }
        }
        if self.core.finished() {
            self.playing = false;
            return self.emit_clock().await;
        }
        if self.last_clock.elapsed() >= self.opts.clock_every {
            self.emit_clock().await?;
        }
        Ok(())
    }
}

/// Starts the replay task. The first items are a `Reset`, the catch-up to `opts.start`, and a
/// `Clock`.
pub fn spawn(
    source: Arc<dyn EventSource>,
    opts: ReplayOptions,
) -> Result<(ReplayHandle, mpsc::Receiver<FeedItem>), String> {
    let core = ReplayCore::new(source, opts.mark_step_ns)?;
    let (out, rx) = mpsc::channel(opts.channel.max(16));
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<(Command, Option<Reply>)>(64);
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
        opts,
    };
    d.state.send_replace(d.clock());
    let state = d.state.subscribe();
    tokio::spawn(async move {
        let start = d.opts.start.unwrap_or(d.core.start());
        if d.seek(start).await.is_err() {
            return;
        }
        let mut commands_open = true;
        loop {
            let wait = match d.speed {
                Speed::Max => Duration::ZERO,
                Speed::Times(_) => d.opts.tick,
            };
            let res = tokio::select! {
                biased;
                cmd = cmd_rx.recv(), if commands_open => match cmd {
                    Some((cmd, reply)) => match d.apply(cmd).await {
                        Ok(applied) => {
                            if let Some(reply) = reply {
                                let _ = reply.send(applied.map(|()| d.clock()));
                            }
                            Ok(())
                        }
                        Err(Closed) => Err(Closed),
                    },
                    None => {
                        commands_open = false;
                        Ok(())
                    }
                },
                _ = tokio::time::sleep(wait), if d.playing => d.tick().await,
                _ = d.out.closed() => Err(Closed),
            };
            if res.is_err() {
                break;
            }
        }
        tracing::debug!("replay task stopped (consumer gone)");
    });
    Ok((ReplayHandle { tx: cmd_tx, state }, rx))
}
