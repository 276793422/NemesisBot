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

// ---------------------------------------------------------------------------
// W4a coverage gap closure (corrupt/empty/unreadable load, in_memory store,
// empty-index rejection, create-dir failure, invalid stored hash)
// ---------------------------------------------------------------------------

#[test]
fn w4a_open_corrupt_json_starts_empty_and_self_heals() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    std::fs::write(&path, b"{ this is not json !!!").unwrap();
    // Corrupt load: starts empty (warn path), writes still attempted
    let store = ChatSecretStore::open(path.clone());
    assert!(!store.has_password("abc12345"));
    // Self-heal: a fresh set_password rewrites valid JSON over the garbage
    store.set_password("abc12345", "hunter2").unwrap();
    let reopened = ChatSecretStore::open(path);
    assert!(reopened.has_password("abc12345"));
    assert!(reopened.verify_password("abc12345", "hunter2"));
}

#[test]
fn w4a_open_empty_file_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    std::fs::write(&path, b"   \n  ").unwrap();
    let store = ChatSecretStore::open(path);
    assert!(!store.has_password("anything"));
    assert!(!store.verify_password("anything", "pw"));
}

#[test]
fn w4a_open_unreadable_path_starts_empty() {
    // Reading a directory as a file errors with a non-NotFound kind,
    // hitting the generic read-failure arm (not first-run, not corrupt).
    let dir = tempfile::tempdir().unwrap();
    let store = ChatSecretStore::open(dir.path().to_path_buf());
    assert!(!store.has_password("anything"));
}

#[test]
fn w4a_in_memory_store_full_lifecycle_without_disk() {
    let store = ChatSecretStore::in_memory();
    assert!(!store.has_password("abc12345"));
    // set/clear/verify all work; persist_locked short-circuits Ok (no path)
    store.set_password("abc12345", "hunter2").unwrap();
    assert!(store.has_password("abc12345"));
    assert!(store.verify_password("abc12345", "hunter2"));
    assert!(!store.verify_password("abc12345", "wrong"));
    store.clear_password("abc12345").unwrap();
    assert!(!store.has_password("abc12345"));
}

#[test]
fn w4a_set_password_rejects_blank_index() {
    let (store, _dir) = temp_store();
    assert!(store.set_password("", "pw").is_err());
    assert!(store.set_password("   ", "pw").is_err());
    assert!(!store.has_password(""));
}

#[test]
fn w4a_set_password_create_dir_failure_returns_err() {
    // Parent of the target path is a regular file -> create_dir_all fails
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"i am a file").unwrap();
    let path = blocker.join("nested").join("secrets.json");
    let store = ChatSecretStore::open(path);
    let err = store.set_password("abc12345", "hunter2").unwrap_err();
    assert!(err.starts_with("create dir"), "unexpected error: {}", err);
    // Note actual behavior: the entry IS inserted into the in-memory map
    // before persist is attempted, so it stays visible despite the error.
    // The Err return is the failure signal; disk state is unchanged.
    assert!(store.has_password("abc12345"));
    // ...and nothing landed on disk under the blocked parent
    assert!(!blocker.join("nested").join("secrets.json").exists());
}

#[test]
fn w4a_verify_against_invalid_stored_hash_returns_false() {
    // Hand-write a map whose hash string is not a valid PHC string; the
    // argon2 parse fails and verify must return false (not panic).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    let mut map = std::collections::HashMap::new();
    map.insert("abc12345".to_string(), "not-a-real-argon2-hash".to_string());
    std::fs::write(&path, serde_json::to_string(&map).unwrap()).unwrap();
    let store = ChatSecretStore::open(path);
    assert!(store.has_password("abc12345"));
    assert!(!store.verify_password("abc12345", "hunter2"));
}
