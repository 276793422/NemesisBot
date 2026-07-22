//! Integration tests for real ONNX plugin.
//!
//! These tests use the shared test fixture so the ONNX plugin is initialized
//! only once per process. Run with:
//!   cargo test -p nemesis-memory --test real_plugin -- --ignored --test-threads=1
//!
//! Requirements:
//!   1. plugin_onnx.dll: cd plugins/plugin-onnx && cargo build --release
//!   2. Test model:       bash test-tools/plugin-onnx-test/scripts/setup-test.sh

use std::collections::HashMap;

use nemesis_memory::__test_fixture;
use nemesis_memory::vector::{StoreConfig, VectorEntry, VectorStore};

fn make_entry(id: &str, content: &str) -> VectorEntry {
    VectorEntry {
        id: id.into(),
        entry_type: "long_term".into(),
        content: content.into(),
        metadata: HashMap::new(),
        tags: vec![],
        score: 0.0,
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
    }
}

#[test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
fn it_onnx_embedding_pipeline() {
    let embed = __test_fixture::shared_embed_func().expect("shared plugin not available");
    let store_config =
        __test_fixture::plugin_store_config("").expect("plugin DLL + model files required");
    let store = VectorStore::new_from_embed(embed, store_config);

    // Store an entry and verify embedding dimension
    store
        .store_entry(&make_entry("onnx-1", "hello world"))
        .unwrap();
    assert_eq!(store.len(), 1);

    // Query should work and produce results
    let result = store.query("hello", 10, &[]).unwrap();
    assert!(result.total >= 1, "Should find result for 'hello'");
    assert!(result.entries[0].score > 0.0, "Score should be positive");
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn it_onnx_vector_store_with_manager() {
    use nemesis_memory::manager::{Config, MemoryManager};
    use nemesis_memory::types::{Entry, MemoryType};
    use std::sync::Arc;

    let data_dir = tempfile::tempdir().unwrap();
    let config = Config::new(data_dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));

    // Initialize vector store with shared plugin fixture
    let embed = __test_fixture::shared_embed_func().expect("shared plugin not available");
    let store_config = StoreConfig {
        similarity_threshold: 0.1,
        storage_path: data_dir
            .path()
            .join("vector")
            .join("store.jsonl")
            .to_string_lossy()
            .to_string(),
        ..__test_fixture::plugin_store_config("").expect("plugin DLL + model files required")
    };
    mgr.init_vector_store_with_embed(embed, store_config)
        .unwrap();

    // Use store_entry (not store_fact) to also store in vector store
    let id = mgr
        .store_entry(Entry::new(
            MemoryType::LongTerm,
            "Cats are independent animals".to_string(),
        ))
        .await
        .unwrap();
    assert!(!id.is_empty());

    // Search for it semantically — "feline pets" should match "Cats are independent animals"
    let results = mgr.search("feline pets", None, 10).await.unwrap();
    assert!(
        results.total >= 1,
        "Should find semantically related result, got {}",
        results.total
    );
    assert_eq!(
        results.entries[0].entry.content,
        "Cats are independent animals"
    );

    // Do NOT call mgr.close() — shared fixture must not be released
}
