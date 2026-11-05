// （子代理可见性）：集群 delegate 任务并入子代理注册表
// 的 loop 级咬合测试。begin = `save_async_continuation`（`__ASYNC__`
// marker 落地侧），finish = `dispatch_continuation`（续行回灌终态侧）。
// 两条被测路径都不触发 LLM：save 侧 continuation_manager=None 跳过快照；
// dispatch 侧默认 inline（permits=0）+ manager/tx 皆 None = 纯台账副作用。
//
// 注册表是进程级全局单例——id 一律带纳秒唯一 marker，断言按 id 精确
// 取行（`get`），测试收尾 finish 落终态防泄漏 Running 视图。

use super::*;

/// 占位 provider（恒 Err）：被测路径不该调 LLM，调用即测试自身 bug。
struct NeverCalledProvider;

#[async_trait]
impl LlmProvider for NeverCalledProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("delegate registry tests must not call the LLM".to_string())
    }
}

fn d5_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn d5_call(id: &str, args: &str) -> crate::types::ToolCallInfo {
    crate::types::ToolCallInfo {
        id: id.to_string(),
        name: "cluster_rpc".to_string(),
        arguments: args.to_string(),
    }
}

fn d5_msg(
    content: &str,
    metadata: std::collections::HashMap<String, String>,
) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "system".to_string(),
        chat_id: "chat1".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: "web:chat1".to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    }
}

fn d5_unique(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("d5-{tag}-{nanos}")
}

// ---------------------------------------------------------------------------
// begin 侧：合法 `__ASYNC__` marker → 注册表 delegate 行全字段投影
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delegate_begin_registers_row_with_full_projection() {
    let loop_ = AgentLoop::new(Box::new(NeverCalledProvider), d5_config());
    loop_.set_loop_origin("project:p1");
    let instance = AgentInstance::new(d5_config());
    let ctx = RequestContext::new("web", "chat-d5a", "user1", "session-d5a");
    let task_id = d5_unique("begin");
    let tc = d5_call("call-1", r#"{"target":"Node-B","message":"帮我编译内核"}"#);
    let marker = format!("__ASYNC__:{task_id}:node-b-1:Node-B");

    let ev = loop_
        .save_async_continuation(&instance, &ctx, &tc, &marker)
        .await;
    assert!(
        matches!(ev, Some(AgentEvent::Done(_))),
        "合法 marker 必须命中（Some(Done)）"
    );

    let reg = crate::subagent_registry::global();
    let row = reg.get(&task_id).expect("delegate 行必须入册");
    assert_eq!(row.origin, "delegate");
    assert_eq!(row.agent_id, "node-b-1");
    assert_eq!(
        row.task, "帮我编译内核",
        "任务文本 = cluster_rpc args.message"
    );
    assert_eq!(row.role, "");
    assert_eq!(row.tools_profile, "");
    assert_eq!(row.depth, 0);
    assert!(row.background, "delegate 恒后台");
    assert_eq!(row.loop_origin, "project:p1", "loop 宿主形态抄进注册表");
    assert_eq!(row.bg_task_id.as_deref(), Some(task_id.as_str()));
    assert_eq!(row.parent_session_key, "session-d5a");
    assert_eq!(row.parent_chat_id, "chat-d5a");
    assert_eq!(
        row.status,
        crate::subagent_registry::SubagentStatus::Running
    );
    assert!(row.finished_at_ms.is_none());

    // 清理：落终态防泄漏 Running 视图到同进程其他测试。
    reg.finish(
        &task_id,
        crate::subagent_registry::SubagentStatus::Done,
        None,
        None,
        None,
    );
}

// ---------------------------------------------------------------------------
// begin 侧：畸形 marker → None 落回批次，不入册
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delegate_no_begin_on_malformed_marker() {
    let loop_ = AgentLoop::new(Box::new(NeverCalledProvider), d5_config());
    let instance = AgentInstance::new(d5_config());
    let ctx = RequestContext::new("web", "chat-d5b", "user1", "session-d5b");
    let bad_id = d5_unique("bad");
    let tc = d5_call("call-1", r#"{"target":"Node-B","message":"x"}"#);

    // 两段式（<3 段）畸形 marker → None（落回批次继续），不入册。
    let ev = loop_
        .save_async_continuation(&instance, &ctx, &tc, &format!("__ASYNC__:{bad_id}"))
        .await;
    assert!(ev.is_none(), "畸形 marker 必须 None");
    assert!(
        crate::subagent_registry::global().get(&bad_id).is_none(),
        "畸形 marker 不入册"
    );
}

// ---------------------------------------------------------------------------
// finish 侧：续行回灌 → Done / Failed 终态投影
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delegate_finish_on_continuation_arrival() {
    let loop_ = AgentLoop::new(Box::new(NeverCalledProvider), d5_config());
    let instance = AgentInstance::new(d5_config());
    let ctx = RequestContext::new("web", "chat-d5c", "user1", "session-d5c");
    let reg = crate::subagent_registry::global();

    // Done 臂：begin 入册（最短咬合路径）→ 空 metadata 回灌 → Done。
    let done_id = d5_unique("fin-done");
    let tc = d5_call("call-1", r#"{"message":"任务A"}"#);
    loop_
        .save_async_continuation(&instance, &ctx, &tc, &format!("__ASYNC__:{done_id}:w1:W1"))
        .await;
    loop_
        .dispatch_continuation(
            done_id.clone(),
            &d5_msg("任务完成正文", std::collections::HashMap::new()),
        )
        .await;
    let row = reg.get(&done_id).expect("Done 臂行必须在册");
    assert_eq!(row.status, crate::subagent_registry::SubagentStatus::Done);
    assert_eq!(row.summary.as_deref(), Some("任务完成正文"));
    assert_eq!(row.error, None);
    assert!(row.finished_at_ms.is_some(), "终态行带时长锚点");

    // Failed 臂：metadata status=error + error 字段 → Failed + error 摘录。
    let failed_id = d5_unique("fin-fail");
    let tc2 = d5_call("call-2", r#"{"message":"任务B"}"#);
    loop_
        .save_async_continuation(
            &instance,
            &ctx,
            &tc2,
            &format!("__ASYNC__:{failed_id}:w2:W2"),
        )
        .await;
    let mut meta = std::collections::HashMap::new();
    meta.insert("status".to_string(), "error".to_string());
    meta.insert("error".to_string(), "远端节点炸了".to_string());
    loop_
        .dispatch_continuation(failed_id.clone(), &d5_msg("", meta))
        .await;
    let row = reg.get(&failed_id).expect("Failed 臂行必须在册");
    assert_eq!(row.status, crate::subagent_registry::SubagentStatus::Failed);
    assert_eq!(row.error.as_deref(), Some("远端节点炸了"));
    assert_eq!(row.summary, None, "空回复 → summary 诚实留白");
}

// ---------------------------------------------------------------------------
// finish 侧：不在册 task_id（后台子代理续行形态）→ 静默 miss
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delegate_finish_silently_misses_unknown_task() {
    let loop_ = AgentLoop::new(Box::new(NeverCalledProvider), d5_config());
    // 后台子代理续行（`subagent_continuation:{task_id}`）的 task_id 不在
    // 注册表（行 id 是 `subagent:{uuid}`）→ finish 静默 miss =「不在册
    // 不回写」，不得 panic。
    let unknown = d5_unique("unknown");
    loop_
        .dispatch_continuation(
            unknown.clone(),
            &d5_msg("x", std::collections::HashMap::new()),
        )
        .await;
    assert!(
        crate::subagent_registry::global().get(&unknown).is_none(),
        "不在册 id 不得被凭空建行"
    );
}

// ---------------------------------------------------------------------------
// finish 侧：用户 cancel 后回灌 → 真实结局覆盖本地视图
//（bus.rs 侧注释钉死的语义：B 端结果才是任务真相，cancel 只代表
// 「不再等待显示」）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delegate_real_outcome_overwrites_user_cancel_view() {
    let loop_ = AgentLoop::new(Box::new(NeverCalledProvider), d5_config());
    let instance = AgentInstance::new(d5_config());
    let ctx = RequestContext::new("web", "chat-d5d", "user1", "session-d5d");
    let reg = crate::subagent_registry::global();
    let task_id = d5_unique("cancel");
    let tc = d5_call("call-1", r#"{"message":"任务C"}"#);
    loop_
        .save_async_continuation(&instance, &ctx, &tc, &format!("__ASYNC__:{task_id}:w3:W3"))
        .await;

    // 用户侧 cancel → Cancelled 本地视图。
    assert!(reg.cancel(&task_id), "cancel 必须命中在册任务");
    assert_eq!(
        reg.get(&task_id).unwrap().status,
        crate::subagent_registry::SubagentStatus::Cancelled
    );

    // 真实结局回灌 → 覆盖 Cancelled（不猜不保留旧态）。
    loop_
        .dispatch_continuation(
            task_id.clone(),
            &d5_msg("实际完成了", std::collections::HashMap::new()),
        )
        .await;
    let row = reg.get(&task_id).expect("覆盖后行仍在册");
    assert_eq!(row.status, crate::subagent_registry::SubagentStatus::Done);
    assert_eq!(row.summary.as_deref(), Some("实际完成了"));
}
