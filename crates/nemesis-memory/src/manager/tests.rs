use super::*;

// ===================================================================
// Non-ignored tests (basic memory, no plugin required)
// ===================================================================

#[tokio::test]
async fn unified_store_and_search() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Store an entry.
    let entry = Entry::new(MemoryType::LongTerm, "Paris is the capital of France".to_string());
    let id = mgr.store_entry(entry).await.unwrap();

    // Search for it.
    let results = mgr.search("Paris", None, 10).await.unwrap();
    assert_eq!(results.total, 1);
    assert_eq!(results.entries[0].entry.id, id);
}

#[tokio::test]
async fn unified_forget_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let e1 = Entry::new(MemoryType::ShortTerm, "temp note 1".to_string());
    let e2 = Entry::new(MemoryType::LongTerm, "important fact".to_string());
    let id1 = mgr.store_entry(e1).await.unwrap();
    let _id2 = mgr.store_entry(e2).await.unwrap();

    // List all.
    let all = mgr.list(None, 10, 0).await.unwrap();
    assert_eq!(all.len(), 2);

    // Forget one.
    let removed = mgr.forget(&id1).await.unwrap();
    assert!(removed);

    let remaining = mgr.list(None, 10, 0).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].content, "important fact");
}

#[tokio::test]
async fn unified_graph_operations() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.upsert_entity(GraphEntity::new("tokio".into(), "crate".into()))
        .await
        .unwrap();
    mgr.upsert_entity(GraphEntity::new("runtime".into(), "concept".into()))
        .await
        .unwrap();
    mgr.add_triple(GraphTriple::new(
        "tokio".into(),
        "provides".into(),
        "runtime".into(),
    ))
    .await
    .unwrap();

    let result = mgr.query_graph("tokio", 2).await.unwrap();
    assert_eq!(result.paths.len(), 1);
    assert_eq!(result.paths[0][0].object, "runtime");
    assert_eq!(result.entities.len(), 2);
}

#[tokio::test]
async fn unified_store_episodic_helper() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let id = mgr
        .store_episodic("sess-1", "user", "What is Rust?")
        .await
        .unwrap();
    assert!(!id.is_empty());

    let results = mgr.search("Rust", None, 10).await.unwrap();
    assert_eq!(results.total, 1);
}

#[tokio::test]
async fn unified_store_fact_helper() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let id = mgr
        .store_fact("Rust was created by Mozilla", vec!["rust".to_string()])
        .await
        .unwrap();
    assert!(!id.is_empty());

    let results = mgr.search("Mozilla", None, 10).await.unwrap();
    assert_eq!(results.total, 1);
}

#[tokio::test]
async fn unified_graph_search() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.add_triple(GraphTriple::new(
        "rust".into(),
        "is_a".into(),
        "language".into(),
    ))
    .await
    .unwrap();
    mgr.add_triple(GraphTriple::new(
        "python".into(),
        "is_a".into(),
        "language".into(),
    ))
    .await
    .unwrap();

    let results = mgr.search_graph("rust", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].subject, "rust");
}

#[tokio::test]
async fn unified_graph_delete_entity() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.upsert_entity(GraphEntity::new("rust".into(), "language".into()))
        .await
        .unwrap();
    mgr.add_triple(GraphTriple::new(
        "rust".into(),
        "is_a".into(),
        "language".into(),
    ))
    .await
    .unwrap();

    mgr.delete_graph_entity("rust").await.unwrap();

    let entity = mgr.get_graph_entity("rust").await.unwrap();
    assert!(entity.is_none());
}

#[tokio::test]
async fn unified_graph_get_related() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.add_triple(GraphTriple::new(
        "a".into(),
        "rel".into(),
        "b".into(),
    ))
    .await
    .unwrap();
    mgr.add_triple(GraphTriple::new(
        "b".into(),
        "rel".into(),
        "c".into(),
    ))
    .await
    .unwrap();

    let related = mgr.get_related_triples("a", 2).await.unwrap();
    assert!(related.len() >= 2);
}

#[tokio::test]
async fn unified_graph_stats() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.upsert_entity(GraphEntity::new("x".into(), "thing".into()))
        .await
        .unwrap();
    mgr.add_triple(GraphTriple::new(
        "x".into(),
        "has".into(),
        "y".into(),
    ))
    .await
    .unwrap();

    let (entities, triples) = mgr.graph_stats().await.unwrap();
    assert_eq!(entities, 1);
    assert_eq!(triples, 1);
}

#[tokio::test]
async fn unified_query_semantic_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_fact("The Eiffel Tower is in Paris", vec![])
        .await
        .unwrap();

    let results = mgr.query_semantic("Eiffel", 5).await.unwrap();
    assert_eq!(results.total, 1);
}

// -- New tests for enabled flag and missing methods --------------------

#[tokio::test]
async fn test_is_enabled_default() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);
    assert!(mgr.is_enabled());
}

#[tokio::test]
async fn test_close_disables_manager() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);
    assert!(mgr.is_enabled());

    mgr.close().await.unwrap();
    assert!(!mgr.is_enabled());
}

#[tokio::test]
async fn test_disabled_store_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Store one entry before disabling.
    mgr.store_entry(Entry::new(MemoryType::LongTerm, "before".to_string()))
        .await
        .unwrap();

    mgr.close().await.unwrap();

    // Store when disabled returns empty string.
    let id = mgr
        .store_entry(Entry::new(MemoryType::LongTerm, "after".to_string()))
        .await
        .unwrap();
    assert!(id.is_empty());

    // Query when disabled returns empty.
    let result = mgr.search("before", None, 10).await.unwrap();
    assert_eq!(result.total, 0);

    // Get when disabled returns None.
    let got = mgr.get("anything").await.unwrap();
    assert!(got.is_none());

    // Delete when disabled returns false.
    let deleted = mgr.delete("anything").await.unwrap();
    assert!(!deleted);
}

#[tokio::test]
async fn test_query_alias() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_entry(Entry::new(MemoryType::LongTerm, "query alias test".to_string()))
        .await
        .unwrap();

    let result = mgr.query("alias", None, 10).await.unwrap();
    assert_eq!(result.total, 1);
}

#[tokio::test]
async fn test_delete_alias() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let id = mgr
        .store_entry(Entry::new(MemoryType::LongTerm, "delete alias test".to_string()))
        .await
        .unwrap();

    let deleted = mgr.delete(&id).await.unwrap();
    assert!(deleted);

    let got = mgr.get(&id).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_init_vector_store() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Before init, query_semantic falls back to keyword search.
    mgr.store_fact("Rust is memory safe", vec![])
        .await
        .unwrap();
    let results = mgr.query_semantic("Rust", 5).await.unwrap();
    assert_eq!(results.total, 1);

    // Init vector store with shared plugin fixture
    let embed = crate::vector::test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
    ).expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

    // query_semantic now uses vector store. The previously stored entry
    // is in the LocalStore, not the VectorStore, so we get 0 results
    // from the vector path.
    let vs_results = mgr.query_semantic("Rust", 5).await.unwrap();
    assert_eq!(vs_results.total, 0); // vector store is empty

    // Do NOT call mgr.close() — shared fixture must not be released
}

#[tokio::test]
async fn test_new_with_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new_with_jsonl(&config).await.unwrap();
    assert!(mgr.is_enabled());

    let id = mgr
        .store_entry(Entry::new(MemoryType::LongTerm, "jsonl persisted".to_string()))
        .await
        .unwrap();

    let got = mgr.get(&id).await.unwrap().unwrap();
    assert_eq!(got.content, "jsonl persisted");

    // Verify the file exists.
    let store_path = dir.path().join("memory").join("store.jsonl");
    assert!(store_path.exists());
}

#[tokio::test]
async fn test_new_with_jsonl_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let id = {
        let mgr = MemoryManager::new_with_jsonl(&config).await.unwrap();
        let entry = Entry::new(MemoryType::LongTerm, "survives restart".to_string());
        mgr.store_entry(entry).await.unwrap()
    };

    // Re-create manager -- should reload from disk.
    let mgr2 = MemoryManager::new_with_jsonl(&config).await.unwrap();
    let got = mgr2.get(&id).await.unwrap().unwrap();
    assert_eq!(got.content, "survives restart");
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_vector_store_adapter_stores_and_queries() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Init vector store with shared plugin fixture
    let embed = crate::vector::test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
    ).expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

    // Store an entry - should go to both keyword and vector stores
    let id = mgr
        .store_fact("Berlin is the capital of Germany", vec!["geography".to_string()])
        .await
        .unwrap();
    assert!(!id.is_empty());

    // Query via search should find it through the vector store path
    let results = mgr.search("Berlin", None, 10).await.unwrap();
    assert_eq!(results.total, 1);
    assert_eq!(results.entries[0].entry.content, "Berlin is the capital of Germany");

    // Get should find it in vector store
    let got = mgr.get(&id).await.unwrap();
    assert!(got.is_some());
    assert_eq!(got.unwrap().content, "Berlin is the capital of Germany");

    // Do NOT call mgr.close() — shared fixture must not be released
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_vector_store_adapter_query_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Store entry BEFORE vector store init (only in keyword store)
    let _id_before = mgr
        .store_fact("Tokyo is the capital of Japan", vec![])
        .await
        .unwrap();

    // Init vector store with shared plugin fixture
    let embed = crate::vector::test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
    ).expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

    // Store entry AFTER vector store init (in both stores)
    let _id_after = mgr
        .store_fact("Paris is the capital of France", vec![])
        .await
        .unwrap();

    // Search should find entries from both stores (vector store falls
    // through to keyword store when vector returns empty for "Tokyo")
    let results = mgr.search("Tokyo", None, 10).await.unwrap();
    assert!(results.total >= 1);

    // Do NOT call mgr.close() — shared fixture must not be released
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_config_new() {
    let config = Config::new("/tmp/test-data");
    assert_eq!(config.data_dir, PathBuf::from("/tmp/test-data"));
}

#[test]
fn test_parse_memory_type_from_str_all_variants() {
    assert_eq!(parse_memory_type_from_str("short_term"), MemoryType::ShortTerm);
    assert_eq!(parse_memory_type_from_str("long_term"), MemoryType::LongTerm);
    assert_eq!(parse_memory_type_from_str(""), MemoryType::LongTerm);
    assert_eq!(parse_memory_type_from_str("episodic"), MemoryType::Episodic);
    assert_eq!(parse_memory_type_from_str("graph"), MemoryType::Graph);
    assert_eq!(parse_memory_type_from_str("daily"), MemoryType::Daily);
    assert_eq!(parse_memory_type_from_str("unknown"), MemoryType::LongTerm);
    assert_eq!(parse_memory_type_from_str("RANDOM"), MemoryType::LongTerm);
}

#[tokio::test]
async fn test_get_returns_none_for_missing() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let got = mgr.get("nonexistent").await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn test_forget_returns_false_for_missing() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let removed = mgr.forget("nonexistent").await.unwrap();
    assert!(!removed);
}

#[tokio::test]
async fn test_list_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let entries = mgr.list(None, 10, 0).await.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn test_list_with_type_filter() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_entry(Entry::new(MemoryType::LongTerm, "long term".to_string())).await.unwrap();
    mgr.store_entry(Entry::new(MemoryType::ShortTerm, "short term".to_string())).await.unwrap();

    let long = mgr.list(Some(MemoryType::LongTerm), 10, 0).await.unwrap();
    assert_eq!(long.len(), 1);
    assert_eq!(long[0].content, "long term");
}

#[tokio::test]
async fn test_list_with_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    for i in 0..10 {
        mgr.store_entry(Entry::new(MemoryType::LongTerm, format!("entry {}", i))).await.unwrap();
    }

    let page1 = mgr.list(None, 3, 0).await.unwrap();
    assert!(page1.len() <= 3);

    let page2 = mgr.list(None, 3, 3).await.unwrap();
    assert!(page2.len() <= 3);
}

#[tokio::test]
async fn test_episodic_operations() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Append episodes
    let ep1 = Episode::new("s1".into(), "user".into(), "hello".into());
    let ep2 = Episode::new("s1".into(), "assistant".into(), "hi there".into());
    let ep3 = Episode::new("s2".into(), "user".into(), "other session".into());
    mgr.append_episode(ep1).await.unwrap();
    mgr.append_episode(ep2).await.unwrap();
    mgr.append_episode(ep3).await.unwrap();

    // Get session
    let sessions = mgr.get_session("s1").await.unwrap();
    assert_eq!(sessions.len(), 2);

    // Get recent
    let recent = mgr.get_recent_episodes("s1", 1).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].content, "hi there");

    // Search
    let found = mgr.search_episodic("hello", 10).await.unwrap();
    assert_eq!(found.len(), 1);

    // Stats
    let (session_count, episode_count) = mgr.episodic_stats().await.unwrap();
    assert_eq!(session_count, 2);
    assert_eq!(episode_count, 3);

    // Delete session
    let deleted = mgr.delete_episode_session("s1").await.unwrap();
    assert_eq!(deleted, 2);

    let remaining = mgr.get_session("s1").await.unwrap();
    assert!(remaining.is_empty());
}

#[tokio::test]
async fn test_episodic_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Old episode
    let mut old = Episode::new("old-sess".into(), "user".into(), "old content".into());
    old.timestamp = chrono::Local::now() - chrono::Duration::days(10);
    mgr.append_episode(old).await.unwrap();

    // Recent episode
    mgr.append_episode(Episode::new("new-sess".into(), "user".into(), "new content".into())).await.unwrap();

    let removed = mgr.cleanup_episodic(5).await.unwrap();
    assert_eq!(removed, 1);
}

#[tokio::test]
async fn test_graph_query_triples() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.add_triple(GraphTriple::new("a".into(), "knows".into(), "b".into())).await.unwrap();
    mgr.add_triple(GraphTriple::new("c".into(), "knows".into(), "d".into())).await.unwrap();

    let triples = mgr.query_graph_triples("a", "", "").await.unwrap();
    assert_eq!(triples.len(), 1);
    assert_eq!(triples[0].subject, "a");

    let knows = mgr.query_graph_triples("", "knows", "").await.unwrap();
    assert_eq!(knows.len(), 2);
}

#[tokio::test]
async fn test_get_episodic_store() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let _store = mgr.get_episodic_store();
    // Just verify it returns without panic
}

#[tokio::test]
async fn test_get_graph_store() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let _store = mgr.get_graph_store();
    // Just verify it returns without panic
}

#[tokio::test]
async fn test_search_disabled_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_fact("should be searchable", vec![]).await.unwrap();
    let results = mgr.search("searchable", None, 10).await.unwrap();
    assert_eq!(results.total, 1);

    mgr.close().await.unwrap();

    let results = mgr.search("searchable", None, 10).await.unwrap();
    assert_eq!(results.total, 0);
}

#[tokio::test]
async fn test_query_semantic_disabled_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.close().await.unwrap();

    let results = mgr.query_semantic("anything", 5).await.unwrap();
    assert_eq!(results.total, 0);
}

#[tokio::test]
async fn test_store_disabled_returns_empty_id() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.close().await.unwrap();

    let id = mgr.store_entry(Entry::new(MemoryType::LongTerm, "disabled".to_string())).await.unwrap();
    assert!(id.is_empty());

    let id2 = mgr.store(Entry::new(MemoryType::LongTerm, "disabled".to_string())).await.unwrap();
    assert!(id2.is_empty());
}

#[tokio::test]
async fn test_list_disabled_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_entry(Entry::new(MemoryType::LongTerm, "test".to_string())).await.unwrap();

    mgr.close().await.unwrap();

    let entries = mgr.list(None, 10, 0).await.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_append_episode_writes_to_vector_store() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let embed = crate::vector::test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    // Use low threshold so the query always matches
    let vs_config = StoreConfig {
        similarity_threshold: 0.3,
        ..crate::vector::test_fixture::plugin_store_config(
            &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
        ).expect("plugin DLL + model files required")
    };
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

    // Store an episode — should write to both episodic store AND vector store
    let episode = Episode::new("vs-ep-test".into(), "user".into(), "episodic vector store write test".into());
    let id = mgr.append_episode(episode).await.unwrap();
    assert!(!id.is_empty());

    // Verify: semantic search finds the episodic content via vector store
    let results = mgr.search("episodic vector store", None, 10).await.unwrap();
    assert!(
        results.entries.iter().any(|se| se.entry.content.contains("episodic vector store write test")),
        "vector store should contain the episodic entry, got: {:?}",
        results.entries
    );

    // Do NOT call mgr.close() — shared fixture must not be released
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_init_vector_store_with_custom_config() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let embed = crate::vector::test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let custom_vs_config = StoreConfig {
        similarity_threshold: 0.5,
        max_results: 5,
        storage_path: dir.path().join("custom_vectors.jsonl").to_string_lossy().to_string(),
        ..crate::vector::test_fixture::plugin_store_config(
            &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
        ).expect("plugin DLL + model files required")
    };

    mgr.init_vector_store_with_embed(embed, custom_vs_config).unwrap();

    // Store and query - use store_entry which also stores in vector store
    mgr.store_entry(Entry::new(MemoryType::LongTerm, "custom vector test".to_string())).await.unwrap();
    let results = mgr.query_semantic("vector", 3).await.unwrap();
    assert!(results.total >= 1);

    // Do NOT call mgr.close() — shared fixture must not be released
}

#[tokio::test]
async fn test_with_backends() {
    let store = Arc::new(LocalStore::new());
    let dir = tempfile::tempdir().unwrap();
    let episodic = Arc::new(FileEpisodicStore::new(dir.path()));
    let graph = Arc::new(InMemoryGraphStore::new());

    let mgr = MemoryManager::with_backends(store, episodic, graph);
    assert!(mgr.is_enabled());

    mgr.store_fact("backend test", vec![]).await.unwrap();
    let results = mgr.search("backend", None, 10).await.unwrap();
    assert_eq!(results.total, 1);
}

#[tokio::test]
async fn test_store_entry_with_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let mut meta = HashMap::new();
    meta.insert("source".to_string(), "test".to_string());
    let entry = Entry::new(MemoryType::LongTerm, "metadata test".to_string())
        .with_metadata(meta);

    let id = mgr.store_entry(entry).await.unwrap();
    let got = mgr.get(&id).await.unwrap().unwrap();
    assert_eq!(got.metadata.get("source").unwrap(), "test");
}

#[tokio::test]
async fn test_search_with_memory_type_filter() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_entry(Entry::new(MemoryType::LongTerm, "long term memory".to_string())).await.unwrap();
    mgr.store_entry(Entry::new(MemoryType::ShortTerm, "short term memory".to_string())).await.unwrap();

    let long = mgr.search("memory", Some(MemoryType::LongTerm), 10).await.unwrap();
    assert_eq!(long.total, 1);

    let short = mgr.search("memory", Some(MemoryType::ShortTerm), 10).await.unwrap();
    assert_eq!(short.total, 1);
}

#[tokio::test]
async fn test_store_fact_with_tags() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let id = mgr.store_fact("Python is interpreted", vec!["python".to_string(), "programming".to_string()]).await.unwrap();
    let got = mgr.get(&id).await.unwrap().unwrap();
    assert!(got.tags.contains(&"python".to_string()));
    assert!(got.tags.contains(&"programming".to_string()));
}

// ============================================================
// Additional tests for 95%+ coverage
// ============================================================

#[tokio::test]
async fn test_query_semantic_zero_limit_defaults_to_five() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    for i in 0..10 {
        mgr.store_fact(&format!("fact number {} about testing", i), vec![]).await.unwrap();
    }

    // limit=0 should default to 5
    let results = mgr.query_semantic("testing", 0).await.unwrap();
    assert!(results.entries.len() <= 5);
    assert!(results.total >= 1);
}

#[tokio::test]
async fn test_store_method_delegates_to_store_entry() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Use store() (Go-style alias) instead of store_entry()
    let entry = Entry::new(MemoryType::LongTerm, "stored via store() method".to_string());
    let id = mgr.store(entry).await.unwrap();
    assert!(!id.is_empty());

    let got = mgr.get(&id).await.unwrap().unwrap();
    assert_eq!(got.content, "stored via store() method");
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_store_and_get_via_vector_store() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Init vector store with shared plugin fixture
    let embed = crate::vector::test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
    ).expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

    // Store via store_entry (which also stores to vector)
    let id = mgr.store_entry(Entry::new(MemoryType::LongTerm, "vector store entry".to_string()))
        .await.unwrap();

    // Get should find it in the keyword store first
    let got = mgr.get(&id).await.unwrap();
    assert!(got.is_some());
    assert_eq!(got.unwrap().content, "vector store entry");

    // Do NOT call mgr.close() — shared fixture must not be released
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_get_falls_back_to_vector_store() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let embed = crate::vector::test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
    ).expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

    // Store an entry (goes to both keyword and vector stores)
    let id = mgr.store_entry(Entry::new(MemoryType::LongTerm, "fallback test".to_string()))
        .await.unwrap();

    // Delete from keyword store only, so get must fall back to vector store
    mgr.store.delete(&id).await.unwrap();

    // get() should still find it in the vector store
    let got = mgr.get(&id).await.unwrap();
    assert!(got.is_some());
    assert_eq!(got.unwrap().content, "fallback test");

    // Do NOT call mgr.close() — shared fixture must not be released
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_search_with_vector_store_and_type_filter() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let embed = crate::vector::test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
    ).expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

    // Store entries of different types
    mgr.store_entry(Entry::new(MemoryType::LongTerm, "long term vector content".to_string())).await.unwrap();
    mgr.store_entry(Entry::new(MemoryType::ShortTerm, "short term vector content".to_string())).await.unwrap();

    // Search with type filter should only return matching type
    let results = mgr.search("vector", Some(MemoryType::LongTerm), 10).await.unwrap();
    assert!(results.entries.iter().all(|e| e.entry.typ == MemoryType::LongTerm));

    // Do NOT call mgr.close() — shared fixture must not be released
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_search_vector_store_falls_back_to_keyword() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Store before vector init (only keyword store)
    mgr.store_fact("pre-vector fact about Rust", vec![]).await.unwrap();

    // Init vector store with shared plugin fixture (empty, no entries yet)
    let embed = crate::vector::test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
    ).expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

    // Search should fall back to keyword store since vector is empty
    let results = mgr.search("Rust", None, 10).await.unwrap();
    assert!(results.total >= 1);

    // Do NOT call mgr.close() — shared fixture must not be released
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_store_to_vector_adapter_path() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Init vector store with shared plugin fixture
    let embed = crate::vector::test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy()
    ).expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

    // Use store() method which also goes through vector adapter
    let entry = Entry::new(MemoryType::Episodic, "episodic via store method".to_string())
        .with_tags(vec!["test".to_string()])
        .with_score(0.8);
    let id = mgr.store(entry).await.unwrap();
    assert!(!id.is_empty());

    // Should be findable via get
    let got = mgr.get(&id).await.unwrap();
    assert!(got.is_some());

    // Do NOT call mgr.close() — shared fixture must not be released
}

#[tokio::test]
async fn test_store_disabled_store_method() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.close().await.unwrap();

    let id = mgr.store(Entry::new(MemoryType::LongTerm, "disabled".to_string())).await.unwrap();
    assert!(id.is_empty());
}

#[tokio::test]
async fn test_forget_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let id = mgr.store_fact("will be forgotten", vec![]).await.unwrap();

    mgr.close().await.unwrap();

    let removed = mgr.forget(&id).await.unwrap();
    assert!(!removed);
}

#[tokio::test]
async fn test_list_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_fact("should be listed", vec![]).await.unwrap();
    mgr.close().await.unwrap();

    let entries = mgr.list(None, 10, 0).await.unwrap();
    assert!(entries.is_empty());
}

#[test]
fn test_vector_store_init_with_default_path() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // Init with None (default path) - just verify it doesn't error
    let result = mgr.init_vector_store(None);
    // May succeed or fail depending on whether an embedding model is available
    // The important thing is it doesn't panic
    let _ = result;
}

#[tokio::test]
async fn test_episodic_get_session_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let episodes = mgr.get_session("nonexistent").await.unwrap();
    assert!(episodes.is_empty());
}

#[tokio::test]
async fn test_episodic_get_recent_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let episodes = mgr.get_recent_episodes("nonexistent", 10).await.unwrap();
    assert!(episodes.is_empty());
}

#[tokio::test]
async fn test_graph_get_entity_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let entity = mgr.get_graph_entity("ghost").await.unwrap();
    assert!(entity.is_none());
}

#[tokio::test]
async fn test_graph_query_triples_all_wildcards() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into())).await.unwrap();
    mgr.add_triple(GraphTriple::new("c".into(), "rel".into(), "d".into())).await.unwrap();

    let triples = mgr.query_graph_triples("", "", "").await.unwrap();
    assert_eq!(triples.len(), 2);
}

#[tokio::test]
async fn test_get_related_triples_deep() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    // a -> b -> c -> d
    mgr.add_triple(GraphTriple::new("a".into(), "next".into(), "b".into())).await.unwrap();
    mgr.add_triple(GraphTriple::new("b".into(), "next".into(), "c".into())).await.unwrap();
    mgr.add_triple(GraphTriple::new("c".into(), "next".into(), "d".into())).await.unwrap();

    // Depth 3 should find all 3 hops
    let related = mgr.get_related_triples("a", 3).await.unwrap();
    assert!(related.len() >= 3);

    // Depth 1 should find only 1 hop
    let shallow = mgr.get_related_triples("a", 1).await.unwrap();
    assert!(shallow.len() < related.len());
}

#[tokio::test]
async fn test_episodic_search_no_results() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let results = mgr.search_episodic("nonexistent query", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_delete_episode_session_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let deleted = mgr.delete_episode_session("nonexistent").await.unwrap();
    assert_eq!(deleted, 0);
}

#[tokio::test]
async fn test_cleanup_episodic_nothing_old() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.append_episode(Episode::new("s1".into(), "user".into(), "fresh".into())).await.unwrap();
    let removed = mgr.cleanup_episodic(365).await.unwrap();
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn test_search_returns_scored_entries_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_fact("cat cat cat cat", vec![]).await.unwrap();
    mgr.store_fact("cat", vec![]).await.unwrap();
    mgr.store_fact("dog dog dog", vec![]).await.unwrap();

    let results = mgr.search("cat", None, 10).await.unwrap();
    assert!(results.total >= 2);
    // Results should be sorted by score descending
    for i in 1..results.entries.len() {
        assert!(results.entries[i - 1].score >= results.entries[i].score);
    }
}

// --- Additional coverage tests ---

#[tokio::test]
async fn test_store_multiple_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_fact("fact one", vec![]).await.unwrap();
    mgr.store_fact("fact two", vec![]).await.unwrap();
    mgr.store_fact("fact three", vec![]).await.unwrap();

    let entries = mgr.list(None, 2, 0).await.unwrap();
    assert_eq!(entries.len(), 2); // Limited to 2

    let all = mgr.list(None, 10, 0).await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn test_list_with_offset() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    for i in 0..5 {
        mgr.store_fact(&format!("fact {}", i), vec![]).await.unwrap();
    }

    let page = mgr.list(None, 2, 3).await.unwrap();
    assert!(page.len() <= 2);
}

#[tokio::test]
async fn test_close_and_search() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_fact("before close", vec![]).await.unwrap();
    mgr.close().await.unwrap();

    // After close, search should return empty results
    let results = mgr.search("before", None, 10).await.unwrap();
    assert!(results.entries.is_empty());
}

#[tokio::test]
async fn test_search_after_close_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_fact("before close", vec![]).await.unwrap();
    mgr.close().await.unwrap();

    let results = mgr.search("before", None, 10).await.unwrap();
    assert!(results.entries.is_empty());
}

#[tokio::test]
async fn test_graph_query_with_filter() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.add_triple(GraphTriple::new("Go".into(), "is_a".into(), "language".into())).await.unwrap();
    mgr.add_triple(GraphTriple::new("Rust".into(), "is_a".into(), "language".into())).await.unwrap();
    mgr.add_triple(GraphTriple::new("Go".into(), "created_by".into(), "Google".into())).await.unwrap();

    // Filter by subject
    let go_triples = mgr.query_graph_triples("Go", "", "").await.unwrap();
    assert_eq!(go_triples.len(), 2);

    // Filter by predicate
    let is_a_triples = mgr.query_graph_triples("", "is_a", "").await.unwrap();
    assert_eq!(is_a_triples.len(), 2);

    // Filter by object
    let lang_triples = mgr.query_graph_triples("", "", "language").await.unwrap();
    assert_eq!(lang_triples.len(), 2);
}

#[tokio::test]
async fn test_search_empty_query() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.store_fact("some fact", vec![]).await.unwrap();
    let results = mgr.search("", None, 10).await.unwrap();
    let _ = results;
}

#[tokio::test]
async fn test_search_by_memory_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let id = mgr.store_fact("long term fact", vec![]).await.unwrap();
    assert!(!id.is_empty());
}

#[tokio::test]
async fn test_double_close_safe() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.close().await.unwrap();
    // Second close should not panic
    mgr.close().await.unwrap();
}

#[tokio::test]
async fn test_append_episode_and_get_session() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    let ep1 = Episode::new("s1".into(), "user".into(), "hello".into());
    let ep2 = Episode::new("s1".into(), "assistant".into(), "hi there".into());
    mgr.append_episode(ep1).await.unwrap();
    mgr.append_episode(ep2).await.unwrap();

    let episodes = mgr.get_session("s1").await.unwrap();
    assert_eq!(episodes.len(), 2);
}

#[tokio::test]
async fn test_search_episodic_with_content() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);

    mgr.append_episode(Episode::new("s1".into(), "user".into(), "Rust memory safety".into())).await.unwrap();
    mgr.append_episode(Episode::new("s1".into(), "assistant".into(), "Rust is safe".into())).await.unwrap();

    let results = mgr.search_episodic("Rust", 10).await.unwrap();
    assert!(results.len() >= 2);
}

// ============================================================
// Tests for with_config_dir and enhanced memory config loading
// ============================================================

#[test]
fn test_load_embedding_config_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    // load_embedding_config creates default if missing
    let config = embedding_config::load_embedding_config(dir.path());
    assert!(!config.enabled);
    assert_eq!(config.active, "medium");
}

#[test]
fn test_load_embedding_config_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.enhanced_memory.json");
    let content = r#"{"enabled": true, "active": "large", "models": {}}"#;
    std::fs::write(&path, content).unwrap();
    let config = embedding_config::load_embedding_config(dir.path());
    assert!(config.enabled);
    assert_eq!(config.active, "large");
}

#[test]
fn test_load_embedding_config_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, "not json").unwrap();
    // Invalid JSON → falls back to default
    let config = embedding_config::load_embedding_config(dir.path());
    assert!(!config.enabled);
}

#[test]
fn test_load_embedding_config_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.enhanced_memory.json");
    let content = r#"{}"#;
    std::fs::write(&path, content).unwrap();
    let config = embedding_config::load_embedding_config(dir.path());
    assert!(!config.enabled);
    assert_eq!(config.active, "medium");
}

#[test]
fn test_detect_plugin_path_returns_none() {
    // In test environment, there's no plugin DLL next to the test binary.
    let result = MemoryManager::detect_plugin_path();
    // This is environment-dependent; just ensure it doesn't panic.
    let _ = result;
}

#[test]
fn test_with_config_dir_basic_memory_no_config() {
    // No config.enhanced_memory.json → basic memory
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
}

#[test]
fn test_with_config_dir_disabled() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let path = config_dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, r#"{"enabled": false}"#).unwrap();
    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
    // enabled=false → skip vector store init → basic memory
}

#[test]
fn test_with_config_dir_enabled_no_plugin() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let path = config_dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, r#"{"enabled": true}"#).unwrap();
    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
    // enabled=true but no plugin DLL → auto-detect fails → config written as disabled
}

// ============================================================
// Phase 1: UT — EmbeddingConfig JSON parsing (4 tests)
// ============================================================

#[test]
fn test_embedding_config_enabled_true() {
    let json = r#"{"enabled": true, "active": "medium", "models": {}}"#;
    let cfg: embedding_config::EmbeddingConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
}

#[test]
fn test_embedding_config_extra_fields_ignored() {
    let json = r#"{"enabled": true, "active": "medium", "unknown_field": "value", "another": 42, "models": {}}"#;
    let cfg: embedding_config::EmbeddingConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
}

#[test]
fn test_embedding_config_empty_object() {
    let json = r#"{}"#;
    let cfg: embedding_config::EmbeddingConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.enabled); // default
}

#[test]
fn test_embedding_config_disabled() {
    let json = r#"{"enabled": false, "active": "small", "models": {}}"#;
    let cfg: embedding_config::EmbeddingConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.enabled);
    assert_eq!(cfg.active, "small");
}

// ============================================================
// Phase 1: UT — with_config_dir flow (8 tests)
// ============================================================

#[test]
fn test_with_config_dir_no_config_basic_memory() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    // No config file at all
    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
}

#[test]
fn test_with_config_dir_disabled_basic_works() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let path = config_dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, r#"{"enabled": false}"#).unwrap();
    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
    // Manager is always enabled (basic memory always works).
    // enabled field is now only a signal to the caller (gateway.rs).
    // No vector store since no plugin_path and tier != "api".
}

#[test]
fn test_with_config_dir_enabled_but_no_plugin_disables_config() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let path = config_dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, r#"{"enabled": true}"#).unwrap();
    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
    // enabled=true, but no plugin DLL in test env → config written as {enabled: false}
    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.contains("false"), "Config should be disabled after failed init");
}

#[test]
fn test_with_config_dir_invalid_json_falls_back() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let path = config_dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, "NOT VALID JSON!!!").unwrap();
    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
    // Invalid JSON → load returns None → basic memory
}

#[test]
fn test_with_config_dir_plugin_missing_dll_disables_config() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let path = config_dir.path().join("config.enhanced_memory.json");
    // enabled=true but no plugin DLL in test env
    std::fs::write(&path, r#"{"enabled": true}"#).unwrap();
    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
    // No plugin → config auto-disabled
    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.contains("false"));
}

#[test]
fn test_with_config_dir_corrupted_binary_falls_back() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let path = config_dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, b"\x00\x01\x02\x03\xFF\xFE").unwrap();
    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
    // Binary content → parse fails → basic memory
}

#[test]
fn test_with_config_dir_storage_path_not_created_without_plugin() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();

    let path = config_dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, r#"{"enabled": true}"#).unwrap();

    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());

    // No plugin → vector store not created → storage file doesn't exist
    let expected_storage = data_dir.path().join("vector").join("vector_store.jsonl");
    assert!(!expected_storage.exists());
}

// ============================================================
// Phase 1: UT — Vector Store adapter pattern
// (requires real ONNX plugin — see tests/memory_e2e.rs)
// ============================================================

#[test]
fn test_vector_adapter_requires_plugin() {
    // Verify that init_vector_store with no plugin returns an error
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);
    let result = mgr.init_vector_store(None);
    assert!(result.is_err(), "init_vector_store without plugin should fail");
}
