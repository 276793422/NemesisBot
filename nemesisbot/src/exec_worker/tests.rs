//! exec_worker 测试：U11 用户态沙盒路径决策表（纯函数，跨平台）。
//! 真实行为断言（landlock 写外拒/写内放行）在 `tests/executor.rs` 的
//! Linux e2e（cfg linux）里。

// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;

#[cfg(feature = "sandbox")]
mod userland_plan {
    use super::userland::{Plan, plan};
    use nemesis_sandbox::backend::BackendForm;

    #[test]
    fn no_marker_runs_plain() {
        assert_eq!(
            plan(false, false, Some(BackendForm::SelfApply)),
            Plan::Plain
        );
        // 即使 bwrap 可用，没标记也不介入
        assert_eq!(
            plan(false, false, Some(BackendForm::WrapCommand)),
            Plan::Plain
        );
    }

    #[test]
    fn reexeced_instance_never_re_engages() {
        // 防环核心：盒内实例（REEXEC=1）见到标记也直接 Plain
        assert_eq!(plan(true, true, Some(BackendForm::SelfApply)), Plan::Plain);
        assert_eq!(
            plan(true, true, Some(BackendForm::WrapCommand)),
            Plan::Plain
        );
    }

    #[test]
    fn marker_with_no_backend_degrades_plain() {
        // 降级验收语义：无后端 → Plain（warn 由 engage 打），不 Err
        assert_eq!(plan(true, false, None), Plan::Plain);
    }

    #[test]
    fn marker_with_self_apply_backend_applies() {
        assert_eq!(
            plan(true, false, Some(BackendForm::SelfApply)),
            Plan::SelfApply
        );
    }

    #[test]
    fn marker_with_wrap_backend_reexeces() {
        assert_eq!(
            plan(true, false, Some(BackendForm::WrapCommand)),
            Plan::WrapReexec
        );
    }
}

// ---------------------------------------------------------------------------
// P5-2：engage 的严格模式改判（fail-closed）与默认 fail-open
// ---------------------------------------------------------------------------
// Windows 上 detect_backend() 恒 None（设计契约，Sandboxie 承担）→ engage
// 确定走 Plain 路径，strict 的 bail/不 bail 可确定性断言。**必须** cfg
// windows：Linux 上 detect_backend 可能返回 Some → SelfApply → engage 会
// 真对测试进程装 landlock（不可逆，污染同进程其他测试）。
#[cfg(all(feature = "sandbox", windows))]
mod engage_strict {
    use super::userland::{Outcome, engage};

    fn seed_home(strict: Option<bool>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = match strict {
            None => r#"{ "executor": { "enabled": true, "sandbox": true } }"#.to_string(),
            Some(s) => format!(
                r#"{{ "executor": {{ "enabled": true, "sandbox": true, "strict": {s} }} }}"#
            ),
        };
        std::fs::write(dir.path().join("config.json"), body).expect("seed config.json");
        dir
    }

    #[test]
    fn strict_refuses_when_no_backend() {
        let home = seed_home(Some(true));
        let err = engage("/ws", Some(home.path())).expect_err("strict must refuse");
        assert!(err.to_string().contains("strict mode"), "err: {err}");
        assert!(
            err.to_string().contains("refusing to run unsandboxed"),
            "err: {err}"
        );
    }

    #[test]
    fn no_strict_config_keeps_fail_open() {
        // 缺 strict 键（= 默认 false）：无后端 → warn + Continue（现状字节不变）
        let home = seed_home(None);
        assert!(matches!(
            engage("/ws", Some(home.path())),
            Ok(Outcome::Continue)
        ));
    }

    #[test]
    fn strict_false_keeps_fail_open() {
        let home = seed_home(Some(false));
        assert!(matches!(
            engage("/ws", Some(home.path())),
            Ok(Outcome::Continue)
        ));
    }

    #[test]
    fn no_home_defaults_fail_open() {
        // home=None（测试/裸构造）→ strict 读不到 → false → fail-open
        assert!(matches!(engage("/ws", None), Ok(Outcome::Continue)));
    }
}

// ---------------------------------------------------------------------------
// dispatch 协议分发（一行 JSON 请求 → ExecutorResponse）——纯逻辑 + mock 工具，
// 不 spawn 任何进程。stdio/pipe 两个 transport loop 只是对 dispatch 的包装，
// 它们本身需要真 stdio/管道（结构性，见 tests/executor.rs 的真链路 e2e）。
// ---------------------------------------------------------------------------
mod dispatch_protocol {
    use super::*;
    use nemesis_agent::context::RequestContext;
    use nemesis_agent::r#loop::Tool;
    use std::collections::HashMap;

    /// 恒 Ok 的哑工具：回显 args + 记录收到的 args/ctx 供透传断言。
    struct EchoTool {
        seen: std::sync::Mutex<Vec<(String, String, String, String)>>,
    }

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        async fn execute(&self, args: &str, ctx: &RequestContext) -> Result<String, String> {
            self.seen.lock().unwrap().push((
                args.to_string(),
                ctx.channel.clone(),
                ctx.user.clone(),
                ctx.session_key.clone(),
            ));
            Ok(format!("ok:{args}"))
        }
    }

    /// 恒 Err 的哑工具。
    struct BoomTool;

    #[async_trait::async_trait]
    impl Tool for BoomTool {
        async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
            Err("boom".to_string())
        }
    }

    fn registry_with_echo() -> (HashMap<String, Box<dyn Tool>>, std::sync::Arc<EchoTool>) {
        let recorder = std::sync::Arc::new(EchoTool {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let mut m: HashMap<String, Box<dyn Tool>> = HashMap::new();
        // 通过句柄转发：注册表里放一个引用同一记录端的轻量包装。
        struct FwdTool(std::sync::Arc<EchoTool>);
        #[async_trait::async_trait]
        impl Tool for FwdTool {
            async fn execute(&self, args: &str, ctx: &RequestContext) -> Result<String, String> {
                self.0.execute(args, ctx).await
            }
        }
        m.insert("echo".to_string(), Box::new(FwdTool(recorder.clone())));
        m.insert("boom".to_string(), Box::new(BoomTool));
        (m, recorder)
    }

    const CTX_JSON: &str = r#"{"channel":"web","chat_id":"c1","user":"u1","session_key":"s1"}"#;

    #[tokio::test]
    async fn malformed_line_is_rejected_without_panicking() {
        let (tools, _) = registry_with_echo();
        for bad in ["", "not json", "{", r#"{"tool": 123}"#, "[]"] {
            let resp = dispatch(&tools, bad).await;
            assert!(!resp.ok, "line {bad:?} must not pass");
            assert!(
                resp.error.starts_with("bad request line"),
                "line {bad:?}: {}",
                resp.error
            );
            assert!(resp.result.is_empty());
        }
    }

    #[tokio::test]
    async fn bad_context_field_is_reported_as_context_error() {
        let (tools, _) = registry_with_echo();
        // tool/args 合法但 context 不是 RequestContext 形状（类型错）。
        let line = r#"{"tool":"echo","args":"","context":123}"#;
        let resp = dispatch(&tools, line).await;
        assert!(!resp.ok);
        assert!(resp.error.starts_with("bad context"), "err: {}", resp.error);
    }

    #[tokio::test]
    async fn missing_context_required_fields_is_context_error() {
        let (tools, _) = registry_with_echo();
        // context 是 JSON 对象但缺 required 字段（channel 等 String 无 default）。
        let line = r#"{"tool":"echo","args":"","context":{"channel":"web"}}"#;
        let resp = dispatch(&tools, line).await;
        assert!(!resp.ok);
        assert!(resp.error.starts_with("bad context"), "err: {}", resp.error);
    }

    #[tokio::test]
    async fn unknown_tool_is_rejected_by_name() {
        let (tools, _) = registry_with_echo();
        let line = format!(r#"{{"tool":"nope","args":"","context":{CTX_JSON}}}"#);
        let resp = dispatch(&tools, &line).await;
        assert!(!resp.ok);
        assert_eq!(resp.error, "unknown tool: nope");
    }

    #[tokio::test]
    async fn ok_tool_result_round_trips_with_args_passthrough() {
        let (tools, echo) = registry_with_echo();
        let line = format!(r#"{{"tool":"echo","args":"{{\"x\":1}}","context":{CTX_JSON}}}"#);
        let resp = dispatch(&tools, &line).await;
        assert!(resp.ok, "err: {}", resp.error);
        assert_eq!(resp.result, r#"ok:{"x":1}"#);
        assert!(resp.error.is_empty());
        let seen = echo.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "tool executed exactly once");
        assert_eq!(seen[0].0, r#"{"x":1}"#, "args passed through verbatim");
        assert_eq!(seen[0].1, "web", "context.channel reconstructed");
        assert_eq!(seen[0].2, "u1", "context.user reconstructed");
        assert_eq!(seen[0].3, "s1", "context.session_key reconstructed");
    }

    #[tokio::test]
    async fn err_tool_maps_to_error_response() {
        let (tools, _) = registry_with_echo();
        let line = format!(r#"{{"tool":"boom","args":"","context":{CTX_JSON}}}"#);
        let resp = dispatch(&tools, &line).await;
        assert!(!resp.ok);
        assert_eq!(resp.error, "boom");
        assert!(resp.result.is_empty());
    }

    #[tokio::test]
    async fn optional_context_fields_default_cleanly() {
        // correlation_id 缺省 → None；async_callback serde(skip) → None（重建语义）。
        let (tools, echo) = registry_with_echo();
        let line = r#"{"tool":"echo","args":"a","context":{"channel":"rpc","chat_id":"c","user":"u","session_key":"s","correlation_id":"corr-42"}}"#;
        let resp = dispatch(&tools, line).await;
        assert!(resp.ok, "err: {}", resp.error);
        let seen = echo.seen.lock().unwrap();
        assert_eq!(seen[0].1, "rpc");
    }
}

/// run() 前置守卫：workspace env 缺失必须干净报错（而不是进 stdio 循环挂死）。
/// GLOBAL_STATE_LOCK：remove_var 是进程级环境操作，与其它 env 测试互斥。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
async fn run_requires_workspace_env() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    unsafe {
        std::env::remove_var("NEMESISBOT_EXECUTOR_WORKSPACE");
    }
    let err = run()
        .await
        .expect_err("missing workspace env must fail fast");
    assert!(
        err.to_string().contains("NEMESISBOT_EXECUTOR_WORKSPACE"),
        "err: {err:#}"
    );
}

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：executor_main 粘合层。
// - stdio 传输：无 PIPE/无 SANDBOX 标记 → register_shared_tools + stdio_loop
//   （cargo test 的 stdin 是管道 EOF → 立即干净退出）。
// - 沙盒标记：fail-open（warn+Continue）与 P5-2 严格模式（engage Err →
//   emit_error_response 一行协议错误 + Ok 退出，不读 stdin）。
// - Windows 具名管道传输：run() 全链（server 假 gateway 一来一回 + EOF）。
// =========================================================================

// 整 mod Windows 形态（3/3 测试全走 Windows 进程边界；sandbox 项另有
// feature 双门控，随 mod 一并消失于 Linux）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod executor_main_glue {
    use super::*;

    /// env RAII：设置一组 executor env，Drop 全清（测间互不泄漏）。
    struct ExecEnvGuard {
        vars: Vec<&'static str>,
    }
    impl ExecEnvGuard {
        fn set(workspace: &std::path::Path) -> Self {
            let mut vars = vec![
                "NEMESISBOT_EXECUTOR_WORKSPACE",
                "NEMESISBOT_EXECUTOR_SANDBOX",
                "NEMESISBOT_EXECUTOR_REEXEC",
                "NEMESISBOT_EXECUTOR_HOME",
            ];
            if cfg!(windows) {
                vars.push("NEMESISBOT_EXECUTOR_PIPE");
            }
            unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_WORKSPACE", workspace) };
            for v in vars.iter().skip(1) {
                unsafe { std::env::remove_var(v) };
            }
            Self { vars }
        }
        // Only the sandbox+windows tests below call these; gate them with the
        // same cfg so other targets don't see them as dead code
        // (Linux CI clippy runs with -D warnings).
        #[cfg(all(feature = "sandbox", windows))]
        fn set_marker_and_home(&mut self, home: &std::path::Path) {
            unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_SANDBOX", "1") };
            unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_HOME", home) };
        }
    }
    impl Drop for ExecEnvGuard {
        fn drop(&mut self) {
            for v in self.vars.iter() {
                unsafe { std::env::remove_var(v) };
            }
        }
    }

    #[cfg(all(feature = "sandbox", windows))]
    fn config_with(strict: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("config.json"),
            format!(
                r#"{{ "executor": {{ "enabled": true, "sandbox": true, "strict": {strict} }} }}"#
            ),
        )
        .expect("seed config");
        dir
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn run_stdio_transport_registers_tools_and_exits_on_eof() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let _env = ExecEnvGuard::set(tmp.path());

        // stdio（无 PIPE）+ 无沙盒标记：注册共享工具集后 stdin EOF → Ok。
        run()
            .await
            .expect("stdio transport must exit cleanly on EOF");
    }

    #[cfg(all(feature = "sandbox", windows))]
    #[tokio::test]
    async fn run_with_sandbox_marker_fail_open_continues_to_stdio() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let home = config_with(false);
        let mut env = ExecEnvGuard::set(tmp.path());
        env.set_marker_and_home(home.path());

        // Windows 无用户态后端 + strict=false → warn + Continue → stdio EOF Ok。
        run()
            .await
            .expect("fail-open marker must continue to the tool loop");
    }

    #[cfg(all(feature = "sandbox", windows))]
    #[tokio::test]
    async fn run_with_sandbox_marker_strict_emits_error_line_and_exits_ok() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let home = config_with(true);
        let mut env = ExecEnvGuard::set(tmp.path());
        env.set_marker_and_home(home.path());

        // P5-2：engage Err（严格模式拒绝）→ emit_error_response（stdout 一行
        // 协议错误，libtest 捕获）→ Ok(()) —— 不读 stdin、不挂死。
        run()
            .await
            .expect("engage failure must exit Ok after emitting the error line");
    }
}

/// Windows 具名管道传输：run() 全链（gateway 侧假 server 一来一回 + EOF 退出）。
#[cfg(windows)]
mod pipe_transport_via_run {
    use super::*;
    use nemesis_agent::executor_pipe::{create_server, pipe_name, unique_pipe_id};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_over_named_pipe_round_trip_then_clean_eof_exit() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let name = pipe_name(&unique_pipe_id());
        let mut server = create_server(&name).expect("create pipe server");

        unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_WORKSPACE", tmp.path()) };
        unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_PIPE", &name) };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_SANDBOX") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_REEXEC") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_HOME") };

        // run() 在专用线程里自建 current_thread runtime（与生产形态一致）。
        let worker = std::thread::Builder::new()
            .name("t-exec-pipe-worker".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("worker runtime");
                rt.block_on(run())
            })
            .expect("spawn worker thread");

        // gateway 侧：等子进程连上 → 发一行请求 → 读一行响应。
        server.connect().await.expect("server side connect");
        let req = r#"{"tool":"__no_such_tool__","args":"{}","context":{"channel":"web","chat_id":"c1","user":"u1","session_key":"s1"}}"#;
        server
            .write_all(format!("{req}\n").as_bytes())
            .await
            .expect("write request");
        server.flush().await.expect("flush request");

        let mut reader = BufReader::new(&mut server).lines();
        let resp_line = reader
            .next_line()
            .await
            .expect("read response")
            .expect("response line present");
        let v: serde_json::Value = serde_json::from_str(&resp_line).expect("response json");
        assert_eq!(v["ok"], false, "unknown tool must fail: {resp_line}");
        assert!(
            v["error"].as_str().unwrap_or("").contains("unknown tool"),
            "resp: {resp_line}"
        );

        // gateway 关管道 → 子进程 EOF → run() 干净退出 Ok。
        drop(reader);
        drop(server);
        let res = worker.join().expect("worker join");
        res.expect("pipe transport run must exit Ok on EOF");

        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_PIPE") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_WORKSPACE") };
    }
}

// =========================================================================
// wave_b（覆盖率补测 2026-08-27）：pipe_loop 的读错误臂（exec_worker.rs:358）
// ——tokio lines() 对非 UTF-8 行返回 InvalidData → pipe_loop 上抛
// "pipe read: ..."（区别于 Ok(None) 的干净 EOF 退出）。沿用上面同一套
// 具名管道 + worker 线程夹具；全程零子进程、零生产端口。
// =========================================================================
#[cfg(windows)]
mod wave_b {
    use super::*;
    use nemesis_agent::executor_pipe::{create_server, pipe_name, unique_pipe_id};
    use tokio::io::AsyncWriteExt;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_pipe_read_invalid_utf8_line_surfaces_error() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let name = pipe_name(&unique_pipe_id());
        let mut server = create_server(&name).expect("create pipe server");

        unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_WORKSPACE", tmp.path()) };
        unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_PIPE", &name) };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_SANDBOX") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_REEXEC") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_HOME") };

        // run() 在专用线程里自建 current_thread runtime（与生产形态一致）。
        let worker = std::thread::Builder::new()
            .name("t-exec-pipe-badutf8".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("worker runtime");
                rt.block_on(run())
            })
            .expect("spawn worker thread");

        // 假 gateway 写入一行带非法 UTF-8 字节的请求（以 \n 结尾触发行解码）：
        // lines() 解码失败 → Err(InvalidData) → pipe_loop 的 pipe-read 错误臂。
        server.connect().await.expect("server side connect");
        let bad: Vec<u8> = b"\xff\xfe not-utf8 \x80 payload\n".to_vec();
        server
            .write_all(&bad)
            .await
            .expect("write invalid utf8 line");
        server.flush().await.expect("flush");
        drop(server); // 关管道，别让服务端悬着

        let res = worker.join().expect("worker join");
        let err = res.expect_err("非 UTF-8 行必须让 pipe loop 以错误退出");
        assert!(
            err.to_string().contains("pipe read"),
            "必须是 pipe read 错误臂: {err:#}"
        );

        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_PIPE") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_WORKSPACE") };
    }
}

// =========================================================================
// wave_r10（95% 覆盖率 goal 第七波）：stdio_loop 循环体经【真实子进程】走通。
//
// dispatch 协议四臂已由上方 dispatch_protocol 单测逐行钉死；本批补的是
// stdio_loop 本体的 读一行→dispatch→序列化→write_all→flush→再读 循环
// （exec_worker.rs ~380-388）——libtest 内进程的 stdin 是死的、无法注入多行，
// 只能按 gateway 生产同款 spawn 形态（ROLE=executor + 绝对路径 WORKSPACE、
// 不带任何 CLI 参数 = main() 短路在 clap 之前）喂两条协议请求：
//   ① 坏 JSON 行      → {"ok":false,...,"error":"bad request line: ..."}
//   ② 未知工具合法行   → {"ok":false,...,"error":"unknown tool: r10_no_such_tool"}
// 然后关 stdin 让子进程在 EOF 干净退出（退码 0）。全程看门狗限时强杀保护。
//
// 隔离：WORKSPACE 指向独立 tempdir 绝对路径；显式 env_remove 掉 PIPE/SANDBOX/
// REEXEC/HOME 四个干扰键，绝不触任何真实 home。不占任何端口。
// 仅 Windows：resolve_nemesisbot_bin 解析 .exe（与其他子进程测试同门）。
// =========================================================================
#[cfg(windows)]
mod wave_r10_stdio_subprocess {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use test_harness::resolve_nemesisbot_bin;

    /// executor 子进程协议全链：坏 JSON 行 + 未知工具行 → 逐行错误响应；
    /// stdin EOF → 干净退出 0。覆盖 stdio_loop 的读-分发-写-刷循环体。
    #[test]
    fn r10_exec_child_stdio_two_request_protocol_round_trip() {
        let bin = match resolve_nemesisbot_bin() {
            Ok(p) => p,
            Err(e) => panic!("nemesisbot.exe 未找到（先构建或设 NEMESISBOT_TEST_BIN）: {e}"),
        };
        let tmp = tempfile::tempdir().expect("tempdir workspace");

        let mut child = Command::new(&bin)
            .env("NEMESISBOT_ROLE", "executor")
            // 绝对路径 tempdir 工作空间；生产 spawn 形态同款 env 键
            .env("NEMESISBOT_EXECUTOR_WORKSPACE", tmp.path())
            .env_remove("NEMESISBOT_EXECUTOR_PIPE")
            .env_remove("NEMESISBOT_EXECUTOR_SANDBOX")
            .env_remove("NEMESISBOT_EXECUTOR_REEXEC")
            .env_remove("NEMESISBOT_EXECUTOR_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn executor-role child");

        // stdout/stderr 各一条排水线程（防管道堵死 + 收集诊断证据）
        let out_pipe = child.stdout.take().expect("piped stdout");
        let err_pipe = child.stderr.take().expect("piped stderr");
        let out_handle = std::thread::spawn(move || {
            let mut lines = Vec::new();
            for l in std::io::BufReader::new(out_pipe).lines() {
                match l {
                    Ok(x) => lines.push(x),
                    Err(_) => break,
                }
            }
            lines
        });
        let err_handle = std::thread::spawn(move || {
            let mut s = String::new();
            use std::io::Read;
            let _ = std::io::BufReader::new(err_pipe).read_to_string(&mut s);
            s
        });

        // 写两条协议请求后关 stdin → 子进程处理两行 → EOF → run_loop Ok(()) 退出
        let bad_line = "{ definitely broken json";
        let req = serde_json::json!({
            "tool": "r10_no_such_tool",
            "args": "{\"k\":1}",
            "context": { "channel": "web", "chat_id": "c1", "user": "u1", "session_key": "s1" },
        });
        {
            let mut sin = child.stdin.take().expect("piped stdin");
            writeln!(sin, "{bad_line}")
                .and_then(|_| writeln!(sin, "{req}"))
                .expect("write protocol lines");
            let _ = sin.flush();
            drop(sin); // EOF 触发器
        }

        // 限时收齐恰好两行响应
        let deadline = Instant::now() + Duration::from_secs(60);
        let lines = loop {
            if out_handle.is_finished() {
                break out_handle.join().expect("stdout drain join");
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for two protocol responses"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        assert_eq!(
            lines.len(),
            2,
            "exactly one response per request line expected, got:\n{lines:?}"
        );

        // 行①：坏 JSON → "bad request line: ..." 且 ok=false
        let r1: serde_json::Value = serde_json::from_str(&lines[0])
            .unwrap_or_else(|e| panic!("response line 1 not JSON ({e}): {:?}", lines[0]));
        assert_eq!(r1["ok"], serde_json::json!(false), "{r1}");
        assert!(
            r1["error"]
                .as_str()
                .unwrap_or("")
                .starts_with("bad request line"),
            "line1 must be a bad-request-line error: {r1}"
        );

        // 行②：未知工具合法行 → "unknown tool: r10_no_such_tool"（指向工具名）
        let r2: serde_json::Value = serde_json::from_str(&lines[1])
            .unwrap_or_else(|e| panic!("response line 2 not JSON ({e}): {:?}", lines[1]));
        assert_eq!(r2["ok"], serde_json::json!(false), "{r2}");
        assert_eq!(
            r2["error"].as_str(),
            Some("unknown tool: r10_no_such_tool"),
            "line2 must name the unknown tool verbatim: {r2}"
        );

        // EOF → 子进程自退 0；限时兜底强杀防挂死测试二进制
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut status_opt = None;
        while Instant::now() < deadline {
            if let Ok(Some(s)) = child.try_wait() {
                status_opt = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        match status_opt {
            Some(status) => assert!(
                status.success(),
                "child must exit cleanly on stdin EOF, got {status}; stderr:\n{}",
                err_handle.join().unwrap_or_default()
            ),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("executor child didn't exit after stdin EOF within 30s");
            }
        }
        drop(err_handle);
    }
}
