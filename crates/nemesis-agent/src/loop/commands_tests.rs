//! 自定义 slash 命令改写（2026-08-29）：`rewrite_custom_command` 的决策表
//! 测试——展开/占位替换/无占位追加/内置跳过/未命中不动/非 slash 不动。
//! 命令表写进临时目录的 `config.commands.json`，经 `set_commands_path` 走
//! 与生产一致的加载路径。

use super::*;
use crate::types::AgentConfig;

struct NoopProvider;

#[async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("not used in rewrite tests".into())
    }
}

fn loop_with_commands_table(dir: &std::path::Path) -> AgentLoop {
    let al = AgentLoop::new(Box::new(NoopProvider), AgentConfig::default());
    al.set_commands_path(dir.join("config.commands.json"));
    al
}

fn write_table(dir: &std::path::Path, body: &str) {
    std::fs::write(dir.join("config.commands.json"), body).unwrap();
}

fn inbound(content: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".into(),
        sender_id: "user".into(),
        chat_id: "chat".into(),
        content: content.into(),
        media: Vec::new(),
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: Default::default(),
        voice_playback: None,
    }
}

const TABLE: &str = r#"{
  "commands": [
    { "name": "review", "description": "d", "argument_hint": "<路径>",
      "prompt": "请审查 $ARGUMENTS 的代码质量" },
    { "name": "daily", "description": "d",
      "prompt": "总结今天的工作" }
  ]
}"#;

fn msg_content_after(al: &AgentLoop, content: &str) -> String {
    let mut msg = inbound(content);
    al.rewrite_custom_command(&mut msg);
    msg.content
}

#[test]
fn rewrites_placeholder_with_args() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    let out = msg_content_after(&al, "/review src/main.rs");
    assert_eq!(out, "请审查 src/main.rs 的代码质量");
}

#[test]
fn rewrites_without_args_empty_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    // 有占位符但未带参数 → 替换为空串（模板原样保留其余文字）。
    let out = msg_content_after(&al, "/review");
    assert_eq!(out, "请审查  的代码质量");
}

#[test]
fn appends_args_when_template_lacks_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    // daily 模板无 $ARGUMENTS → 带参数时追加为独立段（防参数被吞）。
    let out = msg_content_after(&al, "/daily 周五冲刺");
    assert_eq!(out, "总结今天的工作\n\n周五冲刺");
    // 不带参数 → 模板原样。
    let out = msg_content_after(&al, "/daily");
    assert_eq!(out, "总结今天的工作");
}

#[test]
fn builtin_names_skip_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    // 内置优先：即使命令表里恰好有同名条目也不改写（测试表里没有，
    // 这里验证内置名不被任何路径吞掉——内容原样保留）。
    let out = msg_content_after(&al, "/model deepseek-v4");
    assert_eq!(out, "/model deepseek-v4");
}

#[test]
fn unknown_name_and_non_slash_untouched() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    let out = msg_content_after(&al, "/no_such_cmd args");
    assert_eq!(out, "/no_such_cmd args");
    let out = msg_content_after(&al, "普通消息 /review 不受影响");
    assert_eq!(out, "普通消息 /review 不受影响");
}

#[test]
fn mtime_reload_picks_up_table_edits_without_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());
    assert_eq!(msg_content_after(&al, "/daily"), "总结今天的工作");

    // 改表（mtime 变化）→ 下一条消息即用新表（无需重建 loop）。
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_table(
        dir.path(),
        r#"{ "commands": [ { "name": "daily", "prompt": "新模板" } ] }"#,
    );
    let out = msg_content_after(&al, "/daily");
    assert_eq!(out, "新模板");
}
