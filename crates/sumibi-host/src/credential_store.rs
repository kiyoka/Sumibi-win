//! Host-only bridge to Windows Credential Manager. Consent remains separate.
use crate::manual::Credential;
use crate::policy::{ChangeError, PolicyAuthority, PolicyVersion};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use sumibi_credentials::StoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialTaskError {
    Busy,
    Spawn,
    WorkerFailed,
    Policy(ChangeError),
}

/// Owned receipt returned to the Broker thread after the storage worker exits.
#[must_use = "publish the policy version and check the storage outcome"]
pub struct OwnedCredentialUpdate {
    authority: Arc<PolicyAuthority>,
    version: PolicyVersion,
    outcome: Result<(), StoreError>,
}
impl OwnedCredentialUpdate {
    pub fn version(&self) -> PolicyVersion {
        self.version
    }
    pub fn outcome(&self) -> Result<(), StoreError> {
        self.outcome
    }
    pub(crate) fn belongs_to(&self, authority: &PolicyAuthority) -> bool {
        std::ptr::eq(Arc::as_ptr(&self.authority), authority)
    }
}

/// One outstanding credential operation per authority. Dropping this task does
/// not cancel an already-started store operation; the Broker must continue
/// publishing policy changes even if the settings UI closes.
#[must_use = "poll the task and publish its returned update"]
pub struct CredentialTask {
    worker: Option<JoinHandle<Result<OwnedCredentialUpdate, CredentialTaskError>>>,
}
impl CredentialTask {
    pub fn reload(authority: Arc<PolicyAuthority>) -> Result<Self, CredentialTaskError> {
        Self::start_with(authority, load_credential)
    }

    /// Explicitly save a user-provided key on the settings worker.
    pub fn save(
        authority: Arc<PolicyAuthority>,
        secret: String,
    ) -> Result<Self, CredentialTaskError> {
        Self::start_with(authority, move || save_credential(secret))
    }

    /// Explicitly remove the fixed key on the settings worker.
    pub fn delete(authority: Arc<PolicyAuthority>) -> Result<Self, CredentialTaskError> {
        Self::start_with(authority, delete_credential)
    }

    fn start_with(
        authority: Arc<PolicyAuthority>,
        operation: impl FnOnce() -> Result<Option<Credential>, StoreError> + Send + 'static,
    ) -> Result<Self, CredentialTaskError> {
        if !authority.reserve_configuration_task() {
            return Err(CredentialTaskError::Busy);
        }
        let worker_authority = authority.clone();
        let result = thread::Builder::new()
            .name("sumibi-credential-store".into())
            .spawn(move || {
                struct Reservation(Arc<PolicyAuthority>);
                impl Drop for Reservation {
                    fn drop(&mut self) {
                        self.0.release_configuration_task();
                    }
                }
                let _reservation = Reservation(worker_authority.clone());
                let (version, outcome) = worker_authority
                    .update_stored_credential(operation)
                    .map_err(CredentialTaskError::Policy)?;
                Ok(OwnedCredentialUpdate {
                    authority: worker_authority,
                    version,
                    outcome,
                })
            });
        match result {
            Ok(worker) => Ok(Self {
                worker: Some(worker),
            }),
            Err(_) => {
                authority.release_configuration_task();
                Err(CredentialTaskError::Spawn)
            }
        }
    }

    /// Nonblocking poll. A completed worker is consumed exactly once.
    pub fn try_take(&mut self) -> Option<Result<OwnedCredentialUpdate, CredentialTaskError>> {
        if !self.worker.as_ref()?.is_finished() {
            return None;
        }
        Some(
            self.worker
                .take()
                .unwrap()
                .join()
                .unwrap_or(Err(CredentialTaskError::WorkerFailed)),
        )
    }

    #[cfg(test)]
    pub(crate) fn start_fixture(
        authority: Arc<PolicyAuthority>,
    ) -> Result<Self, CredentialTaskError> {
        Self::start_with(authority, || Err(StoreError::Unavailable))
    }
}

/// Both success and store failure carry a policy version that must be
/// published to Broker clients before a settings UI reports completion.
#[must_use = "publish the policy version and check the storage outcome"]
pub struct CredentialUpdate<'a> {
    authority: &'a PolicyAuthority,
    version: PolicyVersion,
    outcome: Result<(), StoreError>,
}
impl CredentialUpdate<'_> {
    pub fn version(&self) -> PolicyVersion {
        self.version
    }
    pub fn outcome(&self) -> Result<(), StoreError> {
        self.outcome
    }
    pub(crate) fn belongs_to(&self, authority: &PolicyAuthority) -> bool {
        std::ptr::eq(self.authority, authority)
    }
}

/// Synchronize a stored key on a settings worker. Errors leave sending denied.
pub fn reload(authority: &PolicyAuthority) -> Result<CredentialUpdate<'_>, ChangeError> {
    apply(authority, load_credential)
}

/// Save the provided key and revoke old consent even when the store fails.
pub fn save(
    authority: &PolicyAuthority,
    secret: String,
) -> Result<CredentialUpdate<'_>, ChangeError> {
    apply(authority, || save_credential(secret))
}

/// Remove the stored key and revoke old consent even when deletion fails.
pub fn delete(authority: &PolicyAuthority) -> Result<CredentialUpdate<'_>, ChangeError> {
    apply(authority, delete_credential)
}

fn save_credential(secret: String) -> Result<Option<Credential>, StoreError> {
    sumibi_credentials::save(&secret)?;
    Credential::new(secret)
        .map(Some)
        .ok_or(StoreError::InvalidSecret)
}

fn delete_credential() -> Result<Option<Credential>, StoreError> {
    sumibi_credentials::delete()?;
    Ok(None)
}

fn apply(
    authority: &PolicyAuthority,
    operation: impl FnOnce() -> Result<Option<Credential>, StoreError>,
) -> Result<CredentialUpdate<'_>, ChangeError> {
    let (version, outcome) = authority.update_stored_credential(operation)?;
    Ok(CredentialUpdate {
        authority,
        version,
        outcome,
    })
}

#[cfg(test)]
pub(crate) fn synthetic_update(
    authority: &PolicyAuthority,
    outcome: Result<Option<Credential>, StoreError>,
) -> Result<CredentialUpdate<'_>, ChangeError> {
    apply(authority, || outcome)
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
    fn reload_worker_is_bounded_and_returns_an_owned_failure_receipt() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let destination = Destination::new(Scheme::Https, "example.test", 443, "/v1").unwrap();
        let authority = Arc::new(PolicyAuthority::new(destination.clone()));
        authority
            .replace_credential(Credential::new("old-key".into()))
            .unwrap();
        let old = authority.grant_manual_consent(&destination).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut task = CredentialTask::start_with(authority.clone(), move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Err(StoreError::Unavailable)
        })
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(task.try_take().is_none());
        assert!(matches!(
            CredentialTask::save(authority.clone(), "synthetic".into()),
            Err(CredentialTaskError::Busy)
        ));
        assert!(matches!(
            CredentialTask::delete(authority.clone()),
            Err(CredentialTaskError::Busy)
        ));
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let update = loop {
            if let Some(result) = task.try_take() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(update.outcome(), Err(StoreError::Unavailable));
        assert_eq!(update.version().authorization, old.authorization + 1);
        assert!(update.belongs_to(&authority));
        assert!(!authority.snapshot().unwrap().allowed);
        assert!(task.try_take().is_none());
        let mut next = CredentialTask::start_with(authority.clone(), || Ok(None)).unwrap();
        loop {
            if let Some(result) = next.try_take() {
                assert!(result.unwrap().outcome().is_ok());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn invalid_save_on_worker_revokes_previous_key_without_storing() {
        use std::time::{Duration, Instant};
        let destination = Destination::new(Scheme::Https, "example.test", 443, "/v1").unwrap();
        let authority = Arc::new(PolicyAuthority::new(destination.clone()));
        authority
            .replace_credential(Credential::new("old-key".into()))
            .unwrap();
        let before = authority.grant_manual_consent(&destination).unwrap();
        let mut task = CredentialTask::save(authority.clone(), "invalid key".into()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let update = loop {
            if let Some(result) = task.try_take() {
                break result.unwrap();
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(update.outcome(), Err(StoreError::InvalidSecret));
        assert_eq!(update.version().authorization, before.authorization + 1);
        let state = authority.snapshot().unwrap();
        assert!(!state.allowed && state.credential.is_none());
    }

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
            .outcome()
        });
        started_rx.recv().unwrap();
        assert!(store_rx.recv_timeout(Duration::from_millis(30)).is_err());
        drop(guard);
        store_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(worker.join().unwrap().is_ok());
        assert!(!authority.snapshot().unwrap().allowed);
    }
}
