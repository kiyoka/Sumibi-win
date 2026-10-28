use super::*;
use crate::install_transaction::{Checkpoint, InstallSpec};
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

fn complete_removal(journal: &mut impl Journal) {
    let mut state = first();
    journal.append(&state).unwrap();
    state.phase = Phase::Removing;
    journal.append(&state).unwrap();
    state.phase = Phase::Removed;
    journal.append(&state).unwrap();
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
    fs::write(&collision, b"keep").unwrap();
    assert!(store.begin_new().is_err());
    assert_eq!(fs::read(&collision).unwrap(), b"keep");
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
        fs::write(path.join(format!("removed-{id:032x}.bin")), b"").unwrap();
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
