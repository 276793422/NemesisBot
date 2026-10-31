// R1（2026-09-21）：中间轮正文事件（AgentEvent::RoundText）的发布语义测试。
//
// 背景：LLM 循环里带工具调用的中间轮，其正文（模型的过程叙述）此前只进
// history——web 前端只见工具卡与最终回复。修复后中间轮正文非空即经
// agent_event_tx 广播 RoundText（web pump 默认路径投递 + 入环回放）。
//
// 批次二（2026-10-31）：RoundText 扩思考链字段（reasoning）——叙述与
// 思考皆空才不发；thinking-only 轮照发；终答轮思考链以 ("", reasoning)
// 形态发布（正文走 Done 既有通路）。
//
// 覆盖面：
// ① 两轮带叙述的中间轮各发布一条，内容与顺序一一对应；
// ② RoundText 走观察者通道（agent_event_tx），不混入 run() 返回的
//    chat 语义事件 Vec（ToolCall/Done 等的通道）；
// ③ 纯工具调用轮（叙述与思考皆空）不发布——无可观察输出；
// ④ 最终回复轮正文不发布（它是 Done，常规渲染路径）；
// ⑤ thinking-only 轮（叙述空、思考有值）照发布（批次二）；
// ⑥ 终答轮思考链以 ("", reasoning) 发布（批次二）。
//
// 自带迷你 Mock（兄弟模块私有类型不共享，f1/f8 同款）。

use super::*;

/// 脚本化响应的迷你 provider（按序弹出；弹尽后回退恒定终答防挂）。
struct ScriptedProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            return Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "fallback final".to_string(),
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

/// 迷你测试工具（执行成功返回固定文本）。
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("echo_ok".to_string())
    }
}

fn rt_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["echo".to_string()],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn resp(content: &str, tool_calls: Vec<ToolCallInfo>, finished: bool) -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: content.to_string(),
        tool_calls,
        finished,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 带思考链的响应（批次二）。
fn resp_r(
    content: &str,
    reasoning: Option<&str>,
    tool_calls: Vec<ToolCallInfo>,
    finished: bool,
) -> LlmResponse {
    LlmResponse {
        reasoning_content: reasoning.map(|s| s.to_string()),
        ..resp(content, tool_calls, finished)
    }
}

fn echo_call(id: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_string(),
        name: "echo".to_string(),
        arguments: r#"{"x":1}"#.to_string(),
    }
}

/// 收干 channel 里全部事件并挑出 RoundText（其他事件照单全收后丢弃——
/// 同通道上可能还有 hook 系事件，与本测试无关）。
fn drain_round_texts(
    rx: &mut tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
) -> Vec<nemesis_types::agent::AgentEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, nemesis_types::agent::AgentEvent::RoundText { .. }) {
            out.push(ev);
        }
    }
    out
}

#[tokio::test]
async fn intermediate_round_prose_published_as_round_text() {
    let provider = ScriptedProvider {
        responses: std::sync::Mutex::new(vec![
            resp(
                "第一步：先执行 echo 确认工具可用。",
                vec![echo_call("tc_1")],
                false,
            ),
            resp(
                "第二步：再执行一次，交叉验证。",
                vec![echo_call("tc_2")],
                false,
            ),
            resp("两步都完成了，最终结论：一切正常。", Vec::new(), true),
        ]),
    };
    let mut al = AgentLoop::new(Box::new(provider), rt_config());
    al.register_tool("echo".to_string(), Box::new(EchoTool));
    let (tx, mut rx) = tokio::sync::broadcast::channel(32);
    al.set_agent_event_tx(Some(tx));

    let instance = AgentInstance::new(rt_config());
    let context = RequestContext::new("web", "rtchat", "rtuser", "rtsession");
    let events = al.run(&instance, "跑两步验证", &context).await;

    let texts = drain_round_texts(&mut rx);
    assert_eq!(
        texts.len(),
        2,
        "两轮带叙述的中间轮各发一条，实际: {texts:?}"
    );
    match &texts[0] {
        nemesis_types::agent::AgentEvent::RoundText {
            session_key,
            chat_id,
            content,
            reasoning,
        } => {
            assert_eq!(content, "第一步：先执行 echo 确认工具可用。");
            assert_eq!(chat_id, "rtchat");
            assert!(!session_key.is_empty(), "session_key 供 web pump 路由");
            assert!(reasoning.is_empty(), "无思考模型 reasoning 恒空串");
        }
        other => panic!("必须是 RoundText，实际 {other:?}"),
    }
    match &texts[1] {
        nemesis_types::agent::AgentEvent::RoundText { content, .. } => {
            assert_eq!(content, "第二步：再执行一次，交叉验证。");
        }
        other => panic!("必须是 RoundText，实际 {other:?}"),
    }
    // 访问器对齐（f1 先例）——web pump 按 session_key()/chat_id() 路由。
    assert_eq!(texts[0].kind(), "RoundText");
    assert_eq!(texts[0].chat_id(), "rtchat");
    assert!(texts[0].session_key().is_some());

    // RoundText 只走观察者通道（agent_event_tx）；run() 返回的是本 crate
    // 的 chat 语义 AgentEvent（types.rs），两者类型不同域，天然隔离——
    // 这里只钉最终回复照常抵达。
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))));
}

#[tokio::test]
async fn empty_prose_round_not_published() {
    // 纯工具调用轮（content 空串、无思考）不发 RoundText——既有「只报
    // 工具卡」形态的回归钉；最终轮（无 tool_calls）走 Done，同样不发布。
    let provider = ScriptedProvider {
        responses: std::sync::Mutex::new(vec![
            resp("", vec![echo_call("tc_1")], false),
            resp("完成。", Vec::new(), true),
        ]),
    };
    let mut al = AgentLoop::new(Box::new(provider), rt_config());
    al.register_tool("echo".to_string(), Box::new(EchoTool));
    let (tx, mut rx) = tokio::sync::broadcast::channel(32);
    al.set_agent_event_tx(Some(tx));

    let instance = AgentInstance::new(rt_config());
    let context = RequestContext::new("web", "rtchat", "rtuser", "rtsession");
    let events = al.run(&instance, "纯工具轮", &context).await;

    let texts = drain_round_texts(&mut rx);
    assert!(
        texts.is_empty(),
        "皆空中间轮不得发布 RoundText，实际: {texts:?}"
    );
    assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCall(_))));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))));
}

#[tokio::test]
async fn thinking_only_round_published() {
    // 批次二：叙述空、思考有值的工具轮照发布（UI 折叠展示面）；
    // thinking-only RoundText 的 content 为空、reasoning 带值。
    let provider = ScriptedProvider {
        responses: std::sync::Mutex::new(vec![
            resp_r(
                "",
                Some("先想清楚改哪里再动手"),
                vec![echo_call("tc_1")],
                false,
            ),
            resp("改完了。", Vec::new(), true),
        ]),
    };
    let mut al = AgentLoop::new(Box::new(provider), rt_config());
    al.register_tool("echo".to_string(), Box::new(EchoTool));
    let (tx, mut rx) = tokio::sync::broadcast::channel(32);
    al.set_agent_event_tx(Some(tx));

    let instance = AgentInstance::new(rt_config());
    let context = RequestContext::new("web", "rtchat", "rtuser", "rtsession");
    let _ = al.run(&instance, "思考轮", &context).await;

    let texts = drain_round_texts(&mut rx);
    assert_eq!(texts.len(), 1, "thinking-only 轮必须发布，实际: {texts:?}");
    match &texts[0] {
        nemesis_types::agent::AgentEvent::RoundText {
            content, reasoning, ..
        } => {
            assert!(content.is_empty());
            assert_eq!(reasoning, "先想清楚改哪里再动手");
        }
        other => panic!("必须是 RoundText，实际 {other:?}"),
    }
}

#[tokio::test]
async fn final_answer_reasoning_published_as_empty_content_round() {
    // 批次二：终答轮思考链以 ("", reasoning) 发布——正文走 Done 通路，
    // 思考链单独成轮（直答轮也能看到思考）。
    let provider = ScriptedProvider {
        responses: std::sync::Mutex::new(vec![resp_r(
            "最终答案。",
            Some("直答前的推敲"),
            Vec::new(),
            true,
        )]),
    };
    let mut al = AgentLoop::new(Box::new(provider), rt_config());
    al.register_tool("echo".to_string(), Box::new(EchoTool));
    let (tx, mut rx) = tokio::sync::broadcast::channel(32);
    al.set_agent_event_tx(Some(tx));

    let instance = AgentInstance::new(rt_config());
    let context = RequestContext::new("web", "rtchat", "rtuser", "rtsession");
    let events = al.run(&instance, "直答带思考", &context).await;

    let texts = drain_round_texts(&mut rx);
    assert_eq!(texts.len(), 1, "终答思考链一条，实际: {texts:?}");
    match &texts[0] {
        nemesis_types::agent::AgentEvent::RoundText {
            content, reasoning, ..
        } => {
            assert!(content.is_empty(), "终答正文不重发（走 Done）");
            assert_eq!(reasoning, "直答前的推敲");
        }
        other => panic!("必须是 RoundText，实际 {other:?}"),
    }
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))));
}
