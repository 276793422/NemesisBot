//! exec_workspace 单测（P4/E3 worker 侧档案工作副本）。
//!
//! 场景矩阵：非档案任务零接管 / 基线解包（收件箱消费+sidecar）/ 空树基线 /
//! prompt 附加段 / 全树 diff（增/改/删/未动）/ finish 全链（真 outbox 入队
//! + 变更集随行 + 工作副本清扫）/ 零差异纯记录 / 残留清扫。

use std::path::PathBuf;

use super::{
    BASELINE_SIDECAR_NAME, BaselineSidecar, exec_dir_for, exec_root_in_workspace, finish_task_exec,
    sweep_exec_residual, write_sidecar,
};
use crate::changeset::{CHANGESET_DIR_NAME, read_changeset};
use crate::outbox::{TransferOutbox, TransferTransport};
use crate::transfer::{
    TRANSFER_KIND_EXECUTION_RECORDS, TRANSFER_KIND_PROJECT_BASELINE, TransferFileEntry, sha256_hex,
};

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
        panic!("exec_workspace 单测不得触发网络推送");
    }
}

// ---------------------------------------------------------------------------

struct Ws {
    root: PathBuf,
}

impl Ws {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("nmb-exec-ws-{}-{name}", std::process::id()));
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

    /// 造 worker 侧执行记录（cluster_logs/<source>/<ts>_<task>/log.md）。
    fn seed_logs(&self, task_id: &str) {
        let dev = nemesis_path::resolve_cluster_logs_dir_in_workspace(&self.root).join("node-a");
        let dir = dev.join(format!("2026-09-14_00-00-00-000_{task_id}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("log.md"), b"execution log").unwrap();
    }

    /// 造收件箱基线档（transfer.end 落地形态）。
    fn seed_baseline_inbox(&self, task_id: &str, files: &[(&str, &[u8])]) {
        let inbox = nemesis_path::cluster_dir_in_workspace(&self.root)
            .join("inbox")
            .join(task_id);
        let files_root = inbox.join("files");
        std::fs::create_dir_all(&files_root).unwrap();
        let mut entries = Vec::new();
        for (rel, data) in files {
            let dest = files_root.join(rel);
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p).unwrap();
            }
            std::fs::write(&dest, data).unwrap();
            entries.push(TransferFileEntry {
                path: rel.to_string(),
                size: data.len() as u64,
                sha256: sha256_hex(data),
            });
        }
        let begin = crate::transfer::TransferBegin {
            transfer_id: format!("{task_id}-x"),
            task_id: task_id.to_string(),
            kind: TRANSFER_KIND_PROJECT_BASELINE.into(),
            source_node: "node-a".into(),
            total_bytes: entries.iter().map(|e| e.size).sum(),
            chunk_size: 1024,
            chunk_count: 1,
            files: entries,
            content_hash: "hash".into(),
        };
        std::fs::write(
            inbox.join("manifest.json"),
            serde_json::to_string_pretty(&begin).unwrap(),
        )
        .unwrap();
        std::fs::write(
            inbox.join("landed.json"),
            serde_json::json!({ "kind": TRANSFER_KIND_PROJECT_BASELINE }).to_string(),
        )
        .unwrap();
    }

    fn payload_with_baseline(commit: &str) -> serde_json::Value {
        serde_json::json!({ "content": "task", "_baseline_commit": commit })
    }
}

impl Drop for Ws {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ---------------------------------------------------------------------------

/// 非档案任务（payload 无 `_baseline_commit`）零接管。
#[test]
fn non_archive_payload_is_noop() {
    let ws = Ws::new("noop");
    let hook = super::ExecReceiveHook::new(&ws.root);
    assert!(
        hook.receive("t-noop-1", &serde_json::json!({ "content": "x" }))
            .is_none()
    );
    assert!(!exec_dir_for(&ws.root, "t-noop-1").exists());
}

/// 基线解包：收件箱 → 工作副本 + sidecar + prompt 附加段；收件箱被消费。
#[test]
fn unpack_baseline_consumes_inbox_and_writes_sidecar() {
    let ws = Ws::new("unpack");
    ws.seed_baseline_inbox("t-unpack-1", &[("src/a.h", b"alpha\n" as &[u8])]);
    let hook = super::ExecReceiveHook::new(&ws.root);
    let appendix = hook
        .receive("t-unpack-1", &Ws::payload_with_baseline("c0ffee"))
        .expect("_baseline_commit 在场必须产出 prompt 附加段");
    assert!(
        appendix.contains("Working Directory"),
        "附加段必须含工作目录段: {appendix}"
    );
    assert!(
        appendix.contains("t-unpack-1")
            || appendix.contains(
                exec_dir_for(&ws.root, "t-unpack-1")
                    .to_string_lossy()
                    .as_ref()
            )
    );

    let exec = exec_dir_for(&ws.root, "t-unpack-1");
    assert_eq!(std::fs::read(exec.join("src/a.h")).unwrap(), b"alpha\n");
    assert!(exec.join(BASELINE_SIDECAR_NAME).exists());
    assert!(
        !nemesis_path::cluster_dir_in_workspace(&ws.root)
            .join("inbox")
            .join("t-unpack-1")
            .exists(),
        "解包成功必须消费收件箱"
    );
}

/// 空树基线：收件箱无档（master NothingToSend 形态）→ 空工作副本 + 空 sidecar。
#[test]
fn empty_baseline_tree_gets_empty_sidecar() {
    let ws = Ws::new("emptytree");
    let hook = super::ExecReceiveHook::new(&ws.root);
    let appendix = hook
        .receive("t-emptytree-1", &Ws::payload_with_baseline("cafe01"))
        .expect("空树基线同样产出附加段");
    assert!(appendix.contains("0 个文件"));
    let exec = exec_dir_for(&ws.root, "t-emptytree-1");
    let sidecar_raw = std::fs::read_to_string(exec.join(BASELINE_SIDECAR_NAME)).unwrap();
    let sidecar: BaselineSidecar = serde_json::from_str(&sidecar_raw).unwrap();
    assert_eq!(sidecar.baseline_commit, "cafe01");
    assert!(sidecar.files.is_empty());
}

/// finish：非档案任务（无 exec 目录）= Ok(false)，调用方走既有纯记录路径。
#[test]
fn finish_without_exec_dir_is_not_archive() {
    let ws = Ws::new("finish-no");
    let ob = ws.outbox();
    assert!(
        !finish_task_exec(&ws.root, &ob, "t-fn-1", "node-a").unwrap(),
        "无 exec 目录必须返回 Ok(false)"
    );
}

/// finish 全链：改动文件 → 变更集随行入队 + 工作副本清扫。
#[test]
fn finish_builds_changeset_and_cleans_up() {
    let ws = Ws::new("finish-yes");
    let task = "t-fy-1";
    ws.seed_logs(task);

    // 工作副本：基线 a.h(改) + b.h(未动) + c.h(新) ；基线里的 d.h 被删。
    let exec = exec_dir_for(&ws.root, task);
    std::fs::create_dir_all(exec.join("src")).unwrap();
    std::fs::write(exec.join("src/a.h"), b"alpha-v2\n").unwrap();
    std::fs::write(exec.join("src/b.h"), b"beta\n").unwrap();
    std::fs::write(exec.join("src/c.h"), b"gamma\n").unwrap();
    write_sidecar(
        &exec,
        &BaselineSidecar {
            baseline_commit: "base123".into(),
            files: vec![
                TransferFileEntry {
                    path: "src/a.h".into(),
                    size: 8,
                    sha256: sha256_hex(b"alpha-v1"),
                },
                TransferFileEntry {
                    path: "src/b.h".into(),
                    size: 5,
                    sha256: sha256_hex(b"beta\n"),
                },
                TransferFileEntry {
                    path: "src/d.h".into(),
                    size: 5,
                    sha256: sha256_hex(b"delta"),
                },
            ],
        },
    )
    .unwrap();

    let ob = ws.outbox();
    assert!(
        finish_task_exec(&ws.root, &ob, task, "node-a").unwrap(),
        "档案管线 finish 必须返回 Ok(true)"
    );
    assert!(!exec.exists(), "终结后工作副本必须清扫");
    // 变更集暂存目录同样清理。
    assert!(
        !exec_root_in_workspace(&ws.root)
            .join(format!(".cs-{task}"))
            .exists()
    );

    // outbox 载荷内：执行记录 + 变更集同行；diff 结果正确（a 改 / c 新 / d 删）。
    let payload = ob.outbox_root().join(task).join("payload");
    assert!(payload.join("log.md").exists(), "执行记录必须随行");
    let (manifest, contents) = read_changeset(&payload)
        .expect("变更集必须随载荷在场")
        .expect("变更集必须合法");
    assert_eq!(manifest.base_commit, "base123");
    let paths: Vec<&str> = manifest.upserts.iter().map(|u| u.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["src/a.h", "src/c.h"],
        "改动+新增入 upsert，b 未动不入"
    );
    assert_eq!(manifest.deletions, vec!["src/d.h"]);
    assert_eq!(
        contents
            .iter()
            .find(|c| c.path == "src/a.h")
            .unwrap()
            .content,
        b"alpha-v2\n"
    );
    assert!(
        payload
            .join(CHANGESET_DIR_NAME)
            .join("files/src/a.h")
            .exists(),
        "变更集文件内容必须落载荷 files/"
    );
}

/// finish 零差异：不产变更集（纯记录交付），工作副本仍清扫。
#[test]
fn finish_untouched_tree_skips_changeset() {
    let ws = Ws::new("finish-clean");
    let task = "t-fc-1";
    ws.seed_logs(task);
    let exec = exec_dir_for(&ws.root, task);
    std::fs::create_dir_all(&exec).unwrap();
    std::fs::write(exec.join("same.h"), b"same\n").unwrap();
    write_sidecar(
        &exec,
        &BaselineSidecar {
            baseline_commit: "b".into(),
            files: vec![TransferFileEntry {
                path: "same.h".into(),
                size: 5,
                sha256: sha256_hex(b"same\n"),
            }],
        },
    )
    .unwrap();

    let ob = ws.outbox();
    assert!(
        finish_task_exec(&ws.root, &ob, task, "node-a").unwrap(),
        "零差异 finish 必须返回 Ok(true)"
    );
    let payload = ob.outbox_root().join(task).join("payload");
    assert!(payload.join("log.md").exists());
    assert!(read_changeset(&payload).is_none(), "零差异不得产变更集");
    assert!(!exec.exists());
}

/// finish 入队失败（无执行记录）= Err 且现场保留（清扫可重试）。
#[test]
fn finish_enqueue_failure_preserves_scene() {
    let ws = Ws::new("finish-err");
    let task = "t-fe-1";
    // 故意不造 cluster_logs 记录 → enqueue 失败。
    let exec = exec_dir_for(&ws.root, task);
    std::fs::create_dir_all(&exec).unwrap();
    std::fs::write(exec.join("x.h"), b"x\n").unwrap();
    write_sidecar(
        &exec,
        &BaselineSidecar {
            baseline_commit: "b".into(),
            files: vec![],
        },
    )
    .unwrap();

    let ob = ws.outbox();
    assert!(finish_task_exec(&ws.root, &ob, task, "node-a").is_err());
    assert!(exec.exists(), "入队失败必须保留工作副本现场");
}

/// 残留清扫：带 sidecar 的 exec 目录重新入队并清理。
#[test]
fn sweep_residual_reenqueues_and_cleans() {
    let ws = Ws::new("sweep");
    let task = "t-sw-1";
    ws.seed_logs(task);
    let exec = exec_dir_for(&ws.root, task);
    std::fs::create_dir_all(&exec).unwrap();
    std::fs::write(exec.join("w.h"), b"w\n").unwrap();
    write_sidecar(
        &exec,
        &BaselineSidecar {
            baseline_commit: "b".into(),
            files: vec![],
        },
    )
    .unwrap();

    let ob = ws.outbox();
    assert_eq!(sweep_exec_residual(&ws.root, &ob), 1);
    assert!(!exec.exists());
    assert!(
        ob.outbox_root()
            .join(task)
            .join("payload")
            .join("changeset")
            .exists()
    );
}

/// 静态保障：执行记录与基线 kind 常量不漂移（wire 契约）。
#[test]
fn transfer_kinds_stable() {
    assert_eq!(TRANSFER_KIND_EXECUTION_RECORDS, "execution_records");
    assert_eq!(TRANSFER_KIND_PROJECT_BASELINE, "project_baseline");
}
