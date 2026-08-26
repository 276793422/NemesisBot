//! W2c batch tests (Phase 3 / quality-hardening goal).
//!
//! Targets vector/store.rs branches not exercised by tests.rs / extra_tests.rs:
//! - load_persisted_sync with non-UTF-8 content → Err (io error path)
//! - persist_entry_sync failure arms (parent is a file / storage path is a dir)
//! - rewrite_persist_file failure is silent (unwritable persist path)
//! - failing embed backend: store_entry / query propagate the error
//! - embed_handle returns a working standalone handle (M5 lock discipline)
//! - exact total-vs-capped-entries accounting in query results

use super::*;

fn stub_embed() -> Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync> {
    Box::new(|s: &str| {
        if s.is_empty() {
            return Ok(vec![0.0, 0.0, 0.0, 0.0]);
        }
        Ok(vec![s.len() as f32, 1.0, 0.5, 0.25])
    })
}

fn failing_embed() -> Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync> {
    Box::new(|_: &str| Err("embed down".into()))
}

fn make_cfg(path: &std::path::Path) -> StoreConfig {
    StoreConfig {
        embedding_tier: "plugin".into(),
        plugin_path: None,
        config_dir: None,
        max_results: 10,
        similarity_threshold: -1.0, // accept everything
        storage_path: path.to_string_lossy().to_string(),
    }
}

fn make_entry(id: &str, content: &str) -> VectorEntry {
    VectorEntry {
        id: id.into(),
        entry_type: "long_term".into(),
        content: content.into(),
        metadata: HashMap::new(),
        tags: vec![],
        score: 0.0,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

// ============================================================
// load_persisted_sync — non-UTF-8 content
// ============================================================

#[test]
fn load_persisted_sync_invalid_utf8_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_utf8.jsonl");
    std::fs::write(&path, [0xFFu8, 0xFE, 0x00, 0x01, 0xF8]).unwrap();

    let store = VectorStore::new_from_embed(stub_embed(), make_cfg(&path));
    // new_from_embed swallows the auto-load error; the explicit call must
    // surface the io error (read_to_string on non-UTF-8 fails).
    let err = store.load_persisted_sync().unwrap_err();
    assert!(
        err.contains("stream") || err.to_lowercase().contains("invalid") || !err.is_empty(),
        "unexpected error: {}",
        err
    );
    // Nothing loadable was added.
    assert!(store.is_empty());
}

// ============================================================
// persist_entry_sync failure arms
// ============================================================

#[test]
fn persist_entry_sync_parent_is_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("blocker.txt");
    std::fs::write(&file, "i am a file").unwrap();
    // Parent of the persist path is the file above → create_dir_all fails.
    let path = dir.path().join("blocker.txt").join("v.jsonl");

    let store = VectorStore::new_from_embed(stub_embed(), make_cfg(&path));
    let err = store.persist_entry_sync(&make_entry("p1", "x")).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn persist_entry_sync_storage_path_is_dir_errors() {
    let dir = tempfile::tempdir().unwrap();
    // The persist path itself is a directory → open(create+append) fails.
    let store = VectorStore::new_from_embed(stub_embed(), make_cfg(dir.path()));
    let err = store.persist_entry_sync(&make_entry("p2", "x")).unwrap_err();
    assert!(!err.is_empty());
}

// ============================================================
// rewrite_persist_file failure is silent
// ============================================================

#[test]
fn delete_entry_with_unwritable_persist_path_still_deletes() {
    let dir = tempfile::tempdir().unwrap();
    // Persist path inside a directory that does not exist — File::create on
    // the tmp file fails, the failure must be swallowed (no panic) and the
    // in-memory delete must still succeed.
    let path = dir.path().join("missing_dir").join("v.jsonl");

    let store = VectorStore::new_from_embed(stub_embed(), make_cfg(&path));
    store.store_entry(&make_entry("d1", "x")).unwrap();
    store.store_entry(&make_entry("d2", "y")).unwrap();
    assert_eq!(store.len(), 2);

    assert!(store.delete_entry("d1"));
    assert_eq!(store.len(), 1);
    assert!(!path.exists(), "no persist file should have been created");
}

// ============================================================
// Failing embed backend
// ============================================================

#[test]
fn store_entry_with_failing_embed_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let store = VectorStore::new_from_embed(failing_embed(), make_cfg(&path));
    let err = store.store_entry(&make_entry("f1", "x")).unwrap_err();
    assert_eq!(err, "embed down");
    assert!(store.is_empty());
}

#[test]
fn query_with_failing_embed_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let store = VectorStore::new_from_embed(failing_embed(), make_cfg(&path));
    let err = store.query("anything", 5, &[]).unwrap_err();
    assert_eq!(err, "embed down");
}

// ============================================================
// embed_handle (M5 lock discipline entry point)
// ============================================================

#[test]
fn embed_handle_returns_working_handle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let store = VectorStore::new_from_embed(stub_embed(), make_cfg(&path));
    let handle = store.embed_handle();
    // The handle must run inference without any store lock held.
    let v = handle("abcd").unwrap();
    assert_eq!(v, vec![4.0, 1.0, 0.5, 0.25]);
    // Store still usable concurrently.
    store.store_entry(&make_entry("h1", "x")).unwrap();
    assert_eq!(store.len(), 1);
}

// ============================================================
// Query result accounting
// ============================================================

#[test]
fn query_total_exact_when_capped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let store = VectorStore::new_from_embed(stub_embed(), make_cfg(&path));
    for i in 0..5 {
        store
            .store_entry(&make_entry(&format!("c{}", i), "identical"))
            .unwrap();
    }
    let r = store.query("identical", 2, &[]).unwrap();
    assert_eq!(r.entries.len(), 2, "entries capped at limit");
    assert_eq!(r.total, 5, "total counts ALL matches, not just returned");
}

#[test]
fn query_result_carries_query_string() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let store = VectorStore::new_from_embed(stub_embed(), make_cfg(&path));
    let r = store.query("the query echo", 5, &[]).unwrap();
    assert_eq!(r.query, "the query echo");
}
