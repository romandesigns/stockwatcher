//! x-halt-1 "resumed up after a halt", the detector itself: std only, no server types, so a
//! research harness compiles THIS FILE unchanged (`#[path]`) and replays the research's own
//! inputs through it (the parity harness,
//! `C:/wavystack/outputs/stockwatcher-research-v2/50-ship/run-halt/`). Everything that decides
//! whether x-halt-1 fires lives here; `super` only feeds it prints, halt statuses, a clock and
//! the day's facts, and turns a [`Fire`] into a notice.
//!
//! The research definition is StockWatcher research v2, wave 2, lane x-halt (branch
//! `research/v2-x-halt` 860e7af1, finalist `30-w2/finalists/x-halt-1.json` sha256 `cd040948…`,
//! the frozen walk-forward pick `RU-u0.05-open0-Kany`): `research/v2/x_halt/halt_events.py`
//! (`one`) for the halt facts and `streams.py` (`build_streams`, `_decide`) for the stream,
//! step by step with the research's own arithmetic:
//!
//! 1. **Bars.** 15 s bars and 1 s bars of the day's prints on the 04:00 ET grid, prices in
//!    integer micro-dollars from OHLC-counted prints. A slot with only volume or uncounted prints
//!    is still a row with `c == 0` ("no price"), like the research cache's rows
//!    (`build_cache.py` `bar_frame`: "c 0 = no OHLC print"). [`Builder`] makes them from prints;
//!    the parity harness feeds the cache's own bars instead.
//! 2. **Halt facts** at the halt instant `halt_ns`, from the 15 s bars with a price that END at
//!    or before it (no later bar):
//!    - `halt_px` = the close of the last such bar;
//!    - `low15` = the lowest low of those ending in the last 15 minutes (`end > halt_ns − 15 min`;
//!      with none there, the last bar alone: "flat by definition");
//!    - `high60` = the highest high of those ending in the last 60 s (`end > halt_ns − 60 s`), or
//!      none;
//!    - `run_in = halt_px / low15 − 1` in `f64`.
//! 3. **Eligible halt** (`build_streams`): typed as an **LULD pause or "News pending"**, the
//!    symbol has a previous close, `halt_px` exists, `run_in ≥ 0.05` (`u0.05`), and
//!    `halt_px ≥ 0.99 × high60` when there is a `high60`. Every qualifying halt of the day counts
//!    (`Kany`), one alert per halt.
//! 4. **Reopen** (`one`): from `rfrom` = the listed resume, never before `halt_ns + 1 ns`; the
//!    first bar **with a price** that STARTS at or after `rfrom`, on 1 s bars when the symbol's
//!    1 s rows began at or before `rfrom` (`st1[0] <= rfrom`), else on 15 s bars. `reopen_px` =
//!    that bar's open; `reopen_known_ns` = that bar's END. (The research also had a no-resume
//!    rule, `halt_ns + 5 min`; it produced **0 of the stream's 2,668 Train rows**, and live does
//!    not use it: see "Listed resume" below.)
//! 5. **Decision** (`_decide`, mode `open0`): `reopen_px ≥ halt_px × 1.0`; the alert instant is
//!    `a = reopen_known_ns + 1 s`; the price band **$0.10–$25** (inclusive, micro-dollars) on
//!    `reopen_px`; and **not halted again**: no other halt of the symbol started in
//!    `(halt_ns, a]` (any halt, typed or not). The alert's price is `reopen_px`.
//!
//! # Live inputs, and how they differ from the research's (each counted in the parity report)
//!
//! - **Halt typing.** The research typed the feed's halts by the NYSE historical list
//!   (`is_luld`, `reason`, matched within 120 s). Live types each halt by its own status message
//!   (Alpaca `rc`, `crate::alerts::halts::reason_label`: `LUDP`/`LUDS`/`P` → LULD pause, `T1` →
//!   News pending). [`typed_reason`] is that rule.
//! - **Listed resume.** The research used the feed's resume time; live uses the resume status
//!   (`chart.halt` with an end), keyed by the halt's own start. **No resume status, no alert.**
//!   The research's fallback for a feed row without a resume (look from `halt_ns + 5 min`) is
//!   not used: it made none of the 2,668 Train rows, and the tape does print during listed halts
//!   (BBLG 2022-01-10 printed at 816 s and 820 s of a halt listed to resume at 824 s; NVCT
//!   2022-02-07 one second before its resume), so a fallback taken before a late resume status
//!   would reopen on those prints, where the research waits for the listed time. A first draft
//!   of this port did use it and missed exactly those two Train rows (parity report). Priced
//!   bars that start after the halt and arrive before its resume status are held (at most
//!   [`HELD_MAX`]) and looked at when the status arrives, so a reopening print that beats its
//!   own status (the SIP and the participant stamp different clocks) still counts.
//! - **1 s coverage.** The research cache kept 1 s rows only for "gated" symbols, so 151 of its
//!   8,656 Train reopens were found on 15 s bars (L-x-halt §5). Live builds 1 s bars for every
//!   symbol from its first print, so the rule picks 1 s bars essentially always; the alert is
//!   then up to 14 s earlier and priced at the reopening second's open rather than the 15 s
//!   bar's. The rule itself is kept, so the research's inputs reproduce the research's rows.
//! - **The universe.** The research scanned its cache universe (universe-v1 types, end-of-day
//!   membership); live excludes universe-v1's excluded types (`crate::n1_live::sec_type`) and
//!   needs the day's previous close (`crate::n1_live` facts), and sees every other symbol.
//! - **Late input.** A print for a bar already closed is dropped and counted ([`Late`]); a halt
//!   status stamped at or before an alert's instant that arrives after it was sent cannot
//!   withdraw it (counted as `rehalt_after_fire`).

use std::collections::VecDeque;

pub const SEC: i64 = 1_000_000_000;
pub const MIN: i64 = 60 * SEC;
/// The 15 s bar grid (the research cache's slots).
pub const SLOT: i64 = 15 * SEC;
/// Run-in floor `u0.05`: `halt_px / low15 − 1 ≥ 0.05`.
pub const RUN_IN_MIN: f64 = 0.05;
/// `halt_px ≥ 0.99 × high60`.
pub const NEAR_HIGH60: f64 = 0.99;
/// Mode `open0`: `reopen_px ≥ halt_px × 1.0`.
pub const OPEN_RATIO: f64 = 1.0;
/// The run-in and near-high windows, on bar ends.
pub const WIN15: i64 = 15 * MIN;
pub const WIN60: i64 = 60 * SEC;
/// Priced bars held per waiting halt until its resume status arrives (module docs, "Listed
/// resume"); prices rarely print during a halt, so this is a bound, not a budget.
pub const HELD_MAX: usize = 64;
/// The alert is decided this long after the reopen bar ends.
pub const LAG: i64 = SEC;
/// The price band on `reopen_px` (micro-dollars, inclusive).
pub const BAND_LO_U: i64 = 100_000;
pub const BAND_HI_U: i64 = 25_000_000;
/// The research's configuration id, carried on every fire and in the ledger.
pub const CONFIG: &str = "RU-u0.05-open0-Kany";
/// The finalist's frozen identity: finalist id, config sha256 prefix and research code commit.
pub const VERSION: &str = "x-halt-1@cd040948/860e7af1";

/// Whether a halt reason (Alpaca `rc`, or the label it maps to) is one the stream takes: an LULD
/// pause or "News pending" (`is_luld == 1 or reason == "News pending"`). `label` is
/// `crate::alerts::halts::reason_label` of the status's reason.
pub fn typed_reason(label: &str) -> bool {
    label == "LULD pause" || label == "News pending"
}

/// One bar. Prices in micro-dollars; `c == 0` means the bar has no OHLC-counted print (a
/// volume-only or uncounted slot), and then `o`, `h`, `l` are 0 too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    /// Start, epoch ns (04:00 ET + slot × length).
    pub start: i64,
    pub o: i64,
    pub h: i64,
    pub l: i64,
    pub c: i64,
}

impl Bar {
    pub fn priced(&self) -> bool {
        self.c > 0
    }
}

/// The facts of a halt taken at its instant (module docs, step 2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pre {
    pub halt_px: i64,
    pub low15: i64,
    /// `None`: no priced bar ended in the 60 s before the halt.
    pub high60: Option<i64>,
    pub run_in: f64,
}

/// Where the reopen was found (module docs, step 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Res {
    Sec1,
    Sec15,
}

impl Res {
    pub fn len_ns(self) -> i64 {
        match self {
            Res::Sec1 => SEC,
            Res::Sec15 => SLOT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Reopen {
    start: i64,
    px: i64,
    res: Res,
}

impl Reopen {
    fn known(&self) -> i64 {
        self.start + self.res.len_ns()
    }
    fn decision(&self) -> i64 {
        self.known() + LAG
    }
}

#[derive(Clone, Debug)]
struct Pending {
    halt_ns: i64,
    typed: bool,
    pre: Option<Pre>,
    resume_ns: Option<i64>,
    reopen: Option<Reopen>,
    /// Priced bars after the halt that arrived before its resume status, in arrival order.
    held: Vec<(Bar, Res)>,
}

impl Pending {
    /// `rfrom` once it is known: the listed resume, never before `halt_ns + 1` (module docs,
    /// step 4 and "Listed resume").
    fn rfrom(&self) -> Option<i64> {
        self.resume_ns.map(|r| r.max(self.halt_ns + 1))
    }

    /// `b` (of resolution `res`) as the reopen, if it is the first priced bar of the wanted
    /// resolution starting at or after `rfrom`.
    fn try_reopen(&mut self, b: Bar, res: Res, first_b1: Option<i64>) {
        if self.reopen.is_some() || !b.priced() {
            return;
        }
        let Some(rfrom) = self.rfrom() else {
            if b.start > self.halt_ns && self.held.len() < HELD_MAX {
                self.held.push((b, res));
            }
            return;
        };
        // `st1[0] <= rfrom`: the 1 s rows cover the reopen (module docs, step 4).
        let want = if first_b1.is_some_and(|f| f <= rfrom) {
            Res::Sec1
        } else {
            Res::Sec15
        };
        if want == res && b.start >= rfrom {
            self.reopen = Some(Reopen {
                start: b.start,
                px: b.o,
                res,
            });
        }
    }
}

/// One x-halt-1 alert: the halt, its facts, the reopen and the decision.
#[derive(Clone, Debug, PartialEq)]
pub struct Fire {
    pub halt_ns: i64,
    pub resume_ns: Option<i64>,
    pub pre: Pre,
    pub reopen_start: i64,
    pub reopen_px: i64,
    pub res: Res,
    /// The alert instant `a = reopen_known_ns + 1 s` (epoch ns).
    pub at_ns: i64,
}

/// Late or skipped input, counted (never silently different from the research).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Late {
    /// A print for a bar already closed ([`Builder`]).
    pub prints_dropped: u64,
    /// A halt stamped at or before an alert that was already decided (it could not withdraw it).
    pub rehalt_after_fire: u64,
}

/// What became of one eligible-or-not halt, for the day's counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub halts: u64,
    pub typed: u64,
    pub eligible: u64,
    pub reopened: u64,
    pub below_halt: u64,
    pub out_of_band: u64,
    pub rehalted: u64,
    pub fires: u64,
}

/// One symbol's day: the recent priced 15 s bars, the 1 s coverage start, every halt start, and
/// the halts still waiting for their reopen or their decision.
#[derive(Clone, Debug, Default)]
pub struct Sym {
    /// Priced 15 s bars, start order, the last one always kept, older ones only while they END
    /// within [`WIN15`] of the newest end (nothing a later halt reads is dropped: a halt at `h`
    /// reads `end > h − 15 min` and the newest end is at or before `h`).
    b15: VecDeque<Bar>,
    /// The start of the symbol's first 1 s row today (`st1[0]`).
    first_b1: Option<i64>,
    /// Every halt start of the day (the "not halted again" check).
    halts: Vec<i64>,
    pending: Vec<Pending>,
    /// Halt starts already decided (fired or not), for [`Late::rehalt_after_fire`].
    fired_at: Vec<(i64, i64)>,
    pub counts: Counts,
    pub late: Late,
}

impl Sym {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether anything is waiting (a halt not decided yet).
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    /// A 15 s bar, once it is complete (its end has passed), in start order.
    pub fn push_bar15(&mut self, b: Bar) {
        if b.priced() {
            self.b15.push_back(b);
            let newest_end = b.start + SLOT;
            while self.b15.len() > 1 && self.b15[0].start + SLOT <= newest_end - WIN15 {
                self.b15.pop_front();
            }
        }
        self.reopen_on(b, Res::Sec15);
    }

    /// A 1 s bar, once it is complete, in start order.
    pub fn push_bar1(&mut self, b: Bar) {
        if self.first_b1.is_none() {
            self.first_b1 = Some(b.start);
        }
        self.reopen_on(b, Res::Sec1);
    }

    fn reopen_on(&mut self, b: Bar, res: Res) {
        let first_b1 = self.first_b1;
        for p in self.pending.iter_mut() {
            p.try_reopen(b, res, first_b1);
        }
    }

    /// A halt started at `halt_ns` (every complete 15 s bar ending at or before it already
    /// pushed). `typed`: [`typed_reason`] of its reason.
    pub fn halt(&mut self, halt_ns: i64, typed: bool) {
        for &(h, a) in &self.fired_at {
            if halt_ns > h && halt_ns <= a {
                self.late.rehalt_after_fire += 1;
            }
        }
        self.halts.push(halt_ns);
        self.counts.halts += 1;
        self.counts.typed += typed as u64;
        let pre = self.pre(halt_ns);
        self.pending.push(Pending {
            halt_ns,
            typed,
            pre,
            resume_ns: None,
            reopen: None,
            held: Vec::new(),
        });
    }

    /// The halt that started at `halt_ns` resumed at `resume_ns`: the bars held for it are
    /// looked at now, in the order they came.
    pub fn resume(&mut self, halt_ns: i64, resume_ns: i64) {
        let first_b1 = self.first_b1;
        if let Some(p) = self
            .pending
            .iter_mut()
            .find(|p| p.halt_ns == halt_ns && p.resume_ns.is_none())
        {
            p.resume_ns = Some(resume_ns);
            for (b, res) in std::mem::take(&mut p.held) {
                p.try_reopen(b, res, first_b1);
            }
        }
    }

    fn pre(&self, halt_ns: i64) -> Option<Pre> {
        let end = |b: &Bar| b.start + SLOT;
        let pre: Vec<&Bar> = self.b15.iter().filter(|b| end(b) <= halt_ns).collect();
        let last = *pre.last()?;
        let halt_px = last.c;
        let mut w15: Vec<&Bar> = pre
            .iter()
            .copied()
            .filter(|b| end(b) > halt_ns - WIN15)
            .collect();
        if w15.is_empty() {
            w15.push(last);
        }
        let low15 = w15.iter().map(|b| b.l).min().expect("non-empty");
        let high60 = pre
            .iter()
            .filter(|b| end(b) > halt_ns - WIN60)
            .map(|b| b.h)
            .max();
        Some(Pre {
            halt_px,
            low15,
            high60,
            run_in: halt_px as f64 / low15 as f64 - 1.0,
        })
    }

    /// The earliest instant a waiting halt can be decided at, if its reopen is known.
    pub fn next_due(&self) -> Option<i64> {
        self.pending
            .iter()
            .filter_map(|p| p.reopen.map(|r| r.decision()))
            .min()
    }

    /// Decides every waiting halt whose alert instant has come (`now ≥ a`; every halt status
    /// stamped at or before `now` already given). `c1_known`: the symbol has a previous close
    /// (and is not an excluded type). Returns the fires, in halt order.
    pub fn decide(&mut self, now: i64, c1_known: bool) -> Vec<Fire> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.pending.len() {
            let p = &self.pending[i];
            let Some(r) = p.reopen.filter(|r| r.decision() <= now) else {
                i += 1;
                continue;
            };
            let p = self.pending.remove(i);
            let a = r.decision();
            self.fired_at.push((p.halt_ns, a));
            if let Some(f) = self.judge(&p, r, a, c1_known) {
                out.push(f);
            }
        }
        out
    }

    fn judge(&mut self, p: &Pending, r: Reopen, a: i64, c1_known: bool) -> Option<Fire> {
        let pre = p.pre?;
        // Eligible (step 3): typed, C1 known, the run-in, near the 60 s high.
        if !(p.typed && c1_known && pre.run_in >= RUN_IN_MIN) {
            return None;
        }
        if pre
            .high60
            .is_some_and(|h| (pre.halt_px as f64) < NEAR_HIGH60 * h as f64)
        {
            return None;
        }
        self.counts.eligible += 1;
        self.counts.reopened += 1;
        // Step 5, in `_decide`'s order: open0, the band, not halted again.
        if (r.px as f64) < pre.halt_px as f64 * OPEN_RATIO {
            self.counts.below_halt += 1;
            return None;
        }
        if r.px < BAND_LO_U || r.px > BAND_HI_U {
            self.counts.out_of_band += 1;
            return None;
        }
        if self.halts.iter().any(|&h| h > p.halt_ns && h <= a) {
            self.counts.rehalted += 1;
            return None;
        }
        self.counts.fires += 1;
        Some(Fire {
            halt_ns: p.halt_ns,
            resume_ns: p.resume_ns,
            pre,
            reopen_start: r.start,
            reopen_px: r.px,
            res: r.res,
            at_ns: a,
        })
    }
}

/// An open bar being built.
#[derive(Clone, Copy, Debug)]
struct Open {
    start: i64,
    /// `(exec ns, px)` of the earliest and the latest OHLC print (the later arrival on a tie for
    /// the close, the earlier for the open), and the extremes.
    first: Option<(i64, i64)>,
    last: Option<(i64, i64)>,
    h: i64,
    l: i64,
}

impl Open {
    fn new(start: i64) -> Self {
        Open {
            start,
            first: None,
            last: None,
            h: 0,
            l: 0,
        }
    }
    fn add(&mut self, ns: i64, px: i64, ohlc: bool) {
        if !ohlc {
            return;
        }
        match self.first {
            Some((t, _)) if t <= ns => {}
            _ => self.first = Some((ns, px)),
        }
        match self.last {
            Some((t, _)) if t > ns => {}
            _ => self.last = Some((ns, px)),
        }
        if self.h == 0 || px > self.h {
            self.h = px;
        }
        if self.l == 0 || px < self.l {
            self.l = px;
        }
    }
    fn bar(&self) -> Bar {
        match (self.first, self.last) {
            (Some((_, o)), Some((_, c))) => Bar {
                start: self.start,
                o,
                h: self.h,
                l: self.l,
                c,
            },
            _ => Bar {
                start: self.start,
                o: 0,
                h: 0,
                l: 0,
                c: 0,
            },
        }
    }
}

/// A print for a bar already closed ([`Builder::print`]): dropped, and counted by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatePrint;

/// One bar length's builder for one symbol: prints in, complete bars out (start order).
#[derive(Clone, Debug)]
pub struct Builder {
    t0: i64,
    len: i64,
    open: Option<Open>,
}

impl Builder {
    /// Bars of `len` ns on the grid from `t0` (04:00 ET).
    pub fn new(t0: i64, len: i64) -> Self {
        Builder {
            t0,
            len,
            open: None,
        }
    }

    /// One print at `ns` (exec time) of `px` micro-dollars. Every print opens its bar (the
    /// research store's rows); only an OHLC-counted one prices it. Returns the bar it closed,
    /// if this print starts a later one; `Err(LatePrint)` for a print of an already closed bar (late).
    pub fn print(&mut self, ns: i64, px: i64, ohlc: bool) -> Result<Option<Bar>, LatePrint> {
        if ns < self.t0 {
            return Ok(None);
        }
        let start = self.t0 + (ns - self.t0).div_euclid(self.len) * self.len;
        match self.open.as_mut() {
            Some(o) if o.start == start => {
                o.add(ns, px, ohlc);
                Ok(None)
            }
            Some(o) if o.start > start => Err(LatePrint),
            _ => {
                let done = self.open.take().map(|o| o.bar());
                let mut o = Open::new(start);
                o.add(ns, px, ohlc);
                self.open = Some(o);
                Ok(done)
            }
        }
    }

    /// The open bar, if its end is at or before `now` (it is complete; a later print for it is
    /// late).
    pub fn close_through(&mut self, now: i64) -> Option<Bar> {
        if self.open.is_some_and(|o| o.start + self.len <= now) {
            return self.open.take().map(|o| o.bar());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_718_092_800 * SEC; // any 04:00

    fn b(start: i64, o: i64, h: i64, l: i64, c: i64) -> Bar {
        Bar { start, o, h, l, c }
    }

    /// A stock running from $2.00 to $2.20 into an LULD halt at 600 s, reopening at $2.25.
    fn sym_with_run(reopen_px: i64) -> Sym {
        let mut s = Sym::new();
        s.push_bar1(b(T0, 2_000_000, 2_000_000, 2_000_000, 2_000_000));
        for k in 0..40 {
            let st = T0 + k * SLOT;
            let px = 2_000_000 + k * 5_000;
            s.push_bar15(b(st, px, px, px, px));
        }
        s.halt(T0 + 600 * SEC, true);
        s.resume(T0 + 600 * SEC, T0 + 900 * SEC);
        s.push_bar1(b(
            T0 + 900 * SEC,
            reopen_px,
            reopen_px,
            reopen_px,
            reopen_px,
        ));
        s
    }

    #[test]
    fn fires_one_second_after_the_reopen_second() {
        let mut s = sym_with_run(2_250_000);
        assert_eq!(s.next_due(), Some(T0 + 902 * SEC));
        assert!(s.decide(T0 + 901 * SEC, true).is_empty());
        let f = s.decide(T0 + 902 * SEC, true);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].at_ns, T0 + 902 * SEC);
        assert_eq!(f[0].reopen_px, 2_250_000);
        assert_eq!(f[0].pre.halt_px, 2_195_000);
        assert_eq!(f[0].res, Res::Sec1);
        assert!(!s.waiting());
    }

    #[test]
    fn below_the_halt_price_or_rehalted_or_untyped_does_not_fire() {
        let mut s = sym_with_run(2_100_000);
        assert!(s.decide(T0 + 902 * SEC, true).is_empty());
        assert_eq!(s.counts.below_halt, 1);
        let mut s = sym_with_run(2_250_000);
        s.halt(T0 + 902 * SEC, false); // halted again at a itself: (halt, a] includes a
        assert!(s.decide(T0 + 902 * SEC, true).is_empty());
        assert_eq!(s.counts.rehalted, 1);
        let mut s = sym_with_run(2_250_000);
        assert!(s.decide(T0 + 902 * SEC, false).is_empty(), "no C1");
    }

    #[test]
    fn no_resume_no_alert_and_held_bars_wait_for_the_status() {
        // No 1 s rows: the 15 s bars decide. A print during the halt (before the status) is
        // held, and does not count: the status lists the resume after it.
        let mut s = Sym::new();
        for k in 0..40 {
            let st = T0 + k * SLOT;
            let px = 2_000_000 + k * 5_000;
            s.push_bar15(b(st, px, px, px, px));
        }
        s.halt(T0 + 600 * SEC, true);
        let early = T0 + 900 * SEC;
        s.push_bar15(b(early, 2_100_000, 2_100_000, 2_100_000, 2_100_000));
        assert_eq!(s.next_due(), None, "no resume status yet");
        // The reopening print beat its own status: held, then taken when the status comes.
        let st = T0 + 915 * SEC;
        s.push_bar15(b(st, 2_300_000, 2_300_000, 2_300_000, 2_300_000));
        s.resume(T0 + 600 * SEC, T0 + 912 * SEC);
        let f = s.decide(i64::MAX, true);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].res, Res::Sec15);
        assert_eq!((f[0].reopen_start, f[0].at_ns), (st, st + SLOT + SEC));
        // Without a resume status nothing is ever decided.
        let mut s = Sym::new();
        s.push_bar15(b(T0, 2_000_000, 2_000_000, 1_800_000, 2_000_000));
        s.halt(T0 + 600 * SEC, true);
        s.push_bar15(b(
            T0 + 900 * SEC,
            2_300_000,
            2_300_000,
            2_300_000,
            2_300_000,
        ));
        assert!(s.decide(i64::MAX, true).is_empty());
        assert!(s.waiting());
    }

    #[test]
    fn builder_bars_follow_the_grid() {
        let mut g = Builder::new(T0, SEC);
        assert_eq!(g.print(T0 + 100, 5, true), Ok(None));
        assert_eq!(g.print(T0 + 200, 7, true), Ok(None));
        assert_eq!(g.print(T0 + 300, 9, false), Ok(None));
        let done = g.print(T0 + SEC + 1, 6, true).unwrap().unwrap();
        assert_eq!(done, b(T0, 5, 7, 5, 7));
        assert_eq!(g.print(T0 + 5, 1, true), Err(LatePrint));
        assert_eq!(g.close_through(T0 + 2 * SEC), Some(b(T0 + SEC, 6, 6, 6, 6)));
        assert_eq!(g.print(T0 + 3 * SEC, 1, false), Ok(None));
        assert_eq!(
            g.close_through(T0 + 4 * SEC),
            Some(b(T0 + 3 * SEC, 0, 0, 0, 0))
        );
    }
}
