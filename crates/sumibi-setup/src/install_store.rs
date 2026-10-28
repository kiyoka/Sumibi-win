//! Fixed-lock lifecycle around successive registration journals.
//!
//! The caller must first authenticate the directory, its ancestors, owner,
//! ACLs, and lack of reparse points. This module does not confer that trust.
//! No product registration command uses it yet.
use crate::install_journal::FileJournal;
use crate::install_transaction::{Error, Journal, Phase, Result};
use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const LOCK: &str = "registration.lock";
const CURRENT: &str = "current.bin";
const MAX_ARCHIVES: usize = 64;
const REPARSE_POINT: u32 = 0x400;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;

fn failure(error: impl std::fmt::Display) -> Error {
    Error(format!("registration store: {error}"))
}

fn plain(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(failure)?;
    if metadata.file_attributes() & REPARSE_POINT != 0
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err(failure("store item is not a plain file or directory"));
    }
    Ok(())
}

fn archive_name(transaction: &str) -> Result<String> {
    if transaction.len() != 32 || !transaction.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(failure("invalid archived transaction ID"));
    }
    Ok(format!("removed-{}.bin", transaction.to_ascii_lowercase()))
}

fn is_archive_name(name: &str) -> bool {
    name.strip_prefix("removed-")
        .and_then(|value| value.strip_suffix(".bin"))
        .is_some_and(|id| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
}

/// Holds one fixed exclusive lock throughout journal selection and mutation.
/// A privileged caller must validate the entire protected path and ACL before
/// acquisition; passing an arbitrary writable directory would bypass safety.
pub struct RegistrationStoreLock {
    root: PathBuf,
    _lock: File,
}

/// A journal cannot outlive the fixed store lock that selected it.
pub struct LockedJournal<'a> {
    _store: &'a RegistrationStoreLock,
    inner: FileJournal,
}

impl Journal for LockedJournal<'_> {
    fn latest(&self) -> Result<Option<crate::install_transaction::Checkpoint>> {
        self.inner.latest()
    }

    fn append(&mut self, checkpoint: &crate::install_transaction::Checkpoint) -> Result<()> {
        self.inner.append(checkpoint)
    }
}

impl RegistrationStoreLock {
    pub fn acquire_at_verified_directory(root: &Path) -> Result<Self> {
        plain(root, true)?;
        let path = root.join(LOCK);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).share_mode(0);
        options.custom_flags(OPEN_REPARSE_POINT);
        let lock = options.open(&path).map_err(failure)?;
        plain(&path, false)?;
        if lock.metadata().map_err(failure)?.len() != 0 {
            return Err(failure("lock file must be empty"));
        }
        let store = Self {
            root: root.to_path_buf(),
            _lock: lock,
        };
        store.validate_layout()?;
        Ok(store)
    }

    fn validate_layout(&self) -> Result<usize> {
        plain(&self.root, true)?;
        let mut archives = 0;
        for entry in fs::read_dir(&self.root).map_err(failure)? {
            let entry = entry.map_err(failure)?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| failure("non-UTF-8 store item"))?;
            if name == LOCK || name == CURRENT {
                plain(&entry.path(), false)?;
            } else if is_archive_name(name) {
                plain(&entry.path(), false)?;
                archives += 1;
            } else {
                return Err(failure("unexpected item in registration store"));
            }
        }
        if archives > MAX_ARCHIVES {
            return Err(failure("registration archive count limit reached"));
        }
        Ok(archives)
    }

    /// Open the single active transaction. Absence is distinct from an empty
    /// newly created journal; a corrupt existing file is never replaced.
    pub fn open_current(&self) -> Result<Option<LockedJournal<'_>>> {
        self.validate_layout()?;
        let path = self.root.join(CURRENT);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(LockedJournal {
            _store: self,
            inner: FileJournal::open(&path)?,
        }))
    }

    /// Begin another transaction only after the previous journal is fully
    /// removed. A crash before the first intent may leave an empty current file,
    /// which can be reused because no registration API was called yet.
    pub fn begin_new(&self) -> Result<LockedJournal<'_>> {
        let archive_count = self.validate_layout()?;
        if archive_count == MAX_ARCHIVES {
            return Err(failure("registration archive count limit reached"));
        }
        let current = self.root.join(CURRENT);
        if current.exists() {
            let journal = FileJournal::open(&current)?;
            if let Some(last) = journal.latest()? {
                if last.phase != Phase::Removed {
                    return Err(failure(
                        "active registration requires recovery or uninstall",
                    ));
                }
                let archived = self.root.join(archive_name(&last.spec.transaction)?);
                if archived.exists() {
                    return Err(failure("removed transaction archive already exists"));
                }
                drop(journal);
                fs::rename(&current, &archived).map_err(failure)?;
            } else {
                return Ok(LockedJournal {
                    _store: self,
                    inner: journal,
                });
            }
        }
        Ok(LockedJournal {
            _store: self,
            inner: FileJournal::create(&current)?,
        })
    }
}

#[cfg(test)]
#[path = "install_store_tests.rs"]
mod tests;
