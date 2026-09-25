// claude_cli.rs 覆盖率补充测试（build_system_prompt 跳过非 function 工具
// 101、messages_to_prompt 通配 role 142、chat 本体 220 + spawn 失败
// 268-270、成功链路 parse_response 尾巴 288、stderr 错误与 is_error 路径）。

use super::*;
use crate::types::{ChatOptions, Message, ToolDefinition};
use std::path::PathBuf;

fn provider_with(command: &str) -> ClaudeCliProvider {
    ClaudeCliProvider::new(ClaudeCliConfig {
        command: command.to_string(),
        workspace: String::new(),
        default_model: "cov-model".to_string(),
    })
}

fn temp_dir(name: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "nmb-claudecli-cov-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_batch(name: &str, body: &str) -> PathBuf {
    let dir = temp_dir(name);
    let path = dir.join(format!("{name}.cmd"));
    std::fs::write(&path, format!("@echo off\r\n{body}")).unwrap();
    path
}

fn function_tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: name.to_string(),
            description: format!("{name} 的说明"),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
    }
}

/// build_system_prompt：非 function 工具跳过（101），function 工具带
/// 描述与参数渲染。
#[test]
fn build_system_prompt_skips_non_function_tools() {
    let provider = provider_with("claude");
    let mut web = function_tool("search");
    web.tool_type = "web_search".to_string();

    let prompt = provider.build_system_prompt(&[], &[web, function_tool("exec")]);
    assert!(
        !prompt.contains("#### search"),
        "非 function 工具必须跳过：{prompt}"
    );
    assert!(prompt.contains("#### exec"), "{prompt}");
    assert!(prompt.contains("exec 的说明"), "{prompt}");
}

/// messages_to_prompt：未知 role 走通配臂被丢弃（142）；tool 结果带
/// call_id 渲染；单 user 消息简化。
#[test]
fn messages_to_prompt_wildcard_and_tool_forms() {
    let provider = provider_with("claude");
    let mut tool_msg = Message::text("tool", "工具结果内容");
    tool_msg.tool_call_id = Some("call-9".to_string());
    let messages = vec![
        Message::text("system", "系统提示"),
        Message::text("user", "问题"),
        Message::text("carrier-pigeon", "不该出现"),
        tool_msg,
    ];
    let prompt = provider.messages_to_prompt(&messages);
    assert!(prompt.contains("[Tool Result for call-9]"), "{prompt}");
    assert!(!prompt.contains("不该出现"), "未知 role 必须丢弃：{prompt}");
}

/// 不存在的命令 → spawn 失败诚实上报（268-270）。
#[tokio::test]
async fn chat_nonexistent_command_reports_spawn_failure() {
    let provider = provider_with("nmb-cov-nonexistent-claude-xyz");
    let messages = vec![Message::text("user", "hi")];
    let err = provider
        .chat(&messages, &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Unknown { message, .. } => {
            assert!(
                message.contains("failed to execute claude cli"),
                "{message}"
            );
        }
        other => panic!("预期 Unknown，得到 {other:?}"),
    }
}

/// 非零退出 + stderr → 「claude cli error: …」。
#[tokio::test]
async fn chat_nonzero_exit_with_stderr_reports_stderr() {
    let bat = write_batch("stderr-out", "echo boom 1>&2\r\nexit /b 3\r\n");
    let provider = provider_with(bat.to_str().unwrap());
    let messages = vec![Message::text("user", "hi")];
    let err = provider
        .chat(&messages, &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Unknown { message, .. } => {
            assert!(message.contains("claude cli error"), "{message}");
            assert!(message.contains("boom"), "{message}");
        }
        other => panic!("预期 Unknown，得到 {other:?}"),
    }
}

/// .bat 假 CLI 输出 result JSON → 成功链路（288 parse_response 尾巴 +
/// usage 汇总）。
#[tokio::test]
async fn chat_success_via_fake_batch() {
    let bat = write_batch(
        "fake-claude",
        "echo {\"type\":\"result\",\"is_error\":false,\"result\":\"hello from fake claude\",\"usage\":{\"input_tokens\":3,\"cache_creation_input_tokens\":1,\"cache_read_input_tokens\":2,\"output_tokens\":5}}\r\n",
    );
    let provider = provider_with(bat.to_str().unwrap());
    let messages = vec![Message::text("user", "hi")];
    let resp = provider
        .chat(&messages, &[], "m", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "hello from fake claude");
    let usage = resp.usage.expect("usage 必须汇总");
    assert_eq!(usage.prompt_tokens, 6);
    assert_eq!(usage.completion_tokens, 5);
    assert_eq!(usage.cached_tokens, Some(2));
}

/// is_error = true → Unknown 错误透传 result 文本。
#[test]
fn parse_response_is_error_returns_unknown() {
    let provider = provider_with("claude");
    let err = provider
        .parse_response("{\"type\":\"result\",\"is_error\":true,\"result\":\"额度用尽\"}")
        .unwrap_err();
    match err {
        FailoverError::Unknown { message, .. } => assert_eq!(message, "额度用尽"),
        other => panic!("预期 Unknown，得到 {other:?}"),
    }
}
