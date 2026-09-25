//! cov 补测（2026-09-25）：`ClusterResultPersisterAdapter::on_task_terminal`
//! 的发件箱三臂（outbox 缺席早退 / workspace 在场但无执行档案 → 回落 enqueue /
//! 无 workspace 直接入队并真实入队成功）。
//! 全进程内：TaskResultStore 纯内存，TransferOutbox 落 tempdir，传输用哑实现
//! （入队只写本地 entry.json，不触网）。

use super::*;

/// 永不被调用的哑传输（enqueue 只落盘；kick 的推送循环不在本测试装配）。
struct NoTransport;

#[async_trait::async_trait]
impl nemesis_cluster::outbox::TransferTransport for NoTransport {
    async fn call(
        &self,
        _peer: &str,
        _action: &str,
        _payload: serde_json::Value,
        _timeout: std::time::Duration,
    ) -> Result<serde_json::Value, String> {
        Err("no transport in unit test".to_string())
    }
}

/// 组装 persister：outbox/workspace 按需装配。
fn persister(
    tmp: &tempfile::TempDir,
    with_outbox: bool,
    workspace: Option<&std::path::Path>,
) -> ClusterResultPersisterAdapter {
    let outbox = if with_outbox {
        // 生产形态：outbox 与 adapter workspace 共用同一 workspace 根
        // （logs_root / outbox_root 都从它派生）；无 workspace 时退 tempdir 根。
        let base = workspace.unwrap_or(tmp.path());
        Some(std::sync::Arc::new(
            nemesis_cluster::outbox::TransferOutbox::new(
                base,
                "self-node".to_string(),
                std::sync::Arc::new(NoTransport),
                Box::new(|| 1 << 20),
            ),
        ))
    } else {
        None
    };
    ClusterResultPersisterAdapter {
        result_store: std::sync::Arc::new(
            nemesis_cluster::task_result_store::TaskResultStore::new(16),
        ),
        node_id: "self-node".to_string(),
        outbox,
        workspace: workspace.map(|p| p.to_path_buf()),
    }
}

/// 种一条 worker 侧执行记录（cluster_logs/{source}/{ts}_{task}/log.md），
/// 供 enqueue 的 find_task_dir 命中。
fn seed_task_log(workspace: &std::path::Path, source: &str, task_id: &str) {
    let logs = nemesis_path::resolve_cluster_logs_dir_in_workspace(workspace);
    let dir = logs.join(source).join(format!("1700000000_{task_id}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("log.md"), "exec record").unwrap();
}

#[test]
fn on_task_terminal_without_outbox_is_noop() {
    use nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister;
    let tmp = tempfile::tempdir().unwrap();
    let p = persister(&tmp, false, None);
    // outbox None → 早退；不 panic、不落任何东西。
    p.on_task_terminal("task-noob", "peer-a");
    let outbox_dir = tmp.path().join("cluster").join("outbox");
    assert!(!outbox_dir.exists(), "无 outbox 时不得建发件箱目录");
}

#[test]
fn on_task_terminal_enqueues_seedless_record_fails_honest() {
    use nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister;
    let tmp = tempfile::tempdir().unwrap();
    // 无 workspace：直接 enqueue；cluster_logs 无该任务记录 → 诚实 Err（warn 臂）。
    let p = persister(&tmp, true, None);
    p.on_task_terminal("task-ghost", "peer-a");
    let entry = tmp
        .path()
        .join("cluster")
        .join("outbox")
        .join("task-ghost")
        .join("entry.json");
    assert!(!entry.exists(), "无执行记录不得产出 entry.json");
}

#[test]
fn on_task_terminal_enqueues_seeded_record() {
    use nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    seed_task_log(&ws, "peer-a", "task-ok");
    // workspace 在场但无 exec 工作副本 sidecar → finish_task_exec Ok(false)
    // → 回落纯记录 enqueue → 命中 seeded 记录 → entry.json 落盘。
    let p = persister(&tmp, true, Some(&ws));
    p.on_task_terminal("task-ok", "peer-a");
    let entry = ws
        .join("cluster")
        .join("outbox")
        .join("task-ok")
        .join("entry.json");
    assert!(entry.exists(), "seeded 记录必须成功入队（entry.json 在场）");
    let entry_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&entry).unwrap()).unwrap();
    assert_eq!(entry_json["task_id"], "task-ok");
    assert_eq!(entry_json["source_node"], "peer-a");
    assert_eq!(entry_json["state"], "pending");
}

#[test]
fn on_task_terminal_with_workspace_and_empty_source_still_fails_honest() {
    use nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    // workspace 在场（无 exec sidecar → Ok(false) 回落 enqueue）+ 空 source
    // → enqueue 拒绝（task_id/source 为空诚实臂）。
    let p = persister(&tmp, true, Some(&ws));
    p.on_task_terminal("task-empty-src", "");
    let entry = ws
        .join("cluster")
        .join("outbox")
        .join("task-empty-src")
        .join("entry.json");
    assert!(!entry.exists(), "空 source 不得入队");
}

/// 执行档案 sidecar 损坏（非 JSON）→ finish_task_exec Err → 现场保留 +
/// 不回落 enqueue（Err 臂直接 return）。
#[test]
fn on_task_terminal_exec_sidecar_corrupt_keeps_scene_and_skips_enqueue() {
    use nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let exec_dir = nemesis_cluster::exec_workspace::exec_dir_for(&ws, "task-bad-sidecar");
    std::fs::create_dir_all(&exec_dir).unwrap();
    std::fs::write(
        exec_dir.join(nemesis_cluster::exec_workspace::BASELINE_SIDECAR_NAME),
        "{not json",
    )
    .unwrap();
    let p = persister(&tmp, true, Some(&ws));
    p.on_task_terminal("task-bad-sidecar", "peer-a");
    // 现场保留：sidecar 原样在；诚实降级：无 entry.json。
    assert!(
        exec_dir
            .join(nemesis_cluster::exec_workspace::BASELINE_SIDECAR_NAME)
            .exists(),
        "损坏 sidecar 现场必须保留"
    );
    assert!(
        !ws.join("cluster")
            .join("outbox")
            .join("task-bad-sidecar")
            .join("entry.json")
            .exists(),
        "Err 臂不得回落纯记录 enqueue"
    );
}

/// 执行档案在场且零差异 → 纯记录交付归档（Ok(true) 臂）：exec 目录清空、
/// entry.json 落盘。
#[test]
fn on_task_terminal_exec_zero_diff_archives_and_clears_exec_dir() {
    use nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let task = "task-zero-diff";
    seed_task_log(&ws, "peer-a", task);
    let exec_dir = nemesis_cluster::exec_workspace::exec_dir_for(&ws, task);
    std::fs::create_dir_all(&exec_dir).unwrap();
    let sidecar = nemesis_cluster::exec_workspace::BaselineSidecar {
        baseline_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        files: Vec::new(),
    };
    std::fs::write(
        exec_dir.join(nemesis_cluster::exec_workspace::BASELINE_SIDECAR_NAME),
        serde_json::to_string(&sidecar).unwrap(),
    )
    .unwrap();

    let p = persister(&tmp, true, Some(&ws));
    p.on_task_terminal(task, "peer-a");

    // Ok(true)：exec 目录已清（归档完成）；记录入队成功。
    assert!(!exec_dir.exists(), "零差异归档后 exec 工作副本目录必须清除");
    assert!(
        ws.join("cluster")
            .join("outbox")
            .join(task)
            .join("entry.json")
            .exists(),
        "零差异路径仍需纯记录入队"
    );
}
