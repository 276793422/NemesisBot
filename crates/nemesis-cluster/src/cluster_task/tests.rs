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
    assert!(recovered.is_empty(), "Completed and Failed tasks should not be recovered");

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
    assert!(result.unwrap().is_none(), "Expected None when all senders are dropped");
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
