//! 会话资源盘点——nemesis-agent 侧 provider 集（2026-11-17 会话资源盘点
//! 看板计划 B1）。契约（trait / CategoryView / SessionLoopApi）住
//! [`nemesis_types::resources`]；本模块交付：
//!
//! - **7 个全局 provider**：子代理 / 待办 / 文件变更 / 会话信息 / 后台进程 /
//!   会话钩子 / 全局钩子——聚合面直采，不经 per-loop 路由；
//! - **3 个 per-loop provider**：检查点 / 上下文 / 执行态——经
//!   [`SessionLoopApi`] 抽象消费 loop 运行态；
//! - **`impl SessionLoopApi for AgentLoop`**：4 方法委托既有查询面
//!   （`session_context_status` / `inbox_status` / work_timeline 快照 /
//!   checkpoint store），loop.rs 本体零改动。
//!
//! 装配纪律（计划 §3.3）：provider 构造收 `Option<件>` **无条件注册**，
//! 运行态缺席在 `collect` 内诚实判（Unavailable / Empty），不做装配期
//! 分支——gateway 装配点保持「一行一个」的平铺形态。
//!
//! 口径地雷（计划 §4，逐条对应）：
//! - hooks 双口径：会话级直读 `hooks.session.json`（键 = **裸 sid**，
//!   [`ResourceCtx::bare_sid`]）；全局侧经 hooks 桥 `matches_session`
//!   双串匹配（完整 session_key）。
//! - checkpoint：`CheckpointMeta` 的 session 投影已随 B1 落地
//!   （`CheckpointMeta::session`），此处只取**本会话**条目。
//! - todo / hooks 文件路径统一走 `nemesis-path` 单一真相源（内部自
//!   sanitize），provider 不自拼路径。

use nemesis_types::resources::{
    CategoryView, CheckpointSummary, GlobalResourceProvider, PerLoopResourceProvider, ProviderMeta,
    ResourceCtx, SessionLoopApi,
};
use serde_json::{Value, json};
use std::sync::Arc;

/// 主 loop 的重建存活槽（nemesis-web `AppState.agent_loop` 同构类型）——
/// hooks_global 类目经它在 collect 时解析**当前** loop 的 hooks 桥。
/// 不直接捕获 `Arc<CcHookBridge>`：桥随 loop 重建换实例（agent_factory
/// 每次 build 重载），捕获旧 Arc = hooks.json 运行期新建后盘点面永远
/// Empty 的陈旧读。
pub type LoopSlot = Arc<parking_lot::RwLock<Option<Arc<crate::r#loop::AgentLoop>>>>;

/// 条目行截断（char 安全）。长文本字段（prompt / task / summary）进
/// summary 前统一收口，防止看板卡被单条长任务撑爆。
fn trunc(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}…")
}

/// 摘要行 cap：`(保留行, 省略数)`。行超 [`SUMMARY_ROW_CAP`] 时保**前**
/// cap 行（调用方保证传入顺序 = 展示优先级：时间序 / 注册序）。
const SUMMARY_ROW_CAP: usize = 5;

fn cap_rows(rows: Vec<Value>) -> (Vec<Value>, usize) {
    let omitted = rows.len().saturating_sub(SUMMARY_ROW_CAP);
    (rows.into_iter().take(SUMMARY_ROW_CAP).collect(), omitted)
}

// ---------------------------------------------------------------------------
// Global providers（7 个）
// ---------------------------------------------------------------------------

/// 子代理工作面（`subagents` / 子代理）。数据源 =
/// [`crate::subagent_registry`] 全局单例，按 `parent_session_key` 归会话；
/// 已归档条目（`archived_at` 在场）不在工作面，不计入。
pub struct SubagentsProvider;

impl GlobalResourceProvider for SubagentsProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("subagents", "子代理", 70, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let mine: Vec<_> = crate::subagent_registry::global()
            .list()
            .into_iter()
            .filter(|t| t.parent_session_key == ctx.session_key && t.archived_at.is_none())
            .collect();
        if mine.is_empty() {
            return CategoryView::empty();
        }
        let active = mine
            .iter()
            .filter(|t| t.status == crate::subagent_registry::SubagentStatus::Running)
            .count();
        let rows: Vec<Value> = mine
            .iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "task": trunc(&t.task, 80),
                    "role": t.role,
                    "status": t.status,
                    "background": t.background,
                    "bg_task_id": t.bg_task_id,
                    "started_at_ms": t.started_at_ms,
                    "finished_at_ms": t.finished_at_ms,
                    "summary": t.summary.as_deref().map(|s| trunc(s, 120)),
                    "error": t.error.as_deref().map(|s| trunc(s, 120)),
                })
            })
            .collect();
        let (rows, omitted) = cap_rows(rows);
        CategoryView::ok(
            mine.len(),
            json!({"active": active, "rows": rows, "omitted": omitted}),
        )
    }
}

/// 会话待办（`todos` / 待办）。路径走 [`nemesis_path::
/// session_todo_file_in_workspace`] 单一真相源（与 TodoWriteTool 写路径、
/// todo 收尾 hook、回收站寻址同源）；文件缺失 / 损坏 = 空清单（与 web
/// `read_session_todos` 同一口径，损坏 warn 留痕）。
pub struct TodosProvider;

impl GlobalResourceProvider for TodosProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("todos", "待办", 90, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let path =
            nemesis_path::session_todo_file_in_workspace(ctx.workspace_root, ctx.session_key);
        let todos: Vec<nemesis_types::agent::TodoItem> = match std::fs::read_to_string(&path) {
            Ok(body) => match serde_json::from_str(&body) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "[resources] corrupt todo file; treating as empty"
                    );
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        if todos.is_empty() {
            return CategoryView::empty();
        }
        let pending = todos
            .iter()
            .filter(|t| t.status == nemesis_types::agent::TodoStatus::Pending)
            .count();
        let in_progress = todos
            .iter()
            .filter(|t| t.status == nemesis_types::agent::TodoStatus::InProgress)
            .count();
        let rows: Vec<Value> = todos
            .iter()
            .map(|t| json!({"content": trunc(&t.content, 120), "status": t.status}))
            .collect();
        let (rows, omitted) = cap_rows(rows);
        CategoryView::ok(
            todos.len(),
            json!({
                "pending": pending,
                "in_progress": in_progress,
                "completed": todos.len() - pending - in_progress,
                "rows": rows,
                "omitted": omitted,
            }),
        )
    }
}

/// 会话文件变更（`file_changes` / 文件变更）。数据源 = chat_log assistant
/// 行的声明式 `file_changes`（D3）——全量读者形态滑窗（与 fork/export
/// 同一读契约），按日志时间序收集；summary 保最近 5 条 + 全量计数。
pub struct FileChangesProvider;

impl GlobalResourceProvider for FileChangesProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("file_changes", "文件变更", 100, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let (lines, _, _, _) = crate::chat_log::read_chat_log(ctx.session_key, usize::MAX, None);
        let mut rows: Vec<Value> = Vec::new();
        for line in &lines {
            if line.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(changes) = line.get("file_changes").and_then(Value::as_array) else {
                continue;
            };
            for c in changes {
                let Some(path) = c.get("path").and_then(Value::as_str) else {
                    continue;
                };
                rows.push(json!({
                    "path": trunc(path, 160),
                    "kind": c.get("kind").cloned().unwrap_or(Value::Null),
                }));
            }
        }
        if rows.is_empty() {
            return CategoryView::empty();
        }
        let total = rows.len();
        // 日志时间序 = 旧在前；看板展示最近的，保**尾** 5 条。
        let omitted = total.saturating_sub(SUMMARY_ROW_CAP);
        let rows: Vec<Value> = rows.split_off(total.saturating_sub(SUMMARY_ROW_CAP));
        CategoryView::ok(total, json!({"rows": rows, "omitted": omitted}))
    }
}

/// 会话信息（`identity` / 会话信息）。数据源 = chat_log sidecar meta
/// （E4 全量读：标题 / 分叉谱系 / 项目归属 / 未送达计数）。sidecar 缺席
/// （会话从未命名 / 无分叉 / 无项目的全新会话）= Empty。
pub struct IdentityProvider;

impl GlobalResourceProvider for IdentityProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("identity", "会话信息", 200, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let Some(m) = crate::chat_log::read_session_meta_full(ctx.session_key) else {
            return CategoryView::empty();
        };
        CategoryView::ok(
            1,
            json!({
                "title": m.title,
                "title_manual": m.title_manual,
                "project_id": m.project_id,
                "project_path": m.project_path,
                "parent": m.parent,
                "forked_at_turn": m.forked_at_turn,
                "fork_reason": m.fork_reason,
                "undelivered": m.undelivered,
                "has_branch_summary": m.branch_summary.is_some(),
            }),
        )
    }
}

/// 后台进程（`background` / 后台进程）。构造收注册表句柄（gateway
/// `SharedResources.background_registry` 单例，跨 loop 重建存活）；None
/// （headless / 测试缺省）= Unavailable 诚实降级。
pub struct BackgroundProvider {
    registry: Option<Arc<crate::background_registry::BackgroundProcessRegistry>>,
}

impl BackgroundProvider {
    pub fn new(
        registry: Option<Arc<crate::background_registry::BackgroundProcessRegistry>>,
    ) -> Self {
        Self { registry }
    }
}

impl GlobalResourceProvider for BackgroundProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("background", "后台进程", 60, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let Some(reg) = &self.registry else {
            return CategoryView::unavailable("后台进程注册表未装配");
        };
        // 同步 try_lock 快照（collect 是 sync 契约；注册表主锁 tokio Mutex
        // 不能 blocking_lock）。忙 = 该拍 Unavailable、下拍自愈。
        let rows = match reg.list_by_session_snapshot(ctx.session_key) {
            Ok(rows) => rows,
            Err(reason) => return CategoryView::unavailable(reason),
        };
        if rows.is_empty() {
            return CategoryView::empty();
        }
        let running = rows
            .iter()
            .filter(|r| r.get("running").and_then(Value::as_bool) == Some(true))
            .count();
        let total = rows.len();
        let omitted = total.saturating_sub(SUMMARY_ROW_CAP);
        let rows: Vec<Value> = rows.into_iter().take(SUMMARY_ROW_CAP).collect();
        CategoryView::ok(
            total,
            json!({"running": running, "rows": rows, "omitted": omitted}),
        )
    }
}

/// 会话级钩子（`hooks_session` / 会话钩子）。口径（计划 §4 地雷 3）：
/// 直读 `<workspace>/config/hooks.session.json`（agent `hooks` 工具存储，
/// 键 = **裸 sid**），与盘上真相一致；桥内存态是同一文件的懒重载镜像，
/// 两读等价、盘读更直接。0 条 = Empty（会话没装钩子是常态）。
pub struct HooksSessionProvider;

impl GlobalResourceProvider for HooksSessionProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("hooks_session", "会话钩子", 30, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let dir = nemesis_path::workspace_config_dir(ctx.workspace_root);
        let store = crate::cc_hooks::read_session_hook_store(&dir);
        let count = store.session_len(ctx.bare_sid);
        if count == 0 {
            return CategoryView::empty();
        }
        CategoryView::ok(count, json!({"count": count}))
    }
}

/// 全局钩子（`hooks_global` / 全局钩子）。构造收主 loop 槽（[`LoopSlot`]），
/// collect 时解析**当前** loop 的 hooks 桥（跨重建存活）；槽空 / 桥缺席
/// （workspace 无任何钩子配置）= Empty（合法无）；有桥无匹配 = Empty；
/// 有匹配 = 按事件名计数。匹配口径复用桥侧 `matches_session` 双串匹配
/// （完整 session_key）。
pub struct HooksGlobalProvider {
    loop_slot: Option<LoopSlot>,
}

impl HooksGlobalProvider {
    pub fn new(loop_slot: Option<LoopSlot>) -> Self {
        Self { loop_slot }
    }
}

impl GlobalResourceProvider for HooksGlobalProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("hooks_global", "全局钩子", 40, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let Some(slot) = &self.loop_slot else {
            return CategoryView::empty();
        };
        let bridge = slot.read().as_ref().and_then(|l| l.cc_hook_bridge());
        let Some(bridge) = bridge else {
            // 桥缺席 = load_from_dir 判定两侧（hooks.json / hooks.session.json）
            // 均无内容——全局钩子合法无。
            return CategoryView::empty();
        };
        let counts = bridge.global_session_match_counts(ctx.session_key);
        if counts.is_empty() {
            return CategoryView::empty();
        }
        let total: usize = counts.iter().map(|(_, n)| n).sum();
        let by_event: serde_json::Map<String, Value> = counts
            .into_iter()
            .map(|(event, n)| (event.to_string(), json!(n)))
            .collect();
        CategoryView::ok(total, json!({"by_event": by_event}))
    }
}

/// 待审批（`approvals_pending` / 待审批，瞬态类目）。数据源 = 当前 loop 的
/// `ApprovalResponder::pending()`（WebApprovalManager / Channel / ACP 三
/// manager 共同 trait 面），按行 `session_key` 过滤——批准面是全局单例，
/// 盘点是按会话切面。行字段 = entry_json 形态（含 stale 卡片，前端可裁决）。
/// 槽 / loop / responder 缺席（headless）= Unavailable（查不了 ≠ 没有）。
pub struct ApprovalsPendingProvider {
    loop_slot: Option<LoopSlot>,
}

impl ApprovalsPendingProvider {
    pub fn new(loop_slot: Option<LoopSlot>) -> Self {
        Self { loop_slot }
    }
}

impl GlobalResourceProvider for ApprovalsPendingProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("approvals_pending", "待审批", 10, true)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let Some(slot) = &self.loop_slot else {
            return CategoryView::unavailable("审批面未装配");
        };
        let responder = slot.read().as_ref().and_then(|l| l.approval_responder());
        let Some(responder) = responder else {
            return CategoryView::unavailable("审批面未装配");
        };
        let mine: Vec<Value> = responder
            .pending()
            .into_iter()
            .filter(|r| r.get("session_key").and_then(Value::as_str) == Some(ctx.session_key))
            .collect();
        if mine.is_empty() {
            return CategoryView::empty();
        }
        let total = mine.len();
        let (rows, omitted) = cap_rows(mine);
        CategoryView::ok(total, json!({"rows": rows, "omitted": omitted}))
    }
}

/// 待提问（`questions_pending` / 待提问，瞬态类目）。数据源 = 当前 loop 的
/// `QuestionResponder::pending()`（WebQuestionBroker trait 面），会话过滤
/// 与缺席语义同 [`ApprovalsPendingProvider`]。
pub struct QuestionsPendingProvider {
    loop_slot: Option<LoopSlot>,
}

impl QuestionsPendingProvider {
    pub fn new(loop_slot: Option<LoopSlot>) -> Self {
        Self { loop_slot }
    }
}

impl GlobalResourceProvider for QuestionsPendingProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("questions_pending", "待提问", 20, true)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let Some(slot) = &self.loop_slot else {
            return CategoryView::unavailable("提问面未装配");
        };
        let responder = slot.read().as_ref().and_then(|l| l.question_responder());
        let Some(responder) = responder else {
            return CategoryView::unavailable("提问面未装配");
        };
        let mine: Vec<Value> = responder
            .pending()
            .into_iter()
            .filter(|r| r.get("session_key").and_then(Value::as_str) == Some(ctx.session_key))
            .collect();
        if mine.is_empty() {
            return CategoryView::empty();
        }
        let total = mine.len();
        let (rows, omitted) = cap_rows(mine);
        CategoryView::ok(total, json!({"rows": rows, "omitted": omitted}))
    }
}

// ---------------------------------------------------------------------------
// PerLoop providers（3 个）
// ---------------------------------------------------------------------------

/// 检查点（`checkpoint` / 检查点）。数据源 = [`SessionLoopApi::
/// checkpoint_summary`]（本会话过滤后的 [`CheckpointSummary`] 向量）。
pub struct CheckpointProvider;

impl PerLoopResourceProvider for CheckpointProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("checkpoint", "检查点", 110, false)
    }

    fn collect(&self, ctx: &ResourceCtx, api: &dyn SessionLoopApi) -> CategoryView {
        let metas = api.checkpoint_summary(ctx.session_key);
        if metas.is_empty() {
            return CategoryView::empty();
        }
        let total = metas.len();
        // metas 是 turn 升序（checkpoint store list_meta 契约）——看板展示
        // **最近**的检查点，保尾 5 条（file_changes 同款），omitted 为更旧的。
        let mut rows: Vec<Value> = metas
            .iter()
            .map(|m| {
                json!({
                    "turn": m.turn,
                    "time": m.time,
                    "prompt": trunc(&m.prompt, 120),
                })
            })
            .collect();
        let omitted = total.saturating_sub(SUMMARY_ROW_CAP);
        let rows: Vec<Value> = rows.split_off(total.saturating_sub(SUMMARY_ROW_CAP));
        let latest = metas.last().expect("non-empty checked above");
        CategoryView::ok(
            total,
            json!({
                "latest_turn": latest.turn,
                "latest_time": latest.time,
                "rows": rows,
                "omitted": omitted,
            }),
        )
    }
}

/// 上下文占用（`context` / 上下文）。数据源 = [`SessionLoopApi::
/// context_status`]（M5 快照：窗口占用 / 压缩触发点 / 来源 / 压缩点
/// 百分比）。loop 内现成查询面直通，provider 不复制口径。
pub struct ContextProvider;

impl PerLoopResourceProvider for ContextProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("context", "上下文", 120, false)
    }

    fn collect(&self, ctx: &ResourceCtx, api: &dyn SessionLoopApi) -> CategoryView {
        match api.context_status(ctx.session_key) {
            Some(v) => CategoryView::ok(1, v),
            None => CategoryView::unavailable("上下文快照不可用"),
        }
    }
}

/// 执行态（`execution` / 执行态，瞬态类目）。数据源 = work_timeline 运行态
/// 快照 + inbox 两队列计数。空闲且无排队 = Empty（看板「空闲整卡隐藏」
/// 判据：status != ok 或 running=false，前端按 summary.running 表达）。
pub struct ExecutionProvider;

impl PerLoopResourceProvider for ExecutionProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("execution", "执行态", 0, true)
    }

    fn collect(&self, ctx: &ResourceCtx, api: &dyn SessionLoopApi) -> CategoryView {
        let timeline = api.work_timeline(ctx.session_key);
        let (steer, follow_up) = api.inbox_pending(ctx.session_key);
        let running = timeline.is_some();
        if !running && steer == 0 && follow_up == 0 {
            return CategoryView::empty();
        }
        CategoryView::ok(
            usize::from(running) + steer + follow_up,
            json!({
                "running": running,
                "turn": timeline,
                "steer_pending": steer,
                "followup_pending": follow_up,
            }),
        )
    }
}

// ---------------------------------------------------------------------------
// SessionLoopApi 实现——4 方法委托既有查询面（loop.rs 本体零改动）
// ---------------------------------------------------------------------------

impl SessionLoopApi for crate::r#loop::AgentLoop {
    fn context_status(&self, session_key: &str) -> Option<Value> {
        Some(self.session_context_status(session_key))
    }

    fn inbox_pending(&self, session_key: &str) -> (usize, usize) {
        let st = self.inbox_status(session_key);
        (st.next_turn, st.next_step)
    }

    fn work_timeline(&self, session_key: &str) -> Option<Value> {
        self.work_timeline_snapshot(session_key).map(|e| {
            json!({
                "turn_started_ms": e.turn_started_ms,
                "current_tool": e.current_tool.map(|(name, started_ms)| {
                    json!({"name": name, "started_ms": started_ms})
                }),
            })
        })
    }

    fn checkpoint_summary(&self, session_key: &str) -> Vec<CheckpointSummary> {
        let Some(store) = self.checkpoint_store() else {
            return Vec::new();
        };
        store
            .list_meta()
            .into_iter()
            .filter(|m| m.session.as_deref() == Some(session_key))
            .map(|m| CheckpointSummary {
                turn: m.turn,
                time: m.time,
                prompt: m.prompt,
                session: m.session,
            })
            .collect()
    }
}
