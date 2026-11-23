//! **News pending card** (owner, 2026-11-22): one card per stock the exchange halted because news
//! is pending (a T1 halt), walked through the halt's life so a trader sees the halt, then the news
//! the moment it lands, before the reopen:
//!
//! | Phase | Entered on | The card says |
//! |---|---|---|
//! | `halted` | a T1 halt status (or T2 / T3 first seen: a news halt either way) | halt time, halt price, % into the halt, a count-up |
//! | `news` | a news item for the symbol (the news seam) or the exchange's T2 "news released" | the headline and its source; an offering item flags it at once |
//! | `resuming` | the exchange's T3 "news and resumption times" or the quote-resumption status (`Q`) | a countdown to the reopen |
//! | `resumed` | the resume status | the reopen, then the card leaves after [`DEFAULT_GRACE_SECS`] |
//!
//! The phase is the most advanced one reached: a resumption set with no story keeps `news` empty
//! and the card still counts down. It works with the halts alone; news only adds to it.
//!
//! # Inputs (all already in the market core; no new feed)
//!
//! - **Statuses**: every trading status of every live symbol, held or loaded, at the same place
//!   the stock-context engine gets them (`Market` ingest, "documentary status"). The exchange's
//!   reason code (`rc`, else the phrase `rm`) says which halt it is: T1 "news pending", T2 "news
//!   released", T3 "news and resumption times" (Nasdaq; NYSE's phrases match by their words).
//!   The quote-resumption status `Q` starts the quote-only period before a halt cross; trading
//!   resumes about [`QUOTE_LEAD_MS`] later, so the reopen time from it is marked an estimate.
//!   The SIP carries no resumption time itself.
//! - **Halt price, % into the halt**: the quote board's last price and its split-guarded previous
//!   close at the first halt status of the episode (what the app's "% up" reads). The card is
//!   made only for a halt price inside the platform band ($0.10–$25 by default); a stock with no
//!   price yet is not shown (nothing true to say about it).
//! - **News**: the stock-context evidence log's news items as they are delivered to the live board
//!   (`Market::deliver_evidence`, every 15 s), through [`hit_from_item`] — the one place that
//!   reads a `stock_context::NewsItem`, so the news seam (ross/news: SEC EDGAR, press wires,
//!   headlines and URLs on the item) plugs in there. Items of the T1 halt itself (provider
//!   `halt_t1`) are our own echo and skipped. An item counts for a halt when it was published at
//!   most [`NEWS_LOOKBACK_MS`] before the halt began (a story that crossed the wire seconds before
//!   the halt is the halt's story) or any time after.
//! - **Offering**: an item classed as an offering filing (`offering_filed`) or financing
//!   (`financing`: offering, placement, registered direct, warrants…), or whose headline says so.
//! - **Session**: the halt start's session (`pre` / `regular` / `after`), so a client can tag an
//!   after-hours halt.
//!
//! # What it is not (T1 study, Train, `outputs/stockwatcher-research-v2/60-t1/T1-STUDY.md`)
//!
//! - **Not a trade signal.** Buying after the reopen lost money on average (-3.1 % net, n=423),
//!   so the card informs and the clients use no action wording. What separated outcomes, and is
//!   known by the reopen, is what the card shows: an offering before the reopen (-7.1 % net, the
//!   one real separation: the danger flag), the story's kind (its source and headline) and the
//!   session (after-hours T1s reopened up less often).
//! - **Not a notification.** It sends nothing: T1 pushes stay the Halts alert's own
//!   (`crate::alerts::halts`, "News pending"), with its coverage unchanged.
//! - **Not the scheduled corporate-action halts.** A "news pending" halt starting at exactly
//!   19:50:00 ET is the halt the evening before a reverse split / corporate action takes effect
//!   (309 in band in Train), not news; it never makes a card ([`scheduled_action_halt`]).
//!
//! # Traffic
//!
//! `pending.subscribe` → `pending.snapshot`, then `pending.update` with only the changed cards
//! and removed ids, **once per state change** (decided on the Runners board's tick, never per
//! print). Every timer is computed by the client from the timestamps. A few T1 halts a day means
//! a few dozen small frames a day.
//!
//! # Switch
//!
//! `PENDING_CARD=0` (env, a restart) removes it; on by default for the live market with
//! `ROCKET_WATCH`. `PENDING_GRACE_SECS` sets how long a resumed card stays (default 120).
//!
//! # Known limits
//!
//! - A restart during a halt loses it: the card comes back only if the exchange sends another
//!   status for that halt (a T2 / T3 does). Deploys stay outside market hours.
//! - The reopen estimate assumes the usual 5-minute quote-only period; an exchange that extends
//!   it reopens later than the countdown (the card then waits at 0:00 until the resume status).

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use market_events::UnixNanos;
use market_events::session::{SessionKind, classify, et, trading_date};
use serde_json::{Value, json};

/// The welcome feature a client checks before sending `pending.subscribe`.
pub const FEATURE: &str = "pending.v1";
/// How long a resumed card stays before it is removed (`PENDING_GRACE_SECS`).
pub const DEFAULT_GRACE_SECS: i64 = 120;
/// The usual quote-only period between the quote resumption (`Q`) and the reopening cross.
pub const QUOTE_LEAD_MS: i64 = 5 * 60_000;
/// A story published up to this long before the halt began is the halt's story.
pub const NEWS_LOOKBACK_MS: i64 = 5 * 60_000;
/// Stories kept on a card (newest first).
pub const MAX_NEWS: usize = 3;
/// Cards kept at most (the oldest resumed, then the oldest, go first). T1 halts are a handful a
/// day; this only bounds a broken feed.
pub const MAX_CARDS: usize = 64;
/// How long a news item that matched no card is kept for a halt that may follow it.
const RECENT_KEEP_MS: i64 = 15 * 60_000;

/// Whether a halt starting at `start_ms` is the scheduled corporate-action halt: the second
/// 19:50:00 ET (module docs). The feed labels it "news pending" too.
pub fn scheduled_action_halt(start_ms: i64) -> bool {
    let ns = UnixNanos(start_ms.saturating_mul(1_000_000));
    let Some(s) = classify(ns) else {
        return false;
    };
    et(s.date, 19, 50, 0, 0).is_some_and(|t| ns.0 >= t.0 && ns.0 < t.0 + 1_000_000_000)
}

/// The halt start's session as the wire spells it.
fn session_wire(start_ms: i64) -> &'static str {
    match classify(UnixNanos(start_ms.saturating_mul(1_000_000))).map(|s| s.kind) {
        Some(SessionKind::Pre) => "pre",
        Some(SessionKind::Regular) => "regular",
        Some(SessionKind::After) => "after",
        Some(SessionKind::Overnight) => "overnight",
        None => "closed",
    }
}

/// The halt reason, as far as this card is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewsHalt {
    /// T1: news pending.
    Pending,
    /// T2: news released.
    Released,
    /// T3: news and resumption times disseminated.
    ResumeSet,
}

/// A halt reason (`rc`, else the phrase `rm`) as a news halt, or None for any other halt.
pub fn news_halt(reason: &str) -> Option<NewsHalt> {
    let r = reason.trim();
    match r.to_ascii_uppercase().as_str() {
        "T1" => return Some(NewsHalt::Pending),
        "T2" => return Some(NewsHalt::Released),
        "T3" => return Some(NewsHalt::ResumeSet),
        _ => {}
    }
    let l = r.to_ascii_lowercase();
    if l.contains("news pending") {
        Some(NewsHalt::Pending)
    } else if l.contains("news released") || l.contains("news dissemination") {
        Some(NewsHalt::Released)
    } else if l.contains("resumption time") || l.contains("news and resumption") {
        Some(NewsHalt::ResumeSet)
    } else {
        None
    }
}

/// What kind of status a feed status is, for this card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusSignal {
    Halt,
    Resume,
    /// `Q`: quotation resumes (the reopening cross follows).
    QuoteResume,
    Other,
}

/// The signal of a status from its kind and raw status code (`sc`).
pub fn signal(kind: market_events::StatusKind, code: &str) -> StatusSignal {
    match kind {
        market_events::StatusKind::Halt => StatusSignal::Halt,
        market_events::StatusKind::Resume => StatusSignal::Resume,
        _ if code.trim().eq_ignore_ascii_case("Q") => StatusSignal::QuoteResume,
        _ => StatusSignal::Other,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    Halted,
    News,
    Resuming,
    Resumed,
}

impl Phase {
    fn wire(self) -> &'static str {
        match self {
            Phase::Halted => "halted",
            Phase::News => "news",
            Phase::Resuming => "resuming",
            Phase::Resumed => "resumed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKind {
    Catalyst,
    Offering,
}

/// One story for a card: the local shape of the news seam's item (source, symbol, time,
/// headline, url, kind), so the seam is read in one place ([`hit_from_item`]).
#[derive(Clone, Debug, PartialEq)]
pub struct NewsHit {
    pub id: String,
    /// `provider/sub` as the item carries it: `alpaca/benzinga`, `sec_edgar/8-K`, `press_wire/globenewswire`.
    pub source: String,
    pub headline: Option<String>,
    pub url: Option<String>,
    /// Published (ms).
    pub at_ms: i64,
    /// First observed by us (ms).
    pub seen_ms: i64,
    pub kind: HitKind,
}

/// Headline words that mark an offering (dilution) when the item's class does not.
const OFFERING_WORDS: [&str; 8] = [
    "offering",
    "private placement",
    "registered direct",
    "at-the-market",
    "pricing of",
    "warrants",
    "convertible",
    "shelf registration",
];

fn offering_class(event_type: &str) -> bool {
    matches!(event_type, "offering_filed" | "financing")
}

fn offering_headline(h: &str) -> bool {
    let l = h.to_ascii_lowercase();
    OFFERING_WORDS.iter().any(|w| l.contains(w))
}

/// The seam: a stock-context news item as `(symbol, hit)`, or None for an item that is not a
/// story about one symbol (a tombstone, an irrelevant tag, our own T1 echo).
///
/// The headline and URL come from the ross/news seam's `NewsItem` (an SEC filing's form and
/// company, a story's title); an offering headline flags the card too.
pub fn hit_from_item(item: &stock_context::NewsItem) -> Option<(String, NewsHit)> {
    if item.tag != stock_context::Tag::Present || !item.relevant {
        return None;
    }
    let provider = item.source.split('/').next().unwrap_or("");
    if provider == "halt_t1" {
        return None;
    }
    let symbol = item.instrument.0.strip_prefix("sym:")?.to_owned();
    let headline = item.headline.clone();
    let url = item.url.clone();
    let offering =
        offering_class(&item.event_type) || headline.as_deref().is_some_and(offering_headline);
    let at = item.published_at.unwrap_or(item.observed_at);
    Some((
        symbol,
        NewsHit {
            id: item.id.clone(),
            source: item.source.clone(),
            headline,
            url,
            at_ms: at.div_euclid(1_000_000),
            seen_ms: item.observed_at.div_euclid(1_000_000),
            kind: if offering {
                HitKind::Offering
            } else {
                HitKind::Catalyst
            },
        },
    ))
}

/// The source as a person reads it: "Benzinga", "SEC 8-K", "GlobeNewswire".
pub fn source_label(source: &str) -> String {
    let (provider, sub) = source.split_once('/').unwrap_or((source, ""));
    let wire = |s: &str| -> Option<&'static str> {
        Some(match s.to_ascii_lowercase().as_str() {
            "benzinga" => "Benzinga",
            "globenewswire" => "GlobeNewswire",
            "prnewswire" => "PR Newswire",
            "businesswire" => "Business Wire",
            "accesswire" => "ACCESSWIRE",
            "newsfile" => "Newsfile",
            _ => return None,
        })
    };
    match provider {
        "sec_edgar" if !sub.is_empty() => format!("SEC {sub}"),
        "sec_edgar" => "SEC filing".into(),
        "alpaca" => wire(sub).unwrap_or("Benzinga").into(),
        "press_wire" => wire(sub).unwrap_or("Press wire").into(),
        _ => wire(sub)
            .or_else(|| wire(provider))
            .map(str::to_owned)
            .unwrap_or_else(|| if sub.is_empty() { provider } else { sub }.to_owned()),
    }
}

#[derive(Clone, Debug)]
pub struct Card {
    pub id: String,
    pub sym: String,
    pub halted_at: i64,
    pub code: String,
    pub price: Option<f64>,
    pub prev_close: Option<f64>,
    pub phase: Phase,
    pub released_at: Option<i64>,
    pub news: Vec<NewsHit>,
    pub resume_at: Option<i64>,
    pub resume_est: bool,
    pub resumed_at: Option<i64>,
    /// Wall clock of the resume, for the grace period.
    resumed_wall: Option<i64>,
    pub reopen_price: Option<f64>,
    pub updated: i64,
}

impl Card {
    pub fn offering(&self) -> bool {
        self.news.iter().any(|n| n.kind == HitKind::Offering)
    }

    fn chg_pct(&self) -> Option<f64> {
        let (p, c) = (self.price?, self.prev_close?);
        (c.is_finite() && c > 0.0 && p.is_finite()).then(|| (p / c - 1.0) * 100.0)
    }

    fn advance(&mut self, p: Phase) {
        self.phase = self.phase.max(p);
    }

    /// Adds a story (newest first, at most [`MAX_NEWS`]); false when it is not this halt's or
    /// already there.
    fn add_news(&mut self, hit: &NewsHit) -> bool {
        if hit.at_ms < self.halted_at - NEWS_LOOKBACK_MS || self.news.iter().any(|n| n.id == hit.id)
        {
            return false;
        }
        self.news.push(hit.clone());
        self.news.sort_by_key(|n| std::cmp::Reverse(n.at_ms));
        // An offering is never the one pushed off: it is the item that kills the move.
        while self.news.len() > MAX_NEWS {
            let drop = self
                .news
                .iter()
                .rposition(|n| n.kind != HitKind::Offering)
                .unwrap_or(self.news.len() - 1);
            self.news.remove(drop);
        }
        self.advance(Phase::News);
        true
    }
}

/// What the board needs from the quote board for one symbol: last price, previous close.
pub type Quote = Option<(f64, Option<f64>)>;

/// Every live news-halt card, in the market core.
#[derive(Debug)]
pub struct PendingBoard {
    on: bool,
    band: (f64, f64),
    grace_ms: i64,
    /// Any halt in progress (any reason): its start and the price then, so a LULD pause that
    /// turns into a T1 keeps its real start.
    halted: HashMap<String, (i64, Option<f64>, Option<f64>)>,
    /// The cards by symbol (one live card per symbol; a new halt replaces a resumed one).
    cards: HashMap<String, Card>,
    /// Stories that matched no card yet (a halt may follow within [`NEWS_LOOKBACK_MS`]).
    recent: Vec<(String, NewsHit)>,
    seen: HashSet<String>,
    changed: HashSet<String>,
    removed: Vec<String>,
    generation: u64,
    day: Option<NaiveDate>,
}

impl Default for PendingBoard {
    fn default() -> Self {
        Self::new()
    }
}

fn round(x: f64, k: i32) -> f64 {
    let p = 10f64.powi(k);
    (x * p).round() / p
}

fn opt(x: Option<f64>, k: i32) -> Value {
    x.filter(|v| v.is_finite())
        .map_or(Value::Null, |v| json!(round(v, k)))
}

impl PendingBoard {
    pub fn new() -> Self {
        PendingBoard {
            on: false,
            band: (0.10, 25.0),
            grace_ms: DEFAULT_GRACE_SECS * 1000,
            halted: HashMap::new(),
            cards: HashMap::new(),
            recent: Vec::new(),
            seen: HashSet::new(),
            changed: HashSet::new(),
            removed: Vec::new(),
            generation: 0,
            day: None,
        }
    }

    /// Turns it on with the platform band and the resumed cards' grace period.
    pub fn enable(&mut self, band: (f64, f64), grace_secs: i64) {
        self.on = true;
        self.band = band;
        self.grace_ms = grace_secs.max(0) * 1000;
    }

    pub fn enabled(&self) -> bool {
        self.on
    }

    pub fn cards(&self) -> impl Iterator<Item = &Card> {
        self.cards.values()
    }

    pub fn card(&self, sym: &str) -> Option<&Card> {
        self.cards.get(sym)
    }

    fn in_band(&self, p: f64) -> bool {
        p.is_finite() && p >= self.band.0 - 1e-9 && p <= self.band.1 + 1e-9
    }

    fn touch(&mut self, sym: &str, now: i64) {
        if let Some(c) = self.cards.get_mut(sym) {
            c.updated = now;
        }
        self.changed.insert(sym.to_owned());
    }

    /// One trading status (`ts` feed clock ms, `now` wall ms). `quote` is read only at a halt's
    /// first status. O(1).
    pub fn on_status(
        &mut self,
        symbol: &str,
        sig: StatusSignal,
        reason: &str,
        ts: i64,
        quote: impl FnOnce() -> Quote,
        now: i64,
    ) {
        if !self.on {
            return;
        }
        match sig {
            StatusSignal::Halt => {
                let (start, price, pc) =
                    *self.halted.entry(symbol.to_owned()).or_insert_with(|| {
                        let q = quote();
                        (ts, q.map(|q| q.0), q.and_then(|q| q.1))
                    });
                let Some(class) = news_halt(reason) else {
                    return;
                };
                let live = self
                    .cards
                    .get(symbol)
                    .is_some_and(|c| c.phase != Phase::Resumed && c.halted_at == start);
                if !live {
                    if scheduled_action_halt(start) {
                        return;
                    }
                    let Some(p) = price.filter(|p| self.in_band(*p)) else {
                        return;
                    };
                    if let Some(old) = self.cards.remove(symbol) {
                        self.removed.push(old.id);
                    }
                    let mut card = Card {
                        id: format!("{symbol}-{start}"),
                        sym: symbol.to_owned(),
                        halted_at: start,
                        code: reason.trim().chars().take(40).collect(),
                        price: Some(p),
                        prev_close: pc,
                        phase: Phase::Halted,
                        released_at: None,
                        news: Vec::new(),
                        resume_at: None,
                        resume_est: false,
                        resumed_at: None,
                        resumed_wall: None,
                        reopen_price: None,
                        updated: now,
                    };
                    for (s, h) in &self.recent {
                        if s == symbol {
                            card.add_news(h);
                        }
                    }
                    self.removed.retain(|id| *id != card.id);
                    self.cards.insert(symbol.to_owned(), card);
                    self.bound();
                }
                let c = self.cards.get_mut(symbol).expect("live card");
                match class {
                    NewsHalt::Pending => {}
                    NewsHalt::Released => {
                        c.released_at.get_or_insert(ts);
                        c.advance(Phase::News);
                    }
                    NewsHalt::ResumeSet => c.advance(Phase::Resuming),
                }
                self.touch(symbol, now);
            }
            StatusSignal::QuoteResume => {
                if let Some(c) = self
                    .cards
                    .get_mut(symbol)
                    .filter(|c| c.phase != Phase::Resumed)
                {
                    if c.resume_at.is_none() {
                        c.resume_at = Some(ts + QUOTE_LEAD_MS);
                        c.resume_est = true;
                    }
                    c.advance(Phase::Resuming);
                    self.touch(symbol, now);
                }
            }
            StatusSignal::Resume => {
                self.halted.remove(symbol);
                if let Some(c) = self
                    .cards
                    .get_mut(symbol)
                    .filter(|c| c.phase != Phase::Resumed)
                {
                    c.resumed_at = Some(ts);
                    c.resumed_wall = Some(now);
                    c.phase = Phase::Resumed;
                    self.touch(symbol, now);
                }
            }
            StatusSignal::Other => {}
        }
    }

    /// Stories delivered from the evidence log (`hit_from_item` output). Each id once.
    pub fn on_news(&mut self, hits: impl IntoIterator<Item = (String, NewsHit)>, now: i64) {
        if !self.on {
            return;
        }
        for (sym, hit) in hits {
            if !self.seen.insert(hit.id.clone()) {
                continue;
            }
            let added = self.cards.get_mut(&sym).is_some_and(|c| c.add_news(&hit));
            if added {
                self.touch(&sym, now);
            } else if hit.at_ms >= now - RECENT_KEEP_MS {
                self.recent.push((sym, hit));
            }
        }
    }

    /// The tick: a new day's cleanup, resumed cards past their grace removed, the reopen price
    /// read for a resumed card (`last(symbol)` = last price and its feed time, ms).
    pub fn tick(&mut self, now: i64, last: impl Fn(&str) -> Option<(f64, i64)>) {
        if !self.on {
            return;
        }
        let date = trading_date(UnixNanos(now.saturating_mul(1_000_000)));
        if self.day.is_some_and(|d| d != date) {
            // A new day: the resumed cards and yesterday's stories go; a halt still open stays.
            let gone: Vec<String> = self
                .cards
                .iter()
                .filter(|(_, c)| c.phase == Phase::Resumed)
                .map(|(s, _)| s.clone())
                .collect();
            for s in gone {
                self.remove(&s);
            }
            self.seen.clear();
        }
        self.day = Some(date);
        self.recent.retain(|(_, h)| h.at_ms >= now - RECENT_KEEP_MS);
        let mut due = Vec::new();
        let mut reopened = Vec::new();
        for (s, c) in &self.cards {
            if c.phase != Phase::Resumed {
                continue;
            }
            if c.resumed_wall.is_some_and(|w| now - w >= self.grace_ms) {
                due.push(s.clone());
            } else if c.reopen_price.is_none()
                && let Some((p, t)) = last(s)
                && t >= c.resumed_at.unwrap_or(i64::MAX)
            {
                reopened.push((s.clone(), p));
            }
        }
        for s in due {
            self.remove(&s);
        }
        for (s, p) in reopened {
            if let Some(c) = self.cards.get_mut(&s) {
                c.reopen_price = Some(p);
            }
            self.touch(&s, now);
        }
    }

    fn remove(&mut self, sym: &str) {
        if let Some(c) = self.cards.remove(sym) {
            self.changed.remove(sym);
            self.removed.push(c.id);
        }
    }

    fn bound(&mut self) {
        while self.cards.len() > MAX_CARDS {
            let oldest = self
                .cards
                .values()
                .min_by_key(|c| (c.phase != Phase::Resumed, c.halted_at))
                .map(|c| c.sym.clone());
            match oldest {
                Some(s) => self.remove(&s),
                None => break,
            }
        }
    }

    fn sorted(&self) -> Vec<&Card> {
        let mut v: Vec<&Card> = self.cards.values().collect();
        v.sort_by(|a, b| b.halted_at.cmp(&a.halted_at).then(a.sym.cmp(&b.sym)));
        v
    }

    /// `pending.snapshot`: every card.
    pub fn snapshot_frame(&self) -> String {
        json!({
            "type": "pending.snapshot",
            "enabled": self.on,
            "gen": self.generation,
            "grace_secs": self.grace_ms / 1000,
            "band": [self.band.0, self.band.1],
            "cards": self.sorted().into_iter().map(card_wire).collect::<Vec<_>>(),
        })
        .to_string()
    }

    /// `pending.update` with the cards changed and removed since the last one, or None.
    pub fn take_update(&mut self) -> Option<String> {
        if self.changed.is_empty() && self.removed.is_empty() {
            return None;
        }
        let prev = self.generation;
        self.generation += 1;
        let mut cards: Vec<&Card> = self
            .changed
            .iter()
            .filter_map(|s| self.cards.get(s))
            .collect();
        cards.sort_by(|a, b| b.halted_at.cmp(&a.halted_at).then(a.sym.cmp(&b.sym)));
        let frame = json!({
            "type": "pending.update",
            "enabled": self.on,
            "prev_gen": prev,
            "gen": self.generation,
            "cards": cards.into_iter().map(card_wire).collect::<Vec<_>>(),
            "removed": self.removed,
        })
        .to_string();
        self.changed.clear();
        self.removed.clear();
        Some(frame)
    }

    /// The cost line's entry.
    pub fn cost(&self) -> Value {
        json!({"cards": self.cards.len(), "halted": self.halted.len(), "recent": self.recent.len()})
    }
}

pub fn card_wire(c: &Card) -> Value {
    json!({
        "id": c.id,
        "sym": c.sym,
        "halted_at": c.halted_at,
        "code": c.code,
        "reason": "News pending",
        "price": opt(c.price, 4),
        "prev_close": opt(c.prev_close, 4),
        "chg_pct": opt(c.chg_pct(), 2),
        "phase": c.phase.wire(),
        "session": session_wire(c.halted_at),
        "released_at": c.released_at,
        "news": c.news.iter().map(|n| json!({
            "id": n.id,
            "source": n.source,
            "label": source_label(&n.source),
            "headline": n.headline,
            "url": n.url,
            "at": n.at_ms,
            "seen": n.seen_ms,
            "kind": match n.kind { HitKind::Catalyst => "catalyst", HitKind::Offering => "offering" },
        })).collect::<Vec<_>>(),
        "offering": c.offering(),
        "resume_at": c.resume_at,
        "resume_est": c.resume_est,
        "resumed_at": c.resumed_at,
        "reopen_price": opt(c.reopen_price, 4),
        "updated": c.updated,
    })
}
