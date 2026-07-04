use super::*;

fn temp_store() -> (ChatSecretStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    (ChatSecretStore::open(path), dir)
}

#[test]
fn set_and_verify_password_roundtrip() {
    let (store, _dir) = temp_store();
    store.set_password("abc12345", "hunter2").unwrap();
    assert!(store.has_password("abc12345"));
    assert!(store.verify_password("abc12345", "hunter2"));
    assert!(!store.verify_password("abc12345", "wrong"));
}

#[test]
fn verify_missing_index_returns_false_and_consumes_time() {
    let (store, _dir) = temp_store();
    // No set_password call — index has no entry
    let start = std::time::Instant::now();
    let result = store.verify_password("deadbeef", "anything");
    let elapsed_missing = start.elapsed();
    assert!(!result);

    // Compare against the timing of a wrong-password check on a real
    // entry. Both should be in the same order of magnitude (argon2
    // verify takes ~10-50ms). If verify_missing returned immediately,
    // this is a timing-attack smoking gun.
    store.set_password("realindx", "correct").unwrap();
    let start = std::time::Instant::now();
    let _ = store.verify_password("realindx", "wrong");
    let elapsed_wrong = start.elapsed();

    // Loose ratio — both should be >5ms. If decoy path returned in 0ms,
    // ratio would blow up.
    assert!(
        elapsed_missing.as_millis() >= 5,
        "decoy verify too fast: {:?}",
        elapsed_missing
    );
    let ratio = if elapsed_wrong.as_millis() == 0 {
        9999.0
    } else {
        elapsed_missing.as_millis() as f64 / elapsed_wrong.as_millis() as f64
    };
    assert!(
        ratio > 0.1 && ratio < 10.0,
        "timing divergent: missing={:?} wrong={:?}",
        elapsed_missing,
        elapsed_wrong
    );
}

#[test]
fn clear_password_removes_entry() {
    let (store, _dir) = temp_store();
    store.set_password("abc12345", "hunter2").unwrap();
    assert!(store.has_password("abc12345"));
    store.clear_password("abc12345").unwrap();
    assert!(!store.has_password("abc12345"));
    // Clearing missing index is no-op, not error
    store.clear_password("never_set").unwrap();
}

#[test]
fn persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    {
        let store = ChatSecretStore::open(path.clone());
        store.set_password("abc12345", "hunter2").unwrap();
    }
    // Reopen — should still have the entry
    let reopened = ChatSecretStore::open(path);
    assert!(reopened.has_password("abc12345"));
    assert!(reopened.verify_password("abc12345", "hunter2"));
}
