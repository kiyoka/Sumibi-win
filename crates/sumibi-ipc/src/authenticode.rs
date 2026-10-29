//! Offline embedded-signature inspection, not process or publisher trust.
//! This does not authorize launching a file: a product caller must separately
//! authenticate the publisher, protected path and exact running image.
use std::fs::OpenOptions;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, Prefix};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Security::WinTrust::*;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectionError {
    InvalidPath,
    OpenFailed,
    NotPlainFile,
    NotTrusted(i32),
    CleanupFailed(i32),
}

/// Inspect a local file's embedded Authenticode signature using cached trust
/// data only. This does not check online revocation or a particular signer.
/// Hold the file open without write/delete sharing through the complete call.
/// This synchronous operation belongs on a worker, not a TSF callback.
pub fn inspect_embedded(path: &Path) -> Result<(), InspectionError> {
    if !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        || !path.is_absolute()
    {
        return Err(InspectionError::InvalidPath);
    }
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.is_empty() || wide.contains(&0) {
        return Err(InspectionError::InvalidPath);
    }
    wide.push(0);
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|_| InspectionError::OpenFailed)?;
    let metadata = file.metadata().map_err(|_| InspectionError::OpenFailed)?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(InspectionError::NotPlainFile);
    }
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide.as_ptr()),
        hFile: HANDLE(file.as_raw_handle()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: all backing buffers and the readable file remain live throughout
    // both calls; UI is disabled and VERIFY is paired with CLOSE even on error.
    let status = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    let closed = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        )
    };
    if closed != 0 {
        return Err(InspectionError::CleanupFailed(closed));
    }
    if status != 0 {
        return Err(InspectionError::NotTrusted(status));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_relative_remote_and_unsigned_files() {
        assert_eq!(
            inspect_embedded(Path::new("relative.exe")),
            Err(InspectionError::InvalidPath)
        );
        assert_eq!(
            inspect_embedded(Path::new(r"\\server\share\file.exe")),
            Err(InspectionError::InvalidPath)
        );
        let executable = std::env::current_exe().unwrap();
        assert!(matches!(
            inspect_embedded(&executable),
            Err(InspectionError::NotTrusted(_))
        ));
    }
    #[test]
    fn inspects_an_embedded_signed_system_tool_when_present() {
        let path =
            Path::new(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
        if path.is_file() {
            assert_eq!(inspect_embedded(path), Ok(()));
        }
    }
}
