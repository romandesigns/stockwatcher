//! Free-space checks for the archive volume.

use crate::error::ArchiveError;
use std::path::Path;

/// Bytes available to this process on the volume holding `path` (the nearest existing ancestor
/// is used when `path` does not exist yet).
pub fn available_bytes(path: &Path) -> Result<u64, ArchiveError> {
    let mut probe = path;
    while !probe.exists() {
        match probe.parent() {
            Some(p) if !p.as_os_str().is_empty() => probe = p,
            _ => {
                probe = Path::new(".");
                break;
            }
        }
    }
    fs4::available_space(probe).map_err(|e| ArchiveError::io(probe, e))
}

/// `Err(InsufficientSpace)` when the volume is below `floor` bytes.
pub fn ensure_floor(path: &Path, floor: u64) -> Result<u64, ArchiveError> {
    if floor == 0 {
        return Ok(u64::MAX);
    }
    let free = available_bytes(path)?;
    if free < floor {
        return Err(ArchiveError::InsufficientSpace {
            path: path.to_path_buf(),
            free,
            floor,
        });
    }
    Ok(free)
}
