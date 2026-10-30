//! User settings without credentials, input text or consent. Call file I/O on
//! a settings worker; the caller chooses the per-user location.
use crate::manual::{Destination, Scheme};
use crate::policy::{ChangeError, PolicyAuthority, PolicyVersion};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_SETTINGS_BYTES: u64 = 16 * 1024;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsError {
    Invalid,
    Io,
    Policy(ChangeError),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    scheme: StoredScheme,
    host: String,
    port: u16,
    chat_path: String,
    model: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum StoredScheme {
    Https,
    HttpLoopback,
}

/// Fully resolved destination; key and send consent are deliberately absent.
pub struct Settings {
    destination: Destination,
    model: String,
}

/// The version must be acknowledged by the matching Broker before the
/// settings UI reports completion; keep it to retry an expired notification.
#[must_use = "publish the version to the matching Broker"]
pub struct SettingsUpdate<'a> {
    authority: &'a PolicyAuthority,
    version: PolicyVersion,
}
impl SettingsUpdate<'_> {
    pub fn version(&self) -> PolicyVersion {
        self.version
    }
    #[cfg(windows)]
    pub(crate) fn belongs_to(&self, authority: &PolicyAuthority) -> bool {
        std::ptr::eq(self.authority, authority)
    }
}
impl Settings {
    pub fn new(
        scheme: Scheme,
        host: &str,
        port: u16,
        base_path: &str,
        model: &str,
    ) -> Result<Self, SettingsError> {
        let destination = Destination::for_chat_completions(scheme, host, port, base_path)
            .ok_or(SettingsError::Invalid)?;
        if !crate::first_conversion::valid_model(model) {
            return Err(SettingsError::Invalid);
        }
        Ok(Self {
            destination,
            model: model.to_owned(),
        })
    }

    pub fn destination(&self) -> &Destination {
        &self.destination
    }
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Restoring settings never restores a key or authorizes network sending.
    pub fn new_authority(&self) -> Result<PolicyAuthority, SettingsError> {
        let authority = PolicyAuthority::new(self.destination.clone());
        authority
            .set_model(&self.model)
            .map_err(SettingsError::Policy)?;
        Ok(authority)
    }

    /// Save and atomically update the matching Host policy. Call off the UI
    /// and TSF threads; Broker publication is a separate required step.
    pub fn save_and_apply<'a>(
        &self,
        path: &Path,
        authority: &'a PolicyAuthority,
    ) -> Result<SettingsUpdate<'a>, SettingsError> {
        let version =
            authority.save_settings(self.destination.clone(), self.model.clone(), || {
                self.save(path)
            })?;
        Ok(SettingsUpdate { authority, version })
    }

    fn record(&self) -> Record {
        Record {
            version: 1,
            scheme: match self.destination.scheme() {
                Scheme::Https => StoredScheme::Https,
                Scheme::HttpLoopback => StoredScheme::HttpLoopback,
            },
            host: self.destination.host().to_owned(),
            port: self.destination.port(),
            chat_path: self.destination.base_path().to_owned(),
            model: self.model.clone(),
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, SettingsError> {
        if bytes.is_empty() || bytes.len() as u64 > MAX_SETTINGS_BYTES {
            return Err(SettingsError::Invalid);
        }
        let record: Record = serde_json::from_slice(bytes).map_err(|_| SettingsError::Invalid)?;
        if record.version != 1 {
            return Err(SettingsError::Invalid);
        }
        let scheme = match record.scheme {
            StoredScheme::Https => Scheme::Https,
            StoredScheme::HttpLoopback => Scheme::HttpLoopback,
        };
        let settings = Self::new(
            scheme,
            &record.host,
            record.port,
            &record.chat_path,
            &record.model,
        )?;
        // A saved record names the complete destination, not a base URL whose
        // meaning might change in a later version.
        if settings.destination.base_path() != record.chat_path {
            return Err(SettingsError::Invalid);
        }
        Ok(settings)
    }

    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        if !path.is_absolute() {
            return Err(SettingsError::Invalid);
        }
        let file = fs::File::open(path).map_err(|_| SettingsError::Io)?;
        let mut bytes = Vec::new();
        file.take(MAX_SETTINGS_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| SettingsError::Io)?;
        Self::parse(&bytes)
    }

    /// Replace from a newly created sibling file after flushing its contents.
    /// The caller serializes writes and supplies a trusted user-settings path.
    /// Atomic replacement does not by itself guarantee power-loss durability.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        if !path.is_absolute() {
            return Err(SettingsError::Invalid);
        }
        let parent = path.parent().ok_or(SettingsError::Invalid)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(SettingsError::Invalid)?;
        let bytes = serde_json::to_vec(&self.record()).map_err(|_| SettingsError::Invalid)?;
        if bytes.len() as u64 > MAX_SETTINGS_BYTES {
            return Err(SettingsError::Invalid);
        }
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), id));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| SettingsError::Io)?;
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }
        let _cleanup = Temp(temp.clone());
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| SettingsError::Io)?;
        drop(file);
        fs::rename(&temp, path).map_err(|_| SettingsError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::PolicySource;

    #[test]
    fn round_trip_restores_no_key_or_consent() {
        let settings = Settings::new(
            Scheme::Https,
            "API.Example.test",
            443,
            "/v1",
            "fixture-model",
        )
        .unwrap();
        let bytes = serde_json::to_vec(&settings.record()).unwrap();
        let restored = Settings::parse(&bytes).unwrap();
        assert_eq!(restored.destination().base_path(), "/v1/chat/completions");
        assert_eq!(restored.destination().host(), "api.example.test");
        assert_eq!(restored.model(), "fixture-model");
        let authority = restored.new_authority().unwrap();
        let snapshot = authority.snapshot().unwrap();
        assert!(
            !snapshot.allowed && snapshot.credential.is_none() && snapshot.manual_consent.is_none()
        );
        assert_eq!(snapshot.model.as_deref(), Some("fixture-model"));
    }

    #[test]
    fn refuses_unknown_duplicate_and_unresolved_fields() {
        for json in [
            r#"{"version":2,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1/chat/completions","model":"x"}"#,
            r#"{"version":1,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1","model":"x"}"#,
            r#"{"version":1,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1/chat/completions","model":"x","key":"secret"}"#,
            r#"{"version":1,"version":1,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1/chat/completions","model":"x"}"#,
            r#"{"version":1,"scheme":"http-loopback","host":"example.test","port":80,"chat_path":"/v1/chat/completions","model":"x"}"#,
        ] {
            assert!(Settings::parse(json.as_bytes()).is_err());
        }
        assert!(Settings::parse(&vec![b' '; MAX_SETTINGS_BYTES as usize + 1]).is_err());
    }

    #[test]
    fn atomic_file_replacement_preserves_complete_record() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        let directory = root.join(format!(
            "sumibi-settings-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_file(self.0.join("settings.json"));
                let _ = fs::remove_dir(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        let path = directory.join("settings.json");
        let first = Settings::new(Scheme::Https, "example.test", 443, "/", "first").unwrap();
        first.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap().model(), "first");
        let second = Settings::new(Scheme::Https, "example.test", 443, "/v1", "second").unwrap();
        let authority = first.new_authority().unwrap();
        authority
            .replace_credential(crate::manual::Credential::new("fixture".into()))
            .unwrap();
        let granted = authority.grant_manual_consent(first.destination()).unwrap();
        let update = second.save_and_apply(&path, &authority).unwrap();
        assert_eq!(update.version().settings, granted.settings + 1);
        assert_eq!(update.version().authorization, granted.authorization);
        assert!(authority.snapshot().unwrap().allowed);
        assert_eq!(Settings::load(&path).unwrap().model(), "second");
        let third = Settings::new(Scheme::Https, "other.test", 443, "/v1", "third").unwrap();
        assert!(third
            .save_and_apply(&directory.join("missing").join("settings.json"), &authority)
            .is_err());
        let unchanged = authority.snapshot().unwrap();
        assert!(unchanged.allowed);
        assert_eq!(unchanged.model.as_deref(), Some("second"));
        let changed = third.save_and_apply(&path, &authority).unwrap();
        assert_eq!(changed.version().authorization, granted.authorization + 1);
        let updated = authority.snapshot().unwrap();
        assert!(!updated.allowed && updated.manual_consent.is_none());
        assert_eq!(updated.model.as_deref(), Some("third"));
        assert_eq!(updated.destination.host(), "other.test");
        assert_eq!(Settings::load(&path).unwrap().model(), "third");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        assert!(Settings::load(Path::new("relative.json")).is_err());
    }
}
