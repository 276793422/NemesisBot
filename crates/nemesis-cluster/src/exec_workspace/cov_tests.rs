// exec_workspace.rs 覆盖率补充测试（receive 解包失败诚实 None / trait 委托
// 面 / sweep 跳过形态（文件位、.cs- 前缀、无 sidecar）/ sweep finish 失败
// warn 臂 + find_source 反查成功与落空）。
//
// 豁免：334（`dir.file_name()` 对真实目录项恒 Some，`..` 形态读目录枚举
// 不产出——死防御）；341（Ok(false) 需 sidecar 在 320 检查后、205 检查前
// 消失——TOCTOU 竞态面）。

use super::{
    BASELINE_SIDECAR_NAME, BaselineSidecar, ExecReceiveHook, exec_root_in_workspace,
    sweep_exec_residual,
};
use crate::outbox::{TransferOutbox, TransferTransport};
use std::path::PathBuf;

// ---------------------------------------------------------------------------

/// 本套件只测入队（磁盘形态），不驱动推送循环——transport 恒不该被调用。
struct NoopTransport;

#[async_trait::async_trait]
impl TransferTransport for NoopTransport {
    async fn call(
        &self,
        _peer: &str,
        _action: &str,
        _payload: serde_json::Value,
        _timeout: std::time::Duration,
    ) -> Result<serde_json::Value, String> {
        panic!("cov 单测不得触发网络推送");
    }
}

struct Ws {
    root: PathBuf,
}

impl Ws {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("nmb-exec-cov-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn outbox(&self) -> TransferOutbox {
        TransferOutbox::new(
            &self.root,
            "node-w".into(),
            std::sync::Arc::new(NoopTransport),
            Box::new(|| 0),
        )
    }

    fn exec_root(&self) -> PathBuf {
        exec_root_in_workspace(&self.root)
    }
}

impl Drop for Ws {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ---------------------------------------------------------------------------
// receive / trait 委托
// ---------------------------------------------------------------------------

/// 解包失败（cluster 目录位是文件 → 建工作副本目录失败）→ 诚实 None +
/// 不 panic（任务照常执行的降级契约）。
#[test]
fn receive_unpack_failure_honest_none() {
    let ws = Ws::new("unpack-fail");
    // cluster 位变文件：exec 与 inbox 全在其下 → create_dir_all 必炸。
    let cluster = nemesis_path::cluster_dir_in_workspace(&ws.root);
    std::fs::create_dir_all(cluster.parent().unwrap()).unwrap();
    std::fs::write(&cluster, "not a dir").unwrap();

    let hook = ExecReceiveHook::new(&ws.root);
    assert!(
        hook.receive(
            "t-unpack-fail",
            &serde_json::json!({"_baseline_commit": "abc"})
        )
        .is_none()
    );
}

/// trait 委托面：经 `dyn TaskReceiveHook` 对象调用与直连 receive 等价。
#[test]
fn trait_object_delegate_calls_receive() {
    let ws = Ws::new("trait-delegate");
    let hook = ExecReceiveHook::new(&ws.root);
    let dyn_hook: &dyn crate::rpc::peer_chat_handler::TaskReceiveHook = &hook;
    // 无 _baseline_commit → None（非档案形态直通）。
    assert!(
        dyn_hook
            .on_task_received("t-dyn", &serde_json::json!({}))
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// sweep 跳过形态 + 失败 warn 臂
// ---------------------------------------------------------------------------

/// sweep：文件位子项跳过；`.cs-` 前缀目录跳过；无 sidecar 残留 warn 跳过
/// ——三者都不计数、原样保留。
#[test]
fn sweep_skips_files_and_cs_dirs_and_sidecarless() {
    let ws = Ws::new("sweep-skip");
    let root = ws.exec_root();
    std::fs::create_dir_all(&root).unwrap();

    std::fs::write(root.join("loose-file"), "x").unwrap();
    std::fs::create_dir_all(root.join(".cs-tmp")).unwrap();
    std::fs::write(root.join(".cs-tmp").join("changeset.json"), "{}").unwrap();
    let sidecarless = root.join("t-noside");
    std::fs::create_dir_all(&sidecarless).unwrap();

    let done = sweep_exec_residual(&ws.root, &ws.outbox());
    assert_eq!(done, 0, "全跳过零处理");
    assert!(sidecarless.exists(), "无 sidecar 残留保留现场");
    assert!(root.join("loose-file").exists());
    assert!(root.join(".cs-tmp").exists());
}

/// sweep：sidecar 损坏 → finish 报 Err → warn + 现场保留（且 find_source
/// 无日志可反查落空走 None）。
#[test]
fn sweep_finish_err_warns_and_keeps_scene() {
    let ws = Ws::new("sweep-err");
    let dir = ws.exec_root().join("t-badside");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(BASELINE_SIDECAR_NAME), "{corrupt json").unwrap();

    let done = sweep_exec_residual(&ws.root, &ws.outbox());
    assert_eq!(done, 0);
    assert!(dir.exists(), "失败现场必须保留");
}

/// sweep 正常路径：合法 sidecar + cluster_logs 反查到 source → 零差异纯
/// 记录入队，done=1，exec 目录被清理。
#[test]
fn sweep_happy_zero_diff_enqueues_via_logs_lookup() {
    let ws = Ws::new("sweep-happy");
    let dir = ws.exec_root().join("task-cov-1");
    std::fs::create_dir_all(&dir).unwrap();
    super::write_sidecar(
        &dir,
        &BaselineSidecar {
            baseline_commit: "abc123".into(),
            files: Vec::new(),
        },
    )
    .unwrap();
    // cluster_logs/<dev>/<ts>_<task>/ 反查源（dev 字典序确定性）。
    let dev = nemesis_path::resolve_cluster_logs_dir_in_workspace(&ws.root).join("node-a");
    let rec = dev.join("2026-09-14_00-00-00-000_task-cov-1");
    std::fs::create_dir_all(&rec).unwrap();
    std::fs::write(rec.join("log.md"), b"execution log").unwrap();

    let done = sweep_exec_residual(&ws.root, &ws.outbox());
    assert_eq!(done, 1, "零差异 → 纯记录入队成功");
    assert!(!dir.exists(), "入队成功后工作副本清理");
}
