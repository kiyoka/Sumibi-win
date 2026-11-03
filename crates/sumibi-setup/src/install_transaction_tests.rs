use super::*;

pub(super) fn spec() -> InstallSpec {
    InstallSpec {
        registration_target: "sumibi-registration-fixture-x64-v1".into(),
        transaction: "1".repeat(32),
        dll_path: r"D:\Program Files\Sumibi\dev-test\sumibi_tip.dll".into(),
        sha256: "2".repeat(64),
        signer_thumbprint: "3".repeat(40),
    }
}

#[derive(Default, Clone)]
struct MemoryJournal {
    states: Vec<Checkpoint>,
    fail_append: Option<(usize, bool)>,
    poisoned: bool,
}
impl Journal for MemoryJournal {
    fn latest(&self) -> Result<Option<Checkpoint>> {
        if self.poisoned {
            return Err(Error("poisoned store".into()));
        }
        Ok(self.states.last().cloned())
    }
    fn append(&mut self, state: &Checkpoint) -> Result<()> {
        self.latest()?;
        state.follows(self.states.last())?;
        if self.fail_append == Some((self.states.len(), false)) {
            self.poisoned = true;
            return Err(Error("injected pre-write failure".into()));
        }
        self.states.push(state.clone());
        if self.fail_append == Some((self.states.len() - 1, true)) {
            self.poisoned = true;
            return Err(Error("injected sync failure".into()));
        }
        Ok(())
    }
}
impl MemoryJournal {
    fn reopen(&mut self) {
        self.poisoned = false;
        self.fail_append = None;
    }
}

struct Model {
    entries: [Observation; 4],
    creates: Vec<Resource>,
    deletes: Vec<Resource>,
    fail_create: Option<(Resource, bool)>,
    fail_delete: Option<(Resource, bool)>,
    preflight_denied: bool,
    incomplete_success: Option<Resource>,
    interfere_after: Option<(Resource, Resource)>,
}
impl Default for Model {
    fn default() -> Self {
        Self {
            entries: [Observation::Absent; 4],
            creates: vec![],
            deletes: vec![],
            fail_create: None,
            fail_delete: None,
            preflight_denied: false,
            incomplete_success: None,
            interfere_after: None,
        }
    }
}
fn index(resource: Resource) -> usize {
    RESOURCES.iter().position(|r| *r == resource).unwrap()
}
impl Backend for Model {
    fn preflight(&mut self, _: &InstallSpec) -> Result<()> {
        if self.preflight_denied {
            Err(Error("policy denied".into()))
        } else {
            Ok(())
        }
    }
    fn observe(&mut self, _: &InstallSpec, resource: Resource) -> Result<Observation> {
        Ok(self.entries[index(resource)])
    }
    fn create(&mut self, _: &InstallSpec, resource: Resource) -> Result<()> {
        assert_eq!(self.entries[index(resource)], Observation::Absent);
        if self.fail_create == Some((resource, false)) {
            return Err(Error("create failed".into()));
        }
        self.entries[index(resource)] = Observation::Owned;
        if self.incomplete_success == Some(resource) {
            self.entries[index(resource)] = Observation::OwnedIncomplete;
        }
        if let Some((after, victim)) = self.interfere_after {
            if after == resource {
                self.entries[index(victim)] = Observation::Conflict;
            }
        }
        self.creates.push(resource);
        if self.fail_create == Some((resource, true)) {
            return Err(Error("partial create failed".into()));
        }
        Ok(())
    }
    fn remove_owned(&mut self, _: &InstallSpec, resource: Resource) -> Result<()> {
        assert!(matches!(
            self.entries[index(resource)],
            Observation::Owned | Observation::OwnedIncomplete
        ));
        if self.fail_delete == Some((resource, false)) {
            return Err(Error("delete failed".into()));
        }
        self.entries[index(resource)] = Observation::Absent;
        self.deletes.push(resource);
        if self.fail_delete == Some((resource, true)) {
            return Err(Error("delete completed but API failed".into()));
        }
        Ok(())
    }
}

#[test]
fn installed_recovery_preserves_registration_and_uninstall_is_idempotent() {
    let mut backend = Model::default();
    let mut journal = MemoryJournal::default();
    install(&mut backend, &mut journal, spec()).unwrap();
    assert_eq!(journal.latest().unwrap().unwrap().phase, Phase::Installed);
    recover(&mut backend, &mut journal).unwrap();
    assert!(backend.deletes.is_empty());
    uninstall(&mut backend, &mut journal).unwrap();
    assert_eq!(
        backend.deletes,
        RESOURCES.into_iter().rev().collect::<Vec<_>>()
    );
    let records = journal.states.len();
    recover(&mut backend, &mut journal).unwrap();
    uninstall(&mut backend, &mut journal).unwrap();
    assert_eq!(journal.states.len(), records);
    assert_eq!(backend.entries, [Observation::Absent; 4]);
}

#[test]
fn every_create_failure_before_or_after_side_effect_rolls_back() {
    for resource in RESOURCES {
        for after in [false, true] {
            let mut backend = Model {
                fail_create: Some((resource, after)),
                ..Default::default()
            };
            let mut journal = MemoryJournal::default();
            let error = install(&mut backend, &mut journal, spec()).unwrap_err();
            assert!(
                error.0.contains("rollback complete"),
                "{resource:?}/{after}: {error}"
            );
            assert_eq!(backend.entries, [Observation::Absent; 4]);
            assert_eq!(journal.latest().unwrap().unwrap().phase, Phase::Removed);
        }
    }
}

#[test]
fn every_install_journal_failure_stops_mutation_until_reopen() {
    // initial + 4 intents + Installed commit, including an uncertain commit.
    for step in 0..6 {
        for persisted in [false, true] {
            let mut backend = Model::default();
            let mut journal = MemoryJournal {
                fail_append: Some((step, persisted)),
                ..Default::default()
            };
            install(&mut backend, &mut journal, spec()).unwrap_err();
            assert_eq!(backend.creates.len(), step.saturating_sub(1).min(4));
            assert!(backend.deletes.is_empty());
            recover(&mut backend, &mut journal).unwrap_err();
            journal.reopen();
            if let Some(state) = journal.latest().unwrap() {
                recover(&mut backend, &mut journal).unwrap();
                if state.phase == Phase::Installed {
                    assert_eq!(backend.entries, [Observation::Owned; 4]);
                    uninstall(&mut backend, &mut journal).unwrap();
                }
            }
            assert_eq!(backend.entries, [Observation::Absent; 4]);
        }
    }
}

#[test]
fn every_delete_failure_can_resume_even_after_side_effect() {
    for resource in RESOURCES {
        for after in [false, true] {
            let mut backend = Model::default();
            let mut journal = MemoryJournal::default();
            install(&mut backend, &mut journal, spec()).unwrap();
            backend.fail_delete = Some((resource, after));
            uninstall(&mut backend, &mut journal).unwrap_err();
            backend.fail_delete = None;
            recover(&mut backend, &mut journal).unwrap();
            assert_eq!(backend.entries, [Observation::Absent; 4]);
            assert_eq!(backend.deletes.len(), 4);
        }
    }
}

#[test]
fn every_cleanup_journal_failure_can_resume_without_repeated_deletion() {
    // Removing, each of 4 cursor advances, Removed.
    for step in 6..12 {
        for persisted in [false, true] {
            let mut backend = Model::default();
            let mut journal = MemoryJournal::default();
            install(&mut backend, &mut journal, spec()).unwrap();
            journal.fail_append = Some((step, persisted));
            uninstall(&mut backend, &mut journal).unwrap_err();
            let deletes = backend.deletes.len();
            recover(&mut backend, &mut journal).unwrap_err();
            assert_eq!(backend.deletes.len(), deletes);
            journal.reopen();
            if journal.latest().unwrap().unwrap().phase == Phase::Installed {
                uninstall(&mut backend, &mut journal).unwrap();
            } else {
                recover(&mut backend, &mut journal).unwrap();
            }
            assert_eq!(backend.entries, [Observation::Absent; 4]);
            assert_eq!(backend.deletes.len(), 4);
        }
    }
}

#[test]
fn conflict_anywhere_prevents_all_cleanup_not_just_conflicting_resource() {
    for resource in RESOURCES {
        let mut backend = Model::default();
        let mut journal = MemoryJournal::default();
        install(&mut backend, &mut journal, spec()).unwrap();
        backend.entries[index(resource)] = Observation::Conflict;
        let before = journal.states.len();
        uninstall(&mut backend, &mut journal).unwrap_err();
        assert!(backend.deletes.is_empty());
        assert_eq!(journal.states.len(), before);
    }
}

#[test]
fn preexisting_resource_or_preflight_failure_does_not_create_journal() {
    for resource in RESOURCES {
        let mut backend = Model::default();
        backend.entries[index(resource)] = Observation::Conflict;
        let mut journal = MemoryJournal::default();
        install(&mut backend, &mut journal, spec()).unwrap_err();
        assert!(backend.creates.is_empty());
        assert!(journal.states.is_empty());
    }
    let mut backend = Model {
        preflight_denied: true,
        ..Default::default()
    };
    let mut journal = MemoryJournal::default();
    install(&mut backend, &mut journal, spec()).unwrap_err();
    assert!(journal.states.is_empty());
}

#[test]
fn recovery_does_not_touch_resources_without_durable_intent() {
    let mut backend = Model::default();
    let mut journal = MemoryJournal {
        fail_append: Some((2, false)),
        ..Default::default()
    };
    install(&mut backend, &mut journal, spec()).unwrap_err();
    journal.reopen();
    backend.entries[3] = Observation::Owned;
    recover(&mut backend, &mut journal).unwrap_err();
    assert!(backend.deletes.is_empty());
    backend.entries[3] = Observation::Absent;
    recover(&mut backend, &mut journal).unwrap();
    assert_eq!(backend.deletes, [Resource::ComServer]);
}

#[test]
fn invalid_spec_is_rejected_before_preflight_or_mutation() {
    for field in 0..5 {
        let mut spec = spec();
        match field {
            0 => spec.transaction = "../bad".into(),
            1 => spec.dll_path.push('\0'),
            2 => spec.sha256 = "z".repeat(64),
            3 => spec.signer_thumbprint.clear(),
            _ => spec.registration_target = "../another-target".into(),
        }
        let mut backend = Model::default();
        let mut journal = MemoryJournal::default();
        install(&mut backend, &mut journal, spec).unwrap_err();
        assert!(journal.states.is_empty());
    }
}

#[test]
fn existing_journal_cannot_be_repurposed_for_another_install() {
    let mut backend = Model::default();
    let mut journal = MemoryJournal::default();
    install(&mut backend, &mut journal, spec()).unwrap();
    uninstall(&mut backend, &mut journal).unwrap();
    install(&mut backend, &mut journal, spec()).unwrap_err();
    assert_eq!(backend.creates.len(), 4);
}

#[test]
fn incomplete_owned_content_is_cleanup_eligible_but_cannot_commit() {
    for resource in RESOURCES {
        let mut backend = Model {
            incomplete_success: Some(resource),
            ..Default::default()
        };
        let mut journal = MemoryJournal::default();
        let error = install(&mut backend, &mut journal, spec()).unwrap_err();
        assert!(error.0.contains("rollback complete"));
        assert_eq!(journal.latest().unwrap().unwrap().phase, Phase::Removed);
        assert_eq!(backend.entries, [Observation::Absent; 4]);
    }
}

#[test]
fn late_conflict_preserves_created_resources_until_external_state_is_resolved() {
    let mut backend = Model {
        interfere_after: Some((Resource::LanguageProfile, Resource::KeyboardCategory)),
        ..Default::default()
    };
    let mut journal = MemoryJournal::default();
    let error = install(&mut backend, &mut journal, spec()).unwrap_err();
    assert!(error.0.contains("recovery required"));
    assert_eq!(
        backend.creates,
        [Resource::ComServer, Resource::LanguageProfile]
    );
    assert!(backend.deletes.is_empty());
    backend.entries[2] = Observation::Absent;
    recover(&mut backend, &mut journal).unwrap();
    assert_eq!(
        backend.deletes,
        [Resource::LanguageProfile, Resource::ComServer]
    );
}
