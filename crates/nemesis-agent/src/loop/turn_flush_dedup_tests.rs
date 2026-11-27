//! turn.rs 异常收尾 chat_log 落盘回归测试。
//!
//! 错误收尾轮的中间正文唯一持久化面 = 终局 assistant 行的 `work` 键
//! （work_timeline.rs，`work.rounds[].text`）。曾经的「异常收尾冲刷中间
//! 正文为独立 jsonl 行」（对话链路三缺陷 BUG-2，d083e51ed 2026-10-23）
//! 已于 2026-11-28 移除（BUG-20261128-012）：它与 work 键对同一正文双
//! 持久化，切窗重载（WS/SSE 重连 → 全量 loadHistory）后前端如实渲染
//! jsonl → 同一内容双重渲染（独立 NB 气泡 + 时间线思考卡各一份）。
//!
//! 本文件钉死两条不变量：
//! 1. 错误收尾轮 jsonl 恒「恰好 +2 行」（1 user + 1 assistant 终局行），
//!    终局错误只由主路径落一行（裸 history 形态不得经任何旁路重复落盘）；
//! 2. 中间正文不得作为独立行出现——只允许活在终局行的 work 键里。

use super::*;

/// Auth 失败 provider：错误串直配分类器 Auth 词表（"unauthorized"）→
/// `finish_llm_error_no_retry` 终局——history 落 `Error: {raw}`，用户文案 =
/// `Error: 认证失败…（原始错误：{raw}）`（两形态任何通道都不等值）。
struct AuthFailProvider;

#[async_trait]
impl LlmProvider for AuthFailProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("request failed: unauthorized".to_string())
    }
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 错误终局轮：chat_log 的终局错误恰好一行（主路径落盘），裸错误
/// history 形态不得经任何旁路重复落盘。
#[tokio::test]
async fn error_ending_flush_does_not_duplicate_terminal_error_row() {
    let al = AgentLoop::new(Box::new(AuthFailProvider), test_config());
    let key = "agent:main:session:flushdup";
    crate::chat_log::delete_chat_log(key);

    let msg = nemesis_types::channel::InboundMessage::user_ingress(
        "web".to_string(),
        "flushuser".to_string(),
        "flushchat".to_string(),
        "hi".to_string(),
    )
    .with_session_key(key.to_string());
    let (_, resp, err, _) = al.process_inbound_message(&msg).await;
    assert!(
        err.is_some(),
        "auth 失败必须以 Error 终局: err={err:?}, resp={resp}"
    );

    let (rows, _, _, _) = crate::chat_log::read_chat_log(key, 200, None);
    // jsonl 恒「恰好 +2 行」（1 user + 1 assistant 终局行）。
    assert_eq!(rows.len(), 2, "错误收尾轮 jsonl 必须恰好两行: {rows:?}");
    // 终局错误行（改写形态，含「原始错误」）恰好一行——主路径落盘。
    let final_rows = rows
        .iter()
        .filter(|r| {
            r["role"] == "assistant" && r["content"].as_str().unwrap_or("").contains("原始错误")
        })
        .count();
    assert_eq!(final_rows, 1, "终局错误行必须恰好一行（主路径落盘）");
    // 裸错误 history 形态（"Error: {raw}"）不得经任何旁路再落一行。
    let raw_rows = rows
        .iter()
        .filter(|r| {
            r["role"] == "assistant"
                && r["content"].as_str().unwrap_or("") == "Error: request failed: unauthorized"
        })
        .count();
    assert_eq!(
        raw_rows, 0,
        "终局错误不得经任何旁路重复落盘（裸形态行数应为 0）"
    );

    crate::chat_log::delete_chat_log(key);
}

// ---------------------------------------------------------------------------
// 回归锁（BUG-20261128-012）：错误收尾轮带中间正文 → jsonl 恒恰好两行，
// 中间正文只活在终局行的 work 键里（独立行必须为零）。
// ---------------------------------------------------------------------------

/// 第一轮：带过程叙述 + 工具调用（正常推进）；第二轮：auth 失败直落
/// Error 终局——复刻真机形态「模型干了几步活，然后失败收尾」。
struct NarrateThenFailProvider(std::sync::atomic::AtomicUsize);

#[async_trait]
impl LlmProvider for NarrateThenFailProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "FLUSH_MARKER_中间叙述正文".to_string(),
                tool_calls: vec![crate::types::ToolCallInfo {
                    id: "t1".to_string(),
                    name: "flush_probe_tool".to_string(),
                    arguments: "{}".to_string(),
                }],
                finished: false,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Err("request failed: unauthorized".to_string())
        }
    }
}

/// 探针工具：成败无所谓——任何结果都照常回灌，循环推进到失败终局轮。
struct FlushProbeTool;

#[async_trait]
impl Tool for FlushProbeTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("ran".to_string())
    }
}

/// 带中间正文的错误收尾轮：jsonl 恰好两行、中间正文零独立行、
/// 中间正文随终局行 work 键落盘（唯一持久化面）。
#[tokio::test]
async fn error_ending_interim_narration_lives_only_in_work_key() {
    let mut al = AgentLoop::new(
        Box::new(NarrateThenFailProvider(
            std::sync::atomic::AtomicUsize::new(0),
        )),
        test_config(),
    );
    al.register_tool("flush_probe_tool".to_string(), Box::new(FlushProbeTool));
    let key = "agent:main:session:flushwork";
    crate::chat_log::delete_chat_log(key);

    let msg = nemesis_types::channel::InboundMessage::user_ingress(
        "web".to_string(),
        "flushuser".to_string(),
        "flushchat".to_string(),
        "hi".to_string(),
    )
    .with_session_key(key.to_string());
    let (_, resp, err, _) = al.process_inbound_message(&msg).await;
    assert!(
        err.is_some(),
        "第二轮 auth 失败必须以 Error 终局: err={err:?}, resp={resp}"
    );

    let (rows, _, _, _) = crate::chat_log::read_chat_log(key, 200, None);
    // jsonl 恒「恰好 +2 行」——冲刷已移除，中间正文不再出独立行。
    assert_eq!(
        rows.len(),
        2,
        "带中间正文的错误收尾轮 jsonl 必须恰好两行（1 user + 1 终局行）: {rows:?}"
    );
    let assistant_rows = rows.iter().filter(|r| r["role"] == "assistant").count();
    assert_eq!(assistant_rows, 1, "assistant 终局行必须恰好一行");
    // 中间正文零独立行（曾经冲刷块的唯一职责面——移除后必须恒零）。
    let interim_standalone = rows
        .iter()
        .filter(|r| {
            r["role"] == "assistant"
                && r["content"]
                    .as_str()
                    .unwrap_or("")
                    .contains("FLUSH_MARKER_中间叙述正文")
        })
        .count();
    assert_eq!(
        interim_standalone, 0,
        "中间叙述正文不得作为独立 jsonl 行落盘（重载双重渲染的根源）"
    );
    // 中间正文必须随终局行的 work 键落盘（work.rounds[].text）。
    let final_row = rows
        .iter()
        .find(|r| r["role"] == "assistant")
        .expect("assistant 终局行必须存在");
    let rounds = final_row["work"]["rounds"]
        .as_array()
        .expect("错误收尾轮终局行必须带 work 键（rounds 数组）");
    assert!(
        rounds.iter().any(|r| r["text"]
            .as_str()
            .unwrap_or("")
            .contains("FLUSH_MARKER_中间叙述正文")),
        "中间叙述正文必须随 work 键落盘（唯一持久化面）: {rounds:?}"
    );

    crate::chat_log::delete_chat_log(key);
}
