// codex_cli.rs 覆盖率补充测试（build_prompt 空消息循环体 106、JSONL 解析
// 空行 171 / 未知事件 209 / error 事件终局 224 / 纯文本 stop 臂 229、
// 空命令拒绝 255、spawn 失败 303-305、空 stdout 成功退出终局解析 332、
// .bat 假 CLI 成功链路）。

use super::*;
use crate::types::{ChatOptions, Message, ToolDefinition};
use std::path::PathBuf;

fn provider_with(command: &str) -> CodexCliProvider {
    CodexCliProvider::new(CodexCliConfig {
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
        "nmb-codexcli-cov-{}-{name}-{n}",
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

/// build_prompt：空消息列表（消息循环体不执行，106）+ 未知 role 走通配臂。
#[test]
fn build_prompt_with_empty_and_odd_role_messages() {
    let provider = provider_with("codex");
    let prompt = provider.build_prompt(&[], &[]);
    let _ = prompt; // 只求走通空循环体（106），不断言具体文案。

    let odd = vec![Message::text("carrier-pigeon", "非标准角色")];
    let prompt = provider.build_prompt(&odd, &[]);
    assert!(
        !prompt.contains("非标准角色"),
        "未知 role 的内容不得混入 prompt：{prompt}"
    );
}

/// parse_jsonl_events：空行（171）与坏 JSON 行静默跳过，正常事件照收。
#[test]
fn parse_blank_and_malformed_lines_are_skipped() {
    let provider = provider_with("codex");
    let out = "\n   \n{not json}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"ok\"}}\n";
    let resp = provider.parse_jsonl_events(out).unwrap();
    assert_eq!(resp.content, "ok");
}

/// 未知事件类型走通配臂（209），turn.failed 记录但不吞内容。
#[test]
fn parse_unknown_event_and_turn_failed_are_tolerated() {
    let provider = provider_with("codex");
    let out = concat!(
        "{\"type\":\"cov.unknown.event\",\"data\":1}\n",
        "{\"type\":\"turn.failed\",\"error\":{\"message\":\"failed hard\"}}\n",
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"still fine\"}}\n",
    );
    let resp = provider.parse_jsonl_events(out).unwrap();
    assert_eq!(resp.content, "still fine");
}

/// error 事件 + 无内容 → Err「codex cli: …」（224）。
#[test]
fn parse_error_event_with_no_content_returns_unknown_err() {
    let provider = provider_with("codex");
    let err = provider
        .parse_jsonl_events("{\"type\":\"error\",\"message\":\"boom\"}\n")
        .unwrap_err();
    match err {
        FailoverError::Unknown { message, .. } => {
            assert!(message.contains("codex cli: boom"), "{message}");
        }
        other => panic!("预期 Unknown，得到 {other:?}"),
    }
}

/// 纯 agent_message 无工具 → finish_reason = stop（229）。
#[test]
fn parse_agent_message_without_tools_finishes_stop() {
    let provider = provider_with("codex");
    let out =
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"hi\"}}\n";
    let resp = provider.parse_jsonl_events(out).unwrap();
    assert_eq!(resp.finish_reason, "stop");
    assert!(resp.tool_calls.is_empty());
}

/// 空命令 → 直接拒绝（255-260）。
#[tokio::test]
async fn chat_empty_command_rejected() {
    let provider = provider_with("");
    let messages = vec![Message::text("user", "hi")];
    let err = provider
        .chat(&messages, &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Unknown { message, .. } => {
            assert!(
                message.contains("codex command not configured"),
                "{message}"
            );
        }
        other => panic!("预期 Unknown，得到 {other:?}"),
    }
}

/// 不存在的命令 → spawn 失败诚实上报（303-305）。
#[tokio::test]
async fn chat_nonexistent_command_reports_spawn_failure() {
    let provider = provider_with("nmb-cov-nonexistent-cmd-xyz");
    let messages = vec![Message::text("user", "hi")];
    let err = provider
        .chat(&messages, &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Unknown { message, .. } => {
            assert!(message.contains("failed to execute codex cli"), "{message}");
        }
        other => panic!("预期 Unknown，得到 {other:?}"),
    }
}

/// 成功退出但 stdout 为空 → 落终局 parse（332），返回空内容响应。
#[tokio::test]
async fn chat_empty_stdout_success_exit_falls_to_final_parse() {
    let bat = write_batch("empty-out", "exit /b 0\r\n");
    let provider = provider_with(bat.to_str().unwrap());
    let messages = vec![Message::text("user", "hi")];
    let resp = provider
        .chat(&messages, &[], "m", &ChatOptions::default())
        .await
        .unwrap();
    assert!(resp.content.is_empty(), "{:?}", resp.content);
}

/// .bat 假 CLI 全链路：JSONL 输出 → content + usage + stop。
#[tokio::test]
async fn chat_success_via_fake_batch() {
    let bat = write_batch(
        "fake-codex",
        "echo {\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"hi from fake codex\"}}\r\necho {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":2,\"cached_input_tokens\":1,\"output_tokens\":3}}\r\n",
    );
    let provider = provider_with(bat.to_str().unwrap());
    let messages = vec![Message::text("user", "hi")];
    let resp = provider
        .chat(&messages, &[], "m", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "hi from fake codex");
    let usage = resp.usage.expect("usage 必须解析");
    assert_eq!(usage.prompt_tokens, 3);
    assert_eq!(usage.completion_tokens, 3);
}

/// build_tools_prompt 对照（工具段渲染不炸）。
#[test]
fn build_tools_prompt_renders_function_tools() {
    let provider = provider_with("codex");
    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "exec".to_string(),
            description: "跑命令".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];
    let prompt = provider.build_tools_prompt(&tools);
    assert!(prompt.contains("exec"), "{prompt}");
}

/// agent_message 文本内嵌 tool_calls JSON → 提取为工具调用（224/229 的
/// tool_calls 两臂）+ 正文剥离。
#[test]
fn parse_agent_message_with_embedded_tool_calls() {
    let provider = provider_with("codex");
    let out = concat!(
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":",
        "\" 前缀 {\\\"tool_calls\\\":[{\\\"id\\\":\\\"c1\\\",\\\"type\\\":\\\"function\\\",\\\"function\\\":{\\\"name\\\":\\\"exec\\\",\\\"arguments\\\":\\\"{}\\\"}}]}\"}}
"
    );
    let resp = provider.parse_jsonl_events(out).unwrap();
    assert_eq!(resp.finish_reason, "tool_calls");
    assert_eq!(resp.tool_calls.len(), 1, "{:?}", resp.tool_calls);
    assert_eq!(resp.tool_calls[0].function.as_ref().unwrap().name, "exec");
    assert!(!resp.content.contains("tool_calls"), "{}", resp.content);
}
