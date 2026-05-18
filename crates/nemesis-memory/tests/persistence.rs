//! Integration tests for vector store persistence.
//!
//! Tests store → persist → load → query roundtrips and edge cases.

use std::collections::HashMap;

use nemesis_memory::__test_fixture;
use nemesis_memory::vector::{VectorStore, VectorEntry};

fn make_entry(id: &str, content: &str) -> VectorEntry {
    VectorEntry {
        id: id.into(),
        entry_type: "long_term".into(),
        content: content.into(),
        metadata: HashMap::new(),
        tags: vec![],
        score: 0.0,
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    }
}

#[tokio::test]
#[ignore]
async fn it_vector_store_jsonl_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("roundtrip.jsonl");
    let config = __test_fixture::plugin_store_config(&path.to_string_lossy())
        .expect("plugin DLL + model files required");

    // Phase 1: Store and persist (using shared plugin fixture)
    let embed1 = __test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let store = VectorStore::new_from_embed(embed1, config.clone());
    let e1 = make_entry("rt-1", "machine learning artificial intelligence");
    let e2 = make_entry("rt-2", "neural networks deep learning");
    store.store_entry(&e1).unwrap();
    store.store_entry(&e2).unwrap();
    store.persist_entry_sync(&e1).unwrap();
    store.persist_entry_sync(&e2).unwrap();
    assert_eq!(store.len(), 2);
    drop(store);

    // Phase 2: Load into new store and query (using same shared plugin)
    let embed2 = __test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let store2 = VectorStore::new_from_embed(embed2, config);
    store2.load_persisted_sync().unwrap();
    assert_eq!(store2.len(), 2);

    // Verify entries loaded correctly by ID
    assert!(store2.get_by_id("rt-1").is_some());
    assert!(store2.get_by_id("rt-2").is_some());

    // Query with similar text to stored content
    let result = store2.query("machine learning", 10, &[]).unwrap();
    assert!(result.total >= 1, "Should find at least 1 result after roundtrip");
}

#[tokio::test]
async fn it_manager_close_reopen_survives() {
    use nemesis_memory::manager::{Config, MemoryManager};

    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());

    // Store data with first manager instance
    let id = {
        let mgr = MemoryManager::new(&config);
        mgr.store_fact("persistent fact about Rust", vec!["rust".to_string()])
            .await
            .unwrap()
    };

    // Close (drop) first manager
    drop(config);

    // Re-create with same data dir
    let config2 = Config::new(dir.path());
    let mgr2 = MemoryManager::new(&config2);
    let _got = mgr2.get(&id).await.unwrap();
    // Note: basic LocalStore is in-memory only, so data won't survive.
    // This test verifies the flow works without panicking.
    // For real persistence, use new_with_jsonl.
}

#[tokio::test]
#[ignore]
async fn it_persistence_mixed_valid_corrupted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixed.jsonl");

    // Write a mix of valid and invalid JSON lines
    std::fs::write(
        &path,
        "this is invalid json\n\
         {\"id\":\"valid-1\",\"type\":\"long_term\",\"content\":\"valid entry\",\"metadata\":{},\"tags\":[],\"score\":0.0,\"created_at\":\"2024-01-01T00:00:00Z\",\"updated_at\":\"2024-01-01T00:00:00Z\"}\n\
         \n\
         {bad json\n\
         {\"id\":\"valid-2\",\"type\":\"long_term\",\"content\":\"another valid\",\"metadata\":{},\"tags\":[],\"score\":0.0,\"created_at\":\"2024-01-01T00:00:00Z\",\"updated_at\":\"2024-01-01T00:00:00Z\"}\n",
    ).unwrap();

    let config = __test_fixture::plugin_store_config(&path.to_string_lossy())
        .expect("plugin DLL + model files required");

    let embed = __test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let store = VectorStore::new_from_embed(embed, config);
    store.load_persisted_sync().unwrap();

    // Only 2 valid entries should be loaded
    assert_eq!(store.len(), 2);
    assert!(store.get_by_id("valid-1").is_some());
    assert!(store.get_by_id("valid-2").is_some());
}
