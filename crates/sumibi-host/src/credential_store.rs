//! Host-only bridge to Windows Credential Manager. Consent remains separate.
use crate::manual::Credential;
use crate::policy::{ChangeError, PolicyAuthority, PolicyVersion};
use sumibi_credentials::StoreError;

/// Both success and store failure carry a policy version that must be
/// published to Broker clients before a settings UI reports completion.
#[must_use = "publish the policy version and check the storage outcome"]
pub struct CredentialUpdate {
    pub version: PolicyVersion,
    pub outcome: Result<(), StoreError>,
}

/// Synchronize a stored key on a settings worker. Errors leave sending denied.
pub fn reload(authority: &PolicyAuthority) -> Result<CredentialUpdate, ChangeError> {
    apply(authority, load_credential)
}

/// Save the provided key and revoke old consent even when the store fails.
pub fn save(authority: &PolicyAuthority, secret: String) -> Result<CredentialUpdate, ChangeError> {
    apply(authority, || {
        sumibi_credentials::save(&secret)?;
        Credential::new(secret)
            .map(Some)
            .ok_or(StoreError::InvalidSecret)
    })
}

/// Remove the stored key and revoke old consent even when deletion fails.
pub fn delete(authority: &PolicyAuthority) -> Result<CredentialUpdate, ChangeError> {
    apply(authority, || {
        sumibi_credentials::delete()?;
        Ok(None)
    })
}

fn apply(
    authority: &PolicyAuthority,
    operation: impl FnOnce() -> Result<Option<Credential>, StoreError>,
) -> Result<CredentialUpdate, ChangeError> {
    let (version, outcome) = authority.update_stored_credential(operation)?;
    Ok(CredentialUpdate { version, outcome })
}

/// Load a key as a Host credential. The caller must replace the policy key
/// through PolicyAuthority so existing consent is invalidated and new consent
/// is explicitly requested. Storage errors must not preserve old authority.
pub fn load_credential() -> Result<Option<Credential>, StoreError> {
    match sumibi_credentials::load()? {
        Some(secret) => Credential::new(secret)
            .map(Some)
            .ok_or(StoreError::InvalidRecord),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{Destination, PolicySource, Scheme};

    #[test]
    fn storage_failure_invalidates_existing_consent_and_publishes_version() {
        let destination = Destination::new(Scheme::Https, "example.test", 443, "/v1").unwrap();
        let authority = PolicyAuthority::new(destination.clone());
        authority
            .replace_credential(Credential::new("old-key".into()))
            .unwrap();
        let prior = authority.grant_manual_consent(&destination).unwrap();
        let update = apply(&authority, || Err(StoreError::Unavailable)).unwrap();
        assert_eq!(update.outcome, Err(StoreError::Unavailable));
        assert_eq!(update.version.authorization, prior.authorization + 1);
        let snapshot = authority.snapshot().unwrap();
        assert!(!snapshot.allowed);
        assert!(snapshot.manual_consent.is_none());
        assert!(snapshot.credential.is_none());
        let wire = authority.wire_change_after(prior).unwrap().unwrap();
        assert_eq!(wire.authorization, update.version.authorization);
        assert!(!wire.allowed);
        assert_eq!(
            authority.grant_manual_consent(&destination),
            Err(ChangeError::InvalidConsent)
        );
        let loaded = apply(&authority, || Ok(Credential::new("new-key".into()))).unwrap();
        assert_eq!(loaded.outcome, Ok(()));
        assert!(!authority.snapshot().unwrap().allowed);
        assert!(authority.grant_manual_consent(&destination).is_ok());
    }

    #[test]
    fn update_waits_for_active_send_guard_before_touching_store() {
        use std::sync::{mpsc, Arc};
        use std::time::Duration;
        let destination = Destination::new(Scheme::Https, "example.test", 443, "/v1").unwrap();
        let authority = Arc::new(PolicyAuthority::new(destination.clone()));
        authority
            .replace_credential(Credential::new("old-key".into()))
            .unwrap();
        let version = authority.grant_manual_consent(&destination).unwrap();
        let credential = authority.snapshot().unwrap().credential.unwrap();
        let guard = authority
            .authorize_send_start(
                sumibi_protocol::message::Revision {
                    input: 1,
                    settings: version.settings,
                    authorization: version.authorization,
                },
                &destination,
                &credential,
            )
            .unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (store_tx, store_rx) = mpsc::channel();
        let other = authority.clone();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            apply(&other, || {
                store_tx.send(()).unwrap();
                Ok(None)
            })
            .unwrap()
        });
        started_rx.recv().unwrap();
        assert!(store_rx.recv_timeout(Duration::from_millis(30)).is_err());
        drop(guard);
        store_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(worker.join().unwrap().outcome.is_ok());
        assert!(!authority.snapshot().unwrap().allowed);
    }
}
