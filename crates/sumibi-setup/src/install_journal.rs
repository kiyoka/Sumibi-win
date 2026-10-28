//! Bounded append-only write-ahead journal. Checksums detect corruption, not
//! malicious edits: a machine installer must place this under protected ACLs.
//!
//! An incomplete final frame can be discarded: no registration API may be
//! called until its preceding intent frame has been completely synced. A bad
//! complete frame or invalid history fails closed without truncating anything.
#[cfg(windows)]
use crate::install_transaction::Journal;
use crate::install_transaction::{Checkpoint, Error, Result};
#[cfg(windows)]
use std::fs::{File, OpenOptions};
#[cfg(windows)]
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(windows)]
use std::path::Path;

const MAGIC: &[u8; 8] = b"SMBIJNL1";
const HEADER: usize = 16;
const MAX_PAYLOAD: usize = 40 * 1024;
const MAX_RECORDS: usize = 16;
const MAX_BYTES: usize = MAX_RECORDS * (HEADER + MAX_PAYLOAD);

#[cfg(windows)]
fn io(error: std::io::Error) -> Error {
    Error(format!("registration journal I/O: {error}"))
}

// IEEE CRC-32; not an authentication tag.
fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn frame(state: &Checkpoint) -> Result<Vec<u8>> {
    state.validate()?;
    let payload = serde_json::to_vec(state).map_err(|e| Error(e.to_string()))?;
    if payload.len() > MAX_PAYLOAD {
        return Err(Error("registration journal payload is too large".into()));
    }
    let mut bytes = Vec::with_capacity(HEADER + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&checksum(&payload).to_le_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

struct History {
    latest: Option<Checkpoint>,
    records: usize,
    valid_bytes: usize,
}
fn decode(bytes: &[u8]) -> Result<History> {
    if bytes.len() > MAX_BYTES {
        return Err(Error("registration journal exceeds size limit".into()));
    }
    let mut history = History {
        latest: None,
        records: 0,
        valid_bytes: 0,
    };
    while history.valid_bytes < bytes.len() {
        let tail = &bytes[history.valid_bytes..];
        if tail.len() < HEADER {
            // A short header must still be a prefix of the format marker.
            let prefix = tail.len().min(MAGIC.len());
            if tail[..prefix] != MAGIC[..prefix] {
                return Err(Error("invalid registration journal tail".into()));
            }
            break;
        }
        if &tail[..8] != MAGIC {
            return Err(Error("invalid registration journal magic".into()));
        }
        let length = u32::from_le_bytes(tail[8..12].try_into().unwrap()) as usize;
        let expected = u32::from_le_bytes(tail[12..16].try_into().unwrap());
        if length == 0 || length > MAX_PAYLOAD || history.records == MAX_RECORDS {
            return Err(Error(
                "invalid registration journal frame length/count".into(),
            ));
        }
        if tail.len() < HEADER + length {
            break;
        }
        let payload = &tail[HEADER..HEADER + length];
        if checksum(payload) != expected {
            return Err(Error("registration journal checksum mismatch".into()));
        }
        let state: Checkpoint =
            serde_json::from_slice(payload).map_err(|e| Error(e.to_string()))?;
        state.follows(history.latest.as_ref())?;
        history.latest = Some(state);
        history.records += 1;
        history.valid_bytes += HEADER + length;
    }
    Ok(history)
}

/// Exclusive handle on Windows, retained across the complete transaction.
/// The caller must validate the parent directories and ACL before opening a
/// privileged journal. No registry paths, scripts or DLLs are executed from it.
#[cfg(windows)]
pub struct FileJournal {
    file: File,
    latest: Option<Checkpoint>,
    records: usize,
    poisoned: bool,
    discarded_tail: bool,
}

#[cfg(windows)]
impl FileJournal {
    pub fn create(path: &Path) -> Result<Self> {
        Self::open_impl(path, true)
    }

    pub fn open(path: &Path) -> Result<Self> {
        Self::open_impl(path, false)
    }

    fn open_impl(path: &Path, create: bool) -> Result<Self> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        let mut options = OpenOptions::new();
        options.read(true).write(true).share_mode(0);
        // Open a reparse point itself, then reject it rather than following it.
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
        if create {
            options.create_new(true);
        }
        let mut file = options.open(path).map_err(io)?;
        let metadata = file.metadata().map_err(io)?;
        if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
            return Err(Error("registration journal must be a plain file".into()));
        }
        if metadata.len() > MAX_BYTES as u64 {
            return Err(Error("registration journal exceeds size limit".into()));
        }
        let mut bytes = Vec::new();
        // Take also bounds reads if an externally held handle appends data.
        (&mut file)
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        let history = decode(&bytes)?;
        let discarded_tail = history.valid_bytes != bytes.len();
        if discarded_tail {
            file.set_len(history.valid_bytes as u64).map_err(io)?;
            file.sync_all().map_err(io)?;
        }
        file.seek(SeekFrom::End(0)).map_err(io)?;
        Ok(Self {
            file,
            latest: history.latest,
            records: history.records,
            poisoned: false,
            discarded_tail,
        })
    }

    pub fn discarded_incomplete_tail(&self) -> bool {
        self.discarded_tail
    }
}

#[cfg(windows)]
impl Journal for FileJournal {
    fn latest(&self) -> Result<Option<Checkpoint>> {
        if self.poisoned {
            return Err(Error(
                "uncertain journal write; close and reopen before recovery".into(),
            ));
        }
        Ok(self.latest.clone())
    }

    fn append(&mut self, state: &Checkpoint) -> Result<()> {
        self.latest()?;
        state.follows(self.latest.as_ref())?;
        if self.records == MAX_RECORDS {
            return Err(Error("registration journal record limit reached".into()));
        }
        let bytes = frame(state)?;
        // Do not trust the in-memory checkpoint after ANY I/O error. A complete
        // frame may have reached disk even if sync_all reported failure.
        self.poisoned = true;
        self.file.write_all(&bytes).map_err(io)?;
        self.file.sync_all().map_err(io)?;
        self.latest = Some(state.clone());
        self.records += 1;
        self.poisoned = false;
        Ok(())
    }
}

#[cfg(test)]
#[path = "install_journal_tests.rs"]
mod tests;
