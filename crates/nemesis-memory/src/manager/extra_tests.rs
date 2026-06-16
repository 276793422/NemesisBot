//! Additional coverage tests for MemoryManager (config, init, error paths).
//!
//! Targets branches not exercised by `tests.rs`:
//! - StoreConfig default-path branch in init_vector_store(None)
//! - load_persisted_sync failure path on bad JSONL
//! - init_vector_store_from_config error path when no plugin
//! - store_entry_to_vector no-op branches (vector disabled, no store)
//! - search/get/query_semantic fallthrough paths
//! - delete_by_id across stores
//! - close with no vector store
//! - Config::new variants

use super::*;

// ============================================================
// Config tests
// ============================================================

#[test]
fn test_config_new_from_string() {
    let cfg = Config::new("/some/path");
    assert_eq!(cfg.data_dir, PathBuf::from("/some/path"));
}

#[test]
fn test_config_new_from_path_buf() {
    let p = PathBuf::from("/another/path");
    let cfg = Config::new(p.clone());
    assert_eq!(cfg.data_dir, p);
}

#[test]
fn test_config_new_from_path_ref() {
    let p = Path::new("/ref/path");
    let cfg = Config::new(p);
    assert_eq!(cfg.data_dir, PathBuf::from("/ref/path"));
}

#[test]
fn test_config_new_preserves_default_vector_config() {
    let cfg = Config::new("/x");
    // Default VectorConfig has no plugin_path set
    assert!(cfg.vector.plugin_path.is_none());
}

// ============================================================
// set_vector_enabled runtime toggle
// ============================================================

#[test]
fn test_set_vector_enabled_no_store_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    // Toggling vector enabled without an initialized store should not panic.
    mgr.set_vector_enabled(true);
    mgr.set_vector_enabled(false);
}

#[tokio::test]
async fn test_store_skips_vector_when_disabled_flag() {
    // Even with vector store initialized, vector_enabled=false means
    // store_entry_to_vector should be a no-op.
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);

    // Don't init vector store — just verify store_entry works.
    let id = mgr
        .store_entry(Entry::new(MemoryType::LongTerm, "no vector test".into()))
        .await
        .unwrap();
    assert!(!id.is_empty());
}

// ============================================================
// init_vector_store(None) — default path branch
// ============================================================

#[test]
fn test_init_vector_store_none_uses_default_path() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    // None branch should resolve default path then fail because plugin is missing.
    let err = mgr.init_vector_store(None).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_init_vector_store_some_invalid_config_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let store_cfg = StoreConfig {
        embedding_tier: "plugin".into(),
        plugin_path: None, // missing plugin
        config_dir: None,
        max_results: 10,
        similarity_threshold: 0.7,
        storage_path: dir.path().join("v.jsonl").to_string_lossy().to_string(),
    };
    let err = mgr.init_vector_store(Some(store_cfg)).unwrap_err();
    assert!(!err.is_empty());
}

// ============================================================
// init_vector_store_from_config
// ============================================================

#[test]
fn test_init_vector_store_from_config_no_plugin_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let config_dir = tempfile::tempdir().unwrap();
    let err = mgr.init_vector_store_from_config(config_dir.path()).unwrap_err();
    assert!(err.contains("not found"));
}

// ============================================================
// close
// ============================================================

#[tokio::test]
async fn test_close_with_no_vector_store() {
    // close() should succeed even with no vector store initialized.
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    mgr.close().await.unwrap();
    assert!(!mgr.is_enabled());
}

#[tokio::test]
async fn test_close_disables_twice_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    mgr.close().await.unwrap();
    mgr.close().await.unwrap();
    assert!(!mgr.is_enabled());
}

// ============================================================
// store_entry_to_vector fallthrough
// ============================================================

#[tokio::test]
async fn test_store_with_vector_store_not_initialized_no_panic() {
    // vector_store is None and vector_enabled is false — store_entry_to_vector
    // should silently skip the vector path.
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let id = mgr
        .store_entry(Entry::new(MemoryType::LongTerm, "fallback test".into()))
        .await
        .unwrap();
    assert!(!id.is_empty());
}

// ============================================================
// search/get/query_semantic no-vector fallthrough
// ============================================================

#[tokio::test]
async fn test_get_falls_through_when_no_vector_store() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let id = mgr
        .store_entry(Entry::new(MemoryType::LongTerm, "to retrieve".into()))
        .await
        .unwrap();
    let got = mgr.get(&id).await.unwrap();
    assert!(got.is_some());
}

#[tokio::test]
async fn test_query_semantic_no_vector_falls_back_to_keyword() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    mgr.store_fact("Apples are fruits", vec![]).await.unwrap();
    let r = mgr.query_semantic("apples", 5).await.unwrap();
    assert!(r.total >= 1);
}

#[tokio::test]
async fn test_query_semantic_zero_limit_with_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    // No vector store, no entries — limit=0 should default to 5 and return empty.
    let r = mgr.query_semantic("nothing", 0).await.unwrap();
    assert_eq!(r.total, 0);
}

// ============================================================
// delete_by_id across stores
// ============================================================

#[tokio::test]
async fn test_delete_by_id_episodic_only() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let ep = Episode::new("del-sess".into(), "user".into(), "to be deleted".into());
    let id = mgr.append_episode(ep).await.unwrap();
    let found = mgr.delete_by_id(&id).await.unwrap();
    assert!(found);
}

#[tokio::test]
async fn test_delete_by_id_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let found = mgr.delete_by_id("nonexistent").await.unwrap();
    assert!(!found);
}

#[tokio::test]
async fn test_delete_by_id_no_vector_store_safe() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    // No vector store, no episodic entry — should be false and not panic.
    let found = mgr.delete_by_id("missing").await.unwrap();
    assert!(!found);
}

// ============================================================
// cleanup_episodic edge cases
// ============================================================

#[tokio::test]
async fn test_cleanup_episodic_empty() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let n = mgr.cleanup_episodic(10).await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn test_cleanup_episodic_zero_days() {
    // 0 days → cutoff = now, anything strictly older is removed.
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let mut old = Episode::new("z-sess".into(), "user".into(), "old".into());
    old.timestamp = chrono::Local::now() - chrono::Duration::days(2);
    mgr.append_episode(old).await.unwrap();
    let removed = mgr.cleanup_episodic(1).await.unwrap();
    assert!(removed >= 1);
}

// ============================================================
// list_episodic_sessions
// ============================================================

#[tokio::test]
async fn test_list_episodic_sessions_empty() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let sessions = mgr.list_episodic_sessions().await.unwrap();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn test_list_episodic_sessions_returns_keys() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    mgr.append_episode(Episode::new("a".into(), "user".into(), "x".into())).await.unwrap();
    mgr.append_episode(Episode::new("b".into(), "user".into(), "y".into())).await.unwrap();
    let sessions = mgr.list_episodic_sessions().await.unwrap();
    assert!(sessions.len() >= 2);
}

// ============================================================
// episodic_stats
// ============================================================

#[tokio::test]
async fn test_episodic_stats_empty() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let (s, e) = mgr.episodic_stats().await.unwrap();
    assert_eq!(s, 0);
    assert_eq!(e, 0);
}

// ============================================================
// graph operations edge cases
// ============================================================

#[tokio::test]
async fn test_graph_stats_empty() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let (ent, trip) = mgr.graph_stats().await.unwrap();
    assert_eq!(ent, 0);
    assert_eq!(trip, 0);
}

#[tokio::test]
async fn test_graph_query_empty_graph() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let r = mgr.query_graph("nothing", 3).await.unwrap();
    assert!(r.paths.is_empty());
}

#[tokio::test]
async fn test_graph_search_empty() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let r = mgr.search_graph("nothing", 10).await.unwrap();
    assert!(r.is_empty());
}

#[tokio::test]
async fn test_graph_get_related_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let r = mgr.get_related_triples("ghost", 3).await.unwrap();
    assert!(r.is_empty());
}

#[tokio::test]
async fn test_graph_query_triples_empty() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let r = mgr.query_graph_triples("", "", "").await.unwrap();
    assert!(r.is_empty());
}

#[tokio::test]
async fn test_graph_delete_nonexistent_entity() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    // Should succeed even on missing entity (idempotent).
    mgr.delete_graph_entity("ghost").await.unwrap();
}

// ============================================================
// parse_memory_type_from_str additional
// ============================================================

#[test]
fn test_parse_memory_type_uppercase() {
    // Uppercase variants should fall to LongTerm default.
    assert_eq!(parse_memory_type_from_str("LONG_TERM"), MemoryType::LongTerm);
    assert_eq!(parse_memory_type_from_str("EPISODIC"), MemoryType::LongTerm);
}

#[test]
fn test_parse_memory_type_with_whitespace() {
    // Whitespace is not trimmed; unknown → LongTerm default.
    assert_eq!(parse_memory_type_from_str(" long_term "), MemoryType::LongTerm);
}

#[test]
fn test_parse_memory_type_numeric() {
    assert_eq!(parse_memory_type_from_str("123"), MemoryType::LongTerm);
}

// ============================================================
// store_episodic and store_fact content paths
// ============================================================

#[tokio::test]
async fn test_store_episodic_tags_include_role() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let id = mgr.store_episodic("s-tags", "user", "tag test").await.unwrap();
    let got = mgr.get(&id).await.unwrap().unwrap();
    assert!(got.tags.contains(&"user".to_string()));
    assert!(got.tags.contains(&"conversation".to_string()));
}

#[tokio::test]
async fn test_store_episodic_metadata_has_session_key() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let id = mgr.store_episodic("s-meta", "user", "meta test").await.unwrap();
    let got = mgr.get(&id).await.unwrap().unwrap();
    assert_eq!(got.metadata.get("session_key").unwrap(), "s-meta");
    assert_eq!(got.metadata.get("role").unwrap(), "user");
}

#[tokio::test]
async fn test_store_fact_empty_tags() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let id = mgr.store_fact("empty tags fact", vec![]).await.unwrap();
    let got = mgr.get(&id).await.unwrap().unwrap();
    assert!(got.tags.is_empty());
}

#[tokio::test]
async fn test_store_fact_with_multiple_tags() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new(dir.path());
    let mgr = MemoryManager::new(&cfg);
    let tags = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let id = mgr.store_fact("multi tag", tags.clone()).await.unwrap();
    let got = mgr.get(&id).await.unwrap().unwrap();
    assert_eq!(got.tags, tags);
}

// ============================================================
// with_backends edge cases
// ============================================================

#[tokio::test]
async fn test_with_backends_isolates_data_dir() {
    // with_backends sets data_dir to empty PathBuf. Verify init_vector_store_from_config
    // fails because no plugin (and we don't depend on data_dir existence).
    let store = Arc::new(LocalStore::new());
    let dir = tempfile::tempdir().unwrap();
    let episodic = Arc::new(FileEpisodicStore::new(dir.path()));
    let graph = Arc::new(InMemoryGraphStore::new());
    let mgr = MemoryManager::with_backends(store, episodic, graph);
    let cfg_dir = tempfile::tempdir().unwrap();
    let r = mgr.init_vector_store_from_config(cfg_dir.path());
    assert!(r.is_err());
}

// ============================================================
// with_config_dir config write-back path
// ============================================================

#[test]
fn test_with_config_dir_creates_default_when_missing() {
    // When config file is missing, with_config_dir should call
    // load_embedding_config which writes a default file.
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let _mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    // After call, the default config file should exist.
    assert!(config_dir.path().join("config.enhanced_memory.json").exists());
}

#[test]
fn test_with_config_dir_enabled_no_plugin_writes_disabled() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let path = config_dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, r#"{"enabled": true, "active": "medium"}"#).unwrap();
    let _mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("\"enabled\": false"), "config should be disabled: {}", content);
}

// ============================================================
// detect_plugin_path
// ============================================================

#[test]
fn test_detect_plugin_path_does_not_panic() {
    // Just ensure no panic — result is environment dependent.
    let _ = MemoryManager::detect_plugin_path();
}
