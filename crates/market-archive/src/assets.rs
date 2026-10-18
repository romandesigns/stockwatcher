//! Asset metadata beside the days: `$ARCHIVE_ROOT/assets.json`.
//!
//! ```json
//! {"AAL": {"name": "American Airlines Group Inc.", "exchange": "NASDAQ"}, ...}
//! ```
//!
//! Written by `swimport alpaca-assets` from Alpaca's assets endpoint (the provider's own names and
//! listing exchanges, e.g. `NASDAQ`, `NYSE`, `ARCA`, `AMEX`, `BATS`, `OTC`), read once by the
//! server at startup so the ticker search can show a real company name and filter by exchange.
//!
//! It is **optional** and not market data: the server runs without it (names fall back to the
//! symbol, exchange chips match nothing) and never invents a name for a symbol it lacks. It sits at
//! the archive root, not under a day, because listings change slowly and one current snapshot is
//! what a search wants; [`crate::Archive::days`] only lists date-named directories, so neither
//! replay nor retention ever mistakes it for a day.

use crate::Archive;
use crate::error::ArchiveError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const ASSETS_FILE: &str = "assets.json";

/// One asset's display metadata, as the provider reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetMeta {
    pub name: String,
    pub exchange: String,
}

/// Symbol → metadata, sorted so the file diffs cleanly between runs.
pub type AssetMap = BTreeMap<String, AssetMeta>;

impl Archive {
    pub fn assets_path(&self) -> PathBuf {
        self.root().join(ASSETS_FILE)
    }

    /// The asset metadata, or `None` when the file does not exist (it is optional).
    pub fn read_assets(&self) -> Result<Option<AssetMap>, ArchiveError> {
        let path = self.assets_path();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(ArchiveError::io(&path, e)),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| ArchiveError::format(&path, e.to_string()))
    }

    /// Replaces the asset metadata atomically (temp file, fsync, rename), so the server starting
    /// mid-write reads either the old file or the new one, never half of one.
    pub fn write_assets(&self, assets: &AssetMap) -> Result<PathBuf, ArchiveError> {
        let root = self.root();
        std::fs::create_dir_all(root).map_err(|e| ArchiveError::io(root, e))?;
        let path = self.assets_path();
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(assets).expect("asset map serializes");
        std::fs::write(&tmp, json).map_err(|e| ArchiveError::io(&tmp, e))?;
        if let Ok(f) = std::fs::File::open(&tmp) {
            let _ = f.sync_all();
        }
        std::fs::rename(&tmp, &path).map_err(|e| ArchiveError::io(&path, e))?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArchiveConfig;

    fn archive(dir: &std::path::Path) -> Archive {
        let root = dir.join("archive").to_string_lossy().into_owned();
        Archive::new(
            ArchiveConfig::from_lookup(|k| match k {
                "ARCHIVE_ROOT" => Some(root.clone()),
                "ARCHIVE_MIN_FREE_MB" => Some("0".into()),
                _ => None,
            })
            .unwrap(),
        )
    }

    #[test]
    fn missing_is_none_and_a_write_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        assert!(a.read_assets().unwrap().is_none());
        let mut m = AssetMap::new();
        m.insert(
            "AAL".into(),
            AssetMeta {
                name: "American Airlines Group Inc.".into(),
                exchange: "NASDAQ".into(),
            },
        );
        a.write_assets(&m).unwrap();
        assert_eq!(a.read_assets().unwrap(), Some(m));
        assert!(!a.assets_path().with_extension("json.tmp").exists());
        assert!(a.days().unwrap().is_empty(), "never mistaken for a day");
    }

    #[test]
    fn invalid_json_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let a = archive(dir.path());
        std::fs::create_dir_all(a.root()).unwrap();
        std::fs::write(a.assets_path(), b"not json").unwrap();
        assert!(a.read_assets().is_err());
    }
}
