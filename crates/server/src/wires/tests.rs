use super::classify::{WireKind, classify};
use super::parse::{parse_feed, parse_time};
use super::tickers::{Listing, Venue, listings, ticker_of, venue_of};
use super::*;
use market_archive::AssetMeta;

const PRN: &str = include_str!("fixtures/prn_rss.xml");
const ATOM: &str = include_str!("fixtures/gnw_atom.xml");
const SEC: i64 = 1_000_000_000;

fn ns(s: &str) -> i64 {
    parse_time(s).unwrap()
}

fn universe() -> Arc<AssetMap> {
    let mut m = AssetMap::new();
    for (s, x) in [
        ("ZEPP", "NYSE"),
        ("LUCA", "NASDAQ"),
        ("LUCMF", "OTC"),
        ("EYPT", "NASDAQ"),
        ("EXMP", "NASDAQ"),
        ("ACMB", "NASDAQ"),
        ("BOLT", "AMEX"),
        ("FHN", "NYSE"),
        ("F", "NYSE"),
        ("PR", "NYSE"),
        ("PL", "NYSE"),
        ("MS", "NYSE"),
        ("AI", "NYSE"),
        ("CEO", "NYSE"),
        ("EXRSF", "OTC"),
        ("BRK.B", "NYSE"),
    ] {
        m.insert(
            s.into(),
            AssetMeta {
                name: s.into(),
                exchange: x.into(),
            },
        );
    }
    Arc::new(m)
}

fn by_title<'a>(recs: &'a [WireRecord], t: &str) -> &'a WireRecord {
    recs.iter().find(|r| r.title.starts_with(t)).unwrap()
}

#[test]
fn rss_parses_fields_times_and_entities() {
    let e = parse_feed(PRN).unwrap();
    assert_eq!(e.len(), 6);
    let z = &e[1];
    assert!(z.title.starts_with("Zepp Health Corporation Reports"));
    assert_eq!(z.published_at_ns, Some(ns("2026-11-23T01:00:00Z")));
    assert!(
        z.summary
            .contains("(\"Zepp\" or the \"Company\") (NYSE: ZEPP)"),
        "{}",
        z.summary
    );
    assert!(!z.summary.contains("<p>"));
    assert_eq!(z.guid, z.link);
    // Zone-less pubDate: kept raw, not guessed.
    let nz = &e[5];
    assert_eq!(
        nz.published_raw.as_deref(),
        Some("Mon, 23 Nov 2026 01:30:00")
    );
    assert_eq!(nz.published_at_ns, None);
}

#[test]
fn atom_parses_links_published_over_updated_and_escaped_html() {
    let e = parse_feed(ATOM).unwrap();
    assert_eq!(e.len(), 4);
    assert_eq!(e[0].guid, "urn:fixture:1");
    assert_eq!(e[0].link, "https://example.invalid/release/1?a=1&b=2");
    assert_eq!(e[0].published_at_ns, Some(ns("2026-11-23T13:00:00Z")));
    assert!(
        e[0].summary
            .contains("(Nasdaq: ACMB) today announced\u{2026}"),
        "{}",
        e[0].summary
    );
    // No `published`: `updated` is used.
    assert_eq!(e[1].published_at_ns, Some(ns("2026-11-23T13:01:00Z")));
    assert_eq!(e[1].link, "https://example.invalid/release/2");
}

#[test]
fn times_parse_with_zones_only() {
    let t = ns("2026-11-23T02:17:24Z");
    assert_eq!(parse_time("Mon, 23 Nov 2026 02:17:24 UT"), Some(t));
    assert_eq!(parse_time("Mon, 23 Nov 2026 02:17:24 GMT"), Some(t));
    assert_eq!(parse_time("Sun, 22 Nov 2026 21:17:24 EST"), Some(t));
    assert_eq!(parse_time("2026-11-22T21:17:24-05:00"), Some(t));
    assert_eq!(parse_time("Mon, 23 Nov 2026 02:17:24"), None);
    assert_eq!(parse_time("2026-11-23T02:17:24"), None);
    assert_eq!(parse_time("yesterday"), None);
}

#[test]
fn malformed_feeds_are_errors_and_change_nothing() {
    let mut c = Collector::new(universe());
    let now = ns("2026-11-23T02:26:00Z");
    let cut = &PRN[..PRN.len() / 2];
    assert!(c.ingest("prn", cut, now).is_err());
    let akamai = "<HTML><HEAD><TITLE>Access Denied</TITLE></HEAD><BODY>denied</BODY></HTML>";
    assert!(c.ingest("prn", akamai, now).is_err());
    assert!(c.ingest("prn", "", now).is_err());
    assert!(
        c.ingest("prn", "<rss><channel><item><title>a</ti></item>", now)
            .is_err()
    );
    // Nothing was marked seen: the whole feed still comes through afterwards.
    let ok = c.ingest("prn", PRN, now).unwrap();
    assert_eq!(ok.records.len(), 6);
}

#[test]
fn rss_ingest_maps_strictly_classifies_and_gates_on_time() {
    let mut c = Collector::new(universe());
    let now = ns("2026-11-23T02:26:00Z");
    let out = c.ingest("prn", PRN, now).unwrap();
    let r = &out.records;
    assert_eq!(r.len(), 6);

    let hua = by_title(r, "Hua Medicine");
    assert_eq!(hua.status, "no_symbol");
    assert!(hua.listings.is_empty(), "HKEX stock code is not a venue");

    let zepp = by_title(r, "Zepp");
    assert_eq!(
        (zepp.status.as_str(), zepp.kind),
        ("delivered", WireKind::Catalyst)
    );
    assert_eq!(zepp.symbols, ["ZEPP"]);
    assert_eq!(zepp.first_observed_at, now);

    // TSXV LUCA is a Canadian listing: never the NASDAQ LUCA. The OTCQX one maps.
    let luca = by_title(r, "Luca Mining");
    assert_eq!(luca.listings, ["OTC:LUCMF"]);
    assert_eq!(luca.symbols, ["LUCMF"]);

    let eypt = by_title(r, "EyePoint");
    assert_eq!(
        (eypt.status.as_str(), eypt.kind),
        ("delivered", WireKind::Offering)
    );

    assert_eq!(by_title(r, "Old Release").status, "stale");
    assert_eq!(by_title(r, "Example Corp Wins").status, "no_published_time");

    let items = &out.items;
    let syms: Vec<&str> = items.iter().map(|i| i.symbol.as_str()).collect();
    assert_eq!(syms, ["ZEPP", "LUCMF", "EYPT"]);
    let z = &items[0];
    assert_eq!(z.source, "wire:prn");
    assert_eq!(z.observed_at_ns, now);
    assert_eq!(z.published_at_ns, Some(ns("2026-11-23T01:00:00Z")));
    assert_eq!(z.url.as_deref(), Some(zepp.link.as_str()));
    assert_eq!(items[2].kind, WireKind::Offering);

    // The next poll returns the same entries: nothing new is archived or delivered.
    let again = c.ingest("prn", PRN, now + 60 * SEC).unwrap();
    assert!(again.records.is_empty() && again.items.is_empty());
}

#[test]
fn atom_two_tickers_and_class_ticker_false_positives() {
    let mut c = Collector::new(universe());
    let now = ns("2026-11-23T13:06:00Z");
    let out = c.ingest("gnw", ATOM, now).unwrap();
    let r = &out.records;

    let merger = by_title(r, "Bolt Mining");
    assert_eq!(merger.symbols, ["BOLT", "ACMB"]);

    // `FHN PR E` and `MS/PL` are invalid pieces, dropped whole: no FHN, PR, MS, PL. Bare
    // words (CEO, AI, PR, F) never count.
    let fhn = by_title(r, "First Horizon");
    assert!(fhn.listings.is_empty(), "{:?}", fhn.listings);
    assert_eq!(fhn.status, "no_symbol");

    // TSX-V ACMB is not the NASDAQ ACMB; TSX BOLT is not the NYSE American BOLT.
    let x = by_title(r, "Cross-listed");
    assert_eq!(x.symbols, ["EXRSF"]);

    let delivered: Vec<(&str, &str)> = out
        .items
        .iter()
        .map(|i| (i.symbol.as_str(), i.id.as_str()))
        .collect();
    assert_eq!(
        delivered,
        [
            ("ACMB", "wire:gnw:urn:fixture:1"),
            ("BOLT", "wire:gnw:urn:fixture:2"),
            ("ACMB", "wire:gnw:urn:fixture:2"),
            ("EXRSF", "wire:gnw:urn:fixture:4"),
        ]
    );
}

fn one_item_rss(guid: &str, title: &str, ticker: &str, pub_date: &str) -> String {
    format!(
        "<rss version=\"2.0\"><channel><item><title>{title}</title><link>https://example.invalid/{guid}</link>\
         <guid>{guid}</guid><pubDate>{pub_date}</pubDate>\
         <description>Acme (NASDAQ: {ticker}) said.</description></item></channel></rss>"
    )
}

#[test]
fn the_same_release_on_two_wires_is_delivered_once() {
    let mut c = Collector::new(universe());
    let t0 = ns("2026-11-23T13:00:30Z");
    let pd = "Mon, 23 Nov 2026 13:00:00 +0000";
    let a = one_item_rss(
        "a1",
        "Acme Bio Receives FDA Fast Track Designation for ACM-101",
        "ACMB",
        pd,
    );
    let b = one_item_rss(
        "b1",
        "ACME BIO Receives FDA Fast-Track Designation for ACM-101",
        "ACMB",
        pd,
    );
    assert_eq!(c.ingest("wa", &a, t0).unwrap().items.len(), 1);
    let dup = c.ingest("wb", &b, t0 + 90 * SEC).unwrap();
    assert!(dup.items.is_empty());
    assert_eq!(dup.records[0].status, "duplicate");
    assert_eq!(dup.records[0].dup_of.as_deref(), Some("wire:wa:a1"));
    // A different release for the same symbol is not a duplicate.
    let other = one_item_rss("b2", "Acme Bio Announces Third Quarter Results", "ACMB", pd);
    assert_eq!(
        c.ingest("wb", &other, t0 + 100 * SEC).unwrap().items.len(),
        1
    );
    // Outside the window the same headline is delivered again (a re-issue, e.g. a correction).
    let late = one_item_rss(
        "c1",
        "Acme Bio Receives FDA Fast Track Designation for ACM-101",
        "ACMB",
        pd,
    );
    let l = c
        .ingest("wc", &late, t0 + DEDUPE_WINDOW_NS + 2 * SEC)
        .unwrap();
    assert_eq!(l.items.len(), 1);
}

#[test]
fn venue_must_match_the_asset_exchange() {
    let mut c = Collector::new(universe());
    let now = ns("2026-11-23T13:00:30Z");
    // ZEPP is NYSE in the universe: a NASDAQ: ZEPP mention does not map.
    let x = one_item_rss("z", "Zepp news", "ZEPP", "Mon, 23 Nov 2026 13:00:00 +0000");
    let out = c.ingest("w", &x, now).unwrap();
    assert_eq!(out.records[0].listings, ["NASDAQ:ZEPP"]);
    assert_eq!(out.records[0].status, "no_symbol");
    // Not in the universe at all: dropped.
    let y = one_item_rss(
        "q",
        "Unknown news",
        "QQQQX",
        "Mon, 23 Nov 2026 13:00:00 +0000",
    );
    assert_eq!(
        c.ingest("w", &y, now).unwrap().records[0].status,
        "no_symbol"
    );
}

#[test]
fn ticker_rules() {
    let l = |v, t: &str| Listing {
        venue: v,
        ticker: t.into(),
    };
    assert_eq!(
        listings("Acme (NASDAQ: ABCD) said"),
        [l(Venue::Nasdaq, "ABCD")]
    );
    assert_eq!(listings("(Nasdaq:ABCD)"), [l(Venue::Nasdaq, "ABCD")]);
    assert_eq!(
        listings("(NYSE American: ABC)"),
        [l(Venue::NyseAmerican, "ABC")]
    );
    assert_eq!(listings("(NYSE: BRK.B)"), [l(Venue::Nyse, "BRK.B")]);
    assert_eq!(listings("(NYSE: BRK-B)"), [l(Venue::Nyse, "BRK.B")]);
    assert_eq!(
        listings("(NASDAQ: ABCD, ABCDW)"),
        [l(Venue::Nasdaq, "ABCD"), l(Venue::Nasdaq, "ABCDW")]
    );
    assert_eq!(
        listings("(TSX-V: ABC; NASDAQ: ABCD)"),
        [l(Venue::Nasdaq, "ABCD")]
    );
    assert_eq!(listings("(TSX: ABC, NYSE: ABC)"), [l(Venue::Nyse, "ABC")]);
    assert_eq!(
        listings("(OTCQB: ABCDF) (OTC Pink: WXYZ)"),
        [l(Venue::Otc, "ABCDF"), l(Venue::Otc, "WXYZ")]
    );
    assert_eq!(listings("(NasdaqCM: ABCD)"), [l(Venue::Nasdaq, "ABCD")]);
    // Repeated mentions collapse.
    assert_eq!(
        listings("(NASDAQ: A) and (NASDAQ: A)"),
        [l(Venue::Nasdaq, "A")]
    );
    // Continuations after a foreign venue stay unmapped.
    assert!(listings("(TSXV: ABC, ABCD)").is_empty());
    // Never bare words, never outside parentheses, never class-ticker fragments.
    for t in [
        "NASDAQ: ABCD",
        "Acme ABCD rose",
        "(ABCD)",
        "(NYSE: FHN PR E)",
        "(NYSE: MS/PL)",
        "(NYSE: abc)",
        "(NYSE: AB1)",
        "(NYSE: TOOLONG)",
        "(NYSE: )",
        "(the \"Company\")",
        "(HKEX stock code: 2552)",
        "(ASX: ABC)",
        "(NASDAQ: ABCD",
    ] {
        assert!(listings(t).is_empty(), "{t}: {:?}", listings(t));
    }
    assert_eq!(ticker_of("ABC.WS").as_deref(), Some("ABC.WS"));
    assert_eq!(ticker_of("ABC.WSX"), None);
    assert_eq!(venue_of(" nyse   american "), Some(Venue::NyseAmerican));
    assert_eq!(venue_of("NYSE-American"), Some(Venue::NyseAmerican));
    assert!(Venue::NyseAmerican.matches("AMEX") && !Venue::Nyse.matches("AMEX"));
}

#[test]
fn offering_headlines() {
    for h in [
        "Acme Announces Pricing of $5 Million Public Offering",
        "Acme Announces $3.2 Million Registered Direct Offering Priced At-the-Market",
        "Acme Enters Into Warrant Inducement Agreement",
        "Acme Announces Closing of Private Placement",
        "Acme Establishes At The Market Offering Program",
        "Acme Announces Proposed Offering of Common Stock",
        "Acme Secures up to S$32 Million Share Subscription Facility from GEM",
        "Acme Announces Immediate Exercise of Warrants for $4 Million Gross Proceeds",
    ] {
        assert_eq!(classify(h), WireKind::Offering, "{h}");
    }
    for h in [
        "Acme Receives FDA Fast Track Designation",
        "Acme Announces Positive Topline Results from Phase 2 Trial",
        "Acme Expands Its Product Offerings in Europe",
        "Acme Reports Third Quarter 2026 Results",
        "Acme Signs $40 Million Supply Agreement",
    ] {
        assert_eq!(classify(h), WireKind::Catalyst, "{h}");
    }
}

#[test]
fn law_firm_solicitations_are_noise() {
    // Real headline shape from the 2026-11-23 PR Newswire sample.
    let mut c = Collector::new(universe());
    let now = ns("2026-11-23T13:00:30Z");
    let x = one_item_rss(
        "lf",
        "SHAREHOLDER ALERT: Bronstein, Gewirtz &amp; Grossman, LLC Announces Investigation of EyePoint, Inc.",
        "EYPT",
        "Mon, 23 Nov 2026 13:00:00 +0000",
    );
    let out = c.ingest("w", &x, now).unwrap();
    assert_eq!(out.records[0].symbols, ["EYPT"]);
    assert_eq!(out.records[0].status, "noise");
    assert!(out.items.is_empty());
    assert!(classify::is_noise(
        "Acme Inc. Investor News",
        "The Gross Law Firm reminds..."
    ));
    for (h, s) in [
        (
            "Acme Announces Dismissal of Lawsuit",
            "Acme (NASDAQ: ACMB) said",
        ),
        ("Acme Reports Results", "Acme Investigational drug ACM-1"),
    ] {
        assert!(!classify::is_noise(h, s), "{h}");
    }
}

#[test]
fn config_switch_and_feeds() {
    let cfg = |pairs: &[(&str, &str)]| {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        WiresConfig::from_lookup(|k| m.get(k).cloned())
    };
    let off = cfg(&[]).unwrap();
    assert!(!off.on && off.feeds.is_empty() && off.poll == MIN_POLL);
    let on = cfg(&[
        ("WIRES_NEWS", "1"),
        (
            "WIRES_FEEDS",
            "a=https://x.invalid/a.rss, b_2=https://x.invalid/b",
        ),
        ("WIRES_POLL_SECS", "5"),
    ])
    .unwrap();
    assert!(on.on);
    assert_eq!(on.feeds.len(), 2);
    assert_eq!(on.feeds[1].id, "b_2");
    assert_eq!(on.poll, MIN_POLL, "never under 60 s");
    assert!(!cfg(&[("WIRES_NEWS", "0")]).unwrap().on);
    assert!(cfg(&[("WIRES_NEWS", "yes please")]).is_err());
    assert!(cfg(&[("WIRES_FEEDS", "a=http://x.invalid/")]).is_err());
    assert!(cfg(&[("WIRES_FEEDS", "A=https://x.invalid/")]).is_err());
    assert!(cfg(&[("WIRES_FEEDS", "https://x.invalid/")]).is_err());
    assert!(cfg(&[("WIRES_FEEDS", "a=https://x/1,a=https://x/2")]).is_err());
}

#[test]
fn backoff_doubles_to_a_ceiling() {
    let p = MIN_POLL;
    assert_eq!(backoff(p, 0), p);
    assert_eq!(backoff(p, 1), p * 2);
    assert_eq!(backoff(p, 3), p * 8);
    assert_eq!(backoff(p, 50), Duration::from_secs(1800));
}

#[test]
fn archive_round_trip_and_restart_keeps_first_seen_times() {
    let dir = tempfile::tempdir().unwrap();
    let wdir = wires_dir(dir.path());
    let t0 = ns("2026-11-23T02:26:00Z");
    let mut c = Collector::new(universe());
    let first = c.ingest("prn", PRN, t0).unwrap();
    append_records(&wdir, &first.records).unwrap();
    assert!(wdir.join("2026-11.jsonl").exists());
    // A torn last line (a crash mid-write) is skipped, not fatal.
    std::fs::OpenOptions::new()
        .append(true)
        .open(wdir.join("2026-11.jsonl"))
        .unwrap()
        .write_all(b"{\"v\":1,\"feed\":")
        .unwrap();

    // Restart three minutes later.
    let t1 = t0 + 3 * 60 * SEC;
    let recs = read_recent_records(&wdir, t1);
    assert_eq!(recs, first.records);
    let mut c2 = Collector::new(universe());
    let again = c2.restore(&recs, t1);
    assert_eq!(
        again, first.items,
        "restored with the original first-seen times"
    );
    // The feed still lists the same entries: nothing re-archived, nothing re-stamped.
    let poll = c2.ingest("prn", PRN, t1).unwrap();
    assert!(poll.records.is_empty() && poll.items.is_empty());
    // A copy on another wire right after the restart is still a duplicate.
    let dup = one_item_rss(
        "x",
        "Zepp Health Corporation Reports Third Quarter of 2026 Unaudited Financial Results",
        "ZEPP",
        "Mon, 23 Nov 2026 01:00:00 +0000",
    )
    .replace("NASDAQ", "NYSE");
    let d = c2.ingest("other", &dup, t1 + 60 * SEC).unwrap();
    assert_eq!(d.records[0].status, "duplicate");

    // More than a day later nothing is current.
    let mut c3 = Collector::new(universe());
    assert!(c3.restore(&recs, t0 + MAX_AGE_NS + SEC).is_empty());
}
