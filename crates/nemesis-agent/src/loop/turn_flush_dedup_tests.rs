//! turn.rs 异常收尾冲刷去重（BUG-20261031-237）回归测试。
//!
//! 终局错误在 history 落**裸错误**（`Error: {raw}`，recovery.rs 三处
//! add_assistant_message），用户可见终局是**改写形态**（可行动提示
//! `Error: {hint}（原始错误：{raw}）`/ vision 兜底提示追加），turn.rs
//! 冲刷过滤器的等值比较 `c != final_response` 对改写形态必然失效 →
//! 冲刷把终局错误再落一行 + 主路径终局行 = 同一失败在 chat_log 双份
//! （违背该块注释「避免双份」的声明意图）。修后：终局错误只由主路径
//! 落一行，冲刷只承担它真正的职责（中间正文）。

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
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 错误终局轮：chat_log 的终局错误恰好一行（主路径落盘），裸错误
/// history 形态不得经冲刷重复落盘。
#[tokio::test]
async fn error_ending_flush_does_not_duplicate_terminal_error_row() {
    let al = AgentLoop::new(Box::new(AuthFailProvider), test_config());
    let key = "agent:main:session:flushdup";
    crate::chat_log::delete_chat_log(key);

    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "flushuser".to_string(),
        chat_id: "flushchat".to_string(),
        content: "hi".to_string(),
        media: vec![],
        session_key: key.to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    let (_, resp, err) = al.process_inbound_message(&msg).await;
    assert!(
        err.is_some(),
        "auth 失败必须以 Error 终局: err={err:?}, resp={resp}"
    );

    let (rows, _, _, _) = crate::chat_log::read_chat_log(key, 200, None);
    // 终局错误行（改写形态，含「原始错误」）恰好一行——主路径落盘。
    let final_rows = rows
        .iter()
        .filter(|r| {
            r["role"] == "assistant" && r["content"].as_str().unwrap_or("").contains("原始错误")
        })
        .count();
    assert_eq!(final_rows, 1, "终局错误行必须恰好一行（主路径落盘）");
    // 裸错误 history 形态（"Error: {raw}"）不得经冲刷再落一行。
    let raw_rows = rows
        .iter()
        .filter(|r| {
            r["role"] == "assistant"
                && r["content"].as_str().unwrap_or("") == "Error: request failed: unauthorized"
        })
        .count();
    assert_eq!(
        raw_rows, 0,
        "终局错误不得经冲刷重复落盘（裸形态行数应为 0）"
    );

    crate::chat_log::delete_chat_log(key);
}
