//! Files the production build (fdb0871) wrote still load in this one.
//!
//! The only persisted struct that gained fields since then is the warmup request (`attempts`,
//! `last_error`, both `#[serde(default)]`); evidence records, manifests and recordings are
//! unchanged, and admission journals are new files a journal-less day simply lacks
//! (`crates/server/src/history.rs` tests the restore of such a day). The literal below is
//! byte-for-byte what fdb0871's `request_warmup` serializes.

use chrono::NaiveDate;
use market_archive::{Archive, ArchiveConfig};

#[test]
fn a_warmup_request_written_by_the_production_build_still_loads() {
    let tmp = tempfile::tempdir().expect("tmp");
    let a = Archive::new(ArchiveConfig {
        root: tmp.path().to_path_buf(),
        retention_days: 0,
        min_free_bytes: 0,
    });
    let d = NaiveDate::from_ymd_opt(2026, 10, 16).expect("valid");
    let dir = tmp.path().join("warmup").join("2026-10-16");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(
        dir.join("ABCD.json"),
        br#"{"symbol":"ABCD","session_date":"2026-10-16","requested_at":"2026-10-16T14:31:02.123456789Z","reason":"server:band-entry","outcome":null,"done_at":null}"#,
    )
    .expect("write");
    let got = a.pending_warmups(d, 10).expect("an fdb0871 request reads");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].symbol, "ABCD");
    assert_eq!((got[0].attempts, got[0].last_error.as_deref()), (0, None));
}
