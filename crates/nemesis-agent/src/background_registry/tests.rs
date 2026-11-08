//! （2026-09-05）后台进程四动词测试（steer 为 2026-11-03 扩展）。
//!
//! 全部走真实子进程（cmd /C 或 sh -c），覆盖：echo 完成读回、退出码、
//! 增量分页、kill（含幂等）、上限拒绝与已完成任务淘汰、输出 cap 丢头部
//! 保尾部、cwd 工作区边界、注册表 Drop 树杀残余、注册接线四动词、
//! steer e2e 回执（stdin 环回）与诚实拒绝面。

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
/// chat_event_log GLOBAL_TABLE_LOCK。
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
    let started = reg.start("echo hello_b4_marker", "/", false).await.unwrap();
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
    let started = reg.start("exit 7", "/", false).await.unwrap();
    let id = started["job_id"].as_u64().unwrap();
    let done = wait_done(&reg, id, 15).await;
    assert_eq!(done["exit_code"].as_i64(), Some(7));
    assert_eq!(done["success"].as_bool(), Some(false));
}

/// list() 投影：id 倒序、字段齐全、运行/终态两态正确、终态平铺。
#[tokio::test]
async fn list_projects_rows_newest_first_with_exit_flat() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let fin = reg
        .start("echo list_done_marker", "/", false)
        .await
        .unwrap();
    let fin_id = fin["job_id"].as_u64().unwrap();
    let long = reg.start(long_cmd(), "/", false).await.unwrap();
    let long_id = long["job_id"].as_u64().unwrap();
    wait_done(&reg, fin_id, 15).await;

    let rows = reg.list().await;
    assert!(rows.len() >= 2, "两任务都在册: {}", rows.len());
    let ids: Vec<u64> = rows.iter().map(|r| r["id"].as_u64().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.reverse();
    assert_eq!(ids, sorted, "id 倒序 = 新任务在前");

    let fin_row = rows
        .iter()
        .find(|r| r["id"].as_u64() == Some(fin_id))
        .unwrap();
    assert_eq!(fin_row["running"], serde_json::json!(false));
    assert_eq!(fin_row["exit_code"].as_i64(), Some(0));
    assert_eq!(fin_row["success"], serde_json::json!(true));
    assert_eq!(fin_row["killed"], serde_json::json!(false));
    assert_eq!(fin_row["steerable"], serde_json::json!(false));
    assert!(fin_row["total_bytes"].as_u64().unwrap() > 0);
    assert_eq!(fin_row["dropped_bytes"], serde_json::json!(0));
    assert!(fin_row["started_at_unix"].as_u64().unwrap() > 0);
    assert!(
        fin_row["command"]
            .as_str()
            .unwrap()
            .contains("list_done_marker")
    );

    let long_row = rows
        .iter()
        .find(|r| r["id"].as_u64() == Some(long_id))
        .unwrap();
    assert_eq!(long_row["running"], serde_json::json!(true));
    // 运行中行无终态字段平铺。
    assert!(long_row.get("exit_code").is_none());

    // kill 后行翻终态（killed=true 平铺）。
    reg.kill(long_id).await.unwrap();
    let rows = reg.list().await;
    let long_row = rows
        .iter()
        .find(|r| r["id"].as_u64() == Some(long_id))
        .unwrap();
    assert_eq!(long_row["running"], serde_json::json!(false));
    assert_eq!(long_row["killed"], serde_json::json!(true));
}

#[tokio::test]
async fn unknown_job_id_errors() {
    let reg = BackgroundProcessRegistry::new();
    assert!(reg.output(9999, 0).await.is_err());
    assert!(reg.kill(9999).await.is_err());
    assert!(reg.steer(9999, "x").await.is_err());
}

// ---------------------------------------------------------------------------
// kill：运行中、幂等
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kill_running_job_reports_killed() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let started = reg.start(long_cmd(), "/", false).await.unwrap();
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
    let started = reg.start("echo quick_b4", "/", false).await.unwrap();
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
    let j1 = reg.start(long_cmd(), "/", false).await.unwrap();
    let j2 = reg.start(long_cmd(), "/", false).await.unwrap();

    let err = reg.start(long_cmd(), "/", false).await.unwrap_err();
    assert!(err.contains("limit"), "err: {}", err);

    // 清场：杀掉两个长跑任务（不留 30s 孤儿给测试 runner）。
    reg.kill(j1["job_id"].as_u64().unwrap()).await.unwrap();
    reg.kill(j2["job_id"].as_u64().unwrap()).await.unwrap();
}

#[tokio::test]
async fn finished_job_evicted_at_cap() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::with_limits(2, 64 * 1024);
    let echo_id = reg.start("echo evict_b4", "/", false).await.unwrap()["job_id"]
        .as_u64()
        .unwrap();
    wait_done(&reg, echo_id, 15).await;

    let s1 = reg.start(long_cmd(), "/", false).await.unwrap();
    // 满员：echo 已完成 → 被淘汰，新任务顶上。
    let s2 = reg.start(long_cmd(), "/", false).await.unwrap();

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
    let started = reg.start(&big_output_cmd(), "/", false).await.unwrap();
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
    let started = reg.start(&cmd, "/", false).await.unwrap();
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
    assert!(!BackgroundKillTool::new(reg.clone()).is_read_only());
    // steer 是写动作（stdin 注入）。
    assert!(!BackgroundSteerTool::new(reg).is_read_only());
}

#[tokio::test]
async fn registration_requires_registry() {
    let ws = "/tmp".to_string();

    // None → 四动词不注册（exec_worker / 基线形态）。
    let cfg = SharedToolConfig {
        workspace: Some(ws.clone()),
        background_registry: None,
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(!tools.contains_key("background_start"));
    assert!(!tools.contains_key("background_output"));
    assert!(!tools.contains_key("background_kill"));
    assert!(!tools.contains_key("background_steer"));

    // Some → 四动词齐全（起含 steer）。
    let cfg = SharedToolConfig {
        workspace: Some(ws),
        background_registry: Some(Arc::new(BackgroundProcessRegistry::new())),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(tools.contains_key("background_start"));
    assert!(tools.contains_key("background_output"));
    assert!(tools.contains_key("background_kill"));
    assert!(tools.contains_key("background_steer"));
}

// ---------------------------------------------------------------------------
// steer 动词：stdin 活管道 + 注入回执
// ---------------------------------------------------------------------------

/// stdin→stdout 回显过滤器（steer e2e 的回执语义载体：注入什么回什么）。
/// Windows 用 `more`（管道形态无分页提示，逐块流式回显）；Unix 用 `cat`。
fn echo_filter_cmd() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "more"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "cat"
    }
}

/// 轮询任务输出直到出现 `needle`（steer 回执等待）。
async fn wait_output_contains(
    reg: &BackgroundProcessRegistry,
    id: u64,
    needle: &str,
    secs: u64,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let v = reg.output(id, 0).await.expect("output should exist");
        let chunk = v["chunk"].as_str().unwrap_or("").to_string();
        if chunk.contains(needle) {
            return chunk;
        }
        assert!(
            Instant::now() < deadline,
            "steer receipt '{}' did not appear within {}s; last chunk: {}",
            needle,
            secs,
            chunk
        );
        sleep(Duration::from_millis(100)).await;
    }
}

/// 验收 e2e：spawn 可操控常驻进程 → steer 注入 → 回执语义断言（注入文本
/// 经 stdin→stdout 环回出现在输出缓冲）。回执出现即证「stdin 活管道 +
/// 写入 + flush」全链路真实打通。
#[tokio::test]
async fn steer_e2e_echo_receipt() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let started = reg.start(echo_filter_cmd(), "/", true).await.unwrap();
    let id = started["job_id"].as_u64().unwrap();
    assert_eq!(started["steerable"].as_bool(), Some(true));

    let receipt = reg.steer(id, "steer_l11_marker_xyz").await.unwrap();
    assert_eq!(receipt["job_id"].as_u64(), Some(id));
    assert_eq!(receipt["running"].as_bool(), Some(true));

    let chunk = wait_output_contains(&reg, id, "steer_l11_marker_xyz", 10).await;
    // 回执原样环回（字节级写入，无协议封装痕迹）。
    assert!(chunk.contains("steer_l11_marker_xyz"));

    // 二连注入（写入队列串行形态）：两行先后落盘都可回读。
    reg.steer(id, "second_line_b4").await.unwrap();
    wait_output_contains(&reg, id, "second_line_b4", 10).await;

    // 清场（不留常驻进程给测试 runner）。
    let killed = reg.kill(id).await.unwrap();
    assert_eq!(killed["killed"].as_bool(), Some(true));
}

/// 非 steerable 任务 steer = 诚实拒绝（不是静默吞掉）。
#[tokio::test]
async fn steer_rejects_non_steerable_job() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let started = reg.start(long_cmd(), "/", false).await.unwrap();
    let id = started["job_id"].as_u64().unwrap();
    assert_eq!(started["steerable"].as_bool(), Some(false));

    let err = reg.steer(id, "hello").await.unwrap_err();
    assert!(err.contains("steerable"), "err: {}", err);

    reg.kill(id).await.unwrap();
}

/// 已结束任务 steer = 诚实拒绝（not running 检查在 steerable 检查之前）。
#[tokio::test]
async fn steer_finished_job_errors() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let started = reg.start("echo done_b4", "/", true).await.unwrap();
    let id = started["job_id"].as_u64().unwrap();
    wait_done(&reg, id, 15).await;

    let err = reg.steer(id, "late").await.unwrap_err();
    assert!(err.contains("not running"), "err: {}", err);
}

/// 工具层参数面：空文本拒绝 + 缺字段拒绝（不起进程）。
#[tokio::test]
async fn steer_tool_arg_guards() {
    let tool = BackgroundSteerTool::new(Arc::new(BackgroundProcessRegistry::new()));
    let empty = serde_json::json!({"job_id": 1, "text": "  "});
    let err = tool.execute(&empty.to_string(), &ctx()).await.unwrap_err();
    assert!(err.contains("empty"), "err: {}", err);

    let missing = serde_json::json!({"job_id": 1});
    let err = tool
        .execute(&missing.to_string(), &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("text"), "err: {}", err);
}

// ---------------------------------------------------------------------------
// Drop 树杀：注册表销毁后进程活不过 SUPERVISE_INTERVAL + tree_kill
// ---------------------------------------------------------------------------

#[tokio::test]
async fn drop_kills_residual_process() {
    let _serial = TEST_LOCK.lock().await;
    let reg = Arc::new(BackgroundProcessRegistry::new());
    let started = reg.start(long_cmd(), "/", false).await.unwrap();
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

// ---------------------------------------------------------------------------
// 停止阶梯：优雅停 → 宽限窗 → 强杀升级
// ---------------------------------------------------------------------------

/// TERM-忽略命令（POSIX 阶梯升级的前提：trap 掉 TERM 后必须靠 SIGKILL
/// 收场；Windows 无优雅通道，等价长跑命令）。
fn stubborn_cmd() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "ping -n 30 127.0.0.1"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "trap '' TERM; sleep 30"
    }
}

/// 直接 spawn 一个 shell 子进程（测 stop_ladder 本体，不经注册表）。
async fn spawn_shell(cmd: &str) -> (u32, Arc<tokio::sync::Mutex<tokio::process::Child>>) {
    let child = build_platform_shell(cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn shell");
    let pid = child.id().expect("pid before exit");
    (pid, Arc::new(tokio::sync::Mutex::new(child)))
}

#[tokio::test]
async fn stop_ladder_terminates_stubborn_process() {
    let _serial = TEST_LOCK.lock().await;
    let (pid, child) = spawn_shell(stubborn_cmd()).await;
    let started = Instant::now();
    stop_ladder(pid, &child, Duration::from_millis(500)).await;
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "ladder must finish promptly, took {:?}",
        started.elapsed()
    );
    let mut c = child.lock().await;
    // 阶梯契约 = 杀指令发完即返回，不等内核收割（见 stop_ladder 文档
    // 「调用方随后照常 try_wait 收尸」）。高载 runner 上 graceful 段 kill
    // 二进制 spawn 可吃掉大半宽限窗，升级段 KILL 投递→收割延迟几十 ms——
    // 返回后立即断言 = Ok(None) 假红（ET Linux，3107 过
    // 仅此 1 挂）。给有界收割窗，超窗才判死。
    let reap_deadline = Instant::now() + Duration::from_secs(10);
    let st = loop {
        if let Some(s) = c.try_wait().expect("wait ok") {
            break s;
        }
        assert!(
            Instant::now() < reap_deadline,
            "stubborn process must be dead after ladder (10s reap window exhausted)"
        );
        sleep(Duration::from_millis(50)).await;
    };
    #[cfg(not(target_os = "windows"))]
    assert!(
        st.code().is_none(),
        "TERM-ignoring process must die by SIGKILL (code None), got {:?}",
        st.code()
    );
    #[cfg(target_os = "windows")]
    assert!(!st.success(), "force-killed process must not be success");
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn stop_ladder_graceful_exit_within_window_no_escalation() {
    let _serial = TEST_LOCK.lock().await;
    // trap TERM → exit 0：优雅段信号即走，窗内自退，绝不升级强杀。
    // 竞态闸：spawn 返回只保证 exec(sh) 完成，不保证 trap 已装——TERM 落在
    // exec 与 trap 之间按默认处置死（code()=None 假红；高载 runner 上
    // 动态链接/启动被调度延迟，窗口可拉到几十 ms）。trap 装好后写 marker，
    // 等 marker 出现再发车：首记 TERM 必落在 trap 生效后。
    let tmp = tempfile::tempdir().expect("tempdir");
    let marker = tmp.path().join("trap_ready");
    let (pid, child) = spawn_shell(&format!(
        "trap 'exit 0' TERM; : > '{}'; sleep 30",
        marker.display()
    ))
    .await;
    let ready_deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(
            Instant::now() < ready_deadline,
            "trap 未就绪（marker 未出现）"
        );
        sleep(Duration::from_millis(20)).await;
    }
    let started = Instant::now();
    stop_ladder(pid, &child, Duration::from_secs(5)).await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "graceful exit must be fast, took {:?}",
        started.elapsed()
    );
    let mut c = child.lock().await;
    let st = c
        .try_wait()
        .expect("wait ok")
        .expect("graceful process must have exited within window");
    assert_eq!(
        st.code(),
        Some(0),
        "clean exit-0 proves no SIGKILL escalation fired"
    );
}

#[tokio::test]
async fn background_kill_ladder_terminates_stubborn_job_end_to_end() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let started = reg.start(stubborn_cmd(), "/", false).await.unwrap();
    let id = started["job_id"].as_u64().unwrap();
    // 阶梯升级前（TERM-only）：POSIX 上 TERM-忽略进程永远收不掉，kill()
    // 必落 KILL_WAIT 超时 Err；升级后 3s 宽限 + SIGKILL 落在 5s 窗内。
    let result = reg
        .kill(id)
        .await
        .expect("kill must succeed within KILL_WAIT now that the ladder escalates");
    assert_eq!(result["killed"].as_bool(), Some(true));
    assert_eq!(result["success"].as_bool(), Some(false));
}

/// 回归锁：background_start 的平台 shell 与 ExecTool
/// 「同语义」（本模块注释原文），语义含 环境凭据剥离——此前漏接。
/// 敏感探针变量必须剥、非敏感探针必须可见（探针管道自证）。
#[tokio::test]
async fn build_platform_shell_strips_sensitive_env() {
    nemesis_utils::env_sanitize::set_child_env_sanitize_enabled(true);
    unsafe { std::env::set_var("NB_BUG178_PROBE_KEY", "sensitive-marker") };
    unsafe { std::env::set_var("NB_BUG178_PROBE_PLAIN", "plain-marker") };
    #[cfg(windows)]
    let mut c =
        build_platform_shell("echo SENS=%NB_BUG178_PROBE_KEY% PLAIN=%NB_BUG178_PROBE_PLAIN%");
    #[cfg(not(windows))]
    let mut c = build_platform_shell("echo SENS=$NB_BUG178_PROBE_KEY PLAIN=$NB_BUG178_PROBE_PLAIN");
    c.stdout(std::process::Stdio::piped());
    let out = c
        .spawn()
        .expect("spawn ok")
        .wait_with_output()
        .await
        .expect("wait ok");
    unsafe { std::env::remove_var("NB_BUG178_PROBE_KEY") };
    unsafe { std::env::remove_var("NB_BUG178_PROBE_PLAIN") };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        text.contains("PLAIN=plain-marker"),
        "非敏感变量必须原样可见（探针管道自证）: {text}"
    );
    assert!(
        !text.contains("sensitive-marker"),
        "敏感变量必须被 T4 剥离，不得进入后台命令环境: {text}"
    );
}

// ---------------------------------------------------------------------------
// BUG-20261108-027：chunk 边界劈开多字节 UTF-8 字符 → 替换符 mojibake
// ---------------------------------------------------------------------------

/// 静默长跑命令（无 stdout 输出）：e2e 测试需要向运行中任务的缓冲注入
/// 合成字节，真实进程若同时产出会污染劈字符的确定性断言。
fn silent_cmd() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "ping -n 30 127.0.0.1 > nul"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "sleep 30"
    }
}

/// `utf8_safe_len` 判定矩阵：完整 UTF-8 全长消费；尾部不完整序列截到
/// 合法前缀末尾（整段就是半截 → 0）；中段真非法序列不截（lossy 诚实
/// 降级 + 保证推进）。
#[test]
fn utf8_safe_len_matrix_bug_20261108_027() {
    use super::utf8_safe_len;
    // 完整 UTF-8（ASCII / CJK / emoji 混排）→ 全长。
    assert_eq!(utf8_safe_len("ab".as_bytes()), 2);
    assert_eq!(utf8_safe_len("你好".as_bytes()), 6);
    assert_eq!(utf8_safe_len("x🦀z".as_bytes()), "x🦀z".len());
    // 「你」= E4 BD A0 劈尾：合法前缀 + 尾部不完整序列 → 截到前缀末尾。
    assert_eq!(utf8_safe_len(&[0x61, 0xE4, 0xBD]), 1); // 'a' + 半个你
    assert_eq!(utf8_safe_len(&[0xE4, 0xBD]), 0); // 整段就是半截 → 空消费
    assert_eq!(utf8_safe_len(&[0xE4]), 0);
    // emoji 🦀 = F0 9F A6 80 劈在各中段 → 整段属于不完整序列 → 0。
    assert_eq!(utf8_safe_len(&[0xF0, 0x9F]), 0);
    assert_eq!(utf8_safe_len(&[0xF0, 0x9F, 0xA6]), 0);
    // 中段真非法序列（error_len Some）不截：continuation 字节开头（头部
    // 钳制落进字符中间的诚实降级面）与裸非法字节。
    assert_eq!(utf8_safe_len(&[0xBD, 0xA0]), 2);
    assert_eq!(utf8_safe_len(&[0x61, 0xFF, 0x62]), 3);
}

/// e2e 回归锁（真实运行中任务 + 注入合成字节）：劈在轮询边界的 CJK
/// 三字节字符必须跨轮拼回完整字符，不得渲染替换符；运行中 next_offset
/// 停在字符边界（不越过半截字节），终态语义（推进到底）由既有
/// offset_paging_covers_output / output_cap_drops_head_keeps_tail 覆盖。
#[tokio::test]
async fn split_cjk_char_composes_across_polls_bug_20261108_027() {
    let _serial = TEST_LOCK.lock().await;
    let reg = BackgroundProcessRegistry::new();
    let started = reg.start(silent_cmd(), "/", false).await.unwrap();
    let id = started["job_id"].as_u64().unwrap();

    // 向运行中任务的输出缓冲注入「你」的前两个字节（绕过真实进程输出
    // 时机的非确定性，确定性构造「字符劈在 chunk 边界」场景）。
    let output_buf = {
        let jobs = reg.jobs.lock().await;
        jobs.get(&id).expect("job entry").output.clone() // Arc<Mutex<OutputBuffer>> —— guard 必须先放（output 会再锁 jobs）
    };
    output_buf.lock().await.append(&[0xE4, 0xBD]);

    // 第一轮：尾部不完整序列被截住——不渲染替换符，offset 不越过半截
    // 字节（等下一轮与后续输出拼齐）。
    let first = reg.output(id, 0).await.expect("first poll");
    let chunk1 = first["chunk"].as_str().unwrap().to_string();
    let off1 = first["next_offset"].as_u64().unwrap();
    let total1 = first["total_bytes"].as_u64().unwrap();
    assert_eq!(
        total1, 2,
        "silent cmd must produce no output of its own: total={total1}"
    );
    assert!(
        !chunk1.contains('\u{FFFD}'),
        "running job must not render the split tail: {chunk1:?}"
    );
    assert_eq!(
        off1,
        total1 - 2,
        "next_offset must stop before the incomplete 3-byte char"
    );

    // 注入剩余字节：字符拼齐后下一轮读回完整「你」。
    output_buf.lock().await.append(&[0xA0]);
    let second = reg.output(id, off1).await.expect("second poll");
    let chunk2 = second["chunk"].as_str().unwrap();
    assert_eq!(
        chunk2, "你",
        "split char must compose across polls: {chunk2:?}"
    );
    assert_eq!(second["next_offset"].as_u64(), Some(off1 + 3));

    // 收尾：树杀长跑命令，不留挂任务。
    let result = reg.kill(id).await.expect("kill within KILL_WAIT");
    assert_eq!(result["killed"].as_bool(), Some(true));
}
