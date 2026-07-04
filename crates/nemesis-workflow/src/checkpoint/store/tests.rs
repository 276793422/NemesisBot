use super::*;
use chrono::Utc;
use std::collections::{HashMap, HashSet};

fn make_checkpoint(exec_id: &str, id: &str, ts_offset_secs: i64) -> Checkpoint {
    Checkpoint {
        id: id.to_string(),
        execution_id: exec_id.to_string(),
        saved_at: Utc::now() + chrono::Duration::seconds(ts_offset_secs),
        completed_nodes: HashSet::new(),
        waiting_node: None,
        parent_execution_id: None,
        trigger_source: None,
        terminal: false,
        context_snapshot: super::super::types::SerializableContext {
            variables: HashMap::new(),
            node_results: HashMap::new(),
            input: HashMap::new(),
        },
        workflow_hash: "h".to_string(),
    }
}

#[tokio::test]
async fn test_save_and_load() {
    let store = InMemoryCheckpointStore::new();
    let cp = make_checkpoint("exec_a", "cp1", 0);
    let id = store.save(cp.clone()).await.unwrap();
    assert_eq!(id, "cp1");

    let loaded = store.load("exec_a", "cp1").await.unwrap();
    assert_eq!(loaded.id, "cp1");
    assert_eq!(loaded.execution_id, "exec_a");
}

#[tokio::test]
async fn test_load_missing_returns_not_found() {
    let store = InMemoryCheckpointStore::new();
    let err = store.load("nope", "nope").await.unwrap_err();
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[tokio::test]
async fn test_latest_returns_most_recent() {
    let store = InMemoryCheckpointStore::new();
    let cp1 = make_checkpoint("e", "cp1", 0);
    let cp2 = make_checkpoint("e", "cp2", 10);
    let cp3 = make_checkpoint("e", "cp3", 5);
    // Insert out of order — store must sort by saved_at.
    store.save(cp1).await.unwrap();
    store.save(cp2).await.unwrap();
    store.save(cp3).await.unwrap();

    let latest = store.latest("e").await.unwrap().unwrap();
    assert_eq!(latest.id, "cp2");
}

#[tokio::test]
async fn test_latest_missing_execution_returns_none() {
    let store = InMemoryCheckpointStore::new();
    assert!(store.latest("none").await.unwrap().is_none());
}

#[tokio::test]
async fn test_list_returns_oldest_first() {
    let store = InMemoryCheckpointStore::new();
    store.save(make_checkpoint("e", "cp2", 10)).await.unwrap();
    store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
    store.save(make_checkpoint("e", "cp3", 20)).await.unwrap();

    let list = store.list("e").await.unwrap();
    let ids: Vec<_> = list.into_iter().map(|m| m.id).collect();
    assert_eq!(ids, vec!["cp1", "cp2", "cp3"]);
}

#[tokio::test]
async fn test_list_executions_dedup() {
    let store = InMemoryCheckpointStore::new();
    store.save(make_checkpoint("e1", "cp1", 0)).await.unwrap();
    store.save(make_checkpoint("e1", "cp2", 1)).await.unwrap();
    store.save(make_checkpoint("e2", "cp3", 2)).await.unwrap();

    let mut execs = store.list_executions().await.unwrap();
    execs.sort();
    assert_eq!(execs, vec!["e1".to_string(), "e2".to_string()]);
}

#[tokio::test]
async fn test_isolation_between_executions() {
    let store = InMemoryCheckpointStore::new();
    store.save(make_checkpoint("a", "cp_a", 0)).await.unwrap();
    store.save(make_checkpoint("b", "cp_b", 0)).await.unwrap();

    // Cross-execution query must not find other execution's checkpoints.
    let err = store.load("a", "cp_b").await.unwrap_err();
    assert!(matches!(err, StoreError::NotFound { .. }));
    assert!(store.latest("a").await.unwrap().unwrap().id == "cp_a");
    assert!(store.latest("b").await.unwrap().unwrap().id == "cp_b");
}

#[tokio::test]
async fn test_delete_removes_checkpoint() {
    let store = InMemoryCheckpointStore::new();
    store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
    store.delete("e", "cp1").await.unwrap();

    assert!(store.list("e").await.unwrap().is_empty());
    assert!(store.latest("e").await.unwrap().is_none());
}

#[tokio::test]
async fn test_delete_missing_is_ok() {
    let store = InMemoryCheckpointStore::new();
    // Deleting something that was never saved is a no-op.
    store.delete("none", "none").await.unwrap();
}
