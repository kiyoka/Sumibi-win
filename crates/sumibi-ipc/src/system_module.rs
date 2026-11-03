//! Leased system DLL inspection and export catalog binding.
//! Bounded to GetSystemDirectoryW, read-only without write/delete sharing,
//! and no symbolic links or reparse points.

use crate::dll_exports::{inspect_file, DllExports};
use crate::export_resolver::{normalize_module_name, ExportCatalog, ResolveError, ResolvedExport};
use crate::host_image::ImportSymbol;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

pub const MAX_SYSTEM_MODULES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemModuleError {
    InvalidName,
    ApiSetUnresolved,
    OpenFailed,
    NotPlainFile,
    Image(crate::host_image::ImageError),
    DuplicateModule,
    Capacity,
    Resolve(ResolveError),
}

impl From<ResolveError> for SystemModuleError {
    fn from(error: ResolveError) -> Self {
        match error {
            ResolveError::InvalidModule => Self::InvalidName,
            ResolveError::ApiSetUnresolved => Self::ApiSetUnresolved,
            other => Self::Resolve(other),
        }
    }
}

/// Keeps a system DLL open with shared-read only (no write/delete sharing).
/// This prevents tampering while exports are being evaluated.
#[must_use = "keep the system module lease alive while exports are used"]
pub struct SystemModuleLease {
    name: String,
    file: File,
    exports: DllExports,
}

impl std::fmt::Debug for SystemModuleLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemModuleLease")
            .field("name", &self.name)
            .finish()
    }
}

impl SystemModuleLease {
    /// Open a validated system module directly under System32.
    /// Never resolves paths from untrusted forwarders or general directories.
    pub fn open(name: &str) -> Result<Self, SystemModuleError> {
        let normalized = normalize_module_name(name)?;
        let system_dir = get_system_directory()?;
        let path = system_dir.join(&normalized);
        let mut file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&path)
            .map_err(|_| SystemModuleError::OpenFailed)?;
        let metadata = file.metadata().map_err(|_| SystemModuleError::OpenFailed)?;
        if !metadata.is_file() || (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0)
        {
            return Err(SystemModuleError::NotPlainFile);
        }
        let exports = inspect_file(&mut file).map_err(SystemModuleError::Image)?;
        Ok(Self {
            name: normalized,
            file,
            exports,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn exports(&self) -> &DllExports {
        &self.exports
    }

    pub fn file(&self) -> &File {
        &self.file
    }
}

fn get_system_directory() -> Result<PathBuf, SystemModuleError> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: writable bounded buffer passed to Windows API.
    let count = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if count == 0 || count >= buffer.len() {
        return Err(SystemModuleError::OpenFailed);
    }
    Ok(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..count],
    )))
}

/// Catalog owning the open file leases of all registered system modules.
#[derive(Default)]
pub struct SystemModuleCatalog {
    leases: BTreeMap<String, SystemModuleLease>,
}

impl SystemModuleCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, lease: SystemModuleLease) -> Result<(), SystemModuleError> {
        if self.leases.contains_key(&lease.name) {
            return Err(SystemModuleError::DuplicateModule);
        }
        if self.leases.len() >= MAX_SYSTEM_MODULES {
            return Err(SystemModuleError::Capacity);
        }
        self.leases.insert(lease.name.clone(), lease);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.leases.len()
    }

    pub fn is_empty(&self) -> bool {
        self.leases.is_empty()
    }

    pub fn contains(&self, module: &str) -> bool {
        if let Ok(name) = normalize_module_name(module) {
            self.leases.contains_key(&name)
        } else {
            false
        }
    }

    pub fn resolve(
        &self,
        module: &str,
        symbol: &ImportSymbol,
    ) -> Result<ResolvedExport, ResolveError> {
        let mut catalog = ExportCatalog::new();
        for (name, lease) in &self.leases {
            catalog.insert(name, &lease.exports)?;
        }
        catalog.resolve(module, symbol)
    }

    pub fn leases(&self) -> impl Iterator<Item = &SystemModuleLease> {
        self.leases.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_module_lease_rejects_invalid_names_and_api_sets() {
        assert_eq!(
            SystemModuleLease::open("").unwrap_err(),
            SystemModuleError::InvalidName
        );
        assert_eq!(
            SystemModuleLease::open("../kernel32.dll").unwrap_err(),
            SystemModuleError::InvalidName
        );
        assert_eq!(
            SystemModuleLease::open("api-ms-win-core-synch-l1-2-0.dll").unwrap_err(),
            SystemModuleError::ApiSetUnresolved
        );
        assert_eq!(
            SystemModuleLease::open("ext-ms-win-gdi-draw-l1-1-0.dll").unwrap_err(),
            SystemModuleError::ApiSetUnresolved
        );
    }

    #[test]
    fn system_module_catalog_rejects_duplicates() {
        let mut catalog = SystemModuleCatalog::new();
        let kernel32 = SystemModuleLease::open("kernel32.dll").unwrap();
        assert_eq!(kernel32.name(), "kernel32.dll");
        assert!(catalog.insert(kernel32).is_ok());
        assert_eq!(catalog.len(), 1);
        assert!(catalog.contains("kernel32"));
        assert!(catalog.contains("KERNEL32.DLL"));

        let kernel32_dup = SystemModuleLease::open("KERNEL32").unwrap();
        assert_eq!(
            catalog.insert(kernel32_dup),
            Err(SystemModuleError::DuplicateModule)
        );
    }
}
