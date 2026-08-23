//! Tests for `crate::cc_hooks` (K2 — U14 seventh batch, CC 方言层).
//!
//! Acceptance mapping (goal §二 第七批 K2 / U14 原验收):
//! - **现有 CC hook 脚本在我方会话真实触发** → 脚本级测试全部走**真子进程**
//!   （Windows `cmd /C` / 其他 `sh -c`，与桥的执行协议完全一致）：
//!   `pre_tool_use_exit_2_blocks_with_stderr`（lint-on-edit 形态：读
//!   tool_input、stderr 拦停）、`loop_integration_*` 三条（经 AgentLoop 真
//!   run/handle_tool_call 路径）。
//! - **退出码拦停语义正确** → 2=拦（stderr 作 reason）、0=放行、JSON
//!   decision=block=拦、超时/其他=非阻断放行，逐条钉住。
//! - 方言别名：matcher 对 CC 名（Edit/Bash/...）与原始名都命中；
//!   stdin payload 带 `tool_input.file_path`（真 `jq .tool_input.file_path`
//!   脚本的依赖）。

use std::sync::Arc;

use async_trait::async_trait;

use super::{
    CcHookBridge, ScriptOutcome, build_event_payload, cc_tool_alias, parse_cc_hooks,
    pre_tool_use_payload,
};
use crate::context::RequestContext;
use crate::hooks::{HookPrompt, HookToolCall, HookTurnEnd, LifecycleHook, ToolHook};
use crate::instance::AgentInstance;
use crate::r#loop::{AgentLoop, LlmMessage, LlmResponse, LlmProvider, Tool};
use crate::types::{AgentConfig, ChatOptions, ToolCallInfo};

// ---------------------------------------------------------------------------
// 平台脚本助手（hooks.json 里写的 command 字符串；桥自己负责 cmd/sh 包裹）
// ---------------------------------------------------------------------------

/// stderr 输出 msg 并 exit 2（CC 阻断形态）。
fn block_cmd(msg: &str) -> String {
    if cfg!(windows) {
        format!("echo {msg} 1>&2 & exit 2")
    } else {
        format!("echo \"{msg}\" >&2; exit 2")
    }
}

/// exit 0（放行）。
fn allow_cmd() -> &'static str {
    "exit 0"
}

/// stdout 打 JSON decision=block（CC 新式阻断形态）。
fn json_block_cmd(reason: &str) -> String {
    if cfg!(windows) {
        // cmd 的 echo 原样打印引号与花括号。
        format!("echo {{\"decision\":\"block\",\"reason\":\"{reason}\"}}")
    } else {
        format!("echo '{{\"decision\":\"block\",\"reason\":\"{reason}\"}}'")
    }
}

/// 追加一行到 marker 文件（SessionStart/UserPromptSubmit 触发计数用）。
fn append_cmd(path: &std::path::Path, tag: &str) -> String {
    let p = path.to_string_lossy();
    if cfg!(windows) {
        format!("echo {tag}>>{p}")
    } else {
        format!("echo {tag} >> {p}")
    }
}

/// 阻塞 ~3s（超时测试用，配 timeout 1s）。
fn slow_cmd() -> String {
    if cfg!(windows) {
        "ping -n 4 127.0.0.1 >nul".to_string()
    } else {
        "sleep 3".to_string()
    }
}

/// flag 文件存在则 exit 0，否则 stderr+exit 2（Stop 两段式：先拒停后放行）。
fn stop_two_phase_cmd(flag: &std::path::Path) -> String {
    let f = flag.to_string_lossy();
    if cfg!(windows) {
        format!("if exist {f} (exit 0) else (echo more work to do 1>&2 & exit 2)")
    } else {
        format!("[ -f {f} ] && exit 0 || {{ echo \"more work to do\" >&2; exit 2; }}")
    }
}

fn marker_lines(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

fn tempdir() -> std::path::PathBuf {
    tempfile::tempdir()
        .expect("tempdir")
        .keep() // 测试进程内自管生命周期；keep 免得还回句柄
}

fn hook_json(events: &str) -> String {
    format!("{{\"hooks\":{{{events}}}}}")
}

/// 程序化构造单事件 hooks 文档（json! 负责转义——命令串里的引号/反斜杠
/// 直接拼 JSON 字符串必炸）。
fn hooks_doc(event: &str, matcher: Option<&str>, command: &str, timeout: Option<u64>) -> String {
    let mut hook = serde_json::json!({ "command": command });
    if let Some(t) = timeout {
        hook["timeout"] = serde_json::json!(t);
    }
    let mut group = serde_json::json!({ "hooks": [hook] });
    if let Some(m) = matcher {
        group["matcher"] = serde_json::json!(m);
    }
    let mut events = serde_json::Map::new();
    events.insert(event.to_string(), serde_json::Value::Array(vec![group]));
    serde_json::json!({ "hooks": events }).to_string()
}

fn edit_call(args: &str) -> HookToolCall {
    HookToolCall {
        name: "edit".to_string(),
        arguments: args.to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: "session1".to_string(),
    }
}

// ---------------------------------------------------------------------------
// 解析 / matcher / payload（纯函数）
// ---------------------------------------------------------------------------

#[test]
fn parse_full_cc_format() {
    let json = hook_json(
        "\"PreToolUse\":[{\"matcher\":\"Edit|Write\",\"hooks\":[{\"type\":\"command\",\"command\":\"lint.py\"}]}],\
         \"PostToolUse\":[{\"hooks\":[{\"command\":\"log.sh\"}]}],\
         \"SessionStart\":[{\"hooks\":[{\"command\":\"hi.sh\",\"timeout\":5}]}],\
         \"UserPromptSubmit\":[{\"hooks\":[{\"command\":\"p.sh\"}]}],\
         \"Stop\":[{\"hooks\":[{\"command\":\"s.sh\"}]}]",
    );
    let ev = parse_cc_hooks(&json).expect("parse");
    assert_eq!(ev.total_scripts(), 5);
    // 事件字段 PascalCase 反序列化正确（分组数）。
    assert_eq!(ev.pre_tool_use.len(), 1);
    assert_eq!(ev.stop.len(), 1);
}

#[test]
fn parse_bare_top_level_and_ignores_unknown_type() {
    // 裸顶层（无 "hooks" 外层）也要收——外层花括号还是得有（JSON 根必须是
    // 对象）。
    let ev = parse_cc_hooks("{\"PreToolUse\":[{\"hooks\":[{\"command\":\"a\"}]}]}")
        .expect("bare parse");
    assert_eq!(ev.total_scripts(), 1);
    // 空 / 无已知事件 = 空配置。
    assert!(parse_cc_hooks("{}").expect("empty").is_empty());
    // type != command 的跳过。
    let ev = parse_cc_hooks(&hook_json(
        "\"PreToolUse\":[{\"hooks\":[{\"type\":\"http\",\"command\":\"x\"},{\"command\":\"y\"}]}]",
    ))
    .expect("mixed types");
    assert_eq!(ev.total_scripts(), 1);
}

#[test]
fn parse_rejects_garbage() {
    assert!(parse_cc_hooks("not json").is_err());
    // 数字顶层等非对象形态。
    assert!(parse_cc_hooks("42").is_err());
}

#[test]
fn matcher_hits_cc_alias_and_raw_name() {
    let mk = |pattern: &str| super::CcHookGroup {
        matcher: Some(pattern.to_string()),
        hooks: vec![],
    };
    // CC 名 matcher 命中我们的原始工具名（经别名）。
    assert!(mk("Edit").matches("edit"));
    assert!(mk("^Edit$").matches("edit"));
    assert!(mk("Bash").matches("exec"));
    // 原始名 matcher 也直接命中。
    assert!(mk("edit").matches("edit"));
    assert!(mk("mytool").matches("mytool"));
    // 不命中。
    assert!(!mk("Write").matches("edit"));
    // 无 matcher = 全命中。
    let all = super::CcHookGroup {
        matcher: None,
        hooks: vec![],
    };
    assert!(all.matches("anything"));
    // 无效正则（"edit(" 括号不闭）退化为字面子串匹配：命中原名含该子串的
    // 自定义工具；不含则不命中。
    assert!(mk("edit(").matches("edit(v2)"));
    assert!(!mk("edit(").matches("edit"));
}

#[test]
fn alias_table_shape() {
    assert_eq!(cc_tool_alias("exec"), Some("Bash"));
    assert_eq!(cc_tool_alias("edit"), Some("Edit"));
    assert_eq!(cc_tool_alias("write_file"), Some("Write"));
    assert_eq!(cc_tool_alias("read_file"), Some("Read"));
    assert_eq!(cc_tool_alias("grep"), Some("Grep"));
    assert_eq!(cc_tool_alias("weather"), None);
}

#[test]
fn pre_tool_use_payload_has_cc_dialect_fields() {
    // 真 lint-on-edit 脚本依赖 tool_input.file_path + tool_name=Edit。
    let tmp = tempdir();
    let p = pre_tool_use_payload(
        &edit_call(r#"{"path":"src/main.rs","old_string":"a","new_string":"b"}"#),
        &tmp,
    );
    let v: serde_json::Value = serde_json::from_str(&p).unwrap();
    assert_eq!(v["hook_event_name"], "PreToolUse");
    assert_eq!(v["tool_name"], "Edit");
    assert_eq!(v["tool_input"]["file_path"], "src/main.rs");
    // 原字段保留。
    assert_eq!(v["tool_input"]["old_string"], "a");
    assert!(v["session_id"].is_string());
    assert_eq!(v["transcript_path"], "");
}

#[test]
fn build_event_payload_common_fields() {
    let tmp = tempdir();
    let p = build_event_payload(
        "Stop",
        "sess-1",
        &tmp,
        serde_json::json!({"stop_hook_active": true}),
    );
    let v: serde_json::Value = serde_json::from_str(&p).unwrap();
    assert_eq!(v["session_id"], "sess-1");
    assert_eq!(v["hook_event_name"], "Stop");
    assert_eq!(v["stop_hook_active"], true);
    assert_eq!(v["cwd"], tmp.to_string_lossy().to_string());
}

#[test]
fn script_outcome_exit_semantics_pure() {
    let ok = ScriptOutcome {
        code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
    };
    assert!(!ok.is_blocking_exit());
    assert!(ok.json_block_reason().is_none());

    let blocked = ScriptOutcome {
        code: Some(2),
        stdout: String::new(),
        stderr: "lint failed".into(),
        timed_out: false,
    };
    assert!(blocked.is_blocking_exit());
    assert_eq!(blocked.block_text(), "lint failed");

    // exit 0 + JSON decision。
    let json_block = ScriptOutcome {
        code: Some(0),
        stdout: "{\"decision\":\"block\",\"reason\":\"bad\"}".into(),
        stderr: String::new(),
        timed_out: false,
    };
    assert_eq!(json_block.json_block_reason().as_deref(), Some("bad"));
    // stderr 空时回落 stdout。
    assert_eq!(json_block.block_text(), "{\"decision\":\"block\",\"reason\":\"bad\"}");

    // 超时 / 其他退出码 = 非阻断。
    let to = ScriptOutcome {
        code: None,
        stdout: String::new(),
        stderr: "hook timed out".into(),
        timed_out: true,
    };
    assert!(!to.is_blocking_exit());
}

// ---------------------------------------------------------------------------
// 桥 + 真子进程（CC 脚本执行协议）
// ---------------------------------------------------------------------------

fn bridge_with(json: &str, project_dir: &std::path::Path) -> CcHookBridge {
    CcHookBridge::from_json(json, project_dir.to_path_buf()).expect("bridge")
}

#[tokio::test]
async fn pre_tool_use_exit_2_blocks_with_stderr() {
    let tmp = tempdir();
    let b = bridge_with(
        &hooks_doc("PreToolUse", Some("Edit"), &block_cmd("lint failed: trailing whitespace"), None),
        &tmp,
    );
    let d = b.pre_tool_use(&edit_call("{}")).await;
    match d {
        crate::hooks::HookDecision::Block { reason } => {
            assert!(reason.contains("lint failed"), "reason={reason}");
        }
        other => panic!("expected Block, got {other:?}"),
    }
}

#[tokio::test]
async fn pre_tool_use_exit_0_allows() {
    let tmp = tempdir();
    let b = bridge_with(
        &hooks_doc("PreToolUse", Some("Edit"), allow_cmd(), None),
        &tmp,
    );
    assert_eq!(
        b.pre_tool_use(&edit_call("{}")).await,
        crate::hooks::HookDecision::Allow
    );
}

#[tokio::test]
async fn pre_tool_use_json_decision_blocks() {
    let tmp = tempdir();
    let b = bridge_with(
        &hooks_doc("PreToolUse", None, &json_block_cmd("lint fail"), None),
        &tmp,
    );
    match b.pre_tool_use(&edit_call("{}")).await {
        crate::hooks::HookDecision::Block { reason } => {
            assert!(reason.contains("lint fail"), "reason={reason}");
        }
        other => panic!("expected Block, got {other:?}"),
    }
}

#[tokio::test]
async fn pre_tool_use_timeout_is_non_blocking() {
    let tmp = tempdir();
    let b = bridge_with(
        &hooks_doc("PreToolUse", None, &slow_cmd(), Some(1)),
        &tmp,
    );
    // 超时（1s）→ 非阻断 → 放行。
    assert_eq!(
        b.pre_tool_use(&edit_call("{}")).await,
        crate::hooks::HookDecision::Allow
    );
}

#[tokio::test]
async fn pre_tool_use_non_matching_tool_skips_script() {
    let tmp = tempdir();
    // matcher 只挂 Write；edit 不命中 → 脚本不该跑（block 脚本存在也放行）。
    let b = bridge_with(
        &hooks_doc("PreToolUse", Some("Write"), &block_cmd("must not fire"), None),
        &tmp,
    );
    assert_eq!(
        b.pre_tool_use(&edit_call("{}")).await,
        crate::hooks::HookDecision::Allow
    );
}

#[tokio::test]
async fn post_tool_use_exit_2_appends_note() {
    let tmp = tempdir();
    let b = bridge_with(
        &hooks_doc("PostToolUse", Some("Edit"), &block_cmd("post-hint: run fmt"), None),
        &tmp,
    );
    match b.post_tool_use(&edit_call("{}"), "ok").await {
        crate::hooks::PostHookAction::Replace(r) => {
            assert!(r.starts_with("ok"), "original kept: {r}");
            assert!(r.contains("[hook]"), "note tagged: {r}");
            assert!(r.contains("post-hint"), "note body: {r}");
        }
        other => panic!("expected Replace, got {other:?}"),
    }
}

#[tokio::test]
async fn user_prompt_block_and_session_start_fires_once() {
    let tmp = tempdir();
    let start_marker = tmp.join("start.txt");
    let prompt_marker = tmp.join("prompt.txt");
    // 双事件文档（json! 转义路径反斜杠）。
    let doc = serde_json::json!({
        "hooks": {
            "SessionStart": [{ "hooks": [{ "command": append_cmd(&start_marker, "started") }] }],
            "UserPromptSubmit": [{ "hooks": [{ "command": block_cmd("no off-topic prompts") }] }],
        }
    });
    let _ = prompt_marker; // 计数对照位（本测试未挂 prompt 计数脚本）
    let b = bridge_with(&doc.to_string(), &tmp);
    let mk_prompt = || HookPrompt {
        session_key: "sess-1".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        prompt: "hello".to_string(),
    };
    // 第一条 prompt：SessionStart（写 marker）+ UserPromptSubmit（拦）。
    match b.on_user_prompt(&mk_prompt()).await {
        crate::hooks::PromptDecision::Block { reason } => {
            assert!(reason.contains("off-topic"), "reason={reason}");
        }
        other => panic!("expected Block, got {other:?}"),
    }
    // 第二条：SessionStart 不再跑（marker 仍 1 行）。
    assert!(matches!(
        b.on_user_prompt(&mk_prompt()).await,
        crate::hooks::PromptDecision::Block { .. }
    ));
    assert_eq!(marker_lines(&start_marker), 1, "SessionStart fires once");
    assert_eq!(marker_lines(&prompt_marker), 0, "no prompt marker configured");
}

#[tokio::test]
async fn stop_block_continue_then_flag_allows_and_resets() {
    let tmp = tempdir();
    let flag = tmp.join("flag.txt");
    let b = bridge_with(
        &hooks_doc("Stop", None, &stop_two_phase_cmd(&flag), None),
        &tmp,
    );
    let mk_end = |active: bool| HookTurnEnd {
        session_key: "sess-1".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        final_content: "done".to_string(),
        stop_hook_active: active,
    };
    // 第一次：无 flag → 拒停（feedback 带 stderr 文案）。
    match b.on_turn_end(&mk_end(false)).await {
        crate::hooks::TurnEndDecision::Continue { feedback } => {
            assert!(feedback.contains("more work"), "feedback={feedback}");
        }
        other => panic!("expected Continue, got {other:?}"),
    }
    assert!(b.stop_hook_active_for("sess-1"), "second stop sees active=true");
    // 落 flag → 下次放行 + 计数复位。
    std::fs::write(&flag, b"x").unwrap();
    assert_eq!(b.on_turn_end(&mk_end(true)).await, crate::hooks::TurnEndDecision::Stop);
    assert!(!b.stop_hook_active_for("sess-1"), "reset on accepted stop");
}

// ---------------------------------------------------------------------------
// AgentLoop 集成（我方会话真路径）
// ---------------------------------------------------------------------------

/// 记录型 provider（同 hooks/tests.rs 的共享态句柄模式）。
#[derive(Clone)]
struct Recorder(Arc<RecorderState>);

struct RecorderState {
    calls: std::sync::Mutex<Vec<Vec<LlmMessage>>>,
    script: std::sync::Mutex<Vec<LlmResponse>>,
}

struct RecordingProvider {
    state: Arc<RecorderState>,
}

fn recording_provider(script: Vec<LlmResponse>) -> (Box<RecordingProvider>, Recorder) {
    let state = Arc::new(RecorderState {
        calls: std::sync::Mutex::new(Vec::new()),
        script: std::sync::Mutex::new(script),
    });
    (
        Box::new(RecordingProvider {
            state: state.clone(),
        }),
        Recorder(state),
    )
}

impl Recorder {
    fn calls(&self) -> std::sync::MutexGuard<'_, Vec<Vec<LlmMessage>>> {
        self.0.calls.lock().unwrap()
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.state.calls.lock().unwrap().push(messages);
        let mut script = self.state.script.lock().unwrap();
        Ok(if script.is_empty() {
            scripted_resp("script-exhausted")
        } else {
            script.remove(0)
        })
    }
}

fn scripted_resp(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

struct MarkerTool;

#[async_trait]
impl Tool for MarkerTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        Ok("tool-ok".to_string())
    }
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["marker".to_string()],
        models: std::collections::HashMap::new(),
    }
}

fn test_context() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "session1")
}

fn first_done(events: &[crate::types::AgentEvent]) -> String {
    events
        .iter()
        .find_map(|e| match e {
            crate::types::AgentEvent::Done(msg) => Some(msg.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// 验收（U14 原文「现有 CC hook 脚本在我方会话真实触发」）：**真·CC 形态**
/// lint-on-edit 脚本——磁盘上的 python 脚本文件 + hooks.json 按路径引用 +
/// 脚本自己读 stdin JSON、取 `tool_input.file_path`（依赖我们的别名增补）、
/// 对受保护文件 exit 2 + stderr。放行文件 exit 0。与现有 CC 生态脚本同形，
/// 不是合成 echo 单行。python 缺席的环境跳过（早退 + 输出说明）。
#[tokio::test]
async fn loop_integration_real_lint_script_blocks_protected_file() {
    let py = if cfg!(windows) { "python" } else { "python3" };
    if std::process::Command::new(py)
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping: {py} not available on this machine");
        return;
    }
    let tmp = tempdir();
    // 真 lint 脚本：读 stdin JSON（CC 协议），lint 规则 = bad.rs 受保护。
    let script = tmp.join("cc_lint.py");
    std::fs::write(
        &script,
        r#"import json, sys
d = json.load(sys.stdin)
fp = d.get("tool_input", {}).get("file_path", "")
if fp.endswith("bad.rs"):
    print("lint error: bad.rs is protected (edit denied)", file=sys.stderr)
    sys.exit(2)
sys.exit(0)
"#,
    )
    .unwrap();
    let cmd = format!("{py} {}", script.to_string_lossy());
    let bridge = Arc::new(
        CcHookBridge::from_json(
            &hooks_doc("PreToolUse", Some("Edit"), &cmd, Some(30)),
            tmp.clone(),
        )
        .expect("bridge"),
    );
    let mut lp = AgentLoop::new(Box::new(NoopProvider2), test_config());
    lp.register_tool("edit".to_string(), Box::new(MarkerTool));
    bridge.register(&lp);

    let call = |file: &str| {
        ToolCallInfo {
            id: format!("call-{file}"),
            name: "edit".to_string(),
            arguments: format!(r#"{{"path":"{file}"}}"#),
        }
    };
    // 放行文件：脚本 exit 0 → 工具真执行，返回工具本体结果。
    let ok = lp.handle_tool_call(&call("src/good.rs"), &test_context()).await;
    assert_eq!(ok, "tool-ok", "allowed edit must run the tool body");
    // 受保护文件：脚本 exit 2 + stderr → 拦停，工具不执行。
    let blocked = lp.handle_tool_call(&call("src/bad.rs"), &test_context()).await;
    assert!(blocked.contains("⛔ HOOK BLOCKED"), "blocked={blocked}");
    assert!(blocked.contains("bad.rs is protected"), "blocked={blocked}");
}

/// 验收主路径：CC 格式 PreToolUse 脚本经 AgentLoop 真 dispatch 拦下工具。
#[tokio::test]
async fn loop_integration_pre_tool_use_blocks_dispatch() {
    let tmp = tempdir();
    let bridge = Arc::new(
        CcHookBridge::from_json(
            &hooks_doc("PreToolUse", Some("Edit"), &block_cmd("lint failed: fix it"), None),
            tmp.clone(),
        )
        .expect("bridge"),
    );
    let mut lp = AgentLoop::new(Box::new(NoopProvider2), test_config());
    lp.register_tool("edit".to_string(), Box::new(MarkerTool));
    bridge.register(&lp);

    let result = lp
        .handle_tool_call(
            &ToolCallInfo {
                id: "c1".to_string(),
                name: "edit".to_string(),
                arguments: r#"{"path":"a.rs"}"#.to_string(),
            },
            &test_context(),
        )
        .await;
    assert!(result.contains("⛔ HOOK BLOCKED"), "result={result}");
    assert!(result.contains("lint failed"), "result={result}");
}

/// UserPromptSubmit exit 2 → prompt 被拦，LLM 从未被调。
#[tokio::test]
async fn loop_integration_prompt_block_aborts_before_llm() {
    let tmp = tempdir();
    let bridge = Arc::new(
        CcHookBridge::from_json(
            &hooks_doc("UserPromptSubmit", None, &block_cmd("prompt denied by policy"), None),
            tmp,
        )
        .expect("bridge"),
    );
    let (provider, recorder) = recording_provider(vec![scripted_resp("never")]);
    let lp = AgentLoop::new(provider, test_config());
    bridge.register(&lp);

    let instance = AgentInstance::new(test_config());
    let events = lp.run(&instance, "do something bad", &test_context()).await;

    let done = first_done(&events);
    assert!(done.contains("⛔ HOOK BLOCKED"), "done={done}");
    assert!(done.contains("prompt denied"), "done={done}");
    assert!(recorder.calls().is_empty(), "LLM must not be called");
    // 拦下的 prompt 不进 history（CC 语义：模型永远看不到）。
    let hist = instance.get_history();
    assert!(
        hist.iter().all(|m| !m.content.contains("do something bad")),
        "blocked prompt must not enter history"
    );
}

/// Stop exit 2 → 拒停：feedback 进 user 消息、模型再答一轮；轮次预算
/// （MAX_TURN_END_CONTINUES=2）耗尽后 fail-open 停在最后一个响应上。
#[tokio::test]
async fn loop_integration_stop_block_forces_extra_rounds() {
    let tmp = tempdir();
    let bridge = Arc::new(
        CcHookBridge::from_json(
            &hooks_doc("Stop", None, &block_cmd("more work to do"), None),
            tmp,
        )
        .expect("bridge"),
    );
    // r1 → Stop 拒停 → r2 → Stop 拒停 → r3 → Stop 拒停但预算（2）耗尽 →
    // fail-open 停在 r3。共 1 + 2 次 LLM 调用。
    let (provider, recorder) = recording_provider(vec![
        scripted_resp("r1"),
        scripted_resp("r2"),
        scripted_resp("r3"),
    ]);
    let lp = AgentLoop::new(provider, test_config());
    bridge.register(&lp);

    let instance = AgentInstance::new(test_config());
    let events = lp.run(&instance, "work", &test_context()).await;

    assert_eq!(first_done(&events), "r3", "fail-open stops on last response");
    let calls = recorder.calls();
    assert_eq!(
        calls.len(),
        1 + crate::hooks::MAX_TURN_END_CONTINUES as usize,
        "initial + budget-forced rounds, no more"
    );
    let second = &calls[1];
    assert!(
        second
            .iter()
            .any(|m| m.role == "user" && m.content.contains("more work to do")),
        "hook feedback must reach the model as a user message"
    );
}

// handle_tool_call 集成需要一个 provider（AgentLoop 构造要求），但不会被调。
struct NoopProvider2;

#[async_trait]
impl LlmProvider for NoopProvider2 {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("NoopProvider2 must not be called".to_string())
    }
}
