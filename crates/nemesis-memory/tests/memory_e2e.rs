//! Full E2E test for the enhanced memory pipeline.
//!
//! Simulates the gateway flow:
//!   1. Create MemoryManager with basic memory
//!   2. Initialize vector store with explicit plugin path (same as with_config_dir)
//!   3. Use MemoryToolExecutor to store and search memories (episodic + graph)
//!   4. Verify keyword and graph search find stored content
//!
//! Run with:
//!   cargo test -p nemesis-memory --test memory_e2e -- --ignored --test-threads=1
//!
//! Requirements:
//!   1. plugin_onnx.dll: built at plugins/plugin-onnx/target/release/
//!   2. Model files: test-data/memory-e2e/model.onnx + tokenizer.json

use std::sync::Arc;

use nemesis_memory::__test_fixture;
use nemesis_memory::manager::{Config, MemoryManager};
use nemesis_memory::memory_tools::MemoryToolExecutor;
use nemesis_memory::vector::StoreConfig;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn e2e_gateway_memory_pipeline() {
    let data_dir = tempfile::tempdir().unwrap();

    // Step 1: Create MemoryManager (basic memory)
    let memory_data_dir = data_dir.path().join("memory");
    let mgr_config = Config::new(&memory_data_dir);
    let mgr = Arc::new(MemoryManager::new(&mgr_config));
    assert!(mgr.is_enabled());
    println!("[Step 1] MemoryManager created");

    // Step 2: Initialize vector store with shared plugin fixture
    let storage_path = memory_data_dir.join("vector").join("vector_store.jsonl");
    let embed = __test_fixture::shared_embed_func().expect("shared plugin not available");
    let store_config = StoreConfig {
        similarity_threshold: 0.7,
        storage_path: storage_path.to_string_lossy().to_string(),
        ..__test_fixture::plugin_store_config("").expect("plugin DLL + model files required")
    };
    mgr.init_vector_store_with_embed(embed, store_config)
        .expect("Vector store init should succeed");
    println!("[Step 2] Vector store initialized");

    // Step 3: Create tool executor (same as gateway.rs)
    let executor = Arc::new(MemoryToolExecutor::new(mgr));
    println!("[Step 3] MemoryToolExecutor created");

    // Step 4: Store episodic memories via tools
    let store_result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "My name is Alice and I love programming in Rust. I work on AI systems.",
                "session_key": "e2e-test-session"
            }),
        )
        .await;
    assert!(
        store_result.success,
        "Store should succeed: {}",
        store_result.content
    );
    println!("[Step 4] Memory stored: {}", store_result.content);

    let store_result2 = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "The Rust programming language provides memory safety without garbage collection.",
                "session_key": "e2e-test-session-2"
            }),
        )
        .await;
    assert!(store_result2.success, "Second store should succeed");
    println!("[Step 4b] Second memory stored");

    // Step 5: Keyword search for episodic memories
    let search_result = executor
        .execute(
            "memory_search",
            &serde_json::json!({
                "query": "Alice",
                "limit": 5
            }),
        )
        .await;
    assert!(
        search_result.success,
        "Search should succeed: {}",
        search_result.content
    );
    println!("[Step 5] Search result:\n{}", search_result.content);
    assert!(
        search_result.content.to_lowercase().contains("alice"),
        "Search should find content containing 'Alice'"
    );

    // Step 6: Store a knowledge graph entry
    let graph_store = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "triple_subject": "Alice",
                "triple_predicate": "loves",
                "triple_object": "programming in Rust",
                "confidence": 0.95
            }),
        )
        .await;
    assert!(
        graph_store.success,
        "Graph store should succeed: {}",
        graph_store.content
    );
    println!("[Step 6] Graph stored: {}", graph_store.content);

    // Step 7: Search knowledge graph
    let graph_search = executor
        .execute(
            "memory_search",
            &serde_json::json!({
                "query": "Alice",
                "memory_type": "graph",
                "limit": 5
            }),
        )
        .await;
    assert!(graph_search.success, "Graph search should succeed");
    println!("[Step 7] Graph search result:\n{}", graph_search.content);
    assert!(
        graph_search.content.contains("Alice") && graph_search.content.contains("programming"),
        "Graph search should find Alice -> loves -> programming"
    );

    // Step 8: Search by Rust keyword across all memory types
    let search_rust = executor
        .execute(
            "memory_search",
            &serde_json::json!({
                "query": "Rust",
                "limit": 5
            }),
        )
        .await;
    assert!(search_rust.success, "Rust search should succeed");
    println!("[Step 8] Rust search:\n{}", search_rust.content);
    assert!(
        search_rust.content.to_lowercase().contains("rust"),
        "Should find Rust-related memories"
    );

    // Step 9: List memory status
    let list_result = executor
        .execute("memory_list", &serde_json::json!({"list_type": "status"}))
        .await;
    assert!(list_result.success, "List should succeed");
    assert!(
        list_result.content.contains("Episodic"),
        "Should show episodic store status"
    );
    println!("[Step 9] Memory status:\n{}", list_result.content);

    // Do NOT call mgr.close() — shared fixture must not be released

    println!("\nFull E2E memory pipeline test PASSED!");
}

#[tokio::test]
async fn e2e_disabled_memory_no_executor() {
    // Verify that when memory.enabled = false in config.json,
    // gateway.rs would not create MemoryManager
    use nemesis_types::config::AppConfig;
    let json = r#"{"memory": {"enabled": false}}"#;
    let cfg: AppConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.memory.enabled, "memory.enabled should be false");
    println!("[Disabled test] memory.enabled = false -> gateway would NOT create MemoryManager");
}

#[tokio::test]
async fn e2e_enabled_via_nemesis_config() {
    // Verify that nemesis-config's MemoryFlagConfig works correctly
    use nemesis_config::Config;
    let json = r#"{"memory": {"enabled": true}}"#;
    let cfg: Config = serde_json::from_str(json).unwrap();
    let memory_enabled = cfg.memory.as_ref().map(|m| m.enabled).unwrap_or(false);
    assert!(
        memory_enabled,
        "memory.enabled should be true via nemesis-config"
    );
    println!("[Enabled test] memory.enabled = true via nemesis-config");
}
