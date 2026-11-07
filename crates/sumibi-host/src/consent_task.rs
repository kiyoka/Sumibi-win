//! Version-bound consent for an explicit UI action, with nonblocking polling.
use crate::manual::{Destination, Scheme};
use crate::policy::{ChangeError, PolicyAuthority, PolicyVersion};
use crate::settings::Settings;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentTaskError {
    Busy,
    Spawn,
    WorkerFailed,
    Policy(ChangeError),
}

/// Immutable, nonsecret view of exactly the authority and epochs to approve.
/// Render its destination and model before an explicit grant action. Refresh
/// after changes; cloning or rendering an offer never grants consent.
#[derive(Clone)]
pub struct ConsentOffer {
    authority: Arc<PolicyAuthority>,
    version: PolicyVersion,
    destination: Destination,
    model: Option<String>,
    credential_present: bool,
    allowed: bool,
}
impl ConsentOffer {
    /// A busy authority returns None immediately. Disable consent actions until
    /// a fresh offer can be obtained; do not substitute a cached offer.
    pub fn try_capture(authority: Arc<PolicyAuthority>) -> Result<Option<Self>, ChangeError> {
        let Some(state) = authority.try_snapshot()? else {
            return Ok(None);
        };
        Ok(Some(Self {
            authority,
            version: PolicyVersion {
                settings: state.settings,
                authorization: state.authorization,
            },
            destination: state.destination,
            model: state.model,
            credential_present: state.credential.is_some(),
            allowed: state.allowed,
        }))
    }
    pub fn version(&self) -> PolicyVersion {
        self.version
    }
    pub fn destination(&self) -> &Destination {
        &self.destination
    }
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
    pub fn credential_present(&self) -> bool {
        self.credential_present
    }
    pub fn allowed(&self) -> bool {
        self.allowed
    }
    pub fn can_grant(&self) -> bool {
        self.destination.scheme() == Scheme::Https
            && self.credential_present
            && self
                .model
                .as_deref()
                .is_some_and(crate::first_conversion::valid_model)
    }
    pub(crate) fn belongs_to(&self, authority: &PolicyAuthority) -> bool {
        std::ptr::eq(Arc::as_ptr(&self.authority), authority)
    }
}

/// Local mutation is not delivery completion. Keep this receipt and have the
/// matching Broker publish it and reap cancelled workers before reporting done.
#[must_use = "acknowledge consent through the matching Broker"]
pub struct ConsentUpdate {
    authority: Arc<PolicyAuthority>,
    version: PolicyVersion,
}
impl ConsentUpdate {
    pub fn version(&self) -> PolicyVersion {
        self.version
    }
    pub(crate) fn belongs_to(&self, authority: &PolicyAuthority) -> bool {
        std::ptr::eq(Arc::as_ptr(&self.authority), authority)
    }
}

/// Shares the authority's single configuration slot with saves and credential
/// tasks. Drop does not cancel an explicit action already in progress. The
/// Broker must continue publishing changes if a UI or task handle disappears.
#[must_use = "poll consent and publish its returned version"]
pub struct ConsentTask {
    worker: Option<JoinHandle<Result<ConsentUpdate, ConsentTaskError>>>,
}
impl ConsentTask {
    pub fn grant(offer: ConsentOffer) -> Result<Self, ConsentTaskError> {
        Self::grant_remembered(offer, None)
    }

    pub(crate) fn grant_remembered(
        offer: ConsentOffer,
        path: Option<PathBuf>,
    ) -> Result<Self, ConsentTaskError> {
        if !offer.can_grant() {
            return Err(ConsentTaskError::Policy(ChangeError::InvalidConsent));
        }
        let authority = offer.authority.clone();
        Self::start(authority, move |authority| {
            authority.grant_manual_consent_at_version(
                &offer.destination,
                offer.model.as_deref().unwrap(),
                offer.version,
                || match path {
                    Some(path) => Settings::write_manual_consent(
                        &path,
                        Some((&offer.destination, offer.model.as_deref().unwrap())),
                    ),
                    None => Ok(()),
                },
            )
        })
    }

    /// Revocation intentionally needs no fresh offer: it always revokes the
    /// current policy once it obtains the lock, including after settings change.
    pub fn revoke(authority: Arc<PolicyAuthority>) -> Result<Self, ConsentTaskError> {
        Self::start(authority, PolicyAuthority::revoke_manual_consent)
    }

    pub(crate) fn revoke_remembered(
        authority: Arc<PolicyAuthority>,
        path: Option<PathBuf>,
    ) -> Result<Self, ConsentTaskError> {
        Self::start(authority, move |authority| {
            authority.revoke_manual_consent_with(|| match path {
                Some(path) => Settings::write_manual_consent(&path, None),
                None => Ok(()),
            })
        })
    }

    fn start(
        authority: Arc<PolicyAuthority>,
        operation: impl FnOnce(&PolicyAuthority) -> Result<PolicyVersion, ChangeError> + Send + 'static,
    ) -> Result<Self, ConsentTaskError> {
        if !authority.reserve_configuration_task() {
            return Err(ConsentTaskError::Busy);
        }
        let worker_authority = authority.clone();
        let result = thread::Builder::new()
            .name("sumibi-consent".into())
            .spawn(move || {
                struct Reservation(Arc<PolicyAuthority>);
                impl Drop for Reservation {
                    fn drop(&mut self) {
                        self.0.release_configuration_task();
                    }
                }
                let _reservation = Reservation(worker_authority.clone());
                let version = operation(&worker_authority).map_err(ConsentTaskError::Policy)?;
                Ok(ConsentUpdate {
                    authority: worker_authority,
                    version,
                })
            });
        match result {
            Ok(worker) => Ok(Self {
                worker: Some(worker),
            }),
            Err(_) => {
                authority.release_configuration_task();
                Err(ConsentTaskError::Spawn)
            }
        }
    }

    pub fn try_take(&mut self) -> Option<Result<ConsentUpdate, ConsentTaskError>> {
        if !self.worker.as_ref()?.is_finished() {
            return None;
        }
        Some(
            self.worker
                .take()
                .unwrap()
                .join()
                .unwrap_or(Err(ConsentTaskError::WorkerFailed)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{Credential, PolicySource};
    use std::time::{Duration, Instant};
    use sumibi_protocol::message::Revision;

    fn configured() -> Arc<PolicyAuthority> {
        let destination =
            Destination::for_chat_completions(Scheme::Https, "example.test", 443, "/proxy")
                .unwrap();
        let authority = Arc::new(PolicyAuthority::new(destination));
        authority.set_model("fixture-model").unwrap();
        authority
            .replace_credential(Credential::new("synthetic-key".into()))
            .unwrap();
        authority
    }
    fn offer(authority: &Arc<PolicyAuthority>) -> ConsentOffer {
        ConsentOffer::try_capture(authority.clone())
            .unwrap()
            .unwrap()
    }
    fn take(task: &mut ConsentTask) -> Result<ConsentUpdate, ConsentTaskError> {
        let limit = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = task.try_take() {
                return result;
            }
            assert!(Instant::now() < limit);
            std::thread::yield_now();
        }
    }

    #[test]
    fn offers_are_nonsecret_observations_and_explicit_actions_advance_epochs() {
        let authority = configured();
        let preview = offer(&authority);
        assert_eq!(preview.destination().host(), "example.test");
        assert_eq!(
            preview.destination().base_path(),
            "/proxy/v1/chat/completions"
        );
        assert_eq!(preview.model(), Some("fixture-model"));
        assert!(preview.credential_present() && preview.can_grant() && !preview.allowed());
        assert!(!authority.snapshot().unwrap().allowed);
        let mut grant = ConsentTask::grant(preview.clone()).unwrap();
        let receipt = take(&mut grant).unwrap();
        assert_eq!(
            receipt.version().authorization,
            preview.version().authorization + 1
        );
        assert!(receipt.belongs_to(&authority));
        assert!(!receipt.belongs_to(&configured()));
        assert!(grant.try_take().is_none());
        assert!(offer(&authority).allowed());
        let mut revoke = ConsentTask::revoke(authority.clone()).unwrap();
        let revoked = take(&mut revoke).unwrap();
        assert_eq!(
            revoked.version().authorization,
            receipt.version().authorization + 1
        );
        let state = authority.snapshot().unwrap();
        assert!(!state.allowed && state.manual_consent.is_none());
        assert!(state.credential.is_some());
    }

    #[test]
    fn stale_offers_never_approve_changed_settings_keys_or_authorization() {
        for change in 0..5 {
            let authority = configured();
            let preview = offer(&authority);
            match change {
                0 => {
                    authority.set_model("other-model").unwrap();
                }
                1 => {
                    authority
                        .replace_credential(Credential::new("synthetic-key".into()))
                        .unwrap();
                }
                2 => {
                    authority
                        .set_destination(
                            Destination::new(Scheme::Https, "other.test", 443, "/").unwrap(),
                        )
                        .unwrap();
                }
                3 => {
                    authority.revoke_manual_consent().unwrap();
                }
                _ => {
                    let mut task = ConsentTask::grant(preview.clone()).unwrap();
                    let _receipt = take(&mut task).unwrap();
                }
            }
            let before = offer(&authority);
            let mut task = ConsentTask::grant(preview).unwrap();
            assert!(matches!(
                take(&mut task),
                Err(ConsentTaskError::Policy(ChangeError::StalePolicy))
            ));
            let after = offer(&authority);
            assert_eq!(after.version(), before.version());
            assert_eq!(after.allowed(), before.allowed());
        }
    }

    #[test]
    fn missing_model_key_or_https_cannot_start_a_grant() {
        for missing in 0..3 {
            let destination = Destination::new(
                if missing == 2 {
                    Scheme::HttpLoopback
                } else {
                    Scheme::Https
                },
                "127.0.0.1",
                1234,
                "/",
            )
            .unwrap();
            let authority = Arc::new(PolicyAuthority::new(destination));
            if missing != 0 {
                authority.set_model("fixture-model").unwrap();
            }
            if missing != 1 {
                authority
                    .replace_credential(Credential::new("synthetic-key".into()))
                    .unwrap();
            }
            let preview = offer(&authority);
            assert!(!preview.can_grant());
            let version = preview.version();
            assert!(matches!(
                ConsentTask::grant(preview),
                Err(ConsentTaskError::Policy(ChangeError::InvalidConsent))
            ));
            assert_eq!(offer(&authority).version(), version);
        }
    }

    #[test]
    fn consent_poll_and_capture_do_not_wait_for_an_outbound_send() {
        let authority = configured();
        let destination = offer(&authority).destination().clone();
        let version = authority.grant_manual_consent(&destination).unwrap();
        let preview = offer(&authority);
        let state = authority.snapshot().unwrap();
        let guard = authority
            .authorize_send_start(
                Revision {
                    input: 1,
                    settings: version.settings,
                    authorization: version.authorization,
                },
                &destination,
                state.credential.as_ref().unwrap(),
            )
            .unwrap();
        assert!(ConsentOffer::try_capture(authority.clone())
            .unwrap()
            .is_none());
        let mut revoke = ConsentTask::revoke(authority.clone()).unwrap();
        assert!(revoke.try_take().is_none());
        assert!(matches!(
            ConsentTask::grant(preview),
            Err(ConsentTaskError::Busy)
        ));
        assert!(matches!(
            crate::credential_store::CredentialTask::reload(authority.clone()),
            Err(crate::credential_store::CredentialTaskError::Busy)
        ));
        let settings = crate::settings::Settings::new(
            Scheme::Https,
            "example.test",
            443,
            "/",
            "fixture-model",
        )
        .unwrap();
        assert!(matches!(
            crate::settings_task::SettingsSaveTask::start(
                authority.clone(),
                std::path::PathBuf::new(),
                settings.clone(),
                settings
            ),
            Err(crate::settings_task::SaveTaskError::Busy)
        ));
        drop(guard);
        let _receipt = take(&mut revoke).unwrap();
        assert!(!offer(&authority).allowed());
    }

    #[test]
    fn dropping_an_explicit_action_does_not_cancel_its_eventual_revocation() {
        let authority = configured();
        let destination = offer(&authority).destination().clone();
        let version = authority.grant_manual_consent(&destination).unwrap();
        let state = authority.snapshot().unwrap();
        let guard = authority
            .authorize_send_start(
                Revision {
                    input: 1,
                    settings: version.settings,
                    authorization: version.authorization,
                },
                &destination,
                state.credential.as_ref().unwrap(),
            )
            .unwrap();
        drop(ConsentTask::revoke(authority.clone()).unwrap());
        assert!(matches!(
            ConsentTask::revoke(authority.clone()),
            Err(ConsentTaskError::Busy)
        ));
        drop(guard);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if ConsentOffer::try_capture(authority.clone())
                .unwrap()
                .is_some_and(|view| !view.allowed())
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
