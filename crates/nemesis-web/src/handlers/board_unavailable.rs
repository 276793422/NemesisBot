//! U-B（2026-10-12 无人值守 §9.2）：LLM 端点不可用本地封存。
//!
//! 三机共享一个云端模型端点是集群常态——端点挂掉时，master 侧拆解/评审/
//! 自动重派链会各自撞连接失败并烧重试预算（N2 重评链、拆解重拆链、停车场
//! sweep 反复发车）。本模块实现 master 侧 circuit breaker：
//!
//! - **检测**：只认**连接建立类**失败（[`nemesis_utils::llm_error_class::is_connect_error`]）
//!   ——超时不计入（慢模型跑满 provider 600s 是端点活着的证据，计入会误
//!   封存）；4xx/5xx/解析失败同理不计入。连续 `connect_fail_threshold`
//!   次连接类失败 → 端点 unavailable（封存）。
//! - **封存**：只短路「下一步需要 LLM 的自动动作」——拆解发起
//!   （[`super::board::execute_plan_chain`] 入口闸）、评审触发（nemesisbot
//!   board_review 三入口闸）、停车场 sweep 重派（[`super::board::sweep_parked_dispatches_with_config`]
//!   闸）。不需 LLM 的动作（dossier 导出/锚点执行/评论状态变更）照常。
//!   在途工作逐一写**中断档案** `<workspace>/board/interruptions/<NB-x>.json`
//!   （端点恢复后探针逐单续做）。
//! - **恢复**：探针在 nemesisbot 侧（`nemesisbot/src/board_unavailable.rs`，
//!   走 default_slot 裸 provider 最小 completion——TCP 通 ≠ LLM 活），解封
//!   后读中断档案逐单走既有恢复路径，档案归档不删。
//! - **与 estop 的关系**：estop 优先级更高（estop 挂起时探针不探测不恢复）；
//!   封存独立于 estop（estop release 不解封，只有探针/手动放行解封）。
//!
//! 状态是进程级单例（[`UNAVAILABLE`] OnceLock）——同 [`super::board::BOARD_ESTOP`]
//! 理由（AppState 字面量测试点太多）：**判定逻辑全部在可实例化的
//! [`UnavailableState`] 方法上**，单测自建实例零全局污染（estop 纯函数
//! 内核同款纪律）；全局壳函数只做一行委托。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use nemesis_board::models::Issue;
use nemesis_board::store::BoardStore;

/// 模块级状态单例（gateway 未装配也恒有实例——get_or_init 零成本懒构造；
/// 未装配 config 读取失败的默认值 freeze=true，封存语义是无人值守默认正确
/// 行为）。
static UNAVAILABLE: OnceLock<UnavailableState> = OnceLock::new();

fn state() -> &'static UnavailableState {
    UNAVAILABLE.get_or_init(UnavailableState::new)
}

// ---------------------------------------------------------------------------
// 状态机（可实例化——单测自建实例，不碰全局）
// ---------------------------------------------------------------------------

/// 封存状态（streak / frozen / trip 现场）。字段私有，测试经 [`UnavailableState::new`]
/// 构造后走公开方法驱动。
pub struct UnavailableState {
    /// 连续连接类失败计数（成功清零；freeze=false 时不累计）。
    streak: AtomicU32,
    /// 封存中。
    frozen: AtomicBool,
    /// trip 时刻（RFC3339）与触发原因（末次连接错误截断）。
    tripped_at: Mutex<Option<String>>,
    reason: Mutex<String>,
}

impl Default for UnavailableState {
    fn default() -> Self {
        Self::new()
    }
}

impl UnavailableState {
    pub fn new() -> Self {
        Self {
            streak: AtomicU32::new(0),
            frozen: AtomicBool::new(false),
            tripped_at: Mutex::new(None),
            reason: Mutex::new(String::new()),
        }
    }

    /// 是否封存中。
    pub fn is_frozen(&self) -> bool {
        self.frozen.load(Ordering::SeqCst)
    }

    /// 当前连续失败计数（观测用）。
    pub fn streak(&self) -> u32 {
        self.streak.load(Ordering::SeqCst)
    }

    /// 记录一次 LLM 失败。返回 `true` = 本次失败**恰好触发封存**（调用方
    /// 据此写中断档案/发 WARN——只在边沿 true，封存持续期的后续失败返回
    /// false，防重复副作用）。
    ///
    /// `cfg.freeze=false` = 功能关（旧行为：失败各自 fail_class 处理），
    /// 不累计不封存。非连接类失败（超时/4xx/5xx/解析）不计入。
    pub fn record_failure(&self, err: &str, cfg: &nemesis_config::BoardUnavailableConfig) -> bool {
        if !cfg.freeze || !nemesis_utils::llm_error_class::is_connect_error(err) {
            return false;
        }
        let streak = self.streak.fetch_add(1, Ordering::SeqCst) + 1;
        // 阈值 0/负数视作 1（一败即封——用户显式配 0 的意图是激进封存）。
        if streak < cfg.connect_fail_threshold.max(1) {
            return false;
        }
        let first = !self.frozen.swap(true, Ordering::SeqCst);
        if first {
            if let Ok(mut at) = self.tripped_at.lock() {
                *at = Some(chrono::Utc::now().to_rfc3339());
            }
            if let Ok(mut reason) = self.reason.lock() {
                *reason = err.chars().take(300).collect();
            }
        }
        first
    }

    /// 记录一次 LLM 成功（评审/planner 出结论、探针最小 completion 通）。
    /// 连击清零；封存中成功 = 端点已恢复的直接证据（生产里封存期唯一还能
    /// 发 LLM 调用的就是探针与手动操作），返回 `true` = 恰好解封（边沿）。
    pub fn record_success(&self, cfg: &nemesis_config::BoardUnavailableConfig) -> bool {
        self.streak.store(0, Ordering::SeqCst);
        if !cfg.freeze {
            return false;
        }
        self.frozen.swap(false, Ordering::SeqCst)
    }

    /// 手动放行（WSAPI `board.unavailable.release`）。无视配置——人工意志
    /// 优先。返回 = 之前是否封存中。
    pub fn manual_release(&self) -> bool {
        self.streak.store(0, Ordering::SeqCst);
        self.frozen.swap(false, Ordering::SeqCst)
    }

    /// 观测快照（WSAPI `board.unavailable.status` 数据源）。
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "frozen": self.is_frozen(),
            "connect_streak": self.streak(),
            "tripped_at": self.tripped_at.lock().ok().and_then(|g| g.clone()),
            "reason": self.reason.lock().ok().map(|g| g.clone()).unwrap_or_default(),
        })
    }
}

// ---------------------------------------------------------------------------
// 全局壳（生产路径一行委托；config 读取失败 = 缺省值 freeze=true）
// ---------------------------------------------------------------------------

/// 从 home 读 `board.unavailable` 段（home/读取失败 = 缺省：freeze=true /
/// threshold=3 / probe=300——读取失败时封存是无人值守的安全方向）。
pub fn unavailable_cfg(home: Option<&Path>) -> nemesis_config::BoardUnavailableConfig {
    home.and_then(|h| nemesis_config::load_config(&h.join("config.json")).ok())
        .and_then(|c| c.board)
        .map(|b| b.unavailable)
        .unwrap_or_default()
}

/// 是否封存中（全局壳）。
pub fn frozen_now() -> bool {
    state().is_frozen()
}

/// 全局失败记录（评审/planner 失败路径调用）。返回 = 恰好触发封存。
pub fn record_llm_failure(cfg: &nemesis_config::BoardUnavailableConfig, err: &str) -> bool {
    let tripped = state().record_failure(err, cfg);
    if tripped {
        tracing::warn!(
            "[Board][U-B] LLM 端点连接连续 {} 次失败，封存自动动作（拆解/评审/重派挂起；中断档案写盘，恢复探针每 {}s 探测）：{err}",
            cfg.connect_fail_threshold,
            cfg.probe_interval_secs,
        );
    }
    tripped
}

/// 全局成功记录（评审/planner 出结论路径调用）。返回 = 恰好解封。
pub fn record_llm_success(cfg: &nemesis_config::BoardUnavailableConfig) -> bool {
    let released = state().record_success(cfg);
    if released {
        tracing::info!("[Board][U-B] LLM 调用成功，端点不可用封存解除");
    }
    released
}

/// 手动放行（全局壳）。返回 = 之前是否封存中。
pub fn manual_release() -> bool {
    let was = state().manual_release();
    if was {
        tracing::info!("[Board][U-B] LLM 端点不可用封存已手动放行");
    }
    was
}

/// 观测快照（WSAPI 数据源）。
pub fn snapshot_json() -> serde_json::Value {
    state().snapshot()
}

// ---------------------------------------------------------------------------
// 中断档案（在途工作的可恢复快照；探针 resume 的数据源）
// ---------------------------------------------------------------------------

/// 中断档案路径（`<workspace>/board/interruptions/<issue_number>.json`；
/// 每单一文件最新覆盖——重复挂起不刷屏；resume 后移 `archive/` 带时戳保
/// 历史）。
pub fn interruption_path(workspace: &Path, issue_number: &str) -> PathBuf {
    workspace
        .join("board")
        .join("interruptions")
        .join(format!("{issue_number}.json"))
}

/// 写中断档案（显式 workspace 版——单测不装配全局槽即可测）。
/// `pending` = 恢复动作（`review` = 复评该单 / `plan` = 续做拆解 /
/// `project` = 项目收口复评）；`issue_key` = 档案名片段（单子用 number，
/// 项目用 `project-<id>`——项目无 comment 载体，档案是其唯一现场）。
/// 写失败 WARN 返回 None（档案是恢复链的增强，不阻塞主流程）。
pub fn write_interruption_doc(
    workspace: &Path,
    issue_key: &str,
    issue_id: i64,
    pending: &str,
    hint: String,
    environment: serde_json::Value,
) -> Option<PathBuf> {
    let doc = serde_json::json!({
        "schema": 1,
        "issue": issue_key,
        "issue_id": issue_id,
        "written_at": chrono::Utc::now().to_rfc3339(),
        "stop_kind": "llm_unavailable",
        "pending_action": pending,
        "suggested_action": "retry",
        "hint": hint,
        "environment": environment,
        "resume_entry": match pending {
            "review" => "自动续评（端点恢复探针）或 review.retrigger（number=NB-x）",
            "plan" => "自动续拆（端点恢复探针）或 issue.plan（number=NB-x）",
            _ => "自动恢复（端点恢复探针）",
        },
    });
    let path = interruption_path(workspace, issue_key);
    let parent = path.parent()?;
    let body = serde_json::to_string_pretty(&doc)
        .map_err(|e| tracing::warn!("[Board][U-B] {issue_key} 中断档案序列化失败：{e}"))
        .ok()?;
    if let Err(e) = std::fs::create_dir_all(parent).and_then(|_| std::fs::write(&path, body)) {
        tracing::warn!("[Board][U-B] {issue_key} 中断档案写失败：{e}");
        return None;
    }
    Some(path)
}

/// 单子中断档案便捷形态（从 Issue 取 number/id + verdict_chain——与 U-A
/// 停车档案同源聚合）。
pub fn write_interruption(
    workspace: &Path,
    store: &BoardStore,
    issue: &Issue,
    pending: &str,
    hint: String,
    environment: serde_json::Value,
) -> Option<PathBuf> {
    let chain = nemesis_board::handoff::verdict_chain_from(store, issue.id);
    let doc = serde_json::json!({
        "schema": 1,
        "issue": issue.number,
        "issue_id": issue.id,
        "written_at": chrono::Utc::now().to_rfc3339(),
        "stop_kind": "llm_unavailable",
        "pending_action": pending,
        "suggested_action": "retry",
        "hint": hint,
        "environment": environment,
        "verdict_chain": chain,
        "resume_entry": match pending {
            "review" => "自动续评（端点恢复探针）或 review.retrigger（number=NB-x）",
            "plan" => "自动续拆（端点恢复探针）或 issue.plan（number=NB-x）",
            _ => "自动恢复（端点恢复探针）",
        },
    });
    let path = interruption_path(workspace, &issue.number);
    let parent = path.parent()?;
    let body = serde_json::to_string_pretty(&doc)
        .map_err(|e| tracing::warn!("[Board][U-B] {} 中断档案序列化失败：{e}", issue.number))
        .ok()?;
    if let Err(e) = std::fs::create_dir_all(parent).and_then(|_| std::fs::write(&path, body)) {
        tracing::warn!("[Board][U-B] {} 中断档案写失败：{e}", issue.number);
        return None;
    }
    Some(path)
}

/// trip 边沿的中断档案批量落盘：focus（触发封存的那条在途工作，可空）+
/// 全部在途拆解链（RUNNING_PLANS 快照，跳过 focus 同单防双写）。
/// workspace 未装配（单测/极简）= 静默跳过。
#[cfg(feature = "cluster")]
pub fn write_trip_interruptions(
    workspace: Option<&Path>,
    store: &BoardStore,
    focus: Option<(&Issue, &str, String, serde_json::Value)>,
    err: &str,
) {
    let Some(ws) = workspace else {
        return;
    };
    let focus_id = focus.as_ref().map(|(i, ..)| i.id);
    if let Some((issue, pending, hint, env)) = focus {
        write_interruption(ws, store, issue, pending, hint, env);
    }
    for (plan_id, issue_id, attempt) in super::board::running_plans_snapshot() {
        if Some(issue_id) == focus_id {
            continue; // focus 已写（同单双 pending 无意义）
        }
        if let Ok(issue) = store.get_issue(issue_id) {
            write_interruption(
                ws,
                store,
                &issue,
                "plan",
                format!("拆解在途（attempt {attempt}）时 LLM 端点不可用封存：恢复后自动续拆"),
                serde_json::json!({ "plan_id": plan_id, "attempt": attempt, "trip_error": err.chars().take(200).collect::<String>() }),
            );
        }
    }
}

/// 读取待续做中断档案（探针 resume 数据源；`read_dir` 显式排序——顺序是
/// 平台实现细节）。坏行跳过（投影文件允许手改）。
pub fn read_pending_interruptions(workspace: &Path) -> Vec<serde_json::Value> {
    let dir = workspace.join("board").join("interruptions");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut entries: Vec<std::fs::DirEntry> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    let mut out = Vec::new();
    for e in entries {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        if let Ok(body) = std::fs::read_to_string(&p)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&body)
        {
            out.push(v);
        }
    }
    out
}

/// 中断档案归档（resume 处理完移 `archive/<written_at||now>_<原名>`——
/// 归档不删，可追溯）。返回是否移动成功（false = 调用方保留原文件下轮
/// 重试）。
pub fn archive_interruption(workspace: &Path, source: &Path) -> bool {
    let archive_dir = workspace
        .join("board")
        .join("interruptions")
        .join("archive");
    let Some(name) = source.file_name() else {
        return false;
    };
    let stamped = format!(
        "{}_{}",
        chrono::Local::now().format("%Y%m%d_%H%M%S"),
        name.to_string_lossy()
    );
    if let Err(e) = std::fs::create_dir_all(&archive_dir)
        .and_then(|_| std::fs::rename(source, archive_dir.join(&stamped)))
    {
        tracing::warn!("[Board][U-B] 中断档案归档失败（{}）：{e}", source.display());
        return false;
    }
    true
}

#[cfg(test)]
mod tests;
