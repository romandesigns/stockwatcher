//! The FMP news worker (`market_providers::import::fmp_news`) on recorded-shape fixtures: the
//! switch, the parser, Eastern wall time to UTC across both DST changes, the paging stop rule,
//! strict ticker mapping, classification, the record-only kind, and end to end through the
//! archive, the seam and the news pillar (a record-only item changes no decision). No network.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use market_archive::evidence_load::source_news_items;
use market_archive::{Archive, ArchiveConfig, DailyCloses};
use market_providers::import::fmp_news::*;
use market_providers::import::http::{Http, HttpResponse};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use stock_context::{EvidenceStore, InstrumentId, NewsCoverage, Policy, Tri};

const SEC: i64 = 1_000_000_000;
const KEY: &str = "test-key-123456";

fn utc(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone(&Utc)
        .timestamp_nanos_opt()
        .unwrap()
}

fn archive() -> (tempfile::TempDir, Archive) {
    let dir = tempfile::tempdir().unwrap();
    let a = Archive::new(ArchiveConfig {
        root: dir.path().to_path_buf(),
        retention_days: 0,
        min_free_bytes: 0,
    });
    (dir, a)
}

fn with_universe(a: &Archive, syms: &[&str]) {
    let mut closes = DailyCloses::new();
    let day: BTreeMap<String, f64> = syms.iter().map(|s| (s.to_string(), 3.0)).collect();
    closes.insert(NaiveDate::from_ymd_opt(2026, 11, 20).unwrap(), day);
    a.write_daily_closes(&closes).unwrap();
}

fn cfg(extra: &[(&str, &str)]) -> FmpNewsConfig {
    let extra: Vec<(String, String)> = extra
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    match FmpNewsConfig::from_lookup(|k| {
        if k == "FMP_API_KEY" {
            return Some(KEY.into());
        }
        if k == "FMP_BASE" {
            return Some("https://fmp.test".into());
        }
        extra.iter().find(|(e, _)| e == k).map(|(_, v)| v.clone())
    })
    .unwrap()
    {
        FmpNewsSwitch::On(c) => c,
        FmpNewsSwitch::Off(l) => panic!("off: {l}"),
    }
}

#[derive(Default)]
struct Script {
    replies: VecDeque<Result<HttpResponse, String>>,
    urls: Vec<String>,
}

impl Http for Script {
    fn get(&mut self, url: &str, _headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.urls.push(url.to_string());
        self.replies
            .pop_front()
            .unwrap_or_else(|| panic!("the script ran out of replies at {url}"))
    }
}

fn ok(body: String) -> Result<HttpResponse, String> {
    Ok(HttpResponse {
        status: 200,
        body: body.into_bytes(),
        retry_after: None,
    })
}

fn item(symbol: &str, published: &str, publisher: &str, title: &str, url: &str) -> String {
    serde_json::json!({
        "symbol": symbol,
        "publishedDate": published,
        "publisher": publisher,
        "title": title,
        "image": "https://images.test/x.jpg",
        "site": "prnewswire.com",
        "text": "The company announced today ...",
        "url": url,
        "somethingNew": {"ignored": true}
    })
    .to_string()
}

fn page(items: &[String]) -> String {
    format!("[{}]", items.join(","))
}

#[test]
fn the_switch_is_on_with_a_key_record_only_by_default() {
    assert!(matches!(
        FmpNewsConfig::from_lookup(|_| None).unwrap(),
        FmpNewsSwitch::Off(OFF_NO_KEY)
    ));
    assert!(matches!(
        FmpNewsConfig::from_lookup(|k| match k {
            "FMP_API_KEY" => Some(KEY.into()),
            "FMP_NEWS" => Some("0".into()),
            _ => None,
        })
        .unwrap(),
        FmpNewsSwitch::Off(OFF_SWITCHED)
    ));
    let c = cfg(&[]);
    assert!(!c.count, "record-only unless FMP_NEWS_COUNT=1");
    assert_eq!((c.poll_secs, c.idle_poll_secs, c.limit), (60, 600, 100));
    assert!(
        c.on_line()
            .contains("FMP news ON (record-only; FMP_NEWS_COUNT=1 makes it count)"),
        "{}",
        c.on_line()
    );
    assert!(!c.on_line().contains(KEY));
    assert!(cfg(&[("FMP_NEWS_COUNT", "1")]).count);
    assert!(
        FmpNewsConfig::from_lookup(|k| match k {
            "FMP_API_KEY" => Some(KEY.into()),
            "FMP_NEWS_COUNT" => Some("maybe".into()),
            _ => None,
        })
        .is_err()
    );
}

#[test]
fn the_parser_keeps_usable_items_and_counts_the_rest() {
    let body = format!(
        "[{},{},{},{},{},{},{}]",
        item(
            "ACME",
            "2026-11-23 00:26:00",
            "PRNewsWire",
            "Acme Announces Positive Topline Results",
            "https://www.prnewswire.com/a"
        ),
        item(
            "BRK-B",
            "2026-11-23 00:20:00",
            "GlobeNewsWire",
            "Berkshire update",
            "https://www.globenewswire.com/b"
        ),
        r#"{"publishedDate":"2026-11-23 00:10:00","title":"No symbol","url":"u"}"#,
        r#"{"symbol":"ACME","title":"No date","url":"u"}"#,
        r#"{"symbol":"ACME","publishedDate":"yesterday","title":"Bad date","url":"u"}"#,
        // A non-ET-looking date: an explicit UTC stamp is read as UTC, not as Eastern.
        r#"{"symbol":"ACME","publishedDate":"2026-11-23T05:26:00.000Z","title":"Zulu","url":"z"}"#,
        r#"{"symbol":"ACME","publishedDate":"2026-11-23 00:00:00","title":"","url":"u"}"#,
    );
    let p = parse_page(body.as_bytes()).unwrap();
    assert_eq!(p.raw, 7);
    assert_eq!(p.items.len(), 3);
    assert_eq!(p.items[0].published_at, utc("2026-11-23T05:26:00Z"));
    assert_eq!(p.items[0].publisher, "PRNewsWire");
    assert_eq!(p.items[1].symbols, vec!["BRK-B"]);
    assert_eq!(p.items[2].title, "Zulu");
    assert_eq!(p.items[2].published_at, utc("2026-11-23T05:26:00Z"));
    let want: BTreeMap<&str, u64> = [
        ("missing_symbol", 1),
        ("missing_date", 1),
        ("bad_date", 1),
        ("missing_title", 1),
    ]
    .into();
    assert_eq!(p.skipped, want);
    // FMP's refusal arrives as an object with a 200.
    let e = parse_page(br#"{"Error Message":"Limit Reach . Please upgrade your plan"}"#);
    assert!(e.unwrap_err().contains("Limit Reach"));
    assert!(parse_page(b"<html>").is_err());
    assert_eq!(parse_page(b"[]").unwrap().raw, 0);
}

#[test]
fn eastern_wall_time_converts_across_both_dst_changes() {
    // Winter (EST, -5): the live sample, 7 minutes old at 05:33 UTC.
    assert_eq!(
        parse_published("2026-11-23 00:26:00"),
        Some(utc("2026-11-23T05:26:00Z"))
    );
    // Summer (EDT, -4).
    assert_eq!(
        parse_published("2026-07-01 09:30:00"),
        Some(utc("2026-07-01T13:30:00Z"))
    );
    // Spring forward, 2026-03-08: 01:59 EST, 03:00 EDT; 02:30 does not exist and reads as 03:30.
    assert_eq!(
        parse_published("2026-03-08 01:59:00"),
        Some(utc("2026-03-08T06:59:00Z"))
    );
    assert_eq!(
        parse_published("2026-03-08 03:00:00"),
        Some(utc("2026-03-08T07:00:00Z"))
    );
    assert_eq!(
        parse_published("2026-03-08 02:30:00"),
        Some(utc("2026-03-08T07:30:00Z"))
    );
    // Fall back, 2026-11-01: 01:30 happens twice; the earlier (EDT) instant is taken.
    assert_eq!(
        parse_published("2026-11-01 01:30:00"),
        Some(utc("2026-11-01T05:30:00Z"))
    );
    assert_eq!(
        parse_published("2026-11-01 02:30:00"),
        Some(utc("2026-11-01T07:30:00Z"))
    );
    // Other spellings.
    assert_eq!(
        parse_published("2026-11-23T00:26:00"),
        Some(utc("2026-11-23T05:26:00Z"))
    );
    assert_eq!(
        parse_published("2026-11-23T00:26:00-05:00"),
        Some(utc("2026-11-23T05:26:00Z"))
    );
    assert_eq!(parse_published("23/11/2026"), None);
}

#[test]
fn paging_continues_only_while_a_full_page_is_all_new() {
    assert!(next_page_wanted(&[false, false, false], 3, 1, 3));
    assert!(!next_page_wanted(&[false, true, false], 3, 1, 3), "overlap");
    assert!(!next_page_wanted(&[false, false], 3, 1, 3), "short page");
    assert!(!next_page_wanted(&[false, false, false], 3, 3, 3), "cap");
    assert!(!next_page_wanted(&[], 0, 1, 3), "empty");
}

#[test]
fn tickers_map_whole_and_strictly_to_the_universe() {
    let u: BTreeSet<String> = ["ACME", "BRK.B", "USB.PRA"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(map_symbol("ACME", &u), Ok("ACME".into()));
    assert_eq!(map_symbol("acme", &u), Ok("ACME".into()));
    assert_eq!(map_symbol("BRK-B", &u), Ok("BRK.B".into()));
    assert_eq!(map_symbol("USB-PA", &u), Ok("USB.PRA".into()));
    assert_eq!(map_symbol("RY.TO", &u), Err("bad_notation"));
    assert_eq!(map_symbol("FHN PR E", &u), Err("bad_notation"));
    assert_eq!(map_symbol("ZZZZ", &u), Err("not_in_universe"));
    assert_eq!(
        map_symbol("BRK", &u),
        Err("not_in_universe"),
        "never a prefix"
    );
    assert_eq!(map_symbol("ACME", &BTreeSet::new()), Err("no_universe"));
}

#[test]
fn classification_catches_solicitations_offerings_and_third_parties() {
    let c = |t: &str, p: &str| classify(t, "", p, "");
    assert_eq!(
        c(
            "Qfin Holdings, Inc. Sued for Securities Law Violations - Contact the DJS Law Group to Discuss Your Rights - QFIN",
            "GlobeNewsWire"
        ),
        FmpClass::Noise
    );
    assert_eq!(
        c(
            "SHAREHOLDER ALERT: Pomerantz Law Firm Investigates Claims On Behalf of Investors of Acme (NASDAQ: ACME)",
            "PRNewsWire"
        ),
        FmpClass::Noise
    );
    assert_eq!(
        c(
            "Acme Class Action Filed: Investors Who Lost Money Should Contact ...",
            "Business Wire"
        ),
        FmpClass::Noise
    );
    assert_eq!(
        classify(
            "Important notice for Acme shareholders",
            "The Rosen Law Firm, a global investor rights law firm, reminds ...",
            "GlobeNewsWire",
            ""
        ),
        FmpClass::Noise,
        "a solicitation found by its body"
    );
    assert_eq!(
        c(
            "Acme Announces Pricing of $5 Million Registered Direct Offering",
            "GlobeNewsWire"
        ),
        FmpClass::Offering
    );
    assert_eq!(
        c("Acme Announces $8 Million Private Placement", "ACCESSWIRE"),
        FmpClass::Offering
    );
    assert_eq!(
        c(
            "Acme Announces Positive Topline Results from Phase 2 Trial",
            "PRNewsWire"
        ),
        FmpClass::Catalyst
    );
    assert_eq!(
        c("Acme Signs Supply Agreement", "Newsfile Corp"),
        FmpClass::Catalyst
    );
    assert_eq!(
        c("Why Acme Stock Is Soaring Today", "Motley Fool"),
        FmpClass::ThirdParty
    );
    assert_eq!(
        c("Acme (ACME) Upgraded to Buy", "Zacks Investment Research"),
        FmpClass::ThirdParty
    );
    assert_eq!(
        classify("Acme Signs Deal", "", "", "businesswire.com"),
        FmpClass::Catalyst,
        "a blank publisher falls back to the site"
    );
    assert_eq!(source_slug("PRNewsWire"), "prnewswire");
    assert_eq!(source_slug("Business Wire"), "businesswire");
}

#[test]
fn record_only_never_gives_a_kind_that_can_pass() {
    for class in [
        FmpClass::Catalyst,
        FmpClass::Offering,
        FmpClass::Noise,
        FmpClass::ThirdParty,
    ] {
        for stale in [false, true] {
            let (kind, et) = kind_of(class, false, stale);
            assert_eq!(kind, "recorded");
            assert_eq!(et, Some(format!("shadow_{}", class.as_str())));
        }
    }
    assert_eq!(kind_of(FmpClass::Catalyst, true, false), ("catalyst", None));
    assert_eq!(kind_of(FmpClass::Offering, true, false), ("offering", None));
    assert_eq!(
        kind_of(FmpClass::Noise, true, false),
        ("recorded", Some("noise".into()))
    );
    assert_eq!(
        kind_of(FmpClass::ThirdParty, true, false),
        ("recorded", Some("third_party".into()))
    );
    assert_eq!(
        kind_of(FmpClass::Catalyst, true, true),
        ("recorded", Some("stale_catalyst".into()))
    );
}

/// Ticks the collector once at `at` with `replies`, returning the report and the URLs asked.
fn tick_at(
    c: &mut FmpNewsCollector,
    a: &Archive,
    at: DateTime<Utc>,
    replies: Vec<String>,
) -> (PollReport, Vec<String>) {
    let mut http = Script {
        replies: replies.into_iter().map(ok).collect(),
        ..Script::default()
    };
    let (r, _) = c.tick(a, &mut http, &mut || at, &mut |_| {}, &mut |_| {});
    assert!(
        http.replies.is_empty(),
        "every scripted reply was asked for"
    );
    (r, http.urls)
}

#[test]
fn a_poll_pages_records_once_and_resumes_after_a_restart() {
    let (_d, a) = archive();
    with_universe(&a, &["ACME", "BETA", "GAMA", "BRK.B"]);
    let c0 = cfg(&[("FMP_NEWS_LIMIT", "3")]);
    // 05:33 UTC Monday = 00:33 ET: outside 04:00-20:00, so the idle cadence.
    let t1 = Utc.with_ymd_and_hms(2026, 11, 23, 5, 33, 0).unwrap();
    let mut c = FmpNewsCollector::open(c0.clone(), &a, t1, &mut |_| {});
    let p0 = page(&[
        item(
            "ACME",
            "2026-11-23 00:26:00",
            "PRNewsWire",
            "Acme Wins Contract",
            "https://x/1",
        ),
        item(
            "BRK-B",
            "2026-11-23 00:25:00",
            "GlobeNewsWire",
            "Berkshire Note",
            "https://x/2",
        ),
        item(
            "NOPE",
            "2026-11-23 00:24:00",
            "PRNewsWire",
            "Not ours",
            "https://x/3",
        ),
    ]);
    let p1 = page(&[item(
        "BETA",
        "2026-11-23 00:20:00",
        "Zacks Investment Research",
        "Beta Earnings Preview",
        "https://x/4",
    )]);
    let (r, urls) = tick_at(&mut c, &a, t1, vec![p0.clone(), p1]);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(urls.len(), 2, "a full all-new page asks for the next");
    assert!(
        urls[0].starts_with("https://fmp.test/stable/news/stock-latest?page=0&limit=3&apikey=")
    );
    assert!(urls[1].contains("page=1&limit=3"));
    assert_eq!((r.items, r.mapped, r.written), (4, 3, 3));
    assert_eq!(r.dropped.get("not_in_universe"), Some(&1));
    assert_eq!(
        r.newest_lag_ns,
        Some(7 * 60 * SEC),
        "fetch time minus newest publication"
    );
    assert!(!r.line().contains(KEY));

    let stored = a.read_source_news(None).unwrap();
    assert_eq!(stored.len(), 3);
    for s in &stored {
        assert_eq!((s.provider.as_str(), s.rules.as_str()), (PROVIDER, RULES));
        assert_eq!(s.kind, "recorded", "record-only");
        assert_eq!(s.first_observed_at, t1.timestamp_nanos_opt().unwrap());
        assert!(s.published_at < s.first_observed_at);
    }
    let acme = stored.iter().find(|s| s.symbols == ["ACME"]).unwrap();
    assert_eq!(acme.source, "fmp/prnewswire");
    assert_eq!(acme.event_type.as_deref(), Some("shadow_catalyst"));
    assert_eq!(acme.published_at, utc("2026-11-23T05:26:00Z"));
    assert_eq!(acme.headline.as_deref(), Some("Acme Wins Contract"));
    assert_eq!(acme.url.as_deref(), Some("https://x/1"));
    assert!(stored.iter().any(|s| s.symbols == ["BRK.B"]));
    let beta = stored.iter().find(|s| s.symbols == ["BETA"]).unwrap();
    assert_eq!(beta.event_type.as_deref(), Some("shadow_third_party"));

    // Not due yet: nothing asked.
    let (r, urls) = tick_at(&mut c, &a, t1 + chrono::Duration::seconds(30), vec![]);
    assert!(!r.polled && urls.is_empty());

    // Ten minutes on: one new item on top of what was seen; no second page.
    let t2 = t1 + chrono::Duration::seconds(600);
    let p0b = page(&[
        item(
            "GAMA",
            "2026-11-23 00:40:00",
            "PRNewsWire",
            "Gama Launches Product",
            "https://x/5",
        ),
        item(
            "ACME",
            "2026-11-23 00:26:00",
            "PRNewsWire",
            "Acme Wins Contract",
            "https://x/1",
        ),
        item(
            "BRK-B",
            "2026-11-23 00:25:00",
            "GlobeNewsWire",
            "Berkshire Note",
            "https://x/2",
        ),
    ]);
    let (r, urls) = tick_at(&mut c, &a, t2, vec![p0b.clone()]);
    assert_eq!(urls.len(), 1, "overlap stops the paging");
    assert_eq!(r.written, 1);
    assert_eq!(a.read_source_news(None).unwrap().len(), 4);

    // A restart reads what is stored: the same page writes nothing.
    let mut c2 = FmpNewsCollector::open(c0, &a, t2, &mut |_| {});
    assert_eq!(c2.seen_len(), 4);
    let (r, urls) = tick_at(&mut c2, &a, t2 + chrono::Duration::seconds(5), vec![p0b]);
    assert_eq!((urls.len(), r.written), (1, 0));
    assert_eq!(a.read_source_news(None).unwrap().len(), 4);
}

#[test]
fn a_refusal_is_an_error_with_the_key_redacted_and_backs_off() {
    let (_d, a) = archive();
    with_universe(&a, &["ACME"]);
    let t = Utc.with_ymd_and_hms(2026, 11, 23, 15, 0, 0).unwrap(); // 10:00 ET
    let mut c = FmpNewsCollector::open(cfg(&[]), &a, t, &mut |_| {});
    let mut http = Script::default();
    http.replies.push_back(Ok(HttpResponse {
        status: 403,
        body: format!("bad key {KEY}").into_bytes(),
        retry_after: None,
    }));
    let (r, summary) = c.tick(&a, &mut http, &mut || t, &mut |_| {}, &mut |_| {});
    assert_eq!(r.errors.len(), 1);
    assert!(!r.errors[0].contains(KEY), "{}", r.errors[0]);
    assert!(r.errors[0].contains("apikey=REDACTED"));
    let s = summary.expect("the first summary comes with the first poll");
    assert!(
        s.starts_with("fmp news summary (record-only): 1 polls (1 failed)"),
        "{s}"
    );
    // 60 s is the active cadence; one failure doubles it.
    let (r, _) = c.tick(
        &a,
        &mut Script::default(),
        &mut || t + chrono::Duration::seconds(90),
        &mut |_| {},
        &mut |_| {},
    );
    assert!(!r.polled, "backed off past 60 s");
    // A 429 is retried after its Retry-After, then succeeds.
    let mut http = Script::default();
    http.replies.push_back(Ok(HttpResponse {
        status: 429,
        body: Vec::new(),
        retry_after: Some(std::time::Duration::from_secs(7)),
    }));
    http.replies.push_back(ok("[]".into()));
    let mut slept = Vec::new();
    let (r, _) = c.tick(
        &a,
        &mut http,
        &mut || t + chrono::Duration::seconds(121),
        &mut |d| slept.push(d),
        &mut |_| {},
    );
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.requests, 2);
    assert!(slept.contains(&std::time::Duration::from_secs(7)));
}

#[test]
fn counting_mode_and_stale_items() {
    let (_d, a) = archive();
    with_universe(&a, &["ACME", "BETA", "GAMA"]);
    let t = Utc.with_ymd_and_hms(2026, 11, 23, 15, 0, 0).unwrap();
    let mut c = FmpNewsCollector::open(cfg(&[("FMP_NEWS_COUNT", "1")]), &a, t, &mut |_| {});
    let p = page(&[
        item(
            "ACME",
            "2026-11-23 09:55:00",
            "PRNewsWire",
            "Acme Wins Contract",
            "https://x/1",
        ),
        item(
            "BETA",
            "2026-11-23 09:50:00",
            "GlobeNewsWire",
            "Beta Announces Pricing of $4 Million Public Offering",
            "https://x/2",
        ),
        // First seen 30 h after publication: a back-fill, never news.
        item(
            "GAMA",
            "2026-11-22 03:00:00",
            "PRNewsWire",
            "Gama Old News",
            "https://x/3",
        ),
    ]);
    let (r, _) = tick_at(&mut c, &a, t, vec![p]);
    assert_eq!(r.written, 3);
    let s = a.read_source_news(None).unwrap();
    let get = |sym: &str| s.iter().find(|r| r.symbols == [sym]).unwrap();
    assert_eq!(
        (get("ACME").kind.as_str(), get("ACME").event_type.clone()),
        ("catalyst", None)
    );
    assert_eq!(get("BETA").kind, "offering");
    assert_eq!(get("GAMA").kind, "recorded");
    assert_eq!(get("GAMA").event_type.as_deref(), Some("stale_catalyst"));
}

/// The news pillar for `sym` at `cutoff` over the records stored in `a`, with market-wide
/// coverage over the whole freshness window (so "no news" is a FAIL, not UNKNOWN).
fn pillar(a: &Archive, sym: &str, cutoff: i64) -> Tri {
    let policy = Policy::current();
    let mut store = EvidenceStore::default();
    for n in source_news_items(&a.read_source_news(None).unwrap(), None) {
        store.add_news(n);
    }
    store.add_coverage(NewsCoverage {
        id: "cov".into(),
        instrument: InstrumentId::market(),
        sources: vec!["alpaca/benzinga".into()],
        covered_from: cutoff - 30 * 3600 * SEC,
        covered_through: cutoff,
        observed_at: cutoff,
    });
    let (r, _) = stock_context::evaluate::news(&policy, &store, &InstrumentId::symbol(sym), cutoff);
    r.state
}

#[test]
fn a_record_only_item_changes_no_news_pillar_decision() {
    let t = Utc.with_ymd_and_hms(2026, 11, 23, 15, 0, 0).unwrap();
    let cutoff = (t + chrono::Duration::minutes(5))
        .timestamp_nanos_opt()
        .unwrap();
    let p = page(&[item(
        "ACME",
        "2026-11-23 09:55:00",
        "PRNewsWire",
        "Acme Announces Positive Topline Results",
        "https://x/1",
    )]);

    // Without the item: FAIL (covered, nothing found).
    let (_d0, empty) = archive();
    assert_eq!(pillar(&empty, "ACME", cutoff), Tri::Fail);

    // Record-only: the same catalyst is stored, loaded through the seam, and still FAILs.
    let (_d, a) = archive();
    with_universe(&a, &["ACME"]);
    let mut c = FmpNewsCollector::open(cfg(&[]), &a, t, &mut |_| {});
    assert_eq!(tick_at(&mut c, &a, t, vec![p.clone()]).0.written, 1);
    assert_eq!(
        source_news_items(&a.read_source_news(None).unwrap(), None).len(),
        1
    );
    assert_eq!(
        pillar(&a, "ACME", cutoff),
        Tri::Fail,
        "record-only must not pass"
    );

    // FMP_NEWS_COUNT=1: the same item passes, so the test above can see a change.
    let (_d2, b) = archive();
    with_universe(&b, &["ACME"]);
    let mut c = FmpNewsCollector::open(cfg(&[("FMP_NEWS_COUNT", "1")]), &b, t, &mut |_| {});
    assert_eq!(tick_at(&mut c, &b, t, vec![p]).0.written, 1);
    assert_eq!(pillar(&b, "ACME", cutoff), Tri::Pass);
    // ... and never before we first saw it.
    assert_eq!(
        pillar(&b, "ACME", t.timestamp_nanos_opt().unwrap() - SEC),
        Tri::Fail
    );
}

#[test]
fn only_record_only_fmp_records_are_shadow() {
    let r = |kind: &str, et: Option<&str>, rules: &str| market_archive::SourceNewsRecord {
        v: 1,
        provider: PROVIDER.into(),
        source: "fmp/prnewswire".into(),
        id: "fmp:x".into(),
        version: 1,
        symbols: vec!["ACME".into()],
        published_at: 1,
        first_observed_at: 2,
        headline: None,
        url: None,
        kind: kind.into(),
        event_type: et.map(str::to_string),
        rules: rules.into(),
    };
    assert!(is_shadow(&r("recorded", Some("shadow_catalyst"), RULES)));
    assert!(!is_shadow(&r("catalyst", None, RULES)));
    assert!(!is_shadow(&r("recorded", Some("noise"), RULES)));
    assert!(!is_shadow(&r(
        "recorded",
        Some("shadow_catalyst"),
        "wire-rules-v1"
    )));
}
