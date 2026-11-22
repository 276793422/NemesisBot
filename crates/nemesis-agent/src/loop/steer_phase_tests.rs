//! 插话专轮（steer phase，2026-11-21 用户拍板件）行为测试。
//!
//! 修正模型（四轮挑战定稿）：键入时刻只决定排队顺序；轮边界（一轮本地
//! 操作完成后、返回之前）是唯一 claim 点；已下发的工具批次恒执行。claim
//! 非空批次且非首轮 → 置位插话专轮：contract 注入 → LLM 专轮处理插话
//! （可用全量工具）→ 纯文本 = 收口（中间回应即时出站，不落终答）→ 排空
//! 队列 → 注入续跑提醒 → 外层续跑。
//!
//! 覆盖：claim 原子性（阶段进行中不认领后续插话，收口才排空）、两段专轮
//! 首尾衔接、专轮文本轮预算豁免（max_turns=3 下豁免与否决定终答能否
//! 产出）、首轮 claim 不置位（新 turn 输入语义保持）。
//!
//! 白盒抓手：ScriptProvider 逐调用记录请求消息内容——contract / 插话文本
//! / RESUME 提醒的出现窗口是阶段生命周期的直接证据。

use super::*;

fn test_config_max_turns(max_turns: u32) -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 脚本化 provider + 逐调用请求录制（contract / 插话 / RESUME 出现窗口的
/// 白盒证据）。脚本耗尽回退 "No more responses"（finished=true）——与
/// tests.rs 的 MockLlmProvider 同兜底语义。
struct ScriptProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    request_log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}
impl ScriptProvider {
    fn new(
        responses: Vec<LlmResponse>,
        request_log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
            request_log,
        }
    }
}
#[async_trait]
impl LlmProvider for ScriptProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.request_log.lock().unwrap().push(
            messages
                .iter()
                .map(|m| format!("{}:{}", m.role, m.content))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            return Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "No more responses".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            });
        }
        Ok(responses.remove(0))
    }
}

/// 纯文本脚本轮（finished=true、无工具调用）。
fn resp_text(content: &str) -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 工具调用脚本轮（无叙述正文）。
fn resp_tool(id: &str, tool: &str) -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: id.to_string(),
            name: tool.to_string(),
            arguments: "{}".to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 执行时把构造期定死的 `!` 插话内容排进 inbox（真实链路里 gate 的 ⚡ 臂
/// 在 busy 时进队；这里由脚本工具轮代行——让插话恰好在指定轮到达）。
struct SteerEnqueueTool {
    inbox: crate::inbox::SharedInbox,
    session_key: String,
    steer_content: String,
}

#[async_trait]
impl Tool for SteerEnqueueTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        self.inbox.enqueue(
            &self.session_key,
            crate::inbox::QueuedMessage {
                msg: nemesis_types::channel::InboundMessage::user_ingress(
                    "web".to_string(),
                    "tester".to_string(),
                    "chat1".to_string(),
                    self.steer_content.clone(),
                )
                .with_session_key(self.session_key.clone())
                .with_metadata_map(Default::default()),
                timestamp: String::new(),
                priority: crate::inbox::MessagePriority::Normal,
            },
        );
        Ok("STEER_ENQUEUED".to_string())
    }
}

fn uniq_key(tag: &str) -> String {
    format!(
        "agent:test:session:steer_phase_{}_{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

fn make_inbound(content: &str, session_key: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage::user_ingress(
        "web".to_string(),
        "user1".to_string(),
        "chat1".to_string(),
        content.to_string(),
    )
    .with_session_key(session_key.to_string())
}

/// 全链主用例：原子性 + 排空 + 两段专轮 + 预算豁免 + 续跑。
///
/// 时序（max_turns=3）：
/// - R1 工具轮：工具 A 入队插话 A（turns_used=1）
/// - R2 专轮 A 置位（contract + 插话 A 进上下文）；再调工具 B 入队插话 B
///   （专轮内工具照常执行、工具轮照常计数 → turns_used=2）
/// - R3 专轮 A 第二轮：claim 短路（插话 B 留队列——原子性），纯文本
///   「回应A」收口（预算豁免 → turns_used 仍 2）；队列有 B → 排空
///   Continue（不注 RESUME）
/// - R4 专轮 B 置位（新 contract）；纯文本「回应B」收口（豁免 → 仍 2）；
///   队列空 → 注入 RESUME
/// - R5 续跑轮：turns_used(2) < 3 → 主线收尾终答 → Done
///
/// 预算豁免是本用例的判别器：无豁免则 R3 收口计满（=3），R4 顶 cap 消耗
/// grace，R5 硬停——终答永远出不来。帧序 [inject_A, response_A, inject_B,
/// response_B] 同时是 claim 短路的黑盒证据（无短路则 B 在 R3 被并入 A 段，
/// inject_B 抢在 response_A 之前）。请求录制提供直接证据：R3 请求无插话
/// B 文本（短路成立）、R4 请求带新 contract + 插话 B、R5 请求带 RESUME
/// 且无 contract。
#[tokio::test]
async fn steer_phase_atomic_drain_budget_exempt_and_resume() {
    let key = uniq_key("full");
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(32);
    let request_log: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let provider = ScriptProvider::new(
        vec![
            resp_tool("t1", "steer_enqueue_a"),
            resp_tool("t2", "steer_enqueue_b"),
            resp_text("回应A"),
            resp_text("回应B"),
            resp_text("主线收尾"),
        ],
        request_log.clone(),
    );
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config_max_turns(3),
        outbound_tx,
        ConcurrentMode::Steer,
        8,
        0,
    );
    agent_loop.register_tool(
        "steer_enqueue_a".to_string(),
        Box::new(SteerEnqueueTool {
            inbox: agent_loop.inbox.clone(),
            session_key: key.clone(),
            steer_content: "! 补充边界说明".to_string(),
        }),
    );
    agent_loop.register_tool(
        "steer_enqueue_b".to_string(),
        Box::new(SteerEnqueueTool {
            inbox: agent_loop.inbox.clone(),
            session_key: key.clone(),
            steer_content: "! 换个思路".to_string(),
        }),
    );

    let msg = make_inbound("干第一轮活", &key);
    let (_id, resp, err, _) = agent_loop.process_inbound_message(&msg).await;
    assert!(err.is_none(), "turn must not error: {err:?}");
    assert_eq!(resp, "主线收尾", "终答 = 续跑轮的主线文本");

    // ① note_kind 帧序：claim(A) → 收口(A) → claim(B) → 收口(B)。B 的
    // claim 帧排在 A 的收口帧之后 = 阶段进行中 B 未被认领（原子性黑盒）。
    let mut note_seq: Vec<(String, String)> = Vec::new();
    while let Ok(frame) = outbound_rx.try_recv() {
        if let Some(kind) = frame.meta.note_kind.as_deref()
            && (kind == "steer_inject" || kind == "steer_response")
        {
            note_seq.push((kind.to_string(), frame.content.clone()));
        }
    }
    assert_eq!(note_seq.len(), 4, "两段专轮各一 claim 一收口: {note_seq:?}");
    assert!(
        note_seq[0].0 == "steer_inject" && note_seq[0].1.contains("补充边界说明"),
        "帧1 = 插话 A 的 claim 注记: {note_seq:?}"
    );
    assert!(
        note_seq[1].0 == "steer_response" && note_seq[1].1 == "回应A",
        "帧2 = A 段收口回应: {note_seq:?}"
    );
    assert!(
        note_seq[2].0 == "steer_inject" && note_seq[2].1.contains("换个思路"),
        "帧3 = 插话 B 的 claim 注记（排在 A 收口之后 = 排队到收口才认领）: {note_seq:?}"
    );
    assert!(
        note_seq[3].0 == "steer_response" && note_seq[3].1 == "回应B",
        "帧4 = B 段收口回应: {note_seq:?}"
    );

    // ② 请求录制：contract / 插话文本 / RESUME 的出现窗口。
    let calls = request_log.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        5,
        "五轮 LLM 调用（工具×2 + A段文本 + B段文本 + 续跑）: {}",
        calls.len()
    );
    let contract = "[E2E_STEER_CONTRACT]";
    let resume = "[E2E_STEER_RESUME]";
    assert!(
        !calls[0].contains(contract) && !calls[0].contains("补充边界说明"),
        "R1 无 contract、无插话（首轮工具轮）"
    );
    assert!(
        calls[1].contains(contract) && calls[1].contains("补充边界说明"),
        "R2 = 专轮 A：contract + 插话 A 在请求里"
    );
    assert!(!calls[1].contains("换个思路"), "R2 不含插话 B（尚未入队）");
    assert!(
        calls[2].contains(contract),
        "R3 仍是专轮 A（contract 每轮重建）"
    );
    assert!(
        !calls[2].contains("换个思路"),
        "原子性核心证据：R3 请求不含插话 B——阶段进行中 claim 短路，B 留队列"
    );
    assert!(
        calls[3].contains(contract) && calls[3].contains("换个思路"),
        "R4 = 专轮 B：新 contract + 排空认领的插话 B"
    );
    assert!(
        calls[4].contains(resume) && !calls[4].contains(contract),
        "R5 = 续跑轮：RESUME 提醒在场、contract 已消失"
    );
}

/// 首轮 claim 不置位：turn 开始前已滞留的插话在首轮顶被认领 = 新 turn
/// 输入的一部分（存量语义）——无 contract、无收口帧、单轮直答终局。
#[tokio::test]
async fn steer_phase_first_round_claim_does_not_arm() {
    let key = uniq_key("first");
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let request_log: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let provider = ScriptProvider::new(vec![resp_text("直接完成")], request_log.clone());
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config_max_turns(5),
        outbound_tx,
        ConcurrentMode::Steer,
        8,
        0,
    );
    // turn 开始前预置插话（gate busy 臂的真实形态 here 由测试直塞 inbox）。
    agent_loop.inbox.enqueue(
        &key,
        crate::inbox::QueuedMessage {
            msg: make_inbound("! 预置插话", &key),
            timestamp: String::new(),
            priority: crate::inbox::MessagePriority::Normal,
        },
    );

    let msg = make_inbound("干第一轮活", &key);
    let (_id, resp, err, _) = agent_loop.process_inbound_message(&msg).await;
    assert!(err.is_none(), "turn must not error: {err:?}");
    assert_eq!(resp, "直接完成", "首轮 claim = 正常输入，直答终局");

    // 无 contract（未置位）；插话文本在首轮请求里（并入输入）。
    let calls = request_log.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "单轮直答");
    assert!(
        !calls[0].contains("[E2E_STEER_CONTRACT]"),
        "首轮 claim 不置位：无 contract"
    );
    assert!(calls[0].contains("预置插话"), "插话并入首轮输入");

    // claim 仍发注记帧（注记不依赖置位——插话落点可见性恒在）。
    let mut saw_inject = false;
    let mut saw_response = false;
    while let Ok(frame) = outbound_rx.try_recv() {
        match frame.meta.note_kind.as_deref() {
            Some("steer_inject") => saw_inject = true,
            Some("steer_response") => saw_response = true,
            _ => {}
        }
    }
    assert!(saw_inject, "首轮 claim 照发注记帧");
    assert!(!saw_response, "无专轮 = 无收口帧");
}

/// 恒拒终答裁决策略（BUG-20261122-002 回归注入）：对任何候选终答恒返
/// RetryWithNudge——模拟策略槽（WASM 对接面）的病态/缺陷实现。
struct AlwaysRetryPolicy;

#[async_trait::async_trait]
impl crate::r#loop::policy::FinalJudgePolicy for AlwaysRetryPolicy {
    async fn judge(
        &self,
        _input: &crate::r#loop::policy::FinalJudgeInput,
    ) -> crate::r#loop::policy::FinalJudgeVerdict {
        crate::r#loop::policy::FinalJudgeVerdict::RetryWithNudge(
            "policy 恒拒（BUG-20261122-002 回归注入）".to_string(),
        )
    }
}

/// BUG-20261122-002 回归：策略槽恒返 RetryWithNudge 时，steer 阶段内
/// 纯文本轮不得无限豁免。
///
/// 修复前路径：恒拒 → 纯文本轮不进 Accept 臂（闩不清）→ 落进预算豁免
///（旧条件只看 `tool_calls.is_empty()`）→ turns_used 冻结 → max_turns
/// 闸永不触发 → 无限 LLM 轮（builtin 裁决空内容有 empty_final_count
/// 预算不受影响；本面专由自定义 policy 触发）。
///
/// 修复后：豁免带独立预算 MAX_STEER_EXEMPT_ROUNDS(8)，耗尽后轮恢复计入
/// → max_turns 闸兜底终局（grace 一轮后暂停文案收轮）。
///
/// 时序推演（max_turns=3）：R1 工具轮计数（=1）+入队插话；R2 专轮置位，
/// 纯文本被恒拒——豁免 1..8（R2..R9）；R10 预算尽轮计入（=2）；R11 计入
/// （=3）；R12 顶闸命中 → grace → R12 计入（=4）；R13 顶闸 grace 已耗
/// → 暂停终局。LLM 调用总数 = 12（ScriptProvider 供给 2 个脚本响应后由
/// 「No more responses」兜底臂接管，兜底臂同为纯文本照样推进）。
#[tokio::test]
async fn steer_phase_exempt_budget_bounds_policy_always_retry_loop() {
    let key = uniq_key("exemptcap");
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(32);
    let request_log: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let provider = ScriptProvider::new(
        vec![resp_tool("t1", "steer_enqueue_a"), resp_text("尝试回应")],
        request_log.clone(),
    );
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config_max_turns(3),
        outbound_tx,
        ConcurrentMode::Steer,
        8,
        0,
    );
    agent_loop.register_tool(
        "steer_enqueue_a".to_string(),
        Box::new(SteerEnqueueTool {
            inbox: agent_loop.inbox.clone(),
            session_key: key.clone(),
            steer_content: "! 补充边界说明".to_string(),
        }),
    );
    agent_loop.set_final_judge_policy(std::sync::Arc::new(AlwaysRetryPolicy));

    let msg = make_inbound("干第一轮活", &key);
    let (_id, resp, err, _) = agent_loop.process_inbound_message(&msg).await;
    // 有界终局：不无限循环，预算暂停文案收轮。
    assert!(err.is_none(), "turn must not error: {err:?}");
    assert!(
        resp.contains("轮工具调用后暂停"),
        "豁免预算耗尽后必须由 max_turns 闸兜底终局: {resp}"
    );

    // 判别器：LLM 调用总数 = 12（时序推演见函数注释）。修复前此循环
    // 无界（测试挂起/超时）；任何让轮数偏离 12 的实现变化都应触发重审。
    let calls = request_log.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        12,
        "豁免预算(8)+计入轮(2)+grace(1)+工具轮(1) = 12 次调用"
    );

    // 辅证：恒拒下闩从未清 → 全程无 steer_response 收口帧。
    let mut saw_response = false;
    while let Ok(frame) = outbound_rx.try_recv() {
        if frame.meta.note_kind.as_deref() == Some("steer_response") {
            saw_response = true;
        }
    }
    assert!(!saw_response, "恒拒策略下专轮永不收口，无收口帧");
}
