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
fn w4a_open_corrupt_json_poisons_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    std::fs::write(&path, b"{ this is not json !!!").unwrap();
    // Corrupt load: the store poisons (fail-closed, 2026-10-16 hardening).
    let store = ChatSecretStore::open(path.clone());
    assert!(store.is_poisoned());
    // Reads fail closed: state is unknown, so it must look password-
    // protected and verification must deny.
    assert!(store.has_password("abc12345"));
    assert!(!store.verify_password("abc12345", "hunter2"));
    // Writes are refused — the unreadable file may hold recoverable data.
    let err = store.set_password("abc12345", "hunter2").unwrap_err();
    assert!(err.contains("locked"), "unexpected error: {}", err);
    assert!(store.clear_password("abc12345").is_err());
    // The original file is preserved untouched for manual recovery.
    assert_eq!(std::fs::read(&path).unwrap(), b"{ this is not json !!!");
    // ...and a diagnostic copy exists alongside it.
    let copies: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
        .collect();
    assert_eq!(
        copies.len(),
        1,
        "expected one .corrupt-<ts> diagnostic copy"
    );
}

#[test]
fn w4a_open_unreadable_path_poisons() {
    // Reading a directory as a file errors with a non-NotFound kind,
    // hitting the generic read-failure arm — state unknown → poisoned
    // (fail-closed), same contract as corrupt JSON.
    let dir = tempfile::tempdir().unwrap();
    let store = ChatSecretStore::open(dir.path().to_path_buf());
    assert!(store.is_poisoned());
    assert!(store.has_password("anything"));
    assert!(store.set_password("abc", "pw").is_err());
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
fn w4a_in_memory_store_full_lifecycle_without_disk() {
    let store = ChatSecretStore::in_memory();
    assert!(!store.is_poisoned());
    assert!(!store.has_password("abc12345"));
    // set/clear/verify all work; persist_candidate_locked short-circuits
    // (no path → pure swap)
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
fn w4a_set_password_create_dir_failure_leaves_memory_untouched() {
    // Parent of the target path is a regular file -> create_dir_all fails
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"i am a file").unwrap();
    let path = blocker.join("nested").join("secrets.json");
    let store = ChatSecretStore::open(path);
    let err = store.set_password("abc12345", "hunter2").unwrap_err();
    assert!(
        err.contains("persist chat secrets"),
        "unexpected error: {}",
        err
    );
    // Persist-then-swap (2026-10-16 hardening): the in-memory map is only
    // swapped after a successful write — a failed persist must NOT leave
    // the password visible in memory while disk has nothing.
    assert!(!store.has_password("abc12345"));
    assert!(!store.verify_password("abc12345", "hunter2"));
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

// ---------------------------------------------------------------------------
// 2026-10-16 hardening: canonical 16-hex keys + legacy 8-hex compat
// ---------------------------------------------------------------------------

#[test]
fn canonical_index_password_roundtrip_and_legacy_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    let store = ChatSecretStore::open(path.clone());
    let canonical = "0123456789abcdef"; // 16 hex chars
    store.set_password(canonical, "hunter2").unwrap();
    assert!(store.has_password(canonical));
    assert!(store.verify_password(canonical, "hunter2"));

    // A second store opened over the same file must see the entry.
    drop(store);
    let reopened = ChatSecretStore::open(path);
    assert!(reopened.has_password(canonical));
    assert!(reopened.verify_password(canonical, "hunter2"));

    // Now hand-write a LEGACY 8-hex entry (pre-hardening layout) and check
    // the canonical key resolves it via the prefix fallback.
    let dir2 = tempfile::tempdir().unwrap();
    let path2 = dir2.path().join("secrets.json");
    let mut map = std::collections::HashMap::new();
    map.insert(canonical[..8].to_string(), hash_password("oldpw").unwrap());
    std::fs::write(&path2, serde_json::to_string(&map).unwrap()).unwrap();
    let legacy_store = ChatSecretStore::open(path2);
    assert!(legacy_store.has_password(canonical));
    assert!(legacy_store.verify_password(canonical, "oldpw"));
    assert!(!legacy_store.verify_password(canonical, "hunter2"));
}

#[test]
fn canonical_set_password_removes_legacy_twin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    let canonical = "fedcba9876543210";
    // Seed with a legacy 8-hex entry.
    let mut map = std::collections::HashMap::new();
    map.insert(canonical[..8].to_string(), hash_password("oldpw").unwrap());
    std::fs::write(&path, serde_json::to_string(&map).unwrap()).unwrap();
    let store = ChatSecretStore::open(path.clone());
    assert!(store.verify_password(canonical, "oldpw"));

    // Setting a new password under the canonical key removes the legacy
    // twin in the same write (self-healing convergence).
    store.set_password(canonical, "newpw").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let map: HashMap<String, String> = serde_json::from_str(&text).unwrap();
    assert!(
        map.contains_key(canonical) && !map.contains_key(&canonical[..8]),
        "expected canonical key only, got keys: {:?}",
        map.keys().collect::<Vec<_>>()
    );
    assert!(store.verify_password(canonical, "newpw"));
    assert!(!store.verify_password(canonical, "oldpw"));
}

#[test]
fn clear_password_accepts_legacy_form() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    // Password written under the legacy 8-hex key; clearing with the
    // LEGACY form must still find and remove it.
    let mut map = std::collections::HashMap::new();
    map.insert("abcd1234".to_string(), hash_password("pw").unwrap());
    std::fs::write(&path, serde_json::to_string(&map).unwrap()).unwrap();
    let store = ChatSecretStore::open(path);
    assert!(store.has_password("abcd1234"));
    store.clear_password("abcd1234").unwrap();
    assert!(!store.has_password("abcd1234"));
}

// 非 16-hex canonical 形态但字节数恰为 16、字节 8 落在多字节字符中间的
// 串（web 层 canonical_chat_index 对解析不到的 index 原样透传，客户端
// 可构造）——四个入口都不得 panic，且按普通键完整工作（2026-10-17 全仓
// 横扫：legacy_key_of 裸 [..8] 曾在字节 8 非边界时崩；仅边界安全化又会
// 让 set_password 的孪生清理把刚写入的条目立即删掉=静默 no-op，故闸门
// 升级为 is_canonical_form（字节长+ASCII），多字节串根本不进 legacy 分支）。
#[test]
fn sixteen_byte_multibyte_index_never_panics() {
    // 7 ASCII + 「中」(3B, 占 7..10) + 6 ASCII = 16 字节，字节 8 是续字节。
    let evil = format!("{}中{}", "a".repeat(7), "b".repeat(6));
    assert_eq!(evil.len(), 16);
    let store = ChatSecretStore::in_memory();
    assert!(!store.has_password(&evil));
    assert!(!store.verify_password(&evil, "pw"));

    // 按普通键完整往返（is_canonical_form 门：不走 legacy 孪生分支）。
    store.set_password(&evil, "pw").unwrap();
    assert!(store.has_password(&evil));
    assert!(store.verify_password(&evil, "pw"));
    store.clear_password(&evil).unwrap();
    assert!(!store.has_password(&evil));
}
