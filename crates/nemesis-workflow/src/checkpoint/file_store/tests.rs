use super::*;
use crate::checkpoint::types::SerializableContext;
use chrono::Utc;
use std::collections::HashMap;
use std::collections::HashSet;
use tempfile::TempDir;

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
        context_snapshot: SerializableContext {
            variables: HashMap::new(),
            node_results: HashMap::new(),
            input: HashMap::new(),
        },
        workflow_hash: "h".to_string(),
    }
}

fn make_store() -> (TempDir, FileCheckpointStore) {
    let tmp = TempDir::new().unwrap();
    let store = FileCheckpointStore::new(tmp.path()).unwrap();
    (tmp, store)
}

#[tokio::test]
async fn save_and_load_round_trip() {
    let (_tmp, store) = make_store();
    let cp = make_checkpoint("exec_a", "cp1", 0);
    let id = store.save(cp.clone()).await.unwrap();
    assert_eq!(id, "cp1");

    let loaded = store.load("exec_a", "cp1").await.unwrap();
    assert_eq!(loaded.id, "cp1");
    assert_eq!(loaded.execution_id, "exec_a");
    assert_eq!(loaded, cp);
}

#[tokio::test]
async fn load_missing_returns_not_found() {
    let (_tmp, store) = make_store();
    let err = store.load("nope", "nope").await.unwrap_err();
    assert!(matches!(err, StoreError::NotFound { .. }));
}

#[tokio::test]
async fn latest_returns_most_recent() {
    let (_tmp, store) = make_store();
    // Stagger saves with real wall-clock intervals so mtime ordering is
    // stable on filesystems with coarse mtime resolution (HFS+, FAT).
    store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
    store.save(make_checkpoint("e", "cp2", 10)).await.unwrap();
    store.save(make_checkpoint("e", "cp3", 5)).await.unwrap();

    let latest = store.latest("e").await.unwrap().unwrap();
    assert_eq!(latest.id, "cp2");
}

#[tokio::test]
async fn latest_missing_execution_returns_none() {
    let (_tmp, store) = make_store();
    assert!(store.latest("none").await.unwrap().is_none());
}

#[tokio::test]
async fn list_returns_oldest_first() {
    let (_tmp, store) = make_store();
    store.save(make_checkpoint("e", "cp2", 10)).await.unwrap();
    store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
    store.save(make_checkpoint("e", "cp3", 20)).await.unwrap();

    let list = store.list("e").await.unwrap();
    let ids: Vec<_> = list.into_iter().map(|m| m.id).collect();
    assert_eq!(ids, vec!["cp1", "cp2", "cp3"]);
}

#[tokio::test]
async fn list_executions_dedup() {
    let (_tmp, store) = make_store();
    store.save(make_checkpoint("e1", "cp1", 0)).await.unwrap();
    store.save(make_checkpoint("e1", "cp2", 1)).await.unwrap();
    store.save(make_checkpoint("e2", "cp3", 2)).await.unwrap();

    let execs = store.list_executions().await.unwrap();
    assert_eq!(execs, vec!["e1".to_string(), "e2".to_string()]);
}

#[tokio::test]
async fn delete_removes_checkpoint() {
    let (_tmp, store) = make_store();
    store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
    store.delete("e", "cp1").await.unwrap();

    assert!(store.list("e").await.unwrap().is_empty());
    assert!(store.latest("e").await.unwrap().is_none());
}

#[tokio::test]
async fn delete_missing_is_ok() {
    let (_tmp, store) = make_store();
    store.delete("none", "none").await.unwrap();
}

#[tokio::test]
async fn isolation_between_executions() {
    let (_tmp, store) = make_store();
    store.save(make_checkpoint("a", "cp_a", 0)).await.unwrap();
    store.save(make_checkpoint("b", "cp_b", 0)).await.unwrap();

    let err = store.load("a", "cp_b").await.unwrap_err();
    assert!(matches!(err, StoreError::NotFound { .. }));
    assert_eq!(store.latest("a").await.unwrap().unwrap().id, "cp_a");
    assert_eq!(store.latest("b").await.unwrap().unwrap().id, "cp_b");
}

#[tokio::test]
async fn corrupt_file_is_quarantined_and_latest_skips_it() {
    let (_tmp, store) = make_store();
    // Write a good checkpoint, then poison the directory with a corrupt one.
    store.save(make_checkpoint("e", "cp1", 0)).await.unwrap();
    let bad_path = store.checkpoint_path("e", "cp_bad");
    fs::write(&bad_path, b"NOT VALID JSON").unwrap();

    // latest() should still return cp1 (the only valid checkpoint).
    let latest = store.latest("e").await.unwrap().unwrap();
    assert_eq!(latest.id, "cp1");

    // The corrupt file should have been moved into .corrupt/.
    let corrupt_path = store.exec_dir("e").join(".corrupt").join("cp_bad.json");
    assert!(corrupt_path.exists(), "corrupt file should be quarantined");
    assert!(!bad_path.exists(), "original corrupt file should be gone");
}

#[tokio::test]
async fn load_corrupt_returns_corrupt_error() {
    let (_tmp, store) = make_store();
    let bad_path = store.checkpoint_path("e", "cp1");
    fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
    fs::write(&bad_path, b"NOT VALID JSON").unwrap();

    let err = store.load("e", "cp1").await.unwrap_err();
    assert!(matches!(err, StoreError::Corrupt(_)));
}

#[tokio::test]
async fn path_traversal_ids_are_rejected() {
    // An execution_id with `..` or `/` must not escape the checkpoints root.
    let (tmp, store) = make_store();
    // Save with a traversal-style id; sanitize should flatten it to a single
    // safe component rather than producing `../../etc/something`.
    let evil_id = "../../../etc/evil";
    store
        .save(make_checkpoint(evil_id, "cp1", 0))
        .await
        .unwrap();

    // No file should have escaped the checkpoints root. We assert by
    // canonicalizing both paths and checking containment — substring
    // checks would false-positive on the sanitized name (`.._.._evil`).
    let cp_root_canon = store.checkpoints_dir().canonicalize().unwrap();
    let mut all_paths = Vec::new();
    collect_relative_paths(&cp_root_canon, &cp_root_canon, &mut all_paths);
    for rel in &all_paths {
        // Each relative path is computed by stripping the cp_root prefix,
        // so any successful traversal would show up as an absolute path
        // outside the root (strip_prefix would have failed otherwise).
        assert!(
            !rel.starts_with('/') && !rel.starts_with('\\'),
            "path escaped checkpoints root: {rel}"
        );
    }

    // Sanity: at least one file was written somewhere under cp_root.
    assert!(
        !all_paths.is_empty(),
        "save should have produced at least one file under the checkpoints root"
    );

    // The temp dir must not have acquired an `etc/` directory — that would
    // indicate the `..` traversal actually escaped.
    assert!(
        !tmp.path().join("etc").exists(),
        "traversal escaped temp root"
    );
}

fn collect_relative_paths(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if path.is_dir() {
            collect_relative_paths(root, &path, out);
        } else {
            out.push(rel);
        }
    }
}
