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
            // C8 (2026-09-06): run_checks spawns cargo/npm/go — same write
            // surface as exec (target/, node_modules/).
            "run_checks",
            "read_file",
            "write_file",
            "list_dir",
            "edit_file",
            // A7 (2026-09-06): batch edit_file — contained identically.
            "multiedit",
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
    // 本地化代码页变体（2026-09-17 实测）：中文 Windows 的 banner 含中文
    // 字符（"版本"等），cmd.exe 以 OEM 代码页 CP936/GBK 写管道——严格
    // UTF-8 解码（BufReader::lines）失败 → "valid UTF-8" 错误臂。协议是
    // UTF-8 JSONL，非 UTF-8 输出报错是正确行为；英文 Windows（ASCII
    // banner）走 parse 臂，两种环境都在接受集合内。
    assert!(
        err.contains("parse executor response")
            || err.contains("exited without a response")
            || err.contains("timed out")
            || err.contains("valid UTF-8"),
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

// P24（2026-09-26）：Windows 盒缺位时的用户态 ACL 降级通道判定。
#[cfg(windows)]
#[test]
fn p24_userland_fallback_channel_selection() {
    // 未注入 fallback → 恒管道（现状字节不变）。
    let bare = ExecutorChannel::new(
        PathBuf::from("/x/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| false),
    );
    assert!(!bare.picks_stdio_userland_fallback());

    // fallback 判定 false（如选型指向 sandboxie / ACL 不可用）→ 管道。
    let declined = ExecutorChannel::new(
        PathBuf::from("/x/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| false),
    )
    .with_userland_fallback(Arc::new(|| false));
    assert!(!declined.picks_stdio_userland_fallback());

    // 无盒 wrap + fallback 判定 true → stdio + 用户态标记（子进程自装 ACL）。
    let mut engaged = ExecutorChannel::new(
        PathBuf::from("/x/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| false),
    )
    .with_userland_fallback(Arc::new(|| true));
    assert!(engaged.picks_stdio_userland_fallback());

    // 盒 wrap 在场时判定短路（Start.exe 通道不降级）——手动置 start_exe
    // 模拟 L2.2 构造（with_start_exe 是 dead_code 标注的既有 builder）。
    engaged.start_exe = Some(PathBuf::from("/x/Start.exe"));
    assert!(!engaged.picks_stdio_userland_fallback());
}

// ---------------------------------------------------------------------------
// P3 T3：persistent env 白名单 + home 重定向（纯函数，无进程 env 竞态）
// ---------------------------------------------------------------------------

#[test]
fn allowlist_covers_runtime_baseline_and_blocks_credentials() {
    // 基线在表内（路径/系统/身份/临时/locale）。
    for k in [
        "PATH",
        "SYSTEMROOT",
        "COMSPEC",
        "USERNAME",
        "COMPUTERNAME",
        "TEMP",
        "LANG",
    ] {
        assert!(env_allowlisted(k), "{k} 应在白名单内");
    }
    // 凭据/令牌类一律不在表内——白名单语义即闸。
    for k in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "GITHUB_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "MY_SERVICE_SECRET",
    ] {
        assert!(!env_allowlisted(k), "{k} 不得进白名单");
    }
    // NEMESISBOT_* 配置面（含 HOME）必须剥——workspace/home/role 由 spawn
    // 侧显式注入，宿主语义对 agent home 是错的。
    for k in [
        "NEMESISBOT_HOME",
        "NEMESISBOT_EXECUTOR_WORKSPACE",
        "NEMESISBOT_ROLE",
        "NEMESISBOT_EXECUTOR_AGENT_ID",
    ] {
        assert!(!env_allowlisted(k), "{k} 必须由显式注入而非继承");
    }
    // home 族不继承（persistent 重定向到 agent home）。
    for k in [
        "USERPROFILE",
        "HOME",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
    ] {
        assert!(!env_allowlisted(k), "{k} 走重定向不走继承");
    }
}

#[test]
#[cfg(windows)]
fn allowlist_match_is_case_insensitive_on_windows() {
    assert!(env_allowlisted("Path"));
    assert!(env_allowlisted("path"));
    assert!(env_allowlisted("SystemRoot"));
    assert!(!env_allowlisted("PATH_X"));
}

#[test]
fn filter_env_allowlist_keeps_baseline_drops_the_rest() {
    let host = vec![
        ("PATH".to_string(), "/bin".to_string()),
        ("SYSTEMROOT".to_string(), r"C:\Windows".to_string()),
        ("OPENAI_API_KEY".to_string(), "sk-leak".to_string()),
        ("NEMESISBOT_HOME".to_string(), "/host/ws".to_string()),
        ("APPDATA".to_string(), r"C:\Users\host\AppData".to_string()),
        ("TERM".to_string(), "xterm".to_string()),
    ];
    let kept = filter_env_allowlist(host);
    let names: Vec<&str> = kept.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        names,
        vec!["PATH", "SYSTEMROOT", "TERM"],
        "顺序保持且只留基线"
    );
    assert!(!serde_json::to_string(&kept).unwrap().contains("sk-leak"));
}

#[test]
fn home_redirect_points_every_home_family_var_into_agent_home() {
    let home = Path::new("/agent-homes/a1");
    let pairs = home_redirect_env_pairs(home);
    let get = |k: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
    };
    // HOME 恒在（两平台）且直指 agent home。
    assert_eq!(get("HOME").as_deref(), Some("/agent-homes/a1"));
    #[cfg(windows)]
    {
        assert_eq!(get("USERPROFILE").as_deref(), Some("/agent-homes/a1"));
        assert_eq!(
            get("APPDATA").as_deref(),
            Some(r"/agent-homes/a1\AppData/Roaming")
        );
        assert_eq!(
            get("LOCALAPPDATA").as_deref(),
            Some(r"/agent-homes/a1\AppData/Local")
        );
    }
    #[cfg(not(windows))]
    {
        assert_eq!(
            get("XDG_CONFIG_HOME").as_deref(),
            Some("/agent-homes/a1/.config")
        );
        assert_eq!(
            get("XDG_DATA_HOME").as_deref(),
            Some("/agent-homes/a1/.local/share")
        );
        assert_eq!(
            get("XDG_CACHE_HOME").as_deref(),
            Some("/agent-homes/a1/.cache")
        );
    }
}

// ---------------------------------------------------------------------------
// P3 T7：常驻路由四臂（mock executor，无子进程）。回落锚 = 通道 exe 不存在
// （回落必产 "executor unavailable: … failed to spawn" 形态）；错误分型纪律
// 的行为面钉死——Ok 短路 / ToolFailed 原样上抛不回落 / Unavailable 回落 /
// resolve=None 不触碰常驻执行体。
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicUsize, Ordering};

struct MockExecutor {
    calls: AtomicUsize,
    /// Some = 首调按配置回分型错误；None = 回 Ok("persistent-ok")。
    reply: Mutex<Option<Result<String, PersistentCallError>>>,
    /// 收到的调用参数透传锚：(agent_id, exec_root, tool)。
    seen: Mutex<Vec<(String, String, String)>>,
}

#[async_trait::async_trait]
impl PersistentExecutor for MockExecutor {
    async fn execute_persistent(
        &self,
        agent_id: &str,
        exec_root: &std::path::Path,
        tool: &str,
        _args: &str,
        _context: &RequestContext,
    ) -> Result<String, PersistentCallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push((
            agent_id.to_string(),
            exec_root.to_string_lossy().to_string(),
            tool.to_string(),
        ));
        match self.reply.lock().unwrap().take() {
            Some(r) => r,
            None => Ok("persistent-ok".to_string()),
        }
    }
}

fn mock_executor(reply: Option<Result<String, PersistentCallError>>) -> Arc<MockExecutor> {
    Arc::new(MockExecutor {
        calls: AtomicUsize::new(0),
        reply: Mutex::new(reply),
        seen: Mutex::new(Vec::new()),
    })
}

fn bogus_channel() -> Arc<ExecutorChannel> {
    Arc::new(
        ExecutorChannel::new(
            PathBuf::from("/definitely/not/a/real/nemesisbot.exe"),
            "/ws".into(),
            Arc::new(|| false),
        )
        .with_timeout(Duration::from_secs(5)),
    )
}

fn routing_tool(
    resolve: Option<(String, PathBuf)>,
    executor: Arc<MockExecutor>,
) -> RemoteExecutorTool {
    let local = RecordingTool {
        ctx: Mutex::new((String::new(), String::new())),
    };
    RemoteExecutorTool::new("exec".to_string(), Box::new(local), bogus_channel()).with_persistent(
        PersistentRoute {
            resolve: Arc::new(move || resolve.clone()),
            executor,
        },
    )
}

#[tokio::test]
async fn persistent_route_ok_short_circuits_channel() {
    let executor = mock_executor(None);
    let tool = routing_tool(
        Some(("ag-1".to_string(), PathBuf::from("/root/ws"))),
        executor.clone(),
    );
    let out = tool.execute("{}", &req_ctx()).await.expect("persistent ok");
    assert_eq!(out, "persistent-ok", "常驻结果直接返回");
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    let seen = executor.seen.lock().unwrap();
    assert_eq!(seen[0].0, "ag-1", "agent_id 透传");
    assert_eq!(seen[0].1, "/root/ws", "exec_root 透传");
    assert_eq!(seen[0].2, "exec", "工具名透传");
}

#[tokio::test]
async fn persistent_route_tool_failed_propagates_without_fallback() {
    let executor = mock_executor(Some(Err(PersistentCallError::ToolFailed(
        "side effect happened".into(),
    ))));
    let tool = routing_tool(
        Some(("ag-1".to_string(), PathBuf::from("/root/ws"))),
        executor.clone(),
    );
    let err = tool
        .execute("{}", &req_ctx())
        .await
        .expect_err("ToolFailed 必须上抛");
    assert_eq!(err, "side effect happened", "原样上抛——回落会造成重复执行");
    assert!(!err.contains("failed to spawn"), "绝不能走到 per-call 通道");
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn persistent_route_unavailable_falls_back_to_percall() {
    // host_fallback=true（process 档槽满等宿主同视图形态）→ 走既有回落。
    let executor = mock_executor(Some(Err(PersistentCallError::Unavailable {
        message: "slots full".into(),
        host_fallback: true,
    })));
    let tool = routing_tool(
        Some(("ag-1".to_string(), PathBuf::from("/root/ws"))),
        executor.clone(),
    );
    let err = tool
        .execute("{}", &req_ctx())
        .await
        .expect_err("回落通道 exe 不存在，必产错误");
    assert!(
        err.starts_with("executor unavailable:") && err.contains("failed to spawn"),
        "Unavailable(host_fallback=true) 必须回落到 per-call 通道: {err}"
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
}

// R1 回归锁：host_fallback=false（vm 档 guest 异文件视图）→ 诚实错误上抛，
// 绝不 spawn 宿主执行器（通道 exe 不存在——被触碰即败形可辨）。
#[tokio::test]
async fn persistent_route_unavailable_no_host_fallback_propagates_honest_error() {
    let executor = mock_executor(Some(Err(PersistentCallError::Unavailable {
        message: "vm image missing".into(),
        host_fallback: false,
    })));
    let tool = routing_tool(
        Some(("ag-1".to_string(), PathBuf::from("/root/ws"))),
        executor.clone(),
    );
    let err = tool
        .execute("{}", &req_ctx())
        .await
        .expect_err("host_fallback=false 必须上抛，不得回落");
    assert!(
        err.contains("host fallback forbidden") && err.contains("不回落宿主"),
        "错误须含回落禁用契约文案: {err}"
    );
    assert!(
        err.contains("vm image missing"),
        "根因（执行体给的 message）必须保留在错误里: {err}"
    );
    assert!(
        !err.contains("failed to spawn"),
        "per-call 通道必须一次都未被触碰: {err}"
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
}

// F1 回归锁（原三轮复评诊断探针翻转，2026-10-21）：LifecycleRejected（身份
// 已删除/注销/代次失效）→ 无论后端档位一律终止本次调用，绝不 spawn 宿主
// 执行器。旧行为：process 桥把删除拒绝混进 Unavailable(host_fallback=true)，
// 工具经 per-call 照常执行（已删身份的工具在宿主成功跑完 = 事故面）。
#[tokio::test]
async fn persistent_route_lifecycle_rejected_terminates_without_fallback() {
    let executor = mock_executor(Some(Err(PersistentCallError::LifecycleRejected(
        "agent is being removed".into(),
    ))));
    let tool = routing_tool(
        Some(("ag-1".to_string(), PathBuf::from("/root/ws"))),
        executor.clone(),
    );
    let err = tool
        .execute("{}", &req_ctx())
        .await
        .expect_err("LifecycleRejected 必须上抛，不得回落");
    assert!(
        err.contains("lifecycle rejected"),
        "错误须表达生命周期拒绝: {err}"
    );
    assert!(
        err.contains("agent is being removed"),
        "根因（supervisor 给的 message）必须保留: {err}"
    );
    assert!(
        !err.contains("failed to spawn"),
        "per-call 通道必须一次都未被触碰: {err}"
    );
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn persistent_route_resolve_none_skips_executor_and_falls_back() {
    let executor = mock_executor(None);
    let tool = routing_tool(None, executor.clone());
    let err = tool
        .execute("{}", &req_ctx())
        .await
        .expect_err("回落通道 exe 不存在，必产错误");
    assert!(
        err.starts_with("executor unavailable:") && err.contains("failed to spawn"),
        "resolve=None 必须直接走 per-call: {err}"
    );
    assert_eq!(
        executor.calls.load(Ordering::SeqCst),
        0,
        "实体未就绪 = 常驻执行体一次都不被触碰"
    );
}
