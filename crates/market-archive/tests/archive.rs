use chrono::NaiveDate;
use market_archive::fixture::{DEFAULT_SYMBOLS, fixture_symbol, write_fixture_day};
use market_archive::{Archive, ArchiveConfig, ArchiveError, FsyncPolicy};
use market_events::{MarketEvent, Source};

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
}

fn archive(dir: &std::path::Path) -> Archive {
    Archive::new(ArchiveConfig {
        root: dir.join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    })
}

#[test]
fn round_trip_preserves_every_event_in_arrival_order() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path());
    let events = fixture_symbol(day(), "SMPL", 7, 1.0).unwrap();
    assert!(events.len() > 20_000);
    let mut w = a.writer(day(), "SMPL", FsyncPolicy::Never).unwrap();
    for e in &events {
        w.append(e).unwrap();
    }
    let entry = w.finalize().unwrap();
    assert_eq!(entry.events, events.len() as u64);
    assert_eq!(entry.sources, vec![Source::Fixture]);
    assert!(entry.corrections == 1 && entry.cancels == 1 && entry.statuses == 2);
    a.commit(day(), entry.clone()).unwrap();

    let back: Vec<MarketEvent> = a
        .reader(day(), "SMPL")
        .unwrap()
        .map(|r| r.unwrap().event)
        .collect();
    assert_eq!(back, events);

    let m = a.manifest(day()).unwrap().unwrap();
    assert!(m.fixture);
    assert_eq!(m.symbols["SMPL"], entry);
    let (sha, bytes) = market_archive_sha(&a.day_dir(day()).join("SMPL.swa"));
    assert_eq!((sha, bytes), (entry.sha256.clone(), entry.bytes));
    assert!(a.verify(day()).unwrap().ok());
    // Compression actually happens: well under the raw JSON size.
    let raw: usize = events
        .iter()
        .map(|e| serde_json::to_vec(e).unwrap().len())
        .sum();
    assert!((entry.bytes as usize) < raw / 5, "{} vs {raw}", entry.bytes);
}

fn market_archive_sha(path: &std::path::Path) -> (String, u64) {
    use sha2_shim::sha256_hex;
    let bytes = std::fs::read(path).unwrap();
    (sha256_hex(&bytes), bytes.len() as u64)
}

/// An independent SHA-256 (the archive's own implementation must not be checked against itself).
mod sha2_shim {
    pub fn sha256_hex(data: &[u8]) -> String {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        let mut msg = data.to_vec();
        let bitlen = (data.len() as u64) * 8;
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bitlen.to_be_bytes());
        for chunk in msg.chunks(64) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let mut v = h;
            for i in 0..64 {
                let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
                let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
                let t1 = v[7]
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
                let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
                let t2 = s0.wrapping_add(maj);
                v = [
                    t1.wrapping_add(t2),
                    v[0],
                    v[1],
                    v[2],
                    v[3].wrapping_add(t1),
                    v[4],
                    v[5],
                    v[6],
                ];
            }
            for i in 0..8 {
                h[i] = h[i].wrapping_add(v[i]);
            }
        }
        h.iter().map(|x| format!("{x:08x}")).collect()
    }

    #[test]
    fn known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

#[test]
fn verify_detects_a_flipped_byte_and_a_truncation() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path());
    write_fixture_day(&a, day(), &["SMPL".to_string()], 7).unwrap();
    assert!(a.verify(day()).unwrap().ok());
    let path = a.day_dir(day()).join("SMPL.swa");
    let mut bytes = std::fs::read(&path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x40;
    std::fs::write(&path, &bytes).unwrap();
    let rep = a.verify(day()).unwrap();
    assert!(!rep.ok());
    assert!(!rep.symbols[0].sha256_ok);

    bytes[mid] ^= 0x40;
    bytes.truncate(bytes.len() - 100);
    std::fs::write(&path, &bytes).unwrap();
    let rep = a.verify(day()).unwrap();
    assert!(!rep.symbols[0].records_ok, "{rep:?}");
}

#[test]
fn fixture_days_are_byte_identical_across_runs() {
    let t1 = tempfile::tempdir().unwrap();
    let t2 = tempfile::tempdir().unwrap();
    let syms: Vec<String> = DEFAULT_SYMBOLS.iter().map(|s| s.to_string()).collect();
    let m1 = write_fixture_day(&archive(t1.path()), day(), &syms, 7).unwrap();
    let m2 = write_fixture_day(&archive(t2.path()), day(), &syms, 7).unwrap();
    assert_eq!(m1, m2);
    assert_eq!(m1.symbols.len(), 4);
    // Prices span the universe edges: FXLO under $1, FXHI above $25.
    let last_price = |sym: &str| {
        let a = archive(t1.path());
        let mut p = None;
        for e in a.reader(day(), sym).unwrap() {
            if let MarketEvent::Trade(t) = e.unwrap().event {
                p = Some(t.price.to_f64());
            }
        }
        p.unwrap()
    };
    assert!(last_price("FXLO") < 1.0);
    assert!(last_price("FXHI") > 25.0);
}

#[test]
fn free_space_floor_refuses_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let a = Archive::new(ArchiveConfig {
        root: tmp.path().join("archive"),
        retention_days: 0,
        min_free_bytes: u64::MAX,
    });
    match a.writer(day(), "SMPL", FsyncPolicy::Never) {
        Err(ArchiveError::InsufficientSpace { floor, .. }) => assert_eq!(floor, u64::MAX),
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("writer opened below the floor"),
    }
    assert!(a.days().unwrap().is_empty());
}

#[test]
fn retention_prunes_old_days_only() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = ArchiveConfig {
        root: tmp.path().join("archive"),
        retention_days: 0,
        min_free_bytes: 0,
    };
    let a = Archive::new(cfg.clone());
    let d1 = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    for d in [d1, day()] {
        let w = a.writer(d, "SMPL", FsyncPolicy::Never).unwrap();
        a.commit(d, w.finalize().unwrap()).unwrap();
    }
    let today = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
    assert!(a.prune(today).unwrap().is_empty(), "retention 0 keeps all");
    cfg.retention_days = 7;
    let a = Archive::new(cfg);
    assert_eq!(a.prune(today).unwrap(), vec![d1]);
    assert_eq!(a.days().unwrap(), vec![day()]);
}

#[test]
fn rejects_bad_symbols_and_foreign_files() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path());
    assert!(matches!(
        a.writer(day(), "../X", FsyncPolicy::Never),
        Err(ArchiveError::InvalidSymbol(_))
    ));
    let p = tmp.path().join("junk.swa");
    std::fs::write(&p, b"not an archive file at all").unwrap();
    assert!(matches!(
        market_archive::SymbolReader::open(&p),
        Err(ArchiveError::Format { .. })
    ));
}

/// A failed warmup attempt is not the symbol's whole day: the request stays open for the next
/// run with its attempt counted, and is only closed once its attempts are spent.
///
/// Closing on the first error (the audit's point about the maintainer `.done`-ing a temporary
/// failure) meant one provider hiccup cost an intraday band entrant its baseline until the next
/// session date.
#[test]
fn a_failed_warmup_stays_open_until_its_attempts_are_spent() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path());
    let d = day();
    let at = chrono::Utc::now();
    let req = market_archive::WarmupRequest {
        symbol: "NEWW".into(),
        session_date: d,
        requested_at: at,
        reason: "server:no_daily_baseline".into(),
        outcome: None,
        done_at: None,
        attempts: 0,
        last_error: None,
    };
    assert!(a.request_warmup(&req).unwrap());

    // Two failures with three attempts allowed: still open, attempts and the error recorded.
    for expected in 1..=2u32 {
        let open = a.pending_warmups(d, 10).unwrap();
        assert_eq!(open.len(), 1, "still open before attempt {expected}");
        let closed = a
            .defer_warmup(open[0].clone(), format!("boom {expected}"), at, 3)
            .unwrap();
        assert!(!closed, "attempt {expected} of 3 is not the last");
        let again = a.pending_warmups(d, 10).unwrap();
        assert_eq!(again[0].attempts, expected);
        assert_eq!(
            again[0].last_error.as_deref(),
            Some(format!("boom {expected}").as_str())
        );
        assert!(again[0].outcome.is_none() && again[0].done_at.is_none());
    }

    // The third spends the allowance: closed with the error, and never retried.
    let open = a.pending_warmups(d, 10).unwrap();
    assert!(
        a.defer_warmup(open[0].clone(), "boom 3".into(), at, 3)
            .unwrap()
    );
    assert!(a.pending_warmups(d, 10).unwrap().is_empty());
    assert!(
        !a.request_warmup(&req).unwrap(),
        "a closed request is not asked for again that day"
    );
}

/// A success closes the request outright, with no attempt bookkeeping left behind.
#[test]
fn a_successful_warmup_closes_on_the_first_run() {
    let tmp = tempfile::tempdir().unwrap();
    let a = archive(tmp.path());
    let d = day();
    let at = chrono::Utc::now();
    a.request_warmup(&market_archive::WarmupRequest {
        symbol: "NEWW".into(),
        session_date: d,
        requested_at: at,
        reason: "server:no_daily_baseline".into(),
        outcome: None,
        done_at: None,
        attempts: 0,
        last_error: None,
    })
    .unwrap();
    let open = a.pending_warmups(d, 10).unwrap();
    a.finish_warmup(open[0].clone(), "ok".into(), at).unwrap();
    assert!(a.pending_warmups(d, 10).unwrap().is_empty());
}
