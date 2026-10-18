//! delegate_registry 测试（A3，2026-10-18）。
//!
//! 分层：
//! - 协议 adapter 纯函数面：canned-line 单测（无 IO、无锁、无平台差异）；
//! - 真子进程用例：fake CLI（Windows=批处理跑 `more` / Unix=`cat` 脚本——
//!   都忽略协议参数、stdin 回显 stdout、EOF 退出），模块级 TEST_LOCK 串行
//!   （B4 纪律：并行 spawn/树杀放大进程操作延迟会撞破等待窗口造成假红）；
//! - vault env 白名单：走 nemesis-config 全局解析器槽位（生产同源注入
//!   点），set/clear 复位纪律，不做真实 vault 文件 IO。
//!
//! 诚实边界：claude/codex 真 CLI 的协议握手不在单测面（本机 CI 无
//! CLI）；adapter 解析逻辑由 canned-line 钉住，真机冒烟归交付验收。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{
    DelegateManager, DelegateOutputTool, DelegateProtocolKind, DelegateSendTool, DelegateStartTool,
    DelegateStopTool, MAX_DELEGATES, resolve_env_allowlist, rewrite_rpc_id,
};
use crate::r#loop::Tool;

/// 真子进程用例串行锁（B4 纪律）。
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// 协议 adapter 纯函数面
// ---------------------------------------------------------------------------

#[test]
fn protocol_kind_from_cli_maps_and_rejects_unknown() {
    assert_eq!(
        DelegateProtocolKind::from_cli("claude"),
        Some(DelegateProtocolKind::ClaudeStreamJson)
    );
    assert_eq!(
        DelegateProtocolKind::from_cli("  CODEX "),
        Some(DelegateProtocolKind::CodexAppServer)
    );
    assert_eq!(DelegateProtocolKind::from_cli("gemini"), None);
    assert_eq!(DelegateProtocolKind::from_cli(""), None);
}

#[test]
fn claude_build_args_base_resume_and_permission_mode() {
    let p = DelegateProtocolKind::ClaudeStreamJson;
    let base = p.build_args(None, "").unwrap();
    assert_eq!(
        base,
        vec![
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ]
    );
    let resumed = p.build_args(Some("sess-42"), "acceptEdits").unwrap();
    assert!(resumed.contains(&"--resume".to_string()));
    assert!(resumed.contains(&"sess-42".to_string()));
    assert!(resumed.contains(&"--permission-mode".to_string()));
    assert!(resumed.contains(&"acceptEdits".to_string()));
    // 绝不自动放行：不出现 --dangerously-skip-permissions。
    assert!(
        !resumed
            .iter()
            .any(|a| a.contains("dangerously-skip-permissions"))
    );
}

#[test]
fn codex_build_args_and_resume_rejection() {
    let p = DelegateProtocolKind::CodexAppServer;
    assert_eq!(p.build_args(None, "").unwrap(), vec!["app-server"]);
    let err = p.build_args(Some("sess-1"), "").unwrap_err();
    assert!(err.contains("resume"), "honest rejection: {err}");
}

#[test]
fn claude_user_frame_encodes_stream_json_shape() {
    let frame = DelegateProtocolKind::ClaudeStreamJson.encode_user_frame("hi", None);
    let v: Value = serde_json::from_str(&frame).unwrap();
    assert_eq!(v["type"], "user");
    assert_eq!(v["message"]["role"], "user");
    assert_eq!(v["message"]["content"][0]["type"], "text");
    assert_eq!(v["message"]["content"][0]["text"], "hi");
}

#[test]
fn codex_user_frame_has_placeholder_id_and_conversation() {
    let frame = DelegateProtocolKind::CodexAppServer.encode_user_frame("go", Some("conv-9"));
    let v: Value = serde_json::from_str(&frame).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    // 占位 id=0：发送时由 rewrite_rpc_id 覆写为递增 id。
    assert_eq!(v["id"], 0);
    assert_eq!(v["method"], "sendUserMessage");
    assert_eq!(v["params"]["conversationId"], "conv-9");
    assert_eq!(v["params"]["items"][0]["text"], "go");
}

#[test]
fn claude_extract_assistant_texts_only_assistant_events() {
    let p = DelegateProtocolKind::ClaudeStreamJson;
    // assistant 事件带两个 text 块 → 两段都抽。
    let assistant = json!({
        "type": "assistant",
        "session_id": "s1",
        "message": {"role": "assistant", "content": [
            {"type": "text", "text": "part-one"},
            {"type": "tool_use", "id": "t1", "name": "exec", "input": {}},
            {"type": "text", "text": "part-two"}
        ]}
    })
    .to_string();
    assert_eq!(
        p.extract_assistant_texts(&assistant),
        vec!["part-one".to_string(), "part-two".to_string()]
    );
    assert_eq!(p.extract_session_id(&assistant).as_deref(), Some("s1"));
    // result 事件与 assistant 同轮重复（抽了会双份）→ 不抽。
    let result = json!({
        "type": "result",
        "subtype": "success",
        "result": "part-one part-two",
        "session_id": "s1"
    })
    .to_string();
    assert!(p.extract_assistant_texts(&result).is_empty());
    assert_eq!(p.extract_session_id(&result).as_deref(), Some("s1"));
    // 非 assistant 的流事件（system/init 等）→ 空。
    let init = json!({"type": "system", "subtype": "init", "session_id": "s1"}).to_string();
    assert!(p.extract_assistant_texts(&init).is_empty());
    // 非法行 → 空（诚实忽略，不炸 pump）。
    assert!(p.extract_assistant_texts("not json at all").is_empty());
}

#[test]
fn codex_extract_agent_message_events() {
    let p = DelegateProtocolKind::CodexAppServer;
    let event = json!({
        "jsonrpc": "2.0",
        "method": "codex/event/agent_message",
        "params": {"msg": {"message": "worker says hi"}}
    })
    .to_string();
    assert_eq!(p.extract_assistant_texts(&event), vec!["worker says hi"]);
    // codex 的会话 id 在握手响应里（manager 缓存），不在事件流 → None。
    assert_eq!(p.extract_session_id(&event), None);
}

#[test]
fn rewrite_rpc_id_replaces_only_first_placeholder() {
    let frame = r#"{"jsonrpc":"2.0","id":0,"method":"sendUserMessage","params":{"note":"x"}}"#;
    let rewritten = rewrite_rpc_id(frame, 17);
    assert!(rewritten.contains(r#""id":17"#));
    assert!(!rewritten.contains(r#""id":0,"#));
}

// ---------------------------------------------------------------------------
// fake CLI + 真子进程用例
// ---------------------------------------------------------------------------

/// fake delegate CLI：忽略全部参数、stdin 回显 stdout、EOF 退出。
/// Windows 批处理跑 `more`（stdin 管道形态无分页提示），Unix 跑 `cat`。
fn make_fake_echo_cli(dir: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        let p = dir.join("fake_delegate_cli.cmd");
        std::fs::write(&p, "@echo off\r\nmore\r\n").expect("write fake cli");
        p.to_string_lossy().to_string()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("fake_delegate_cli.sh");
        std::fs::write(&p, "#!/bin/sh\ncat\n").expect("write fake cli");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        p.to_string_lossy().to_string()
    }
}

fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!(
        "nb_delegate_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// 有界轮询 output(id, 0) 直到谓词成立（manager 状态面全走 output JSON）。
/// 超时返回末次快照供断言自带现场。
async fn wait_output_pred(
    mgr: &Arc<DelegateManager>,
    id: u64,
    pred: impl Fn(&Value) -> bool,
) -> Value {
    wait_output_pred_for(mgr, id, Duration::from_secs(5), pred).await
}

async fn wait_output_pred_for(
    mgr: &Arc<DelegateManager>,
    id: u64,
    timeout: Duration,
    pred: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + timeout;
    loop {
        let last = mgr.output(id, 0).await.expect("output ok");
        if pred(&last) || Instant::now() >= deadline {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn start_send_output_roundtrip_with_fake_cli() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("roundtrip");
    let cli = make_fake_echo_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());

    let started = mgr
        .start(
            DelegateProtocolKind::ClaudeStreamJson,
            &cli,
            None,
            dir.to_string_lossy().as_ref(),
            None,
            "",
            Default::default(),
            Duration::MAX,
        )
        .await
        .expect("start ok");
    let id = started["delegate_id"].as_u64().unwrap();
    assert!(started["pid"].as_u64().unwrap() > 0);
    assert_eq!(started["protocol"], "claude-stream-json");

    // steer：stdin 帧注入后应从 stdout 回显出来（fake CLI = echo）。
    mgr.send(id, "hello-steer").await.expect("send ok");
    let out = wait_output_pred(&mgr, id, |o| {
        o["raw"]["chunk"]
            .as_str()
            .is_some_and(|c| c.contains("hello-steer"))
    })
    .await;
    assert!(
        out["raw"]["chunk"]
            .as_str()
            .unwrap()
            .contains("hello-steer"),
        "steer text must round-trip through stdin/stdout: {out}"
    );
    // 回显的是 user 帧（type=user）——assistant 抽取必须为空（不误抽）。
    assert!(out["assistant"].as_array().unwrap().is_empty());
    // fake CLI 不发 session_id → null；进程还活着。
    assert!(out["session_id"].is_null());
    assert_eq!(out["running"], true);

    mgr.stop(id).await.expect("stop ok");
}

#[tokio::test]
async fn stop_terminates_delegate() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("stop");
    let cli = make_fake_echo_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());
    let started = mgr
        .start(
            DelegateProtocolKind::ClaudeStreamJson,
            &cli,
            None,
            dir.to_string_lossy().as_ref(),
            None,
            "",
            Default::default(),
            Duration::MAX,
        )
        .await
        .expect("start ok");
    let id = started["delegate_id"].as_u64().unwrap();

    let stopped = mgr.stop(id).await.expect("stop ok");
    assert_eq!(stopped["stopped"], true);
    // fake CLI（more/cat）遇 stdin EOF 或树杀必退——6s 窗口内应拿到终态。
    assert!(
        stopped.get("exit").is_some(),
        "stop should report exit within the wait window: {stopped}"
    );
    // 停止后 send 诚实报错。
    let err = mgr.send(id, "again").await.unwrap_err();
    assert!(
        err.contains("not running") || err.contains("stdin is closed"),
        "{err}"
    );
}

#[tokio::test]
async fn lifetime_watchdog_tree_kills_and_marks_output() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("watchdog");
    let cli = make_fake_echo_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());
    let started = mgr
        .start(
            DelegateProtocolKind::ClaudeStreamJson,
            &cli,
            None,
            dir.to_string_lossy().as_ref(),
            None,
            "",
            Default::default(),
            Duration::from_secs(1),
        )
        .await
        .expect("start ok");
    let id = started["delegate_id"].as_u64().unwrap();

    let out = wait_output_pred(&mgr, id, |o| o["running"] == false).await;
    assert_eq!(
        out["running"], false,
        "watchdog should tree-kill within ~1s lifetime budget: {out}"
    );
    assert!(
        out["raw"]["chunk"]
            .as_str()
            .unwrap_or("")
            .contains("lifetime budget exhausted"),
        "honest marker expected in output stream: {out}"
    );
}

#[tokio::test]
async fn estop_engaged_rejects_start() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("estop_reject");
    let cli = make_fake_echo_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());
    let estop = Arc::new(crate::estop::EstopState::new());
    mgr.attach_estop(estop.clone());
    estop.trigger();

    let err = mgr
        .start(
            DelegateProtocolKind::ClaudeStreamJson,
            &cli,
            None,
            dir.to_string_lossy().as_ref(),
            None,
            "",
            Default::default(),
            Duration::MAX,
        )
        .await
        .unwrap_err();
    assert!(err.contains("estop"), "honest rejection: {err}");
}

#[tokio::test]
async fn estop_trigger_kills_running_delegates() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("estop_kill");
    let cli = make_fake_echo_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());
    let estop = Arc::new(crate::estop::EstopState::new());
    mgr.attach_estop(estop.clone());

    let started = mgr
        .start(
            DelegateProtocolKind::ClaudeStreamJson,
            &cli,
            None,
            dir.to_string_lossy().as_ref(),
            None,
            "",
            Default::default(),
            Duration::MAX,
        )
        .await
        .expect("start ok (estop not engaged yet; watcher spawns here)");
    let id = started["delegate_id"].as_u64().unwrap();

    estop.trigger();
    let out = wait_output_pred(&mgr, id, |o| o["running"] == false).await;
    assert_eq!(
        out["running"], false,
        "estop must kill running delegates: {out}"
    );
}

#[tokio::test]
async fn manager_evicts_oldest_finished_at_capacity() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("capacity");
    let cli = make_fake_echo_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());

    // 起满 MAX_DELEGATES 个，然后逐个 stop（产生「已完成条目」）。
    let mut ids = Vec::new();
    for _ in 0..MAX_DELEGATES {
        let started = mgr
            .start(
                DelegateProtocolKind::ClaudeStreamJson,
                &cli,
                None,
                dir.to_string_lossy().as_ref(),
                None,
                "",
                Default::default(),
                Duration::MAX,
            )
            .await
            .expect("start ok");
        ids.push(started["delegate_id"].as_u64().unwrap());
    }
    for &id in &ids {
        mgr.stop(id).await.expect("stop ok");
    }
    // 满 + 最旧已完成 → 下一个 start 淘汰最旧条目成功而非拒绝。
    let started = mgr
        .start(
            DelegateProtocolKind::ClaudeStreamJson,
            &cli,
            None,
            dir.to_string_lossy().as_ref(),
            None,
            "",
            Default::default(),
            Duration::MAX,
        )
        .await;
    assert!(
        started.is_ok(),
        "oldest finished delegate should be evicted: {:?}",
        started.err()
    );
    let new_started = started.unwrap();
    // 淘汰的是最旧的 id==1：查询它应报 not found。
    let err = mgr.output(1, 0).await.unwrap_err();
    assert!(
        err.contains("not found"),
        "evicted entry should be gone: {err}"
    );
    // 新条目可用。
    let new_id = new_started["delegate_id"].as_u64().unwrap();
    assert!(new_id > MAX_DELEGATES as u64);
    mgr.stop(new_id).await.expect("cleanup stop");
}

#[tokio::test]
async fn all_running_at_capacity_rejects_honestly() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("full");
    let cli = make_fake_echo_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());

    let mut ids = Vec::new();
    for _ in 0..MAX_DELEGATES {
        let started = mgr
            .start(
                DelegateProtocolKind::ClaudeStreamJson,
                &cli,
                None,
                dir.to_string_lossy().as_ref(),
                None,
                "",
                Default::default(),
                Duration::MAX,
            )
            .await
            .expect("start ok");
        ids.push(started["delegate_id"].as_u64().unwrap());
    }
    // 全在跑 → 诚实拒绝（不淘汰、不静默）。
    let err = mgr
        .start(
            DelegateProtocolKind::ClaudeStreamJson,
            &cli,
            None,
            dir.to_string_lossy().as_ref(),
            None,
            "",
            Default::default(),
            Duration::MAX,
        )
        .await
        .unwrap_err();
    assert!(err.contains("limit reached"), "{err}");
    for id in ids {
        mgr.stop(id).await.expect("cleanup stop");
    }
}

// ---------------------------------------------------------------------------
// 真机冒烟（#[ignore]：CI 默认跳过；本机 claude CLI 在场且有登录态时
// `cargo test -p nemesis-agent -- --ignored delegate_real_claude_smoke`
// 手动触发。缺席 → 诚实 SKIP，不是失败。）
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "real-machine smoke: needs claude CLI on PATH + login state"]
async fn delegate_real_claude_smoke() {
    let Some(cli) = crate::loop_tools::cli_delegation::find_cli_on_path("claude") else {
        eprintln!("SKIP: claude CLI not on PATH — real-machine smoke not applicable here");
        return;
    };
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("smoke");
    let mgr = Arc::new(DelegateManager::new());
    let started = mgr
        .start(
            DelegateProtocolKind::ClaudeStreamJson,
            &cli,
            Some("Reply with exactly this token and nothing else: DELEGATE_SMOKE_OK"),
            dir.to_string_lossy().as_ref(),
            None,
            "",
            Default::default(),
            Duration::from_secs(180),
        )
        .await
        .expect("claude start ok");
    let id = started["delegate_id"].as_u64().unwrap();
    eprintln!("started: {started}");

    // 真 LLM 往返（冷启动 + 首 token）远超单测轮询窗——给 120s。
    let out = wait_output_pred_for(&mgr, id, Duration::from_secs(120), |o| {
        o["assistant"].as_array().is_some_and(|a| !a.is_empty())
    })
    .await;
    let assistant = out["assistant"].as_array().unwrap();
    assert!(
        !assistant.is_empty(),
        "expected at least one assistant text from real claude: {out}"
    );
    let joined = assistant
        .iter()
        .filter_map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!("assistant reply: {joined}");
    assert!(
        joined.contains("DELEGATE_SMOKE_OK"),
        "claude reply should contain the smoke token: {joined}"
    );
    // session_id 抽取（resume 能力的真机证据）。
    eprintln!("session_id: {:?}", out["session_id"]);

    let stopped = mgr.stop(id).await.expect("stop ok");
    eprintln!("stopped: {stopped}");
}

// ---------------------------------------------------------------------------
// env 白名单（vault 解析器槽位，生产同源注入点）
// ---------------------------------------------------------------------------

#[test]
fn empty_allowlist_is_noop_without_resolver() {
    // 空白名单不触解析器（两种 cfg 臂同语义）。
    let out = resolve_env_allowlist("unused", &[]).expect("empty allowlist is a no-op");
    assert!(out.is_empty());
}

#[cfg(feature = "security")]
#[test]
fn env_allowlist_via_global_vault_resolver() {
    use nemesis_config::{clear_global_vault_resolver, set_global_vault_resolver};

    // 复位纪律：先清槽防外部注入干扰，测完再清（本进程内只有 vault 用例
    // 触全局槽，且都是 set→断言→clear 的同步短临界区，无 await 穿插）。
    clear_global_vault_resolver();
    // 解析器未安装 → 诚实报错。
    let err = resolve_env_allowlist("unused", &["ANY_KEY".to_string()]).unwrap_err();
    assert!(err.contains("no vault resolver"), "{err}");

    // 安装桩解析器：GOOD_KEY → s3cret；BAD_KEY → Err。
    set_global_vault_resolver(Arc::new(|alias| match alias {
        "GOOD_KEY" => Ok("s3cret".to_string()),
        other => Err(format!("alias '{other}' not in vault")),
    }));
    let ok = resolve_env_allowlist("unused", &["GOOD_KEY".to_string()]).unwrap();
    assert_eq!(ok.get("GOOD_KEY").map(String::as_str), Some("s3cret"));
    // 任一失败 → 逐项列明（成功的别名不进错误面）。
    let err = resolve_env_allowlist("unused", &["GOOD_KEY".to_string(), "BAD_KEY".to_string()])
        .unwrap_err();
    assert!(err.contains("resolution failed"), "{err}");
    assert!(
        err.contains("BAD_KEY"),
        "failed alias must be listed: {err}"
    );
    assert!(
        !err.contains("GOOD_KEY"),
        "succeeded alias must not be listed: {err}"
    );
    clear_global_vault_resolver();
}

// ---------------------------------------------------------------------------
// 工具 schema 面（描述/参数形状冒烟——不 spawn 进程）
// ---------------------------------------------------------------------------

#[test]
fn tool_schemas_expose_expected_parameters() {
    let start = DelegateStartTool::new(
        "ws",
        true,
        3600,
        Some("claude".into()),
        None,
        Arc::new(DelegateManager::new()),
    );
    let schema = start.parameters();
    assert_eq!(schema["required"][0], "cli");
    assert!(
        schema["properties"]["cli"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("claude"))
    );
    // codex 缺席（None）→ 执行时诚实报错，schema 仍声明 codex 选项。
    assert!(schema["properties"]["resume_session_id"].is_object());

    let send = DelegateSendTool::new(Arc::new(DelegateManager::new()));
    let s = send.parameters();
    assert_eq!(s["required"][0], "delegate_id");
    assert_eq!(s["required"][1], "text");

    let out = DelegateOutputTool::new(Arc::new(DelegateManager::new()));
    assert!(out.parameters()["properties"]["offset"].is_object());

    let stop = DelegateStopTool::new(Arc::new(DelegateManager::new()));
    assert_eq!(stop.parameters()["required"][0], "delegate_id");

    // 描述面：四工具都声明与 delegate 家族的关系（LLM 选型依据）。
    assert!(start.description().contains("delegate_send"));
    assert!(send.description().contains("delegate_start"));
    assert!(out.description().contains("offset"));
    assert!(stop.description().contains("tree-kills"));
}
