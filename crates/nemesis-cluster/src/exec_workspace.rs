//! worker 侧档案工作副本（exec workspace，看板项目档案 goal P4/E3）。
//!
//! 档案管线任务（master 派发 payload 带 `_baseline_commit`）在 worker 侧的
//! 全生命周期，**全部代码级、不依赖 LLM 自觉**：
//!
//! 1. **接收**（[`ExecReceiveHook`]，PeerChatHandler work-queue 路径调用）：
//!    把 master 已推送落地的基线（收件箱 `<workspace>/cluster/inbox/<task_id>/`，
//!    kind=`project_baseline`）解包到工作副本
//!    `<workspace>/cluster/exec/<task_id>/`，基线树清单存 sidecar
//!    `.baseline.json`，并在任务 content 尾部追加 prompt 工作目录段。
//!    master 推基线与发 peer_chat 的先后由派发链保证（push_dispatch_baseline
//!    同步完成后才 spawn 真 RPC）——收到 `_baseline_commit` 时基线必已落地
//!    或为空树（NothingToSend，sidecar 记空清单）。
//! 2. **终结**（[`finish_task_exec`]，`on_task_terminal` 钩子调用）：全树
//!    扫描对 sidecar 清单 hash diff 得变更集（新增/修改/删除全集），搭执行
//!    记录同一 outbox 载荷回传（[`TransferOutbox::enqueue_with_changeset`]，
//!    保序：合并不可能抢在交付前）；随后清扫工作副本。**零差异 = 不产
//!    变更集**（纯记录交付，master 空集宽容接管）。
//! 3. **崩溃兜底**（[`sweep_exec_residual`]，worker 启动清扫调用）：残留
//!    exec 目录（终态钩子没来得及执行）重新组装并入队。**必须在
//!    `TransferOutbox::sweep_startup` 之前调用**——后者会对 cluster_logs
//!    残留补纯记录入队，条目一旦先建，带变更集的入队就被幂等挡死。
//!
//! 失败语义：任何一步失败**保留现场**（exec 目录/收件箱不删），返回 Err
//! 由调用方 WARN——绝不静默吞掉，启动清扫可重试。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::changeset::{
    self, CHANGESET_VERSION, ChangesetContent, ChangesetManifest, ChangesetUpsert,
};
use crate::outbox::TransferOutbox;
use crate::transfer::{
    TRANSFER_KIND_PROJECT_BASELINE, TransferFileEntry, collect_dir_files, copy_dir_recursive,
    sanitize_transfer_id,
};

/// 工作副本根目录名（`<workspace>/cluster/exec`）。
pub const EXEC_DIR_NAME: &str = "exec";
/// 基线 sidecar 文件名（exec 目录内；diff 扫描时排除）。
pub const BASELINE_SIDECAR_NAME: &str = ".baseline.json";

/// 基线 sidecar（工作副本内持久化——任务终结时 diff 的对照清单）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineSidecar {
    /// master 派发时下发的基线 commit hex（变更集申报原样回带，E8 归属校验用）。
    pub baseline_commit: String,
    /// 基线树清单（master TransferBegin.files 原样；空树 = 空 vec）。
    pub files: Vec<TransferFileEntry>,
}

// ---------------------------------------------------------------------------
// 目录定位
// ---------------------------------------------------------------------------

/// 工作副本根：`<workspace>/cluster/exec`。
pub fn exec_root_in_workspace(workspace: &Path) -> PathBuf {
    nemesis_path::cluster_dir_in_workspace(workspace).join(EXEC_DIR_NAME)
}

/// 单任务工作副本目录（sanitize 同 outbox 条目目录名规则）。
pub fn exec_dir_for(workspace: &Path, task_id: &str) -> PathBuf {
    exec_root_in_workspace(workspace).join(sanitize_transfer_id(task_id))
}

// ---------------------------------------------------------------------------
// 1. 接收：解包基线 + prompt 工作目录段
// ---------------------------------------------------------------------------

/// 接收钩子（[`crate::rpc::peer_chat_handler::PeerChatHandler`] work-queue
/// 路径在收到任务时调用）。payload 带 `_baseline_commit` = 档案管线任务：
/// 解包基线工作副本并返回 prompt 工作目录段（追加进任务 content）；否则
/// `None`（既有非档案行为，零改动）。
///
/// 失败诚实降级：解包失败 WARN + `None`——任务照常执行，但终结侧无 sidecar
/// 不产变更集（纯记录交付；master 空集宽容接管，不炸管线）。
pub struct ExecReceiveHook {
    workspace: PathBuf,
}

impl ExecReceiveHook {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
        }
    }

    /// payload `_baseline_commit` 在场即档案管线；解包 + 产出 prompt 附加段。
    /// 生产入口（trait 方法）把解包失败吞成 `None` + ERROR 日志（任务照常
    /// 执行；终结侧无 sidecar 不产变更集，master 空集宽容接管）。
    pub fn receive(&self, task_id: &str, payload: &serde_json::Value) -> Option<String> {
        let commit = payload
            .get("_baseline_commit")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if commit.is_empty() {
            return None;
        }
        match self.unpack_baseline(task_id, commit) {
            Ok((exec_dir, file_count)) => {
                Some(render_workdir_section(&exec_dir, commit, file_count))
            }
            Err(e) => {
                tracing::error!(
                    task_id = %task_id,
                    "[Exec] 基线解包失败（任务无基线照常执行，变更集将缺失）: {e}"
                );
                None
            }
        }
    }

    /// 解包：收件箱基线（kind=project_baseline）→ exec 目录 + sidecar；
    /// 收件箱无档 = 空树基线（sidecar 记空清单）。成功消费收件箱（删除）。
    fn unpack_baseline(
        &self,
        task_id: &str,
        baseline_commit: &str,
    ) -> Result<(PathBuf, usize), String> {
        let inbox_dir = nemesis_path::cluster_dir_in_workspace(&self.workspace)
            .join("inbox")
            .join(sanitize_transfer_id(task_id));
        let exec_dir = exec_dir_for(&self.workspace, task_id);
        let _ = std::fs::remove_dir_all(&exec_dir);
        std::fs::create_dir_all(&exec_dir).map_err(|e| format!("建工作副本目录失败: {e}"))?;

        // 收件箱 landed 回执核对 kind（非基线档 = 不是本管线的，留着不动——
        // master 侧 ingest 有自己的安置语义）。
        let files_root = inbox_dir.join("files");
        let landed_kind = std::fs::read_to_string(inbox_dir.join("landed.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(String::from));
        let files = if landed_kind.as_deref() == Some(TRANSFER_KIND_PROJECT_BASELINE)
            && files_root.exists()
        {
            let manifest = read_transfer_manifest(&inbox_dir)?;
            copy_dir_recursive(&files_root, &exec_dir)
                .map_err(|e| format!("基线解包复制失败: {e}"))?;
            // 消费收件箱（传输闭环：档已入工作副本，收件目录不残留）。
            let _ = std::fs::remove_dir_all(&inbox_dir);
            manifest.files
        } else {
            // 空树基线（master NothingToSend）或收件箱已不在场——空清单开跑。
            Vec::new()
        };
        let count = files.len();
        let sidecar = BaselineSidecar {
            baseline_commit: baseline_commit.to_string(),
            files,
        };
        let json = serde_json::to_string_pretty(&sidecar).map_err(|e| e.to_string())?;
        std::fs::write(exec_dir.join(BASELINE_SIDECAR_NAME), json)
            .map_err(|e| format!("写基线 sidecar 失败: {e}"))?;
        Ok((exec_dir, count))
    }
}

/// 读收件箱落地目录的传输清单（TransferBegin 落盘形态，即基线树全量清单）。
fn read_transfer_manifest(inbox_dir: &Path) -> Result<crate::transfer::TransferBegin, String> {
    let raw = std::fs::read_to_string(inbox_dir.join("manifest.json"))
        .map_err(|e| format!("读基线传输清单失败: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("基线传输清单解析失败: {e}"))
}

impl crate::rpc::peer_chat_handler::TaskReceiveHook for ExecReceiveHook {
    fn on_task_received(&self, task_id: &str, payload: &serde_json::Value) -> Option<String> {
        self.receive(task_id, payload)
    }
}

/// prompt 工作目录段（追加进任务 content；告诉 LLM 在哪干活 + 契约）。
///
/// F-U3-2（UAT U3 实证）：原文本只说「所有文件读写必须在该目录内」，但模型
/// 仍会用相对路径调文件工具（落点在 workspace 根而非本目录——dispatch 层
/// 重写已根修该落点）。这里把相对路径基准的真实语义写明，模型心智模型与
/// 系统行为对齐，不再需要模型自行推断。
fn render_workdir_section(exec_dir: &Path, baseline_commit: &str, file_count: usize) -> String {
    format!(
        "\n\n# Working Directory（项目档案管线）\n本任务的工作副本已预置项目基线快照（基线 commit `{baseline_commit}`，{file_count} 个文件），位于：\n`{}`\n\n所有文件读写必须在该目录内进行。write_file/edit_file/read_file 等文件工具的相对路径、以及 exec 的缺省工作目录均已指向本目录（也可直接用上面的绝对路径）。任务结束后系统会自动扫描该目录生成变更集回传主控端，无需你手动提交、打包或汇报文件清单。",
        exec_dir.display()
    )
}

// ---------------------------------------------------------------------------
// 2. 终结：全树 diff → 变更集 → 入队 → 清扫
// ---------------------------------------------------------------------------

/// 任务终结处理（`TaskResultPersister::on_task_terminal` 调用）。
///
/// - `Ok(true)` = 档案管线已处理（变更集随行或零差异纯记录，**调用方不必
///   再 enqueue**）；
/// - `Ok(false)` = 无工作副本（非档案管线），调用方走既有纯记录 enqueue；
/// - `Err` = 现场保留（exec 目录/变更集目录不删），调用方 WARN；
///   [`sweep_exec_residual`] 启动清扫可重试。
pub fn finish_task_exec(
    workspace: &Path,
    outbox: &TransferOutbox,
    task_id: &str,
    source_node: &str,
) -> Result<bool, String> {
    let exec_dir = exec_dir_for(workspace, task_id);
    if !exec_dir.join(BASELINE_SIDECAR_NAME).exists() {
        // 无 sidecar = 非档案管线（或解包失败的诚实降级形态）。
        return Ok(false);
    }
    let raw = std::fs::read_to_string(exec_dir.join(BASELINE_SIDECAR_NAME))
        .map_err(|e| format!("读基线 sidecar 失败: {e}"))?;
    let sidecar: BaselineSidecar =
        serde_json::from_str(&raw).map_err(|e| format!("基线 sidecar 解析失败: {e}"))?;

    // 全树扫描（排除 sidecar 自身）对基线清单 hash diff。
    let (upserts, contents, deletions) = diff_tree(&exec_dir, &sidecar.files)?;

    if upserts.is_empty() && deletions.is_empty() {
        // 零差异：纯记录交付（master 空集宽容），不产变更集省传输。
        outbox.enqueue(task_id, source_node)?;
    } else {
        let manifest = ChangesetManifest {
            version: CHANGESET_VERSION,
            base_commit: sidecar.baseline_commit,
            upserts,
            deletions,
        };
        let cs_dir = exec_root_in_workspace(workspace)
            .join(format!(".cs-{}", sanitize_transfer_id(task_id)));
        changeset::write_changeset(&cs_dir, &manifest, &contents)?;
        match outbox.enqueue_with_changeset(task_id, source_node, Some(&cs_dir)) {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&cs_dir);
            }
            Err(e) => {
                // 现场保留（exec + cs 都在）：启动清扫重试前调用方 WARN。
                return Err(format!("变更集入队失败（现场已保留）: {e}"));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&exec_dir);
    Ok(true)
}

/// 全树 diff：工作副本现状 vs 基线清单。返回（upsert 声明, upsert 内容,
/// 删除清单）——三者按 upsert 排序一一对应。
/// executability 跨平台不可靠（Windows 无位），恒 false（master merge 以
/// 100644 入 index；可执行位语义 v1 不保真——诚实边界）。
fn diff_tree(
    exec_dir: &Path,
    baseline_files: &[TransferFileEntry],
) -> Result<(Vec<ChangesetUpsert>, Vec<ChangesetContent>, Vec<String>), String> {
    let mut current: Vec<TransferFileEntry> = collect_dir_files(exec_dir)?
        .into_iter()
        .filter(|f| f.path != BASELINE_SIDECAR_NAME)
        .collect();
    // collect 已按 path 字典序；显式再排一次防上游语义变化（变更集声明序 =
    // contents 序的契约依据）。
    current.sort_by(|a, b| a.path.cmp(&b.path));

    let baseline: std::collections::HashMap<&str, &TransferFileEntry> = baseline_files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect();
    let current_paths: std::collections::HashSet<&str> =
        current.iter().map(|f| f.path.as_str()).collect();

    let mut upserts = Vec::new();
    let mut contents = Vec::new();
    for f in &current {
        let unchanged = baseline
            .get(f.path.as_str())
            .is_some_and(|b| b.sha256 == f.sha256 && b.size == f.size);
        if unchanged {
            continue;
        }
        let data = std::fs::read(exec_dir.join(crate::transfer::safe_relative_path(&f.path)?))
            .map_err(|e| format!("读工作副本文件 {} 失败: {e}", f.path))?;
        upserts.push(ChangesetUpsert {
            path: f.path.clone(),
            sha256: f.sha256.clone(),
            size: f.size,
            executable: false,
        });
        contents.push(ChangesetContent {
            path: f.path.clone(),
            content: data,
            executable: false,
        });
    }
    let deletions: Vec<String> = baseline_files
        .iter()
        .filter(|b| !current_paths.contains(b.path.as_str()))
        .map(|b| b.path.clone())
        .collect();
    Ok((upserts, contents, deletions))
}

// ---------------------------------------------------------------------------
// 3. 启动清扫（崩溃兜底）
// ---------------------------------------------------------------------------

/// 残留工作副本清扫（**必须在 `TransferOutbox::sweep_startup` 之前调用**，
/// 见模块头注释的幂等挡死问题）。每个带 sidecar 的 exec 目录重新走
/// [`finish_task_exec`]；成功入队的目录被清理。返回处理成功数。
pub fn sweep_exec_residual(workspace: &Path, outbox: &TransferOutbox) -> usize {
    let root = exec_root_in_workspace(workspace);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return 0;
    };
    let mut done = 0usize;
    for e in entries.flatten() {
        let dir = e.path();
        if !dir.is_dir()
            || dir
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".cs-"))
        {
            continue;
        }
        if !dir.join(BASELINE_SIDECAR_NAME).exists() {
            // 无 sidecar 残留（解包失败中断/老版本产物）——留着不动，诚实 WARN。
            tracing::warn!(
                dir = %dir.display(),
                "[Exec] 残留工作副本无基线 sidecar（不清扫不重试，留人工处置）"
            );
            continue;
        }
        // task_id 从 sidecar 无法还原（sanitize 有损）——但 enqueue 只需要
        // 目录名一致键 + cluster_logs 匹配走 task_dir_matches 双形态容忍。
        // 此处直接以目录名作 task_id：sanitize_transfer_id 对真实 uuid task
        // id 是恒等（字母数字-_.），有损形态罕见且 cluster_logs 侧同规则
        // sanitize 过，键仍对得上。
        let Some(task_id) = dir.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        // source_node 现场无法还原（目录名不含设备段）→ 从 cluster_logs 反查
        // 执行记录所属设备；查无记录 = 空来源（enqueue 诚实拒绝保现场 WARN）。
        let source = find_source_node_in_logs(workspace, &task_id).unwrap_or_default();
        match finish_task_exec(workspace, outbox, &task_id, &source) {
            Ok(true) => done += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(
                task_id = %task_id,
                "[Exec] 残留工作副本清扫失败（保留现场）: {e}"
            ),
        }
    }
    done
}

/// cluster_logs 反查：遍历设备目录找 `…_{task_id}` 执行记录，返回设备名。
/// 多设备同 task（理论不该发生）取字典序首个——确定性优先。
fn find_source_node_in_logs(workspace: &Path, task_id: &str) -> Option<String> {
    let logs_root = nemesis_path::resolve_cluster_logs_dir_in_workspace(workspace);
    let mut devices: Vec<String> = std::fs::read_dir(&logs_root)
        .ok()?
        .flatten()
        .filter(|d| d.path().is_dir())
        .filter_map(|d| d.file_name().to_str().map(String::from))
        .collect();
    devices.sort();
    for dev in devices {
        if let Ok(entries) = std::fs::read_dir(logs_root.join(&dev)) {
            for e in entries.flatten() {
                if let Some(name) = e.file_name().to_str()
                    && crate::outbox::task_dir_matches(name, task_id)
                {
                    return Some(dev);
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 测试 seam（exec 根 / sidecar 现值；tests.rs 用）
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) fn write_sidecar(dir: &Path, sidecar: &BaselineSidecar) -> Result<(), String> {
    let json = serde_json::to_string_pretty(sidecar).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(BASELINE_SIDECAR_NAME), json).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;

// 覆盖率补充批次：receive 失败臂 / trait 委托 / sweep 跳过与失败形态。
#[cfg(test)]
mod cov_tests;
