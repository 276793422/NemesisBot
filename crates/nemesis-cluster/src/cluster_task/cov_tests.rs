// cluster_task.rs 覆盖率补充测试（save_async_state 会话快照落盘 /
// inject_callback 复活臂 / complete_task 会话文件删除失败 warn /
// persist_conversation 写盘失败 warn / recover_task_ids 双状态 +
// 持久化失败 warn / persist_to_disk 成功尾）。
//
// 豁免：386-387（persist_conversation 的 serde 序列化失败臂——入参是
// `&serde_json::Value`，to_string_pretty 对 Value 数学上不可能失败，
// 死防御）；335 的非 Windows 形态（本文件仅在 Windows 跑删除失败面，
// 文件占用不可删是 Windows 共享语义）。

use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nmb-ctask-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_task(id: &str, status: TaskStatus) -> ClusterTask {
    ClusterTask {
        task_id: id.into(),
        source: TaskSource {
            node_id: "node-a".into(),
            rpc_address: "127.0.0.1:21949".into(),
            session_key: "sess-1".into(),
        },
        status,
        content: "cov content".into(),
        conversation: None,
        waiting_for_task_id: None,
        waiting_tool_call_id: None,
        callback_result: None,
    }
}

/// save_async_state：快照字段落任务 + 会话 JSON 落盘（207）+ 索引
/// persist_to_disk 成功尾（519/521）。
#[test]
fn save_async_state_persists_conversation_and_index() {
    let dir = temp_dir("save-async");
    let list = ClusterTaskList::new(&dir);
    list.create_task(make_task("t-async", TaskStatus::Running));

    list.save_async_state(
        "t-async",
        "child-1".into(),
        "call-1".into(),
        serde_json::json!([
            {"role": "user", "content": "hi"}
        ]),
    );

    let task = list.get_task("t-async").unwrap();
    assert_eq!(task.status, TaskStatus::WaitingRemote);
    assert_eq!(task.waiting_for_task_id.as_deref(), Some("child-1"));
    assert_eq!(task.waiting_tool_call_id.as_deref(), Some("call-1"));
    assert!(task.callback_result.is_none());

    let conv = dir.join("cluster").join("t-async.json");
    assert!(conv.exists(), "会话快照落盘：{}", conv.display());
    let raw = std::fs::read_to_string(&conv).unwrap();
    assert!(raw.contains("\"hi\""));

    // 索引落盘含该任务（WaitingRemote 非终态 → 入 tasks.json）。
    let index = std::fs::read_to_string(dir.join("cluster").join("tasks.json")).unwrap();
    assert!(index.contains("t-async"), "{index}");
}

/// inject_callback：WaitingRemote → Pending + 回填结果 + 清 waiting（269）。
#[test]
fn inject_callback_moves_waiting_to_pending() {
    let dir = temp_dir("inject");
    let list = ClusterTaskList::new(&dir);
    list.create_task(make_task("t-inj", TaskStatus::WaitingRemote));

    list.inject_callback("t-inj", "child result payload");

    let task = list.get_task("t-inj").unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(
        task.callback_result.as_deref(),
        Some("child result payload")
    );
    assert!(task.waiting_for_task_id.is_none());
}

/// complete_task：会话文件被无 DELETE 共享的句柄占用 → 删除失败 warn，
/// 任务仍被移除（335；Windows 共享语义）。
#[cfg(windows)]
#[test]
fn complete_task_conv_file_undeletable_warns_and_removes() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = temp_dir("complete-locked");
    let list = ClusterTaskList::new(&dir);
    list.create_task(make_task("t-done", TaskStatus::Running));
    list.save_async_state(
        "t-done",
        "child".into(),
        "call".into(),
        serde_json::json!([]),
    );

    let conv = dir.join("cluster").join("t-done.json");
    assert!(conv.exists());

    // 占住文件：share_mode 只给读写、不给 FILE_SHARE_DELETE → remove 必败。
    let _guard = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0001 | 0x0002)
        .open(&conv)
        .unwrap();

    list.complete_task("t-done");

    assert!(conv.exists(), "删除失败的会话文件留在原地");
    assert!(list.get_task("t-done").is_none(), "任务条目照常回收");
}

/// persist_conversation 写盘失败（cluster 位是文件 → 父目录建不出）
/// → warn 不 panic（380）。
#[test]
fn persist_conversation_write_failure_warns() {
    let dir = temp_dir("persist-fail");
    // cluster 位被文件占住 → conversation_path 的父目录不可建。
    std::fs::write(dir.join("cluster"), "not a dir").unwrap();
    let list = ClusterTaskList::new(&dir);
    list.create_task(make_task("t-wfail", TaskStatus::Running));

    // 只需不 panic（warn 臂留痕）；任务状态照常更新。
    list.save_async_state(
        "t-wfail",
        "child".into(),
        "call".into(),
        serde_json::json!([]),
    );
    assert_eq!(
        list.get_task("t-wfail").unwrap().status,
        TaskStatus::WaitingRemote
    );
}

/// recover_task_ids：Pending 原样入队、WaitingRemote 重置 Pending，
/// 并把状态变化持久化（460/467/487）。
#[test]
fn recover_task_ids_resets_waiting_and_persists() {
    let dir = temp_dir("recover");
    let list = ClusterTaskList::new(&dir);
    list.create_task(make_task("t-pend", TaskStatus::Pending));
    list.create_task(make_task("t-wait", TaskStatus::WaitingRemote));
    list.create_task(make_task("t-done", TaskStatus::Completed));

    let ids = list.recover_task_ids();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["t-pend", "t-wait"]);
    assert_eq!(list.get_task("t-wait").unwrap().status, TaskStatus::Pending);

    // WaitingRemote → Pending 的状态变化已落 tasks.json。
    let index = std::fs::read_to_string(dir.join("cluster").join("tasks.json")).unwrap();
    assert!(
        index.contains("t-pend") && index.contains("t-wait"),
        "{index}"
    );
    assert!(!index.contains("t-done"), "终态任务不入索引：{index}");
}

/// recover_task_ids：持久化失败（cluster 位是文件）只 warn 不阻断，
/// id 照常返回（485）。
#[test]
fn recover_task_ids_persist_failure_still_returns_ids() {
    let dir = temp_dir("recover-fail");
    std::fs::write(dir.join("cluster"), "not a dir").unwrap();
    let list = ClusterTaskList::new(&dir);
    list.create_task(make_task("t-rp", TaskStatus::Pending));

    let ids = list.recover_task_ids();
    assert_eq!(ids, vec!["t-rp".to_string()]);
}
