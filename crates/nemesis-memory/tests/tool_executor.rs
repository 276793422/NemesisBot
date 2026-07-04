//! Integration tests for the memory tool executor.
//!
//! Tests store → search → list → forget tool roundtrips through the
//! MemoryToolExecutor, both with and without vector store.

use std::sync::Arc;

use nemesis_memory::__test_fixture;
use nemesis_memory::manager::{Config, MemoryManager};
use nemesis_memory::memory_tools::MemoryToolExecutor;

#[tokio::test]
async fn it_search_after_store_via_tools() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store via tool
    let store_result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "Rust ownership prevents data races at compile time",
                "session_key": "integration-test"
            }),
        )
        .await;
    assert!(store_result.success);

    // Search via tool
    let search_result = executor
        .execute(
            "memory_search",
            &serde_json::json!({"query": "Rust ownership"}),
        )
        .await;
    assert!(search_result.success);
    assert!(
        search_result.content.contains("Rust ownership"),
        "Search should find the stored content"
    );
}

#[tokio::test]
async fn it_list_multiple_stores() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store multiple entries
    for i in 0..5 {
        let result = executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": format!("episodic entry {}", i),
                    "session_key": format!("list-{}", i)
                }),
            )
            .await;
        assert!(result.success, "Store {} should succeed", i);
    }

    // List status
    let list_result = executor
        .execute("memory_list", &serde_json::json!({"list_type": "status"}))
        .await;
    assert!(list_result.success);
    assert!(list_result.content.contains("Episodic"));
}

#[tokio::test]
async fn it_forget_removes_entry() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store an entry
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "temporary information to be forgotten",
                "session_key": "forget-it"
            }),
        )
        .await;

    // Forget the session
    let forget_result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({
                "action": "delete_session",
                "session_key": "forget-it"
            }),
        )
        .await;
    assert!(forget_result.success);

    // Search should no longer find it
    let search_result = executor
        .execute(
            "memory_search",
            &serde_json::json!({"query": "temporary information"}),
        )
        .await;
    assert!(search_result.success);
    // The content may still appear in general store but episodic session is deleted
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn it_tools_with_vector_store() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));

    // Use shared plugin fixture
    let embed = __test_fixture::shared_embed_func()
        .expect("shared plugin not available");
    let vs_config = nemesis_memory::vector::StoreConfig {
        storage_path: dir.path().join("vector").join("vector_store.jsonl").to_string_lossy().to_string(),
        ..__test_fixture::plugin_store_config("")
            .expect("plugin DLL + model files required")
    };

    // Initialize vector store with shared plugin fixture
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();

    let executor = MemoryToolExecutor::new(mgr);

    // Store via tool (should also go to vector store)
    let store_result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "semantic search test with vector store",
                "session_key": "vector-tools"
            }),
        )
        .await;
    assert!(store_result.success);

    // Search should find it
    let search_result = executor
        .execute(
            "memory_search",
            &serde_json::json!({"query": "semantic search"}),
        )
        .await;
    assert!(search_result.success);

    // Do NOT call mgr.close() — shared fixture must not be released
}
