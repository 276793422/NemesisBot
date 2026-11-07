// 恢复环 tool_defs 供给闸回归锁。
//
// 契约：context 压缩环（4a）/ 429 限流环（4b）/ transient 退避环（4c）的
// 重试请求，其 tool_defs 必须与主调用同源（`effective_tool_defs`）——
// tier 白名单 / `agents.hidden_tools` / plan 白名单 / connector 总闸 /
// mcp lazy 等供给闸在恢复重试上同样生效。此前四处重建走
// `rebuild_full_tool_defs`（全量注册表无闸），重试请求把被隐藏/白名单外
// 工具全部发给 LLM：//「供给+dispatch 双闸」设计的供给半边在恢复
// 轮整体失效，且过滤激活场景 prompt cache 前缀断裂（自查 #6
// 修排序的动机本体——排序一致但内容不一致时前缀照样断）。
//
// 断言面：hidden 闸激活（`echo_*` 隐藏、`keep_tool` 保留）下，重试调用
// 收到的工具名单必须 == `build_tool_defs()` 名单（内容+序完全一致）。
// 凭据卡重试（第 4 处）与三环同函数同改法，broker/reloader 未装配时
// 提前回落无法直调，靠同构覆盖。
//
// 自带迷你 Mock（兄弟模块私有类型不共享，同 f8/l2 形态）。

use super::*;

/// 记录每次 chat 收到的工具名列表；脚本化应答（前 fail_times 次 Err，
/// 之后 Ok）。Arc 内核可 clone——同一实例句柄分别交给 AgentLoop 自持槽与
/// retry_* 的 active_provider 实参，记录共享可见。
#[derive(Clone)]
struct RecordingProvider(std::sync::Arc<RecShared>);

struct RecShared {
    fail_times: usize,
    err_text: String,
    seen_tools: std::sync::Mutex<Vec<Vec<String>>>,
    calls: std::sync::atomic::AtomicUsize,
}

impl RecordingProvider {
    fn new(fail_times: usize, err_text: &str) -> Self {
        Self(std::sync::Arc::new(RecShared {
            fail_times,
            err_text: err_text.to_string(),
            seen_tools: std::sync::Mutex::new(Vec::new()),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }))
    }

    fn tool_lists(&self) -> Vec<Vec<String>> {
        self.0.seen_tools.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let shared = &self.0;
        let n = shared
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        shared
            .seen_tools
            .lock()
            .unwrap()
            .push(tools.iter().map(|t| t.function.name.clone()).collect());
        if n < shared.fail_times {
            Err(shared.err_text.clone())
        } else {
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "Recovered!".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }
}

/// 迷你测试工具（同 f8 形态；注册名来自 register_tool 实参）。
struct KeepTool;

#[async_trait]
impl Tool for KeepTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("keep_ok".to_string())
    }
}

fn gate_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn def_names(defs: &[crate::types::ToolDefinition]) -> Vec<String> {
    defs.iter().map(|d| d.function.name.clone()).collect()
}

const HIDDEN_CFG: &str = r#"{"agents": {"hidden_tools": ["echo_*"]}}"#;

/// hidden 闸激活的 loop：echo_a/echo_b 被通配隐藏，keep_tool 保留。
/// 返回 loop、「主链路供给闸过滤后的期望名单」与 tempdir——**调用方必须
/// 持有 tempdir**：hidden 配置是 fresh-read 文件，TempDir 过早 drop 会删掉
/// config.json 使过滤静默失效。
fn gate_loop_with_hidden(
    provider: RecordingProvider,
) -> (AgentLoop, Vec<String>, tempfile::TempDir) {
    let mut agent_loop = AgentLoop::new(Box::new(provider), gate_config());
    agent_loop.register_tool("echo_b".to_string(), Box::new(KeepTool));
    agent_loop.register_tool("echo_a".to_string(), Box::new(KeepTool));
    agent_loop.register_tool("keep_tool".to_string(), Box::new(KeepTool));
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(&cfg_path, HIDDEN_CFG).unwrap();
    agent_loop.set_config_path(cfg_path);
    let expected = def_names(&agent_loop.build_tool_defs());
    (agent_loop, expected, tmp)
}

// ---------------------------------------------------------------------------
// 4c transient 退避环
// ---------------------------------------------------------------------------

#[tokio::test]
async fn transient_retry_respects_supply_gate() {
    let provider = RecordingProvider::new(1, "upstream deadline exceeded");
    let (agent_loop, expected, _tmp) = gate_loop_with_hidden(provider.clone());
    assert_eq!(
        expected,
        vec!["keep_tool".to_string()],
        "前置自检：hidden 闸必须滤掉 echo_* 只剩 keep_tool"
    );

    let instance = AgentInstance::new(gate_config());
    let context = RequestContext::new("web", "chat1", "user1", "agent:main:session:rgtrans");
    let opts = crate::types::ChatOptions::default();

    // 首败 <1s → backoff_eligible=false → 零退避，测试毫秒级。
    let (maybe_resp, last_err) = agent_loop
        .retry_transient(
            &instance,
            &context,
            &(std::sync::Arc::new(provider.clone()) as std::sync::Arc<dyn LlmProvider>),
            "test-model",
            &opts,
            std::time::Duration::from_millis(500),
            "upstream deadline exceeded".to_string(),
        )
        .await;

    assert!(maybe_resp.is_some(), "1 败 + 1 成功必须恢复: {last_err}");
    let lists = provider.tool_lists();
    assert_eq!(lists.len(), 2, "1 首捕 + 1 重试: {lists:?}");
    assert_eq!(
        lists[1], expected,
        "transient 重试请求的工具面必须与主链路供给闸一致（BUG-20261102-306）: {:?}",
        lists[1]
    );
}

// ---------------------------------------------------------------------------
// 4b 429 限流环（start_paused 自动推进梯子 sleep，零真实等待）
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn rate_limit_retry_respects_supply_gate() {
    let provider = RecordingProvider::new(1, "rate limited by provider p/m (retry_after=5)");
    let (agent_loop, expected, _tmp) = gate_loop_with_hidden(provider.clone());
    assert_eq!(expected, vec!["keep_tool".to_string()]);

    let instance = AgentInstance::new(gate_config());
    let context = RequestContext::new("web", "chat1", "user1", "agent:main:session:rgrl");

    let (maybe_resp, last_err) = agent_loop
        .retry_rate_limited(
            &context,
            &instance,
            &(std::sync::Arc::new(provider.clone()) as std::sync::Arc<dyn LlmProvider>),
            "test-model",
            &crate::types::ChatOptions::default(),
            "rate limited by provider p/m (retry_after=5)".to_string(),
        )
        .await;

    assert!(maybe_resp.is_some(), "1 败 + 1 成功必须恢复: {last_err}");
    let lists = provider.tool_lists();
    assert_eq!(lists.len(), 2, "1 首捕 + 1 重试: {lists:?}");
    assert_eq!(
        lists[1], expected,
        "429 重试请求的工具面必须与主链路供给闸一致（BUG-20261102-306）: {:?}",
        lists[1]
    );
}

// ---------------------------------------------------------------------------
// 4a context 压缩环（双 provider：自持服务摘要，active 实参服务重呼）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_retry_respects_supply_gate() {
    // 自持 provider（摘要 with_one_retry 恒失败，不推进 covers）；
    // active provider（重呼恒失败 → 环 2 轮耗尽）记录全部重呼工具面。
    let summarizer_llm = RecordingProvider::new(usize::MAX, "summary failed");
    let recall_llm = RecordingProvider::new(usize::MAX, "ctx length exceeded: simulated");
    let (agent_loop, expected, _tmp) = gate_loop_with_hidden(summarizer_llm.clone());
    assert_eq!(expected, vec!["keep_tool".to_string()]);

    let instance = AgentInstance::new(gate_config());
    for i in 0..3 {
        instance.add_user_message(&format!("msg_{i}"));
    }
    let context = RequestContext::new("web", "chat1", "user1", "agent:main:session:rgctx");

    let result = agent_loop
        .retry_after_context_error(
            &instance,
            &context,
            "trace-rg",
            "test-model",
            &crate::types::ChatOptions::default(),
            false,
            false,
            0,
            std::time::Instant::now(),
            &(std::sync::Arc::new(recall_llm.clone()) as std::sync::Arc<dyn LlmProvider>),
            "ctx length exceeded: simulated".to_string(),
        )
        .await;

    assert!(result.is_err(), "摘要与重呼全失败必须诚实 Err（有界耗尽）");
    let lists = recall_llm.tool_lists();
    assert_eq!(lists.len(), 2, "2 轮重呼: {lists:?}");
    for (round, names) in lists.iter().enumerate() {
        assert_eq!(
            names,
            &expected,
            "context 压缩第 {} 轮重呼的工具面必须与主链路供给闸一致（BUG-20261102-306）: {:?}",
            round + 1,
            names
        );
    }
}
