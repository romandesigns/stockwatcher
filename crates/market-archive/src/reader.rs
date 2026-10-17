//! Streaming reader for one `.swa` file. Bounded memory: one decoder, one record buffer.

use crate::error::ArchiveError;
use crate::format::{FORMAT_VERSION, FileHeader, MAGIC, MAX_HEADER_BYTES, MAX_RECORD_BYTES};
use market_events::{CONTRACT_VERSION, VersionedEvent};
use std::fs::File;
use std::io::{BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};

type Decoder = zstd::stream::read::Decoder<'static, BufReader<BufReader<File>>>;

pub struct SymbolReader {
    path: PathBuf,
    header: FileHeader,
    decoder: Decoder,
    buf: Vec<u8>,
    records: u64,
    done: bool,
}

impl SymbolReader {
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        let file = File::open(path).map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                ArchiveError::NotFound(path.to_path_buf())
            } else {
                ArchiveError::io(path, e)
            }
        })?;
        let mut r = BufReader::with_capacity(256 * 1024, file);
        let mut prefix = [0u8; 16];
        r.read_exact(&mut prefix)
            .map_err(|_| ArchiveError::format(path, "file shorter than the 16-byte header"))?;
        if &prefix[0..8] != MAGIC {
            return Err(ArchiveError::format(path, "bad magic (not a .swa file)"));
        }
        let fv = u16::from_le_bytes([prefix[8], prefix[9]]);
        if fv != FORMAT_VERSION {
            return Err(ArchiveError::format(
                path,
                format!("unsupported format version {fv} (expected {FORMAT_VERSION})"),
            ));
        }
        let cv = u16::from_le_bytes([prefix[10], prefix[11]]);
        if cv != CONTRACT_VERSION {
            return Err(ArchiveError::format(
                path,
                format!("unsupported event contract version {cv} (expected {CONTRACT_VERSION})"),
            ));
        }
        let hlen = u32::from_le_bytes([prefix[12], prefix[13], prefix[14], prefix[15]]);
        if hlen > MAX_HEADER_BYTES {
            return Err(ArchiveError::format(path, "header too large"));
        }
        let mut hbuf = vec![0u8; hlen as usize];
        r.read_exact(&mut hbuf)
            .map_err(|_| ArchiveError::format(path, "truncated header"))?;
        let header: FileHeader = serde_json::from_slice(&hbuf)
            .map_err(|e| ArchiveError::format(path, format!("header: {e}")))?;
        let decoder = zstd::stream::read::Decoder::new(r).map_err(|e| ArchiveError::io(path, e))?;
        Ok(SymbolReader {
            path: path.to_path_buf(),
            header,
            decoder,
            buf: Vec::new(),
            records: 0,
            done: false,
        })
    }

    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    pub fn records_read(&self) -> u64 {
        self.records
    }

    fn read_record(&mut self) -> Result<Option<VersionedEvent>, ArchiveError> {
        let mut len = [0u8; 4];
        // A clean end is EOF exactly at a record boundary.
        let mut got = 0;
        while got < 4 {
            match self.decoder.read(&mut len[got..]) {
                Ok(0) if got == 0 => return Ok(None),
                Ok(0) => {
                    return Err(ArchiveError::format(&self.path, "truncated record length"));
                }
                Ok(n) => got += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(ArchiveError::format(&self.path, format!("zstd: {e}"))),
            }
        }
        let n = u32::from_le_bytes(len);
        if n == 0 || n > MAX_RECORD_BYTES {
            return Err(ArchiveError::format(
                &self.path,
                format!("record {} has invalid length {n}", self.records),
            ));
        }
        self.buf.resize(n as usize, 0);
        self.decoder.read_exact(&mut self.buf).map_err(|e| {
            ArchiveError::format(&self.path, format!("record {}: {e}", self.records))
        })?;
        let ev: VersionedEvent = serde_json::from_slice(&self.buf).map_err(|e| {
            ArchiveError::format(&self.path, format!("record {}: {e}", self.records))
        })?;
        self.records += 1;
        Ok(Some(ev))
    }
}

impl Iterator for SymbolReader {
    type Item = Result<VersionedEvent, ArchiveError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.read_record() {
            Ok(Some(e)) => Some(Ok(e)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}
