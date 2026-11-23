//! The SEC EDGAR worker (`market_providers::import::edgar`) on recorded-shape fixtures: feed and
//! daily-index parsing, ticker notation, acceptance-time dating, form classification through the
//! archive and the loader, the restart back-fill and the switch that keeps it off without a
//! User-Agent. No network.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::America::New_York;
use market_archive::evidence_load::filing_items;
use market_archive::{Archive, ArchiveConfig, DATED_BY_ACCEPTANCE, DATED_BY_DAILY_INDEX};
use market_providers::import::edgar::*;
use market_providers::import::http::{Http, HttpResponse};
use std::cell::Cell;
use std::collections::VecDeque;

const SEC_NS: i64 = 1_000_000_000;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/edgar/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn ok(body: Vec<u8>) -> Result<HttpResponse, String> {
    Ok(HttpResponse {
        status: 200,
        body,
        retry_after: None,
    })
}

fn status(code: u16) -> Result<HttpResponse, String> {
    Ok(HttpResponse {
        status: code,
        body: b"not here".to_vec(),
        retry_after: None,
    })
}

#[derive(Default)]
struct Script {
    replies: VecDeque<Result<HttpResponse, String>>,
    urls: Vec<String>,
    agents: Vec<String>,
}

impl Http for Script {
    fn get(&mut self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.urls.push(url.to_string());
        self.agents.push(
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
                .map(|(_, v)| v.to_string())
                .unwrap_or_default(),
        );
        self.replies
            .pop_front()
            .unwrap_or_else(|| panic!("the script ran out of replies at {url}"))
    }
}

fn et(d: (i32, u32, u32), h: u32, m: u32, s: u32) -> DateTime<Utc> {
    New_York
        .with_ymd_and_hms(d.0, d.1, d.2, h, m, s)
        .unwrap()
        .with_timezone(&Utc)
}

fn ns(t: DateTime<Utc>) -> i64 {
    t.timestamp_nanos_opt().unwrap()
}

fn cfg() -> EdgarConfig {
    EdgarConfig::from_lookup(|k| match k {
        "EDGAR_USER_AGENT" => Some("TestCo market-data test@example.invalid".into()),
        "EDGAR_BASE" => Some("https://edgar.test".into()),
        _ => None,
    })
    .unwrap()
    .expect("on with a User-Agent")
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

#[test]
fn the_worker_stays_off_without_a_user_agent() {
    assert!(EdgarConfig::from_lookup(|_| None).unwrap().is_none());
    assert!(
        EdgarConfig::from_lookup(|k| (k == "EDGAR_USER_AGENT").then(|| "   ".to_string()))
            .unwrap()
            .is_none()
    );
    assert!(OFF_LINE.contains("EDGAR_USER_AGENT"));
    let c = cfg();
    assert_eq!(
        (c.poll_secs, c.per_minute),
        (60, 60),
        "one request a second at most"
    );
    assert!(
        EdgarConfig::from_lookup(|k| match k {
            "EDGAR_USER_AGENT" => Some("x y@z".into()),
            "EDGAR_POLL_SECS" => Some("1".into()),
            _ => None,
        })
        .is_err(),
        "a poll faster than 10 s is refused"
    );
}

#[test]
fn the_feed_parses_acceptance_times_roles_items_and_entities() {
    let xml = String::from_utf8(fixture("current-feed-p0.xml")).unwrap();
    let e = parse_current_feed(&xml);
    assert_eq!(e.len(), 8);
    let k = &e[0];
    assert_eq!(k.accession, "0001111111-26-000045");
    assert_eq!(
        (k.cik, k.form.as_str(), k.role.as_str()),
        (1111111, "8-K", "Filer")
    );
    assert_eq!(k.company, "ACME THERAPEUTICS INC");
    assert_eq!(k.accepted_raw, "2026-11-20T09:12:03-05:00");
    assert_eq!(k.accepted_ns, ns(et((2026, 11, 20), 9, 12, 3)));
    assert_eq!(k.filed, NaiveDate::from_ymd_opt(2026, 11, 20));
    assert_eq!(k.items, vec!["1.01", "7.01", "9.01"]);
    assert_eq!(
        k.url,
        "https://www.sec.gov/Archives/edgar/data/1111111/000111111126000045/0001111111-26-000045-index.htm"
    );
    assert_eq!(e[3].role, "Subject");
    assert_eq!(e[3].form, "SC 13D");
    assert_eq!(e[5].company, "A&B FOREIGN HOLDINGS LTD");
    assert!(parse_current_feed("<feed></feed>").is_empty());
    assert!(
        feed_url("https://edgar.test", 200)
            .contains("owner=exclude&start=200&count=100&output=atom")
    );
}

#[test]
fn class_tickers_map_whole_and_are_never_split() {
    assert_eq!(app_symbol("BRK-B").as_deref(), Some("BRK.B"));
    assert_eq!(app_symbol("USB-PA").as_deref(), Some("USB.PRA"));
    assert_eq!(app_symbol("KCAC-UN").as_deref(), Some("KCAC.U"));
    assert_eq!(app_symbol("ACMEW").as_deref(), Some("ACMEW"));
    assert_eq!(app_symbol("f").as_deref(), Some("F"));
    for bad in ["XYZ-WT", "FHN PR E", "MS/PL", "-B", "BRK-", "AB-C-D", ""] {
        assert_eq!(app_symbol(bad), None, "{bad:?}");
    }
    let m = TickerMap::parse(&fixture("company_tickers.json")).unwrap();
    assert_eq!(m.symbols(1111111), ["ACME", "ACMEW"]);
    assert_eq!(m.symbols(5555555), ["BRK.B"]);
    assert_eq!(m.symbols(7777777), ["KCAC.U", "USB.PRA"]);
    // The CIK whose tickers have no one-to-one notation maps to nothing at all: never to the
    // bare "F", "PR", "E", "MS" or "PL" a split would make.
    assert!(m.symbols(8888888).is_empty());
    assert!(TickerMap::parse(b"{}").is_err());
    assert!(TickerMap::parse(b"not json").is_err());
}

#[test]
fn the_daily_index_and_the_conservative_dating() {
    let text = String::from_utf8(fixture("form.20261119.idx")).unwrap();
    let rows = parse_daily_index(&text, "https://edgar.test");
    assert_eq!(rows.len(), 5);
    let k = rows.iter().find(|r| r.form == "8-K").unwrap();
    assert_eq!(
        (k.cik, k.accession.as_str()),
        (1111111, "0001111111-26-000041")
    );
    assert_eq!(k.filed, NaiveDate::from_ymd_opt(2026, 11, 19).unwrap());
    assert_eq!(
        k.url,
        "https://edgar.test/Archives/edgar/data/1111111/000111111126000041/0001111111-26-000041-index.htm"
    );
    assert_eq!(
        daily_index_url(
            "https://edgar.test",
            NaiveDate::from_ymd_opt(2026, 11, 19).unwrap()
        ),
        "https://edgar.test/Archives/edgar/daily-index/2026/QTR4/form.20261119.idx"
    );
    // Earliest acceptance: 17:30 ET on the previous SEC business day.
    let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
    assert_eq!(
        earliest_acceptance(d(2026, 11, 19)),
        ns(et((2026, 11, 18), 17, 30, 0))
    );
    // A Monday: Friday evening.
    assert_eq!(
        earliest_acceptance(d(2026, 11, 23)),
        ns(et((2026, 11, 20), 17, 30, 0))
    );
    // After Veterans Day (Wed 2026-11-11: the exchange trades, EDGAR is closed): Tuesday.
    assert_eq!(
        earliest_acceptance(d(2026, 11, 12)),
        ns(et((2026, 11, 10), 17, 30, 0))
    );
    // After Columbus Day (Mon 2026-10-12): the Friday before.
    assert_eq!(
        earliest_acceptance(d(2026, 10, 13)),
        ns(et((2026, 10, 9), 17, 30, 0))
    );
    // After Thanksgiving (Thu 2026-11-26, both closed): Wednesday.
    assert_eq!(
        earliest_acceptance(d(2026, 11, 27)),
        ns(et((2026, 11, 25), 17, 30, 0))
    );
    let html = String::from_utf8(fixture("filing-index-0001111111-26-000041.htm")).unwrap();
    assert_eq!(
        parse_accepted(&html),
        Some((
            "2026-11-19 16:31:07".to_string(),
            ns(et((2026, 11, 19), 16, 31, 7))
        ))
    );
    assert_eq!(parse_accepted("<html></html>"), None);
}

#[test]
fn a_poll_keeps_the_classified_mapped_filings_once_and_dates_them_by_acceptance() {
    let (_d, a) = archive();
    let now = et((2026, 11, 20), 9, 20, 0);
    let clock = Cell::new(now);
    let mut c = EdgarCollector::open(cfg(), &a, now).unwrap();
    let mut http = Script::default();
    http.replies.push_back(ok(fixture("company_tickers.json")));
    http.replies.push_back(ok(fixture("current-feed-p0.xml")));
    let r = c.tick(&a, &mut http, &mut || clock.get(), &mut |_| {}, &mut |_| {});
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.requests, 2, "the ticker map and one feed page");
    assert!(r.complete, "a short page is the end of the feed");
    assert_eq!(
        http.urls[0],
        "https://edgar.test/files/company_tickers.json"
    );
    assert!(http.urls[1].starts_with("https://edgar.test/cgi-bin/browse-edgar?action=getcurrent"));
    assert!(
        http.agents
            .iter()
            .all(|u| u == "TestCo market-data test@example.invalid")
    );

    let mut kept = a.read_filings(None).unwrap();
    kept.sort_by(|x, y| x.accession.cmp(&y.accession));
    let forms: Vec<(&str, &str, Vec<&str>)> = kept
        .iter()
        .map(|r| {
            (
                r.accession.as_str(),
                r.form.as_str(),
                r.symbols.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    // The 8-K (both tickers), the co-registered S-3 (once, both filers' tickers), the 424B5
    // and the 6-K. Not the SC 13D, the 10-Q, nor the 8-K whose CIK has no ticker.
    assert_eq!(
        forms,
        vec![
            ("0001111111-26-000045", "8-K", vec!["ACME", "ACMEW"]),
            ("0001193125-26-470001", "424B5", vec!["DILU"]),
            ("0002222222-26-000007", "S-3", vec!["DILU", "COREG"]),
            ("0003333333-26-000101", "6-K", vec!["FRGN"]),
        ]
    );
    let k = &kept[0];
    assert_eq!(k.accepted_at_ns, ns(et((2026, 11, 20), 9, 12, 3)));
    assert_eq!(k.dated_by, DATED_BY_ACCEPTANCE);
    assert_eq!(
        k.first_observed_at,
        ns(now),
        "our sighting, never the acceptance"
    );
    let cov = a.read_filings_coverage(None).unwrap();
    assert_eq!(cov.len(), 1);
    assert_eq!(cov[0].provider, "sec_edgar");
    assert_eq!(cov[0].covered_through, ns(now) - 120 * SEC_NS);
    // The pillar's view: the 8-K and 6-K are substantive, the offerings flagged, never a pass.
    let items = filing_items(&kept, None);
    let acme = items
        .iter()
        .find(|n| n.instrument == stock_context::InstrumentId::symbol("ACME"))
        .unwrap();
    assert!(acme.substantive && acme.published_at == Some(k.accepted_at_ns));
    assert!(
        items
            .iter()
            .filter(|n| n.instrument == stock_context::InstrumentId::symbol("DILU"))
            .all(|n| !n.substantive && n.event_type == stock_context::OFFERING_EVENT)
    );
    // Nothing else is stored in the Benzinga directories.
    assert!(a.read_news(None).unwrap().is_empty());
    assert!(a.read_news_coverage(None).unwrap().is_empty());

    // Before the next poll is due: no request at all.
    clock.set(now + chrono::Duration::seconds(30));
    let r = c.tick(&a, &mut http, &mut || clock.get(), &mut |_| {}, &mut |_| {});
    assert_eq!(r.requests, 0);

    // A restart a minute later rebuilds what it has kept from the archive: the same page adds
    // nothing, and the window starts five minutes before the last covered point.
    clock.set(now + chrono::Duration::seconds(90));
    let mut c2 = EdgarCollector::open(cfg(), &a, clock.get()).unwrap();
    assert_eq!(c2.last_through(), Some(ns(now) - 120 * SEC_NS));
    let mut http = Script::default();
    http.replies.push_back(ok(fixture("company_tickers.json")));
    http.replies.push_back(ok(fixture("current-feed-p0.xml")));
    let r = c2.tick(&a, &mut http, &mut || clock.get(), &mut |_| {}, &mut |_| {});
    assert_eq!((r.filings, r.requests), (0, 2));
    assert_eq!(a.read_filings(None).unwrap().len(), 4);
}

/// A full feed page of unmapped 8-Ks, one a minute back from `newest`: the feed never reaches
/// the window's start in one page.
fn full_page(newest: DateTime<Utc>) -> Vec<u8> {
    let mut x = String::from("<feed>");
    for i in 0..FEED_PAGE {
        let t = (newest - chrono::Duration::minutes(i as i64)).with_timezone(&New_York);
        x.push_str(&format!(
            "<entry><title>8-K - NOBODY {i} (0006666666) (Filer)</title><link rel=\"alternate\" type=\"text/html\" href=\"https://www.sec.gov/x/{i}-index.htm\"/><summary type=\"html\">x</summary><updated>{}</updated><category scheme=\"https://www.sec.gov/\" label=\"form type\" term=\"8-K\"/><id>urn:tag:sec.gov,2008:accession-number=0006666666-26-{:06}</id></entry>",
            t.to_rfc3339(),
            i
        ));
    }
    x.push_str("</feed>");
    x.into_bytes()
}

#[test]
fn a_start_the_feed_cannot_cover_backfills_from_the_daily_index() {
    let (_d, a) = archive();
    let now = et((2026, 11, 20), 9, 20, 0);
    let clock = Cell::new(now);
    let mut cfg = cfg();
    cfg.backfill_max_pages = 1;
    let mut c = EdgarCollector::open(cfg, &a, now).unwrap();
    let mut http = Script::default();
    http.replies.push_back(ok(fixture("company_tickers.json")));
    http.replies
        .push_back(ok(full_page(et((2026, 11, 20), 9, 19, 0))));
    http.replies.push_back(ok(fixture("form.20261119.idx")));
    // The 424B2's index page is missing; the 8-K's says when it was accepted.
    http.replies.push_back(status(404));
    http.replies
        .push_back(ok(fixture("filing-index-0001111111-26-000041.htm")));
    let r = c.tick(&a, &mut http, &mut || clock.get(), &mut |_| {}, &mut |_| {});
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert!(!r.complete, "the one page did not reach the window's start");
    assert_eq!(r.requests, 5);
    assert_eq!(
        http.urls[2], "https://edgar.test/Archives/edgar/daily-index/2026/QTR4/form.20261119.idx",
        "only the window's dates before today (today's index is not published yet)"
    );
    let mut kept = a.read_filings(None).unwrap();
    kept.sort_by(|x, y| x.accession.cmp(&y.accession));
    let got: Vec<(&str, &str, &str)> = kept
        .iter()
        .map(|r| (r.form.as_str(), r.source.as_str(), r.dated_by.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("8-K", "daily_index", DATED_BY_ACCEPTANCE),
            ("424B2", "daily_index", DATED_BY_DAILY_INDEX),
            ("8-K/A", "daily_index", DATED_BY_DAILY_INDEX),
        ]
    );
    assert_eq!(kept[0].accepted_at_ns, ns(et((2026, 11, 19), 16, 31, 7)));
    assert_eq!(
        kept[1].accepted_at_ns,
        ns(et((2026, 11, 18), 17, 30, 0)),
        "no time known: the earliest its filing date allows"
    );
    assert!(kept.iter().all(|r| r.first_observed_at == ns(now)));
    // The coverage claim is only what the feed read: from its oldest entry.
    let cov = a.read_filings_coverage(None).unwrap();
    assert_eq!(cov[0].covered_from, ns(et((2026, 11, 20), 7, 40, 0)));

    // The next poll is a forward one: one page, no back-fill again.
    clock.set(now + chrono::Duration::seconds(61));
    let mut http = Script::default();
    http.replies.push_back(ok(fixture("current-feed-p0.xml")));
    let r = c.tick(&a, &mut http, &mut || clock.get(), &mut |_| {}, &mut |_| {});
    assert_eq!(r.requests, 1);
    assert!(r.complete);
}
