//! W2c batch tests (Phase 3 / quality-hardening goal).
//!
//! Targets manager.rs branches not exercised by tests.rs / extra_tests.rs:
//! - close() error concatenation with a failing general store (mock backend)
//! - store_entry backend error propagation
//! - vector-store error propagation in search / query_semantic (contract pin:
//!   vector errors do NOT silently fall back to keyword search)
//! - init_vector_store_from_config "already initialized → just enable" arm
//! - with_config_dir with pre-existing enabled=false config left untouched
//! - episodic delete/cleanup also removing vector entries (no-plugin path)
//! - MemoryType roundtrip through the vector adapter (Display vs Debug bug)
//! - silent persistence failure when the persist path is a directory

use super::*;
use async_trait::async_trait;

// ============================================================
// Mock backend with configurable failure flags
// ============================================================

struct FlakyStore {
    inner: Arc<LocalStore>,
    fail_store: bool,
    fail_close: bool,
}

#[async_trait]
impl MemoryStore for FlakyStore {
    async fn store(&self, entry: Entry) -> Result<String, String> {
        if self.fail_store {
            return Err("store down".into());
        }
        self.inner.store(entry).await
    }

    async fn query(
        &self,
        query: &str,
        memory_type: Option<MemoryType>,
        limit: usize,
    ) -> Result<SearchResult, String> {
        self.inner.query(query, memory_type, limit).await
    }

    async fn get(&self, id: &str) -> Result<Option<Entry>, String> {
        self.inner.get(id).await
    }

    async fn delete(&self, id: &str) -> Result<bool, String> {
        self.inner.delete(id).await
    }

    async fn list(
        &self,
        memory_type: Option<MemoryType>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Entry>, String> {
        self.inner.list(memory_type, limit, offset).await
    }

    async fn close(&self) -> Result<(), String> {
        if self.fail_close {
            return Err("boom".into());
        }
        Ok(())
    }
}

fn mgr_with_flaky(fail_store: bool, fail_close: bool) -> (MemoryManager, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(FlakyStore {
        inner: Arc::new(LocalStore::new()),
        fail_store,
        fail_close,
    });
    let episodic = Arc::new(FileEpisodicStore::new(dir.path().join("episodic")));
    let graph = Arc::new(InMemoryGraphStore::new());
    (
        MemoryManager::with_backends(store, episodic, graph),
        dir,
    )
}

// ============================================================
// Stub embed (deterministic, no plugin)
// ============================================================

fn stub_embed() -> crate::vector::EmbeddingFunc {
    Box::new(|s: &str| Ok(vec![s.len() as f32, 1.0, 0.5, 0.25]))
}

fn failing_embed() -> crate::vector::EmbeddingFunc {
    Box::new(|_: &str| Err("embed down".into()))
}

fn store_cfg_for(path: &std::path::Path, threshold: f64) -> StoreConfig {
    StoreConfig {
        embedding_tier: "plugin".into(),
        plugin_path: None,
        config_dir: None,
        max_results: 10,
        similarity_threshold: threshold,
        storage_path: path.to_string_lossy().to_string(),
    }
}

/// Build a manager with a working stub vector store, vector enabled.
fn mgr_with_stub_vector(threshold: f64) -> (MemoryManager, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let vpath = dir.path().join("v.jsonl");
    mgr.init_vector_store_with_embed(stub_embed(), store_cfg_for(&vpath, threshold))
        .unwrap();
    mgr.set_vector_enabled(true);
    (mgr, dir)
}

// ============================================================
// close() with failing general store
// ============================================================

#[tokio::test]
async fn close_with_failing_general_store_reports_error() {
    let (mgr, _dir) = mgr_with_flaky(false, true);
    let err = mgr.close().await.unwrap_err();
    assert!(
        err.contains("memory close errors"),
        "expected concatenated prefix, got: {}",
        err
    );
    assert!(
        err.contains("store: boom"),
        "expected backend error surfaced, got: {}",
        err
    );
    // close() disables the manager regardless of backend errors.
    assert!(!mgr.is_enabled());
}

// ============================================================
// store_entry backend error propagation
// ============================================================

#[tokio::test]
async fn store_entry_propagates_backend_store_error() {
    let (mgr, _dir) = mgr_with_flaky(true, false);
    let err = mgr
        .store_entry(Entry::new(MemoryType::LongTerm, "x".into()))
        .await
        .unwrap_err();
    assert_eq!(err, "store down");
}

// ============================================================
// Vector error propagation (contract pin)
// ============================================================

#[tokio::test]
async fn search_with_failing_vector_embed_propagates_error() {
    // Contract: when the vector store is enabled and its embedding backend
    // fails, search() propagates the error — it does NOT silently fall back
    // to keyword search (the fallback only happens on EMPTY results).
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let vpath = dir.path().join("v.jsonl");
    mgr.init_vector_store_with_embed(failing_embed(), store_cfg_for(&vpath, 0.0))
        .unwrap();
    mgr.set_vector_enabled(true);
    let err = mgr.search("anything", None, 5).await.unwrap_err();
    assert_eq!(err, "embed down");
}

#[tokio::test]
async fn query_semantic_with_failing_vector_embed_propagates_error() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let vpath = dir.path().join("v.jsonl");
    mgr.init_vector_store_with_embed(failing_embed(), store_cfg_for(&vpath, 0.0))
        .unwrap();
    let err = mgr.query_semantic("anything", 5).await.unwrap_err();
    assert_eq!(err, "embed down");
}

#[tokio::test]
async fn search_vector_empty_result_falls_back_to_keyword() {
    // The `!result.entries.is_empty()` gate: when the vector store returns
    // zero hits (threshold filters everything), search falls back to the
    // keyword store, which has the entry.
    let (mgr, _dir) = mgr_with_stub_vector(1.5); // impossible threshold
    mgr.store_fact("apples are fruits", vec![])
        .await
        .unwrap();
    let r = mgr.search("apples", None, 10).await.unwrap();
    assert!(
        r.entries.iter().any(|se| se.entry.content.contains("apples")),
        "expected keyword fallback hit, got {:?}",
        r.entries
    );
}

// ============================================================
// init_vector_store_from_config — already initialized arm
// ============================================================

#[test]
fn init_vector_store_from_config_already_initialized_just_enables() {
    // When a vector store is already wired (e.g. via test fixture), the
    // Dashboard toggle path must succeed WITHOUT touching plugin detection.
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let vpath = dir.path().join("v.jsonl");
    mgr.init_vector_store_with_embed(stub_embed(), store_cfg_for(&vpath, 0.0))
        .unwrap();
    assert!(!mgr.is_vector_enabled());

    let cfg_dir = tempfile::tempdir().unwrap();
    mgr.init_vector_store_from_config(cfg_dir.path()).unwrap();
    assert!(mgr.is_vector_enabled());
}

// ============================================================
// with_config_dir — disabled config untouched
// ============================================================

#[test]
fn with_config_dir_disabled_config_left_untouched() {
    // enabled=false takes the early-return arm: the file must NOT be
    // rewritten (no disable write-back), custom content preserved verbatim.
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let path = config_dir.path().join("config.enhanced_memory.json");
    let original = r#"{"enabled": false, "active": "small", "models": {"small": {"name": "custom-marker", "dimension": 128, "model_url": "", "tokenizer_url": ""}}}"#;
    std::fs::write(&path, original).unwrap();

    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());

    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, original, "disabled config must not be rewritten");
    assert!(!mgr.is_vector_enabled());
}

// ============================================================
// Episodic delete/cleanup also removing vector entries
// ============================================================

#[tokio::test]
async fn delete_episode_session_removes_vector_entries() {
    let (mgr, _dir) = mgr_with_stub_vector(-1.0);
    let id = mgr
        .append_episode(Episode::new("s1".into(), "user".into(), "vector cleanup".into()))
        .await
        .unwrap();
    // append_episode does NOT touch the general store — a get() hit here
    // proves the entry landed in the vector store.
    assert!(mgr.get(&id).await.unwrap().is_some());

    let n = mgr.delete_episode_session("s1").await.unwrap();
    assert_eq!(n, 1);
    // Vector-side removal: the only remaining backend no longer has it.
    assert!(mgr.get(&id).await.unwrap().is_none());
}

#[tokio::test]
async fn cleanup_episodic_removes_old_vector_entries() {
    let (mgr, _dir) = mgr_with_stub_vector(-1.0);
    let mut old = Episode::new("old-sess".into(), "user".into(), "stale episode".into());
    old.timestamp = chrono::Local::now() - chrono::Duration::days(2);
    let id = mgr.append_episode(old).await.unwrap();
    assert!(mgr.get(&id).await.unwrap().is_some());

    let removed = mgr.cleanup_episodic(1).await.unwrap();
    assert!(removed >= 1);
    assert!(mgr.get(&id).await.unwrap().is_none());
}

// ============================================================
// MemoryType roundtrip through the vector adapter (BUG #10)
// ============================================================

/// Direct pin on the parse helper: every Display form parses back to the
/// same variant, and unknown strings default to LongTerm.
#[test]
fn parse_memory_type_from_str_matches_display_forms() {
    let all = [
        MemoryType::ShortTerm,
        MemoryType::LongTerm,
        MemoryType::Episodic,
        MemoryType::Graph,
        MemoryType::Daily,
    ];
    for mt in all {
        assert_eq!(parse_memory_type_from_str(&mt.to_string()), mt);
    }
    assert_eq!(parse_memory_type_from_str(""), MemoryType::LongTerm);
    assert_eq!(parse_memory_type_from_str("bogus"), MemoryType::LongTerm);
}

/// End-to-end: an entry stored with the vector adapter enabled must come
/// back from the VECTOR store with the same MemoryType. query_semantic is
/// vector-first (unlike get(), which prefers the keyword store), so this
/// exercises the store_entry_to_vector write + parse_memory_type_from_str
/// read pair. Before BUG #10 the write used Debug-lowercase ("shortterm")
/// while the read parsed Display ("short_term") — ShortTerm/Graph/Daily
/// entries came back as LongTerm.
#[tokio::test]
async fn vector_adapter_roundtrips_memory_type() {
    let (mgr, _dir) = mgr_with_stub_vector(-1.0);
    let cases = [
        MemoryType::ShortTerm,
        MemoryType::LongTerm,
        MemoryType::Episodic,
        MemoryType::Graph,
        MemoryType::Daily,
    ];
    for mt in cases {
        let marker = format!("roundtrip probe {}", mt);
        let entry = Entry::new(mt, marker.clone());
        mgr.store_entry(entry).await.unwrap();

        // Vector-first retrieval (no keyword-store shortcut).
        let r = mgr.query_semantic(&marker, 10).await.unwrap();
        assert!(
            r.entries.iter().any(|se| se.entry.typ == mt),
            "{:?} must survive the vector store roundtrip, got {:?}",
            mt,
            r.entries.iter().map(|se| se.entry.typ).collect::<Vec<_>>()
        );
    }
}

/// The search() type filter must match what store_entry_to_vector writes:
/// filter by ShortTerm and only the ShortTerm entry comes back.
#[tokio::test]
async fn search_type_filter_matches_stored_type() {
    let (mgr, _dir) = mgr_with_stub_vector(-1.0);
    mgr.store_entry(Entry::new(MemoryType::ShortTerm, "short term probe one".into()))
        .await
        .unwrap();
    mgr.store_entry(Entry::new(MemoryType::LongTerm, "long term probe two".into()))
        .await
        .unwrap();

    let r = mgr.search("probe", Some(MemoryType::ShortTerm), 10).await.unwrap();
    assert!(!r.entries.is_empty(), "type filter must hit vector entries");
    assert!(
        r.entries.iter().all(|se| se.entry.typ == MemoryType::ShortTerm),
        "filtered results must all be ShortTerm, got {:?}",
        r.entries.iter().map(|se| se.entry.typ).collect::<Vec<_>>()
    );
}

// ============================================================
// Silent persistence failure (persist path is a directory)
// ============================================================

#[tokio::test]
async fn append_episode_vector_persist_failure_is_silent() {
    // storage_path pointing at a DIRECTORY makes persist_entry_sync fail on
    // open — the adapter only logs at debug level; append must still succeed
    // and the in-memory vector copy must serve reads.
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    // Persist path = the tempdir itself (a directory).
    mgr.init_vector_store_with_embed(stub_embed(), store_cfg_for(dir.path(), -1.0))
        .unwrap();
    mgr.set_vector_enabled(true);

    let id = mgr
        .append_episode(Episode::new("persist-fail".into(), "user".into(), "still works".into()))
        .await
        .unwrap();
    assert!(!id.is_empty());
    assert!(mgr.get(&id).await.unwrap().is_some());
}
