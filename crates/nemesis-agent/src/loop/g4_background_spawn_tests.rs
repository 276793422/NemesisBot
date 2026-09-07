// G4 (devtool-upgrade 阶段 3)：subagent 后台化 + 完成回灌测试。
//
// 覆盖面（对应实施计划 G4 验收）：
// ① schema：background 可选参数（不在 required）；
// ② SpawnFn 第 8 参透传——background=true / 缺省 false 到闭包；
// ③ `__BG_SPAWN__` marker → 续行快照 inline 落内存 + 中间回复 + 回合收尾；
// ④ gate_inbound 拦截 `subagent_continuation:` 前缀 → GateOutcome::Continuation
//    （集群前缀不回归、非前缀不误拦）；
// ⑤ 完成回灌端到端——bus 消息 → dispatch_continuation →
//    handle_cluster_continuation 复用路径 → 续行回复出站 + 快照自清；
// ⑥ 无 continuation_manager 时 marker 优雅降级（仍收尾回合，不 panic）；
// ⑦ list_bg_spawn_pending_sync 前缀过滤 + 只读不删（重启恢复数据源）。
//
// 自带迷你 Mock（本文件独立，不与 s9/tests 共享私有类型）。

use super::*;

// ---------------------------------------------------------------------------
// 迷你 provider / 消息 / 响应 helpers（s9 形状）
// ---------------------------------------------------------------------------

struct G4MockProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
}

impl G4MockProvider {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
    }
}

#[async_trait]
impl LlmProvider for G4MockProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "g4 exhausted".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

fn g4_resp(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn g4_tc_resp(calls: Vec<crate::types::ToolCallInfo>) -> LlmResponse {
    LlmResponse {
        content: String::new(),
        tool_calls: calls,
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn g4_call(id: &str, name: &str, args: &str) -> crate::types::ToolCallInfo {
    crate::types::ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        arguments: args.to_string(),
    }
}

fn g4_msg(channel: &str, sender_id: &str, content: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: "chat1".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: format!("{}:{}", channel, "chat1"),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

fn g4_plain(content: &str) -> nemesis_types::channel::InboundMessage {
    g4_msg("web", "user1", content)
}

fn g4_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["spawn".to_string()],
        models: std::collections::HashMap::new(),
    }
}

fn g4_spawn_config() -> crate::loop_tools::SpawnConfig {
    crate::loop_tools::SpawnConfig {
        default_model: "test-model".to_string(),
        max_concurrent: 2,
        max_depth: 3,
    }
}

/// 返回固定 `__BG_SPAWN__` marker 的 spawn 闭包（模拟 agent_factory 后台路径
/// 的返回值——任务已转后台、立即返回 marker）。
fn bg_marker_spawn_fn(task_id: &str) -> crate::loop_tools::SpawnFn {
    let marker = format!("__BG_SPAWN__:{}", task_id);
    std::sync::Arc::new(
        move |_a: &str, _t: &str, _m: &str, _c: &str, _ch: &str, _p: &str, _d: usize, _bg: bool| {
            let marker = marker.clone();
            Box::pin(async move { Ok(marker.clone()) })
        },
    )
}

// ---------------------------------------------------------------------------
// ① schema：background 可选
// ---------------------------------------------------------------------------

#[test]
fn schema_background_is_optional() {
    let tool = crate::loop_tools::SpawnTool::new(g4_spawn_config());
    let schema = tool.parameters();
    assert!(
        schema["properties"]["background"].is_object(),
        "background 必须在 properties: {}",
        schema
    );
    let required = schema["required"].as_array().expect("required 数组");
    assert!(
        !required.iter().any(|v| v == "background"),
        "background 必须可选（不在 required），required={required:?}"
    );
    assert!(
        required.iter().any(|v| v == "task"),
        "task 保持必填，required={required:?}"
    );
}

// ---------------------------------------------------------------------------
// ② SpawnFn 第 8 参透传
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_fn_receives_background_flag() {
    for (args, expect) in [
        (r#"{"task":"t","background":true}"#, true),
        (r#"{"task":"t"}"#, false),
        (r#"{"task":"t","background":false}"#, false),
    ] {
        let mut tool = crate::loop_tools::SpawnTool::new(g4_spawn_config());
        let captured: std::sync::Arc<std::sync::Mutex<Option<bool>>> = Default::default();
        let cap = captured.clone();
        tool.set_spawn_fn(std::sync::Arc::new(
            move |_a: &str,
                  _t: &str,
                  _m: &str,
                  _c: &str,
                  _ch: &str,
                  _p: &str,
                  _d: usize,
                  bg: bool| {
                let cap = cap.clone();
                Box::pin(async move {
                    *cap.lock().unwrap() = Some(bg);
                    Ok("ok".to_string())
                })
            },
        ));
        let ctx = RequestContext::new("web", "chat-1", "user1", "session1");
        tool.execute(args, &ctx).await.expect("spawn 必须成功");
        assert_eq!(
            captured.lock().unwrap().clone(),
            Some(expect),
            "background 透传失配（args={args}）"
        );
    }
}

// ---------------------------------------------------------------------------
// ③ __BG_SPAWN__ marker：快照 inline 落内存 + 中间回复 + 回合收尾
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bg_marker_saves_snapshot_and_ends_turn() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(G4MockProvider::new(vec![
            g4_tc_resp(vec![g4_call(
                "b1",
                "spawn",
                r#"{"task":"long research","background":true}"#,
            )]),
            g4_resp("never reached in the same turn"),
        ])),
        g4_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mut tool = crate::loop_tools::SpawnTool::new(g4_spawn_config());
    tool.set_spawn_fn(bg_marker_spawn_fn("bg_t1"));
    agent_loop.register_tool("spawn".to_string(), Box::new(tool));
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    agent_loop.set_continuation_manager(mgr.clone());

    in_tx.send(g4_plain("run it in background")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    // 中间回复（已派后台任务）先出站，主回合收尾——不再消耗第二个 LLM 响应。
    let out = out_rx.recv().await.expect("intermediate reply published");
    assert!(
        out.content.contains("后台子代理"),
        "intermediate must announce background dispatch, got: {}",
        out.content
    );

    // 快照保存是 inline await —— run_bus_owned 返回时必然已落内存
    // （对照 __ASYNC__ 集群路径的 spawn 保存需要轮询等待）。
    assert!(
        mgr.has_continuation("bg_t1").await,
        "continuation snapshot for bg_t1 must be saved inline"
    );
}

// ---------------------------------------------------------------------------
// ④ gate_inbound 前缀拦截
// ---------------------------------------------------------------------------

#[test]
fn gate_inbound_routes_subagent_continuation_prefix() {
    let agent_loop = AgentLoop::new(Box::new(G4MockProvider::new(vec![])), g4_config());

    // subagent 前缀 → Continuation(task_id)。
    let msg = g4_msg("system", "subagent_continuation:bg_77", "sub-agent result");
    match agent_loop.gate_inbound(&msg) {
        GateOutcome::Continuation(id) => assert_eq!(id, "bg_77"),
        _ => panic!("subagent prefix must route to Continuation"),
    }

    // 集群前缀不回归。
    let msg = g4_msg("system", "cluster_continuation:c_9", "cluster result");
    match agent_loop.gate_inbound(&msg) {
        GateOutcome::Continuation(id) => assert_eq!(id, "c_9"),
        _ => panic!("cluster prefix must still route to Continuation"),
    }

    // 近似前缀不误拦（少冒号 / 前缀是别的前缀的超集情形）。
    for sender in ["subagent_continuationX:bg_1", "cluster_continuation"] {
        let msg = g4_msg("system", sender, "x");
        assert!(
            !matches!(agent_loop.gate_inbound(&msg), GateOutcome::Continuation(_)),
            "sender {sender:?} must not be intercepted"
        );
    }
}

// ---------------------------------------------------------------------------
// ⑤ 完成回灌端到端（复用集群续行路径）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn completion_message_resumes_with_subagent_result() {
    let _logs = crate::test_support::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);

    // 第一回合：spawn(background=true) → __BG_SPAWN__ marker → 中间回复。
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(G4MockProvider::new(vec![
            g4_tc_resp(vec![g4_call(
                "b1",
                "spawn",
                r#"{"task":"long build","background":true}"#,
            )]),
            g4_resp("BG RESULT: 42 files patched"),
        ])),
        g4_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_workspace_root(tmp.path().to_path_buf());
    let mut tool = crate::loop_tools::SpawnTool::new(g4_spawn_config());
    tool.set_spawn_fn(bg_marker_spawn_fn("bg_r1"));
    agent_loop.register_tool("spawn".to_string(), Box::new(tool));
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    agent_loop.set_continuation_manager(mgr.clone());
    // 两个回合各消费一次 run_bus（按值 receiver）——包 Arc 走 run_bus_arc。
    let agent_loop = std::sync::Arc::new(agent_loop);

    in_tx.send(g4_plain("start the build")).await.unwrap();
    drop(in_tx);
    agent_loop.clone().run_bus_arc(in_rx).await;
    let intermediate = out_rx.recv().await.expect("intermediate reply");
    assert!(intermediate.content.contains("后台子代理"));

    // 第二回合：后台完成回灌（agent_factory 后台路径发布的 bus 消息形状）
    // → gate 拦截 → dispatch_continuation（permits=0 inline）→
    // handle_cluster_continuation 复用路径 → 续行 LLM → 最终回复出站。
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("status".to_string(), "ok".to_string());
    metadata.insert("source".to_string(), "background_subagent".to_string());
    let mut completion = g4_msg(
        "system",
        "subagent_continuation:bg_r1",
        "BG RESULT: 42 files patched",
    );
    completion.metadata = metadata;

    let (in_tx2, in_rx2) = tokio::sync::mpsc::channel(16);
    in_tx2.send(completion).await.unwrap();
    drop(in_tx2);
    agent_loop.clone().run_bus_arc(in_rx2).await;

    let final_msg = out_rx.recv().await.expect("resumed final reply");
    assert!(
        final_msg.content.contains("42 files patched"),
        "final reply must carry the sub-agent result, got: {}",
        final_msg.content
    );
    // 快照消费后自清（handle_cluster_continuation → remove_continuation）。
    assert!(
        !mgr.has_continuation("bg_r1").await,
        "snapshot must be cleaned up after resume"
    );
}

// ---------------------------------------------------------------------------
// ⑥ 无 continuation_manager：marker 优雅降级
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bg_marker_without_manager_still_ends_turn() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(G4MockProvider::new(vec![g4_tc_resp(vec![g4_call(
            "b1",
            "spawn",
            r#"{"task":"x","background":true}"#,
        )])])),
        g4_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mut tool = crate::loop_tools::SpawnTool::new(g4_spawn_config());
    tool.set_spawn_fn(bg_marker_spawn_fn("bg_t2"));
    agent_loop.register_tool("spawn".to_string(), Box::new(tool));
    // 刻意不 set_continuation_manager。

    in_tx.send(g4_plain("go")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx
        .recv()
        .await
        .expect("intermediate reply still published");
    assert!(out.content.contains("后台子代理"), "got: {}", out.content);
}

// ---------------------------------------------------------------------------
// ⑦ list_bg_spawn_pending_sync：前缀过滤 + 只读不删
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_bg_spawn_pending_filters_prefix_and_keeps_entries() {
    let mgr = crate::loop_continuation::ContinuationManager::new();
    mgr.save_continuation("bg_a1", vec![], "tc1", "web", "c1", "sk", "")
        .await;
    mgr.save_continuation("cluster_a2", vec![], "tc2", "web", "c1", "sk", "peer1")
        .await;

    let listed = mgr.list_bg_spawn_pending_sync();
    assert_eq!(listed, vec!["bg_a1".to_string()], "只列 bg_ 前缀");

    // 只读不删：再次列举结果一致，快照仍可加载（重启恢复注入后走正常续行）。
    assert_eq!(mgr.list_bg_spawn_pending_sync(), listed);
    assert!(mgr.load_continuation("bg_a1").await.is_some());

    // 正常续行消费后（remove_continuation）从清单消失。
    mgr.remove_continuation("bg_a1").await;
    assert!(mgr.list_bg_spawn_pending_sync().is_empty());
}
