//! User settings without credentials, input text or consent. Call file I/O on
//! a settings worker; the caller chooses the per-user location.
use crate::manual::{Destination, Scheme};
use crate::policy::{ChangeError, PolicyAuthority, PolicyVersion};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
pub use sumibi_protocol::ProtectionProfile;

const MAX_SETTINGS_BYTES: u64 = 16 * 1024;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsError {
    Invalid,
    Io,
    AlreadyExists,
    Busy,
    Conflict,
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
    #[serde(default)]
    protection_profile: ProtectionProfile,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum StoredScheme {
    Https,
    HttpLoopback,
}

/// Fully resolved destination; key and send consent are deliberately absent.
#[derive(Clone)]
pub struct Settings {
    destination: Destination,
    model: String,
    protection_profile: ProtectionProfile,
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
            protection_profile: ProtectionProfile::Standard,
        })
    }

    pub fn with_protection_profile(mut self, profile: ProtectionProfile) -> Self {
        self.protection_profile = profile;
        self
    }

    pub fn set_protection_profile(&mut self, profile: ProtectionProfile) {
        self.protection_profile = profile;
    }

    pub fn protection_profile(&self) -> ProtectionProfile {
        self.protection_profile
    }

    pub fn destination(&self) -> &Destination {
        &self.destination
    }
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Restoring settings never restores a key or authorizes network sending.
    pub fn new_authority(&self) -> Result<PolicyAuthority, SettingsError> {
        let authority =
            PolicyAuthority::new_with_profile(self.destination.clone(), self.protection_profile);
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
        let version = authority.save_settings(
            self.destination.clone(),
            self.model.clone(),
            self.protection_profile,
            || self.save(path),
        )?;
        Ok(SettingsUpdate { authority, version })
    }

    /// First-run creation only. An existing valid or invalid file is never
    /// replaced, and neither a key nor sending consent is created here.
    pub(crate) fn create_and_apply(
        &self,
        path: &Path,
        authority: &PolicyAuthority,
    ) -> Result<PolicyVersion, SettingsError> {
        let parent = path.parent().ok_or(SettingsError::Invalid)?;
        if !path.is_absolute() {
            return Err(SettingsError::Invalid);
        }
        match fs::create_dir(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && parent.is_dir() => {}
            Err(_) => return Err(SettingsError::Io),
        }
        authority.save_settings(
            self.destination.clone(),
            self.model.clone(),
            self.protection_profile,
            || self.create(path),
        )
    }

    /// Settings-editor save with a loaded baseline. Conflicts leave the live
    /// authority unchanged and require the caller to reload before retrying.
    pub fn save_and_apply_if_unchanged<'a>(
        &self,
        path: &Path,
        expected: &Settings,
        authority: &'a PolicyAuthority,
    ) -> Result<SettingsUpdate<'a>, SettingsError> {
        let version = authority.save_settings(
            self.destination.clone(),
            self.model.clone(),
            self.protection_profile,
            || self.save_if_unchanged(path, expected),
        )?;
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
            protection_profile: self.protection_profile,
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
        let mut settings = Self::new(
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
        settings.protection_profile = record.protection_profile;
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

    /// Initial creation only: never replace an existing file, including an
    /// invalid one. An interrupted initial write may leave an invalid record;
    /// load rejects it and the caller must explicitly repair that file.
    pub fn create(&self, path: &Path) -> Result<(), SettingsError> {
        let _lock = write_lock(path)?;
        if !path.is_absolute() {
            return Err(SettingsError::Invalid);
        }
        let bytes = serde_json::to_vec(&self.record()).map_err(|_| SettingsError::Invalid)?;
        if bytes.len() as u64 > MAX_SETTINGS_BYTES {
            return Err(SettingsError::Invalid);
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Allow readers but prevent concurrent writers and deletion while
            // the new file is being initialized.
            options.share_mode(1);
        }
        let mut file = options.open(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                SettingsError::AlreadyExists
            } else {
                SettingsError::Io
            }
        })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| SettingsError::Io)
    }

    /// Unconditionally replace from a newly created sibling file after
    /// flushing its contents. All library writers acquire the fixed sidecar;
    /// editors should use `save_if_unchanged` to reject stale baselines.
    /// The caller supplies a trusted user-settings path.
    /// Atomic replacement does not by itself guarantee power-loss durability.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let _lock = write_lock(path)?;
        self.save_locked(path)
    }

    /// Refuse a stale editor's replacement while another writer is active or
    /// the saved settings have changed since the editor loaded them.
    pub fn save_if_unchanged(&self, path: &Path, expected: &Settings) -> Result<(), SettingsError> {
        let _lock = write_lock(path)?;
        let current = Self::load(path)?;
        if current.destination != expected.destination || current.model != expected.model {
            return Err(SettingsError::Conflict);
        }
        self.save_locked(path)
    }

    fn save_locked(&self, path: &Path) -> Result<(), SettingsError> {
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

// Retain this empty sidecar across file replacements. Never unlink it on
// release: another process may already have it open waiting to acquire it.
fn write_lock(path: &Path) -> Result<fs::File, SettingsError> {
    if !path.is_absolute() {
        return Err(SettingsError::Invalid);
    }
    let parent = path.parent().ok_or(SettingsError::Invalid)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(SettingsError::Invalid)?;
    let lock_path = parent.join(format!(".{name}.lock"));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(3).custom_flags(0x0020_0000); // no delete sharing; open reparse point itself
    }
    let file = options.open(lock_path).map_err(|_| SettingsError::Io)?;
    let metadata = file.metadata().map_err(|_| SettingsError::Io)?;
    if !metadata.is_file() || metadata.len() != 0 {
        return Err(SettingsError::Invalid);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(SettingsError::Invalid);
        }
    }
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => SettingsError::Busy,
        std::fs::TryLockError::Error(_) => SettingsError::Io,
    })?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::PolicySource;

    #[test]
    fn settings_writer_child_fixture() {
        let Some(path) = std::env::var_os("SUMIBI_SETTINGS_LOCK_TEST_PATH") else {
            return;
        };
        let next = Settings::new(Scheme::Https, "example.test", 443, "/v1", "child-model").unwrap();
        let result = next.save(Path::new(&path));
        if std::env::var_os("SUMIBI_SETTINGS_LOCK_TEST_BUSY").is_some() {
            assert_eq!(result, Err(SettingsError::Busy));
        } else {
            assert_eq!(result, Ok(()));
        }
    }

    #[test]
    fn other_process_is_busy_and_stale_save_preserves_disk_and_policy() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!(
                "sumibi-settings-lock-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir(&directory).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_file(self.0.join("settings.json"));
                let _ = fs::remove_file(self.0.join(".settings.json.lock"));
                let _ = fs::remove_dir(&self.0);
            }
        }
        let _cleanup = Cleanup(directory.clone());
        let path = directory.join("settings.json");
        let original =
            Settings::new(Scheme::Https, "example.test", 443, "/v1", "original-model").unwrap();
        original.save(&path).unwrap();
        let child = |busy: bool| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "settings::tests::settings_writer_child_fixture"])
                .env("SUMIBI_SETTINGS_LOCK_TEST_PATH", &path);
            if busy {
                command.env("SUMIBI_SETTINGS_LOCK_TEST_BUSY", "1");
            } else {
                command.env_remove("SUMIBI_SETTINGS_LOCK_TEST_BUSY");
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "settings child fixture failed");
        };
        let lock = write_lock(&path).unwrap();
        child(true);
        assert_eq!(Settings::load(&path).unwrap().model(), "original-model");
        drop(lock);
        child(false);
        assert_eq!(Settings::load(&path).unwrap().model(), "child-model");
        let authority = original.new_authority().unwrap();
        let before = authority.wire_policy().unwrap();
        let stale =
            Settings::new(Scheme::Https, "example.test", 443, "/v1", "stale-model").unwrap();
        assert!(matches!(
            stale.save_and_apply_if_unchanged(&path, &original, &authority),
            Err(SettingsError::Conflict)
        ));
        assert!(authority.wire_policy().unwrap() == before);
        assert_eq!(Settings::load(&path).unwrap().model(), "child-model");
        let baseline = Settings::load(&path).unwrap();
        let _update = stale
            .save_and_apply_if_unchanged(&path, &baseline, &authority)
            .unwrap();
        assert_eq!(Settings::load(&path).unwrap().model(), "stale-model");
        let lock_path = directory.join(".settings.json.lock");
        fs::write(&lock_path, b"unexpected").unwrap();
        assert_eq!(original.save(&path), Err(SettingsError::Invalid));
        assert_eq!(fs::read(&lock_path).unwrap(), b"unexpected");
        assert_eq!(Settings::load(&path).unwrap().model(), "stale-model");
        println!("cross-process settings lock and stale editor rejection completed");
    }

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
        assert_eq!(snapshot.protection_profile, ProtectionProfile::Standard);
        assert_eq!(
            authority.wire_policy().unwrap().protection_profile,
            ProtectionProfile::Standard
        );
    }

    #[test]
    fn refuses_unknown_duplicate_and_unresolved_fields() {
        for json in [
            r#"{"version":2,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1/chat/completions","model":"x"}"#,
            r#"{"version":1,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1","model":"x"}"#,
            r#"{"version":1,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1/chat/completions","model":"x","key":"secret"}"#,
            r#"{"version":1,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1/chat/completions","model":"x","protection_profile":"invalid"}"#,
            r#"{"version":1,"version":1,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1/chat/completions","model":"x"}"#,
            r#"{"version":1,"scheme":"http-loopback","host":"example.test","port":80,"chat_path":"/v1/chat/completions","model":"x"}"#,
        ] {
            assert!(Settings::parse(json.as_bytes()).is_err());
        }
        assert!(Settings::parse(&vec![b' '; MAX_SETTINGS_BYTES as usize + 1]).is_err());
    }

    #[test]
    fn settings_protection_profile_default_and_explicit_options() {
        // 1. Missing protection_profile defaults to Standard (backward compatibility)
        let json_omitted = br#"{"version":1,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1/chat/completions","model":"gpt-4o"}"#;
        let parsed_default = Settings::parse(json_omitted).unwrap();
        assert_eq!(
            parsed_default.protection_profile(),
            ProtectionProfile::Standard
        );

        // 2. Explicit standard profile
        let json_standard = br#"{"version":1,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1/chat/completions","model":"gpt-4o","protection_profile":"standard"}"#;
        let parsed_standard = Settings::parse(json_standard).unwrap();
        assert_eq!(
            parsed_standard.protection_profile(),
            ProtectionProfile::Standard
        );

        // 3. Explicit strict profile
        let json_strict = br#"{"version":1,"scheme":"https","host":"example.test","port":443,"chat_path":"/v1/chat/completions","model":"gpt-4o","protection_profile":"strict"}"#;
        let parsed_strict = Settings::parse(json_strict).unwrap();
        assert_eq!(
            parsed_strict.protection_profile(),
            ProtectionProfile::Strict
        );

        // 4. Roundtrip serialization preserves strict profile
        let bytes = serde_json::to_vec(&parsed_strict.record()).unwrap();
        let restored = Settings::parse(&bytes).unwrap();
        assert_eq!(restored.protection_profile(), ProtectionProfile::Strict);

        // 5. PolicyAuthority reflects profile and supports dynamic update
        let authority = restored.new_authority().unwrap();
        let initial_policy = authority.wire_policy().unwrap();
        assert_eq!(initial_policy.protection_profile, ProtectionProfile::Strict);
        let initial_settings_version = initial_policy.settings;

        // Switch dynamically to standard
        let updated_version = authority
            .set_protection_profile(ProtectionProfile::Standard)
            .unwrap();
        assert_eq!(updated_version.settings, initial_settings_version + 1);
        let updated_policy = authority.wire_policy().unwrap();
        assert_eq!(
            updated_policy.protection_profile,
            ProtectionProfile::Standard
        );

        // Setting same profile does not advance settings version
        let noop_version = authority
            .set_protection_profile(ProtectionProfile::Standard)
            .unwrap();
        assert_eq!(noop_version.settings, updated_version.settings);
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
                let _ = fs::remove_file(self.0.join(".settings.json.lock"));
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
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        assert!(Settings::load(Path::new("relative.json")).is_err());
    }
}
