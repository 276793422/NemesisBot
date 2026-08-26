use super::*;

fn make_task(task_id: &str, status: TaskStatus) -> ClusterTask {
    ClusterTask {
        task_id: task_id.to_string(),
        source: TaskSource {
            node_id: "node-a".to_string(),
            rpc_address: "127.0.0.1:9000".to_string(),
            session_key: "sess-1".to_string(),
        },
        status,
        content: "hello".to_string(),
        conversation: None,
        waiting_for_task_id: None,
        waiting_tool_call_id: None,
        callback_result: None,
    }
}

#[test]
fn test_create_and_get_task() {
    let list = ClusterTaskList::new(std::env::temp_dir());
    let task = make_task("t1", TaskStatus::Pending);
    list.create_task(task);

    let got = list.get_task("t1").unwrap();
    assert_eq!(got.task_id, "t1");
    assert_eq!(got.status, TaskStatus::Pending);
    assert!(list.get_task("nonexistent").is_none());
}

#[test]
fn test_save_async_state_and_find() {
    let list = ClusterTaskList::new(std::env::temp_dir());
    list.create_task(make_task("t1", TaskStatus::Running));

    list.save_async_state(
        "t1",
        "child-123".to_string(),
        "tc_abc".to_string(),
        serde_json::json!([{"role": "user", "content": "hi"}]),
    );

    let found = list.find_by_child_task_id("child-123").unwrap();
    assert_eq!(found, "t1");

    let task = list.get_task("t1").unwrap();
    assert_eq!(task.status, TaskStatus::WaitingRemote);
    assert_eq!(task.waiting_for_task_id.unwrap(), "child-123");
    assert_eq!(task.waiting_tool_call_id.unwrap(), "tc_abc");
    assert!(task.conversation.is_some());
    assert!(list.find_by_child_task_id("nonexistent").is_none());
}

#[test]
fn test_inject_callback() {
    let list = ClusterTaskList::new(std::env::temp_dir());
    list.create_task(make_task("t1", TaskStatus::Running));
    list.save_async_state(
        "t1",
        "child-123".to_string(),
        "tc_abc".to_string(),
        serde_json::json!([{"role": "user", "content": "hi"}]),
    );

    list.inject_callback("t1", "response from remote");

    let task = list.get_task("t1").unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(task.callback_result.unwrap(), "response from remote");
    assert!(task.waiting_for_task_id.is_none());
}

#[test]
fn test_complete_task() {
    let list = ClusterTaskList::new(std::env::temp_dir());
    list.create_task(make_task("t1", TaskStatus::Running));
    list.complete_task("t1");
    assert!(list.get_task("t1").is_none());
}

#[test]
fn test_persist_and_restore() {
    let dir = std::env::temp_dir().join("cluster_test_persist");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    {
        let list = ClusterTaskList::new(&dir);
        list.create_task(make_task("t1", TaskStatus::Pending));
        list.save_async_state(
            "t1",
            "child-1".to_string(),
            "tc_1".to_string(),
            serde_json::json!([{"role": "user", "content": "hello"}]),
        );
        list.persist_to_disk().unwrap();
    }

    let list2 = ClusterTaskList::new(&dir);
    list2.restore_from_disk().unwrap();

    let task = list2.get_task("t1").unwrap();
    assert_eq!(task.task_id, "t1");
    assert_eq!(task.status, TaskStatus::WaitingRemote);
    assert_eq!(task.waiting_for_task_id.unwrap(), "child-1");

    let _ = std::fs::remove_dir_all(&dir);
}

// -------------------------------------------------------------------------
// Additional unit tests
// -------------------------------------------------------------------------

#[test]
fn test_recover_resets_waiting_remote() {
    let dir = std::env::temp_dir().join("cluster_test_recover_waiting");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let list = ClusterTaskList::new(&dir);
    list.create_task(make_task("t-wr", TaskStatus::Running));
    list.save_async_state(
        "t-wr",
        "child-x".to_string(),
        "tc_x".to_string(),
        serde_json::json!([{"role": "user", "content": "test"}]),
    );

    let task = list.get_task("t-wr").unwrap();
    assert_eq!(task.status, TaskStatus::WaitingRemote);

    let recovered = list.recover_task_ids();
    assert_eq!(recovered, vec!["t-wr".to_string()]);

    let task = list.get_task("t-wr").unwrap();
    assert_eq!(task.status, TaskStatus::Pending);

    let list2 = ClusterTaskList::new(&dir);
    list2.restore_from_disk().unwrap();
    let restored = list2.get_task("t-wr").unwrap();
    assert_eq!(restored.task_id, "t-wr");
    assert_eq!(restored.status, TaskStatus::Pending);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_recover_skips_completed_failed() {
    let dir = std::env::temp_dir().join("cluster_test_recover_skip");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let list = ClusterTaskList::new(&dir);
    list.create_task(make_task("t-done", TaskStatus::Running));
    list.create_task(make_task("t-fail", TaskStatus::Running));

    list.complete_task("t-done");
    list.update_status("t-fail", TaskStatus::Failed);

    let recovered = list.recover_task_ids();
    assert!(
        recovered.is_empty(),
        "Completed and Failed tasks should not be recovered"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_save_async_state_clears_old_child() {
    let dir = std::env::temp_dir().join("cluster_test_clear_old_child");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let list = ClusterTaskList::new(&dir);
    list.create_task(make_task("t-chain", TaskStatus::Running));

    list.save_async_state(
        "t-chain",
        "child-1".to_string(),
        "tc-1".to_string(),
        serde_json::json!([{"role": "user", "content": "hop1"}]),
    );
    assert_eq!(list.find_by_child_task_id("child-1").unwrap(), "t-chain");

    list.inject_callback("t-chain", "result-1");
    assert!(list.find_by_child_task_id("child-1").is_none());

    list.save_async_state(
        "t-chain",
        "child-2".to_string(),
        "tc-2".to_string(),
        serde_json::json!([{"role": "user", "content": "hop2"}]),
    );

    assert!(list.find_by_child_task_id("child-1").is_none());
    assert_eq!(list.find_by_child_task_id("child-2").unwrap(), "t-chain");

    let task = list.get_task("t-chain").unwrap();
    assert_eq!(task.waiting_for_task_id.unwrap(), "child-2");
    assert_eq!(task.waiting_tool_call_id.unwrap(), "tc-2");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_work_queue_fifo_ordering() {
    let queue = ClusterWorkQueue::new(10);

    queue.submit("t1".to_string()).unwrap();
    queue.submit("t2".to_string()).unwrap();
    queue.submit("t3".to_string()).unwrap();

    assert_eq!(queue.next().await.unwrap(), "t1");
    assert_eq!(queue.next().await.unwrap(), "t2");
    assert_eq!(queue.next().await.unwrap(), "t3");
}

#[tokio::test]
async fn test_work_queue_returns_none_on_close() {
    use std::time::Duration;

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(2);
    tx.send("last".to_string()).await.unwrap();
    drop(tx);

    let (dummy_tx, _) = tokio::sync::mpsc::channel::<String>(1);
    let queue = ClusterWorkQueue {
        tx: dummy_tx,
        rx: Mutex::new(rx),
    };

    assert_eq!(queue.next().await.unwrap(), "last");

    let result = tokio::time::timeout(Duration::from_secs(2), queue.next()).await;
    assert!(result.is_ok(), "next() should return quickly, not hang");
    assert!(
        result.unwrap().is_none(),
        "Expected None when all senders are dropped"
    );
}

#[test]
fn test_crash_recovery_restores_conversation() {
    let dir = std::env::temp_dir().join("cluster_test_crash_recovery");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let conversation = serde_json::json!([
        {"role": "system", "content": "You are a helpful assistant."},
        {"role": "user", "content": "What is the weather in Tokyo?"},
        {"role": "assistant", "content": "Let me check that for you."},
        {"role": "tool", "content": "Sunny, 25°C"}
    ]);

    {
        let list = ClusterTaskList::new(&dir);
        list.create_task(make_task("t-crash", TaskStatus::Running));
        list.save_async_state(
            "t-crash",
            "child-crash".to_string(),
            "tc_crash".to_string(),
            conversation.clone(),
        );
    }

    let list2 = ClusterTaskList::new(&dir);
    list2.restore_from_disk().unwrap();

    let task = list2.get_task("t-crash").unwrap();
    assert_eq!(task.task_id, "t-crash");
    assert_eq!(task.status, TaskStatus::WaitingRemote);
    assert_eq!(task.waiting_for_task_id.unwrap(), "child-crash");
    assert_eq!(task.waiting_tool_call_id.unwrap(), "tc_crash");

    let restored_conv = task.conversation.expect("conversation should be restored");
    assert_eq!(restored_conv, conversation);
    assert_eq!(restored_conv.as_array().unwrap().len(), 4);

    let recovered = list2.recover_task_ids();
    assert_eq!(recovered, vec!["t-crash".to_string()]);

    let _ = std::fs::remove_dir_all(&dir);
}

// ============================================================
// W3b coverage batch: Display variants, queue sender/full,
// persistence failure warns (data_dir-as-file, conversation
// delete failure), restore error arms (read/parse), terminal
// status skip on restore, corrupt conversation .ok() fallback,
// tasks.json write failure.
// ============================================================

#[test]
fn test_w3b_task_status_display_all_variants() {
    assert_eq!(TaskStatus::Pending.to_string(), "pending");
    assert_eq!(TaskStatus::Running.to_string(), "running");
    assert_eq!(TaskStatus::WaitingRemote.to_string(), "waiting_remote");
    assert_eq!(TaskStatus::Completed.to_string(), "completed");
    assert_eq!(TaskStatus::Failed.to_string(), "failed");
}

#[tokio::test]
async fn test_w3b_work_queue_sender_clone_and_capacity_zero_submit() {
    let queue = ClusterWorkQueue::new(4);

    // sender() yields a clonable handle that feeds the same queue
    let sender = queue.sender();
    sender.send("via-sender".to_string()).await.unwrap();
    queue.submit("via-submit".to_string()).unwrap();

    assert_eq!(queue.next().await.unwrap(), "via-sender");
    assert_eq!(queue.next().await.unwrap(), "via-submit");

    // A capacity-1 queue pre-filled with one item emulates a full queue:
    // the next try_send must fail. (tokio mpsc panics on capacity 0.)
    let full = ClusterWorkQueue::new(1);
    full.submit("occupant".to_string()).unwrap();
    let err = full.submit("nope".to_string()).unwrap_err();
    assert!(err.contains("work queue full"), "unexpected error: {}", err);
}

#[test]
fn test_w3b_persistence_failures_when_data_dir_is_file() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"i am a file").unwrap();

    let list = ClusterTaskList::new(&blocker);
    list.create_task(make_task("t-w3b", TaskStatus::Running));

    // save_async_state: in-memory update succeeds, both disk writes warn
    list.save_async_state(
        "t-w3b",
        "child-w3b".to_string(),
        "tc_w3b".to_string(),
        serde_json::json!([{"role": "user", "content": "x"}]),
    );
    let task = list.get_task("t-w3b").unwrap();
    assert_eq!(task.status, TaskStatus::WaitingRemote);
    assert_eq!(task.waiting_for_task_id.as_deref(), Some("child-w3b"));

    // Direct persist fails at create_dir_all
    let err = list.persist_to_disk().unwrap_err();
    assert!(err.contains("Failed to create cluster dir"), "got: {}", err);

    // recover_task_ids still recovers in-memory tasks (persist warns)
    let recovered = list.recover_task_ids();
    assert_eq!(recovered, vec!["t-w3b".to_string()]);

    // complete_task still removes the task from memory (persist warns)
    list.complete_task("t-w3b");
    assert!(list.get_task("t-w3b").is_none());
}

#[test]
fn test_w3b_restore_missing_read_error_and_parse_error() {
    // 1. No tasks.json at all → Ok, empty
    let dir = tempfile::tempdir().unwrap();
    let list = ClusterTaskList::new(dir.path());
    list.restore_from_disk().unwrap();
    assert!(list.get_task("anything").is_none());

    // 2. tasks.json is a DIRECTORY → read fails → Err
    std::fs::create_dir_all(dir.path().join("cluster").join("tasks.json")).unwrap();
    let err = list.restore_from_disk().unwrap_err();
    assert!(err.contains("Failed to read tasks.json"), "got: {}", err);

    // 3. tasks.json is invalid JSON → parse fails → Err
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir2.path().join("cluster")).unwrap();
    std::fs::write(dir2.path().join("cluster").join("tasks.json"), "{not json").unwrap();
    let list2 = ClusterTaskList::new(dir2.path());
    let err2 = list2.restore_from_disk().unwrap_err();
    assert!(err2.contains("Failed to parse tasks.json"), "got: {}", err2);
}

#[test]
fn test_w3b_restore_skips_terminal_and_running_and_tolerates_corrupt_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = dir.path().join("cluster");
    std::fs::create_dir_all(&cluster).unwrap();

    // Hand-build a tasks.json covering: Running (skipped), Completed (skipped),
    // Failed (skipped), WaitingRemote with NO conversation field in the index
    // but a CORRUPT conversation file on disk (parse .ok() → None).
    let wr_corrupt = {
        let mut t = make_task("t-wr-corrupt", TaskStatus::WaitingRemote);
        t.conversation = None;
        t.waiting_for_task_id = Some("child-c".into());
        t
    };
    let tasks = serde_json::json!([
        make_task("t-running", TaskStatus::Running),
        make_task("t-completed", TaskStatus::Completed),
        make_task("t-failed", TaskStatus::Failed),
        wr_corrupt,
    ]);
    std::fs::write(
        cluster.join("tasks.json"),
        serde_json::to_string_pretty(&tasks).unwrap(),
    )
    .unwrap();
    std::fs::write(cluster.join("t-wr-corrupt.json"), "<<<not json>>>").unwrap();

    let list = ClusterTaskList::new(dir.path());
    list.restore_from_disk().unwrap();

    assert!(list.get_task("t-running").is_none(), "Running must not be restored");
    assert!(list.get_task("t-completed").is_none());
    assert!(list.get_task("t-failed").is_none());
    let restored = list.get_task("t-wr-corrupt").unwrap();
    assert_eq!(restored.status, TaskStatus::WaitingRemote);
    assert!(
        restored.conversation.is_none(),
        "corrupt conversation file must degrade to None, not fail the restore"
    );
}

#[test]
fn test_w3b_complete_task_conversation_delete_failure_still_removes_task() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = dir.path().join("cluster");
    std::fs::create_dir_all(cluster.join("t-w3b-del.json")).unwrap(); // DIRECTORY at conv path

    let list = ClusterTaskList::new(dir.path());
    list.create_task(make_task("t-w3b-del", TaskStatus::Running));
    list.complete_task("t-w3b-del");

    assert!(
        list.get_task("t-w3b-del").is_none(),
        "task must be removed from memory even when the conversation file delete fails"
    );
}

#[test]
fn test_w3b_persist_to_disk_write_failure_when_tasks_json_is_dir() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = dir.path().join("cluster");
    std::fs::create_dir_all(cluster.join("tasks.json")).unwrap(); // dir blocks the write

    let list = ClusterTaskList::new(dir.path());
    list.create_task(make_task("t-w3b-w", TaskStatus::Pending));
    let err = list.persist_to_disk().unwrap_err();
    assert!(err.contains("Failed to write tasks.json"), "got: {}", err);
}

// ============================================================
// S4 coverage: tracing field lines, conversation restore,
// recover_task_ids field lines.
// ============================================================

struct S4AllEventsSubscriber;
impl tracing::Subscriber for S4AllEventsSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::Id) {}
    fn exit(&self, _span: &tracing::Id) {}
}

fn s4_tracing_subscriber() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(S4AllEventsSubscriber);
    });
}

/// save_async_state persists the conversation file next to tasks.json and the
/// tracing info fields evaluate under the S4 subscriber (cluster_task.rs
/// 178-184, 415-417).
#[test]
fn test_s4_save_async_state_persists_conversation_file() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    let list = ClusterTaskList::new(dir.path());
    list.create_task(make_task("t-s4-a", TaskStatus::Running));

    let conversation = serde_json::json!([{"role": "user", "content": "s4 hi"}]);
    list.save_async_state(
        "t-s4-a",
        "child-s4".to_string(),
        "tc_s4".to_string(),
        conversation.clone(),
    );

    let conv_path = dir.path().join("cluster").join("t-s4-a.json");
    assert!(conv_path.exists(), "conversation file must be persisted");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&conv_path).unwrap()).unwrap();
    assert_eq!(on_disk, conversation);

    // tasks.json index is persisted too.
    assert!(dir.path().join("cluster").join("tasks.json").exists());
}

/// inject_callback's success path evaluates its info fields under the
/// subscriber (cluster_task.rs 225-229).
#[test]
fn test_s4_inject_callback_fields() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    let list = ClusterTaskList::new(dir.path());
    list.create_task(make_task("t-s4-b", TaskStatus::Running));
    list.save_async_state(
        "t-s4-b",
        "child-b".to_string(),
        "tc_b".to_string(),
        serde_json::json!([]),
    );

    list.inject_callback("t-s4-b", "s4 callback payload");

    let task = list.get_task("t-s4-b").unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(task.callback_result.as_deref(), Some("s4 callback payload"));
    assert!(task.waiting_for_task_id.is_none());
}

/// persist_conversation with the target path blocked by a directory hits the
/// write-failure warn with field expressions (cluster_task.rs 281-287).
#[test]
fn test_s4_persist_conversation_write_failure_warns() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    let conv_path = dir.path().join("cluster").join("t-s4-c.json");
    std::fs::create_dir_all(&conv_path).unwrap(); // directory blocks the write

    let list = ClusterTaskList::new(dir.path());
    list.persist_conversation("t-s4-c", &serde_json::json!([{"role": "user"}]));

    // Path is still the blocking directory (nothing overwrote it).
    assert!(conv_path.is_dir());
}

/// restore_from_disk re-attaches a conversation from disk when tasks.json has
/// conversation: null for a WaitingRemote task (cluster_task.rs 320-327).
#[test]
fn test_s4_restore_from_disk_loads_conversation_file() {
    let dir = tempfile::tempdir().unwrap();
    let cluster_dir = dir.path().join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();

    // Index entry without an inline conversation snapshot.
    let task = make_task("t-s4-r", TaskStatus::WaitingRemote);
    std::fs::write(
        cluster_dir.join("tasks.json"),
        serde_json::to_string(&vec![task]).unwrap(),
    )
    .unwrap();
    // ...and the snapshot on disk.
    let conversation = serde_json::json!([{"role": "user", "content": "restored"}]);
    std::fs::write(
        cluster_dir.join("t-s4-r.json"),
        serde_json::to_string(&conversation).unwrap(),
    )
    .unwrap();

    let list = ClusterTaskList::new(dir.path());
    list.restore_from_disk().unwrap();

    let restored = list.get_task("t-s4-r").unwrap();
    assert_eq!(restored.conversation.as_ref().unwrap(), &conversation);
    assert_eq!(restored.status, TaskStatus::WaitingRemote);
}

/// recover_task_ids evaluates its per-arm info fields for Pending and
/// WaitingRemote tasks and the summary field line (cluster_task.rs 362-391).
#[test]
fn test_s4_recover_task_ids_fields_for_pending_and_waiting() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    let list = ClusterTaskList::new(dir.path());
    list.create_task(make_task("t-s4-p", TaskStatus::Pending));
    list.create_task(make_task("t-s4-w", TaskStatus::WaitingRemote));
    list.create_task(make_task("t-s4-done", TaskStatus::Completed));

    let ids = list.recover_task_ids();
    assert_eq!(ids.len(), 2, "pending + waiting tasks are re-queued");
    assert!(ids.contains(&"t-s4-p".to_string()));
    assert!(ids.contains(&"t-s4-w".to_string()));

    // WaitingRemote was reset to Pending.
    assert_eq!(list.get_task("t-s4-w").unwrap().status, TaskStatus::Pending);
    // Completed untouched.
    assert_eq!(
        list.get_task("t-s4-done").unwrap().status,
        TaskStatus::Completed
    );
}
