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
fn codex_user_frame_has_placeholder_id_and_thread() {
    let frame = DelegateProtocolKind::CodexAppServer.encode_user_frame("go", Some("thread-9"));
    let v: Value = serde_json::from_str(&frame).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    // 占位 id=0：发送时由 rewrite_rpc_id 覆写为递增 id。
    assert_eq!(v["id"], 0);
    assert_eq!(v["method"], "turn/start");
    assert_eq!(v["params"]["threadId"], "thread-9");
    // 必填字段名是 input（0.149.1 实测；items 会被 missing field `input` 拒）。
    assert_eq!(v["params"]["input"][0]["type"], "text");
    assert_eq!(v["params"]["input"][0]["text"], "go");
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
    // 完成事件（0.149.1 实测形态）：item.type=="agentMessage" → 抽 item.text。
    let completed = json!({
        "jsonrpc": "2.0",
        "method": "item/completed",
        "params": {
            "item": {"type": "agentMessage", "id": "msg_1", "text": "worker says hi"},
            "threadId": "t1",
            "turnId": "u1"
        }
    })
    .to_string();
    assert_eq!(
        p.extract_assistant_texts(&completed),
        vec!["worker says hi"]
    );
    // 流式增量与 completed 同内容——抽了会双份 → 不抽。
    let delta = json!({
        "jsonrpc": "2.0",
        "method": "item/agentMessage/delta",
        "params": {"threadId": "t1", "turnId": "u1", "itemId": "msg_1", "delta": "worker says"}
    })
    .to_string();
    assert!(p.extract_assistant_texts(&delta).is_empty());
    // userMessage 的 completed 是用户回声不是回复 → 不抽。
    let user_echo = json!({
        "jsonrpc": "2.0",
        "method": "item/completed",
        "params": {
            "item": {"type": "userMessage", "id": "i2",
                     "content": [{"type": "text", "text": "worker says hi"}]},
            "threadId": "t1",
            "turnId": "u1"
        }
    })
    .to_string();
    assert!(p.extract_assistant_texts(&user_echo).is_empty());
    // codex 的会话 id 在握手响应里（manager 缓存），不在事件流 → None。
    assert_eq!(p.extract_session_id(&completed), None);
}

#[test]
fn rewrite_rpc_id_replaces_only_first_placeholder() {
    let frame = r#"{"jsonrpc":"2.0","id":0,"method":"turn/start","params":{"note":"x"}}"#;
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
// F7（二轮复评 2026-10-18）：启动在途纳入急停与容量
// ---------------------------------------------------------------------------

/// 等 pid 对应进程消失（F7 取证面：失败路径必须真的杀掉子进程，不许孤儿）。
#[cfg(windows)]
async fn wait_pid_gone(pid: u32) -> bool {
    for _ in 0..50 {
        let out = tokio::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .await;
        if let Ok(o) = out {
            let s = String::from_utf8_lossy(&o.stdout);
            // 无匹配时 tasklist 输出 INFO 行（各语言形态不一），不含 pid。
            if !s.contains(&pid.to_string()) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[cfg(not(windows))]
async fn wait_pid_gone(pid: u32) -> bool {
    for _ in 0..50 {
        let gone = tokio::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .await
            .map(|o| !o.status.success())
            .unwrap_or(true);
        if gone {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// 从「estop engaged: delegate start aborted (pid N killed)」抠 pid。
fn pid_from_abort_err(err: &str) -> Option<u32> {
    let rest = err.split("pid ").nth(1)?;
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse().ok()
}

#[tokio::test]
async fn estop_in_start_window_aborts_and_kills_child() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("estop_window");
    let cli = make_fake_echo_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());
    let estop = Arc::new(crate::estop::EstopState::new());
    mgr.attach_estop(estop.clone());

    // 占住 entries 锁把 bg start 钉在容量闸（入口 estop 检查之后、spawn
    // 之前）——这正是旧实现「watcher 摸不到未入表进程」的窗口。
    let guard = mgr.entries.lock().await;
    let mgr_bg = mgr.clone();
    let cli_bg = cli.clone();
    let cwd_bg = dir.to_string_lossy().to_string();
    let bg = tokio::spawn(async move {
        mgr_bg
            .start(
                DelegateProtocolKind::ClaudeStreamJson,
                &cli_bg,
                None,
                &cwd_bg,
                None,
                "",
                Default::default(),
                Duration::MAX,
            )
            .await
    });
    // current_thread 调度：一次让位足以让 bg 跑到容量闸挂起。
    tokio::time::sleep(Duration::from_millis(100)).await;

    estop.trigger();
    drop(guard);

    let err = bg.await.expect("join ok").expect_err("must abort");
    assert!(
        err.contains("estop engaged: delegate start aborted"),
        "honest abort: {err}"
    );
    // 摘预留零残留： entries 与 starting 都空（测试模块可见私有字段）。
    assert_eq!(mgr.entries.lock().await.len(), 0, "no entry may leak");
    assert_eq!(mgr.starting.lock().len(), 0, "no reservation may leak");
    // 孤儿取证：报告过的 pid 必须真的死了（start_kill + supervise 树杀）。
    let pid = pid_from_abort_err(&err).expect("abort error carries pid");
    assert!(
        wait_pid_gone(pid).await,
        "pid {pid} must be dead, not orphaned"
    );
}

/// barrier 假 codex CLI：挂起直到 `<dir>/go` 出现，然后分两拍打印罐头
/// 握手响应（initialize id=1 / thread/start id=2 带 thread.id，0.149.1
/// 实测形态），再活几秒收尾。两拍间隔 2s 是必须的：rpc_round 按
/// read_mark 消费输出，一次全打印会被 round 1 的整块读取吞掉 id=2 行
/// → round 2 永远等不到。用于把并发 start 确定性地停在握手窗口
/// （starting 表里）。
fn make_barrier_codex_cli(dir: &std::path::Path) -> String {
    let go = dir.join("go");
    #[cfg(windows)]
    {
        let p = dir.join("fake_barrier_codex.cmd");
        let body = format!(
            "@echo off\r\n:loop\r\nif exist \"{}\" goto go\r\nping -n 1 127.0.0.1 >nul\r\ngoto loop\r\n:go\r\necho {{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}\r\nping -n 3 127.0.0.1 >nul\r\necho {{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"thread\":{{\"id\":\"barrier-thread\",\"sessionId\":\"barrier-thread\"}}}}}}\r\nping -n 6 127.0.0.1 >nul\r\n",
            go.to_string_lossy()
        );
        std::fs::write(&p, body).expect("write barrier cli");
        p.to_string_lossy().to_string()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("fake_barrier_codex.sh");
        let body = format!(
            "#!/bin/sh\nwhile [ ! -f {} ]; do sleep 0.05; done\necho '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}'\nsleep 2\necho '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"thread\":{{\"id\":\"barrier-thread\",\"sessionId\":\"barrier-thread\"}}}}}}'\nsleep 5\n",
            go.to_string_lossy()
        );
        std::fs::write(&p, body).expect("write barrier cli");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        p.to_string_lossy().to_string()
    }
}

#[tokio::test]
async fn concurrent_starts_capacity_counts_inflight() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("cap_inflight");
    let cli = make_barrier_codex_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());

    // 9 个并发 start：8 个进握手窗口挂起（占用 starting 座位），第 9 个
    // 必须被容量闸拒（旧实现只数 entries——并发可破 MAX_DELEGATES）。
    let mut bgs = Vec::new();
    for _ in 0..MAX_DELEGATES + 1 {
        let mgr_bg = mgr.clone();
        let cli_bg = cli.clone();
        let cwd_bg = dir.to_string_lossy().to_string();
        bgs.push(tokio::spawn(async move {
            mgr_bg
                .start(
                    DelegateProtocolKind::CodexAppServer,
                    &cli_bg,
                    None,
                    &cwd_bg,
                    None,
                    "",
                    Default::default(),
                    Duration::MAX,
                )
                .await
        }));
    }
    // 等 8 个全停在握手（每 50ms 轮询一次 output）——多等几拍确保全部
    // 越过容量闸。断言窗口内的座位守恒。
    tokio::time::sleep(Duration::from_millis(500)).await;
    let starting_now = mgr.starting.lock().len();
    assert_eq!(
        starting_now, MAX_DELEGATES,
        "all 8 should be parked in the handshake window: {starting_now}"
    );

    // 放行 barrier：8 个握手完成 → 终检（entries+starting ≤ 8）→ 全插入。
    std::fs::write(dir.join("go"), b"1").expect("release barrier");
    let mut oks = Vec::new();
    let mut errs = Vec::new();
    for bg in bgs {
        match bg.await.expect("join ok") {
            Ok(v) => oks.push(v["delegate_id"].as_u64().unwrap()),
            Err(e) => errs.push(e),
        }
    }
    assert_eq!(
        oks.len(),
        MAX_DELEGATES,
        "exactly 8 may start: oks={oks:?} errs={errs:?}"
    );
    assert_eq!(
        errs.len(),
        1,
        "exactly one rejected: oks={oks:?} errs={errs:?}"
    );
    assert!(
        errs[0].contains("limit reached") && errs[0].contains("active/starting"),
        "rejection must name the inflight-aware gate: {}",
        errs[0]
    );
    assert_eq!(mgr.entries.lock().await.len(), MAX_DELEGATES);
    assert_eq!(mgr.starting.lock().len(), 0);

    for id in oks {
        mgr.stop(id).await.expect("cleanup stop");
    }
}

// R6 回归锁（原 review 探针反转，2026-10-20）：容量闸 = 「检查+占位」——
// 许可在 entries 锁内取得（并发准入串行化），MAX+1 个同步并发 start 只有
// MAX 个能越过闸（第 MAX+1 个在闸上即被拒，到不了 admission barrier），
// 座位三段口径 entries+starting+permits 全程 ≤ MAX_DELEGATES。
#[tokio::test]
async fn admission_permit_caps_concurrent_starts_at_limit() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("review_admission");
    let cli = make_barrier_codex_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());
    // barrier 参与方 = MAX+1（MAX 个越闸 start + 本测试）：若回归（闸失去
    // 占位语义），MAX+1 个全挂 barrier、本测试 wait 参与计数不足 → 超时
    // 失败，误超订即时可观测。
    let barrier = Arc::new(tokio::sync::Barrier::new(MAX_DELEGATES + 1));
    *mgr.review_admission_barrier.write() = Some(barrier.clone());
    let mut bgs = Vec::new();
    for _ in 0..MAX_DELEGATES + 1 {
        let mgr = mgr.clone();
        let cli = cli.clone();
        let cwd = dir.to_string_lossy().to_string();
        bgs.push(tokio::spawn(async move {
            mgr.start(
                DelegateProtocolKind::CodexAppServer,
                &cli,
                None,
                &cwd,
                None,
                "",
                Default::default(),
                Duration::MAX,
            )
            .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(10), barrier.wait())
        .await
        .expect("exactly MAX starts may reach the admission barrier");
    // 停驻窗口：座位全部以「许可」形态持有（spawn 尚未发生）。
    let seats = mgr.entries.lock().await.len()
        + mgr.starting.lock().len()
        + mgr
            .starting_permits
            .load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(seats, MAX_DELEGATES, "parked starts hold seats via permits");
    // 放行 CLI barrier：握手完成 → 预留落位（许可释放）→ 终检 → 插表。
    std::fs::write(dir.join("go"), b"1").unwrap();
    let mut oks = Vec::new();
    let mut errs = Vec::new();
    for bg in bgs {
        match bg.await.unwrap() {
            Ok(v) => oks.push(v["delegate_id"].as_u64().unwrap()),
            Err(e) => errs.push(e),
        }
    }
    assert_eq!(
        oks.len(),
        MAX_DELEGATES,
        "MAX ok: oks={oks:?} errs={errs:?}"
    );
    assert_eq!(
        errs.len(),
        1,
        "exactly one rejected at the gate: errs={errs:?}"
    );
    assert!(
        errs[0].contains("limit reached") && errs[0].contains("active/starting"),
        "the MAX+1th start must be rejected by the permit gate: {}",
        errs[0]
    );
    assert_eq!(mgr.entries.lock().await.len(), MAX_DELEGATES);
    assert_eq!(mgr.starting.lock().len(), 0);
    assert_eq!(
        mgr.starting_permits
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no permit may leak"
    );
    mgr.stop_all().await;
}

// ---------------------------------------------------------------------------
// F8：permission_mode 配置缺省贯通（args 缺省 → 配置值；显式 args 恒优先）
// ---------------------------------------------------------------------------

/// 回显自身 argv 的假 claude CLI：把 build_args 形态直接落到 stdout，
/// 让 permission_mode 贯通可观测（`ARGS=-p ... --permission-mode X`）。
fn make_echo_args_cli(dir: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        let p = dir.join("fake_echo_args.cmd");
        std::fs::write(&p, "@echo off\r\necho ARGS=%*\r\nmore\r\n").expect("write fake cli");
        p.to_string_lossy().to_string()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("fake_echo_args.sh");
        std::fs::write(&p, "#!/bin/sh\necho \"ARGS=$*\"\ncat\n").expect("write fake cli");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        p.to_string_lossy().to_string()
    }
}

#[tokio::test]
async fn start_tool_permission_mode_default_and_override() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("perm_mode");
    let cli = make_echo_args_cli(&dir);
    let mgr = Arc::new(DelegateManager::new());
    let workspace = dir.to_string_lossy().to_string();
    let tool = DelegateStartTool::new(
        &workspace,
        true,
        3600,
        Some(cli.clone()),
        None,
        "acceptEdits".to_string(),
        mgr.clone(),
    );
    let ctx = crate::context::RequestContext::new("test", "chat", "user", "session");

    // args 缺省 → 配置缺省值落地。
    let out: Value = serde_json::from_str(
        &tool
            .execute(r#"{"cli":"claude"}"#, &ctx)
            .await
            .expect("start ok"),
    )
    .expect("json out");
    let id = out["delegate_id"].as_u64().unwrap();
    let o = wait_output_pred(&mgr, id, |o| {
        o["raw"]["chunk"]
            .as_str()
            .is_some_and(|c| c.contains("ARGS="))
    })
    .await;
    let chunk = o["raw"]["chunk"].as_str().unwrap();
    assert!(
        chunk.contains("--permission-mode acceptEdits"),
        "configured default must land in build_args: {chunk}"
    );
    mgr.stop(id).await.expect("stop ok");

    // args 显式 → 恒优先（配置值被覆盖）。
    let out: Value = serde_json::from_str(
        &tool
            .execute(
                r#"{"cli":"claude","permission_mode":"bypassPermissions"}"#,
                &ctx,
            )
            .await
            .expect("start ok"),
    )
    .expect("json out");
    let id = out["delegate_id"].as_u64().unwrap();
    let o = wait_output_pred(&mgr, id, |o| {
        o["raw"]["chunk"]
            .as_str()
            .is_some_and(|c| c.contains("ARGS="))
    })
    .await;
    let chunk = o["raw"]["chunk"].as_str().unwrap();
    assert!(
        chunk.contains("--permission-mode bypassPermissions"),
        "explicit args must win: {chunk}"
    );
    assert!(
        !chunk.contains("acceptEdits"),
        "configured default must be overridden: {chunk}"
    );
    mgr.stop(id).await.expect("stop ok");

    // 配置空 + args 缺省 → 不加旗（CLI 默认，现行为）。
    let tool = DelegateStartTool::new(
        &workspace,
        true,
        3600,
        Some(cli),
        None,
        String::new(),
        mgr.clone(),
    );
    let out: Value = serde_json::from_str(
        &tool
            .execute(r#"{"cli":"claude"}"#, &ctx)
            .await
            .expect("start ok"),
    )
    .expect("json out");
    let id = out["delegate_id"].as_u64().unwrap();
    let o = wait_output_pred(&mgr, id, |o| {
        o["raw"]["chunk"]
            .as_str()
            .is_some_and(|c| c.contains("ARGS="))
    })
    .await;
    let chunk = o["raw"]["chunk"].as_str().unwrap();
    assert!(
        !chunk.contains("--permission-mode"),
        "empty config + empty args = CLI default (no flag): {chunk}"
    );
    mgr.stop(id).await.expect("stop ok");
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

#[tokio::test]
#[ignore = "real-machine smoke: needs codex CLI on PATH + login state"]
async fn delegate_real_codex_smoke() {
    let Some(cli) = crate::loop_tools::cli_delegation::find_cli_on_path("codex") else {
        eprintln!("SKIP: codex CLI not on PATH — real-machine smoke not applicable here");
        return;
    };
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("smoke");
    let mgr = Arc::new(DelegateManager::new());
    let started = mgr
        .start(
            DelegateProtocolKind::CodexAppServer,
            &cli,
            Some("Reply with exactly this token and nothing else: DELEGATE_SMOKE_OK"),
            dir.to_string_lossy().as_ref(),
            None,
            "",
            Default::default(),
            Duration::from_secs(180),
        )
        .await
        .expect("codex start ok");
    let id = started["delegate_id"].as_u64().unwrap();
    eprintln!("started: {started}");

    // thread/start → turn/start 全握手 + 真 LLM 往返，给 120s。
    let out = wait_output_pred_for(&mgr, id, Duration::from_secs(120), |o| {
        o["assistant"].as_array().is_some_and(|a| !a.is_empty())
    })
    .await;
    let assistant = out["assistant"].as_array().unwrap();
    assert!(
        !assistant.is_empty(),
        "expected at least one agentMessage from real codex: {out}"
    );
    let joined = assistant
        .iter()
        .filter_map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!("assistant reply: {joined}");
    assert!(
        joined.contains("DELEGATE_SMOKE_OK"),
        "codex reply should contain the smoke token: {joined}"
    );
    // codex 的 session 来自握手（peer_session），out.session_id 恒 None——注释留证。
    eprintln!("peer session: {:?}", out["session_id"]);

    let stopped = mgr.stop(id).await.expect("stop ok");
    eprintln!("stopped: {stopped}");
}

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
        String::new(),
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

/// A2 sleeper CLI：忽略 stdin EOF（模拟轮次中的 delegate——不读 stdin，
/// EOF 优雅语义对它无效，必须走 kill 旗标阶梯）。
fn make_fake_sleeper_cli(dir: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        let p = dir.join("fake_sleeper_cli.cmd");
        std::fs::write(&p, "@echo off\r\nping -n 30 127.0.0.1 >nul\r\n").expect("write fake cli");
        p.to_string_lossy().to_string()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("fake_sleeper_cli.sh");
        std::fs::write(&p, "#!/bin/sh\nsleep 30\n").expect("write fake cli");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        p.to_string_lossy().to_string()
    }
}

/// A2：空闲 delegate（echo CLI，无待处理输入）stop → stdin EOF 优雅窗内
/// 自退 → 不置 kill 旗标，exit.killed 诚实保持 false。
#[tokio::test]
async fn stop_idle_delegate_exits_gracefully_without_kill_flag() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("stop_grace_idle");
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
    assert!(
        stopped.get("exit").is_some(),
        "idle delegate must exit within the EOF grace window: {stopped}"
    );
    assert_eq!(
        stopped["exit"]["killed"], false,
        "graceful EOF stop must NOT raise the kill flag: {stopped}"
    );
}

/// A2：EOF-忽略 delegate（模拟轮次中）stop → 优雅窗超窗 → kill 旗标 →
/// supervise 停止阶梯收尸，killed=true 且 STOP_WAIT 窗内拿到终态。
#[tokio::test]
async fn stop_eof_ignoring_delegate_escalates_to_kill() {
    let _serial = TEST_LOCK.lock().await;
    let dir = unique_temp_dir("stop_grace_ladder");
    let cli = make_fake_sleeper_cli(&dir);
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
    assert!(
        stopped.get("exit").is_some(),
        "EOF-ignoring delegate must still terminate within STOP_WAIT via ladder: {stopped}"
    );
    assert_eq!(
        stopped["exit"]["killed"], true,
        "escalated stop must mark killed: {stopped}"
    );
}
