//! 执行档案落地回灌 + 超限出卡（看板项目档案 goal P3，master 侧）。
//!
//! - [`ingest_landed`]：`TransferSink::set_on_landed` 回调入口。worker 推来
//!   的执行档案已过 D6 manifest 核验（transfer.end 落盘前逐块 sha + 清单
//!   总量核验），本模块把它安置进项目档案目录
//!   `records/<issue>/execution/<ts>/` 并写 timeline。无处安置（非看板
//!   派发 / 存量项目无目录）= **留在收件箱可见**，不删不弃。
//! - [`note_overlimit`]：`TransferSink::set_on_overlimit` 回调入口（master
//!   收 transfer_overlimit RPC 后由 sink 转发）。worker 侧 D4 护栏拦下的
//!   诚实失败 → 决策流卡 + 收件箱卡 + 系统评论 + 评审转人工（已被自动
//!   验收 done 的回滚 in_review）。
//!
//! 纪律对齐 archive_writer（P2/C）：写失败不阻塞主流程，WARN + 审计留痕。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use nemesis_board::assignment::Actor;
use nemesis_board::models::{CommentType, IssueFilter, IssueStatus, NewComment, dispatch_state};
use nemesis_board::store::BoardStore;
use nemesis_cluster::outbox::TransferTransport;
use nemesis_cluster::transfer::{TransferOverlimit, copy_dir_recursive};

use crate::board_review::BoardReviewDeps;

/// 决策流 / 收件箱动作词：档案超护栏（D4）。
pub const ACTION_ARCHIVE_OVERLIMIT: &str = "archive_overlimit";
/// 收件箱通知词：档案超护栏（D4）。
pub const NOTIF_ARCHIVE_OVERLIMIT: &str = "archive_overlimit";
/// 决策流 / 活动词：档案回灌无处安置（诚实可见）。
pub const ACTION_ARCHIVE_ORPHANED: &str = "archive_orphaned";
/// 决策流 / 活动词：变更集 superseded 丢弃（E8 归属校验）。
pub const ACTION_ARCHIVE_SUPERSEDED: &str = "archive_superseded";
/// 决策流 / 活动词：合并停车（冲突/损坏；P4 最小停车，P5 漏斗升级）。
pub const ACTION_MERGE_PARKED: &str = "merge_parked";
/// 决策流 / 活动词：冻结期交付登记待补合并（P5/F3；解冻时串行补）。
pub const ACTION_MERGE_DEFERRED: &str = "merge_deferred";
/// 决策流 / 活动词：变更集已合并（E4）。
pub const ACTION_MERGED: &str = "changeset_merged";

/// 档案回灌入口（on_landed 回调；同步、ms 级——回调在 transfer.end 的
/// handler 内触发）。
///
/// 安置布局：`<project_dir>/records/<issue.number>/execution/<ts>/`，其下
/// 直接是执行记录文件 + `manifest.json`（传输核验凭据）+ `landed.json`。
/// 成功后清收件箱（传输闭环；worker 侧已删发件箱）。返回安置目标目录。
pub fn ingest_landed(store: &BoardStore, task_id: &str, inbox_dir: &Path) -> Option<PathBuf> {
    // issue 绑定解析（单一真相源 board.db）。
    let Some(dispatch) = store.get_dispatch(task_id).ok().flatten() else {
        tracing::debug!(
            task_id = %task_id,
            "[BoardArchive] 收到的执行档案无看板派发绑定，留在收件箱"
        );
        return None;
    };
    let issue = match store.get_issue(dispatch.issue_id) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("[BoardArchive] task {task_id} 关联 issue 读取失败（留在收件箱）: {e}");
            return None;
        }
    };
    let Some(project_id) = issue.project_id else {
        orphan_note(store, &issue, task_id, "issue 未绑定项目");
        return None;
    };
    let project = match store.get_project(project_id) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[BoardArchive] project {project_id} 读取失败（留在收件箱）: {e}");
            return None;
        }
    };
    // 存量项目（directory 未绑定）与 P2 里程碑写入同语义：静默跳过。
    let root = match project.directory.as_ref() {
        Some(d) => PathBuf::from(d),
        None => {
            tracing::debug!(
                issue = %issue.number,
                "[BoardArchive] 存量项目无档案目录，执行档案留在收件箱"
            );
            return None;
        }
    };
    // 脚手架缺失 = 目录被手删（与 P2 writers 同语义：诚实留痕，不代重建）。
    if !root.join("project.json").exists() {
        orphan_note(store, &issue, task_id, "项目档案目录脚手架缺失");
        return None;
    }

    // D6 收件凭据核验：manifest.json / landed.json 必须在场（transfer.end
    // 成功落地的产物；缺失 = 上游异常，记 missing_blocks 对前端可见）。
    let manifest_ok = inbox_dir.join("manifest.json").exists();
    let landed_ok = inbox_dir.join("landed.json").exists();
    if !manifest_ok || !landed_ok {
        let missing = format!(
            "{}:{}",
            issue.number,
            if !manifest_ok && !landed_ok {
                "manifest+landed"
            } else if !manifest_ok {
                "manifest"
            } else {
                "landed"
            }
        );
        tracing::warn!(
            "[BoardArchive] task {task_id} 收件凭据缺失（{missing}），记 missing_blocks"
        );
        note_missing_block(&root, &missing);
    }

    // 安置：files/* → execution/<ts>/，凭据随行。
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S%3f");
    let target = root
        .join("records")
        .join(&issue.number)
        .join("execution")
        .join(ts.to_string());
    if let Err(e) = std::fs::create_dir_all(&target) {
        orphan_note(store, &issue, task_id, &format!("创建安置目录失败: {e}"));
        return None;
    }
    let src_files = inbox_dir.join("files");
    if let Err(e) = copy_dir_recursive(&src_files, &target) {
        orphan_note(store, &issue, task_id, &format!("档案复制失败: {e}"));
        return None;
    }
    for cred in ["manifest.json", "landed.json"] {
        let from = inbox_dir.join(cred);
        if from.exists()
            && let Err(e) = std::fs::copy(&from, target.join(cred))
        {
            tracing::warn!("[BoardArchive] 凭据 {cred} 复制失败: {e}");
        }
    }

    // 成功：timeline + 清收件箱（传输闭环）。
    let file_count = std::fs::read_dir(&target).map(|d| d.count()).unwrap_or(0);
    if let Err(e) = nemesis_board::archive::append_timeline(
        &root,
        "archive",
        Some(&issue.number),
        "board",
        &format!("执行档案落地（task {task_id}，{file_count} 项）"),
    ) {
        tracing::warn!("[BoardArchive] timeline 写入失败（不阻塞）: {e}");
    }
    if let Err(e) = std::fs::remove_dir_all(inbox_dir) {
        tracing::warn!("[BoardArchive] 收件箱清理失败 task {task_id}: {e}");
    }
    tracing::info!(
        task_id = %task_id,
        issue = %issue.number,
        target = %target.display(),
        "[BoardArchive] 执行档案安置完成"
    );
    // P4/E4：安置成功 → 登记 PLACED 注册表 + 合并触发（落地腿；写回腿在
    // write_back_board_dispatch）。谁后到谁触发，双到即合。
    register_placed(task_id, &target);
    let attempt = merge_and_maybe_review(task_id);
    tracing::debug!(task_id = %task_id, attempt = ?attempt, "[BoardArchive] 落地腿合并触发裁决");
    Some(target)
}

/// PLACED 注册表登记（安置成功后调用；单测同款 seam 直驱合并触发的
/// 非 ingest 路径）。
pub(crate) fn register_placed(task_id: &str, dir: &Path) {
    PLACED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(task_id.to_string(), dir.to_path_buf());
}

/// 超限出卡（D4 master 侧；on_overlimit 回调）。
pub fn note_overlimit(store: &BoardStore, req: &TransferOverlimit) {
    let Some(dispatch) = store.get_dispatch(&req.task_id).ok().flatten() else {
        tracing::warn!(
            task_id = %req.task_id,
            "[BoardArchive] 超限通知无看板派发绑定，无法出卡（档案在 worker 收件箱暂停留）"
        );
        return;
    };
    let issue = match store.get_issue(dispatch.issue_id) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("[BoardArchive] 超限通知关联 issue 读取失败: {e}");
            return;
        }
    };
    let actor = Actor::system("board-archive");
    let summary = format!(
        "执行档案 {} 字节超护栏 {} 字节（board.archive.max_transfer_bytes），未传输——绝不截断，评审转人工",
        req.total_bytes, req.limit
    );
    // 决策流卡（E2 审计视图）。
    if let Err(e) = store.add_activity(
        issue.id,
        &actor,
        ACTION_ARCHIVE_OVERLIMIT,
        Some(
            &serde_json::json!({
                "verdict": "over_limit",
                "task_id": req.task_id,
                "total_bytes": req.total_bytes,
                "limit": req.limit,
                "source_node": req.source_node,
            })
            .to_string(),
        ),
    ) {
        tracing::warn!(
            "[BoardArchive] 超限决策流卡落库失败 issue {}: {e}",
            issue.number
        );
    }
    // 收件箱卡（创建者 ∪ 指派 ∪ 订阅者）。
    if let Err(e) = store.notify_dispatch_event(issue.id, NOTIF_ARCHIVE_OVERLIMIT, &summary) {
        tracing::warn!(
            "[BoardArchive] 超限收件箱卡失败 issue {}: {e}",
            issue.number
        );
    }
    // 系统评论留痕（看板列可见）。
    if let Err(e) = store.add_comment(nemesis_board::models::NewComment {
        issue_id: issue.id,
        author: actor.clone(),
        content: format!("⛔ {summary}"),
        parent_id: None,
        ctype: CommentType::System,
    }) {
        tracing::warn!("[BoardArchive] 超限评论失败 issue {}: {e}", issue.number);
    }
    // 评审转人工：已被人工/自动收货 done 的回滚 in_review（人工重看）。
    // done 是状态机终态——必须走 store 终态回滚 bypass（rollback_decision
    // 同款裁决）；普通 transition_issue 会被状态机 loud 拒绝、单据假性
    // 收货（T-XFER-5 首轮实机实证：评论/出卡全落、回滚静默失败）。
    if issue.status == IssueStatus::Done
        && let Err(e) = store.rollback_done_to_in_review(issue.id, "执行档案超护栏，评审转人工")
    {
        tracing::warn!(
            "[BoardArchive] 超限转人工回滚失败 issue {}: {e}",
            issue.number
        );
    }
}

// ---------------------------------------------------------------------------
// 内部
// ---------------------------------------------------------------------------

/// 档案无处安置的诚实留痕（决策流卡可见；档案留在收件箱）。
///
/// F-U6-1 去重（U6 实测根修）：滞留档案每轮 D5 sweep 都会重灌重试，无去重
/// 时同 task 每分钟重复 WARN + 审计入账（965 条/数小时），并把决策流面板
/// 的 500 条窗口完全挤占。同 (task, reason) 只记首次——重试静默（档案仍在
/// 收件箱，后补绑项目的成功安置路径不受影响；重启清零后首个周期再记一次，
/// 低频可接受）。
fn orphan_note(store: &BoardStore, issue: &nemesis_board::models::Issue, task_id: &str, why: &str) {
    static ORPHAN_NOTED: std::sync::LazyLock<Mutex<HashSet<(String, String)>>> =
        std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));
    if !ORPHAN_NOTED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert((task_id.to_string(), why.to_string()))
    {
        tracing::debug!(
            issue = %issue.number,
            task_id = %task_id,
            "[BoardArchive] 执行档案无处安置（重复，已留痕不再重复入账）：{why}"
        );
        return;
    }
    tracing::warn!(
        issue = %issue.number,
        task_id = %task_id,
        "[BoardArchive] 执行档案无处安置（留在收件箱）：{why}"
    );
    let _ = store.add_activity(
        issue.id,
        &Actor::system("board-archive"),
        ACTION_ARCHIVE_ORPHANED,
        Some(&serde_json::json!({ "task_id": task_id, "reason": why }).to_string()),
    );
}

/// project.json missing_blocks 追加（去重；投影损坏则 WARN 让位——投影可重建）。
fn note_missing_block(root: &Path, block: &str) {
    let Some(mut manifest) = nemesis_board::archive::read_manifest(root) else {
        tracing::warn!("[BoardArchive] project.json 不可读，missing_blocks 未记录（{block}）");
        return;
    };
    if !manifest.missing_blocks.iter().any(|b| b == block) {
        manifest.missing_blocks.push(block.to_string());
    }
    if let Err(e) = nemesis_board::archive::write_manifest(root, &manifest) {
        tracing::warn!("[BoardArchive] project.json 写回失败: {e}");
    }
}

// ---------------------------------------------------------------------------
// D5 兜底拉取 sweep（master 侧；挂 board dispatch sweep 周期调用）
// ---------------------------------------------------------------------------

/// pull 超时（RPC 往返；worker 只回 queued/no_archive，不传数据）。
const PULL_RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// 周期对比：任务终态但项目档案缺失 → 主动补齐。
///
/// 两条腿：
/// 1. **收件箱滞留重灌**：landed 但当时无处安置（issue 后补绑项目 / 目录
///    后建）的档案再试安置（成功即清收件箱）。
/// 2. **档案缺失拉取**：终态派发（done/failed）但其项目
///    `records/<issue>/execution/` 缺失、master 收件箱也没有 → 对 worker
///    发 transfer_pull 令其重推（worker 离线 = RPC 失败，下个周期重试；
///    worker 无档 = no_archive，记入 `no_archive_seen` 不再空拉——重启清零，
///    诚实语义）。
pub async fn sweep_missing_archives(
    store: &BoardStore,
    workspace: &Path,
    transport: &dyn TransferTransport,
    no_archive_seen: &tokio::sync::Mutex<std::collections::HashSet<String>>,
) {
    let inbox_root = nemesis_path::cluster_dir_in_workspace(workspace).join("inbox");

    // -- 腿 1：收件箱滞留重灌 ---------------------------------------------
    if let Ok(entries) = std::fs::read_dir(&inbox_root) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || !e.path().is_dir() {
                continue; // .staging / .done / 杂物
            }
            ingest_landed(store, &name, &e.path());
        }
    }

    // -- 腿 2：终态派发档案缺失 -------------------------------------------
    let issues = match store.list_issues(&IssueFilter::default()) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("[BoardArchive] D5 sweep issue 枚举失败: {e}");
            return;
        }
    };
    for issue in issues {
        let Some(project_id) = issue.project_id else {
            continue;
        };
        let Ok(project) = store.get_project(project_id) else {
            continue;
        };
        let Some(dir) = project.directory.as_ref() else {
            continue;
        };
        let records = PathBuf::from(dir)
            .join("records")
            .join(&issue.number)
            .join("execution");
        if records.exists() {
            continue; // 已有档案
        }
        let Ok(dispatches) = store.list_dispatches(issue.id) else {
            continue;
        };
        // 最新终态派发（dispatched_at 降序取第一个 done/failed）。
        let Some(latest) = dispatches
            .iter()
            .filter(|d| d.state == dispatch_state::DONE || d.state == dispatch_state::FAILED)
            .max_by(|a, b| a.dispatched_at.cmp(&b.dispatched_at))
        else {
            continue; // 无终态派发（在途/未派 = 档案本来就还没有）
        };
        let task_id = &latest.task_id;
        // master 收件箱滞留的算「在路上」，腿 1 已处理，不空拉。
        if inbox_root.join(task_id).exists() {
            continue;
        }
        // worker 明确无档的不再拉（防永久空转；重启清零重试）。
        if no_archive_seen.lock().await.contains(task_id) {
            continue;
        }
        let payload = serde_json::json!({ "task_id": task_id });
        match transport
            .call(
                &latest.worker_id,
                nemesis_cluster::transfer::ACTION_TRANSFER_PULL,
                payload,
                PULL_RPC_TIMEOUT,
            )
            .await
        {
            Ok(v) if v.get("status").and_then(|s| s.as_str()) == Some("queued") => {
                tracing::info!(
                    task_id = %task_id,
                    worker = %latest.worker_id,
                    "[BoardArchive] D5 兜底拉取：worker 已重推入队"
                );
            }
            Ok(v) => {
                // no_archive：worker 无执行记录可回（可能 LLM 没产生本地活动）。
                tracing::debug!(
                    task_id = %task_id,
                    reply = %v,
                    "[BoardArchive] D5 兜底拉取：worker 无档（记入 seen，不再空拉）"
                );
                no_archive_seen.lock().await.insert(task_id.clone());
            }
            Err(e) => {
                // worker 离线 / 网络失败：下个 sweep 周期自然重试。
                tracing::debug!(task_id = %task_id, "[BoardArchive] D5 兜底拉取失败（下轮重试）: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// P4/E4：合并触发（看板项目档案 goal 合并批；写回/落地双路径收敛）
//
// 合并门槛 = dispatch done（交付落定）**且** 变更集已落地。两条到达路径
// 谁后到谁触发（保序由 changeset.rs 模块注释钦定：变更集搭执行记录同一
// outbox 载荷原子落地，落地即登记）：
// - **写回腿**：peer_chat_callback → write_back_board_dispatch（dispatch 转
//   done）→ [`merge_and_maybe_review`]；
// - **落地腿**：transfer.end → on_landed → [`ingest_landed`]（安置成功登记
//   [`PLACED`] 后调 [`merge_and_maybe_review`]）。
//
// 单合并保证：PLACED 条目以 `HashMap::remove` 原子取走——并发双路径只有
// 一个线程拿得到，拿到者合并，落空者按在途返回（无害 no-op）。全局再叠
// [`MERGE_SERIAL`] 串行闸（E4：每收一个变更集合一次 commit 一次）。
// ---------------------------------------------------------------------------

/// 已安置执行档案注册表：task_id → 安置目录（records/<issue>/execution/<ts>）。
/// [`ingest_landed`] 成功安置后登记；合并消费（remove）。estop 挂起的合并
/// 条目保留在表内，release 后 [`retry_placed_merges`] 补跑。
static PLACED: std::sync::LazyLock<Mutex<HashMap<String, PathBuf>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// 已合并 task 集合（幂等防线：同任务档案重推/合并触发重入时不再二次
/// 合并+二次评审；含空集宽容路径。进程重启清零——重启后 PLACED 同样为空，
/// 语义一致）。
static MERGED: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

/// 合并全局串行闸（E4 合并时序钦定串行；std Mutex 持锁不跨 await）。
/// P5 起冲突硬解的落盘 commit（commit_resolution）同走本闸——AI 硬解耗时
/// 与合并解耦，但落盘那一下必须与在途合并互斥。
pub(crate) static MERGE_SERIAL: Mutex<()> = Mutex::new(());

/// 合并依赖（gateway 装配后期注入——BoardReviewDeps 全件可用的回调闭包
/// 构造点）。模块级 OnceLock 而非 AppState 字段：AppState 全库字面构造
/// 测试点太多（PARENT_REVIEW_HOOK 先例）。
static MERGE_DEPS: OnceLock<BoardReviewDeps> = OnceLock::new();

/// gateway 装配期注入合并依赖（重复注入 = 保留首份并返回 false——依赖集
/// 不可变，语义安全）。
pub fn install_merge_deps(deps: BoardReviewDeps) -> bool {
    MERGE_DEPS.set(deps).is_ok()
}

/// 合并触发裁决（调用方据此决定补评论/走既有路径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeAttempt {
    /// 无基线记录（存量派发/非档案管线）——调用方走既有写回路径。
    NotArchivePipeline,
    /// 派发未终态（在途）——等写回腿触发。
    WaitingDispatch,
    /// 派发已 done 但变更集未落地——等落地腿触发。
    WaitingChangeset,
    /// 急停中（护栏三不变：estop 下合并一律拒绝）。PLACED 条目保留，
    /// release 后 [`retry_placed_merges`] 补跑。
    EstopHalted,
    /// 变更集按 E8 superseded 丢弃 + 审计（派发终态失败/取消，或申报基线
    /// 与派发基线失配——旧轮次/换人迟到交付）。
    Superseded,
    /// 已合并（含空集宽容 no-op）；评审已 spawn。
    Merged,
    /// 合并撞冲突/数据损坏——停车（P4 最小停车形态；P5 起「冲突」停车
    /// 附带项目域冻结，纯损坏仍走本形态）。
    Parked { reason: String },
    /// P5/F3：项目冲突冻结中——变更集已入档案 + 登记待补合并队列
    /// （pending_merges，持久化 board.db），不合入不评审；解冻时串行补合并。
    FrozenDeferred,
    /// P5/F5 auto 档：冲突已移交 AI 硬解执行体（异步）——本函数同步部分
    /// 到此为止，后续硬解/重派/换人由 [`crate::conflict_resolver`] 接管。
    AutoResolveStarted,
}

/// 合并触发外入口（读模块级 MERGE_DEPS；gateway 运行时必已装配）。
pub fn merge_and_maybe_review(task_id: &str) -> MergeAttempt {
    let Some(deps) = MERGE_DEPS.get() else {
        tracing::warn!(
            task_id = %task_id,
            "[BoardArchive] 合并依赖未装配，触发跳过（装配缺失属装配缺陷，仅留 WARN）"
        );
        return MergeAttempt::WaitingChangeset;
    };
    merge_and_maybe_review_with(deps, task_id)
}

/// 合并触发内入口（deps 显式传入，单测直驱）。
pub fn merge_and_maybe_review_with(deps: &BoardReviewDeps, task_id: &str) -> MergeAttempt {
    let store = deps.store.as_ref();

    // 1) 基线行缺席 = 非档案管线（存量派发），不接管。
    let baseline = match store.get_dispatch_baseline(task_id) {
        Ok(Some(c)) => c,
        Ok(None) => return MergeAttempt::NotArchivePipeline,
        Err(e) => {
            tracing::warn!(task_id = %task_id, "[BoardArchive] 基线行读取失败: {e}");
            return MergeAttempt::NotArchivePipeline;
        }
    };
    // 2) 已合并任务幂等返回（重推/重入防线）。
    if MERGED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(task_id)
    {
        return MergeAttempt::Merged;
    }
    // 3) 派发状态裁决（E4 时序：合并发生在交付落定之后）。
    let Some(dispatch) = store.get_dispatch(task_id).ok().flatten() else {
        tracing::warn!(task_id = %task_id, "[BoardArchive] 有基线行但无派发行（数据异常），不接管");
        return MergeAttempt::NotArchivePipeline;
    };
    if matches!(
        dispatch.state.as_str(),
        dispatch_state::FAILED | dispatch_state::CANCELLED
    ) {
        // 终态失败/取消：档案即使已安置也绝不合入（E8 防旧 worker 污染）。
        let consumed = PLACED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(task_id);
        if let Some(target) = consumed {
            superseded_note(store, &dispatch, task_id, "派发已终态失败/取消", &target);
        }
        return MergeAttempt::Superseded;
    }
    if dispatch.state != dispatch_state::DONE {
        return MergeAttempt::WaitingDispatch;
    }
    // 4) 急停闸（护栏三不变：estop 下合并一律拒绝；PLACED 保留等 release 补跑）。
    if deps.estop.is_engaged() {
        tracing::info!(task_id = %task_id, "[BoardArchive] 急停中，合并挂起（release 后补跑）");
        return MergeAttempt::EstopHalted;
    }
    // 5) 变更集落地裁决（remove 原子取条目 = 单合并保证）。
    let Some(target) = PLACED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(task_id)
    else {
        return MergeAttempt::WaitingChangeset;
    };
    // 6) issue / 项目档案目录解析（基线行在场 ⇒ 派发时项目有目录；目录
    //    此后被手删 = 停车诚实暴露）。
    let issue = match store.get_issue(dispatch.issue_id) {
        Ok(i) => i,
        Err(e) => {
            return park_merge(
                store,
                dispatch.issue_id,
                task_id,
                &format!("关联 issue 读取失败: {e}"),
            );
        }
    };
    let root = match issue
        .project_id
        .and_then(|pid| store.get_project(pid).ok())
        .and_then(|p| p.directory)
    {
        Some(d) => PathBuf::from(d),
        None => {
            return park_merge(
                store,
                issue.id,
                task_id,
                "项目档案目录缺失（存量项目/目录被移除），无法合并",
            );
        }
    };
    // 6.5) P5/F3 冻结期交付闸：项目冲突冻结中 → 交付照常入档案（已在
    //      records/ 持久化），变更集不合入——登记待补合并队列
    //      （placement 目录落 board.db，重启后补合并仍可读），解冻时串行
    //      补合并（F4）。已派任务不打断、交付不丢，是 F3 的钦定语义。
    if let Some(pid) = issue.project_id
        && let Ok(project) = store.get_project(pid)
        && project.conflict_frozen
    {
        let entry = nemesis_board::models::PendingMerge {
            task_id: task_id.to_string(),
            issue_id: issue.id,
            placement_dir: target.display().to_string(),
            reason: "conflict_freeze".to_string(),
            parked_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        };
        if let Err(e) = store.append_pending_merge(pid, entry) {
            // 队列登记失败 = 解冻后无从补合并——诚实停车暴露，不静默丢交付。
            return park_merge(
                store,
                issue.id,
                task_id,
                &format!("待补合并队列登记失败: {e}"),
            );
        }
        let _ = store.add_activity(
            issue.id,
            &Actor::system("board-archive"),
            ACTION_MERGE_DEFERRED,
            Some(
                &serde_json::json!({
                    "task_id": task_id,
                    "project_id": pid,
                    "reason": "项目冲突冻结中，登记待补合并队列",
                })
                .to_string(),
            ),
        );
        let _ = store.add_comment(NewComment {
            issue_id: issue.id,
            author: Actor::system("board-archive"),
            content: format!(
                "⏸ 项目冲突冻结中（task {task_id}）：变更集已入档案（{}），暂不合入；解冻后自动补合并。",
                target.display()
            ),
            parent_id: None,
            ctype: CommentType::System,
        });
        tracing::info!(
            task_id = %task_id,
            issue = %issue.number,
            "[BoardArchive] 项目冻结中，变更集登记待补合并（FrozenDeferred）"
        );
        return MergeAttempt::FrozenDeferred;
    }
    // 7) 读变更集（安置目录 = worker 载荷 files 根同形：changeset/ 平面在场）。
    let changeset = match nemesis_cluster::changeset::read_changeset(&target) {
        Some(Ok(pair)) => Some(pair),
        Some(Err(e)) => {
            return park_merge(store, issue.id, task_id, &format!("变更集损坏：{e}"));
        }
        None => None,
    };
    // 8) 合并串行闸（全局单飞；git 合并 ms 级，同步持锁无碍）。
    let _serial = MERGE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let Some((manifest, contents)) = changeset else {
        // 空集宽容（goal E4）：非文件型任务——无变更集直接进评审（仓库
        // no-op，不产空 commit）。
        MERGED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task_id.to_string());
        return finish_merged_review(
            store,
            &issue,
            "📦 无变更集交付（非文件型任务），直接进入验收评审。",
            None,
        );
    };
    // 9) E8 归属校验：worker 申报基线必须等于本派发下发的基线——旧轮次/
    //    换人迟到的变更集一律丢弃 + 审计，绝不合入。
    if manifest.base_commit != baseline {
        superseded_note(
            store,
            &dispatch,
            task_id,
            &format!(
                "变更集申报基线 {} 与派发基线 {baseline} 失配（旧轮次/换人迟到交付）",
                manifest.base_commit
            ),
            &target,
        );
        return MergeAttempt::Superseded;
    }
    // 10) 合并前置：人工未提交改动先落一笔（merge_changeset 契约：调用方
    //     保证工作区干净；tree 无变化 = no-op）。
    if let Err(e) = nemesis_board::git_repo::ensure_repo(&root) {
        return park_merge(
            store,
            issue.id,
            task_id,
            &format!("档案仓库 ensure 失败: {e}"),
        );
    }
    if let Err(e) = nemesis_board::git_repo::commit_worktree(
        &root,
        &format!("pre-merge: 合并前工作集落定（task {task_id}）"),
    ) {
        return park_merge(
            store,
            issue.id,
            task_id,
            &format!("合并前工作集 commit 失败: {e}"),
        );
    }
    let input = nemesis_board::git_repo::MergeInput {
        baseline_commit: manifest.base_commit,
        upserts: contents
            .into_iter()
            .map(|c| nemesis_board::git_repo::ChangesetFile {
                path: c.path,
                content: c.content,
                executable: c.executable,
            })
            .collect(),
        deletions: manifest.deletions,
    };
    match nemesis_board::git_repo::merge_changeset(&root, &input) {
        Ok(nemesis_board::git_repo::MergeOutcome::Merged { commit_oid }) => {
            MERGED
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(task_id.to_string());
            let _ = store.add_activity(
                issue.id,
                &Actor::system("board-archive"),
                ACTION_MERGED,
                Some(
                    &serde_json::json!({
                        "task_id": task_id,
                        "commit": commit_oid,
                        "worker": dispatch.worker_id,
                    })
                    .to_string(),
                ),
            );
            tracing::info!(
                task_id = %task_id,
                issue = %issue.number,
                commit = %commit_oid,
                "[BoardArchive] 变更集已合并"
            );
            finish_merged_review(
                store,
                &issue,
                &format!(
                    "🔀 变更集已合并（commit {}），进入验收评审。",
                    &commit_oid[..12.min(commit_oid.len())]
                ),
                Some(&root),
            )
        }
        Ok(nemesis_board::git_repo::MergeOutcome::Conflict { files }) => {
            handle_merge_conflict(deps, store, &issue, &dispatch, task_id, files)
        }
        Err(e) => park_merge(store, issue.id, task_id, &format!("三方合并执行失败: {e}")),
    }
}

/// P5 冲突漏斗入口（F2+F5）：真冲突按 `conflict_auto_resolve` 分流——
/// human 档（默认）：项目域冻结（仅本项目，F2 不株连）+ 四审计之
/// `conflict` + 停车明细卡（冲突文件清单+双方改动摘要）；auto 档：移交
/// AI 硬解执行体（[`crate::conflict_resolver`]，异步接管后续硬解/重派/
/// 换人），项目不停摆。
fn handle_merge_conflict(
    deps: &BoardReviewDeps,
    store: &BoardStore,
    issue: &nemesis_board::models::Issue,
    dispatch: &nemesis_board::models::DispatchRecord,
    task_id: &str,
    files: Vec<nemesis_board::git_repo::ConflictFile>,
) -> MergeAttempt {
    let detail = render_conflict_detail(&files);
    // F1 开关现读（与 load_board_flags 同语义）；读失败 = 诚实按 human 档
    // 停车——配置坏了不放大自动处置权限。
    let auto = crate::board_review::load_board_flags(&deps.home)
        .map(|f| f.conflict_auto_resolve)
        .unwrap_or_else(|e| {
            tracing::warn!("[BoardArchive] board 旗标读取失败，冲突按 human 档停车: {e}");
            false
        });
    if auto {
        tracing::info!(
            task_id = %task_id,
            issue = %issue.number,
            files = files.len(),
            "[BoardArchive] 冲突移交 AI 硬解（auto 档）"
        );
        crate::conflict_resolver::spawn_conflict_resolver(
            deps.clone(),
            issue.clone(),
            dispatch.worker_id.clone(),
            task_id.to_string(),
            files,
        );
        return MergeAttempt::AutoResolveStarted;
    }
    // F2 human 档：冻结只落本项目（conflict_frozen），不动 ProjectStatus
    // 状态机；冲突单走既有停车场。
    if let Some(pid) = issue.project_id
        && let Err(e) = store.set_project_conflict_frozen(pid, true)
    {
        tracing::warn!("[BoardArchive] 项目 {pid} 冻结置位失败（继续停车，冻结缺位人工兜底）: {e}");
    }
    crate::board_review::record_auto_decide(
        store,
        issue.id,
        deps.cluster.node_id(),
        "conflict",
        "parked",
        serde_json::json!({
            "task_id": task_id,
            "worker": dispatch.worker_id,
            "mode": "human",
            "conflict_files": files
                .iter()
                .map(|f| serde_json::json!({ "path": f.path, "binary": f.binary }))
                .collect::<Vec<_>>(),
        }),
    );
    park_merge(
        store,
        issue.id,
        task_id,
        &format!("三方合并冲突，项目已冻结待人工解（project.resume 续行）：\n{detail}"),
    )
}

/// 冲突明细卡渲染（F4：文件清单 + 双方改动摘要）。文本文件给逐侧相对
/// 基线的新增行摘要（朴素行集差，各侧截 8 行）+ 行数统计；二进制只给
/// 字节数（内容不可行级展示）。摘要≠补丁——精确内容在档案 records/ 与
/// 仓库历史里。
fn render_conflict_detail(files: &[nemesis_board::git_repo::ConflictFile]) -> String {
    let mut out = String::new();
    for f in files {
        out.push_str(&format!("- `{}`", f.path));
        if f.binary {
            let ours_b = f.ours.as_ref().map(|d| d.len()).unwrap_or(0);
            let theirs_b = f.theirs.as_ref().map(|d| d.len()).unwrap_or(0);
            out.push_str(&format!(
                "（二进制，择边语义：我方 {ours_b}B / 对方 {theirs_b}B）\n"
            ));
            continue;
        }
        let ours_txt = String::from_utf8_lossy(f.ours.as_deref().unwrap_or(b""));
        let theirs_txt = String::from_utf8_lossy(f.theirs.as_deref().unwrap_or(b""));
        let ancestor_txt = String::from_utf8_lossy(f.ancestor.as_deref().unwrap_or(b""));
        let summary = |side: &str, cur: &str| {
            let base: std::collections::HashSet<&str> = ancestor_txt
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            let added: Vec<&str> = cur
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !base.contains(l))
                .collect();
            if added.is_empty() {
                format!("{side}（相对基线无新增行）")
            } else {
                let shown: Vec<String> = added
                    .iter()
                    .take(8)
                    .map(|l| {
                        let t = l.chars().take(80).collect::<String>();
                        format!("  + {t}")
                    })
                    .collect();
                let more = if added.len() > 8 {
                    format!("\n  …（共 {} 行新增）", added.len())
                } else {
                    String::new()
                };
                format!("{side}\n{}{}", shown.join("\n"), more)
            }
        };
        out.push_str(&format!(
            "（我方 {} 行 / 对方 {} 行 / 基线 {} 行）\n{}\n{}\n",
            ours_txt.lines().count(),
            theirs_txt.lines().count(),
            ancestor_txt.lines().count(),
            summary("  我方（当前仓库）改动摘要：", &ours_txt),
            summary("  对方（worker 变更集）改动摘要：", &theirs_txt),
        ));
    }
    out
}

/// estop release 补跑：PLACED 里尚存的条目逐个重试合并（在途派发自然按
/// Waiting* 跳过）。estop release watcher 在释放沿调用。
pub fn retry_placed_merges(deps: &BoardReviewDeps) {
    let keys: Vec<String> = PLACED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .keys()
        .cloned()
        .collect();
    for task_id in keys {
        let attempt = merge_and_maybe_review_with(deps, &task_id);
        if !matches!(
            attempt,
            MergeAttempt::WaitingDispatch
                | MergeAttempt::WaitingChangeset
                | MergeAttempt::NotArchivePipeline
        ) {
            tracing::info!(task_id = %task_id, attempt = ?attempt, "[BoardArchive] estop release 补跑合并");
        }
    }
}

// ---------------------------------------------------------------------------
// P5/F4：project.resume 解冻续行——补合并队列串行回放
// ---------------------------------------------------------------------------

/// 解冻补合并回放（project.resume 的执行臂；nemesis-web handler 经
/// [`install_resume_replay_hook`] 注入的回调调到这里——依赖方向 nemesis-web
/// ← nemesisbot，钩子是唯一通路）。
///
/// F4 钦定顺序：清 `conflict_frozen` → 工作树未提交改动自动 commit
///（message=`manual conflict resolution`，审计留痕）→ 串行补合并
/// `pending_merges`（F3，按完成顺序 pop）→ 返回摘要（调用方据此决定是否
/// 继续恢复派发）。补合并再冲突 → **重新冻结** + 冲突单走漏斗停车
///（resume 是人工在场流程，再冲突回到人工，不进 auto 硬解）+ 剩余队列
/// 原地保留（逐条 pop 的天然性质）。
pub fn replay_pending_merges(
    deps: &BoardReviewDeps,
    project_id: i64,
) -> Result<serde_json::Value, String> {
    let store = deps.store.as_ref();
    if deps.estop.is_engaged() {
        return Err("急停中，暂不解冻（先释放急停再 resume）".to_string());
    }
    let project = store.get_project(project_id)?;
    if !project.conflict_frozen {
        return Ok(serde_json::json!({
            "unfrozen": false, "note": "项目未处于冲突冻结，无需补合并",
            "merged": 0, "superseded": 0, "parked": [], "refrozen": false,
        }));
    }
    let Some(root) = project.directory.clone() else {
        return Err("项目未绑定档案目录，无从补合并".to_string());
    };
    let root = PathBuf::from(root);

    // 1) 清冻结（先清再做后续动作——补合并再冲突会重新置位）。
    store.set_project_conflict_frozen(project_id, false)?;

    // 2) 人工解落定 commit（工作树无改动 = no-op 不产空 commit；有改动 =
    //    用户的 manual resolution 入库，审计留痕在 timeline + 本摘要）。
    let manual_commit = match nemesis_board::git_repo::ensure_repo(&root) {
        Ok(_) => nemesis_board::git_repo::commit_worktree(
            &root,
            "manual conflict resolution（project.resume 解冻续行；人工冲突处理落定）",
        ),
        Err(e) => Err(e),
    }
    .map_err(|e| format!("人工解 commit 失败: {e}"))?;
    let _ = nemesis_board::archive::append_timeline(
        &root,
        "admin",
        None,
        "board",
        &format!(
            "project.resume 解冻续行；人工冲突处理 commit: {}",
            manual_commit
                .as_deref()
                .unwrap_or("(工作树无改动，未产 commit)")
        ),
    );

    // 3) 串行补合并（每条独立 try；一条损坏不连坐其余）。
    let mut merged = 0usize;
    let mut superseded = 0usize;
    let mut parked: Vec<String> = Vec::new();
    let mut refrozen = false;
    while let Some(entry) = store.pop_pending_merge(project_id)? {
        match replay_one_pending(deps, project_id, &root, &entry) {
            PendingReplay::Merged => merged += 1,
            PendingReplay::Superseded => superseded += 1,
            PendingReplay::Parked(reason) => parked.push(format!("{}: {}", entry.task_id, reason)),
            PendingReplay::RefrozenConflict => {
                refrozen = true;
                parked.push(format!("{}: 补合并再冲突，项目重新冻结", entry.task_id));
                break; // 剩余队列原地保留（未 pop），下轮 resume 继续
            }
        }
    }
    Ok(serde_json::json!({
        "unfrozen": !refrozen,
        "manual_commit": manual_commit,
        "merged": merged,
        "superseded": superseded,
        "parked": parked,
        "refrozen": refrozen,
    }))
}

/// 单条补合并结果。
enum PendingReplay {
    Merged,
    Superseded,
    Parked(String),
    /// 补合并再冲突——已重新冻结，调用方停止回放。
    RefrozenConflict,
}

fn replay_one_pending(
    deps: &BoardReviewDeps,
    project_id: i64,
    root: &Path,
    entry: &nemesis_board::models::PendingMerge,
) -> PendingReplay {
    let store = deps.store.as_ref();
    let target = PathBuf::from(&entry.placement_dir);
    let Some(issue) = store.get_issue(entry.issue_id).ok() else {
        return PendingReplay::Parked("关联 issue 读取失败".to_string());
    };
    let _serial = MERGE_SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let (manifest, contents) = match nemesis_cluster::changeset::read_changeset(&target) {
        // 无变更集目录 = 非文件型交付（空集宽容同款）：直接进评审。
        None => {
            MERGED
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(entry.task_id.clone());
            finish_merged_review(
                store,
                &issue,
                "📦 无变更集交付（非文件型任务，冻结期入队），直接进入验收评审。",
                None,
            );
            return PendingReplay::Merged;
        }
        Some(Ok(pair)) => pair,
        Some(Err(e)) => return PendingReplay::Parked(format!("变更集损坏：{e}")),
    };
    // E8 归属校验照常（冻结期长，迟到/换轮概率更高）。
    let baseline = store.get_dispatch_baseline(&entry.task_id).ok().flatten();
    match baseline {
        None => {
            return PendingReplay::Parked("无派发基线记录（非档案管线形态）".to_string());
        }
        Some(b) if b != manifest.base_commit => {
            if let Ok(Some(dispatch)) = store.get_dispatch(&entry.task_id) {
                superseded_note(
                    store,
                    &dispatch,
                    &entry.task_id,
                    &format!(
                        "补合并时变更集申报基线 {} 与派发基线 {b} 失配（迟到/换轮交付）",
                        manifest.base_commit
                    ),
                    &target,
                );
            }
            return PendingReplay::Superseded;
        }
        Some(_) => {}
    }
    if let Err(e) = nemesis_board::git_repo::ensure_repo(root) {
        return PendingReplay::Parked(format!("档案仓库 ensure 失败: {e}"));
    }
    if let Err(e) = nemesis_board::git_repo::commit_worktree(
        root,
        &format!("pre-merge: 补合并前工作集落定（task {}）", entry.task_id),
    ) {
        return PendingReplay::Parked(format!("补合并前工作集 commit 失败: {e}"));
    }
    let input = nemesis_board::git_repo::MergeInput {
        baseline_commit: manifest.base_commit,
        upserts: contents
            .into_iter()
            .map(|c| nemesis_board::git_repo::ChangesetFile {
                path: c.path,
                content: c.content,
                executable: c.executable,
            })
            .collect(),
        deletions: manifest.deletions,
    };
    match nemesis_board::git_repo::merge_changeset(root, &input) {
        Ok(nemesis_board::git_repo::MergeOutcome::Merged { commit_oid }) => {
            MERGED
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(entry.task_id.clone());
            let _ = store.add_activity(
                issue.id,
                &Actor::system("board-archive"),
                ACTION_MERGED,
                Some(
                    &serde_json::json!({
                        "task_id": entry.task_id,
                        "commit": commit_oid,
                        "reason": "解冻补合并",
                    })
                    .to_string(),
                ),
            );
            finish_merged_review(
                store,
                &issue,
                &format!(
                    "🔀 解冻补合并完成（commit {}），进入验收评审。",
                    &commit_oid[..12.min(commit_oid.len())]
                ),
                Some(root),
            );
            PendingReplay::Merged
        }
        Ok(nemesis_board::git_repo::MergeOutcome::Conflict { files }) => {
            // 再冲突 → 重新冻结（resume 是人工在场流程，回人工不进 auto）。
            if let Err(e) = store.set_project_conflict_frozen(project_id, true) {
                tracing::warn!(
                    "[BoardArchive] 补合并再冲突重新冻结失败: {e}（冻结缺位，人工兜底）"
                );
            }
            let detail = render_conflict_detail(&files);
            crate::board_review::record_auto_decide(
                store,
                issue.id,
                deps.cluster.node_id(),
                "conflict",
                "parked",
                serde_json::json!({
                    "task_id": entry.task_id,
                    "mode": "human_replay",
                    "conflict_files": files
                        .iter()
                        .map(|f| serde_json::json!({ "path": f.path, "binary": f.binary }))
                        .collect::<Vec<_>>(),
                }),
            );
            let reason = format!("补合并再冲突，项目重新冻结待人工解：\n{detail}");
            park_merge(store, issue.id, &entry.task_id, &reason);
            PendingReplay::RefrozenConflict
        }
        Err(e) => PendingReplay::Parked(format!("补合并执行失败: {e}")),
    }
}

/// 合并收口：系统评论 + 转 in_review + spawn 评审（正常合并与空集宽容
/// 共用）。transition 失败 = 单据已被别处推进（人工/并发）——不 spawn 评审
/// 不代推进，留 WARN 诚实让位。`root` 在场时补 timeline。
/// P5 起冲突硬解成功路径（conflict_resolver）共用本收口。
pub(crate) fn finish_merged_review(
    store: &BoardStore,
    issue: &nemesis_board::models::Issue,
    comment: &str,
    root: Option<&Path>,
) -> MergeAttempt {
    if let Some(root) = root
        && let Err(e) = nemesis_board::archive::append_timeline(
            root,
            "archive",
            Some(&issue.number),
            "board",
            comment,
        )
    {
        tracing::warn!("[BoardArchive] 合并 timeline 写入失败（不阻塞）: {e}");
    }
    if let Err(e) = store.add_comment(NewComment {
        issue_id: issue.id,
        author: Actor::system("board-archive"),
        content: comment.to_string(),
        parent_id: None,
        ctype: CommentType::System,
    }) {
        tracing::warn!("[BoardArchive] 合并评论失败 issue {}: {e}", issue.number);
    }
    match store.transition_issue(
        issue.id,
        IssueStatus::InReview,
        &Actor::system("board-archive"),
    ) {
        Ok(_) => {
            // 评审触发：模块级 MERGE_DEPS 在场才 spawn（外入口链路必在；
            // 直接持 deps 的调用方——estop 补跑——同样成立）。
            if let Some(deps) = MERGE_DEPS.get() {
                crate::board_review::spawn_board_review(deps.clone(), issue.id);
            }
            MergeAttempt::Merged
        }
        Err(e) => {
            tracing::warn!(
                "[BoardArchive] 合并后转 in_review 失败 issue {}（单据已被别处推进？不 spawn 评审）: {e}",
                issue.number
            );
            MergeAttempt::Merged
        }
    }
}

/// E8 superseded 审计：决策流卡 + 系统评论 + WARN。执行档案已安置的留在
/// records/（诚实存档），变更集绝不入仓库。
fn superseded_note(
    store: &BoardStore,
    dispatch: &nemesis_board::models::DispatchRecord,
    task_id: &str,
    reason: &str,
    target: &Path,
) {
    tracing::warn!(task_id = %task_id, "[BoardArchive] 变更集 superseded 丢弃：{reason}");
    let _ = store.add_activity(
        dispatch.issue_id,
        &Actor::system("board-archive"),
        ACTION_ARCHIVE_SUPERSEDED,
        Some(
            &serde_json::json!({
                "task_id": task_id,
                "reason": reason,
                "archive_dir": target.display().to_string(),
            })
            .to_string(),
        ),
    );
    let _ = store.add_comment(NewComment {
        issue_id: dispatch.issue_id,
        author: Actor::system("board-archive"),
        content: format!(
            "🗑 变更集丢弃（superseded）：{reason}。执行档案已存档 {}，不入仓库。",
            target.display()
        ),
        parent_id: None,
        ctype: CommentType::System,
    });
}

/// P4 最小停车：决策流卡 + ⚠ 系统评论。单据不进 in_review（goal E4：合并
/// 失败不进评审）；P5 漏斗接手项目域冻结 / auto 硬解。冲突硬解执行体的
/// 兜底停车（fallback_freeze）共用本出口。
pub(crate) fn park_merge(
    store: &BoardStore,
    issue_id: i64,
    task_id: &str,
    reason: &str,
) -> MergeAttempt {
    tracing::warn!(task_id = %task_id, "[BoardArchive] 合并停车：{reason}");
    let _ = store.add_activity(
        issue_id,
        &Actor::system("board-archive"),
        ACTION_MERGE_PARKED,
        Some(
            &serde_json::json!({
                "task_id": task_id,
                "reason": reason,
            })
            .to_string(),
        ),
    );
    let _ = store.add_comment(NewComment {
        issue_id,
        author: Actor::system("board-archive"),
        content: format!("⚠ 合并停车（task {task_id}）：{reason}"),
        parent_id: None,
        ctype: CommentType::System,
    });
    MergeAttempt::Parked {
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests;
