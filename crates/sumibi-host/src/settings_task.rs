//! Bounded settings saves with owned receipts for the Broker owner thread.
use crate::policy::{PolicyAuthority, PolicyVersion};
use crate::settings::{Settings, SettingsError};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveTaskError {
    Busy,
    Spawn,
    WorkerFailed,
    Save(SettingsError),
}

#[must_use = "acknowledge the saved settings through the matching Broker"]
pub struct SavedSettings {
    authority: Arc<PolicyAuthority>,
    version: PolicyVersion,
}
impl SavedSettings {
    pub fn version(&self) -> PolicyVersion {
        self.version
    }
    pub(crate) fn belongs_to(&self, authority: &PolicyAuthority) -> bool {
        std::ptr::eq(Arc::as_ptr(&self.authority), authority)
    }
}

/// Settings and credential tasks share one slot per authority. Polling never
/// waits for storage. Dropping a task does not cancel a started file write;
/// the Broker must keep publishing source changes if the UI closes.
#[must_use = "poll the save and publish its returned version"]
pub struct SettingsSaveTask {
    worker: Option<JoinHandle<Result<SavedSettings, SaveTaskError>>>,
}
impl SettingsSaveTask {
    pub fn start(
        authority: Arc<PolicyAuthority>,
        path: PathBuf,
        expected: Settings,
        replacement: Settings,
    ) -> Result<Self, SaveTaskError> {
        if !authority.reserve_configuration_task() {
            return Err(SaveTaskError::Busy);
        }
        let worker_authority = authority.clone();
        let result = thread::Builder::new()
            .name("sumibi-settings-save".into())
            .spawn(move || {
                struct Reservation(Arc<PolicyAuthority>);
                impl Drop for Reservation {
                    fn drop(&mut self) {
                        self.0.release_configuration_task();
                    }
                }
                let _reservation = Reservation(worker_authority.clone());
                let version = replacement
                    .save_and_apply_if_unchanged(&path, &expected, &worker_authority)
                    .map_err(SaveTaskError::Save)?
                    .version();
                Ok(SavedSettings {
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
                Err(SaveTaskError::Spawn)
            }
        }
    }

    pub fn try_take(&mut self) -> Option<Result<SavedSettings, SaveTaskError>> {
        if !self.worker.as_ref()?.is_finished() {
            return None;
        }
        Some(
            self.worker
                .take()
                .unwrap()
                .join()
                .unwrap_or(Err(SaveTaskError::WorkerFailed)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_store::{CredentialTask, CredentialTaskError};
    use crate::manual::{Credential, PolicySource, Scheme};
    use std::time::{Duration, Instant};

    fn settings(model: &str) -> Settings {
        Settings::new(Scheme::Https, "example.test", 443, "/v1", model).unwrap()
    }
    fn take(task: &mut SettingsSaveTask) -> Result<SavedSettings, SaveTaskError> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = task.try_take() {
                return result;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn save_worker_shares_slot_and_rejects_stale_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("sumibi-save-task-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(self.0.join("settings.json"));
                let _ = std::fs::remove_file(self.0.join(".settings.json.lock"));
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let path = dir.join("settings.json");
        settings("old").save(&path).unwrap();
        let authority = Arc::new(settings("old").new_authority().unwrap());
        authority
            .replace_credential(Credential::new("fixture".into()))
            .unwrap();
        let version = authority
            .grant_manual_consent(settings("old").destination())
            .unwrap();
        let state = authority.snapshot().unwrap();
        let guard = authority
            .authorize_send_start(
                sumibi_protocol::message::Revision {
                    input: 1,
                    settings: version.settings,
                    authorization: version.authorization,
                },
                &state.destination,
                state.credential.as_ref().unwrap(),
            )
            .unwrap();
        let mut task = SettingsSaveTask::start(
            authority.clone(),
            path.clone(),
            settings("old"),
            settings("new"),
        )
        .unwrap();
        assert!(task.try_take().is_none());
        assert!(matches!(
            CredentialTask::delete(authority.clone()),
            Err(CredentialTaskError::Busy)
        ));
        assert!(matches!(
            SettingsSaveTask::start(
                authority.clone(),
                path.clone(),
                settings("old"),
                settings("other")
            ),
            Err(SaveTaskError::Busy)
        ));
        drop(guard);
        let saved = take(&mut task).unwrap();
        assert_eq!(saved.version().settings, version.settings + 1);
        assert!(saved.belongs_to(&authority));
        assert_eq!(Settings::load(&path).unwrap().model(), "new");
        assert!(task.try_take().is_none());
        let before = authority.wire_policy().unwrap();
        let mut stale = SettingsSaveTask::start(
            authority.clone(),
            path.clone(),
            settings("old"),
            settings("stale"),
        )
        .unwrap();
        assert!(matches!(
            take(&mut stale),
            Err(SaveTaskError::Save(SettingsError::Conflict))
        ));
        assert!(authority.wire_policy().unwrap() == before);
        assert_eq!(Settings::load(&path).unwrap().model(), "new");
    }
}
