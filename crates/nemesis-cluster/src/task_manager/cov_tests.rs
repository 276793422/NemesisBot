// task_manager.rs 覆盖率补充测试（cleanup_completed 的超龄终态任务
// 清理臂：completed_at 超 2h → store.delete）。

use super::*;
use std::sync::Arc;

fn make_task(id: &str, status: TaskStatus, completed_at: Option<String>) -> Task {
    Task {
        id: id.into(),
        status,
        action: "cov.action".into(),
        peer_id: "peer-a".into(),
        payload: serde_json::Value::Null,
        result: None,
        original_channel: "rpc".into(),
        original_chat_id: "chat-1".into(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at,
    }
}

/// 超龄（3h 前）完成态任务被 delete，新鲜完成态与 pending 保留（502）。
#[test]
fn cleanup_completed_deletes_only_stale_finished_tasks() {
    let store = Arc::new(InMemoryTaskStore::new());
    let stale = (chrono::Local::now() - chrono::Duration::hours(3)).to_rfc3339();
    let fresh = chrono::Local::now().to_rfc3339();

    store
        .create(make_task("t-stale", TaskStatus::Completed, Some(stale)))
        .unwrap();
    store
        .create(make_task("t-fresh", TaskStatus::Completed, Some(fresh)))
        .unwrap();
    store
        .create(make_task("t-pending", TaskStatus::Pending, None))
        .unwrap();

    let pending_timeout = parking_lot::RwLock::new(chrono::Duration::hours(24));
    cleanup_completed(
        &(store.clone() as Arc<dyn TaskStore>),
        &None,
        &pending_timeout,
    );

    assert!(store.get("t-stale").is_err(), "超龄完成态必须被清");
    assert!(store.get("t-fresh").is_ok(), "新鲜完成态保留");
    assert!(store.get("t-pending").is_ok(), "pending 由独立闸管理");
}
