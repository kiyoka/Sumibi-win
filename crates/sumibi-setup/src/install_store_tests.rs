use super::*;
use crate::install_transaction::{
    self as tx, Backend, Checkpoint, InstallSpec, Observation, Resource, RESOURCES,
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn root() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "sumibi-registration-store-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn first() -> Checkpoint {
    Checkpoint {
        schema: 1,
        spec: InstallSpec {
            registration_target: "sumibi-registration-store-fixture-v1".into(),
            transaction: "a".repeat(32),
            dll_path: "fixture.dll".into(),
            sha256: "1".repeat(64),
            signer_thumbprint: "2".repeat(40),
        },
        phase: Phase::Installing,
        prepared: 0,
        remaining: 0,
    }
}

fn complete_removal_for(journal: &mut impl Journal, transaction: &str) {
    let mut state = first();
    state.spec.transaction = transaction.into();
    journal.append(&state).unwrap();
    state.phase = Phase::Removing;
    journal.append(&state).unwrap();
    state.phase = Phase::Removed;
    journal.append(&state).unwrap();
}

fn complete_removal(journal: &mut impl Journal) {
    complete_removal_for(journal, &"a".repeat(32));
}

fn write_archive(path: &Path, transaction: &str) {
    let archive = path.join(format!("removed-{transaction}.bin"));
    let mut journal = FileJournal::create(&archive).unwrap();
    complete_removal_for(&mut journal, transaction);
}

#[test]
fn fixed_lock_prevents_two_sessions_and_rotates_only_removed_history() {
    let path = root();
    let store = RegistrationStoreLock::acquire_at_verified_directory(&path).unwrap();
    assert!(RegistrationStoreLock::acquire_at_verified_directory(&path).is_err());
    assert!(store.open_current().unwrap().is_none());
    let mut journal = store.begin_new().unwrap();
    journal.append(&first()).unwrap();
    assert!(store.begin_new().is_err());
    drop(journal);
    assert!(store.begin_new().is_err());
    let mut journal = store.open_current().unwrap().unwrap();
    let mut state = first();
    state.phase = Phase::Removing;
    journal.append(&state).unwrap();
    state.phase = Phase::Removed;
    journal.append(&state).unwrap();
    drop(journal);
    let journal = store.begin_new().unwrap();
    assert!(journal.latest().unwrap().is_none());
    assert!(path
        .join(format!("removed-{}.bin", "a".repeat(32)))
        .exists());
    drop(journal);
    assert!(store.begin_new().unwrap().latest().unwrap().is_none());
    drop(store);
    fs::remove_file(path.join(CURRENT)).unwrap();
    fs::remove_file(path.join(format!("removed-{}.bin", "a".repeat(32)))).unwrap();
    fs::remove_file(path.join(LOCK)).unwrap();
    fs::remove_dir(path).unwrap();
}

#[test]
fn interrupted_rotation_and_empty_current_can_restart_without_losing_archive() {
    let path = root();
    let store = RegistrationStoreLock::acquire_at_verified_directory(&path).unwrap();
    let mut journal = store.begin_new().unwrap();
    complete_removal(&mut journal);
    drop(journal);
    let archive = path.join(format!("removed-{}.bin", "a".repeat(32)));
    fs::rename(path.join(CURRENT), &archive).unwrap();
    let journal = store.begin_new().unwrap();
    assert!(journal.latest().unwrap().is_none());
    drop(journal);
    let journal = store.begin_new().unwrap();
    assert!(journal.latest().unwrap().is_none());
    drop(journal);
    assert_eq!(
        FileJournal::open(&archive)
            .unwrap()
            .latest()
            .unwrap()
            .unwrap()
            .phase,
        Phase::Removed
    );
    drop(store);
    fs::remove_file(path.join(CURRENT)).unwrap();
    fs::remove_file(archive).unwrap();
    fs::remove_file(path.join(LOCK)).unwrap();
    fs::remove_dir(path).unwrap();
}

#[test]
fn unexpected_or_corrupt_files_are_preserved_and_never_replaced() {
    let path = root();
    let store = RegistrationStoreLock::acquire_at_verified_directory(&path).unwrap();
    let unexpected = path.join("other.bin");
    fs::write(&unexpected, b"keep").unwrap();
    assert!(store.begin_new().is_err());
    assert_eq!(fs::read(&unexpected).unwrap(), b"keep");
    fs::remove_file(&unexpected).unwrap();
    let current = path.join(CURRENT);
    fs::write(&current, b"corrupt").unwrap();
    assert!(store.begin_new().is_err());
    assert_eq!(fs::read(&current).unwrap(), b"corrupt");
    drop(store);
    fs::remove_file(current).unwrap();
    fs::remove_file(path.join(LOCK)).unwrap();
    fs::remove_dir(path).unwrap();
}

#[test]
fn archive_capacity_and_same_transaction_collision_stop_new_install() {
    let path = root();
    let store = RegistrationStoreLock::acquire_at_verified_directory(&path).unwrap();
    let mut journal = store.begin_new().unwrap();
    complete_removal(&mut journal);
    drop(journal);
    let collision = path.join(format!("removed-{}.bin", "a".repeat(32)));
    write_archive(&path, &"a".repeat(32));
    let collision_bytes = fs::read(&collision).unwrap();
    assert!(store.begin_new().is_err());
    assert_eq!(fs::read(&collision).unwrap(), collision_bytes);
    assert_eq!(
        store
            .open_current()
            .unwrap()
            .unwrap()
            .latest()
            .unwrap()
            .unwrap()
            .phase,
        Phase::Removed
    );
    fs::remove_file(&collision).unwrap();
    for id in 0..MAX_ARCHIVES {
        write_archive(&path, &format!("{id:032x}"));
    }
    assert!(store.begin_new().is_err());
    assert!(path.join(CURRENT).exists());
    drop(store);
    for id in 0..MAX_ARCHIVES {
        fs::remove_file(path.join(format!("removed-{id:032x}.bin"))).unwrap();
    }
    fs::remove_file(path.join(CURRENT)).unwrap();
    fs::remove_file(path.join(LOCK)).unwrap();
    fs::remove_dir(path).unwrap();
}

#[test]
fn incomplete_misnamed_or_active_archive_blocks_rotation_without_mutation() {
    let path = root();
    let store = RegistrationStoreLock::acquire_at_verified_directory(&path).unwrap();
    let mut journal = store.begin_new().unwrap();
    complete_removal(&mut journal);
    drop(journal);
    let current = path.join(CURRENT);
    let current_bytes = fs::read(&current).unwrap();
    let archive = path.join(format!("removed-{}.bin", "a".repeat(32)));
    write_archive(&path, &"a".repeat(32));
    {
        let mut file = OpenOptions::new().append(true).open(&archive).unwrap();
        use std::io::Write;
        file.write_all(b"SMBI").unwrap();
    }
    let incomplete = fs::read(&archive).unwrap();
    assert!(store.begin_new().is_err());
    assert_eq!(fs::read(&archive).unwrap(), incomplete);
    assert_eq!(fs::read(&current).unwrap(), current_bytes);
    fs::write(&archive, &incomplete[..incomplete.len() - 4]).unwrap();
    let misnamed = path.join(format!("removed-{}.bin", "b".repeat(32)));
    fs::rename(&archive, &misnamed).unwrap();
    let bytes = fs::read(&misnamed).unwrap();
    assert!(store.begin_new().is_err());
    assert_eq!(fs::read(&misnamed).unwrap(), bytes);
    assert_eq!(fs::read(&current).unwrap(), current_bytes);
    fs::remove_file(&misnamed).unwrap();
    let mut active = FileJournal::create(&archive).unwrap();
    active.append(&first()).unwrap();
    drop(active);
    let active_bytes = fs::read(&archive).unwrap();
    assert!(store.begin_new().is_err());
    assert_eq!(fs::read(&archive).unwrap(), active_bytes);
    assert_eq!(fs::read(&current).unwrap(), current_bytes);
    drop(store);
    fs::remove_file(archive).unwrap();
    fs::remove_file(current).unwrap();
    fs::remove_file(path.join(LOCK)).unwrap();
    fs::remove_dir(path).unwrap();
}

#[derive(Default)]
struct TransactionModel {
    owned: [bool; 4],
    removed: Vec<Resource>,
}

impl Backend for TransactionModel {
    fn preflight(&mut self, _: &InstallSpec) -> Result<()> {
        Ok(())
    }

    fn observe(&mut self, _: &InstallSpec, resource: Resource) -> Result<Observation> {
        let index = RESOURCES.iter().position(|item| *item == resource).unwrap();
        Ok(if self.owned[index] {
            Observation::Owned
        } else {
            Observation::Absent
        })
    }

    fn create(&mut self, _: &InstallSpec, resource: Resource) -> Result<()> {
        let index = RESOURCES.iter().position(|item| *item == resource).unwrap();
        assert!(!self.owned[index]);
        self.owned[index] = true;
        Ok(())
    }

    fn remove_owned(&mut self, _: &InstallSpec, resource: Resource) -> Result<()> {
        let index = RESOURCES.iter().position(|item| *item == resource).unwrap();
        assert!(self.owned[index]);
        self.owned[index] = false;
        self.removed.push(resource);
        Ok(())
    }
}

#[test]
fn transaction_survives_store_reopen_then_uninstall_rotates_complete_history() {
    let path = root();
    let mut backend = TransactionModel::default();
    {
        let store = RegistrationStoreLock::acquire_at_verified_directory(&path).unwrap();
        let mut journal = store.begin_new().unwrap();
        tx::install(&mut backend, &mut journal, first().spec).unwrap();
        assert_eq!(journal.latest().unwrap().unwrap().phase, Phase::Installed);
        assert!(store.begin_new().is_err());
    }
    {
        let store = RegistrationStoreLock::acquire_at_verified_directory(&path).unwrap();
        let mut journal = store.open_current().unwrap().unwrap();
        tx::recover(&mut backend, &mut journal).unwrap();
        assert_eq!(backend.owned, [true; 4]);
        tx::uninstall(&mut backend, &mut journal).unwrap();
        assert_eq!(
            backend.removed,
            RESOURCES.into_iter().rev().collect::<Vec<_>>()
        );
        assert_eq!(journal.latest().unwrap().unwrap().phase, Phase::Removed);
    }
    {
        let store = RegistrationStoreLock::acquire_at_verified_directory(&path).unwrap();
        let journal = store.begin_new().unwrap();
        assert!(journal.latest().unwrap().is_none());
    }
    let archive = path.join(format!("removed-{}.bin", "a".repeat(32)));
    assert_eq!(
        FileJournal::inspect_complete(&archive).unwrap().phase,
        Phase::Removed
    );
    fs::remove_file(path.join(CURRENT)).unwrap();
    fs::remove_file(archive).unwrap();
    fs::remove_file(path.join(LOCK)).unwrap();
    fs::remove_dir(path).unwrap();
}
