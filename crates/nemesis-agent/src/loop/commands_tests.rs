//! 自定义 slash 命令改写：`rewrite_custom_command` 的决策表测试——
//! 展开/占位替换/无占位追加/内置跳过/未命中不动/非 slash 不动，以及
//! K3（devtool-upgrade 阶段 4）新增的 `` !`cmd` `` 注入与技能回落。
//! 命令表写进临时目录的 `config.commands.json`，经 `set_commands_path` 走
//! 与生产一致的加载路径。K3 起 rewrite 是 async（模板注入要跑命令）。

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
      "prompt": "总结今天的工作" },
    { "name": "shout", "description": "d",
      "prompt": "把下面内容念出来：!`echo hello`" },
    { "name": "boom", "description": "d",
      "prompt": "结果：!`exit 3`" },
    { "name": "open_ended", "description": "d",
      "prompt": "未闭合 token：!`echo hi" }
  ]
}"#;

/// 装好一个已安装技能（workspace/skills/<name>/SKILL.md）+ loader。
/// frontmatter name 与目录名一致（scan_single_skill 优先取 frontmatter）。
fn loop_with_skill(root: &std::path::Path, skill_name: &str) -> AgentLoop {
    let skill_dir = root.join("skills").join(skill_name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {skill_name}\ndescription: d\n---\nbody\n"),
    )
    .unwrap();
    let al = AgentLoop::new(Box::new(NoopProvider), AgentConfig::default());
    al.set_skills_loader(std::sync::Arc::new(
        nemesis_skills::loader::SkillsLoader::new(
            root.to_str().unwrap(),
            root.join("global").to_str().unwrap(),
            root.join("builtin").to_str().unwrap(),
        ),
    ));
    al
}

async fn msg_content_after(al: &AgentLoop, content: &str) -> String {
    let mut msg = inbound(content);
    al.rewrite_custom_command(&mut msg).await;
    msg.content
}

#[tokio::test]
async fn rewrites_placeholder_with_args() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    let out = msg_content_after(&al, "/review src/main.rs").await;
    assert_eq!(out, "请审查 src/main.rs 的代码质量");
}

#[tokio::test]
async fn rewrites_without_args_empty_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    // 有占位符但未带参数 → 替换为空串（模板原样保留其余文字）。
    let out = msg_content_after(&al, "/review").await;
    assert_eq!(out, "请审查  的代码质量");
}

#[tokio::test]
async fn appends_args_when_template_lacks_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    // daily 模板无 $ARGUMENTS → 带参数时追加为独立段（防参数被吞）。
    let out = msg_content_after(&al, "/daily 周五冲刺").await;
    assert_eq!(out, "总结今天的工作\n\n周五冲刺");
    // 不带参数 → 模板原样。
    let out = msg_content_after(&al, "/daily").await;
    assert_eq!(out, "总结今天的工作");
}

#[tokio::test]
async fn builtin_names_skip_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    // 内置优先：即使命令表里恰好有同名条目也不改写（测试表里没有，
    // 这里验证内置名不被任何路径吞掉——内容原样保留）。
    let out = msg_content_after(&al, "/model deepseek-v4").await;
    assert_eq!(out, "/model deepseek-v4");
}

#[tokio::test]
async fn unknown_name_and_non_slash_untouched() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    let out = msg_content_after(&al, "/no_such_cmd args").await;
    assert_eq!(out, "/no_such_cmd args");
    let out = msg_content_after(&al, "普通消息 /review 不受影响").await;
    assert_eq!(out, "普通消息 /review 不受影响");
}

// ---------------------------------------------------------------------------
// K3：`` !`cmd` `` 注入
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shell_injection_expands_in_template() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    // echo 跨平台可用（Windows cmd /C 与 Unix sh -c 语义一致）；
    // stdout 尾随换行被 trim（echo 会带 \n / \r\n）。
    let out = msg_content_after(&al, "/shout").await;
    assert_eq!(out, "把下面内容念出来：hello");
}

#[tokio::test]
async fn shell_injection_failure_becomes_note() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    // exit 3 跨平台产生非零退出码 → 注记替换（计划语义：失败注记）。
    let out = msg_content_after(&al, "/boom").await;
    assert_eq!(out, "结果：[command failed: exit 3: exit 3]");
}

#[tokio::test]
async fn unterminated_backtick_kept_literal() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());

    // 无闭合反引号 → 不算注入 token，整段字面保留（不执行、不吞字）。
    let out = msg_content_after(&al, "/open_ended").await;
    assert_eq!(out, "未闭合 token：!`echo hi");
}

#[test]
fn extract_dedupes_and_keeps_first_seen_order() {
    let text = "a !`echo 1` b !`echo 2` c !`echo 1` d !`echo 3`";
    assert_eq!(
        extract_shell_injections(text),
        vec!["echo 1", "echo 2", "echo 3"]
    );
    // 空命令 token 不进清单。
    assert!(extract_shell_injections("!` `").is_empty());
    // 未闭合不算。
    assert!(extract_shell_injections("!`oops").is_empty());
    // 无注入 → 空。
    assert!(extract_shell_injections("plain `code` text").is_empty());
}

#[test]
fn substitute_replaces_via_resolver_and_keeps_edge_tokens() {
    let resolved = substitute_shell_injections("x !`cmd1` y !`cmd1` z", &|c| format!("<{c}>"));
    assert_eq!(resolved, "x <cmd1> y <cmd1> z");
    // 空命令保持字面。
    let empty = substitute_shell_injections("a !` ` b", &|_| "R".into());
    assert_eq!(empty, "a !` ` b");
    // 无注入原样返回（不同 String 实例，值相等）。
    let none = substitute_shell_injections("plain", &|_| "R".into());
    assert_eq!(none, "plain");
}

#[test]
fn injection_output_cap_is_char_boundary_safe() {
    // 多字节字符打满上限再截——byte 切片会 panic（str-slice 教训），
    // 字符截断必须干净落在边界上。SHELL_INJECTION_OUTPUT_CAP 经 super::* 导入。
    let cap = SHELL_INJECTION_OUTPUT_CAP;
    let s = "中".repeat(cap + 10);
    let capped = cap_injection_output(&s);
    assert_eq!(capped.chars().count(), cap + "…[truncated]".chars().count());
    assert!(capped.starts_with(&"中".repeat(cap)));
    assert!(capped.ends_with("…[truncated]"));
    // 上限内不截断。
    assert_eq!(cap_injection_output("short"), "short");
}

#[test]
fn tail_line_takes_last_nonempty_line_capped() {
    assert_eq!(tail_line("a\nb\nc", 10), "c");
    assert_eq!(tail_line("\n\n", 10), "");
    let long = "x".repeat(300);
    let capped = tail_line(&long, 200);
    assert_eq!(capped.chars().count(), 201); // 200 + '…'
    assert!(capped.ends_with('…'));
}

// ---------------------------------------------------------------------------
// K3：技能回落
// ---------------------------------------------------------------------------

#[tokio::test]
async fn skill_fallback_rewrites_installed_skill() {
    let dir = tempfile::tempdir().unwrap();
    let al = loop_with_skill(dir.path(), "weather");

    // 带参数 → "Use the {name} skill to handle: {args}"。
    let out = msg_content_after(&al, "/weather 明天会下雨吗").await;
    assert_eq!(out, "Use the weather skill to handle: 明天会下雨吗");
    // 无参数 → "Use the {name} skill."。
    let out = msg_content_after(&al, "/weather").await;
    assert_eq!(out, "Use the weather skill.");
}

#[tokio::test]
async fn skill_fallback_unknown_name_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let al = loop_with_skill(dir.path(), "weather");

    let out = msg_content_after(&al, "/no_such_skill args").await;
    assert_eq!(out, "/no_such_skill args");
}

#[tokio::test]
async fn builtin_name_not_shadowed_by_skill() {
    let dir = tempfile::tempdir().unwrap();
    // 安装一个与内置同名的技能（"model"）——内置优先级恒最高。
    let al = loop_with_skill(dir.path(), "model");

    let out = msg_content_after(&al, "/model gpt-x").await;
    assert_eq!(out, "/model gpt-x");
}

#[tokio::test]
async fn custom_command_table_shadows_skill() {
    let dir = tempfile::tempdir().unwrap();
    // 同名 "review"：既是已装技能又在命令表里——命令表赢。
    let al = loop_with_skill(dir.path(), "review");
    al.set_commands_path(dir.path().join("config.commands.json"));
    write_table(dir.path(), TABLE);

    let out = msg_content_after(&al, "/review src/lib.rs").await;
    assert_eq!(out, "请审查 src/lib.rs 的代码质量");
}

#[tokio::test]
async fn skill_fallback_without_commands_table() {
    let dir = tempfile::tempdir().unwrap();
    // 未 set_commands_path（无命令表）→ 技能回落照常生效。
    let al = loop_with_skill(dir.path(), "weather");

    let out = msg_content_after(&al, "/weather 总结").await;
    assert_eq!(out, "Use the weather skill to handle: 总结");
}

#[test]
fn builtin_list_matches_shared_truth_source() {
    // K3：loop 内置清单转发自 nemesis-types 单一真相源。
    assert_eq!(
        AgentLoop::BUILTIN_SLASH_COMMANDS,
        nemesis_types::constants::BUILTIN_SLASH_COMMANDS
    );
    for name in ["compact", "clear", "plan", "build"] {
        assert!(AgentLoop::BUILTIN_SLASH_COMMANDS.contains(&name));
    }
}

#[tokio::test]
async fn mtime_reload_picks_up_table_edits_without_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    write_table(dir.path(), TABLE);
    let al = loop_with_commands_table(dir.path());
    assert_eq!(msg_content_after(&al, "/daily").await, "总结今天的工作");

    // 改表（mtime 变化）→ 下一条消息即用新表（无需重建 loop）。
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_table(
        dir.path(),
        r#"{ "commands": [ { "name": "daily", "prompt": "新模板" } ] }"#,
    );
    let out = msg_content_after(&al, "/daily").await;
    assert_eq!(out, "新模板");
}
