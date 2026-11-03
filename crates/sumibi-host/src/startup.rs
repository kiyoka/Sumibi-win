//! Prepare a Host from saved settings and a separately stored credential.
//! Run on a startup worker. No send consent is restored across restarts.
use crate::credential_store;
use crate::manual::Credential;
use crate::policy::PolicyAuthority;
use crate::settings::{Settings, SettingsError};
use std::path::Path;
use std::sync::Arc;
use sumibi_credentials::StoreError;

/// Own this authority while the Broker borrows it. A credential-store failure
/// leaves the Host available for settings repair, with network sending denied.
pub struct PreparedHost {
    settings: Settings,
    authority: Arc<PolicyAuthority>,
    credential_outcome: Result<(), StoreError>,
}
impl PreparedHost {
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        Self::load_with(path, credential_store::load_credential)
    }

    fn load_with(
        path: &Path,
        credential_loader: impl FnOnce() -> Result<Option<Credential>, StoreError>,
    ) -> Result<Self, SettingsError> {
        // Validate the complete saved destination before accessing the key.
        let settings = Settings::load(path)?;
        let authority = Arc::new(settings.new_authority()?);
        let (_, credential_outcome) = authority
            .update_stored_credential(credential_loader)
            .map_err(SettingsError::Policy)?;
        Ok(Self {
            settings,
            authority,
            credential_outcome,
        })
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    pub fn authority(&self) -> &Arc<PolicyAuthority> {
        &self.authority
    }
    pub fn credential_outcome(&self) -> Result<(), StoreError> {
        self.credential_outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{PolicySource, Scheme};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join(format!(
                    "sumibi-startup-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("settings.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.path());
            let _ = std::fs::remove_file(self.0.join(".settings.json.lock"));
            let _ = std::fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn invalid_settings_never_read_a_credential() {
        let fixture = Fixture::new();
        std::fs::write(fixture.path(), br#"{"version":99}"#).unwrap();
        let called = std::cell::Cell::new(false);
        assert!(PreparedHost::load_with(&fixture.path(), || {
            called.set(true);
            Ok(None)
        })
        .is_err());
        assert!(!called.get());
    }

    #[test]
    fn startup_restores_settings_and_keeps_loaded_missing_or_failed_keys_denied() {
        let fixture = Fixture::new();
        Settings::new(Scheme::Https, "example.test", 443, "/v1", "fixture-model")
            .unwrap()
            .save(&fixture.path())
            .unwrap();
        for result in [
            Ok(Credential::new("synthetic-key".into())),
            Ok(None),
            Err(StoreError::Unavailable),
        ] {
            let expected = result.as_ref().map(|_| ()).map_err(|error| *error);
            let expected_key = result.as_ref().is_ok_and(|key| key.is_some());
            let host = PreparedHost::load_with(&fixture.path(), || result).unwrap();
            assert_eq!(host.credential_outcome(), expected);
            assert_eq!(host.settings().model(), "fixture-model");
            let policy = host.authority().snapshot().unwrap();
            assert!(!policy.allowed && policy.manual_consent.is_none());
            assert_eq!(policy.credential.is_some(), expected_key);
            assert_eq!(policy.model.as_deref(), Some("fixture-model"));
        }
    }
}
