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
use market_events::UnixNanos;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

#[derive(Clone, Debug)]
pub struct ReplayOptions {
    /// Where the clock starts (clamped to the day); `None` = the day's first session start.
    pub start: Option<UnixNanos>,
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
}

/// Control and state handle; cheap to clone.
#[derive(Clone)]
pub struct ReplayHandle {
    tx: mpsc::Sender<(Command, Option<oneshot::Sender<ClockState>>)>,
    state: watch::Receiver<ClockState>,
}

impl ReplayHandle {
    /// Sends a command and waits until it has been applied; returns the resulting state.
    pub async fn command(&self, cmd: Command) -> Result<ClockState, String> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send((cmd, Some(tx)))
            .await
            .map_err(|_| "replay stopped".to_string())?;
        rx.await.map_err(|_| "replay stopped".to_string())
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
        let to = UnixNanos(
            to.nanos()
                .clamp(self.core.start().nanos(), self.core.end().nanos()),
        );
        if let Err(e) = self.core.reset() {
            return self.fail(e).await;
        }
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

    async fn apply(&mut self, cmd: Command) -> Result<(), Closed> {
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
                    return self.fail(e).await;
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
            Command::Seek(t) => return self.seek(t).await,
        }
        self.emit_clock().await
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
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<(Command, Option<oneshot::Sender<ClockState>>)>(64);
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
                    Some((cmd, reply)) => {
                        let r = d.apply(cmd).await;
                        if let Some(reply) = reply {
                            let _ = reply.send(d.clock());
                        }
                        r
                    }
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
