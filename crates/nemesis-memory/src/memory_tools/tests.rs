use super::*;

// ===== Approval gate tests =====

/// A gate that denies every store/forget (simulates the user rejecting approval).
struct DenyGate;
#[async_trait::async_trait]
impl MemoryApprovalGate for DenyGate {
    async fn approve_store(&self, _preview: &str) -> bool {
        false
    }
    async fn approve_forget(&self, _preview: &str) -> bool {
        false
    }
}

/// A gate that approves every store/forget.
struct AllowGate;
#[async_trait::async_trait]
impl MemoryApprovalGate for AllowGate {
    async fn approve_store(&self, _preview: &str) -> bool {
        true
    }
    async fn approve_forget(&self, _preview: &str) -> bool {
        true
    }
}

#[tokio::test]
async fn test_store_denied_when_gate_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);
    executor.set_approval_gate(Arc::new(DenyGate));

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({"memory_type": "episodic", "content": "x"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("denied"), "got: {}", result.content);
}

#[tokio::test]
async fn test_forget_denied_when_gate_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);
    executor.set_approval_gate(Arc::new(DenyGate));

    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({"action": "delete_session", "session_key": "s1"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("denied"), "got: {}", result.content);
}

#[tokio::test]
async fn test_store_ungated_when_no_gate_attached() {
    // Backward compat: no gate → store proceeds (must not say "denied").
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({"memory_type": "episodic", "content": "x"}),
        )
        .await;
    assert!(
        !result.content.contains("denied"),
        "ungated store must not be denied: {}",
        result.content
    );
}

#[tokio::test]
async fn test_store_allowed_by_gate_proceeds() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);
    executor.set_approval_gate(Arc::new(AllowGate));

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({"memory_type": "episodic", "content": "x"}),
        )
        .await;
    assert!(
        result.success,
        "allowed store should succeed: {}",
        result.content
    );
}

#[test]
fn test_memory_tool_definitions() {
    let tools = memory_tool_definitions();
    assert_eq!(tools.len(), 4);

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"memory_search"));
    assert!(names.contains(&"memory_store"));
    assert!(names.contains(&"memory_forget"));
    assert!(names.contains(&"memory_list"));
}

#[test]
fn test_tool_result_ok() {
    let result = MemoryToolResult::ok("Found 3 entries");
    assert!(result.success);
    assert_eq!(result.content, "Found 3 entries");
}

#[test]
fn test_tool_result_err() {
    let result = MemoryToolResult::err("Not found");
    assert!(!result.success);
    assert_eq!(result.content, "Not found");
}

#[test]
fn test_tool_definitions_have_required_fields() {
    let tools = memory_tool_definitions();
    for tool in &tools {
        assert!(!tool.name.is_empty());
        assert!(!tool.description.is_empty());
        assert!(tool.parameters.is_object());
    }
}

#[test]
fn test_truncate_text_short() {
    assert_eq!(truncate_text("hello", 10), "hello");
}

#[test]
fn test_truncate_text_long() {
    let result = truncate_text("a very long string that needs truncation", 15);
    assert!(result.len() <= 15);
    assert!(result.len() >= 12); // 15 - 3 for "..."
}

#[test]
fn test_truncate_text_exact() {
    assert_eq!(truncate_text("hello", 5), "hello");
}

#[tokio::test]
async fn test_execute_search_missing_query() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_search", &serde_json::json!({}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("query is required"));
}

#[tokio::test]
async fn test_execute_store_missing_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_store", &serde_json::json!({}))
        .await;
    assert!(!result.success);
    assert!(
        result
            .content
            .contains("content is required for episodic memory")
    );
}

#[tokio::test]
async fn test_execute_forget_missing_action() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_forget", &serde_json::json!({}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("action is required"));
}

#[tokio::test]
async fn test_execute_store_episodic() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "User asked about Rust ownership",
                "role": "user",
                "session_key": "test-session-1"
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("Episodic memory stored"));
}

#[tokio::test]
async fn test_execute_store_graph_entity() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "entity_name": "Rust",
                "entity_type": "language"
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("Entity stored"));
}

#[tokio::test]
async fn test_execute_store_graph_triple() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "triple_subject": "Rust",
                "triple_predicate": "is_a",
                "triple_object": "language",
                "confidence": 0.95
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("Triple stored"));
}

#[tokio::test]
async fn test_execute_list_status() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_list", &serde_json::json!({}))
        .await;
    assert!(result.success);
    assert!(result.content.contains("Memory Store Status"));
}

#[tokio::test]
async fn test_execute_unknown_tool() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("unknown_tool", &serde_json::json!({}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("Unknown memory tool"));
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[tokio::test]
async fn test_execute_search_valid_query() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_search", &serde_json::json!({"query": "test query"}))
        .await;
    assert!(result.success);
    assert!(result.content.contains("test query"));
}

#[tokio::test]
async fn test_execute_search_episodic_only() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_search",
            &serde_json::json!({"query": "test", "memory_type": "episodic"}),
        )
        .await;
    assert!(result.success);
    // When no episodic memories found, it says "No episodic memories found."
    assert!(result.content.contains("episodic"));
}

#[tokio::test]
async fn test_execute_search_graph_only() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_search",
            &serde_json::json!({"query": "test", "memory_type": "graph"}),
        )
        .await;
    assert!(result.success);
    // When no graph entries found, it says "No knowledge graph entries found."
    assert!(result.content.contains("knowledge graph"));
}

#[tokio::test]
async fn test_execute_store_unknown_memory_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({"memory_type": "unknown"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("unknown memory_type"));
}

#[tokio::test]
async fn test_store_episodic_empty_content() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({"memory_type": "episodic", "content": ""}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("content is required"));
}

#[tokio::test]
async fn test_store_episodic_with_tags() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "Tagged content",
                "tags": ["rust", "ownership", "memory"],
                "session_key": "tagged-session"
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("Episodic memory stored"));
}

#[tokio::test]
async fn test_store_graph_no_data_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_store", &serde_json::json!({"memory_type": "graph"}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("entity_name") || result.content.contains("triple"));
}

#[tokio::test]
async fn test_execute_forget_delete_session_missing_key() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({"action": "delete_session"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("session_key is required"));
}

#[tokio::test]
async fn test_execute_forget_cleanup_zero_days_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({"action": "cleanup", "older_than_days": 0}),
        )
        .await;
    assert!(!result.success);
    assert!(
        result
            .content
            .contains("older_than_days must be at least 1")
    );
}

#[tokio::test]
async fn test_execute_forget_unknown_action() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({"action": "unknown_action"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("unknown action"));
}

#[tokio::test]
async fn test_execute_forget_delete_entity_missing_name() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({"action": "delete_entity"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("entity_name is required"));
}

#[tokio::test]
async fn test_execute_list_episodes() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({"list_type": "episodes", "session_key": "test-session"}),
        )
        .await;
    assert!(result.success);
    // No episodes stored yet, should show empty result
}

#[tokio::test]
async fn test_execute_list_unknown_list_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({"list_type": "unknown_type"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("unknown list_type"));
}

#[test]
fn test_memory_tool_result_serialize_deserialize() {
    let ok_result = MemoryToolResult::ok("test content");
    let json = serde_json::to_string(&ok_result).unwrap();
    let deserialized: MemoryToolResult = serde_json::from_str(&json).unwrap();
    assert!(deserialized.success);
    assert_eq!(deserialized.content, "test content");

    let err_result = MemoryToolResult::err("error msg");
    let json = serde_json::to_string(&err_result).unwrap();
    let deserialized: MemoryToolResult = serde_json::from_str(&json).unwrap();
    assert!(!deserialized.success);
    assert_eq!(deserialized.content, "error msg");
}

#[test]
fn test_truncate_text_empty() {
    assert_eq!(truncate_text("", 10), "");
}

#[test]
fn test_truncate_text_small_max() {
    let result = truncate_text("hello", 3);
    assert!(result.len() <= 3);
}

#[test]
fn test_truncate_text_unicode_boundary() {
    let text = "hello world this is a long string";
    let result = truncate_text(text, 10);
    assert!(result.len() <= 10);
}

#[tokio::test]
async fn test_execute_search_with_limit() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_search",
            &serde_json::json!({"query": "test", "limit": 5}),
        )
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_list_episodes_missing_session_key() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_list", &serde_json::json!({"list_type": "episodes"}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("session_key is required"));
}

#[tokio::test]
async fn test_execute_list_graph_related_missing_entity() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({"list_type": "graph_related"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("entity_name is required"));
}

#[tokio::test]
async fn test_store_episodic_auto_session_key() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "Auto session test"
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("manual-"));
}

#[tokio::test]
async fn test_store_graph_entity_with_properties() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "entity_name": "Alice",
                "entity_type": "person",
                "entity_properties": {"age": "30", "city": "Tokyo"}
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("Entity stored"));
}

#[tokio::test]
async fn test_store_graph_triple_with_confidence() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "triple_subject": "Alice",
                "triple_predicate": "works_at",
                "triple_object": "Company",
                "confidence": 0.85
            }),
        )
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_store_graph_triple_zero_confidence_clamped() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "triple_subject": "Bob",
                "triple_predicate": "likes",
                "triple_object": "Tea",
                "confidence": 0.0
            }),
        )
        .await;
    // confidence=0.0 is not in (0.0, 1.0] range, so it's clamped to 1.0
    assert!(result.success);
}

// ============================================================
// Additional coverage tests
// ============================================================

#[tokio::test]
async fn test_execute_store_episodic_with_role() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "Assistant helped with code",
                "role": "assistant",
                "session_key": "role-session"
            }),
        )
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_forget_delete_session_valid() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // First store something
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "test content",
                "session_key": "delete-session"
            }),
        )
        .await;

    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({
                "action": "delete_session",
                "session_key": "delete-session"
            }),
        )
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_forget_cleanup_valid() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({
                "action": "cleanup",
                "older_than_days": 30
            }),
        )
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_forget_delete_entity_valid() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // First store an entity
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "entity_name": "DeleteMe",
                "entity_type": "test"
            }),
        )
        .await;

    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({
                "action": "delete_entity",
                "entity_name": "DeleteMe"
            }),
        )
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_forget_delete_triple_missing_fields() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({
                "action": "delete_triple"
            }),
        )
        .await;
    assert!(!result.success);
}

#[tokio::test]
async fn test_execute_list_graph_query() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store a triple first
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "triple_subject": "Rust",
                "triple_predicate": "is_a",
                "triple_object": "language",
                "confidence": 0.95
            }),
        )
        .await;

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({"list_type": "graph_query", "subject": "Rust"}),
        )
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_list_graph_related() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store entity and triple first
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "entity_name": "Rust",
                "entity_type": "language"
            }),
        )
        .await;
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "triple_subject": "Rust",
                "triple_predicate": "is_a",
                "triple_object": "language"
            }),
        )
        .await;

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({"list_type": "graph_related", "entity_name": "Rust"}),
        )
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_list_episodes_with_data() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store some episodes first
    for i in 0..3 {
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": format!("Episode {}", i),
                    "session_key": "ep-session",
                    "tags": ["test"]
                }),
            )
            .await;
    }

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({
                "list_type": "episodes",
                "session_key": "ep-session",
                "limit": 5
            }),
        )
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_list_graph_query_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({"list_type": "graph_query"}),
        )
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_search_with_stored_data() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store episodic memory
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "Rust ownership model discussion",
                "session_key": "search-session"
            }),
        )
        .await;

    // Search for it
    let result = executor
        .execute("memory_search", &serde_json::json!({"query": "Rust"}))
        .await;
    assert!(result.success);
}

#[test]
fn test_truncate_text_multibyte() {
    // Test with multibyte unicode chars
    let text = "hello world this is a test string for truncation";
    let result = truncate_text(text, 20);
    assert!(result.len() <= 20);
}

#[test]
fn test_memory_tool_serialize() {
    let tool = MemoryTool {
        name: "test".into(),
        description: "desc".into(),
        parameters: serde_json::json!({"type": "object"}),
    };
    let json = serde_json::to_string(&tool).unwrap();
    assert!(json.contains("test"));
}

#[test]
fn test_memory_tool_deserialize() {
    let json = r#"{"name":"x","description":"d","parameters":{"type":"object"}}"#;
    let tool: MemoryTool = serde_json::from_str(json).unwrap();
    assert_eq!(tool.name, "x");
}

#[tokio::test]
async fn test_execute_search_zero_limit() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_search",
            &serde_json::json!({"query": "test", "limit": 0}),
        )
        .await;
    // limit=0 should be treated as default (10)
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_list_episodes_zero_limit() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({
                "list_type": "episodes",
                "session_key": "test",
                "limit": 0
            }),
        )
        .await;
    // limit=0 should be treated as 10
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_list_graph_related_with_depth() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({
                "list_type": "graph_related",
                "entity_name": "Test",
                "depth": 3
            }),
        )
        .await;
    assert!(result.success);
}

// --- Additional coverage tests ---

#[tokio::test]
async fn test_execute_store_episodic_auto_session_key() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // No session_key provided - should generate one with "manual-" prefix
    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "Auto session test"
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("manual-"));
}

#[tokio::test]
async fn test_execute_store_episodic_with_tags() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "Tagged content",
                "session_key": "tag-session",
                "tags": ["rust", "memory", "test"]
            }),
        )
        .await;
    assert!(result.success);

    // Now list episodes to verify tags appear
    let list_result = executor
        .execute(
            "memory_list",
            &serde_json::json!({
                "list_type": "episodes",
                "session_key": "tag-session"
            }),
        )
        .await;
    assert!(list_result.success);
    assert!(list_result.content.contains("rust"));
}

#[tokio::test]
async fn test_execute_store_graph_missing_entity_and_triple() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Neither entity_name nor triple_subject provided
    let result = executor
        .execute("memory_store", &serde_json::json!({"memory_type": "graph"}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("entity_name") || result.content.contains("triple"));
}

#[tokio::test]
async fn test_execute_forget_verify_action_required() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_forget", &serde_json::json!({}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("action is required"));
}

#[tokio::test]
async fn test_execute_forget_delete_entry() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store something first
    let store_result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "Will be forgotten",
                "session_key": "forget-session"
            }),
        )
        .await;
    assert!(store_result.success);

    // Delete the session
    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({
                "action": "delete_session",
                "session_key": "forget-session"
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("deleted"));
}

#[tokio::test]
async fn test_execute_forget_session_missing_key() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({"action": "delete_session"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("session_key is required"));
}

#[tokio::test]
async fn test_execute_list_graph_query_with_results() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store a triple first
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "triple_subject": "Go",
                "triple_predicate": "is_a",
                "triple_object": "language"
            }),
        )
        .await;

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({
                "list_type": "graph_query",
                "subject": "Go"
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("Go"));
}

#[tokio::test]
async fn test_execute_list_status_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_list", &serde_json::json!({"list_type": "status"}))
        .await;
    assert!(result.success);
    assert!(result.content.contains("Memory Store"));
}

#[test]
fn test_truncate_text_boundary_char() {
    let text = "hello world";
    let result = truncate_text(text, 5);
    assert!(result.len() <= 5);
}

#[test]
fn test_memory_tool_result_serialization_roundtrip() {
    let ok_result = MemoryToolResult::ok("test data");
    let json = serde_json::to_string(&ok_result).unwrap();
    let restored: MemoryToolResult = serde_json::from_str(&json).unwrap();
    assert!(restored.success);
    assert_eq!(restored.content, "test data");

    let err_result = MemoryToolResult::err("error msg");
    let json = serde_json::to_string(&err_result).unwrap();
    let restored: MemoryToolResult = serde_json::from_str(&json).unwrap();
    assert!(!restored.success);
}

#[tokio::test]
async fn test_execute_search_graph_with_results() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store a graph entity and triple first
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "entity_name": "Python",
                "entity_type": "language"
            }),
        )
        .await;
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "graph",
                "triple_subject": "Python",
                "triple_predicate": "is_a",
                "triple_object": "language"
            }),
        )
        .await;

    // Search graph specifically
    let result = executor
        .execute(
            "memory_search",
            &serde_json::json!({"query": "Python", "memory_type": "graph"}),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("Python"));
}

#[tokio::test]
async fn test_execute_store_unknown_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({"memory_type": "unknown_type"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("unknown memory_type"));
}

#[tokio::test]
async fn test_execute_store_missing_memory_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_store", &serde_json::json!({}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("required"));
}

#[tokio::test]
async fn test_execute_store_episodic_missing_content() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_store",
            &serde_json::json!({"memory_type": "episodic"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("content is required"));
}

#[tokio::test]
async fn test_execute_list_invalid_list_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute(
            "memory_list",
            &serde_json::json!({"list_type": "unknown_list_type"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("nknown list_type") || result.content.contains("invalid"));
}

#[tokio::test]
async fn test_execute_list_missing_list_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    let result = executor
        .execute("memory_list", &serde_json::json!({}))
        .await;
    // Missing list_type may default to summary
    assert!(result.success || result.content.contains("required"));
}

#[tokio::test]
async fn test_execute_search_all_types() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let executor = MemoryToolExecutor::new(mgr);

    // Store episodic memory
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "Rust ownership discussion",
                "session_key": "search-all"
            }),
        )
        .await;

    // Search with memory_type=all
    let result = executor
        .execute(
            "memory_search",
            &serde_json::json!({"query": "Rust", "memory_type": "all"}),
        )
        .await;
    assert!(result.success);
}

// ============================================================
// Phase 1: UT — Tool executor with vector store (3 tests)
// ============================================================

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn test_execute_store_and_search_with_vector_store() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let embed =
        crate::vector::test_fixture::shared_embed_func().expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy(),
    )
    .expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();
    let executor = MemoryToolExecutor::new(mgr);

    // Store via tool
    executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "vector store roundtrip test content",
                "session_key": "vs-test"
            }),
        )
        .await;

    // Search via tool (default memory_type="all" → includes vector store semantic search)
    let result = executor
        .execute("memory_search", &serde_json::json!({"query": "roundtrip"}))
        .await;
    assert!(result.success, "search should succeed: {}", result.content);
    // Vector store should find the stored episodic content via semantic search
    assert!(
        result.content.contains("vector store roundtrip"),
        "semantic search should find the stored episodic content, got: {}",
        result.content
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
async fn test_execute_list_with_vector_store_entries() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let embed =
        crate::vector::test_fixture::shared_embed_func().expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy(),
    )
    .expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();
    let executor = MemoryToolExecutor::new(mgr);

    // Store multiple entries
    for i in 0..3 {
        executor
            .execute(
                "memory_store",
                &serde_json::json!({
                    "memory_type": "episodic",
                    "content": format!("entry number {}", i),
                    "session_key": "list-test"
                }),
            )
            .await;
    }

    // List status
    let result = executor
        .execute("memory_list", &serde_json::json!({"list_type": "status"}))
        .await;
    assert!(result.success);
    assert!(result.content.contains("Memory Store Status"));

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
async fn test_execute_forget_removes_from_vector_store() {
    let dir = tempfile::tempdir().unwrap();
    let config = crate::manager::Config::new(dir.path());
    let mgr = Arc::new(MemoryManager::new(&config));
    let embed =
        crate::vector::test_fixture::shared_embed_func().expect("shared plugin not available");
    let vs_config = crate::vector::test_fixture::plugin_store_config(
        &dir.path().join("vector").join("vs.jsonl").to_string_lossy(),
    )
    .expect("plugin DLL + model files required");
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();
    let executor = MemoryToolExecutor::new(mgr);

    // Store an entry
    let store_result = executor
        .execute(
            "memory_store",
            &serde_json::json!({
                "memory_type": "episodic",
                "content": "will be forgotten",
                "session_key": "forget-test"
            }),
        )
        .await;
    assert!(store_result.success);

    // Forget the session
    let forget_result = executor
        .execute(
            "memory_forget",
            &serde_json::json!({
                "action": "delete_session",
                "session_key": "forget-test"
            }),
        )
        .await;
    assert!(forget_result.success);

    // Search should no longer find it
    let search_result = executor
        .execute("memory_search", &serde_json::json!({"query": "forgotten"}))
        .await;
    assert!(search_result.success);

    // Do NOT call mgr.close() — shared fixture must not be released
}
