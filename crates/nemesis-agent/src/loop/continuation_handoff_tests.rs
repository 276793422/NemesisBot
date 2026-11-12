// （续行回主路 2026-11-12）：`run_continuation_turn` 交接函数契约哨兵。
//
// 旧路径 `handle_cluster_continuation` 的结构性缺陷即切换动机：LLM 请求
// 硬编码 tools=0、工具执行裸 `execute_tool_for_continuation` 绕过
// dispatch 瀑布。本文件把新交接函数的关键契约逐一钉死：
// ① 续行 LLM 请求**必须携带工具定义**（tools=0 直接回归锁）+ 回合预算
//    常量在位；
// ② 工具执行**必须走 dispatch 瀑布**——hidden_tools 供给+dispatch 双闸
//    对续行回合同样生效（旧裸 execute 会执行被隐藏工具，此测试必红）；
// ③ 出站 meta 保真——source_node/task_id/session_key/receipt/occupies/
//    model 逐字段（deliveries 记账与节点徽章依赖）；
// ④ estop 在回调到达时冻结——零 LLM 调用 + ⛔ 注记出站 + 快照收口；
// ⑤ busy Reject 臂——终局注记（含任务结果文本，不静默丢弃）+ 快照收口；
// ⑥ busy Queue 臂——重投递入队（不消费快照、零出站），回合尾巴 drain
//    自然排空；
// ⑦ final_persisted 幂等闸——崩溃窗口后重投递跳过重复续行；
// ⑧ 落库保真——store 会话历史含结果兜底轮 + assistant 终轮；
// ⑨ 失败任务错误文本（"Error: {error}"）进续行 LLM 上下文；
// ⑩ adversarial：续行回合高危工具调用走审批卡（dispatch 瀑布在续行同样
//    生效——旧裸 execute 无此闸，脚本化 tool_call 会被静默执行）。
//
// 全部经 run_bus_owned 泵全链路驱动（gate → Continuation →
// run_continuation_turn），自带迷你 provider + 计数工具，不与 l5/g4/f8
// 共享私有类型——本仓测试文件独立惯例。

use super::*;

/// 记录每次 chat 收到的 messages + tools 数的 mock provider（哨兵核心：
/// tools 数与上下文内容都从活体请求断言，不看出站猜行为）。
struct HandoffProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    calls: std::sync::Arc<std::sync::Mutex<Vec<HandoffCall>>>,
}

#[derive(Clone)]
struct HandoffCall {
    messages: Vec<LlmMessage>,
    tools_count: usize,
}

impl HandoffProvider {
    fn new(
        responses: Vec<LlmResponse>,
        calls: std::sync::Arc<std::sync::Mutex<Vec<HandoffCall>>>,
    ) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
            calls,
        }
    }
}

#[async_trait]
impl LlmProvider for HandoffProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.lock().unwrap().push(HandoffCall {
            messages,
            tools_count: tools.len(),
        });
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "handoff exhausted".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                finish_reason: None,
                escalated_to: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

fn hs_resp(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        finish_reason: None,
        escalated_to: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 带单个工具调用的响应（让续行回合真正走一轮 dispatch）。
fn hs_tool_call_resp(tool_name: &str) -> LlmResponse {
    LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "call_hs_1".to_string(),
            name: tool_name.to_string(),
            arguments: "{}".to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        finish_reason: None,
        escalated_to: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 计数型测试工具（dispatch 瀑布放行才会被计到——裸 execute 也会计到，
/// 被 hidden 的调用只有瀑布会拒，见 ② 哨兵）。
struct HsTool {
    executions: std::sync::Arc<std::sync::Mutex<usize>>,
}

impl HsTool {
    fn new() -> (Self, std::sync::Arc<std::sync::Mutex<usize>>) {
        let executions = std::sync::Arc::new(std::sync::Mutex::new(0));
        (
            Self {
                executions: executions.clone(),
            },
            executions,
        )
    }
}

#[async_trait]
impl Tool for HsTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        *self.executions.lock().unwrap() += 1;
        Ok("RESULT: handoff tool ran".to_string())
    }
}

fn hs_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: Vec::new(),
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn hs_msg(
    sender_id: &str,
    content: &str,
    metadata: std::collections::HashMap<String, String>,
) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "system".to_string(),
        sender_id: sender_id.to_string(),
        chat_id: "chatHS".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: String::new(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    }
}

/// 种一个最小续行快照（user 消息 + 待应答 tool_call；session=hsess 非
/// 空——busy 闸/活体装配/落库全部有主体）。
async fn seed_continuation(mgr: &crate::loop_continuation::ContinuationManager, task_id: &str) {
    mgr.save_continuation(
        task_id,
        vec![
            LlmMessage {
                role: "user".to_string(),
                content: "run the remote task".to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
                images: Vec::new(),
            },
            LlmMessage {
                role: "assistant".to_string(),
                content: String::new(),
                tool_calls: Some(vec![ToolCallInfo {
                    id: format!("tc_{task_id}"),
                    name: "cluster_rpc".to_string(),
                    arguments: "{}".to_string(),
                }]),
                tool_call_id: None,
                reasoning_content: None,
                images: Vec::new(),
            },
        ],
        &format!("tc_{task_id}"),
        "web",
        "chatHS",
        "hsess",
        "",
    )
    .await;
}

/// 轮询断言「续行无出站」（busy 重投递 / 幂等跳过等零出站臂）。
async fn assert_no_outbound(
    out_rx: &mut tokio::sync::mpsc::Receiver<nemesis_types::channel::OutboundMessage>,
) {
    for _ in 0..40 {
        assert!(
            out_rx.try_recv().is_err(),
            "本臂不得产生出站（零 LLM/零回复语义）"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// ① 核心哨兵：续行 LLM 请求携带工具定义（旧路径 tools=0 的直接回归锁）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn continuation_llm_request_carries_tool_definitions() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(HandoffProvider::new(
            vec![hs_tool_call_resp("echo_hs"), hs_resp("handoff final reply")],
            calls.clone(),
        )),
        hs_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let (tool, executions) = HsTool::new();
    agent_loop.register_tool("echo_hs".to_string(), Box::new(tool));
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "hstools").await;
    agent_loop.set_continuation_manager(mgr);

    let msg = hs_msg(
        &format!(
            "{}hstools",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "task result text",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    // 续行 LLM 第一跳必须看到工具定义（旧迷你循环硬编码 tools=0）。
    let calls = calls.lock().unwrap().clone();
    assert!(!calls.is_empty(), "续行 LLM 调用必须发生");
    assert!(
        calls[0].tools_count >= 1,
        "续行请求必须携带工具定义（旧路径 tools=0 回归锁），实际 {}",
        calls[0].tools_count
    );
    // 工具确实被执行了一轮（dispatch 放行 → execute）。
    assert_eq!(*executions.lock().unwrap(), 1);
    let out = out_rx.recv().await.expect("continuation final published");
    assert!(
        out.content.contains("handoff final reply"),
        "got: {}",
        out.content
    );
}

// ---------------------------------------------------------------------------
// ② dispatch 瀑布哨兵：hidden_tools 双闸对续行回合生效
// ---------------------------------------------------------------------------

#[tokio::test]
async fn continuation_tool_execution_goes_through_dispatch_waterfall() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(HandoffProvider::new(
            vec![
                // 模型幻觉调用被隐藏的工具（供给面已不可见）。
                hs_tool_call_resp("echo_hs"),
                hs_resp("final after rejection"),
            ],
            calls.clone(),
        )),
        hs_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let (tool, executions) = HsTool::new();
    agent_loop.register_tool("echo_hs".to_string(), Box::new(tool));
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");
    std::fs::write(&cfg, r#"{"agents": {"hidden_tools": ["echo_hs"]}}"#).unwrap();
    agent_loop.set_config_path(cfg);
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "hsgate").await;
    agent_loop.set_continuation_manager(mgr);

    let msg = hs_msg(
        &format!(
            "{}hsgate",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "task result text",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let calls = calls.lock().unwrap().clone();
    assert!(
        calls.len() >= 2,
        "拒绝回灌后应有第二跳，实际 {} 跳",
        calls.len()
    );
    // 供给闸：被隐藏工具不进续行请求的工具定义。
    assert_eq!(calls[0].tools_count, 0, "被 hidden 的工具必须从供给面剔除");
    // dispatch 闸：脚本化调用被拦，**不执行**（旧裸 execute 无此闸必红）。
    assert_eq!(*executions.lock().unwrap(), 0, "被隐藏工具不得执行");
    // 第二跳的上下文里带 dispatch 拒绝文案（结构化错误回灌）。
    let second: Vec<&LlmMessage> = calls[1]
        .messages
        .iter()
        .filter(|m| m.role == "tool")
        .collect();
    assert!(
        second
            .iter()
            .any(|m| m.content.contains("hidden by configuration")),
        "dispatch 拒绝必须回灌为 tool 结果，实际: {:?}",
        second.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
    let out = out_rx.recv().await.expect("continuation final published");
    assert!(out.content.contains("final after rejection"));
}

// ---------------------------------------------------------------------------
// ③ 出站 meta 保真（deliveries 记账 + 节点徽章依赖）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn continuation_outbound_meta_fidelity() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(HandoffProvider::new(
            vec![hs_resp("meta final")],
            calls.clone(),
        )),
        hs_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "hsmeta").await;
    agent_loop.set_continuation_manager(mgr);

    let mut metadata = std::collections::HashMap::new();
    metadata.insert("source_node".to_string(), "Node-B".to_string());
    let msg = hs_msg(
        &format!(
            "{}hsmeta",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "task result text",
        metadata,
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx.recv().await.expect("continuation final published");
    assert_eq!(out.channel, "web");
    assert_eq!(out.chat_id, "chatHS");
    assert!(out.content.contains("meta final"), "got: {}", out.content);
    assert_eq!(out.meta.source_node.as_deref(), Some("Node-B"));
    assert_eq!(out.meta.task_id.as_deref(), Some("hsmeta"));
    assert_eq!(out.meta.session_key.as_deref(), Some("hsess"));
    assert!(
        !out.meta.receipt,
        "终局回复 receipt=false（回合产物已落盘）"
    );
    assert_eq!(out.meta.occupies, None, "assistant 占行 occupies=None");
    assert_eq!(out.meta.model.as_deref(), Some("test-model"));
    assert_eq!(out.meta.note_kind, None);
    assert_eq!(out.meta.sync_via, None);
}

// ---------------------------------------------------------------------------
// ④ estop 在回调到达时冻结：零 LLM + ⛔ 注记 + 快照收口
// ---------------------------------------------------------------------------

#[tokio::test]
async fn estop_at_callback_freezes_continuation_no_llm() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(HandoffProvider::new(
            vec![hs_resp("must not run")],
            calls.clone(),
        )),
        hs_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let estop = std::sync::Arc::new(crate::estop::EstopState::new());
    agent_loop.set_estop(estop.clone());
    estop.trigger();
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "hsestop").await;
    agent_loop.set_continuation_manager(mgr.clone());

    let msg = hs_msg(
        &format!(
            "{}hsestop",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "task result text",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let calls = calls.lock().unwrap().clone();
    assert!(calls.is_empty(), "急停期间续行不得开 LLM 轮");
    let out = out_rx.recv().await.expect("estop note published");
    assert!(out.content.contains("⛔"), "got: {}", out.content);
    assert!(
        out.content.contains("estop --release"),
        "got: {}",
        out.content
    );
    // 快照收口（finish_handling）：不再有可续行条目。
    assert!(!mgr.has_continuation("hsestop").await);
}

// ---------------------------------------------------------------------------
// ⑤ busy Reject 臂：终局注记（含任务结果文本）+ 快照收口
// ---------------------------------------------------------------------------

#[tokio::test]
async fn busy_reject_mode_delivers_result_as_terminal_note() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    // 先 owned 装配（&mut 配置面），再 Arc 包装（泵后断言需要活句柄）。
    let mut loop_inner = AgentLoop::new_bus(
        Box::new(HandoffProvider::new(
            vec![hs_resp("must not run")],
            calls.clone(),
        )),
        hs_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "hsbusy").await;
    loop_inner.set_continuation_manager(mgr.clone());

    // 预占会话（另一回合在跑的模拟）。
    assert!(loop_inner.try_acquire_session("hsess"));
    let agent_loop = std::sync::Arc::new(loop_inner);

    let msg = hs_msg(
        &format!(
            "{}hsbusy",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "precious worker result",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.clone().run_bus_arc(in_rx).await;

    let calls = calls.lock().unwrap().clone();
    assert!(calls.is_empty(), "忙臂不得开 LLM 轮");
    let out = out_rx.recv().await.expect("busy note published");
    assert!(out.content.contains("会话正忙"), "got: {}", out.content);
    assert!(
        out.content.contains("hsbusy"),
        "注记必须带任务号: {}",
        out.content
    );
    assert!(
        out.content.contains("precious worker result"),
        "任务结果不得静默丢弃: {}",
        out.content
    );
    // 快照收口。
    assert!(!mgr.has_continuation("hsbusy").await);
    agent_loop.release_session("hsess");
}

// ---------------------------------------------------------------------------
// ⑥ busy Queue 臂：重投递入队（不消费快照、零出站）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn busy_queue_mode_redelivers_without_consuming() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    // 先 owned 装配（&mut 配置面），再 Arc 包装（泵后断言需要活句柄）。
    let mut loop_inner = AgentLoop::new_bus(
        Box::new(HandoffProvider::new(
            vec![hs_resp("must not run")],
            calls.clone(),
        )),
        hs_config(),
        out_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "hsqueue").await;
    loop_inner.set_continuation_manager(mgr.clone());

    assert!(loop_inner.try_acquire_session("hsess"));
    let agent_loop = std::sync::Arc::new(loop_inner);

    let msg = hs_msg(
        &format!(
            "{}hsqueue",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "queued result text",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.clone().run_bus_arc(in_rx).await;

    let calls = calls.lock().unwrap().clone();
    assert!(calls.is_empty(), "忙臂不得开 LLM 轮");
    assert_no_outbound(&mut out_rx).await;
    // 快照未被消费（结局未定，重投递是唯一出路）。
    assert!(mgr.has_continuation("hsqueue").await);
    // 重投递确实在队里（回合尾巴 drain 自然排空）。
    let status = agent_loop.inbox_status("hsess");
    assert_eq!(status.next_turn, 1, "重投递必须入 next-turn 队");
    agent_loop.release_session("hsess");
}

// ---------------------------------------------------------------------------
// ⑦ final_persisted 幂等闸：崩溃窗口后重投递跳过重复续行
// ---------------------------------------------------------------------------

#[tokio::test]
async fn final_persisted_redelivery_skips_rerun() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(HandoffProvider::new(
            vec![hs_resp("must not run")],
            calls.clone(),
        )),
        hs_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let dir = tempfile::tempdir().unwrap();
    let mgr = std::sync::Arc::new(
        crate::loop_continuation::ContinuationManager::with_disk_store(dir.path()),
    );
    seed_continuation(&mgr, "hsfinal").await;
    agent_loop.set_continuation_manager(mgr.clone());

    // 模拟「落库后、快照回收前」崩溃：盘上快照已打标 final_persisted。
    let store = mgr.disk_store.as_ref().expect("disk store present");
    let mut snapshot = store.load("hsfinal").expect("snapshot on disk");
    snapshot.final_persisted = true;
    store.save(&snapshot).expect("flag persist");

    let msg = hs_msg(
        &format!(
            "{}hsfinal",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "task result text",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let calls = calls.lock().unwrap().clone();
    assert!(calls.is_empty(), "已落库的重投递不得重跑 LLM");
    assert_no_outbound(&mut out_rx).await;
    // 幂等收尾：快照回收（无出站无回复，与旧路径 0b 臂一致）。
    assert!(!mgr.has_continuation("hsfinal").await);
}

// ---------------------------------------------------------------------------
// ⑧ 落库保真：store 会话历史含结果兜底轮 + assistant 终轮
// ---------------------------------------------------------------------------

#[tokio::test]
async fn continuation_persists_session_history_to_store() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(HandoffProvider::new(
            vec![hs_resp("persisted final")],
            calls.clone(),
        )),
        hs_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "hsfull").await;
    agent_loop.set_continuation_manager(mgr);
    // run_bus_owned 吃掉 loop 的所有权——store 句柄先取（同一 Arc）。
    let store = agent_loop
        .session_store
        .clone()
        .expect("bus mode wires in-memory session store");

    let msg = hs_msg(
        &format!(
            "{}hsfull",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "full result text",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx.recv().await.expect("continuation final published");
    assert!(out.content.contains("persisted final"));

    // store 会话（非空键快照必须落库）。快照回填 + 结果以 tool 轮在场
    // （assistant tool_calls → 紧随后补 tool 轮）→ 全量 set_history。
    let stored = store.get_or_create("hsess");
    let roles: Vec<&str> = stored.messages.iter().map(|m| m.role.as_str()).collect();
    assert!(
        roles.contains(&"user") && roles.contains(&"assistant") && roles.contains(&"tool"),
        "store 必须含快照回填 user/assistant + 结果 tool 轮 + assistant 终轮，实际 roles: {roles:?}"
    );
    let all_text = stored
        .messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        all_text.contains("full result text"),
        "结果文本必须进会话历史（tool 轮），实际: {all_text}"
    );
    assert!(
        all_text.contains("run the remote task"),
        "快照回填的原始任务文本必须进会话历史，实际: {all_text}"
    );
    let assistant_text = stored
        .messages
        .iter()
        .filter(|m| m.role == "assistant")
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        assistant_text.contains("persisted final"),
        "assistant 终轮必须落库，实际: {assistant_text}"
    );
}

// ---------------------------------------------------------------------------
// ⑨ 失败任务：错误文本（"Error: {error}"）进续行 LLM 上下文
// ---------------------------------------------------------------------------

#[tokio::test]
async fn failed_task_error_text_enters_llm_context() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(HandoffProvider::new(
            vec![hs_resp("ack failure")],
            calls.clone(),
        )),
        hs_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "hsfail").await;
    agent_loop.set_continuation_manager(mgr);

    let mut metadata = std::collections::HashMap::new();
    metadata.insert("status".to_string(), "error".to_string());
    metadata.insert("error".to_string(), "worker exploded".to_string());
    let msg = hs_msg(
        &format!(
            "{}hsfail",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "unused on failure",
        metadata,
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let calls = calls.lock().unwrap().clone();
    assert!(!calls.is_empty(), "续行 LLM 调用必须发生");
    let all: Vec<&LlmMessage> = calls.iter().flat_map(|c| c.messages.iter()).collect();
    assert!(
        all.iter()
            .any(|m| m.content.contains("Error: worker exploded")),
        "失败任务错误文本必须以 Error: 形态进上下文（旧语义同文）"
    );
    let out = out_rx.recv().await.expect("continuation final published");
    assert!(out.content.contains("ack failure"), "got: {}", out.content);
}

// ---------------------------------------------------------------------------
// ⑩ adversarial：续行回合高危工具调用走审批卡（dispatch 瀑布在续行生效）
// ---------------------------------------------------------------------------

/// 全关规则层 + default_action="ask" 的插件（cancel_linkage 同款）：未注册
/// 操作类型 fail-closed CRITICAL → RequireApproval → 审批管理器裁决。
fn hs_ask_plugin() -> std::sync::Arc<nemesis_security::pipeline::SecurityPlugin> {
    std::sync::Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig {
            enabled: true,
            command_guard_enabled: false,
            injection_enabled: false,
            credential_enabled: false,
            dlp_enabled: false,
            ssrf_enabled: false,
            default_action: "ask".to_string(),
            ..Default::default()
        },
    ))
}

/// 拒绝型审批管理器：记录每次审批请求（证明审批卡路径真实触发），裁决恒拒绝。
struct DenyingApprovalManager {
    asked: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

impl nemesis_security::auditor::ApprovalManager for DenyingApprovalManager {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        Ok(nemesis_security::auditor::ApprovalVerdict {
            approved: false,
            note: Some("fallback arm should not be hit".to_string()),
        })
    }
    fn request_approval_sync_ctx(
        &self,
        _request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
        _ctx: &nemesis_security::auditor::ApprovalContext,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        self.asked
            .lock()
            .unwrap()
            .push((format!("{operation}/{target}"), risk_level.to_string()));
        Ok(nemesis_security::auditor::ApprovalVerdict {
            approved: false,
            note: Some("sentinel denies".to_string()),
        })
    }
}

#[tokio::test]
async fn continuation_high_risk_tool_call_goes_through_approval() {
    let _logs = crate::test_support::capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(HandoffProvider::new(
            vec![
                // 模型在续行回合调用高危工具（未注册操作类型 → CRITICAL）。
                hs_tool_call_resp("evil_hs"),
                hs_resp("denied and reported"),
            ],
            calls.clone(),
        )),
        hs_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let (tool, executions) = HsTool::new();
    agent_loop.register_tool("evil_hs".to_string(), Box::new(tool));
    let plugin = hs_ask_plugin();
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    plugin
        .auditor()
        .set_approval_manager(std::sync::Arc::new(DenyingApprovalManager {
            asked: asked.clone(),
        }));
    agent_loop.set_security_plugin(plugin);
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    seed_continuation(&mgr, "hsrisk").await;
    agent_loop.set_continuation_manager(mgr);

    let msg = hs_msg(
        &format!(
            "{}hsrisk",
            nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX
        ),
        "task result text",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    // 审批卡路径真实触发（请求上卡：operation/target + 风险级）。
    let asked = asked.lock().unwrap().clone();
    assert!(
        !asked.is_empty(),
        "高危调用必须产生审批请求（审批卡路径），实际: {asked:?}"
    );
    assert!(
        asked
            .iter()
            .any(|(_, risk)| risk.eq_ignore_ascii_case("critical")),
        "未注册操作类型 fail-closed 必须以 CRITICAL 上卡，实际: {asked:?}"
    );
    // 裁决拒绝 → 工具不执行（旧裸 execute 无审批闸，此断言必红）。
    assert_eq!(*executions.lock().unwrap(), 0, "被拒绝的工具不得执行");
    // 拒绝以结构化文案回灌第二跳（模型可见，不静默）。
    let calls = calls.lock().unwrap().clone();
    assert!(
        calls.len() >= 2,
        "拒绝回灌后应有第二跳，实际 {} 跳",
        calls.len()
    );
    let second: Vec<&LlmMessage> = calls[1]
        .messages
        .iter()
        .filter(|m| m.role == "tool")
        .collect();
    assert!(
        second
            .iter()
            .any(|m| m.content.contains("SECURITY BLOCKED")),
        "审批拒绝必须以结构化文案回灌，实际: {:?}",
        second.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
    let out = out_rx.recv().await.expect("continuation final published");
    assert!(
        out.content.contains("denied and reported"),
        "got: {}",
        out.content
    );
}
