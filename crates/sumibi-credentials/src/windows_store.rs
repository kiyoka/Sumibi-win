use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::*;

// Fixed name: callers cannot overwrite or remove arbitrary credentials.
const TARGET: &str = "Sumibi-win/API-key/v1";
const MAX_BYTES: usize = CRED_MAX_CREDENTIAL_BLOB_SIZE as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    InvalidSecret,
    InvalidRecord,
    Unavailable,
}

fn valid_secret(secret: &[u8]) -> bool {
    !secret.is_empty() && secret.len() <= MAX_BYTES && secret.iter().all(u8::is_ascii_graphic)
}

/// Store a UTF-8 ASCII API key under the fixed per-user target. This operation
/// overwrites only the existing Sumibi key; it does not change Host consent.
pub fn save(secret: &str) -> Result<(), StoreError> {
    save_at(TARGET, secret, CRED_PERSIST_LOCAL_MACHINE)
}

/// Read the key without emitting it in errors or logs. No key means None.
pub fn load() -> Result<Option<String>, StoreError> {
    load_at(TARGET)
}

/// Delete only the fixed Sumibi key; missing credentials are already absent.
pub fn delete() -> Result<(), StoreError> {
    delete_at(TARGET)
}

fn target_wide(target: &str) -> Vec<u16> {
    target.encode_utf16().chain(std::iter::once(0)).collect()
}

fn save_at(target: &str, secret: &str, persistence: CRED_PERSIST) -> Result<(), StoreError> {
    if !valid_secret(secret.as_bytes()) {
        return Err(StoreError::InvalidSecret);
    }
    let mut wide = target_wide(target);
    let record = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(wide.as_mut_ptr()),
        CredentialBlobSize: secret.len() as u32,
        CredentialBlob: secret.as_ptr() as *mut u8,
        Persist: persistence,
        ..Default::default()
    };
    // SAFETY: target and blob remain alive and immutable for the API call.
    unsafe { CredWriteW(&record, 0) }.map_err(|_| StoreError::Unavailable)
}

struct CredentialBuffer(*mut CREDENTIALW);
impl Drop for CredentialBuffer {
    fn drop(&mut self) {
        // SAFETY: pointer was returned by CredReadW and is freed exactly once.
        unsafe { CredFree(self.0.cast()) }
    }
}

fn load_at(target: &str) -> Result<Option<String>, StoreError> {
    let wide = target_wide(target);
    let mut ptr = std::ptr::null_mut();
    // SAFETY: wide is NUL terminated and ptr receives the API-owned buffer.
    if let Err(error) =
        unsafe { CredReadW(PCWSTR(wide.as_ptr()), CRED_TYPE_GENERIC, None, &mut ptr) }
    {
        return if error.code() == ERROR_NOT_FOUND.to_hresult() {
            Ok(None)
        } else {
            Err(StoreError::Unavailable)
        };
    }
    if ptr.is_null() {
        return Err(StoreError::InvalidRecord);
    }
    let guard = CredentialBuffer(ptr);
    // SAFETY: CredReadW returned a live CREDENTIALW owned by guard.
    let record = unsafe { &*guard.0 };
    let size = record.CredentialBlobSize as usize;
    if record.Type != CRED_TYPE_GENERIC
        || size == 0
        || size > MAX_BYTES
        || record.CredentialBlob.is_null()
    {
        return Err(StoreError::InvalidRecord);
    }
    // SAFETY: Windows supplies a buffer of CredentialBlobSize bytes.
    let bytes = unsafe { std::slice::from_raw_parts(record.CredentialBlob, size) };
    if !valid_secret(bytes) {
        return Err(StoreError::InvalidRecord);
    }
    let value = std::str::from_utf8(bytes).map_err(|_| StoreError::InvalidRecord)?;
    Ok(Some(value.to_owned()))
}

fn delete_at(target: &str) -> Result<(), StoreError> {
    let wide = target_wide(target);
    // SAFETY: wide is valid and NUL terminated for the duration of the call.
    match unsafe { CredDeleteW(PCWSTR(wide.as_ptr()), CRED_TYPE_GENERIC, None) } {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
        Err(_) => Err(StoreError::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_validation_rejects_empty_control_and_oversize() {
        assert!(valid_secret(b"synthetic-key"));
        assert!(!valid_secret(b""));
        assert!(!valid_secret(b"contains space"));
        assert!(!valid_secret(b"control\n"));
        assert!(!valid_secret(&vec![b'a'; MAX_BYTES + 1]));
    }
    #[test]
    fn session_fixture_round_trip_and_delete() {
        if std::env::var_os("SUMIBI_CREDENTIAL_TEST").is_none() {
            eprintln!("credential store fixture skipped: not enabled");
            return;
        }
        let target = format!(
            "Sumibi-win/test/{}/{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = delete_at(&self.0);
            }
        }
        let _cleanup = Cleanup(target.clone());
        assert!(load_at(&target).unwrap().is_none());
        save_at(&target, "synthetic-key", CRED_PERSIST_SESSION).unwrap();
        assert_eq!(load_at(&target).unwrap().as_deref(), Some("synthetic-key"));
        save_at(&target, "new-synthetic-key", CRED_PERSIST_SESSION).unwrap();
        assert_eq!(
            load_at(&target).unwrap().as_deref(),
            Some("new-synthetic-key")
        );
        assert_eq!(
            save_at(&target, "bad key", CRED_PERSIST_SESSION),
            Err(StoreError::InvalidSecret)
        );
        assert_eq!(
            load_at(&target).unwrap().as_deref(),
            Some("new-synthetic-key")
        );
        delete_at(&target).unwrap();
        assert!(load_at(&target).unwrap().is_none());
        delete_at(&target).unwrap();
        println!("session-only credential fixture completed and removed");
    }
}
