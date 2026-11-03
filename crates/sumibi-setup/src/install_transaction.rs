//! Write-ahead registration policy, independent of the Windows registration APIs.
//!
//! A backend must provide exclusive creation, durable writes, and exact ownership
//! checks. Matching a DLL path or a TSF GUID alone is not ownership evidence.
use serde::{Deserialize, Serialize};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

/// A transaction identity and immutable artifact identity. Paths are metadata;
/// a privileged backend must independently validate placement, ACLs and signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSpec {
    /// Backend policy/version identifier, not a registry path. Each backend
    /// must reject other identifiers on EVERY operation, including recovery.
    pub registration_target: String,
    pub transaction: String,
    pub dll_path: String,
    pub sha256: String,
    pub signer_thumbprint: String,
}
impl InstallSpec {
    pub fn validate(&self) -> Result<()> {
        fn hex(value: &str, length: usize) -> bool {
            value.len() == length && value.bytes().all(|b| b.is_ascii_hexdigit())
        }
        if !hex(&self.transaction, 32)
            || self.registration_target.is_empty()
            || self.registration_target.len() > 128
            || !self
                .registration_target
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || !hex(&self.sha256, 64)
            || !hex(&self.signer_thumbprint, 40)
            || self.dll_path.is_empty()
            || self.dll_path.len() > 32768
            || self.dll_path.chars().any(char::is_control)
        {
            return Err(Error("invalid registration artifact identity".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resource {
    ComServer,
    LanguageProfile,
    KeyboardCategory,
    DisplayAttributeCategory,
}
/// RegisterProfile registers both service and profile. Do not separately create
/// a broad service registration whose cleanup could remove unrelated profiles.
pub const RESOURCES: [Resource; 4] = [
    Resource::ComServer,
    Resource::LanguageProfile,
    Resource::KeyboardCategory,
    Resource::DisplayAttributeCategory,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    Absent,
    Owned,
    /// Exclusive ownership is proved, but an interrupted create left only
    /// part of the expected content. Eligible for cleanup, never for commit.
    OwnedIncomplete,
    /// Includes unknown values/children, another transaction, or partial state
    /// which cannot be proved to belong exclusively to this transaction.
    Conflict,
}

pub trait Backend {
    /// Revalidate immutable files, dependencies and security policy. Must not
    /// load a DLL before its placement/signature checks, or mutate registration.
    fn preflight(&mut self, spec: &InstallSpec) -> Result<()>;
    /// Must reject an incompatible registration_target, and account for
    /// user/machine shadowing and extra profiles/categories.
    fn observe(&mut self, spec: &InstallSpec, resource: Resource) -> Result<Observation>;
    /// Atomically refuse an existing resource. Profile must be disabled by
    /// default. Never activate it, change default IME, or import a trust root.
    /// On partial failure, observe may return OwnedIncomplete only with positive
    /// ownership proof; otherwise Conflict. Successful writes must be durable.
    fn create(&mut self, spec: &InstallSpec, resource: Resource) -> Result<()>;
    /// Recheck exact ownership immediately before deletion. Remove only the
    /// named resource; never use recursive tree deletion or ALLPROFILES.
    /// Successful deletion must be durable before returning.
    fn remove_owned(&mut self, spec: &InstallSpec, resource: Resource) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Installing,
    Installed,
    Removing,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub schema: u32,
    pub spec: InstallSpec,
    pub phase: Phase,
    /// Prefix for which durable create intent exists (not a success counter).
    pub prepared: u8,
    /// Reverse cleanup cursor; absence is allowed after an interrupted delete.
    pub remaining: u8,
}
impl Checkpoint {
    pub fn validate(&self) -> Result<()> {
        self.spec.validate()?;
        let valid = self.schema == 1
            && self.prepared <= RESOURCES.len() as u8
            && self.remaining <= self.prepared
            && match self.phase {
                Phase::Installing => self.remaining == self.prepared,
                Phase::Installed => self.prepared == 4 && self.remaining == 4,
                Phase::Removing => true,
                Phase::Removed => self.remaining == 0,
            };
        if !valid {
            return Err(Error("invalid registration checkpoint".into()));
        }
        Ok(())
    }

    pub fn follows(&self, previous: Option<&Self>) -> Result<()> {
        self.validate()?;
        let valid = match previous {
            None => self.phase == Phase::Installing && self.prepared == 0,
            Some(old) => {
                old.validate()?;
                self.spec == old.spec
                    && match (old.phase, self.phase) {
                        (Phase::Installing, Phase::Installing) => self.prepared == old.prepared + 1,
                        (Phase::Installing, Phase::Installed) => old.prepared == 4,
                        (Phase::Installing | Phase::Installed, Phase::Removing) => {
                            self.prepared == old.prepared && self.remaining == old.remaining
                        }
                        (Phase::Removing, Phase::Removing) => {
                            self.prepared == old.prepared
                                && old.remaining > 0
                                && self.remaining == old.remaining - 1
                        }
                        (Phase::Removing, Phase::Removed) => {
                            self.prepared == old.prepared && old.remaining == 0
                        }
                        _ => false,
                    }
            }
        };
        if !valid {
            return Err(Error("invalid registration journal transition".into()));
        }
        Ok(())
    }
}

/// The store must hold an exclusive process lock from load through the final
/// mutation. Append is write-through; any uncertain write poisons the session.
/// Recovery must reopen and validate the complete history before any mutation.
/// A production caller must use one fixed protected journal/lock per CLSID,
/// never accept an alternative journal path to bypass the registration lock.
pub trait Journal {
    fn latest(&self) -> Result<Option<Checkpoint>>;
    fn append(&mut self, checkpoint: &Checkpoint) -> Result<()>;
}

fn observations(backend: &mut impl Backend, state: &Checkpoint, forward: bool) -> Result<()> {
    // Validate the whole set before any write/delete. An unexpected child at a
    // later step must not cause us to remove its COM dependency first.
    for (index, resource) in RESOURCES.iter().enumerate() {
        let observed = backend.observe(&state.spec, *resource)?;
        let valid = if forward && index < state.prepared as usize {
            observed == Observation::Owned
        } else if !forward && index < state.remaining as usize {
            matches!(
                observed,
                Observation::Absent | Observation::Owned | Observation::OwnedIncomplete
            )
        } else {
            observed == Observation::Absent
        };
        if !valid {
            return Err(Error(format!(
                "registration conflict at {resource:?}; no further changes"
            )));
        }
    }
    Ok(())
}

/// Install from an empty journal. Backend failures trigger reverse cleanup;
/// failed journal writes stop immediately because their durable result is unknown.
pub fn install(
    backend: &mut impl Backend,
    journal: &mut impl Journal,
    spec: InstallSpec,
) -> Result<()> {
    spec.validate()?;
    if journal.latest()?.is_some() {
        return Err(Error(
            "journal already exists; recover or uninstall explicitly".into(),
        ));
    }
    backend.preflight(&spec)?;
    let mut state = Checkpoint {
        schema: 1,
        spec,
        phase: Phase::Installing,
        prepared: 0,
        remaining: 0,
    };
    observations(backend, &state, true)?;
    journal.append(&state)?;
    for resource in RESOURCES {
        if let Err(cause) = observations(backend, &state, true) {
            return rollback_error(backend, journal, cause);
        }
        state.prepared += 1;
        state.remaining = state.prepared;
        journal.append(&state)?; // durable intent BEFORE calling a registration API
        if let Err(cause) = backend.create(&state.spec, resource) {
            return rollback_error(backend, journal, cause);
        }
    }
    if let Err(cause) = observations(backend, &state, true) {
        return rollback_error(backend, journal, cause);
    }
    state.phase = Phase::Installed;
    journal.append(&state)
}

fn rollback_error(
    backend: &mut impl Backend,
    journal: &mut impl Journal,
    cause: Error,
) -> Result<()> {
    match recover(backend, journal) {
        Ok(()) => Err(Error(format!(
            "registration failed: {cause}; rollback complete"
        ))),
        Err(cleanup) => Err(Error(format!(
            "registration failed: {cause}; recovery required: {cleanup}"
        ))),
    }
}

/// Recover an interrupted transaction. A committed installation is checked and
/// preserved. Removal of a committed installation requires explicit uninstall.
pub fn recover(backend: &mut impl Backend, journal: &mut impl Journal) -> Result<()> {
    let state = journal
        .latest()?
        .ok_or_else(|| Error("registration journal is absent".into()))?;
    state.validate()?;
    if state.phase == Phase::Installed {
        return observations(backend, &state, true);
    }
    cleanup(backend, journal, state)
}

pub fn uninstall(backend: &mut impl Backend, journal: &mut impl Journal) -> Result<()> {
    let state = journal
        .latest()?
        .ok_or_else(|| Error("registration journal is absent".into()))?;
    state.validate()?;
    cleanup(backend, journal, state)
}

fn cleanup(
    backend: &mut impl Backend,
    journal: &mut impl Journal,
    mut state: Checkpoint,
) -> Result<()> {
    observations(backend, &state, false)?;
    if state.phase == Phase::Removed {
        return Ok(());
    }
    if state.phase != Phase::Removing {
        state.phase = Phase::Removing;
        journal.append(&state)?;
    }
    while state.remaining > 0 {
        observations(backend, &state, false)?;
        let resource = RESOURCES[state.remaining as usize - 1];
        if matches!(
            backend.observe(&state.spec, resource)?,
            Observation::Owned | Observation::OwnedIncomplete
        ) {
            backend.remove_owned(&state.spec, resource)?;
        }
        if backend.observe(&state.spec, resource)? != Observation::Absent {
            return Err(Error(format!(
                "resource still present after removal: {resource:?}"
            )));
        }
        state.remaining -= 1;
        journal.append(&state)?;
    }
    observations(backend, &state, false)?;
    state.phase = Phase::Removed;
    journal.append(&state)
}

#[cfg(test)]
#[path = "install_transaction_tests.rs"]
mod tests;
