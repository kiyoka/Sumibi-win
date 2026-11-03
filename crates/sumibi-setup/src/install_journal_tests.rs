use super::*;
use crate::install_transaction::{InstallSpec, Phase};

fn initial() -> Checkpoint {
    Checkpoint {
        schema: 1,
        spec: InstallSpec {
            registration_target: "sumibi-registration-fixture-x64-v1".into(),
            transaction: "1".repeat(32),
            dll_path: "fixture.dll".into(),
            sha256: "2".repeat(64),
            signer_thumbprint: "3".repeat(40),
        },
        phase: Phase::Installing,
        prepared: 0,
        remaining: 0,
    }
}

#[test]
fn checksum_matches_standard_vector() {
    assert_eq!(checksum(b"123456789"), 0xcbf4_3926);
}

#[test]
fn every_incomplete_final_frame_keeps_previous_durable_intent() {
    let first = initial();
    let mut second = first.clone();
    second.prepared = 1;
    second.remaining = 1;
    let head = frame(&first).unwrap();
    let tail = frame(&second).unwrap();
    for cut in 0..tail.len() {
        let mut bytes = head.clone();
        bytes.extend_from_slice(&tail[..cut]);
        let history = decode(&bytes).unwrap();
        assert_eq!(history.latest.as_ref(), Some(&first), "cut {cut}");
        assert_eq!(history.valid_bytes, head.len());
    }
    let mut bytes = head;
    bytes.extend_from_slice(&tail);
    assert_eq!(decode(&bytes).unwrap().latest, Some(second));
}

#[test]
fn corruption_of_each_payload_byte_is_rejected() {
    let original = frame(&initial()).unwrap();
    for position in HEADER..original.len() {
        let mut bytes = original.clone();
        bytes[position] ^= 1;
        assert!(decode(&bytes).is_err(), "position {position}");
    }
    let mut bytes = original;
    bytes[12] ^= 1;
    assert!(decode(&bytes).is_err());
}

#[test]
fn invalid_history_and_identity_changes_are_rejected() {
    let first = initial();
    for case in 0..7 {
        let mut next = first.clone();
        next.prepared = 1;
        next.remaining = 1;
        match case {
            0 => next.spec.transaction = "4".repeat(32),
            1 => next.spec.dll_path = "other.dll".into(),
            2 => next.prepared = 3,
            3 => next.phase = Phase::Installed,
            4 => next.schema = 2,
            5 => next.spec.sha256 = "5".repeat(64),
            _ => next.spec.registration_target = "other-backend-v1".into(),
        }
        let mut bytes = frame(&first).unwrap();
        // Encode directly so syntactically valid but semantically invalid
        // records can also exercise the recovery decoder.
        let payload = serde_json::to_vec(&next).unwrap();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&checksum(&payload).to_le_bytes());
        bytes.extend_from_slice(&payload);
        assert!(decode(&bytes).is_err(), "case {case}");
    }
}

#[test]
fn invalid_lengths_unknown_fields_and_complete_bad_tail_fail_closed() {
    let first = initial();
    let mut bytes = frame(&first).unwrap();
    bytes.extend_from_slice(b"unexpected tail");
    assert!(decode(&bytes).is_err());
    for length in [0, MAX_PAYLOAD + 1] {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&(length as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert!(decode(&bytes).is_err());
    }
    let mut value = serde_json::to_value(first).unwrap();
    value["enableByDefault"] = true.into();
    let payload = serde_json::to_vec(&value).unwrap();
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&checksum(&payload).to_le_bytes());
    bytes.extend_from_slice(&payload);
    assert!(decode(&bytes).is_err());
    assert!(decode(&vec![0; MAX_BYTES + 1]).is_err());
}

#[cfg(windows)]
fn temp_path() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "sumibi-journal-test-{}-{}.bin",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(windows)]
#[test]
fn file_is_exclusive_and_reopens_after_incomplete_append() {
    let path = temp_path();
    let mut journal = FileJournal::create(&path).unwrap();
    journal.append(&initial()).unwrap();
    assert!(FileJournal::open(&path).is_err());
    assert!(FileJournal::create(&path).is_err());
    drop(journal);
    let mut next = initial();
    next.prepared = 1;
    next.remaining = 1;
    let bytes = frame(&next).unwrap();
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&bytes[..bytes.len() - 1]).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let mut reopened = FileJournal::open(&path).unwrap();
    assert!(reopened.discarded_incomplete_tail());
    assert_eq!(reopened.latest().unwrap(), Some(initial()));
    reopened.append(&next).unwrap();
    drop(reopened);
    let reopened = FileJournal::open(&path).unwrap();
    assert!(!reopened.discarded_incomplete_tail());
    assert_eq!(reopened.latest().unwrap(), Some(next));
    drop(reopened);
    std::fs::remove_file(&path).unwrap();
}

#[cfg(windows)]
#[test]
fn corrupt_file_is_preserved_for_diagnosis() {
    let path = temp_path();
    let mut bytes = frame(&initial()).unwrap();
    bytes[HEADER] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    assert!(FileJournal::open(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(&path).unwrap();
}
