//! E5（2026-10-15 收尾保障）：预算暂停前收尾编译探针测试。
//!
//! - 纯函数：select_probe_command（生态选择 + 优先序）/ first_error_excerpt
//!   （cargo short 行摘录 / stdout 回落 / 截断）/ probe_note（四形态文案）。
//! - loop 级咬合锁：写类工具 + 预算暂停 + workspace 命中 Cargo.toml →
//!   Done 追加探针结论（真 cargo 编译单文件语法错误，秒败零依赖）；非写类
//!   工具 / config 显式关闭 两道闸不触发 → Done 无探针字样。

use super::*;
use std::path::Path;

// ---------------------------------------------------------------------------
// 纯函数：select_probe_command
// ---------------------------------------------------------------------------

#[test]
fn select_probe_command_by_marker() {
    // rust：Cargo.toml → cargo check --all-targets（连测试一起编）。
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    let (marker, command) = select_probe_command(ws.path()).unwrap();
    assert_eq!(marker, "Cargo.toml");
    assert!(command.contains("cargo check"));
    assert!(
        command.contains("--all-targets"),
        "必须连测试一起编：{command}"
    );

    // go：go.mod → go build ./...
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("go.mod"), "module x\n").unwrap();
    let (marker, command) = select_probe_command(ws.path()).unwrap();
    assert_eq!(marker, "go.mod");
    assert_eq!(command, "go build ./...");

    // maven：pom.xml → mvn compile。
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("pom.xml"), "<project/>").unwrap();
    let (marker, _) = select_probe_command(ws.path()).unwrap();
    assert_eq!(marker, "pom.xml");

    // 无标记（研究型/非编码工作区）→ None：完全静默。
    let ws = tempfile::tempdir().unwrap();
    assert!(select_probe_command(ws.path()).is_none());

    // node 生态刻意不设探针（JS 无通用编译闸）——不硬猜命令。
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("package.json"), "{}").unwrap();
    assert!(select_probe_command(ws.path()).is_none());
}

#[test]
fn select_probe_command_priority_first_marker_wins() {
    // 多标记共存：与 CHECK_ECOSYSTEMS 同序，首个命中（rust）优先。
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    std::fs::write(ws.path().join("go.mod"), "module x\n").unwrap();
    std::fs::write(ws.path().join("pom.xml"), "<project/>").unwrap();
    let (marker, _) = select_probe_command(ws.path()).unwrap();
    assert_eq!(marker, "Cargo.toml");
}

// ---------------------------------------------------------------------------
// 纯函数：first_error_excerpt
// ---------------------------------------------------------------------------

#[test]
fn first_error_excerpt_prefers_stderr_and_truncates() {
    // cargo --message-format=short 的错误行形态（stderr）。
    let stderr = "   Compiling e5 v0.1.0\nsrc/lib.rs:1:12: error: expected one of `:` or `@`, found `{`\nerror: aborting due to 1 previous error\n";
    let stdout = "irrelevant stdout\n";
    let ex = first_error_excerpt(stdout, stderr);
    assert!(ex.starts_with("src/lib.rs:1:12: error"), "got: {ex}");
    assert!(!ex.contains("Compiling"), "取首个 error 行非进度行");

    // stderr 无 error 签名 → stdout 回落（tsc 形态：错误在 stdout）。
    let ex = first_error_excerpt("src/a.ts(1,5): error TS2304: Cannot find name 'x'", "quiet");
    assert!(ex.contains("TS2304"), "got: {ex}");

    // 双双无 error → 空串（注记退化通用文案）。
    assert!(first_error_excerpt("all good", "").is_empty());

    // 超长行截断到上限。
    let long = format!("{}error tail", "x".repeat(600));
    let ex = first_error_excerpt(&long, "");
    assert_eq!(ex.chars().count(), PROBE_EXCERPT_MAX_CHARS);
}

// ---------------------------------------------------------------------------
// 纯函数：probe_note
// ---------------------------------------------------------------------------

#[test]
fn probe_note_covers_all_outcomes() {
    let pass = probe_note(&ProbeOutcome::Passed {
        marker: "Cargo.toml",
        command: "cargo check --all-targets --message-format=short",
        secs: 3,
    });
    assert!(pass.starts_with("\n\n"), "追加文本以空行起头");
    assert!(pass.contains("编译检查通过"), "got: {pass}");
    assert!(pass.contains("可编译状态"));

    let fail = probe_note(&ProbeOutcome::Failed {
        marker: "Cargo.toml",
        command: "cargo check --all-targets --message-format=short",
        excerpt: "src/lib.rs:1:12: error: expected `;`".to_string(),
    });
    assert!(fail.contains("编译检查失败"), "got: {fail}");
    assert!(fail.contains("src/lib.rs:1:12"), "带错误摘录");
    assert!(fail.contains("先修复"));

    let fail_bare = probe_note(&ProbeOutcome::Failed {
        marker: "go.mod",
        command: "go build ./...",
        excerpt: String::new(),
    });
    assert!(fail_bare.contains("编译检查失败"), "got: {fail_bare}");
    assert!(!fail_bare.contains("首个错误"), "无摘录退化为通用文案");

    let timeout = probe_note(&ProbeOutcome::TimedOut {
        marker: "Cargo.toml",
        command: "cargo check --all-targets --message-format=short",
        timeout_secs: 240,
    });
    assert!(timeout.contains("240s"), "got: {timeout}");
    assert!(timeout.contains("未验证"), "超时=状态未知，诚实注记");

    let missing = probe_note(&ProbeOutcome::ToolMissing {
        marker: "Cargo.toml",
        command: "cargo check --all-targets --message-format=short",
    });
    assert!(missing.contains("跳过"), "got: {missing}");
}

// ---------------------------------------------------------------------------
// loop 级咬合锁
// ---------------------------------------------------------------------------

/// Mock LLM provider（与 tests.rs 同形；本文件独立副本）。
struct ProbeMockProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
}

impl ProbeMockProvider {
    fn tool_loops(n: usize) -> Self {
        Self {
            responses: std::sync::Mutex::new(
                (0..n)
                    .map(|_| LlmResponse {
                        finish_reason: None,
                        escalated_to: None,
                        content: String::new(),
                        tool_calls: vec![ToolCallInfo {
                            id: "tc_e5".to_string(),
                            name: "write_file".to_string(),
                            arguments: r#"{"path":"a.txt","content":"x"}"#.to_string(),
                        }],
                        finished: false,
                        reasoning_content: None,
                        usage: None,
                        raw_request_body: None,
                        raw_response_body: None,
                    })
                    .collect(),
            ),
        }
    }

    /// 非 write 类变体（触发闸对照臂用）。
    fn tool_loops_read(n: usize) -> Self {
        Self {
            responses: std::sync::Mutex::new(
                (0..n)
                    .map(|_| LlmResponse {
                        finish_reason: None,
                        escalated_to: None,
                        content: String::new(),
                        tool_calls: vec![ToolCallInfo {
                            id: "tc_e5r".to_string(),
                            name: "calculator".to_string(),
                            arguments: "{}".to_string(),
                        }],
                        finished: false,
                        reasoning_content: None,
                        usage: None,
                        raw_request_body: None,
                        raw_response_body: None,
                    })
                    .collect(),
            ),
        }
    }
}

#[async_trait]
impl LlmProvider for ProbeMockProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut responses = self.responses.lock().unwrap();
        Ok(responses.remove(0))
    }
}

/// Mock 工具：恒成功（record_write_success 的置位前提）。
struct ProbeMockTool {
    result: String,
}

#[async_trait]
impl Tool for ProbeMockTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(self.result.clone())
    }
}

fn probe_test_config(tool_name: &str) -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 100,
        tools: vec![tool_name.to_string()],
        models: std::collections::HashMap::new(),
    }
}

/// 造「必然编译失败」的最小 cargo 项目：单文件语法错误，零依赖——
/// rustc 秒败，测试环境（能跑 cargo test 的机器）必有 cargo。
fn write_broken_crate(root: &Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"e5probe\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn e5_broken( {\n").unwrap();
}

/// 咬合锁主臂：写类工具 + 预算暂停 + workspace 命中 Cargo.toml → Done
/// 追加「收尾探针：编译检查失败」注记，且暂停文案本体不变（追加非替换）。
#[tokio::test]
async fn budget_pause_appends_compile_probe_note_after_writes() {
    let ws = tempfile::tempdir().unwrap();
    write_broken_crate(ws.path());

    let mut agent_loop = AgentLoop::new(
        Box::new(ProbeMockProvider::tool_loops(20)),
        probe_test_config("write_file"),
    );
    agent_loop.set_workspace_root(ws.path().to_path_buf());
    agent_loop.register_tool(
        "write_file".to_string(),
        Box::new(ProbeMockTool {
            result: "ok".to_string(),
        }),
    );

    let instance = AgentInstance::new(probe_test_config("write_file"));
    let context = RequestContext::new("web", "chat1", "user1", "session-e5-probe");
    let trace_id = "e5-probe".to_string();
    let token = tokio_util::sync::CancellationToken::new();
    let events = agent_loop
        .run_with_trace(
            &instance,
            "edit code forever",
            &context,
            &trace_id,
            false,
            &token,
            Some(1),
            &[],
        )
        .await;

    let done = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .expect("budget stop emits a Done");
    assert!(
        done.contains("已在任务预算 1 轮"),
        "暂停文案本体仍在: {done}"
    );
    assert!(
        done.contains("收尾探针：编译检查失败"),
        "写类工具 + 编译必破 → 注记必须落: {done}"
    );
    assert!(
        done.contains("src/lib.rs:1:") || done.contains("error"),
        "摘录带错误现场: {done}"
    );
}

/// 触发闸对照臂 ①：非写类工具（calculator）不触发探针——workspace 同样
/// 命中 Cargo.toml，Done 也不得出现探针字样（had_writes 闸）。
#[tokio::test]
async fn budget_pause_skips_probe_without_write_tools() {
    let ws = tempfile::tempdir().unwrap();
    write_broken_crate(ws.path());

    let mut agent_loop = AgentLoop::new(
        Box::new(ProbeMockProvider::tool_loops_read(20)),
        probe_test_config("calculator"),
    );
    agent_loop.set_workspace_root(ws.path().to_path_buf());
    agent_loop.register_tool(
        "calculator".to_string(),
        Box::new(ProbeMockTool {
            result: "0".to_string(),
        }),
    );

    let instance = AgentInstance::new(probe_test_config("calculator"));
    let context = RequestContext::new("web", "chat1", "user1", "session-e5-nowrite");
    let trace_id = "e5-probe-ctrl".to_string();
    let token = tokio_util::sync::CancellationToken::new();
    let events = agent_loop
        .run_with_trace(
            &instance,
            "calculate forever",
            &context,
            &trace_id,
            false,
            &token,
            Some(1),
            &[],
        )
        .await;

    let done = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .expect("budget stop emits a Done");
    assert!(!done.contains("收尾探针"), "非写类工具不得触发探针: {done}");
}

/// 触发闸对照臂 ②：config 显式 `pre_pause_probe.enabled=false` → 不触发
/// （配置闸；新鲜读路径——config_path 指向临时 config.json）。
#[tokio::test]
async fn budget_pause_respects_config_disable() {
    let ws = tempfile::tempdir().unwrap();
    write_broken_crate(ws.path());
    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"agents":{"defaults":{"pre_pause_probe":{"enabled":false}}}}"#,
    )
    .unwrap();

    let mut agent_loop = AgentLoop::new(
        Box::new(ProbeMockProvider::tool_loops(20)),
        probe_test_config("write_file"),
    );
    agent_loop.set_workspace_root(ws.path().to_path_buf());
    agent_loop.set_config_path(cfg_path);
    agent_loop.register_tool(
        "write_file".to_string(),
        Box::new(ProbeMockTool {
            result: "ok".to_string(),
        }),
    );

    let instance = AgentInstance::new(probe_test_config("write_file"));
    let context = RequestContext::new("web", "chat1", "user1", "session-e5-off");
    let trace_id = "e5-probe-off".to_string();
    let token = tokio_util::sync::CancellationToken::new();
    let events = agent_loop
        .run_with_trace(
            &instance,
            "edit code forever",
            &context,
            &trace_id,
            false,
            &token,
            Some(1),
            &[],
        )
        .await;

    let done = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .expect("budget stop emits a Done");
    assert!(
        !done.contains("收尾探针"),
        "config 关闭不得触发探针: {done}"
    );
}
