//! In-memory Host authority for a coherent manual-send policy snapshot.
//! Persistence, credential loading, UI consent and IPC publication are separate.
use crate::manual::{Credential, Destination, PolicySnapshot, PolicySource};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyVersion {
    pub settings: u64,
    pub authorization: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeError {
    InvalidConsent,
    Exhausted,
    Unavailable,
}

/// Mutations publish the complete new state under one lock. A failed or
/// poisoned update cannot expose a partly changed policy to a worker.
pub struct PolicyAuthority {
    state: Mutex<PolicySnapshot>,
}

impl PolicyAuthority {
    pub fn new(destination: Destination) -> Self {
        Self {
            state: Mutex::new(PolicySnapshot {
                settings: 1,
                authorization: 1,
                destination,
                manual_consent: None,
                credential: None,
                allowed: false,
            }),
        }
    }

    /// A changed destination invalidates both the old consent and credential
    /// authorization. The credential stays resident but cannot be sent yet.
    pub fn set_destination(&self, destination: Destination) -> Result<PolicyVersion, ChangeError> {
        let mut state = self.state.lock().map_err(|_| ChangeError::Unavailable)?;
        let settings = state
            .settings
            .checked_add(1)
            .ok_or(ChangeError::Exhausted)?;
        let authorization = state
            .authorization
            .checked_add(1)
            .ok_or(ChangeError::Exhausted)?;
        state.destination = destination;
        state.settings = settings;
        state.authorization = authorization;
        state.manual_consent = None;
        state.allowed = false;
        Ok(version(&state))
    }

    /// Even replacing a key with the same bytes changes its authority epoch.
    /// The old key remains only in snapshots already held by a worker.
    pub fn replace_credential(
        &self,
        credential: Option<Credential>,
    ) -> Result<PolicyVersion, ChangeError> {
        let mut state = self.state.lock().map_err(|_| ChangeError::Unavailable)?;
        let authorization = state
            .authorization
            .checked_add(1)
            .ok_or(ChangeError::Exhausted)?;
        state.credential = credential.map(Arc::new);
        state.authorization = authorization;
        state.manual_consent = None;
        state.allowed = false;
        Ok(version(&state))
    }

    /// Called only after an explicit user action by the future settings UI.
    pub fn grant_manual_consent(
        &self,
        destination: &Destination,
    ) -> Result<PolicyVersion, ChangeError> {
        let mut state = self.state.lock().map_err(|_| ChangeError::Unavailable)?;
        if state.destination != *destination || state.credential.is_none() {
            return Err(ChangeError::InvalidConsent);
        }
        let authorization = state
            .authorization
            .checked_add(1)
            .ok_or(ChangeError::Exhausted)?;
        state.authorization = authorization;
        state.manual_consent = Some(destination.clone());
        state.allowed = true;
        Ok(version(&state))
    }

    pub fn revoke_manual_consent(&self) -> Result<PolicyVersion, ChangeError> {
        let mut state = self.state.lock().map_err(|_| ChangeError::Unavailable)?;
        let authorization = state
            .authorization
            .checked_add(1)
            .ok_or(ChangeError::Exhausted)?;
        state.authorization = authorization;
        state.manual_consent = None;
        state.allowed = false;
        Ok(version(&state))
    }
}

impl PolicySource for PolicyAuthority {
    fn snapshot(&self) -> Option<PolicySnapshot> {
        self.state.lock().ok().map(|state| state.clone())
    }
}

fn version(state: &PolicySnapshot) -> PolicyVersion {
    PolicyVersion {
        settings: state.settings,
        authorization: state.authorization,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{
        self, ConversionTransport, ManualRequest, Scheme, SendError, TransportError,
    };
    use std::time::{Duration, Instant};
    use sumibi_protocol::message::Revision;

    fn destination(host: &str) -> Destination {
        Destination::new(Scheme::Https, host, 443, "/convert").unwrap()
    }

    fn request(destination: &Destination, version: PolicyVersion) -> ManualRequest<'_> {
        ManualRequest {
            revision: Revision {
                input: 1,
                settings: version.settings,
                authorization: version.authorization,
            },
            destination,
            source: "nihongo",
            deadline: Instant::now() + Duration::from_secs(1),
        }
    }

    struct Mock<'a> {
        authority: Option<&'a PolicyAuthority>,
    }
    impl ConversionTransport for Mock<'_> {
        fn convert(
            &self,
            _: &Destination,
            credential: &str,
            _: &str,
            _: Instant,
        ) -> Result<String, TransportError> {
            assert_eq!(credential, "synthetic-key");
            if let Some(authority) = self.authority {
                authority.revoke_manual_consent().unwrap();
            }
            Ok("日本語".into())
        }
    }

    #[test]
    fn explicit_consent_is_required_and_revocation_discards_inflight_result() {
        let destination = destination("api.example.test");
        let authority = PolicyAuthority::new(destination.clone());
        let initial = PolicyVersion {
            settings: 1,
            authorization: 1,
        };
        assert_eq!(
            manual::convert(
                request(&destination, initial),
                &authority,
                &Mock { authority: None }
            ),
            Err(SendError::PolicyDenied)
        );
        assert_eq!(
            authority.grant_manual_consent(&destination),
            Err(ChangeError::InvalidConsent)
        );
        let keyed = authority
            .replace_credential(Some(Credential::new("synthetic-key".into()).unwrap()))
            .unwrap();
        assert_eq!(keyed.authorization, 2);
        assert_eq!(
            manual::convert(
                request(&destination, keyed),
                &authority,
                &Mock { authority: None }
            ),
            Err(SendError::PolicyDenied)
        );
        let allowed = authority.grant_manual_consent(&destination).unwrap();
        assert_eq!(
            manual::convert(
                request(&destination, allowed),
                &authority,
                &Mock { authority: None }
            ),
            Ok("日本語".into())
        );
        assert_eq!(
            manual::convert(
                request(&destination, allowed),
                &authority,
                &Mock {
                    authority: Some(&authority)
                }
            ),
            Err(SendError::PolicyDenied)
        );
        assert_eq!(
            manual::convert(
                request(&destination, allowed),
                &authority,
                &Mock { authority: None }
            ),
            Err(SendError::PolicyDenied)
        );
    }

    #[test]
    fn destination_and_key_changes_clear_consent_and_advance_versions() {
        let first = destination("api.example.test");
        let second = destination("other.example.test");
        let authority = PolicyAuthority::new(first.clone());
        authority
            .replace_credential(Some(Credential::new("synthetic-key".into()).unwrap()))
            .unwrap();
        let old = authority.grant_manual_consent(&first).unwrap();
        let changed = authority.set_destination(second.clone()).unwrap();
        assert_eq!(changed.settings, old.settings + 1);
        assert_eq!(changed.authorization, old.authorization + 1);
        assert_eq!(
            authority.grant_manual_consent(&first),
            Err(ChangeError::InvalidConsent)
        );
        assert_eq!(
            manual::convert(request(&first, old), &authority, &Mock { authority: None }),
            Err(SendError::PolicyDenied)
        );
        let renewed = authority.grant_manual_consent(&second).unwrap();
        assert_eq!(
            manual::convert(
                request(&second, renewed),
                &authority,
                &Mock { authority: None }
            ),
            Ok("日本語".into())
        );
        let replaced = authority
            .replace_credential(Some(Credential::new("synthetic-key".into()).unwrap()))
            .unwrap();
        assert_eq!(replaced.settings, renewed.settings);
        assert_eq!(replaced.authorization, renewed.authorization + 1);
        assert_eq!(
            manual::convert(
                request(&second, renewed),
                &authority,
                &Mock { authority: None }
            ),
            Err(SendError::PolicyDenied)
        );
        let deleted = authority.replace_credential(None).unwrap();
        assert_eq!(
            authority.grant_manual_consent(&second),
            Err(ChangeError::InvalidConsent)
        );
        assert_eq!(
            manual::convert(
                request(&second, deleted),
                &authority,
                &Mock { authority: None }
            ),
            Err(SendError::PolicyDenied)
        );
    }

    #[test]
    fn exhausted_epoch_is_atomic_and_poisoned_state_fails_closed() {
        let first = destination("api.example.test");
        let second = destination("other.example.test");
        let settings_exhausted = PolicyAuthority::new(first.clone());
        settings_exhausted.state.lock().unwrap().settings = u64::MAX;
        assert_eq!(
            settings_exhausted.set_destination(second),
            Err(ChangeError::Exhausted)
        );
        let unchanged = settings_exhausted.snapshot().unwrap();
        assert!(unchanged.destination == first);
        assert_eq!(unchanged.authorization, 1);

        let authority = PolicyAuthority::new(destination("api.example.test"));
        {
            let mut state = authority.state.lock().unwrap();
            state.authorization = u64::MAX;
        }
        assert_eq!(
            authority.revoke_manual_consent(),
            Err(ChangeError::Exhausted)
        );
        let snapshot = authority.snapshot().unwrap();
        assert_eq!(snapshot.authorization, u64::MAX);
        assert!(!snapshot.allowed);
        let _ = std::panic::catch_unwind(|| {
            let _guard = authority.state.lock().unwrap();
            panic!("poison test");
        });
        assert!(authority.snapshot().is_none());
        assert_eq!(
            authority.revoke_manual_consent(),
            Err(ChangeError::Unavailable)
        );
    }
}
