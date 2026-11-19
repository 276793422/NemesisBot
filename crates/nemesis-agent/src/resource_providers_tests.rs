//! resource_providers 单元测试（2026-11-17 会话资源盘点 B1）。
//!
//! 覆盖三层：
//! - **PerLoop provider 逻辑**（stub [`SessionLoopApi`] 替身——不构造真
//!   loop，纯逻辑分支：过滤/cap/三态）；
//! - **`impl SessionLoopApi for AgentLoop`**（真 loop 基线：无 store /
//!   空队列 / 无在跑轮次时的诚实零值）;
//! - **Global provider 真数据源**（todos/identity/hooks 文件读、后台注册表
//!   快照、子代理注册表过滤、chat_log 文件变更聚合）。
//!
//! chat_log / hooks 文件类测试沿用本 crate 惯例：不重定向 home，用唯一
//! 键隔离（`rp-` 前缀 + 纳秒戳）。

use crate::r#loop::{LlmMessage, LlmProvider, LlmResponse};
use crate::resource_providers::*;
use nemesis_types::resources::{
    CategoryStatus, GlobalResourceProvider, PerLoopResourceProvider, ResourceCtx, SessionLoopApi,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_sid(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("rp-{tag}-{nanos}")
}

fn ctx_for<'a>(workspace: &'a Path, session_key: &'a str, bare: &'a str) -> ResourceCtx<'a> {
    ResourceCtx {
        session_key,
        bare_sid: bare,
        workspace_root: workspace,
    }
}

/// SessionLoopApi 替身——各字段直接驱动 PerLoop provider 的分支。
struct StubLoopApi {
    context: Option<Value>,
    inbox: (usize, usize),
    timeline: Option<Value>,
    checkpoints: Vec<nemesis_types::resources::CheckpointSummary>,
}

impl Default for StubLoopApi {
    fn default() -> Self {
        Self {
            context: Some(json!({"pct": 12})),
            inbox: (0, 0),
            timeline: None,
            checkpoints: Vec::new(),
        }
    }
}

impl SessionLoopApi for StubLoopApi {
    fn context_status(&self, _session_key: &str) -> Option<Value> {
        self.context.clone()
    }
    fn inbox_pending(&self, _session_key: &str) -> (usize, usize) {
        self.inbox
    }
    fn work_timeline(&self, _session_key: &str) -> Option<Value> {
        self.timeline.clone()
    }
    fn checkpoint_summary(
        &self,
        session_key: &str,
    ) -> Vec<nemesis_types::resources::CheckpointSummary> {
        // 与真实现同契约：只返回本会话条目（trait 文档口径）。
        self.checkpoints
            .iter()
            .filter(|m| m.session.as_deref() == Some(session_key))
            .cloned()
            .collect()
    }
}

fn cp_summary(turn: usize, session: &str) -> nemesis_types::resources::CheckpointSummary {
    nemesis_types::resources::CheckpointSummary {
        turn,
        time: format!("2026-11-17T00:00:{turn:02}"),
        prompt: format!("prompt {turn}"),
        session: Some(session.to_string()),
    }
}

// ---------------------------------------------------------------------------
// ExecutionProvider（执行态）
// ---------------------------------------------------------------------------

#[test]
fn execution_idle_is_empty() {
    let api = StubLoopApi::default();
    let ctx = ctx_for(Path::new("."), "agent:main:session:x", "x");
    let view = ExecutionProvider.collect(&ctx, &api);
    assert_eq!(view.status, CategoryStatus::Empty);
    assert_eq!(view.total, 0);
}

#[test]
fn execution_running_ok_with_tool() {
    let api = StubLoopApi {
        timeline: Some(json!({
            "turn_started_ms": 1_000,
            "current_tool": {"name": "exec", "started_ms": 2_000},
        })),
        ..Default::default()
    };
    let ctx = ctx_for(Path::new("."), "agent:main:session:x", "x");
    let view = ExecutionProvider.collect(&ctx, &api);
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 1);
    assert_eq!(view.summary["running"], json!(true));
    assert_eq!(view.summary["turn"]["current_tool"]["name"], json!("exec"));
    assert_eq!(view.summary["steer_pending"], json!(0));
}

#[test]
fn execution_pending_queues_without_turn() {
    let api = StubLoopApi {
        inbox: (2, 1),
        ..Default::default()
    };
    let ctx = ctx_for(Path::new("."), "agent:main:session:x", "x");
    let view = ExecutionProvider.collect(&ctx, &api);
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 3);
    assert_eq!(view.summary["running"], json!(false));
    assert_eq!(view.summary["steer_pending"], json!(2));
    assert_eq!(view.summary["followup_pending"], json!(1));
}

// ---------------------------------------------------------------------------
// CheckpointProvider（检查点）
// ---------------------------------------------------------------------------

#[test]
fn checkpoint_filters_other_sessions_and_caps_rows() {
    let key = "agent:main:session:ck";
    let api = StubLoopApi {
        checkpoints: (0..7)
            .map(|i| cp_summary(i, key))
            .chain(std::iter::once(cp_summary(99, "agent:main:session:other")))
            .collect(),
        ..Default::default()
    };
    let ctx = ctx_for(Path::new("."), key, "ck");
    let view = CheckpointProvider.collect(&ctx, &api);
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 7); // 他会话条目不计入
    assert_eq!(view.summary["latest_turn"], json!(6));
    let rows = view.summary["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 5); // cap
    // 保尾（最近优先）：turn 升序输入取尾 5 → 首行 turn=2（漏掉 0/1）。
    assert_eq!(rows[0]["turn"], json!(2));
    assert_eq!(view.summary["omitted"], json!(2));
}

#[test]
fn checkpoint_empty_when_no_entries() {
    let api = StubLoopApi::default();
    let ctx = ctx_for(Path::new("."), "agent:main:session:ck", "ck");
    let view = CheckpointProvider.collect(&ctx, &api);
    assert_eq!(view.status, CategoryStatus::Empty);
}

// ---------------------------------------------------------------------------
// ContextProvider（上下文）
// ---------------------------------------------------------------------------

#[test]
fn context_passthrough_ok() {
    let api = StubLoopApi {
        context: Some(json!({"pct": 42, "pct_trigger": 80})),
        ..Default::default()
    };
    let ctx = ctx_for(Path::new("."), "agent:main:session:x", "x");
    let view = ContextProvider.collect(&ctx, &api);
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 1);
    assert_eq!(view.summary["pct"], json!(42));
}

#[test]
fn context_unavailable_when_api_none() {
    let api = StubLoopApi {
        context: None,
        ..Default::default()
    };
    let ctx = ctx_for(Path::new("."), "agent:main:session:x", "x");
    let view = ContextProvider.collect(&ctx, &api);
    assert!(matches!(view.status, CategoryStatus::Unavailable { .. }));
    assert!(view.status.unavailable_reason().is_some());
}

// ---------------------------------------------------------------------------
// TodosProvider（待办）——真文件读
// ---------------------------------------------------------------------------

#[test]
fn todos_counts_and_rows_from_file() {
    let dir = std::env::temp_dir().join(format!("nb-rp-todos-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let key = format!("agent:main:session:{}", unique_sid("todo"));
    let path = nemesis_path::session_todo_file_in_workspace(&dir, &key);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir sessions dir");
    std::fs::write(
        &path,
        serde_json::to_vec(&vec![
            nemesis_types::agent::TodoItem {
                content: "写实现".into(),
                status: nemesis_types::agent::TodoStatus::Completed,
            },
            nemesis_types::agent::TodoItem {
                content: "补测试".into(),
                status: nemesis_types::agent::TodoStatus::InProgress,
            },
            nemesis_types::agent::TodoItem {
                content: "跑门禁".into(),
                status: nemesis_types::agent::TodoStatus::Pending,
            },
        ])
        .expect("serialize"),
    )
    .expect("write");
    let view = TodosProvider.collect(&ctx_for(&dir, &key, "todo"));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 3);
    assert_eq!(view.summary["pending"], json!(1));
    assert_eq!(view.summary["in_progress"], json!(1));
    assert_eq!(view.summary["completed"], json!(1));
    let rows = view.summary["rows"].as_array().expect("rows");
    assert_eq!(rows[0]["status"], json!("completed"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn todos_missing_file_is_empty() {
    let dir = std::env::temp_dir().join(format!("nb-rp-todos-empty-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let key = format!("agent:main:session:{}", unique_sid("todo-none"));
    let view = TodosProvider.collect(&ctx_for(&dir, &key, "todo-none"));
    assert_eq!(view.status, CategoryStatus::Empty);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// IdentityProvider（会话信息）——chat_log sidecar
// ---------------------------------------------------------------------------

#[test]
fn identity_reads_meta_and_absent_is_empty() {
    let key = format!("agent:main:session:{}", unique_sid("ident"));
    crate::chat_log::write_session_meta(&key, "盘点测试会话");
    crate::chat_log::write_session_project(&key, "p-1", "/tmp/proj-a");
    let view = IdentityProvider.collect(&ctx_for(Path::new("."), &key, "ident"));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.summary["title"], json!("盘点测试会话"));
    assert_eq!(view.summary["project_id"], json!("p-1"));
    assert_eq!(view.summary["undelivered"], json!(0));

    let missing = format!("agent:main:session:{}", unique_sid("ident-none"));
    let view = IdentityProvider.collect(&ctx_for(Path::new("."), &missing, "ident-none"));
    assert_eq!(view.status, CategoryStatus::Empty);
}

// ---------------------------------------------------------------------------
// HooksSession / HooksGlobal（钩子双口径）
// ---------------------------------------------------------------------------

#[test]
fn hooks_session_counts_by_bare_sid() {
    let dir = std::env::temp_dir().join(format!("nb-rp-hooks-{}", std::process::id()));
    // RMW 计数测试：先清残留（上次崩溃留下的 hooks.json 会污染分账）。
    let _ = std::fs::remove_dir_all(&dir);
    let config_dir = dir.join("config");
    let _ = std::fs::create_dir_all(&config_dir);
    let sid = unique_sid("hook");
    // 统一存储：经生产 RMW API（add_session_hook）种工具专属圈定组——
    // Stop → h1、PreToolUse → h2/h3。
    let id = crate::cc_hooks::add_session_hook(&config_dir, &sid, "Stop", "", "echo hi", "", None)
        .expect("seed stop");
    assert_eq!(id, "h1");
    let id =
        crate::cc_hooks::add_session_hook(&config_dir, &sid, "PreToolUse", "", "echo a", "", None)
            .expect("seed pre-a");
    assert_eq!(id, "h2");
    let id =
        crate::cc_hooks::add_session_hook(&config_dir, &sid, "PreToolUse", "", "echo b", "", None)
            .expect("seed pre-b");
    assert_eq!(id, "h3");
    let view =
        HooksSessionProvider.collect(&ctx_for(&dir, &format!("agent:main:session:{sid}"), &sid));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 3);
    assert_eq!(view.summary["count"], json!(3));
    // 明细行（2026-11-19「只有个数不知道是啥」补齐）：3 条命令型，事件/
    // 类型/内容逐条对得上（断言按内容寻行不按位次——groups() 迭代序 =
    // 结构体字段序，事件排序不是本函数契约）。
    let rows = view.summary["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r["kind"] == json!("command")));
    let stop = rows
        .iter()
        .find(|r| r["event"] == json!("Stop"))
        .expect("stop row");
    assert_eq!(stop["content"], json!("echo hi"));
    let pre_a = rows
        .iter()
        .find(|r| r["content"] == json!("echo a"))
        .expect("pre a row");
    assert_eq!(pre_a["event"], json!("PreToolUse"));
    assert_eq!(pre_a["matcher"], json!(""));
    // count/list 同谓词：count_session_hooks 与行数一致（双串语义同源）。
    let n = crate::cc_hooks::count_session_hooks(&config_dir, &format!("agent:main:session:{sid}"))
        .expect("count by full key");
    assert_eq!(n, rows.len());
    // 他会话不计入：另一个 sid 无条目 → Empty。
    let other = unique_sid("hook-other");
    let view = HooksSessionProvider.collect(&ctx_for(
        &dir,
        &format!("agent:main:session:{other}"),
        &other,
    ));
    assert_eq!(view.status, CategoryStatus::Empty);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 用户手写圈定组（非工具 RMW API 写入的另一分账来源，prompt 型——线上
/// 实景：Dashboard 手写 Stop prompt 钩子）也进明细行；count 与行数同账。
#[test]
fn hooks_session_rows_expose_user_scoped_prompt_hook() {
    let dir = std::env::temp_dir().join(format!("nb-rp-hookrows-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config_dir = dir.join("config");
    let _ = std::fs::create_dir_all(&config_dir);
    let sid = unique_sid("hookrows");
    std::fs::write(
        config_dir.join("hooks.json"),
        serde_json::json!({
            "hooks": {
                "Stop": [{
                    "scope": {"sessions": [sid]},
                    "hooks": [{"type": "prompt", "prompt": "现在几点了"}]
                }]
            }
        })
        .to_string(),
    )
    .expect("seed hooks.json");
    let view =
        HooksSessionProvider.collect(&ctx_for(&dir, &format!("agent:main:session:{sid}"), &sid));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 1);
    assert_eq!(view.summary["count"], json!(1));
    let rows = view.summary["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["event"], json!("Stop"));
    assert_eq!(rows[0]["kind"], json!("prompt"));
    assert_eq!(rows[0]["content"], json!("现在几点了"));
    let n = crate::cc_hooks::count_session_hooks(&config_dir, &sid).expect("count by bare sid");
    assert_eq!(n, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 最小 LLM provider 替身（SessionLoopApi 实现与 hooks_global 桥解析用；
/// 永不被调用——测试只查查询面）。
struct DeadProvider;

#[async_trait::async_trait]
impl LlmProvider for DeadProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("test-only provider".to_string())
    }
}

#[test]
fn hooks_global_none_slot_and_bridgeless_loop_are_empty() {
    // 槽缺席（headless/测试）→ Empty。
    let view = HooksGlobalProvider::new(None).collect(&ctx_for(
        Path::new("."),
        "agent:main:session:x",
        "x",
    ));
    assert_eq!(view.status, CategoryStatus::Empty);

    // 槽在但 loop 无桥（standalone AgentLoop 默认无桥）→ Empty。
    let slot: LoopSlot = Arc::new(parking_lot::RwLock::new(Some(Arc::new(
        crate::r#loop::AgentLoop::new(Box::new(DeadProvider), test_agent_config()),
    ))));
    let view = HooksGlobalProvider::new(Some(slot)).collect(&ctx_for(
        Path::new("."),
        "agent:main:session:x",
        "x",
    ));
    assert_eq!(view.status, CategoryStatus::Empty);
}

#[test]
fn hooks_global_counts_unscoped_hooks_for_any_session() {
    let bridge = Arc::new(
        crate::cc_hooks::CcHookBridge::from_json(
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"command":"echo one"},{"command":"echo two"}]}],
            "Stop":[{"hooks":[{"command":"echo s"}]}]}}"#,
            std::env::temp_dir(),
        )
        .expect("bridge json"),
    );
    let slot: LoopSlot = Arc::new(parking_lot::RwLock::new(Some(Arc::new({
        let l = crate::r#loop::AgentLoop::new(Box::new(DeadProvider), test_agent_config());
        l.set_cc_hooks_bridge(Arc::clone(&bridge));
        l
    }))));
    // 未圈定组 = 全会话命中；完整 session_key 与裸 sid 双串都应命中。
    let view = HooksGlobalProvider::new(Some(slot)).collect(&ctx_for(
        Path::new("."),
        "agent:main:session:x",
        "x",
    ));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 3);
    assert_eq!(view.summary["by_event"]["PreToolUse"], json!(2));
    assert_eq!(view.summary["by_event"]["Stop"], json!(1));
}

// ---------------------------------------------------------------------------
// ApprovalsPending / QuestionsPending（瞬态切面双件）
// ---------------------------------------------------------------------------

struct StubApprovalResponder {
    entries: Vec<Value>,
}

impl nemesis_types::agent::ApprovalResponder for StubApprovalResponder {
    fn respond(
        &self,
        _request_id: &str,
        _approved: bool,
        _always: bool,
        _note: Option<String>,
    ) -> Result<bool, String> {
        Err("test-only".into())
    }
    fn pending(&self) -> Vec<Value> {
        self.entries.clone()
    }
}

struct StubQuestionResponder {
    entries: Vec<Value>,
}

impl nemesis_types::agent::QuestionResponder for StubQuestionResponder {
    fn respond(&self, _question_id: &str, _selected: Vec<String>) -> Result<bool, String> {
        Err("test-only".into())
    }
    fn respond_form(
        &self,
        _question_id: &str,
        _values: &std::collections::BTreeMap<String, String>,
    ) -> Result<bool, String> {
        Err("test-only".into())
    }
    fn pending(&self) -> Vec<Value> {
        self.entries.clone()
    }
}

#[test]
fn approvals_pending_filters_session_and_reports_absent_as_unavailable() {
    let ctx_key = "agent:main:session:apr";
    // 槽缺席（headless）→ Unavailable。
    let view =
        ApprovalsPendingProvider::new(None).collect(&ctx_for(Path::new("."), ctx_key, "apr"));
    assert!(matches!(view.status, CategoryStatus::Unavailable { .. }));

    // 槽在、loop 无 responder → Unavailable（查不了 ≠ 没有）。
    let slot: LoopSlot = Arc::new(parking_lot::RwLock::new(Some(Arc::new(
        crate::r#loop::AgentLoop::new(Box::new(DeadProvider), test_agent_config()),
    ))));
    let view =
        ApprovalsPendingProvider::new(Some(slot)).collect(&ctx_for(Path::new("."), ctx_key, "apr"));
    assert!(matches!(view.status, CategoryStatus::Unavailable { .. }));

    // responder 在场：本会话 1 条 + 他会话 1 条 → 只回本会话切面。
    let responder: Arc<dyn nemesis_types::agent::ApprovalResponder> =
        Arc::new(StubApprovalResponder {
            entries: vec![
                json!({"request_id": "r1", "session_key": ctx_key, "pattern": "exec *"}),
                json!({"request_id": "r2", "session_key": "agent:main:session:other"}),
            ],
        });
    let slot: LoopSlot = Arc::new(parking_lot::RwLock::new(Some(Arc::new({
        let l = crate::r#loop::AgentLoop::new(Box::new(DeadProvider), test_agent_config());
        l.set_approval_responder(Arc::clone(&responder));
        l
    }))));
    let view = ApprovalsPendingProvider::new(Some(slot.clone())).collect(&ctx_for(
        Path::new("."),
        ctx_key,
        "apr",
    ));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 1);
    let rows = view.summary["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["request_id"], json!("r1"));

    // responder 在场但无本会话条目 → Empty（合法无）。
    let view = ApprovalsPendingProvider::new(Some(slot.clone())).collect(&ctx_for(
        Path::new("."),
        "agent:main:session:nobody",
        "nobody",
    ));
    assert_eq!(view.status, CategoryStatus::Empty);
}

#[test]
fn questions_pending_filters_session_and_reports_absent_as_unavailable() {
    let ctx_key = "agent:main:session:q";
    // 槽在、loop 无 responder → Unavailable。
    let slot: LoopSlot = Arc::new(parking_lot::RwLock::new(Some(Arc::new(
        crate::r#loop::AgentLoop::new(Box::new(DeadProvider), test_agent_config()),
    ))));
    let view =
        QuestionsPendingProvider::new(Some(slot)).collect(&ctx_for(Path::new("."), ctx_key, "q"));
    assert!(matches!(view.status, CategoryStatus::Unavailable { .. }));

    // responder 在场：本会话 2 条 → Ok(2)。
    let responder: Arc<dyn nemesis_types::agent::QuestionResponder> =
        Arc::new(StubQuestionResponder {
            entries: vec![
                json!({"question_id": "q1", "session_key": ctx_key, "question": "A 还是 B？"}),
                json!({"question_id": "q2", "session_key": ctx_key, "question": "继续？"}),
                json!({"question_id": "q3", "session_key": "agent:main:session:elsewhere"}),
            ],
        });
    let slot: LoopSlot = Arc::new(parking_lot::RwLock::new(Some(Arc::new({
        let l = crate::r#loop::AgentLoop::new(Box::new(DeadProvider), test_agent_config());
        l.set_question_responder(Arc::clone(&responder));
        l
    }))));
    let view = QuestionsPendingProvider::new(Some(slot.clone())).collect(&ctx_for(
        Path::new("."),
        ctx_key,
        "q",
    ));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 2);
    let rows = view.summary["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["question_id"], json!("q1"));

    // 无条目会话视角 → Empty。
    let view = QuestionsPendingProvider::new(Some(slot.clone())).collect(&ctx_for(
        Path::new("."),
        "agent:main:session:nobody",
        "nobody",
    ));
    assert_eq!(view.status, CategoryStatus::Empty);
}

// ---------------------------------------------------------------------------
// BackgroundProvider（后台进程）
// ---------------------------------------------------------------------------

#[test]
fn background_unavailable_without_registry_and_empty_on_fresh_registry() {
    let view = BackgroundProvider::new(None).collect(&ctx_for(
        Path::new("."),
        "agent:main:session:x",
        "x",
    ));
    assert!(matches!(view.status, CategoryStatus::Unavailable { .. }));
    assert!(view.status.unavailable_reason().is_some());

    let reg = Arc::new(crate::background_registry::BackgroundProcessRegistry::new());
    let view = BackgroundProvider::new(Some(reg)).collect(&ctx_for(
        Path::new("."),
        "agent:main:session:empty",
        "empty",
    ));
    assert_eq!(view.status, CategoryStatus::Empty);
}

#[test]
fn background_snapshot_filters_by_origin_session() {
    // 空 origin（基线 start() 路径模拟不存在——本测试只验证过滤口径不
    // 需要真任务）：注册表空 = Empty 已在上测覆盖；此处验证 snapshot 的
    // origin 过滤路径在无命中时与 Empty 汇合。
    let reg = Arc::new(crate::background_registry::BackgroundProcessRegistry::new());
    let rows = reg
        .list_by_session_snapshot("agent:main:session:none")
        .expect("snapshot ok");
    assert!(rows.is_empty());
}

// ---------------------------------------------------------------------------
// SubagentsProvider（子代理）
// ---------------------------------------------------------------------------

#[test]
fn subagents_filter_parent_and_exclude_archived() {
    let key = format!("agent:main:session:{}", unique_sid("sub"));
    let other = format!("agent:main:session:{}-other", unique_sid("sub"));
    let reg = crate::subagent_registry::global();
    reg.begin(crate::subagent_registry::SubagentBegin {
        id: format!("{key}::a"),
        parent_session_key: key.clone(),
        parent_chat_id: "web:t".into(),
        origin: "model_spawn".into(),
        agent_id: "agent-x".into(),
        task: "帮我跑测试".into(),
        role: String::new(),
        tools_profile: String::new(),
        depth: 1,
        background: false,
        loop_origin: "main".into(),
        bg_task_id: None,
    });
    reg.begin(crate::subagent_registry::SubagentBegin {
        id: format!("{key}::b"),
        parent_session_key: other.clone(),
        parent_chat_id: "web:t".into(),
        origin: "model_spawn".into(),
        agent_id: "agent-y".into(),
        task: "别家任务".into(),
        role: String::new(),
        tools_profile: String::new(),
        depth: 1,
        background: false,
        loop_origin: "main".into(),
        bg_task_id: None,
    });
    let view = SubagentsProvider.collect(&ctx_for(Path::new("."), &key, "sub"));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 1);
    assert_eq!(view.summary["active"], json!(1));
    assert_eq!(view.summary["rows"][0]["task"], json!("帮我跑测试"));

    // 完成一个 + 归档一个 → 工作面清空。
    reg.finish(
        &format!("{key}::a"),
        crate::subagent_registry::SubagentStatus::Done,
        Some(1),
        Some("done".into()),
        None,
    );
    reg.archive(&[format!("{key}::a")]);
    let view = SubagentsProvider.collect(&ctx_for(Path::new("."), &key, "sub"));
    assert_eq!(view.status, CategoryStatus::Empty);
}

// ---------------------------------------------------------------------------
// FileChangesProvider（文件变更）——chat_log 聚合
// ---------------------------------------------------------------------------

#[test]
fn file_changes_collects_from_assistant_rows() {
    use crate::chat_log::{ChatLogMeta, append_chat_log_meta};
    let key = format!("agent:main:session:{}", unique_sid("fc"));
    let changes = [
        crate::r#loop::FileChange {
            path: "src/a.rs".into(),
            kind: crate::r#loop::FileChangeKind::Create,
        },
        crate::r#loop::FileChange {
            path: "src/b.rs".into(),
            kind: crate::r#loop::FileChangeKind::Modify,
        },
    ];
    // 一条无关 user 行 + 一条带 file_changes 的 assistant 行。
    crate::chat_log::append_chat_log(&key, "user", "帮我加个文件");
    append_chat_log_meta(
        &key,
        "assistant",
        "done",
        &ChatLogMeta {
            file_changes: &changes,
            ..Default::default()
        },
    );
    let view = FileChangesProvider.collect(&ctx_for(Path::new("."), &key, "fc"));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 2);
    let rows = view.summary["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["path"], json!("src/a.rs"));
    assert_eq!(rows[0]["kind"], json!("Create"));
    assert_eq!(rows[1]["path"], json!("src/b.rs"));
    assert_eq!(view.summary["omitted"], json!(0));

    // 无变更会话 → Empty。
    let none = format!("agent:main:session:{}", unique_sid("fc-none"));
    crate::chat_log::append_chat_log(&none, "user", "只是聊天");
    let view = FileChangesProvider.collect(&ctx_for(Path::new("."), &none, "fc-none"));
    assert_eq!(view.status, CategoryStatus::Empty);
}

// ---------------------------------------------------------------------------
// impl SessionLoopApi for AgentLoop（真 loop 基线）
// ---------------------------------------------------------------------------

fn test_agent_config() -> crate::types::AgentConfig {
    crate::types::AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("test".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

#[test]
fn session_loop_api_baseline_honest_zeroes() {
    let l = crate::r#loop::AgentLoop::new(Box::new(DeadProvider), test_agent_config());
    // checkpoint store 未挂 → 空。
    assert!(SessionLoopApi::checkpoint_summary(&l, "agent:main:session:x").is_empty());
    // 无排队 → (0, 0)。
    assert_eq!(
        SessionLoopApi::inbox_pending(&l, "agent:main:session:x"),
        (0, 0)
    );
    // 无在跑轮次 → None。
    assert!(SessionLoopApi::work_timeline(&l, "agent:main:session:x").is_none());
    // context 快照恒有值（空历史按零占用计）。
    let v = SessionLoopApi::context_status(&l, "agent:main:session:x");
    assert!(v.is_some());
}
