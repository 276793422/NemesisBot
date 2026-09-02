use super::*;

#[test]
fn build_command_no_start_exe_is_direct_spawn() {
    let ch = ExecutorChannel::new(
        PathBuf::from("/x/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| false),
    );
    assert!(ch.start_exe.is_none());
    // No error — direct spawn command is built.
    let _ = ch.build_command();
}

#[test]
fn build_command_with_start_exe_wraps() {
    let ch = ExecutorChannel::new(
        PathBuf::from("/x/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| true),
    )
    .with_start_exe(PathBuf::from("/x/Start.exe"));
    assert!(ch.start_exe.is_some());
    // No error — Start.exe wrap command is built (L2.2 form).
    let _ = ch.build_command();
}

#[test]
fn move_tools_is_the_expected_set() {
    assert_eq!(
        MOVE_TOOLS,
        &[
            "exec",
            "run_script",
            "read_file",
            "write_file",
            "list_dir",
            "edit_file",
            "append_file",
            "delete_file",
            "create_dir",
            "delete_dir",
            "grep",
            "git",
        ]
    );
}

// ---------------------------------------------------------------------------
// P5-2 严格模式闸门：fail-closed 拒绝发生在 spawn 之前
// ---------------------------------------------------------------------------

use std::time::Duration;

use crate::context::RequestContext;

/// 通道带 sandbox_probe=true + 严格闸门。exe 指向不存在的路径——若闸门
/// 失效放行到 spawn，错误信息是 "failed to spawn"（可据此区分拒绝对顺序）。
fn strict_test_channel(gate_result: Result<(), String>) -> ExecutorChannel {
    ExecutorChannel::new(
        PathBuf::from("/definitely/not/a/real/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| true),
    )
    .with_timeout(Duration::from_secs(5))
    .with_strict_gate(Arc::new(move || gate_result.clone()))
}

fn req_ctx() -> RequestContext {
    RequestContext::new("web", "test-chat", "test-session", "/ws")
}

#[tokio::test]
async fn strict_gate_refuses_sandboxed_call_before_spawn() {
    let ch = strict_test_channel(Err("Sandboxie engine not ready".into()));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("gate must refuse");
    assert!(err.contains("strict mode"), "err: {err}");
    assert!(err.contains("Sandboxie engine not ready"), "err: {err}");
    // 拒绝发生在 spawn 之前：不是子进程 spawn 失败的错误。
    assert!(
        !err.contains("failed to spawn"),
        "refusal must precede spawn: {err}"
    );
}

#[tokio::test]
async fn strict_gate_pass_proceeds_to_spawn() {
    // 闸门 Ok → 放行到正常 spawn 路径（exe 不存在 → spawn 失败，证明顺序）。
    let ch = strict_test_channel(Ok(()));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("bogus exe must fail");
    assert!(!err.contains("strict mode"), "gate passed: {err}");
}

/// 默认（未注入闸门）行为与改动前逐字节一致：probe=true 直接进 spawn 路径。
#[tokio::test]
async fn no_gate_keeps_default_fail_open_path() {
    let ch = ExecutorChannel::new(
        PathBuf::from("/definitely/not/a/real/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| true),
    )
    .with_timeout(Duration::from_secs(5));
    assert!(ch.strict_gate.is_none(), "default construction has no gate");
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("bogus exe must fail");
    assert!(!err.contains("strict mode"), "no gate = no refusal: {err}");
}

// --- W3a: builders、parse_response、build_request_line、stdio 传输、
// RemoteExecutorTool 委托 ---

#[test]
fn builders_set_fields() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ch = ExecutorChannel::new(
        PathBuf::from("/x/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| false),
    )
    .with_home(tmp.path().to_path_buf())
    .with_start_exe(PathBuf::from("/x/Start.exe"))
    .with_timeout(Duration::from_secs(7));
    assert_eq!(ch.home.as_deref(), Some(tmp.path()));
    assert_eq!(
        ch.start_exe.as_deref(),
        Some(std::path::Path::new("/x/Start.exe"))
    );
    assert_eq!(ch.timeout, Duration::from_secs(7));
    assert_eq!(ch.box_name, "NemesisBox");
}

#[test]
fn parse_response_arms() {
    assert_eq!(
        ExecutorChannel::parse_response(r#"{"ok":true,"result":"hi"}"#),
        Ok("hi".to_string())
    );
    // ok=true 且缺 result → serde default 空串
    assert_eq!(
        ExecutorChannel::parse_response(r#"{"ok":true}"#),
        Ok(String::new())
    );
    assert_eq!(
        ExecutorChannel::parse_response(r#"{"ok":false,"error":"boom"}"#),
        Err("boom".to_string())
    );
    // ok=false 且缺 error → 兜底文案
    assert_eq!(
        ExecutorChannel::parse_response(r#"{"ok":false}"#),
        Err("executor returned an error".to_string())
    );
    let err = ExecutorChannel::parse_response("not json").unwrap_err();
    assert!(err.starts_with("parse executor response"), "err: {err}");
}

#[test]
fn build_request_line_is_newline_terminated_jsonl() {
    let ch = ExecutorChannel::new(
        PathBuf::from("/x/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| false),
    );
    let line = ch
        .build_request_line("exec", r#"{"command":"dir"}"#, &req_ctx())
        .expect("serialize ok");
    assert!(line.ends_with('\n'), "line: {line}");
    let parsed: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(parsed["tool"], "exec");
    assert_eq!(parsed["args"], r#"{"command":"dir"}"#);
    assert!(parsed["context"].is_object(), "context serialized");
}

/// stdio 传输：子进程读完 stdin 后退出、stdout 无输出 →
/// "exited without a response" 错误臂（覆盖 stdin 写入、stdout take、
/// drain_stderr、child.wait）。
#[cfg(windows)]
#[tokio::test]
async fn stdio_transport_child_exits_without_response() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cmd = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
    let ch = ExecutorChannel::new(PathBuf::from(&cmd), "/ws".into(), Arc::new(|| false))
        .with_home(tmp.path().to_path_buf()) // 顺带覆盖 home env 分支
        .with_timeout(Duration::from_secs(15));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("cmd.exe cannot answer the protocol");
    // cmd.exe 无参启动会对管道 stdin 打交互 banner（"Microsoft Windows
    // [Version ...]"）到 stdout → 第一行非 JSON → parse 失败臂（稳定）。
    // 极端环境（AutoRun /q 抑制 banner）下落到无响应/超时臂，也一并接受。
    assert!(
        err.contains("parse executor response")
            || err.contains("exited without a response")
            || err.contains("timed out"),
        "err: {err}"
    );
}

/// stdio 传输：子进程把请求行原样回显（sort.exe 单行排序=原行）→
/// 响应 JSON 缺 `ok` 字段 → parse 失败臂。
#[cfg(windows)]
#[tokio::test]
async fn stdio_transport_echo_child_hits_parse_error() {
    let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    let sort = PathBuf::from(sysroot).join("System32").join("sort.exe");
    if !sort.exists() {
        eprintln!("skip: sort.exe not found at {sort:?}");
        return;
    }
    let ch = ExecutorChannel::new(sort, "/ws".into(), Arc::new(|| false))
        .with_timeout(Duration::from_secs(15));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("echoed request is not a valid response");
    assert!(err.contains("parse executor response"), "err: {err}");
}

/// 非 Windows：cat 回显请求行 → parse 失败臂。
#[cfg(not(windows))]
#[tokio::test]
async fn stdio_transport_echo_child_hits_parse_error() {
    let cat = PathBuf::from("/bin/cat");
    if !cat.exists() {
        eprintln!("skip: /bin/cat not found");
        return;
    }
    let ch = ExecutorChannel::new(cat, "/ws".into(), Arc::new(|| false))
        .with_timeout(Duration::from_secs(15));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("echoed request is not a valid response");
    assert!(err.contains("parse executor response"), "err: {err}");
}

/// 非 Windows：true 立即退出 → "exited without a response"。
#[cfg(not(windows))]
#[tokio::test]
async fn stdio_transport_child_exits_without_response() {
    let ch = ExecutorChannel::new(PathBuf::from("/bin/true"), "/ws".into(), Arc::new(|| false))
        .with_timeout(Duration::from_secs(15));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("true produces no output");
    assert!(err.contains("exited without a response"), "err: {err}");
}

// --- RemoteExecutorTool 委托 ---

use crate::r#loop::{FileChange, FileChangeKind, Tool};
use std::sync::Mutex;

struct RecordingTool {
    ctx: Mutex<(String, String)>,
}

#[async_trait::async_trait]
impl Tool for RecordingTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("local result".to_string())
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        *self.ctx.lock().unwrap() = (channel.to_string(), chat_id.to_string());
    }

    fn description(&self) -> String {
        "recording tool desc".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}})
    }

    fn preview(&self, _args: &str) -> Option<FileChange> {
        Some(FileChange {
            path: "a.txt".to_string(),
            kind: FileChangeKind::Create,
        })
    }
}

#[test]
fn remote_tool_delegates_metadata_and_context() {
    let local = RecordingTool {
        ctx: Mutex::new((String::new(), String::new())),
    };
    let ch = Arc::new(
        ExecutorChannel::new(
            PathBuf::from("/definitely/not/a/real/nemesisbot.exe"),
            "/ws".into(),
            Arc::new(|| false),
        )
        .with_timeout(Duration::from_secs(5)),
    );
    let remote = RemoteExecutorTool::new("exec".to_string(), Box::new(local), ch);

    // 元数据全部委托本地实现（LLM 看到同源 schema）。
    assert_eq!(remote.description(), "recording tool desc");
    assert_eq!(
        remote.parameters(),
        serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}})
    );
    let pv = remote.preview("{}").expect("preview delegated");
    assert_eq!(pv.path, "a.txt");

    // set_context 转发到本地工具。
    remote.set_context("web", "chat-9");
    // 取回 local 的记录：RemoteExecutorTool 持有 Box，无法直接读——
    // 通过再包一层共享状态验证。
}

#[test]
fn remote_tool_set_context_reaches_local_impl() {
    // 用共享 Arc 验证 set_context 穿透 Box<dyn Tool>。
    use std::sync::atomic::{AtomicBool, Ordering};
    struct FlagTool {
        got: Arc<AtomicBool>,
    }
    #[async_trait::async_trait]
    impl Tool for FlagTool {
        async fn execute(&self, _a: &str, _c: &RequestContext) -> Result<String, String> {
            Ok(String::new())
        }
        fn set_context(&self, _ch: &str, _id: &str) {
            self.got.store(true, Ordering::SeqCst);
        }
    }
    let got = Arc::new(AtomicBool::new(false));
    let ch = Arc::new(ExecutorChannel::new(
        PathBuf::from("/x/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| false),
    ));
    let remote = RemoteExecutorTool::new(
        "exec".to_string(),
        Box::new(FlagTool { got: got.clone() }),
        ch,
    );
    assert!(!got.load(Ordering::SeqCst));
    remote.set_context("web", "c1");
    assert!(got.load(Ordering::SeqCst), "set_context must be forwarded");
}

#[tokio::test]
async fn remote_tool_execute_wraps_channel_error() {
    // exe 不存在 → spawn 失败 → execute 映射成 "executor unavailable: ..."。
    struct NopTool;
    #[async_trait::async_trait]
    impl Tool for NopTool {
        async fn execute(&self, _a: &str, _c: &RequestContext) -> Result<String, String> {
            Ok(String::new())
        }
    }
    let ch = Arc::new(
        ExecutorChannel::new(
            PathBuf::from("/definitely/not/a/real/nemesisbot.exe"),
            "/ws".into(),
            Arc::new(|| false),
        )
        .with_timeout(Duration::from_secs(5)),
    );
    let remote = RemoteExecutorTool::new("exec".to_string(), Box::new(NopTool), ch);
    let err = remote
        .execute("{}", &req_ctx())
        .await
        .expect_err("bogus exe must fail");
    assert!(err.starts_with("executor unavailable:"), "err: {err}");
    assert!(err.contains("failed to spawn"), "err: {err}");
}
