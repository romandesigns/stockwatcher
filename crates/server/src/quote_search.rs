//! The ticker search's ranked rows (`quotes.search.v1`): the server filters and ranks its own quote
//! board for a search and pushes only the top N, instead of every client downloading the whole
//! board (thousands of rows on a live day, ~400 KB, then every changed row once a second) to
//! filter and sort it in the browser or on the phone.
//!
//! What stays on the client: fuzzy text matching. It is typing-speed work over a static list (the
//! candidates' symbols and names, `quotes.directory`, fetched once per opening when the list grew),
//! and a round trip per keystroke would only make it slower. The client then pins the matches it
//! shows (`pins`) so their live quotes arrive in the same stream.
//!
//! # Wire (after `welcome`, only to a client that saw `quotes.search.v1`; older clients keep
//! `quotes.subscribe`)
//!
//! - client `quotes.search {id?, filters?, sort?, limit?, pins?}` starts or replaces search `id`
//!   (≤ [`MAX_ID`] chars, absent = `""`; at most [`MAX_SEARCHES`] per connection, `error
//!   too_many_searches` past that):
//!   - `filters`: the search chips' ids, packages/shared `TickerFilterId`: `under1`, `1to5`,
//!     `5to25` (price), `gainers`, `losers` (move), `nasdaq`, `nyse` (exchange), `sim`, `mock`.
//!     Alternatives within a group, every group must pass (the chips' own rule). `sim` passes
//!     every server row; `mock` none (mock rows exist only in the client's fixture list). An
//!     unknown id is `error bad_request` with the `id`.
//!   - `sort`: `activity` (day volume, biggest first; the default) or `move` (|change %|).
//!     Ties by symbol, the order the client's sort kept.
//!   - `limit`: rows ranked, 1..=[`MAX_LIMIT`] (default [`DEFAULT_LIMIT`]); the web list asks
//!     for more as it scrolls.
//!   - `pins`: up to [`MAX_PINS`] symbols whose rows are sent whatever their rank (the query's
//!     matches, the recents), so every row a client draws is live.
//! - server `quotes.search.snapshot {id, t, total, rank, rows}` at once: `total` = rows passing
//!   the filters, `rank` = the top `limit` symbols in order, `rows` = one per ranked or pinned
//!   symbol that has a quote inside the band.
//! - then `quotes.search.update {id, t, rows, total?, rank?, gone?}` at most once per second of
//!   wall time (the connection's own tick), only when something changed: `rank` whole when it
//!   changed, `rows` only the changed or entering ones, `gone` the symbols with no row now.
//! - Row `{s, p, chg, v, name?, x?}`: last price (dollars), change % against the previous close
//!   when known, else the open reference (the quote board's and the chart header's rule, 3
//!   decimals, decided here so every client shows the same figure), day volume; `name` / `x`
//!   only for a row the client does not have yet, and only when `assets.json` has them.
//! - client `quotes.search.stop {id?}` ends one search. A seek, a day switch or a lag resends a
//!   snapshot of each search.
//! - client `quotes.directory` -> server `quotes.directory {rows: [[symbol, name?, exchange?]]}`:
//!   every current candidate (crate::market::Market::symbols), with the name and exchange when
//!   `assets.json` has them. One-shot; the client asks again only when a candidate is missing.
//!
//! # Cost
//!
//! One search is one pass over the board's priced rows ([`QuoteBoard::scan`], no allocation per
//! row), a partial selection of the top `limit`, and the pins' lookups, under the core lock, once
//! a second per open search. The search is open only while a person is looking at it, and a
//! connection holds at most [`MAX_SEARCHES`]. Measured in `search_cost_on_a_13k_board`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use market_archive::AssetMeta;
use market_events::PriceBand;
use serde::{Deserialize, Serialize};

use crate::quotes::{QuoteBoard, ScanRow};

/// What a client must see in `welcome.features` before it sends `quotes.search`.
pub const FEATURE: &str = "quotes.search.v1";
pub const DEFAULT_LIMIT: usize = 50;
/// The web list renders 200 rows at a time and asks for more as it scrolls; past this a person
/// types or filters (the same cut the old client-side list effectively had: nobody scrolls 6,800).
pub const MAX_LIMIT: usize = 1000;
pub const MAX_PINS: usize = 200;
/// The search sheet and the phone dashboard's Most active list can be open together.
pub const MAX_SEARCHES: usize = 4;
pub const MAX_ID: usize = 32;

/// `quotes.search` as received.
#[derive(Debug, Default, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub filters: Vec<String>,
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub pins: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Activity,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chip {
    Under1,
    OneTo5,
    FiveTo25,
    Gainers,
    Losers,
    Nasdaq,
    Nyse,
    Sim,
    Mock,
}

impl Chip {
    fn parse(s: &str) -> Option<Chip> {
        Some(match s {
            "under1" => Chip::Under1,
            "1to5" => Chip::OneTo5,
            "5to25" => Chip::FiveTo25,
            "gainers" => Chip::Gainers,
            "losers" => Chip::Losers,
            "nasdaq" => Chip::Nasdaq,
            "nyse" => Chip::Nyse,
            "sim" => Chip::Sim,
            "mock" => Chip::Mock,
            _ => return None,
        })
    }

    /// The chip's group (packages/shared `TICKER_FILTER_GROUPS`).
    fn group(self) -> usize {
        match self {
            Chip::Sim | Chip::Mock => 0,
            Chip::Under1 | Chip::OneTo5 | Chip::FiveTo25 => 1,
            Chip::Gainers | Chip::Losers => 2,
            Chip::Nasdaq | Chip::Nyse => 3,
        }
    }

    /// The chip's test, the client's `TEST` table: `price` in dollars, `chg` in percent.
    fn test(self, price: f64, chg: f64, exchange: &str) -> bool {
        match self {
            Chip::Under1 => price < 1.0,
            Chip::OneTo5 => (1.0..5.0).contains(&price),
            Chip::FiveTo25 => (5.0..=25.0).contains(&price),
            Chip::Gainers => chg > 0.0,
            Chip::Losers => chg < 0.0,
            Chip::Nasdaq => exchange == "NASDAQ",
            Chip::Nyse => exchange == "NYSE",
            // Every server row has data in the running market; a mock row is the client's own.
            Chip::Sim => true,
            Chip::Mock => false,
        }
    }
}

/// A parsed, bounded search.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub id: String,
    /// The active chips per group (empty = the group does not narrow).
    groups: [Vec<Chip>; 4],
    pub sort: Sort,
    pub limit: usize,
    pub pins: Vec<String>,
}

impl Query {
    /// The request checked and bounded: an unknown chip or sort is an error (a client bug the
    /// person should not silently get an unfiltered list for); `limit` and `pins` are clamped.
    pub fn parse(r: Request) -> Result<Query, String> {
        let id = r.id.unwrap_or_default();
        if id.chars().count() > MAX_ID {
            return Err(format!("search id longer than {MAX_ID} characters"));
        }
        let mut groups: [Vec<Chip>; 4] = Default::default();
        for f in &r.filters {
            let c = Chip::parse(f).ok_or_else(|| format!("unknown filter {f:?}"))?;
            let g = &mut groups[c.group()];
            if !g.contains(&c) {
                g.push(c);
            }
        }
        let sort = match r.sort.as_deref() {
            None | Some("activity") => Sort::Activity,
            Some("move") => Sort::Move,
            Some(s) => return Err(format!("unknown sort {s:?}")),
        };
        let limit = r.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let mut seen = HashSet::new();
        let pins: Vec<String> = r
            .pins
            .into_iter()
            .filter(|s| !s.is_empty() && s.len() <= 16 && seen.insert(s.clone()))
            .take(MAX_PINS)
            .collect();
        Ok(Query {
            id,
            groups,
            sort,
            limit,
            pins,
        })
    }

    fn passes(&self, price: f64, chg: f64, exchange: &str) -> bool {
        self.groups
            .iter()
            .all(|g| g.is_empty() || g.iter().any(|c| c.test(price, chg, exchange)))
    }
}

/// One published row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub price: f64,
    /// Percent, 3 decimals (crate::movers' rounding: a sub-rounding wobble is not a change).
    pub chg: f64,
    pub volume: u64,
}

/// One search's result at one tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Results {
    pub total: usize,
    pub rank: Vec<Arc<str>>,
    /// Ranked and pinned symbols that have a quote inside the band.
    pub rows: HashMap<Arc<str>, Row>,
}

/// Change % by the board's rule (crate::movers::change_pct): against the previous close when
/// known and positive, else the reference; 0 for a 0 reference.
fn change_pct(r: &ScanRow<'_>) -> f64 {
    crate::movers::change_pct(r)
}

/// Runs `q` over the board: rows inside `band`, filtered, the top `limit` by the sort, plus the
/// pins' rows. `exchange` reads a symbol's listing exchange from `assets.json` ("" when unknown).
pub fn search(
    board: &QuoteBoard,
    band: PriceBand,
    assets: &HashMap<String, AssetMeta>,
    q: &Query,
) -> Results {
    #[derive(Clone, Copy)]
    struct Cand<'a> {
        row: ScanRow<'a>,
        price: f64,
        chg: f64,
    }
    let pins: HashSet<&str> = q.pins.iter().map(String::as_str).collect();
    let by_exchange = !q.groups[Chip::Nasdaq.group()].is_empty();
    let mut pinned: Vec<Cand<'_>> = Vec::new();
    let mut cands: Vec<Cand<'_>> = Vec::new();
    for row in board.scan().filter(|r| band.contains(r.last)) {
        let c = Cand {
            row,
            price: row.last.to_f64(),
            chg: change_pct(&row),
        };
        if pins.contains(&**row.symbol) {
            pinned.push(c);
        }
        // Under the core lock, once per row: the name lookup only when an exchange chip asks.
        let exchange = if by_exchange {
            assets
                .get(&**row.symbol)
                .map_or("", |m| m.exchange.as_str())
        } else {
            ""
        };
        if q.passes(c.price, c.chg, exchange) {
            cands.push(c);
        }
    }
    let by = |a: &Cand<'_>, b: &Cand<'_>| {
        let key = match q.sort {
            Sort::Activity => b.row.volume.cmp(&a.row.volume),
            Sort::Move => b.chg.abs().total_cmp(&a.chg.abs()),
        };
        key.then_with(|| a.row.symbol.cmp(b.row.symbol))
    };
    let total = cands.len();
    if cands.len() > q.limit {
        cands.select_nth_unstable_by(q.limit - 1, by);
        cands.truncate(q.limit);
    }
    cands.sort_by(by);
    let mut out = Results {
        total,
        rank: Vec::with_capacity(cands.len()),
        rows: HashMap::with_capacity(cands.len() + pinned.len()),
    };
    for c in cands.iter().chain(pinned.iter()) {
        out.rows.entry(c.row.symbol.clone()).or_insert(Row {
            price: c.price,
            chg: c.chg,
            volume: c.row.volume,
        });
    }
    out.rank = cands.into_iter().map(|c| c.row.symbol.clone()).collect();
    out
}

#[derive(Serialize)]
struct WireRow<'a> {
    s: &'a str,
    p: f64,
    chg: f64,
    v: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<&'a str>,
}

fn wire_row<'a>(
    s: &'a str,
    r: &Row,
    entering: bool,
    assets: &'a HashMap<String, AssetMeta>,
) -> WireRow<'a> {
    let meta = entering.then(|| assets.get(s)).flatten();
    WireRow {
        s,
        p: r.price,
        chg: r.chg,
        v: r.volume,
        name: meta.map(|m| m.name.as_str()).filter(|n| !n.is_empty()),
        x: meta.map(|m| m.exchange.as_str()).filter(|x| !x.is_empty()),
    }
}

/// `type` first, as every frame (the web feed routes on the leading `{"type":"quotes.`).
#[derive(Serialize)]
struct Snapshot<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    id: &'a str,
    t: Option<i64>,
    total: usize,
    rank: Vec<&'a str>,
    rows: Vec<WireRow<'a>>,
}

#[derive(Serialize)]
struct Update<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    id: &'a str,
    t: Option<i64>,
    rows: Vec<WireRow<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rank: Option<Vec<&'a str>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    gone: Vec<&'a str>,
}

/// Rows in a stable order (by symbol), so equal results serialize identically.
fn sorted(rows: &HashMap<Arc<str>, Row>) -> Vec<(&Arc<str>, &Row)> {
    let mut v: Vec<_> = rows.iter().collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    v
}

pub fn snapshot_frame(
    id: &str,
    t: Option<i64>,
    r: &Results,
    assets: &HashMap<String, AssetMeta>,
) -> String {
    serde_json::to_string(&Snapshot {
        kind: "quotes.search.snapshot",
        id,
        t,
        total: r.total,
        rank: r.rank.iter().map(|s| &**s).collect(),
        rows: sorted(&r.rows)
            .into_iter()
            .map(|(s, row)| wire_row(s, row, true, assets))
            .collect(),
    })
    .expect("search snapshot serializes")
}

/// The `quotes.search.update` that takes a client from `prev` to `next`; `None` when they are
/// equal (nothing is sent: the 1 Hz tick costs the client nothing on a quiet board).
pub fn update_frame(
    id: &str,
    t: Option<i64>,
    prev: &Results,
    next: &Results,
    assets: &HashMap<String, AssetMeta>,
) -> Option<String> {
    let rows: Vec<WireRow<'_>> = sorted(&next.rows)
        .into_iter()
        .filter(|(s, r)| prev.rows.get(*s) != Some(*r))
        .map(|(s, r)| wire_row(s, r, !prev.rows.contains_key(s), assets))
        .collect();
    let mut gone: Vec<&str> = prev
        .rows
        .keys()
        .filter(|s| !next.rows.contains_key(*s))
        .map(|s| &**s)
        .collect();
    gone.sort_unstable();
    let rank = (prev.rank != next.rank).then(|| next.rank.iter().map(|s| &**s).collect());
    let total = (prev.total != next.total).then_some(next.total);
    if rows.is_empty() && gone.is_empty() && rank.is_none() && total.is_none() {
        return None;
    }
    Some(
        serde_json::to_string(&Update {
            kind: "quotes.search.update",
            id,
            t,
            rows,
            total,
            rank,
            gone,
        })
        .expect("search update serializes"),
    )
}

#[derive(Serialize)]
struct Directory<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    rows: Vec<Vec<&'a str>>,
}

/// `quotes.directory`: each candidate as `[symbol]`, `[symbol, name]` or `[symbol, name,
/// exchange]` (trailing unknowns left out; an unknown name with a known exchange is `""`).
pub fn directory_frame(symbols: &[String], assets: &HashMap<String, AssetMeta>) -> String {
    let rows = symbols
        .iter()
        .map(|s| {
            let mut row = vec![s.as_str()];
            if let Some(m) = assets.get(s) {
                if !m.exchange.is_empty() {
                    row.push(m.name.as_str());
                    row.push(m.exchange.as_str());
                } else if !m.name.is_empty() {
                    row.push(m.name.as_str());
                }
            }
            row
        })
        .collect();
    serde_json::to_string(&Directory {
        kind: "quotes.directory",
        rows,
    })
    .expect("directory serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quotes::Counted;
    use chrono::NaiveDate;
    use market_events::session::et;
    use market_events::{Price, Trade, UnixNanos};

    fn at(h: u32, m: u32, s: u32) -> UnixNanos {
        et(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap(), h, m, s, 0).unwrap()
    }

    const BOTH: Counted = Counted {
        ohlc: true,
        volume: true,
    };

    /// `n` prints of `size` shares at `cents`, regular session (the first sets the open, so a
    /// row put once is flat on the day).
    fn put(b: &mut QuoteBoard, sym: &str, cents: i64, size: u64, n: u32) {
        for i in 0..n {
            b.apply_trade(
                &Trade::new(
                    sym,
                    at(10, i / 60, i % 60),
                    Price::from_cents(cents),
                    size,
                    1 + i as u64,
                ),
                BOTH,
            );
        }
    }

    fn assets(rows: &[(&str, &str, &str)]) -> HashMap<String, AssetMeta> {
        rows.iter()
            .map(|(s, n, x)| {
                (
                    s.to_string(),
                    AssetMeta {
                        name: n.to_string(),
                        exchange: x.to_string(),
                    },
                )
            })
            .collect()
    }

    fn q(filters: &[&str], sort: Option<&str>, limit: usize, pins: &[&str]) -> Query {
        Query::parse(Request {
            id: Some("s".into()),
            filters: filters.iter().map(|s| s.to_string()).collect(),
            sort: sort.map(String::from),
            limit: Some(limit),
            pins: pins.iter().map(|s| s.to_string()).collect(),
        })
        .unwrap()
    }

    fn syms(r: &Results) -> Vec<&str> {
        r.rank.iter().map(|s| &**s).collect()
    }

    #[test]
    fn ranks_by_activity_or_move_inside_the_band_with_ties_by_symbol() {
        let mut b = QuoteBoard::new();
        put(&mut b, "AAA", 200, 100, 5); // 500 shares, flat
        put(&mut b, "BBB", 300, 100, 9); // 900
        put(&mut b, "CCC", 50, 100, 9); // 900, a tie with BBB
        put(&mut b, "HIGH", 4000, 100, 20); // $40: outside the band
        let a = assets(&[]);
        let r = search(&b, PriceBand::DEFAULT, &a, &q(&[], None, 10, &[]));
        assert_eq!(syms(&r), ["BBB", "CCC", "AAA"]);
        assert_eq!(r.total, 3);
        let r = search(&b, PriceBand::DEFAULT, &a, &q(&[], None, 2, &[]));
        assert_eq!(syms(&r), ["BBB", "CCC"], "the top `limit` only");
        assert_eq!(r.total, 3, "total counts every row that passes");
        assert_eq!(r.rows.len(), 2);
        // Every row opened and closed at its one price: no move, so the move sort is by symbol.
        let r = search(&b, PriceBand::DEFAULT, &a, &q(&[], Some("move"), 10, &[]));
        assert_eq!(syms(&r), ["AAA", "BBB", "CCC"]);
    }

    #[test]
    fn chips_are_alternatives_within_a_group_and_narrow_across_groups() {
        let mut b = QuoteBoard::new();
        put(&mut b, "PENNY", 50, 100, 3);
        put(&mut b, "MID", 300, 100, 3);
        put(&mut b, "TEN", 1000, 100, 3);
        let a = assets(&[
            ("PENNY", "Penny Co", "NASDAQ"),
            ("MID", "Mid Co", "NYSE"),
            ("TEN", "Ten Co", "NASDAQ"),
        ]);
        let r = search(
            &b,
            PriceBand::DEFAULT,
            &a,
            &q(&["under1", "5to25"], None, 10, &[]),
        );
        let mut got = syms(&r);
        got.sort();
        assert_eq!(got, ["PENNY", "TEN"]);
        let r = search(
            &b,
            PriceBand::DEFAULT,
            &a,
            &q(&["under1", "5to25", "nyse"], None, 10, &[]),
        );
        assert!(r.rank.is_empty(), "no NYSE row in those price groups");
        let r = search(&b, PriceBand::DEFAULT, &a, &q(&["sim"], None, 10, &[]));
        assert_eq!(r.total, 3, "sim passes every server row");
        let r = search(&b, PriceBand::DEFAULT, &a, &q(&["mock"], None, 10, &[]));
        assert_eq!(r.total, 0, "mock rows are the client's own");
        assert!(
            Query::parse(Request {
                filters: vec!["bogus".into()],
                ..Request::default()
            })
            .is_err()
        );
        assert!(
            Query::parse(Request {
                sort: Some("price".into()),
                ..Request::default()
            })
            .is_err()
        );
    }

    #[test]
    fn pins_get_rows_outside_the_rank_and_the_diff_names_only_what_changed() {
        let mut b = QuoteBoard::new();
        put(&mut b, "BIG", 200, 100, 9);
        put(&mut b, "SMALL", 200, 100, 1);
        let a = assets(&[("SMALL", "Small Co", "NYSE")]);
        let query = q(&[], None, 1, &["SMALL", "NOPE"]);
        let r1 = search(&b, PriceBand::DEFAULT, &a, &query);
        assert_eq!(syms(&r1), ["BIG"]);
        let mut rows: Vec<&str> = r1.rows.keys().map(|s| &**s).collect();
        rows.sort();
        assert_eq!(rows, ["BIG", "SMALL"], "a pin without a quote has no row");
        let snap = snapshot_frame("s", Some(1), &r1, &a);
        assert!(
            snap.starts_with(r#"{"type":"quotes.search.snapshot","id":"s""#),
            "{snap}"
        );
        assert!(snap.contains(r#""name":"Small Co","x":"NYSE""#), "{snap}");
        assert_eq!(update_frame("s", Some(2), &r1, &r1, &a), None);
        // SMALL trades past BIG: the rank flips and both rows changed volume or not.
        put(&mut b, "SMALL", 200, 100, 20);
        let r2 = search(&b, PriceBand::DEFAULT, &a, &query);
        assert_eq!(syms(&r2), ["SMALL"]);
        let up = update_frame("s", Some(2), &r1, &r2, &a).unwrap();
        let v: serde_json::Value = serde_json::from_str(&up).unwrap();
        assert_eq!(v["type"], "quotes.search.update");
        assert_eq!(v["rank"], serde_json::json!(["SMALL"]));
        assert_eq!(
            v["gone"],
            serde_json::json!(["BIG"]),
            "BIG is neither ranked nor pinned"
        );
        let rows = v["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["s"], "SMALL");
        assert!(
            rows[0].get("name").is_none(),
            "the client already has its name"
        );
        assert!(v.get("total").is_none(), "unchanged");
    }

    #[test]
    fn the_directory_lists_every_candidate_with_what_assets_knows() {
        let a = assets(&[("AAA", "Alpha", "NASDAQ"), ("BBB", "Beta", "")]);
        let f = directory_frame(&["AAA".into(), "BBB".into(), "CCC".into()], &a);
        assert_eq!(
            f,
            r#"{"type":"quotes.directory","rows":[["AAA","Alpha","NASDAQ"],["BBB","Beta"],["CCC"]]}"#
        );
    }

    /// `--nocapture` prints the per-search cost on a 13,000-symbol board (the live universe).
    #[test]
    fn search_cost_on_a_13k_board() {
        let mut b = QuoteBoard::new();
        for i in 0..13_000u32 {
            put(
                &mut b,
                &format!("S{i:05}"),
                10 + (i as i64 % 2400),
                100 + i as u64,
                2,
            );
        }
        let a = assets(&[]);
        let query = q(&["1to5"], None, 200, &["S00001", "S00002"]);
        let start = std::time::Instant::now();
        let n = 20;
        for _ in 0..n {
            std::hint::black_box(search(&b, PriceBand::DEFAULT, &a, &query));
        }
        let all = q(&[], None, 1000, &[]);
        let t_all = std::time::Instant::now();
        let r = search(&b, PriceBand::DEFAULT, &a, &all);
        println!(
            "quotes.search on 13k rows: {:?} per filtered search, {:?} for the unfiltered top 1000 ({} pass)",
            start.elapsed() / n,
            t_all.elapsed(),
            r.total
        );
        assert_eq!(r.rank.len(), 1000);
    }
}
