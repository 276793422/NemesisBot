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
    TRANSFER_KIND_PROJECT_BASELINE, TransferFileEntry, copy_dir_recursive, sanitize_transfer_id,
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
        crate::fsutil::remove_dir_all_resilient(&exec_dir);
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
            crate::fsutil::remove_dir_all_resilient(&inbox_dir);
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
        "\n\n# Working Directory（项目档案管线）\n本任务的工作副本已预置项目基线快照（基线 commit `{baseline_commit}`，{file_count} 个文件），位于：\n`{}`\n\n所有文件读写必须在该目录内进行。write_file/edit_file/read_file 等文件工具的相对路径、以及 exec 的缺省工作目录均已指向本目录（也可直接用上面的绝对路径）。任务结束后系统会自动扫描该目录生成变更集回传主控端，无需你手动提交、打包或汇报文件清单。\n\n# 交付机制（重要）\n- **最终回复是唯一交付通道**：任务完成的判定与验收评审只看你的最终回复文本（worker 汇报回传主控端）。用 board_issue 等工具在本地看板上写状态/评论**不算交付**——那只是本节点仪表盘视图，主控端验收流程看不到；工具汇报照常可用，但不能替代最终回复里的交付汇报。\n- 交付汇报必须自包含：做了什么、关键变更点、如何验证（命令+结果摘要）。主控端验收员看不到你的中间过程，只看得到最终回复。\n- 调试过程（临时脚本、探针输出、实验性改动）一律放工作副本的 `_debug/` 目录——该目录**不进变更集**（不会合入项目），也不会污染交付；正式交付物必须放在 `_debug/` 之外。",
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
    // BUG-2026-10-15 NB-120（A 治本）：任务派生但未被树杀覆盖的残留进程
    // （浏览器实测的 Edge/crashpad 等）会占住工作副本内文件锁，让全树 diff
    // 与清扫整体失败。组装变更集**之前**按命令行清扫（best-effort，护栏见
    // kill_task_leftover_processes；非档案任务不走到这里，零开销）。
    if kill_task_leftover_processes(task_id) > 0 {
        // 强杀后内核释放句柄有毫秒级窗口——小睡一拍，尽量不让可交付文件
        // 沦为 skipped 注记。
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    let raw = std::fs::read_to_string(exec_dir.join(BASELINE_SIDECAR_NAME))
        .map_err(|e| format!("读基线 sidecar 失败: {e}"))?;
    let sidecar: BaselineSidecar =
        serde_json::from_str(&raw).map_err(|e| format!("基线 sidecar 解析失败: {e}"))?;

    // 全树扫描（排除 sidecar 自身）对基线清单 hash diff。
    let (upserts, contents, deletions, skipped) = diff_tree(&exec_dir, &sidecar.files)?;
    if !skipped.is_empty() {
        tracing::warn!(
            task_id = %task_id,
            count = skipped.len(),
            skipped = ?skipped,
            "[Exec] 变更集含不可读跳过文件（已随 manifest 申报 master，不影响其余交付）"
        );
    }

    if upserts.is_empty() && deletions.is_empty() {
        // 零差异：纯记录交付（master 空集宽容），不产变更集省传输。
        outbox.enqueue(task_id, source_node)?;
    } else {
        let manifest = ChangesetManifest {
            version: CHANGESET_VERSION,
            base_commit: sidecar.baseline_commit,
            upserts,
            deletions,
            skipped,
        };
        let cs_dir = exec_root_in_workspace(workspace)
            .join(format!(".cs-{}", sanitize_transfer_id(task_id)));
        changeset::write_changeset(&cs_dir, &manifest, &contents)?;
        match outbox.enqueue_with_changeset(task_id, source_node, Some(&cs_dir)) {
            Ok(()) => {
                crate::fsutil::remove_dir_all_resilient(&cs_dir);
            }
            Err(e) => {
                // 现场保留（exec + cs 都在）：启动清扫重试前调用方 WARN。
                return Err(format!("变更集入队失败（现场已保留）: {e}"));
            }
        }
    }
    crate::fsutil::remove_dir_all_resilient(&exec_dir);
    Ok(true)
}

/// 全树 diff：工作副本现状 vs 基线清单。返回（upsert 声明, upsert 内容,
/// 删除清单）——三者按 upsert 排序一一对应。
/// executability 跨平台不可靠（Windows 无位），恒 false（master merge 以
/// 100644 入 index；可执行位语义 v1 不保真——诚实边界）。
/// PR3（2026-10-10 机制加固）：`_debug/` 前缀 = 调试产物，双向排除——
/// 工作副本里的调试脚本/探针输出不进变更集（不合入项目），基线里残留的
/// 历史调试产物缺失也不报删除（调试产物双向都不是交付物）。
fn is_debug_artifact(path: &str) -> bool {
    path == "_debug" || path.starts_with("_debug/")
}

fn diff_tree(
    exec_dir: &Path,
    baseline_files: &[TransferFileEntry],
) -> Result<
    (
        Vec<ChangesetUpsert>,
        Vec<ChangesetContent>,
        Vec<String>,
        Vec<String>,
    ),
    String,
> {
    let mut skipped = Vec::new();
    let mut current: Vec<TransferFileEntry> = {
        let mut out = Vec::new();
        diff_walk(exec_dir, exec_dir, &mut out, &mut skipped);
        out.into_iter()
            .filter(|f| f.path != BASELINE_SIDECAR_NAME)
            .collect()
    };
    // diff_walk 已按 path 字典序；显式再排一次防上游语义变化（变更集声明序 =
    // contents 序的契约依据）。
    current.sort_by(|a, b| a.path.cmp(&b.path));

    let baseline: std::collections::HashMap<&str, &TransferFileEntry> = baseline_files
        .iter()
        .filter(|f| !is_debug_artifact(&f.path))
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
        // 收集与读取之间文件可能被进程重新锁住（竞态窗口）——同样跳过申报，
        // 不废整个变更集。
        let data = match std::fs::read(exec_dir.join(crate::transfer::safe_relative_path(&f.path)?))
        {
            Ok(d) => d,
            Err(e) => {
                skipped.push(format!("{}（读取失败: {e}）", f.path));
                continue;
            }
        };
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
        .filter(|b| !is_debug_artifact(&b.path))
        .filter(|b| !current_paths.contains(b.path.as_str()))
        .map(|b| b.path.clone())
        .collect();
    Ok((upserts, contents, deletions, skipped))
}

/// 工作副本容错收集（BUG-2026-10-15 NB-120 韧性修复）。与
/// `transfer::collect_dir_files` 同规则（`/` 相对路径、字典序、噪声目录/文件
/// 排除、符号链接跳过），差异三点：
/// 1. `_debug/` 整树**在下降时剪枝**——调试产物本就双向排除（不哈希不收
///    集），其中浏览器 profile 等活文件正是锁停摆源头；
/// 2. 单文件读失败（Windows 共享冲突 os error 32/33 等）= 跳过 + 记入
///    skipped，不废整个变更集；
/// 3. 子目录枚举失败 = 跳过该子树 + 记入 skipped。
///
/// 收集失败不构成错误，跳过清单随变更集 manifest 申报 master。
fn diff_walk(root: &Path, dir: &Path, out: &mut Vec<TransferFileEntry>, skipped: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            let rel = if dir == root {
                "_root_".to_string()
            } else {
                dir.strip_prefix(root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| dir.display().to_string())
            };
            skipped.push(format!("{rel}/（目录枚举失败: {e}）"));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let Some(rel_str) = rel.to_str() else {
            skipped.push(format!("{}（非 UTF-8 路径）", path.display()));
            continue;
        };
        let normalized = rel_str.replace('\\', "/");
        // 围栏自检：收集器自身产物也必须过围栏（畸形路径宁可不传）。
        if crate::transfer::safe_relative_path(&normalized).is_err() {
            continue;
        }
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            // `_debug/` 下降剪枝（活文件锁源头）；噪声目录同 collect 规则。
            if is_debug_artifact(&normalized) || crate::transfer::is_noise_dir(&normalized) {
                continue;
            }
            diff_walk(root, &path, out, skipped);
        } else if meta.is_file() {
            if crate::transfer::is_noise_file(&normalized) {
                continue;
            }
            match std::fs::read(&path) {
                Ok(data) => out.push(TransferFileEntry {
                    path: normalized,
                    size: data.len() as u64,
                    sha256: crate::transfer::sha256_hex(&data),
                }),
                Err(e) => skipped.push(format!("{normalized}（读取失败: {e}）")),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 3. 启动清扫（崩溃兜底）+ 周期兜底（BUG-2026-10-15 D）
// ---------------------------------------------------------------------------

/// 残留工作副本清扫（**必须在 `TransferOutbox::sweep_startup` 之前调用**，
/// 见模块头注释的幂等挡死问题）。每个带 sidecar 的 exec 目录重新走
/// [`finish_task_exec`]；成功入队的目录被清理。`skip_running` 返回 true 的
/// 任务**跳过**（周期兜底用：活跃任务的工作副本不可触碰；启动清扫传
/// `|_| false`——启动时无活跃任务）。返回处理成功数。
pub fn sweep_exec_residual(
    workspace: &Path,
    outbox: &TransferOutbox,
    skip_running: &dyn Fn(&str) -> bool,
) -> usize {
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
        // 周期兜底护栏：活跃任务的工作副本正在被 agent 使用，提前终结 =
        // 交付被拦腰斩断。
        if skip_running(&task_id) {
            continue;
        }
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

/// 周期残留清扫间隔（30 分钟）。
pub const RESIDUAL_SWEEP_PERIOD_SECS: u64 = 1800;

/// 周期残留清扫线程（BUG-2026-10-15 D，NB-120 实证 1.5h 死等）：残留 exec
/// 目录此前只有 worker 启动清扫一条路——终态钩子组装失败后「必须重启
/// worker」才能恢复。30 分钟周期兜底；活跃任务经 `is_running` 跳过。
/// 独立 std::thread（清扫是纯阻塞 fs 工作，不依赖 tokio 运行时存活）。
pub fn spawn_periodic_residual_sweep(
    workspace: PathBuf,
    outbox: std::sync::Arc<TransferOutbox>,
    is_running: std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>,
) {
    let spawned = std::thread::Builder::new()
        .name("exec-residual-sweep".into())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(RESIDUAL_SWEEP_PERIOD_SECS));
                let swept = sweep_exec_residual(&workspace, &outbox, &|id| is_running(id));
                if swept > 0 {
                    tracing::info!("[Exec] 周期残留清扫: {swept} task(s) re-enqueued");
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("[Exec] 周期残留清扫线程启动失败（启动清扫仍在，无周期兜底）: {e}");
    }
}

// ---------------------------------------------------------------------------
// 4. 残留进程清扫（BUG-2026-10-15 NB-120 A 治本）
// ---------------------------------------------------------------------------

/// 任务残留进程清扫：任务派生但未被 background_registry 树杀覆盖的
/// detached 进程（agent 起浏览器实测时的 Edge / crashpad 等）会占住工作
/// 副本内文件锁（os error 32/33），让全树 diff 整体失败、变更集永不入队。
/// 按「命令行含任务标识」匹配强杀（自进程除外），best-effort：任何失败只
/// WARN，绝不阻塞档案管线。返回杀掉的进程数。
///
/// 护栏：标识长度 <16 或含非 `[A-Za-z0-9._-]` 字符直接跳过（短通用名误伤
/// 无关进程；生产 task id 是 uuid ≥36 字符）。Windows 单次 PowerShell CIM
/// 查询（10s 超时防 CIM 服务卡死拖住任务终结）。
pub fn kill_task_leftover_processes(task_id: &str) -> usize {
    let needle = sanitize_transfer_id(task_id);
    if needle.len() < 16
        || !needle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return 0;
    }
    let self_pid = std::process::id();
    match kill_matching_cmdline(&needle, self_pid) {
        Ok(pids) => {
            if !pids.is_empty() {
                tracing::warn!(
                    task_id = %task_id,
                    pids = ?pids,
                    "[Exec] 已清扫持有任务工作副本的残留进程（防档案锁停摆）"
                );
            }
            pids.len()
        }
        Err(e) => {
            tracing::warn!(task_id = %task_id, "[Exec] 残留进程清扫失败（继续档案管线）: {e}");
            0
        }
    }
}

#[cfg(windows)]
fn kill_matching_cmdline(needle: &str, self_pid: u32) -> Result<Vec<u32>, String> {
    // 单次 PowerShell：CIM 枚举命令行 → 匹配（排除 Rust 自进程与查询进程
    // 自身——查询脚本文本里就含 needle，不排除会自杀）→ 强杀 → 回显 PID。
    let script = format!(
        "Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -like '*{needle}*' \
         -and $_.ProcessId -ne {self_pid} -and $_.ProcessId -ne $PID }} \
         | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue; $_.ProcessId }}"
    );
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    let out = run_with_timeout(cmd, std::time::Duration::from_secs(10))?;
    if !out.status.success() {
        return Err(format!(
            "powershell 查询失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|t| t.parse::<u32>().ok())
        .collect())
}

#[cfg(unix)]
fn kill_matching_cmdline(needle: &str, self_pid: u32) -> Result<Vec<u32>, String> {
    let entries = std::fs::read_dir("/proc").map_err(|e| format!("read /proc: {e}"))?;
    let needle_bytes = needle.as_bytes();
    let mut matched = Vec::new();
    for e in entries.flatten() {
        let fname = e.file_name(); // 先绑 OsString（rustc 1.98 的临时生命周期更紧）
        let Some(name) = fname.to_str() else {
            continue;
        };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let Ok(cmd) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue; // 内核线程/已退出/权限不足——跳过
        };
        if cmd.windows(needle_bytes.len()).any(|w| w == needle_bytes) {
            matched.push(pid);
        }
    }
    if matched.is_empty() {
        return Ok(Vec::new());
    }
    let pids: Vec<String> = matched.iter().map(|p| p.to_string()).collect();
    let out = std::process::Command::new("kill")
        .arg("-9")
        .args(&pids)
        .output()
        .map_err(|e| format!("spawn kill: {e}"))?;
    if !out.status.success() {
        // 逐个 kill 的部分失败（目标已退出等）不整体失败——回显实际杀到的。
        tracing::warn!("[Exec] kill 批量退出非零（部分目标可能已自行退出）");
    }
    Ok(matched)
}

/// 带超时的同步子进程运行（stdout/stderr 捕获；超时强杀）。
fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    use std::io::Read;
    use std::process::Stdio;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_end(&mut stdout);
                }
                let mut stderr = Vec::new();
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_end(&mut stderr);
                }
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("子进程超时被强杀".into());
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
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
