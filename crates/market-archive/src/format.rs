//! The `.swa` file header and record framing (see the crate docs for the layout).

use serde::{Deserialize, Serialize};

pub const MAGIC: &[u8; 8] = b"SWARCHV\0";
pub const FORMAT_VERSION: u16 = 1;
pub const MAX_HEADER_BYTES: u32 = 64 * 1024;
pub const MAX_RECORD_BYTES: u32 = 1024 * 1024;
/// File extension of a finalized symbol file.
pub const EXT: &str = "swa";
pub const PARTIAL_SUFFIX: &str = ".partial";
pub const MANIFEST: &str = "manifest.json";

/// The JSON header stored after the fixed prefix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHeader {
    pub symbol: String,
    /// Trading date, `YYYY-MM-DD`.
    pub day: String,
    /// Tool and version that wrote the file, e.g. `"market-archive 0.1.0"`.
    pub created_by: String,
}

/// The fixed 16-byte prefix followed by the JSON header.
pub fn encode_header(header: &FileHeader) -> Vec<u8> {
    let json = serde_json::to_vec(header).expect("header serializes");
    let mut out = Vec::with_capacity(16 + json.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&market_events::CONTRACT_VERSION.to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    out
}
