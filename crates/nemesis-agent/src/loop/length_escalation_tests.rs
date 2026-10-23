//! §6.2 翻倍阶梯（length_escalation）行为测试。
//!
//! - 烧穿判定：`length` + 空正文 +（① 无工具调用 = 纯思考烧穿；② 工具
//!   参数全为空对象/空串/非法 JSON 且 schema 需要参数 = 工具参数截断，
//!   BUG 2026-10-23 第四形态）才翻倍（非空截断 / 带合法参数的工具调用
//!   形态不翻倍，既有 continue-generation 接管）；
//! - 阶梯跳变：8192 起点撞顶直接跳 32K 地板；32768 起点顺 ×2；
//! - 封顶收口：翻到 ceiling（默认 262144）仍空 = 停止重发、响应带
//!   `escalated_to`（空输出 guard 文案的事实值）；
//! - 正常响应零额外请求；
//! - per-model 学习缓存：第二次调用首请求即从 floor 起步。
//!
//! 独立测试文件（生产文件只保留声明行，仓库 2026-08-25 纪律）；mock
//! provider 与 config 构造自建（`loop/tests.rs` 的同名辅助模块私有，跨
//! 文件不可复用，与 cache_warmer_tests 同先例）。standalone AgentLoop
//! （无 config_path）→ 封顶走默认 262144，estop 未装配 → 永不 engaged。

use super::*;
use async_trait::async_trait;
use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// 本地辅助：config 构造 + 脚本型 mock provider
// ---------------------------------------------------------------------------

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn opts(max_tokens: u32) -> Option<crate::types::ChatOptions> {
    Some(crate::types::ChatOptions {
        max_tokens: Some(max_tokens),
        ..Default::default()
    })
}

/// 空输出撞顶响应（思考烧穿形态：length + 空正文 + 零工具调用）。
fn burned_response() -> LlmResponse {
    LlmResponse {
        content: String::new(),
        tool_calls: Vec::new(),
        finished: false,
        reasoning_content: None,
        finish_reason: Some("length".to_string()),
        escalated_to: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 正常完成响应。
fn ok_response(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        finish_reason: Some("stop".to_string()),
        escalated_to: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

#[derive(Clone)]
struct RecordedCall {
    max_tokens: Option<u32>,
}

/// 脚本型 provider：按序弹出脚本响应（耗尽后回落 `ok("final")`），记录每次
/// 收到的 max_tokens。记录/脚本句柄经 Arc 外置（provider 被 Box 进
/// AgentLoop，测试侧留同一份句柄读取与续写）。
struct ScriptedProvider {
    calls: std::sync::Arc<std::sync::Mutex<Vec<RecordedCall>>>,
    script: std::sync::Arc<std::sync::Mutex<VecDeque<LlmResponse>>>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.lock().unwrap().push(RecordedCall {
            max_tokens: options.as_ref().and_then(|o| o.max_tokens),
        });
        let resp = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| ok_response("final"));
        Ok(resp)
    }
}

fn scripted_pair(
    script: Vec<LlmResponse>,
) -> (
    Arc<AgentLoop>,
    std::sync::Arc<std::sync::Mutex<Vec<RecordedCall>>>,
    std::sync::Arc<std::sync::Mutex<VecDeque<LlmResponse>>>,
) {
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let script = std::sync::Arc::new(std::sync::Mutex::new(VecDeque::from(script)));
    let loop_ = Arc::new(AgentLoop::new(
        Box::new(ScriptedProvider {
            calls: calls.clone(),
            script: script.clone(),
        }),
        test_config(),
    ));
    (loop_, calls, script)
}

fn max_tokens_seq(calls: &std::sync::Mutex<Vec<RecordedCall>>) -> Vec<Option<u32>> {
    calls.lock().unwrap().iter().map(|c| c.max_tokens).collect()
}

/// 统一调用形态：provider 快照先落绑定（读守卫不跨 await，clippy 纪律）
/// 再走翻倍阶梯包装；messages/tools 空载即可（mock 不消费）。
async fn call_escalating(
    loop_: &Arc<AgentLoop>,
    model: &str,
    options: Option<crate::types::ChatOptions>,
) -> Result<LlmResponse, String> {
    let provider = loop_.provider.read().clone();
    loop_
        .chat_call_escalating(0, &provider, model, Vec::new(), options, Vec::new())
        .await
}

#[tokio::test]
async fn burned_empty_output_escalates_to_floor() {
    // 8192 起点烧穿 → 翻倍阶梯地板 32768（不浪费 16384 一轮）；32768 正常
    // 返回 → 收口，escalated_to 记录事实值。
    let (loop_, calls, _script) = scripted_pair(vec![burned_response()]);
    let resp = call_escalating(&loop_, "test-model", opts(8192))
        .await
        .expect("call should succeed");
    assert_eq!(resp.content, "final");
    assert_eq!(resp.escalated_to, Some(32_768));
    assert_eq!(max_tokens_seq(&calls), vec![Some(8192), Some(32_768)]);
}

#[tokio::test]
async fn non_empty_truncation_does_not_escalate() {
    // 非空截断（length + 有正文）= 既有 continue-generation 的领域，翻倍
    // 阶梯不接管（零额外请求）。
    let (loop_, calls, _script) = scripted_pair(vec![LlmResponse {
        content: "partial output".to_string(),
        tool_calls: Vec::new(),
        finished: false,
        reasoning_content: None,
        finish_reason: Some("length".to_string()),
        escalated_to: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let resp = call_escalating(&loop_, "test-model", opts(8192))
        .await
        .expect("call should succeed");
    assert_eq!(resp.content, "partial output");
    assert_eq!(resp.escalated_to, None);
    assert_eq!(max_tokens_seq(&calls), vec![Some(8192)]);
}

#[tokio::test]
async fn unknown_tool_schema_empty_args_conservatively_burned() {
    // BUG 2026-10-23 第四形态改写（原 `tool_call_length_does_not_escalate`
    // 的「参数可能被截断但交给下游」假设已被事故证伪——multiedit 大参数
    // 零进度循环到停轮，见 docs/BUG/2026-10-23_max-tokens-8192截断撞顶复
    // 发.md）：length + 空正文 + 参数 `{}` 且 schema 未知 → 保守按截断烧
    // 穿翻倍。误升级代价 = 一次更高 cap 重发收敛；漏判代价 = 停轮。
    let (loop_, calls, _script) = scripted_pair(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc1".to_string(),
                name: "exec".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            finish_reason: Some("length".to_string()),
            escalated_to: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        ok_response("done"),
    ]);
    let resp = call_escalating(&loop_, "test-model", opts(8192))
        .await
        .expect("call should succeed");
    assert_eq!(resp.content, "done");
    assert_eq!(max_tokens_seq(&calls), vec![Some(8192), Some(32_768)]);
}

#[tokio::test]
async fn normal_response_zero_extra_requests() {
    // 正常 stop 响应：一次调用收口，无翻倍。
    let (loop_, calls, _script) = scripted_pair(vec![]);
    let resp = call_escalating(&loop_, "test-model", opts(8192))
        .await
        .expect("call should succeed");
    assert_eq!(resp.escalated_to, None);
    assert_eq!(max_tokens_seq(&calls), vec![Some(8192)]);
}

#[tokio::test]
async fn escalation_stops_at_ceiling_with_escalated_to() {
    // 8192 起点一路烧到封顶：8192→32768→65536→131072→262144（重发），
    // 下一步 524288 > 262144 封顶 → 停止重发（最后一次仍烧穿也收口）。
    // 共 5 次请求，escalated_to = 最后一次实际生效值。
    let (loop_, calls, _script) = scripted_pair(vec![
        burned_response(),
        burned_response(),
        burned_response(),
        burned_response(),
        burned_response(),
    ]);
    let resp = call_escalating(&loop_, "test-model", opts(8192))
        .await
        .expect("call should succeed");
    assert_eq!(
        max_tokens_seq(&calls),
        vec![
            Some(8192),
            Some(32_768),
            Some(65_536),
            Some(131_072),
            Some(262_144),
        ]
    );
    assert_eq!(resp.escalated_to, Some(262_144));
}

#[tokio::test]
async fn floor_start_doubles_smoothly() {
    // 32768 起点（三机 GLM 条目现状）：×2 顺滑衔接不跳地板。
    let (loop_, calls, _script) = scripted_pair(vec![burned_response()]);
    let resp = call_escalating(&loop_, "test-model", opts(32_768))
        .await
        .expect("call should succeed");
    assert_eq!(max_tokens_seq(&calls), vec![Some(32_768), Some(65_536)]);
    assert_eq!(resp.escalated_to, Some(65_536));
}

#[tokio::test]
async fn learned_floor_skips_burn_round_on_next_call() {
    // 学习缓存：第一次调用撞顶学到的 floor，第二次调用首请求直接从
    // 32768 起步（config 8192 陈旧值被 floor 覆盖），不再烧撞顶轮。
    let (loop_, calls, _script) = scripted_pair(vec![burned_response()]);
    call_escalating(&loop_, "test-model", opts(8192))
        .await
        .expect("first call should succeed");
    // 第二次调用：脚本耗尽 → provider 回落 ok("final")。
    call_escalating(&loop_, "test-model", opts(8192))
        .await
        .expect("second call should succeed");
    assert_eq!(
        max_tokens_seq(&calls),
        vec![Some(8192), Some(32_768), Some(32_768)]
    );
}

#[tokio::test]
async fn learned_floor_is_per_model() {
    // 学习缓存按模型隔离：test-model 学到的 floor 不泄漏到其它模型。
    let (loop_, calls, _script) = scripted_pair(vec![burned_response()]);
    call_escalating(&loop_, "test-model", opts(8192))
        .await
        .expect("first call should succeed");
    call_escalating(&loop_, "other-model", opts(8192))
        .await
        .expect("second call should succeed");
    assert_eq!(
        max_tokens_seq(&calls),
        vec![Some(8192), Some(32_768), Some(8192)]
    );
}

#[tokio::test]
async fn learned_floor_respects_larger_config_value() {
    // config 值 ≥ floor 时尊重 config（学习不覆盖用户显式调大的配置）。
    let (loop_, _calls, _script) = scripted_pair(vec![burned_response()]);
    call_escalating(&loop_, "test-model", opts(8192))
        .await
        .expect("first call should succeed");
    assert_eq!(loop_.escalated_floor_for_model("test-model"), 32_768);
    // 直接断言访问器语义：floor 单调不降、clear 清空。
    loop_.note_escalated_floor("test-model", 65_536);
    assert_eq!(loop_.escalated_floor_for_model("test-model"), 65_536);
    loop_.note_escalated_floor("test-model", 40_960);
    assert_eq!(
        loop_.escalated_floor_for_model("test-model"),
        65_536,
        "floor 单调不降：更小值不回退"
    );
    loop_.clear_escalated_floors();
    assert_eq!(loop_.escalated_floor_for_model("test-model"), 0);
}

#[tokio::test]
async fn options_none_never_escalates() {
    // options None（provider 默认 cap，起点未知）：不翻倍——无从推断阶梯。
    let (loop_, calls, _script) = scripted_pair(vec![burned_response()]);
    let resp = call_escalating(&loop_, "test-model", None)
        .await
        .expect("call should succeed");
    assert_eq!(resp.escalated_to, None);
    assert_eq!(max_tokens_seq(&calls), vec![None]);
}

// ---------------------------------------------------------------------------
// BUG 2026-10-23 第四形态专项：工具参数截断（schema 已知形态）
// ---------------------------------------------------------------------------

/// 工具定义（按 props/required 构造 parameters schema）。
fn tool_def(name: &str, props: &[&str], required: &[&str]) -> crate::types::ToolDefinition {
    let properties: serde_json::Map<String, serde_json::Value> = props
        .iter()
        .map(|p| (p.to_string(), serde_json::json!({"type": "string"})))
        .collect();
    crate::types::ToolDefinition {
        tool_type: "function".to_string(),
        function: crate::types::ToolFunctionDef {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": properties,
                "required": required,
            }),
        },
    }
}

/// length + 空正文 + 单工具调用（可注入 args）。
fn args_response(args: &str) -> LlmResponse {
    LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc1".to_string(),
            name: "multiedit".to_string(),
            arguments: args.to_string(),
        }],
        finished: false,
        reasoning_content: None,
        finish_reason: Some("length".to_string()),
        escalated_to: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 带 tools 供给的阶梯调用（第四形态判定依赖 schema）。
async fn call_escalating_with_tools(
    loop_: &Arc<AgentLoop>,
    model: &str,
    options: Option<crate::types::ChatOptions>,
    tools: Vec<crate::types::ToolDefinition>,
) -> Result<LlmResponse, String> {
    let provider = loop_.provider.read().clone();
    loop_
        .chat_call_escalating(0, &provider, model, Vec::new(), options, tools)
        .await
}

#[tokio::test]
async fn truncated_empty_object_args_escalate() {
    // 事故形态重放（anthropic lane）：截断 input 被归一化成 `{}`，schema
    // 需要参数 → 判截断烧穿，8192 → 32K 地板重发，第二次正常收口。
    let (loop_, calls, _script) = scripted_pair(vec![args_response("{}"), ok_response("done")]);
    let resp = call_escalating_with_tools(
        &loop_,
        "test-model",
        opts(8192),
        vec![tool_def("multiedit", &["path", "edits"], &["path"])],
    )
    .await
    .expect("call should succeed");
    assert_eq!(resp.content, "done");
    assert_eq!(max_tokens_seq(&calls), vec![Some(8192), Some(32_768)]);
}

#[tokio::test]
async fn invalid_json_args_escalate() {
    // 事故形态重放（openai 兼容 lane）：截断 args 是非法 JSON 原文
    // （2026-08-10 报告 EOF 指纹）→ 同样翻倍。
    let (loop_, calls, _script) = scripted_pair(vec![
        args_response("{\"edits\":[{\"old_string\""),
        ok_response("done"),
    ]);
    call_escalating_with_tools(
        &loop_,
        "test-model",
        opts(8192),
        vec![tool_def("multiedit", &["edits"], &["edits"])],
    )
    .await
    .expect("call should succeed");
    assert_eq!(max_tokens_seq(&calls), vec![Some(8192), Some(32_768)]);
}

#[tokio::test]
async fn no_arg_tool_empty_args_not_burned() {
    // 无参工具（schema 无 properties/required）的 `{}` 是合法调用 → 不翻倍
    // （交下游：hit_cap 空正文守卫照常兜底）。注意工具名须与 args_response
    // 的调用名一致（schema 按名查）。
    let (loop_, calls, _script) = scripted_pair(vec![args_response("{}")]);
    let resp = call_escalating_with_tools(
        &loop_,
        "test-model",
        opts(8192),
        vec![tool_def("multiedit", &[], &[])],
    )
    .await
    .expect("call should succeed");
    assert_eq!(resp.escalated_to, None);
    assert_eq!(max_tokens_seq(&calls), vec![Some(8192)]);
}

#[tokio::test]
async fn valid_args_not_burned_continuation_takes_over() {
    // 带合法参数的工具调用 = 可执行进度 → 不翻倍（既有 continue-generation
    // 接管，分工边界保持）。
    let (loop_, calls, _script) = scripted_pair(vec![args_response(r#"{"edits":[]}"#)]);
    let resp = call_escalating_with_tools(
        &loop_,
        "test-model",
        opts(8192),
        vec![tool_def("multiedit", &["edits"], &["edits"])],
    )
    .await
    .expect("call should succeed");
    assert_eq!(resp.escalated_to, None);
    assert_eq!(max_tokens_seq(&calls), vec![Some(8192)]);
    assert_eq!(resp.finish_reason.as_deref(), Some("length"));
}

#[tokio::test]
async fn mixed_valid_and_truncated_args_not_burned() {
    // 一合法一截断混合 → 不判烧穿（存在可执行进度，翻倍重发反而丢弃它）。
    let mut resp = args_response("{}");
    resp.tool_calls.push(ToolCallInfo {
        id: "tc2".to_string(),
        name: "multiedit".to_string(),
        arguments: r#"{"edits":[]}"#.to_string(),
    });
    let (loop_, calls, _script) = scripted_pair(vec![resp]);
    call_escalating_with_tools(
        &loop_,
        "test-model",
        opts(8192),
        vec![tool_def("multiedit", &["edits"], &["edits"])],
    )
    .await
    .expect("call should succeed");
    assert_eq!(max_tokens_seq(&calls), vec![Some(8192)]);
}
