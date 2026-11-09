//! effort 400 恢复面（拍板 2026-11-09 A/B）回归锁：
//!
//! - **B 剥键回落**（`retry_effort_stripped`）：400 且带非空非 off effort
//!   → 剥掉 effort 键单发重试。成功 = `Ok(Some)` + 探针标 stale（UI 显
//!   「探针已失效，请重探」→ 重探走 C 自动修正闭环，不往 LLM 回复掺注
//!   记）；失败 = `Ok(None)` 落回终局（原 400 文案 + A 增强建议）；恰好
//!   一发调用，不循环。
//! - **A 读核**（`effort_probe_accepted_at`）：accepted 数组读取的 None
//!   与空数组判别（None = 无记录无建议；空 = ignore 宽容端点改口文案）。

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AO};

fn effort_test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn strip_ok_response() -> LlmResponse {
    LlmResponse {
        content: "stripped-ok".to_string(),
        tool_calls: vec![],
        finished: true,
        reasoning_content: None,
        finish_reason: Some("stop".to_string()),
        escalated_to: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 剥键回落 provider：捕获「重试调用是否真的剥掉了 effort 键」；
/// `fail_all=true` 模拟回落也撞 400（验 Ok(None) 单发语义）。
struct StripCapture {
    calls: std::sync::Arc<AtomicUsize>,
    saw_stripped: std::sync::Arc<AtomicBool>,
    fail_all: bool,
}

#[async_trait]
impl LlmProvider for StripCapture {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.fetch_add(1, AO::SeqCst);
        if options
            .as_ref()
            .is_some_and(|o| o.reasoning_effort.is_none())
        {
            self.saw_stripped.store(true, AO::SeqCst);
        }
        if self.fail_all {
            Err("status 400: reasoning_effort value 'xhigh' not supported".to_string())
        } else {
            Ok(strip_ok_response())
        }
    }
}

fn strip_test_loop(provider: StripCapture) -> std::sync::Arc<AgentLoop> {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(provider),
        effort_test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ))
}

fn strip_ctx() -> (AgentInstance, RequestContext) {
    let instance = AgentInstance::new(effort_test_config());
    let context = RequestContext::new("web", "ef-chat", "ef-sender", "agent:main:session:efstrip");
    (instance, context)
}

fn strip_opts() -> crate::types::ChatOptions {
    let mut o = crate::types::ChatOptions::default();
    o.reasoning_effort = Some("xhigh".to_string());
    o
}

/// 回落成功：Ok(Some) + 重试调用确实剥掉了 effort 键 + 探针标 stale
/// （B 的「不静默」面——stale 让 UI 显「探针已失效，请重探」）。
#[tokio::test]
async fn strip_retry_success_returns_resp_and_marks_stale() {
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let saw_stripped = std::sync::Arc::new(AtomicBool::new(false));
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = dir.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::json!({
            "model_list": [
                {"model": "t/test-model", "model_name": "test-model",
                 "reasoning_effort": "xhigh",
                 "effort_probe": {"accepted": ["low", "high", "max"], "rejected": {}, "control": "strict"}}
            ]
        })
        .to_string(),
    )
    .unwrap();
    let agent_loop = strip_test_loop(StripCapture {
        calls: calls.clone(),
        saw_stripped: saw_stripped.clone(),
        fail_all: false,
    });
    agent_loop.set_config_path(cfg_path.clone());
    let (instance, context) = strip_ctx();
    let cancel = tokio_util::sync::CancellationToken::new();

    let provider = agent_loop.provider.read().clone();
    let result = agent_loop
        .retry_effort_stripped(
            &instance,
            &context,
            &provider,
            "test-model",
            &strip_opts(),
            &cancel,
            None,
        )
        .await;

    let resp = result
        .expect("回落成功不得 Err")
        .expect("回落成功必须 Some");
    assert_eq!(resp.content, "stripped-ok");
    assert!(saw_stripped.load(AO::SeqCst), "回落请求必须剥掉 effort 键");
    assert_eq!(calls.load(AO::SeqCst), 1, "剥键回落是单发重试，不循环");
    // stale 落盘（保鲜层③同钩）：读回 config 断言。
    let raw = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        cfg["model_list"][0]["effort_probe"]["stale"],
        serde_json::json!(true),
        "回落成功必须标探针 stale（UI 提示重探）"
    );
}

/// 回落也撞 400：Ok(None) 落回终局路径（原 400 文案 + A 增强），恰好
/// 一发不循环。
#[tokio::test]
async fn strip_retry_always_400_falls_back_to_none() {
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let saw_stripped = std::sync::Arc::new(AtomicBool::new(false));
    let agent_loop = strip_test_loop(StripCapture {
        calls: calls.clone(),
        saw_stripped,
        fail_all: true,
    });
    let (instance, context) = strip_ctx();
    let cancel = tokio_util::sync::CancellationToken::new();

    let provider = agent_loop.provider.read().clone();
    let result = agent_loop
        .retry_effort_stripped(
            &instance,
            &context,
            &provider,
            "test-model",
            &strip_opts(),
            &cancel,
            None,
        )
        .await;

    assert!(result.is_ok());
    assert!(result.unwrap().is_none(), "回落仍失败必须 None 落终局");
    assert_eq!(
        calls.load(AO::SeqCst),
        1,
        "回落失败不循环（再撞 400 说明问题不止档位键）"
    );
}

/// 已急停 loop：环顶闸，零调用，Err(Done(estop))。
#[tokio::test]
async fn strip_retry_engaged_estop_gate() {
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let agent_loop = strip_test_loop(StripCapture {
        calls: calls.clone(),
        saw_stripped: std::sync::Arc::new(AtomicBool::new(false)),
        fail_all: false,
    });
    let estop = std::sync::Arc::new(crate::estop::EstopState::new());
    agent_loop.set_estop(estop.clone());
    estop.trigger();
    let (instance, context) = strip_ctx();
    let cancel = tokio_util::sync::CancellationToken::new();

    let provider = agent_loop.provider.read().clone();
    let result = agent_loop
        .retry_effort_stripped(
            &instance,
            &context,
            &provider,
            "test-model",
            &strip_opts(),
            &cancel,
            None,
        )
        .await;

    match result {
        Err(AgentEvent::Done(m)) => {
            assert_eq!(m, recovery::ESTOP_DONE_MSG, "急停终局文案与首呼臂一致");
        }
        other => panic!("期望 Err(Done(estop))，得到 {other:?}"),
    }
    assert_eq!(calls.load(AO::SeqCst), 0, "已急停 loop 零调用");
}

/// A 读核：有记录读 accepted；无记录 None；ignore 端点 accepted=[] 为
/// `Some(vec![])`（与 None 判别——建议文案据此改口）。
#[test]
fn effort_probe_accepted_at_reader_semantics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.json");

    // 缺文件 → None。
    assert!(config_watch::effort_probe_accepted_at(&path, "m").is_none());

    let cfg = serde_json::json!({
        "model_list": [
            {"model_name": "m-strict", "effort_probe": {"accepted": ["low", "max"], "control": "strict"}},
            {"model_name": "m-ignore", "effort_probe": {"accepted": [], "control": "ignore"}},
            {"model_name": "m-bare"}
        ]
    });
    std::fs::write(&path, cfg.to_string()).unwrap();

    assert_eq!(
        config_watch::effort_probe_accepted_at(&path, "m-strict"),
        Some(vec!["low".to_string(), "max".to_string()])
    );
    assert_eq!(
        config_watch::effort_probe_accepted_at(&path, "m-ignore"),
        Some(vec![]),
        "空数组 ≠ None：宽容端点建议改口「档位实际不生效」"
    );
    assert!(
        config_watch::effort_probe_accepted_at(&path, "m-bare").is_none(),
        "无 effort_probe 记录 = 无建议可附"
    );
    assert!(
        config_watch::effort_probe_accepted_at(&path, "m-ghost").is_none(),
        "精确命中失败（不扫推断臂）= None"
    );
}
