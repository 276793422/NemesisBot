//! 子代理任务注册表（子代理可见性 P1，2026-11-03 拍板「D1~D7 全做」）。
//!
//! 三种子代理形态的统一台账：内联 detached（`spawn` 工具）、后台子代理
//! （`__BG_SPAWN__` 续行）、集群 delegate（P3 并入）。登记点只在 SpawnFn
//! 闭包（hooks v2 先例：loop 内部委派——board_review / conflict_resolver /
//! web 看板评审——不进列表）。
//!
//! 落盘：`<workspace>/logs/subagent_tasks/tasks.json`（每次变更原子重写，
//! tmp + rename）。重启回读时 Running 条目诚实降级为 [`SubagentStatus::Interrupted`]
//! （进程重启后无从确认死活，不装作还在跑）。容量上限 [`MAX_TASKS`]（500），
//! 超限逐出最老的终态条目（全 Running 时逐最老，保内存有界）。
//!
//! 全局单例 [`global`]：gateway 启动早期 [`init`] 注入落盘路径（幂等）；
//! 未 init = 纯内存态（测试/裸构建）。测试用 [`SubagentRegistry::new`]
//! 本地实例（OnceLock 不可重置）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

/// 列表容量上限：超限逐出最老终态条目（见模块文档）。
pub const MAX_TASKS: usize = 500;

/// task / summary / error 摘录长度（字符数，char 边界安全截断）。
const EXCERPT_TASK: usize = 500;
const EXCERPT_RESULT: usize = 400;

/// 子代理任务状态（序列化 snake_case；单一真相源，前端直接消费）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Running,
    Done,
    Failed,
    Cancelled,
    /// 进程重启时还在 Running 的条目——重启后无从确认死活，诚实降级。
    Interrupted,
}

impl SubagentStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, SubagentStatus::Running)
    }
}

/// 子代理任务条目（注册表 + 落盘共用形态）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubagentTask {
    /// 条目 id = 子代理会话键（`subagent:{uuid}` / `subagent:{label}:{uuid}`）。
    pub id: String,
    /// 发起会话键（父会话）。空 = 客户端手动 spawn（无父会话）。
    pub parent_session_key: String,
    /// 连接级 chat_id（`web:{连接id}`），实时事件 WS push 路由用。
    pub parent_chat_id: String,
    /// 来源：`model_spawn`（模型调 spawn 工具）/ `client_spawn`（客户端
    /// 手动）/ `delegate`（集群委派，P3）。
    pub origin: String,
    pub agent_id: String,
    /// 任务文本摘录。
    pub task: String,
    pub role: String,
    pub tools_profile: String,
    pub depth: usize,
    pub background: bool,
    /// 所属 loop：`main` / `project:{pid}`（L6++ 项目分组）。
    pub loop_origin: String,
    pub status: SubagentStatus,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub rounds: Option<usize>,
    /// 最终回复摘录（Done 文本）。
    pub summary: Option<String>,
    /// 失败/取消原因摘录。
    pub error: Option<String>,
    /// 后台任务 id（`__BG_SPAWN__` marker 里的 task_id；前台 None）。
    pub bg_task_id: Option<String>,
}

/// 登记请求（`begin` 入参；id 由调用方按会话键派生规则预生成）。
#[derive(Debug, Clone)]
pub struct SubagentBegin {
    pub id: String,
    pub parent_session_key: String,
    pub parent_chat_id: String,
    pub origin: String,
    pub agent_id: String,
    pub task: String,
    pub role: String,
    pub tools_profile: String,
    pub depth: usize,
    pub background: bool,
    pub loop_origin: String,
    pub bg_task_id: Option<String>,
}

#[derive(Default)]
struct Inner {
    tasks: HashMap<String, SubagentTask>,
    /// 插入序（列表按此倒序 = 新的在前；逐出按正序找最老终态）。
    order: Vec<String>,
    /// 取消令牌（id → token）；finish/cancel 后摘除。
    tokens: HashMap<String, Arc<CancellationToken>>,
}

/// 子代理任务注册表（线程安全；变更即原子落盘——落盘失败只 WARN 不影响
/// spawn 主链路）。
pub struct SubagentRegistry {
    inner: Mutex<Inner>,
    persist_path: Option<PathBuf>,
}

static GLOBAL: std::sync::OnceLock<SubagentRegistry> = std::sync::OnceLock::new();
static PERSIST_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// gateway 启动早期注入落盘路径（幂等；必须在 [`global`] 首次物化前
/// 调用——OnceLock 物化后路径不再生效）。
pub fn init(persist_path: PathBuf) {
    let _ = PERSIST_PATH.set(persist_path);
}

/// 全局单例（惰性物化，读取 [`init`] 注入的路径；未 init = 纯内存态）。
pub fn global() -> &'static SubagentRegistry {
    GLOBAL.get_or_init(|| {
        let reg = SubagentRegistry::new(PERSIST_PATH.get().cloned());
        reg.load_from_disk();
        reg
    })
}

impl SubagentRegistry {
    /// 本地实例（测试用）。`persist_path = None` = 纯内存。
    pub fn new(persist_path: Option<PathBuf>) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            persist_path,
        }
    }

    /// 登记新任务（Running 态）。id 冲突时覆盖（会话键 uuid 派生，冲突
    /// 概率可忽略；覆盖保证列表不重复）。
    pub fn begin(&self, b: SubagentBegin) {
        let task = SubagentTask {
            id: b.id,
            parent_session_key: b.parent_session_key,
            parent_chat_id: b.parent_chat_id,
            origin: b.origin,
            agent_id: b.agent_id,
            task: excerpt(&b.task, EXCERPT_TASK),
            role: b.role,
            tools_profile: b.tools_profile,
            depth: b.depth,
            background: b.background,
            loop_origin: b.loop_origin,
            status: SubagentStatus::Running,
            started_at_ms: now_ms(),
            finished_at_ms: None,
            rounds: None,
            summary: None,
            error: None,
            bg_task_id: b.bg_task_id,
        };
        let mut inner = self.inner.lock();
        if !inner.tasks.contains_key(&task.id) {
            inner.order.push(task.id.clone());
        }
        inner.tokens.remove(&task.id);
        inner.tasks.insert(task.id.clone(), task);
        evict_if_needed(&mut inner);
        drop(inner);
        self.persist();
    }

    /// 挂取消令牌（cancel 直达执行链）。重复挂载覆盖（同一 id 只跑一次）。
    pub fn attach_token(&self, id: &str, token: Arc<CancellationToken>) {
        self.inner.lock().tokens.insert(id.to_string(), token);
    }

    /// 终态回写（由 `run_detached_events` 在回合结束裁决后调用）。
    /// 条目不存在（重启后回灌的旧任务）= 静默忽略。
    pub fn finish(
        &self,
        id: &str,
        status: SubagentStatus,
        rounds: Option<usize>,
        summary: Option<String>,
        error: Option<String>,
    ) {
        let mut inner = self.inner.lock();
        if let Some(t) = inner.tasks.get_mut(id) {
            t.status = status;
            t.finished_at_ms = Some(now_ms());
            t.rounds = rounds;
            t.summary = summary.map(|s| excerpt(&s, EXCERPT_RESULT));
            t.error = error.map(|e| excerpt(&e, EXCERPT_RESULT));
        }
        inner.tokens.remove(id);
        drop(inner);
        self.persist();
    }

    /// 用户面「终止」：取消令牌（执行链同步中断）+ 状态置 Cancelled。
    /// 返回是否命中任务（WSAPI 诚实报错用）。
    pub fn cancel(&self, id: &str) -> bool {
        let mut inner = self.inner.lock();
        let hit = inner.tasks.contains_key(id);
        if hit {
            if let Some(t) = inner
                .tasks
                .get_mut(id)
                .filter(|t| t.status == SubagentStatus::Running)
            {
                t.status = SubagentStatus::Cancelled;
                t.finished_at_ms = Some(now_ms());
            }
            if let Some(tok) = inner.tokens.remove(id) {
                tok.cancel();
            }
        }
        drop(inner);
        if hit {
            self.persist();
        }
        hit
    }

    /// 全量列表（新的在前）。
    pub fn list(&self) -> Vec<SubagentTask> {
        let inner = self.inner.lock();
        inner
            .order
            .iter()
            .rev()
            .filter_map(|id| inner.tasks.get(id))
            .cloned()
            .collect()
    }

    /// 单条查询。
    pub fn get(&self, id: &str) -> Option<SubagentTask> {
        self.inner.lock().tasks.get(id).cloned()
    }

    /// 重启回读：Running 条目降级 Interrupted（死亡不可确认）。
    fn load_from_disk(&self) {
        let Some(path) = &self.persist_path else {
            return;
        };
        let Ok(bytes) = std::fs::read(path) else {
            return; // 首启无历史 = 正常态，不 WARN
        };
        let Ok(tasks) = serde_json::from_slice::<Vec<SubagentTask>>(&bytes) else {
            tracing::warn!(
                path = %path.display(),
                "[subagent_registry] persisted tasks unreadable; starting empty"
            );
            return;
        };
        let mut inner = self.inner.lock();
        for mut t in tasks {
            if t.status == SubagentStatus::Running {
                t.status = SubagentStatus::Interrupted;
            }
            if !inner.tasks.contains_key(&t.id) {
                inner.order.push(t.id.clone());
            }
            inner.tasks.insert(t.id.clone(), t);
        }
        // 回读也可能超限（上限收紧的场景）——逐出最老。
        while inner.order.len() > MAX_TASKS {
            let Some(oldest) = inner.order.first().cloned() else {
                break;
            };
            inner.order.remove(0);
            inner.tasks.remove(&oldest);
        }
        drop(inner);
    }

    /// 原子落盘（tmp + rename 覆盖）。失败只 WARN（注册表故障不阻断 spawn）。
    fn persist(&self) {
        let Some(path) = &self.persist_path else {
            return;
        };
        let snapshot: Vec<SubagentTask> = {
            let inner = self.inner.lock();
            inner
                .order
                .iter()
                .filter_map(|id| inner.tasks.get(id))
                .cloned()
                .collect()
        };
        let Ok(json) = serde_json::to_string(&snapshot) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        let write = || -> std::io::Result<()> {
            std::fs::write(&tmp, json)?;
            std::fs::rename(&tmp, path)?;
            Ok(())
        };
        if let Err(e) = write() {
            tracing::warn!(error = %e, path = %path.display(), "[subagent_registry] persist failed");
        }
    }
}

/// 容量治理：超 [`MAX_TASKS`] 时从最老开始逐终态条目；全 Running 时逐最老
/// （内存有界优先于保 Running 台账——Running 台账以令牌面为准）。
fn evict_if_needed(inner: &mut Inner) {
    while inner.tasks.len() > MAX_TASKS {
        let victim = inner
            .order
            .iter()
            .find(|id| {
                inner
                    .tasks
                    .get(*id)
                    .map(|t| t.status.is_terminal())
                    .unwrap_or(false)
            })
            .or_else(|| inner.order.first())
            .cloned();
        let Some(victim) = victim else { break };
        inner.order.retain(|id| id != &victim);
        inner.tasks.remove(&victim);
        inner.tokens.remove(&victim);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// char 边界安全截断（runes 计数）。
fn excerpt(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

#[cfg(test)]
mod tests;
