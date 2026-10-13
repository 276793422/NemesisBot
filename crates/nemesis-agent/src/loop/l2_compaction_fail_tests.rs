// L2（2026-10-12 内部专项 T0）：压缩失败诚实报错语义的回归锁。
//
// pi 真实事故（996K/1M 上下文）的教训：压缩（摘要生成）失败必须响亮失败
// ——不能静默继续（用户以为带着全量上下文，实际早已失忆）。本仓 2026-08-25
// 已有「摘要失败绝不推进 covers」契约（compact.rs warn 臂），本文件钉住
// 最后一环：`retry_after_context_error` 的**有界耗尽臂**——context 错误触
// 发的压缩重试环里，摘要 LLM 与重呼 LLM 全部失败时：
//   ① 重试预算有界（2 轮 × 摘要+重呼，不多烧）；
//   ② 返回 Err(AgentEvent::Error) 携带原始错误（不静默、不假装成功）；
//   ③ 错误落 history（assistant "Error: ..." 行，可观测/可审计）；
//   ④ 失败的摘要不推进 covers_up_to（上下文不静默丢失）；
// 另补一臂：第 1 轮重呼成功即恢复（有界环的正向用途）。
//
// 自带脚本 mock（同一共享内核双把手：AgentLoop 自持 provider 服务摘要
// 调用，active_provider 实参服务重呼），不与 s9/tests 共享私有类型。

use super::*;

/// 共享内核：按序消费脚本化应答（FIFO）；脚本耗尽 → Err fallback；计数全部调用。
#[derive(Default)]
struct Shared {
    script: std::sync::Mutex<Vec<Result<LlmResponse, String>>>,
    calls: std::sync::atomic::AtomicUsize,
}

impl Shared {
    fn pop(&self) -> Result<LlmResponse, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut script = self.script.lock().unwrap();
        if script.is_empty() {
            Err("script exhausted".to_string())
        } else {
            script.remove(0)
        }
    }
}

/// 指向共享内核的把手（可实现 LlmProvider；clone 共享同一脚本与计数——
/// AgentLoop 自持 provider 服务摘要调用，active_provider 实参服务重呼，
/// 两者进同一条脚本）。
#[derive(Clone)]
struct ScriptedLlm(std::sync::Arc<Shared>);

impl ScriptedLlm {
    fn new() -> Self {
        Self(std::sync::Arc::new(Shared::default()))
    }
    /// 按消费顺序追加脚本。
    fn queue_ok(&self, content: &str) {
        self.0.script.lock().unwrap().push(Ok(LlmResponse {
            content: content.to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            finish_reason: None,
            escalated_to: None,
            raw_request_body: None,
            raw_response_body: None,
        }));
    }
    fn queue_err(&self, msg: &str) {
        self.0.script.lock().unwrap().push(Err(msg.to_string()));
    }
    fn call_count(&self) -> usize {
        self.0.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmProvider for ScriptedLlm {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.0.pop()
    }
}

fn l2_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: Vec::new(),
        models: std::collections::HashMap::new(),
    }
}

fn l2_instance() -> AgentInstance {
    // system_prompt Some → history[0] 是 system turn；+10 条 user → 11 条。
    let instance = AgentInstance::new(l2_config());
    for i in 0..10 {
        instance.add_user_message(&format!("msg_{i}"));
    }
    instance
}

const CTX_ERR: &str = "ctx length exceeded: simulated";

// ---------------------------------------------------------------------------
// 耗尽臂：摘要与重呼全失败 → 有界（4 次调用）+ 诚实 Err + history 落账 +
// covers 不推进。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_retry_exhausts_bounded_then_fails_loud() {
    let _logs = crate::test_support::capture_logs();
    let llm = ScriptedLlm::new();
    let agent_loop = AgentLoop::new(Box::new(llm.clone()), l2_config());
    let instance = l2_instance();
    let before_covers = instance
        .get_summary_cache()
        .as_ref()
        .map(|c| c.covers_up_to);

    // 2 轮 × (摘要 + 重呼) 全失败——脚本耗尽后的 Err fallback 恰好兜底，
    // 不需要预填脚本。
    let result = agent_loop
        .retry_after_context_error(
            &instance,
            &RequestContext::new("web", "chat1", "user1", "agent:main:session:l2x"),
            "trace-l2",
            "test-model",
            &crate::types::ChatOptions::default(),
            false,
            false,
            0,
            std::time::Instant::now(),
            &(Arc::new(llm.clone()) as Arc<dyn LlmProvider>),
            CTX_ERR.to_string(),
        )
        .await;

    // ① 有界：每轮 = 摘要（with_one_retry：Err 首试+重试 = 2 次）+ 重呼 1 次
    //    = 3；× 2 轮 = 6。预算有界，不烧第 3 轮（防无限重试环回归）。
    assert_eq!(
        llm.call_count(),
        6,
        "重试预算必须恰好 2 轮 × (摘要双试 + 重呼)"
    );

    // ② 诚实报错：Err 携带最后一次重试的错误原文（耗尽臂语义 = 报最终
    //    错误，不静默、不假装成功）。"script exhausted" = mock 的模拟
    //    provider 错误（脚本耗尽 fallback，确定性）。
    let AgentEvent::Error(formatted) = result.expect_err("耗尽必须响亮失败") else {
        panic!("期望 AgentEvent::Error");
    };
    assert!(
        formatted.contains("Error: ") && formatted.contains("script exhausted"),
        "错误文案必须携带最终错误原文：{formatted}"
    );

    // ③ history 落账：最后一条 assistant 消息 = Error: ...（可观测/可审计，
    //    不静默吞掉本轮）。
    let history = instance.get_history();
    let last = history.last().expect("history 非空");
    assert_eq!(last.role, "assistant", "落账角色必须是 assistant");
    assert!(
        last.content.contains("Error: ") && last.content.contains("script exhausted"),
        "history 落账必须携带最终错误：{}",
        last.content
    );

    // ④ 失败的摘要绝不推进 covers（上下文不静默丢失——2026-08-25 契约在
    //    context 重试环内同样成立）。
    let after_covers = instance
        .get_summary_cache()
        .as_ref()
        .map(|c| c.covers_up_to);
    assert_eq!(
        before_covers, after_covers,
        "摘要全失败时 covers_up_to 不得推进"
    );
    assert!(instance.get_summary_cache().is_none());
}

// ---------------------------------------------------------------------------
// 恢复臂：第 1 轮摘要失败但重呼成功 → Ok 收口（有界环的正向用途；错误是
// 瞬态时不升级为终局失败）。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_retry_recovers_when_recall_succeeds() {
    let _logs = crate::test_support::capture_logs();
    let llm = ScriptedLlm::new();
    let agent_loop = AgentLoop::new(Box::new(llm.clone()), l2_config());
    let instance = l2_instance();

    // 调用序：① force_compression 摘要（Err + with_one_retry 重试 Err——
    // covers 不推进）；② 同轮重呼成功 → break。queue 序列 = 消费顺序。
    llm.queue_err("summarizer transient down");
    llm.queue_err("summarizer retry still down");
    llm.queue_ok("recovered after compression");

    let result = agent_loop
        .retry_after_context_error(
            &instance,
            &RequestContext::new("web", "chat1", "user1", "agent:main:session:l2r"),
            "trace-l2r",
            "test-model",
            &crate::types::ChatOptions::default(),
            false,
            false,
            0,
            std::time::Instant::now(),
            &(Arc::new(llm.clone()) as Arc<dyn LlmProvider>),
            CTX_ERR.to_string(),
        )
        .await;

    let resp = result.expect("重呼成功必须返回 Ok");
    assert_eq!(resp.content, "recovered after compression");
    assert_eq!(
        llm.call_count(),
        3,
        "摘要双试(2) + 重呼(1) = 3；第 1 轮恢复即 break，不烧第 2 轮"
    );

    // 成功路径不落 Error 行（history 末尾仍是原 user 消息）；摘要失败同样
    // 不推进 covers（本例摘要两试全败，恢复来自瞬态错误消失后的重呼）。
    let history = instance.get_history();
    let last = history.last().expect("history 非空");
    assert_eq!(last.role, "user", "成功路径不得写 Error 落账");
    assert!(instance.get_summary_cache().is_none());
}
