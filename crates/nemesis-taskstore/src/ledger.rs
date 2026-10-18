//! 运行账本漏斗（G3，2026-10-18 三方复评整改批五）。
//!
//! runs/attempts 统一层（`runs.rs`）进生产调度的**单一适配面**：board 派发、
//! cron 触发、workflow 执行三类调度器经本漏斗记账。集群任务的 runs 行由
//! `SqliteTaskStore` 适配器既有落盘（runs.id = task.id、状态机全包），
//! 本漏斗对集群只补 **attempts 归因账**（谁领的、何时开跑、何时收口）。
//!
//! 设计要点：
//! - **全局安装槽**：gateway 启动 `install` 一次（独立连接，多连接 WAL 是
//!   既定形态）；未安装 = 所有调用零变化。记账面绝不阻断生产——best-effort，
//!   失败 debug/warn 留痕。
//! - **确定性 run id**：`board_dispatch:{task_id}` / `cron_job:{job_id}:{ms}` /
//!   `workflow_run:{execution_id}`——id 先于内容确定，跨重启凭 id 直解，
//!   不需要内存关联表；与 `SqliteTaskStore` 落的裸 task.id 行不冲突
//!   （kind 前缀区分：board/cron/workflow 行是派发/触发/执行生命周期的
//!   账面，与传输层 task 行互补不重叠）。
//! - **带 db 的核心变体**（`*_on`）供单测在临时库上验证漏斗语义，不触全局
//!   槽（并行测试互不污染）。
//! - **诚实边界**：审批跨重启续执行不做（audit-only，文档认可的边界）；
//!   boot sweep 只圈 cron_job（单发语义，网关死亡 = 该次触发失败）——
//!   board/cluster/workflow 的在途 run 由 G5 恢复、dispatch sweep、
//!   resume 链等生产收口路径负责，重启不等于死亡。

use std::sync::Arc;

use crate::db::TaskDb;
use crate::runs::NewRun;

static LEDGER: std::sync::OnceLock<Arc<TaskDb>> = std::sync::OnceLock::new();

/// 安装全局账本（gateway 启动一次；重复安装 = Err，调用方 warn 即可）。
pub fn install(db: Arc<TaskDb>) -> Result<(), String> {
    LEDGER
        .set(db)
        .map_err(|_| "run ledger already installed".to_string())
}

/// 当前全局账本（未安装 = None——测试/无 db 形态，所有调用零变化）。
pub fn installed() -> Option<Arc<TaskDb>> {
    LEDGER.get().cloned()
}

// -- 确定性 run id -----------------------------------------------------------

/// board 派发 run id（task_id 来自 submit_peer_chat）。
pub fn board_dispatch_run_id(task_id: &str) -> String {
    format!("board_dispatch:{task_id}")
}

/// workflow 执行 run id。
pub fn workflow_run_id(execution_id: &str) -> String {
    format!("workflow_run:{execution_id}")
}

/// cron 单次触发 run id（同 job 不同次触发天然区分；毫秒时间戳取自
/// fire 循环的 tick 时钟）。
pub fn cron_job_run_id(job_id: &str, fired_at_ms: i64) -> String {
    format!("cron_job:{job_id}:{fired_at_ms}")
}

// -- 核心（带 db；单测直测面）--------------------------------------------------

/// 开账：建 run（确定性 id；已在账=幂等）+ 置 running + 首个 attempt。
/// 已有在跑 attempt 时不再叠加（防御性重入不产生双开账）。失败 debug
/// 留痕不阻断（记账面 best-effort）。
pub fn begin_run_on(
    db: &TaskDb,
    run_id: &str,
    kind: &str,
    title: &str,
    worker: &str,
    payload: serde_json::Value,
) {
    let new_run = NewRun {
        kind: kind.to_string(),
        title: title.to_string(),
        action: "run".to_string(),
        peer_id: worker.to_string(),
        payload,
    };
    if let Err(e) = db.create_run_with_id(run_id, &new_run) {
        tracing::debug!("[ledger] begin_run {run_id} create failed: {e}");
        return;
    }
    if let Err(e) = db.set_run_status(run_id, "running") {
        // 已被并发方收口到终态等竞争面：账面以先到者为准，debug 即可。
        tracing::debug!("[ledger] begin_run {run_id} -> running failed: {e}");
        return;
    }
    let has_open_attempt = db
        .attempts_of(run_id)
        .map(|a| a.iter().any(|x| x.status == "running"))
        .unwrap_or(false);
    if !has_open_attempt && let Err(e) = db.begin_attempt(run_id, worker) {
        tracing::debug!("[ledger] begin_run {run_id} attempt failed: {e}");
    }
}

/// 收口：终结全部在跑 attempt + run 终态（CAS 幂等：重复收口同值 Ok、
/// 异终态竞争败方 Err——都 debug 留痕不阻断）。
pub fn settle_run_on(db: &TaskDb, run_id: &str, status: &str, error: Option<&str>) {
    finish_open_attempts_on(db, run_id, status, error);
    if let Err(e) = db.set_run_status(run_id, status) {
        tracing::debug!("[ledger] settle {run_id} -> {status} failed: {e}");
    }
}

/// 集群任务 attempt 归因账（runs 行由 `SqliteTaskStore` 适配器落盘并
/// 维护状态机，本漏斗**只补 attempts**）：assign=开账。
pub fn begin_cluster_attempt_on(db: &TaskDb, task_id: &str, worker: &str) {
    if let Err(e) = db.begin_attempt(task_id, worker) {
        tracing::debug!("[ledger] cluster attempt {task_id} begin failed: {e}");
    }
}

/// 集群任务 attempt 归因收账（settle/timeout 回收点调用；不触 run 状态
/// ——那是适配器 update_result_if_status 的职责，双写会造成两套收口时序）。
pub fn finish_cluster_attempt_on(db: &TaskDb, task_id: &str, status: &str, error: Option<&str>) {
    finish_open_attempts_on(db, task_id, status, error);
}

fn finish_open_attempts_on(db: &TaskDb, run_id: &str, status: &str, error: Option<&str>) {
    let Ok(attempts) = db.attempts_of(run_id) else {
        return;
    };
    for a in attempts.iter().filter(|a| a.status == "running") {
        if let Err(e) = db.finish_attempt(a.id, status, error, None) {
            tracing::debug!("[ledger] settle {run_id} attempt {} failed: {e}", a.id);
        }
    }
}

/// 启动 sweep：把上一进程遗留的未收口 cron_job 触发诚实标
/// `failed("gateway restart")`。返回归位条数。board/cluster/workflow
/// 在途 run 不在此圈（各自生产收口路径负责，见模块头「诚实边界」）。
pub fn sweep_restart_orphans_on(db: &TaskDb) -> usize {
    let Ok(runs) = db.list_open_runs("cron_job") else {
        return 0;
    };
    let mut n = 0;
    for r in &runs {
        settle_run_on(db, &r.id, "failed", Some("gateway restart"));
        n += 1;
    }
    n
}

// -- 全局包装（生产调用面；未安装 = 零变化）------------------------------------

/// 开账（生产面）。
pub fn begin_run(run_id: &str, kind: &str, title: &str, worker: &str, payload: serde_json::Value) {
    if let Some(db) = installed() {
        begin_run_on(&db, run_id, kind, title, worker, payload);
    }
}

/// 收口（生产面）。
pub fn settle_run(run_id: &str, status: &str, error: Option<&str>) {
    if let Some(db) = installed() {
        settle_run_on(&db, run_id, status, error);
    }
}

/// 集群任务 attempt 开账（生产面）。
pub fn begin_cluster_attempt(task_id: &str, worker: &str) {
    if let Some(db) = installed() {
        begin_cluster_attempt_on(&db, task_id, worker);
    }
}

/// 集群任务 attempt 收账（生产面）。
pub fn finish_cluster_attempt(task_id: &str, status: &str, error: Option<&str>) {
    if let Some(db) = installed() {
        finish_cluster_attempt_on(&db, task_id, status, error);
    }
}

/// 启动 sweep（生产面；未安装 = 0）。
pub fn sweep_restart_orphans_installed() -> usize {
    installed()
        .map(|db| sweep_restart_orphans_on(&db))
        .unwrap_or(0)
}
