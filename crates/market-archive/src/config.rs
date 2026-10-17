use std::path::PathBuf;

/// Archive configuration. Environment: `ARCHIVE_ROOT` (default `./data/archive`),
/// `ARCHIVE_RETENTION_DAYS` (default 0 = keep forever), `ARCHIVE_MIN_FREE_MB` (default 1024).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveConfig {
    pub root: PathBuf,
    /// Day directories older than this many days (by trading date) are deleted by
    /// [`crate::Archive::prune`]. 0 disables pruning.
    pub retention_days: u32,
    /// Writes are refused while the volume has less than this many bytes available.
    pub min_free_bytes: u64,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        ArchiveConfig {
            root: PathBuf::from("./data/archive"),
            retention_days: 0,
            min_free_bytes: 1024 * 1024 * 1024,
        }
    }
}

impl ArchiveConfig {
    /// Reads the variables above through `get` (so tests need not touch the process env).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut cfg = ArchiveConfig::default();
        if let Some(r) = var("ARCHIVE_ROOT") {
            cfg.root = PathBuf::from(r);
        }
        if let Some(d) = var("ARCHIVE_RETENTION_DAYS") {
            cfg.retention_days = d
                .parse()
                .map_err(|_| format!("ARCHIVE_RETENTION_DAYS is not a whole number: {d}"))?;
        }
        if let Some(m) = var("ARCHIVE_MIN_FREE_MB") {
            let mb: u64 = m
                .parse()
                .map_err(|_| format!("ARCHIVE_MIN_FREE_MB is not a whole number: {m}"))?;
            cfg.min_free_bytes = mb.saturating_mul(1024 * 1024);
        }
        Ok(cfg)
    }
}
