//! B4（2026-09-05）后台进程三件套测试。
//!
//! 全部走真实子进程（cmd /C 或 sh -c），覆盖：echo 完成读回、退出码、
//! 增量分页、kill（含幂等）、上限拒绝与已完成任务淘汰、输出 cap 丢头部
//! 保尾部、cwd 工作区边界、注册表 Drop 树杀残余、注册接线三件套。

use std::sync::Arc;
use std::time::Duration;

use tokio::time::{Instant, sleep};

use super::*;
use crate::context::RequestContext;
use crate::r#loop::Tool;
use crate::loop_tools::{SharedToolConfig, register_shared_tools};

/// 真实子进程用例的模块级串行闸：cargo 默认并行调度下多用例同时
/// spawn/树杀会放大进程创建与终止延迟（慢机上 taskkill 单发可达秒级），
/// 撞破 [`KILL_WAIT`] 的 5s 窗口 → kill() 诚实超时 → 断言假红。
/// 串行化保确定性（生产无此竞争：工具调用天然串行穿 dispatch）。
/// 纯逻辑用例（unknown_job_id_errors / tool_read_only_flags /
/// registration_requires_registry）不起进程，不参与。
/// 先例：chat_event_log GLOBAL_TABLE_LOCK。
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

/// 长跑命令（不会自己退出，靠 kill 收尸）。
fn long_cmd() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "ping -n 30 127.0.0.1"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "sleep 30"
    }
}

/// 两段输出、中间隔 ~1s 的增量命令。
fn incremental_cmd() -> String {
    #[cfg(target_os = "windows")]
    {
        "echo one_b4& ping -n 2 127.0.0.1 >nul& echo two_b4".to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        "echo one_b4; sleep 1; echo two_b4".to_string()
    }
}

/// ~17KB 输出（超 4KB 测试 cap 用）。
fn big_output_cmd() -> String {
    #[cfg(target_os = "windows")]
    {
        "for /L %i in (1,1,800) do @echo abcdefghijklmnopqrst".to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        "yes abcdefghijklmnopqrst | head -n 800".to_string()
    }
}

async fn wait_done(reg: &BackgroundProcessRegistry, id: u64, secs: u64) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let v = reg.output(id, 0).await.expect("output should exist");
        if !v["running"].as_bool().expect("running field") {
            return v;
        }
        assert!(
            Instant::now() < deadline,
            "job {} did not finish in time",
            id
        );
        sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// 基础：起、读、退
// ---------------------------------------------------------------------------

#[tokio::test]
async fn start_echo_completes_with_output() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let started = reg.start("echo hello_b4_marker", "/").await.unwrap();
    let id = started["job_id"].as_u64().unwrap();
    let done = wait_done(&reg, id, 15).await;

    assert_eq!(done["exit_code"].as_i64(), Some(0));
    assert_eq!(done["success"].as_bool(), Some(true));
    assert_eq!(done["killed"].as_bool(), Some(false));
    let chunk = done["chunk"].as_str().unwrap();
    assert!(chunk.contains("hello_b4_marker"), "chunk: {}", chunk);
    // 元数据回显（模型用来辨认任务）。
    assert_eq!(done["command"].as_str(), Some("echo hello_b4_marker"));
    assert!(done["started_at_unix"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn exit_code_preserved() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let started = reg.start("exit 7", "/").await.unwrap();
    let id = started["job_id"].as_u64().unwrap();
    let done = wait_done(&reg, id, 15).await;
    assert_eq!(done["exit_code"].as_i64(), Some(7));
    assert_eq!(done["success"].as_bool(), Some(false));
}

#[tokio::test]
async fn unknown_job_id_errors() {
    let reg = BackgroundProcessRegistry::new();
    assert!(reg.output(9999, 0).await.is_err());
    assert!(reg.kill(9999).await.is_err());
}

// ---------------------------------------------------------------------------
// kill：运行中、幂等
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kill_running_job_reports_killed() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let started = reg.start(long_cmd(), "/").await.unwrap();
    let id = started["job_id"].as_u64().unwrap();

    let running = reg.output(id, 0).await.unwrap();
    assert_eq!(running["running"].as_bool(), Some(true));

    let result = reg.kill(id).await.unwrap();
    assert_eq!(result["killed"].as_bool(), Some(true));
    assert_eq!(result["success"].as_bool(), Some(false));

    // 之后读输出也是完成态。
    let after = reg.output(id, 0).await.unwrap();
    assert_eq!(after["running"].as_bool(), Some(false));
}

#[tokio::test]
async fn kill_finished_job_is_idempotent() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let started = reg.start("echo quick_b4", "/").await.unwrap();
    let id = started["job_id"].as_u64().unwrap();
    wait_done(&reg, id, 15).await;

    let result = reg.kill(id).await.unwrap();
    // 自然退出——killed 旗标没参与，如实标 false。
    assert_eq!(result["killed"].as_bool(), Some(false));
    assert_eq!(result["exit_code"].as_i64(), Some(0));
}

// ---------------------------------------------------------------------------
// 上限：全在跑拒绝；已完成淘汰
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cap_rejects_when_all_running() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::with_limits(2, 64 * 1024);
    let j1 = reg.start(long_cmd(), "/").await.unwrap();
    let j2 = reg.start(long_cmd(), "/").await.unwrap();

    let err = reg.start(long_cmd(), "/").await.unwrap_err();
    assert!(err.contains("limit"), "err: {}", err);

    // 清场：杀掉两个长跑任务（不留 30s 孤儿给测试 runner）。
    reg.kill(j1["job_id"].as_u64().unwrap()).await.unwrap();
    reg.kill(j2["job_id"].as_u64().unwrap()).await.unwrap();
}

#[tokio::test]
async fn finished_job_evicted_at_cap() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::with_limits(2, 64 * 1024);
    let echo_id = reg.start("echo evict_b4", "/").await.unwrap()["job_id"]
        .as_u64()
        .unwrap();
    wait_done(&reg, echo_id, 15).await;

    let s1 = reg.start(long_cmd(), "/").await.unwrap();
    // 满员：echo 已完成 → 被淘汰，新任务顶上。
    let s2 = reg.start(long_cmd(), "/").await.unwrap();

    assert_eq!(reg.job_count().await, 2);
    assert!(
        reg.output(echo_id, 0).await.is_err(),
        "evicted job should be gone"
    );
    reg.kill(s1["job_id"].as_u64().unwrap()).await.unwrap();
    reg.kill(s2["job_id"].as_u64().unwrap()).await.unwrap();
}

// ---------------------------------------------------------------------------
// 输出：cap 丢头部保尾部 + 分页
// ---------------------------------------------------------------------------

#[tokio::test]
async fn output_cap_drops_head_keeps_tail() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::with_limits(1, 4096);
    let started = reg.start(&big_output_cmd(), "/").await.unwrap();
    let id = started["job_id"].as_u64().unwrap();
    let done = wait_done(&reg, id, 20).await;

    let total = done["total_bytes"].as_u64().unwrap();
    let dropped = done["dropped_bytes"].as_u64().unwrap();
    assert!(
        total > 4096,
        "produced {} bytes, expected to exceed the 4096 test cap",
        total
    );
    assert!(dropped > 0, "head should have been dropped");

    // offset 0 落在被丢弃区间 → 钳到窗口起点 + 诚实注记。
    let head = reg.output(id, 0).await.unwrap();
    assert!(head["note"].as_str().is_some(), "skipped note present");
    assert_eq!(head["offset"].as_u64(), Some(0));

    // 尾部数据完好：从 (total - 64) 读，内容是行填充。
    let tail = reg.output(id, total - 64).await.unwrap();
    let chunk = tail["chunk"].as_str().unwrap();
    assert!(
        chunk.contains("abcdefghijklmnopqrst"),
        "tail chunk: {}",
        chunk
    );
}

#[tokio::test]
async fn offset_paging_covers_output() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let cmd = incremental_cmd();
    let started = reg.start(&cmd, "/").await.unwrap();
    let id = started["job_id"].as_u64().unwrap();
    let done = wait_done(&reg, id, 20).await;
    let total = done["total_bytes"].as_u64().unwrap();

    // 从 offset 0 一页页读到尾，拼接应覆盖全部字节。
    let mut offset = 0u64;
    let mut joined = String::new();
    for _ in 0..100 {
        let page = reg.output(id, offset).await.unwrap();
        assert_eq!(page["offset"].as_u64(), Some(offset));
        let chunk = page["chunk"].as_str().unwrap().to_string();
        joined.push_str(&chunk);
        let next = page["next_offset"].as_u64().unwrap();
        if chunk.is_empty() && next == offset {
            break;
        }
        offset = next;
        if offset >= total {
            break;
        }
    }
    assert!(joined.contains("one_b4"), "joined: {}", joined);
    assert!(joined.contains("two_b4"), "joined: {}", joined);
}

// ---------------------------------------------------------------------------
// 工具层：cwd 边界 + read-only 旗标 + 注册接线
// ---------------------------------------------------------------------------

fn temp_ws(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "b4ws_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn cwd_outside_workspace_rejected() {
    let _serial = TEST_LOCK.lock().await;
    let ws = temp_ws("inside");
    let outside = temp_ws("outside");
    let reg = Arc::new(BackgroundProcessRegistry::new());
    let tool = BackgroundStartTool::new(&ws.to_string_lossy(), true, reg.clone());

    let args = serde_json::json!({
        "command": "echo hi",
        "cwd": outside.to_string_lossy(),
    });
    let err = tool.execute(&args.to_string(), &ctx()).await.unwrap_err();
    assert!(err.contains("outside workspace"), "err: {}", err);

    // 相对 cwd 解析到工作区内 → 放行（真实起进程），解析后的绝对路径回显。
    std::fs::create_dir_all(ws.join("sub")).unwrap();
    let args = serde_json::json!({"command": "echo rel_b4", "cwd": "sub"});
    let out = tool.execute(&args.to_string(), &ctx()).await.unwrap();
    let started = serde_json::from_str::<serde_json::Value>(&out).unwrap();
    assert!(
        started["cwd"].as_str().unwrap().contains("sub"),
        "resolved cwd should be the workspace-joined absolute path: {}",
        started["cwd"]
    );
    let done = wait_done(&reg, started["job_id"].as_u64().unwrap(), 15).await;
    assert!(done["chunk"].as_str().unwrap().contains("rel_b4"));

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn tool_read_only_flags() {
    let reg = Arc::new(BackgroundProcessRegistry::new());
    assert!(BackgroundOutputTool::new(reg.clone()).is_read_only());
    assert!(!BackgroundStartTool::new("/", true, reg.clone()).is_read_only());
    assert!(!BackgroundKillTool::new(reg).is_read_only());
}

#[tokio::test]
async fn registration_requires_registry() {
    let ws = "/tmp".to_string();

    // None → 三件套不注册（exec_worker / 基线形态）。
    let cfg = SharedToolConfig {
        workspace: Some(ws.clone()),
        background_registry: None,
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(!tools.contains_key("background_start"));
    assert!(!tools.contains_key("background_output"));
    assert!(!tools.contains_key("background_kill"));

    // Some → 三件套齐全。
    let cfg = SharedToolConfig {
        workspace: Some(ws),
        background_registry: Some(Arc::new(BackgroundProcessRegistry::new())),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(tools.contains_key("background_start"));
    assert!(tools.contains_key("background_output"));
    assert!(tools.contains_key("background_kill"));
}

// ---------------------------------------------------------------------------
// Drop 树杀：注册表销毁后进程活不过 SUPERVISE_INTERVAL + tree_kill
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drop_kills_residual_process() {
    let _serial = TEST_LOCK.lock().await;
    let reg = Arc::new(BackgroundProcessRegistry::new());
    let started = reg.start(long_cmd(), "/").await.unwrap();
    let pid = started["pid"].as_u64().expect("pid present") as u32;

    drop(reg);
    sleep(Duration::from_millis(
        // SUPERVISE_INTERVAL + tree_kill 上限 + 余量。
        300 + 2_500,
    ))
    .await;

    assert!(!process_alive(pid), "pid {} should be dead after drop", pid);
}

/// 平台进程存活检查（locale 无关）。
#[cfg(target_os = "windows")]
fn process_alive(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FO", "CSV", "/NH", "/FI", &format!("PID eq {}", pid)])
        .output()
        .expect("tasklist");
    let text = String::from_utf8_lossy(&out.stdout);
    // CSV 第二列是 PID；引号包裹精确匹配，避免撞上内存列里的数字。
    text.contains(&format!("\"{}\"", pid))
}

#[cfg(not(target_os = "windows"))]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
