// P30（WS14）：canvas 块终答检出 → CanvasOpen 事件 / 语法错误回灌自纠 /
// 回灌预算耗尽诚实放行 的轮级集成测试。
//
// 覆盖面：
// ① 终答含合法 ```canvas 块 → agent_event_tx 广播 CanvasOpen（html/index/
//    session_key/chat_id 访问器对齐），Done 照常抵达；
// ② 终答 canvas 有语法错误 → 不发 CanvasOpen，回灌反馈落 user 消息
//    （provider 下一次调用可见），模型重发自纠后发 CanvasOpen；
// ③ 连续错误烧穿回灌预算（MAX_SYNTAX_RETRIES）→ 诚实放行：不发
//    CanvasOpen、Done 照常、canvas 块以普通代码块留在正文；
// ④ json 数据岛（type="application/json"）不算语法错误，照常发布；
// ⑤ ```html 普通围栏不是 canvas，不触发任何事件。
//
// 自带迷你 Mock（round_text_tests 同款形态，兄弟模块私有类型不共享）。

use super::*;

/// 脚本化响应的迷你 provider（按序弹出；弹尽后回退恒定终答防挂）。
/// 记录每次调用收到的 messages，供回灌落盘断言。内部 Arc 才能便宜 Clone
/// （AgentLoop 只吃 Box<dyn LlmProvider>，测试末尾还要用原件内省记录）。
#[derive(Clone)]
struct ScriptedProvider {
    responses: std::sync::Arc<std::sync::Mutex<Vec<LlmResponse>>>,
    calls: std::sync::Arc<std::sync::Mutex<Vec<Vec<LlmMessage>>>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Arc::new(std::sync::Mutex::new(responses)),
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// 全部调用中出现过的 user 消息正文（含回灌反馈）。
    fn all_user_contents(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .flat_map(|ms| {
                ms.iter()
                    .filter(|m| m.role == "user")
                    .map(|m| m.content.clone())
            })
            .collect()
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.lock().unwrap().push(messages);
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            return Ok(LlmResponse {
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

/// 构造脚本 provider（Clone 进 Box，原件留测试内省调用记录）。
fn scripted(responses: Vec<LlmResponse>) -> ScriptedProvider {
    ScriptedProvider::new(responses)
}

fn canvas_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 8,
        tools: Vec::new(),
        models: std::collections::HashMap::new(),
    }
}

fn resp(content: &str) -> LlmResponse {
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

/// 收干 channel 里全部事件并挑出 CanvasOpen。
fn drain_canvas_opens(
    rx: &mut tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
) -> Vec<nemesis_types::agent::AgentEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, nemesis_types::agent::AgentEvent::CanvasOpen { .. }) {
            out.push(ev);
        }
    }
    out
}

const VALID_CANVAS: &str = "看板如下：\n```canvas\n<html><body><script>const x = (1 + 2); document.body.textContent = 'ok';</script></body></html>\n```\n完。";

#[tokio::test]
async fn valid_canvas_final_answer_publishes_canvas_open() {
    let provider = scripted(vec![resp(VALID_CANVAS)]);
    let al = AgentLoop::new(Box::new(provider.clone()), canvas_config());
    let (tx, mut rx) = tokio::sync::broadcast::channel(32);
    al.set_agent_event_tx(Some(tx));

    let instance = AgentInstance::new(canvas_config());
    let context = RequestContext::new("web", "cvchat", "cvuser", "cvsession");
    let events = al.run(&instance, "画个看板", &context).await;

    let opens = drain_canvas_opens(&mut rx);
    assert_eq!(
        opens.len(),
        1,
        "合法 canvas 块发布一条 CanvasOpen: {opens:?}"
    );
    match &opens[0] {
        nemesis_types::agent::AgentEvent::CanvasOpen {
            session_key,
            chat_id,
            html,
            index,
        } => {
            assert!(html.contains("document.body.textContent"), "html 原文透传");
            assert_eq!(*index, 0);
            assert_eq!(chat_id, "cvchat");
            assert!(!session_key.is_empty(), "session_key 供 web pump 路由");
        }
        other => panic!("必须是 CanvasOpen，实际 {other:?}"),
    }
    // 访问器对齐（web pump 按 session_key()/chat_id() 路由）。
    assert_eq!(opens[0].kind(), "CanvasOpen");
    assert_eq!(opens[0].chat_id(), "cvchat");
    assert!(opens[0].session_key().is_some());

    // Done 照常抵达（canvas 不改终答语义）。
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))));
}

#[tokio::test]
async fn bad_canvas_feedback_retried_then_fixed_opens_canvas() {
    // 第一答：括号不平衡（预检抓）；第二答：修复后的合法块。
    let bad = "看板：\n```canvas\n<script>\nconst a = (1 + 2;\n</script>\n```";
    let good = "看板：\n```canvas\n<script>\nconst a = (1 + 2);\n</script>\n```";
    let provider = scripted(vec![resp(bad), resp(good)]);
    let al = AgentLoop::new(Box::new(provider.clone()), canvas_config());
    let (tx, mut rx) = tokio::sync::broadcast::channel(32);
    al.set_agent_event_tx(Some(tx));

    let instance = AgentInstance::new(canvas_config());
    let context = RequestContext::new("web", "cvchat2", "cvuser", "cvsession");
    let events = al.run(&instance, "画个看板", &context).await;

    // 第一答不得发布；修复后发布一条。
    let opens = drain_canvas_opens(&mut rx);
    assert_eq!(opens.len(), 1, "只发修复后的那块: {opens:?}");
    match &opens[0] {
        nemesis_types::agent::AgentEvent::CanvasOpen { html, index, .. } => {
            assert!(html.contains("(1 + 2)"), "发布的是修复后内容: {html}");
            assert_eq!(*index, 0);
        }
        other => panic!("必须是 CanvasOpen，实际 {other:?}"),
    }
    // 回灌反馈确实落进了下一轮 user 消息（模型可见、可自纠）。
    let user_contents = provider.all_user_contents();
    assert!(
        user_contents
            .iter()
            .any(|c| c.contains("[canvas 预检]") && c.contains("括号")),
        "回灌反馈必须作为 user 消息可见: {user_contents:?}"
    );
    // 两次 provider 调用（原答 + 自纠重发），Done 照常。
    assert_eq!(provider.call_count(), 2);
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))));
}

#[tokio::test]
async fn precheck_budget_exhausted_passes_through_without_canvas_open() {
    // 连续 MAX_SYNTAX_RETRIES + 1 次坏块：2 轮回灌后诚实放行。
    let bad = "```canvas\n<script>\nconst a = (1;\n</script>\n```";
    let provider = scripted(vec![resp(bad), resp(bad), resp(bad)]);
    let al = AgentLoop::new(Box::new(provider.clone()), canvas_config());
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    al.set_agent_event_tx(Some(tx));

    let instance = AgentInstance::new(canvas_config());
    let context = RequestContext::new("web", "cvchat3", "cvuser", "cvsession");
    let events = al.run(&instance, "画个看板", &context).await;

    let opens = drain_canvas_opens(&mut rx);
    assert!(
        opens.is_empty(),
        "预算耗尽后诚实放行，不发 CanvasOpen: {opens:?}"
    );
    // 原答 + 2 轮回灌 = 3 次调用；Done 照常（canvas 留在正文当普通代码块）。
    assert_eq!(provider.call_count(), 3);
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))));
    // 放行的终答正文仍是原内容（不吞块）。
    if let Some(AgentEvent::Done(text)) = events.iter().find(|e| matches!(e, AgentEvent::Done(_))) {
        assert!(
            text.contains("```canvas"),
            "canvas 块按普通代码块保留: {text}"
        );
    }
}

#[tokio::test]
async fn json_data_island_canvas_is_valid_and_emitted() {
    let content = "```canvas\n<html><body>\n<script type=\"application/json\">{\"x\": (坏括号也不检}</script>\n<script>document.title = 'ok';</script>\n</body></html>\n```";
    let provider = scripted(vec![resp(content)]);
    let al = AgentLoop::new(Box::new(provider.clone()), canvas_config());
    let (tx, mut rx) = tokio::sync::broadcast::channel(32);
    al.set_agent_event_tx(Some(tx));

    let instance = AgentInstance::new(canvas_config());
    let context = RequestContext::new("web", "cvchat4", "cvuser", "cvsession");
    let events = al.run(&instance, "画个看板", &context).await;

    let opens = drain_canvas_opens(&mut rx);
    assert_eq!(
        opens.len(),
        1,
        "数据岛不参与预检，整块合法即发布: {opens:?}"
    );
    if let nemesis_types::agent::AgentEvent::CanvasOpen { html, .. } = &opens[0] {
        assert!(
            html.contains("{\"x\": (坏括号也不检}"),
            "数据岛原文必须完整透传（前端 srcdoc 契约）: {html}"
        );
    }
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))));
}

#[tokio::test]
async fn html_fence_is_not_canvas_and_publishes_nothing() {
    let provider = scripted(vec![resp("上图：\n```html\n<p>x</p>\n```\n完。")]);
    let al = AgentLoop::new(Box::new(provider.clone()), canvas_config());
    let (tx, mut rx) = tokio::sync::broadcast::channel(32);
    al.set_agent_event_tx(Some(tx));

    let instance = AgentInstance::new(canvas_config());
    let context = RequestContext::new("web", "cvchat5", "cvuser", "cvsession");
    let events = al.run(&instance, "普通图", &context).await;

    let opens = drain_canvas_opens(&mut rx);
    assert!(opens.is_empty(), "```html 不是 canvas: {opens:?}");
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))));
    assert_eq!(provider.call_count(), 1, "无预检问题，不多烧轮次");
}
