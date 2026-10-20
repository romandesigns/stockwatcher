//! Corporate-action announcements against the research probes' recorded Alpaca responses
//! (`tests/fixtures/alpaca-corporate-actions-*.json`, recorded 2026-10-19). No network, no keys.

use chrono::{DateTime, NaiveDate};
use market_archive::baseline::SplitRatio;
use market_archive::baseline::{Announced, announced};
use market_archive::{Archive, ArchiveConfig, SplitKind};
use market_providers::import::alpaca::AlpacaCredentials;
use market_providers::import::corporate::fetch_announcements;
use market_providers::import::http::{Http, HttpResponse};

const BTOG: &str = include_str!("fixtures/alpaca-corporate-actions-BTOG.json");
const SGRX: &str = include_str!("fixtures/alpaca-corporate-actions-SGRX.json");

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// Chunk 1 (BTOG + 99 fillers) answers with BTOG's page, then SGRX's body as page 2; chunk 2
/// (SGRX alone) fails with 403.
struct Recorded {
    urls: Vec<String>,
}

impl Http for Recorded {
    fn get(&mut self, url: &str, _h: &[(&str, &str)]) -> Result<HttpResponse, String> {
        self.urls.push(url.to_string());
        assert!(url.contains("/v1/corporate-actions?"), "{url}");
        let body = if url.contains("SGRX") {
            return Ok(HttpResponse {
                status: 403,
                body: b"{\"message\":\"forbidden\"}".to_vec(),
                retry_after: None,
            });
        } else if url.contains("page_token=p2") {
            SGRX.to_string()
        } else {
            // BTOG's recorded page, with a next page added.
            BTOG.replace("\"next_page_token\":null", "\"next_page_token\":\"p2\"")
        };
        Ok(HttpResponse {
            status: 200,
            body: body.into_bytes(),
            retry_after: None,
        })
    }
}

#[test]
fn recorded_responses_parse_and_a_failed_chunk_is_not_covered() {
    let mut symbols = vec!["BTOG".to_string(), "SGRX".to_string()];
    symbols.extend((0..99).map(|i| format!("S{i:03}")));
    let mut http = Recorded { urls: Vec::new() };
    let creds = AlpacaCredentials {
        key_id: "K".into(),
        secret: "S".into(),
    };
    let r = fetch_announcements(
        &mut http,
        "https://data.example",
        &creds,
        &symbols,
        d("2026-08-20"),
        d("2026-08-21"),
        60_000,
        "test",
        &mut |_| {},
    );
    assert!(
        http.urls[0].contains(
            "types=forward_split%2Creverse_split%2Cunit_split%2Cname_change&start=2026-08-20&end=2026-08-21&limit=1000"
        ),
        "{}",
        http.urls[0]
    );
    // Sorted: BTOG, S000..S098 (chunk 1, 100 symbols), SGRX (chunk 2, fails).
    assert_eq!(r.symbols_requested.len(), 101);
    assert_eq!(r.symbols_covered.len(), 100);
    assert!(r.covers("BTOG") && r.covers("S098") && !r.covers("SGRX"));
    assert!(!r.success);
    assert_eq!(r.errors.len(), 1);
    assert_eq!(
        r.requests, 2,
        "two pages for chunk 1; the 403 is not a completed request"
    );
    // BTOG's three reverse splits (30:1, 60:1, 5:1) and both name changes, deduplicated by id.
    assert_eq!(r.splits.len(), 3);
    assert!(
        r.splits
            .iter()
            .all(|s| s.kind == SplitKind::Reverse && s.symbol == "BTOG")
    );
    let aug = r
        .splits
        .iter()
        .find(|s| s.ex_date == d("2026-08-21"))
        .unwrap()
        .clone();
    assert_eq!((aug.old_rate, aug.new_rate), (5.0, 1.0));
    assert_eq!(r.name_changes.len(), 2);

    // Stored append-only and read back.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    let archive = Archive::new(
        ArchiveConfig::from_lookup(|k| match k {
            "ARCHIVE_ROOT" => Some(root.clone()),
            "ARCHIVE_MIN_FREE_MB" => Some("0".into()),
            _ => None,
        })
        .unwrap(),
    );
    let mut r = r;
    // As if fetched at 07:00 ET on the ex date (the fetch itself stamps the real clock).
    r.retrieved_at = DateTime::parse_from_rfc3339("2026-08-21T11:00:00Z")
        .unwrap()
        .to_utc();
    archive.write_action_retrieval(&r).unwrap();
    assert!(archive.write_action_retrieval(&r).is_err(), "append-only");
    let back = archive.read_action_retrievals(d("2026-08-21")).unwrap();
    assert_eq!(back, [r.clone()]);

    // The loader's reading of it: BTOG's 5:1 reverse split takes effect on 08-21.
    match announced(&back, "BTOG", d("2026-08-20"), d("2026-08-21")) {
        Announced::Split { ratio, action, .. } => {
            assert_eq!(
                ratio,
                SplitRatio {
                    old_rate: 5,
                    new_rate: 1
                }
            );
            assert_eq!(action, aug.id);
        }
        other => panic!("{other:?}"),
    }
    // SGRX's chunk failed: unknown, never "no split".
    assert_eq!(
        announced(&back, "SGRX", d("2026-08-20"), d("2026-08-21")),
        Announced::Unknown("no_announcement_coverage")
    );
    // On the rename date (BTOG → SGRX, 2026-09-04) nothing is vouched for.
    let mut renamed = r.clone();
    renamed.start = d("2026-09-03");
    renamed.end = d("2026-09-04");
    renamed.retrieved_at = DateTime::parse_from_rfc3339("2026-09-04T11:00:00Z")
        .unwrap()
        .to_utc();
    assert_eq!(
        announced(&[renamed], "BTOG", d("2026-09-03"), d("2026-09-04")),
        Announced::Unknown("symbol_changed")
    );
}
