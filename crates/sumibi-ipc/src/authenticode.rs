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
use windows::Win32::Security::Cryptography::CryptHashCertificate2;
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
    SignerUnavailable,
    UnexpectedSigner,
}

/// SHA-256 of the signer certificate's DER bytes, not its subject name or a
/// proof that an already-running process uses the inspected image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignerFingerprint(pub [u8; 32]);

/// Inspect a local file's embedded Authenticode signature using cached trust
/// data only. This does not check online revocation or a particular signer.
/// Hold the file open without write/delete sharing through the complete call.
/// This synchronous operation belongs on a worker, not a TSF callback.
pub fn inspect_embedded(path: &Path) -> Result<(), InspectionError> {
    inspect_signer(path).map(|_| ())
}

/// Inspect the validated embedded signer. An allowlist based on this value
/// must come from an authenticated, protected installation policy.
pub fn inspect_signer(path: &Path) -> Result<SignerFingerprint, InspectionError> {
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
    let result = if status == 0 {
        // SAFETY: the provider state and its signer certificate remain live
        // until the paired CLOSE below; copy only the resulting hash out.
        unsafe { signer_fingerprint(data.hWVTStateData) }
    } else {
        Err(InspectionError::NotTrusted(status))
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
    result
}

/// Check one pinned certificate fingerprint after successful chain validation.
/// This does not establish trusted placement or bind a running process image.
pub fn inspect_expected_signer(
    path: &Path,
    expected: SignerFingerprint,
) -> Result<(), InspectionError> {
    if inspect_signer(path)? != expected {
        return Err(InspectionError::UnexpectedSigner);
    }
    Ok(())
}

unsafe fn signer_fingerprint(state: HANDLE) -> Result<SignerFingerprint, InspectionError> {
    // SAFETY: caller retains the successful WinVerifyTrust state until return.
    let provider = unsafe { WTHelperProvDataFromStateData(state) };
    if provider.is_null() {
        return Err(InspectionError::SignerUnavailable);
    }
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, false, 0) };
    if signer.is_null() {
        return Err(InspectionError::SignerUnavailable);
    }
    let certificate = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
    if certificate.is_null() {
        return Err(InspectionError::SignerUnavailable);
    }
    let certificate = unsafe { (*certificate).pCert };
    if certificate.is_null() {
        return Err(InspectionError::SignerUnavailable);
    }
    let length = unsafe { (*certificate).cbCertEncoded } as usize;
    let bytes = unsafe { (*certificate).pbCertEncoded };
    if bytes.is_null() || length == 0 || length > 1024 * 1024 {
        return Err(InspectionError::SignerUnavailable);
    }
    let encoded = unsafe { std::slice::from_raw_parts(bytes, length) };
    let mut fingerprint = [0u8; 32];
    let mut size = fingerprint.len() as u32;
    unsafe {
        CryptHashCertificate2(
            windows::core::w!("SHA256"),
            0,
            None,
            Some(encoded),
            Some(fingerprint.as_mut_ptr()),
            &mut size,
        )
    }
    .map_err(|_| InspectionError::SignerUnavailable)?;
    if size != fingerprint.len() as u32 {
        return Err(InspectionError::SignerUnavailable);
    }
    Ok(SignerFingerprint(fingerprint))
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
            let actual = inspect_signer(path).unwrap();
            assert_eq!(inspect_expected_signer(path, actual), Ok(()));
            let mut wrong = actual;
            wrong.0[0] ^= 1;
            assert_eq!(
                inspect_expected_signer(path, wrong),
                Err(InspectionError::UnexpectedSigner)
            );
            if let Ok(expected) = std::env::var("SUMIBI_TEST_SIGNER_SHA256") {
                assert_eq!(expected.len(), 64);
                let mut bytes = [0u8; 32];
                for (i, byte) in bytes.iter_mut().enumerate() {
                    *byte = u8::from_str_radix(&expected[i * 2..i * 2 + 2], 16).unwrap();
                }
                assert_eq!(actual, SignerFingerprint(bytes));
            }
        }
    }
}
