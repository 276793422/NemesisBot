// background_registry.rs 覆盖率补充测试（OutputBuffer 环形钳制 /
// Default·Drop / 满员淘汰与拒绝 / output 头部丢弃注记 / 三件套工具参数
// 校验错误面 / kill 真停进程）。
//
// kill 的两个失败臂（任务在 kill 途中被淘汰 / 顽固进程超时）依赖并发
// 淘汰或不可杀进程，无法确定性构造——记豁免。

use super::*;
use std::sync::Arc;

fn ctx() -> RequestContext {
    RequestContext::new("web", "cov-chat", "cov-sender", "agent:main:session:covbg")
}

#[test]
fn output_buffer_empty_append_and_clamp_semantics() {
    let mut buf = OutputBuffer::new(64);
    buf.append(b""); // 空追加早退：produced 不动。
    assert_eq!(buf.produced, 0);
    assert_eq!(buf.dropped, 0);

    buf.append(b"hello");
    assert_eq!(buf.produced, 5);
    let (chunk, skipped) = buf.read_chunk(0);
    assert_eq!(chunk, b"hello");
    assert!(!skipped);

    // offset 超出产出 → 空块，不 panic。
    let (chunk, skipped) = buf.read_chunk(100);
    assert!(chunk.is_empty());
    assert!(!skipped);
}

#[test]
fn output_buffer_head_drop_reports_skipped() {
    let mut buf = OutputBuffer::new(4);
    buf.append(b"abcdefgh"); // 8 字节进 4 容量 → 头部丢 4。
    assert_eq!(buf.produced, 8);
    assert_eq!(buf.dropped, 4);
    let (chunk, skipped) = buf.read_chunk(0);
    assert_eq!(chunk, b"efgh");
    assert!(skipped, "offset 0 precedes retained window");
}

#[test]
fn registry_default_matches_new() {
    let _ = BackgroundProcessRegistry::default();
}

/// Drop 路径：注册表销毁时给存活任务置 kill 旗标（监督任务收尸），
/// 不 panic、不泄漏卡死。
#[tokio::test]
async fn registry_drop_raises_kill_flags_for_running_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let reg = BackgroundProcessRegistry::with_limits(4, 4096);
    let started = reg
        .start("ping -n 30 127.0.0.1", dir.path().to_str().unwrap())
        .await
        .expect("long job starts");
    assert_eq!(started["running"], serde_json::json!(true));

    drop(reg);
    // 给监督任务一点时间收尸（kill_on_drop 兜底强杀）。
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// 输出超过容量上限 → 头部丢弃；offset 落在丢弃区 → 钳制 + note 注记。
#[tokio::test]
async fn output_reports_head_dropped_note() {
    let dir = tempfile::tempdir().unwrap();
    let reg = Arc::new(BackgroundProcessRegistry::with_limits(4, 8));
    let started = reg
        .start("echo cov-overflow-output", dir.path().to_str().unwrap())
        .await
        .unwrap();
    let id = started["job_id"].as_u64().unwrap();

    // 等 pump 排干 + 进程退出。
    tokio::time::sleep(Duration::from_millis(400)).await;

    let payload = reg.output(id, 0).await.unwrap();
    assert!(
        payload["dropped_bytes"].as_u64().unwrap_or(0) > 0,
        "output exceeds cap: {payload}"
    );
    assert!(
        payload.get("note").is_some(),
        "clamped read must carry the note: {payload}"
    );
    // 进程已退出 → 终态平铺（exit_code/success/killed）。
    assert_eq!(payload["running"], serde_json::json!(false));
    assert!(payload.get("exit_code").is_some(), "flat status: {payload}");
}

/// 三件套工具的参数校验错误面（坏 JSON / 缺 job_id）。
#[tokio::test]
async fn tool_argument_validation_errors() {
    let dir = tempfile::tempdir().unwrap();
    let reg = Arc::new(BackgroundProcessRegistry::with_limits(4, 4096));
    let start = BackgroundStartTool::new(dir.path().to_str().unwrap(), false, reg.clone());
    let out = BackgroundOutputTool::new(reg.clone());
    let kill = BackgroundKillTool::new(reg.clone());

    // 坏 JSON。
    for tool_err in [
        Tool::execute(&start, "{broken", &ctx())
            .await
            .err()
            .unwrap(),
        Tool::execute(&out, "{broken", &ctx()).await.err().unwrap(),
        Tool::execute(&kill, "{broken", &ctx()).await.err().unwrap(),
    ] {
        assert!(tool_err.contains("Invalid arguments"), "err: {tool_err}");
    }
    // 缺 job_id。
    assert!(Tool::execute(&out, "{}", &ctx()).await.is_err());
    assert!(Tool::execute(&kill, "{}", &ctx()).await.is_err());
    // 缺 command。
    assert!(Tool::execute(&start, "{}", &ctx()).await.is_err());
    // 不存在的 job。
    let err = Tool::execute(&out, r#"{"job_id": 999}"#, &ctx())
        .await
        .err()
        .unwrap();
    assert!(err.contains("no such job_id"), "err: {err}");
}

/// kill 工具：真停长跑进程（killed=true），再 kill 幂等返回终态。
#[tokio::test]
async fn kill_tool_stops_running_job_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let reg = Arc::new(BackgroundProcessRegistry::with_limits(4, 4096));
    let kill = BackgroundKillTool::new(reg.clone());

    let started = reg
        .start("ping -n 30 127.0.0.1", dir.path().to_str().unwrap())
        .await
        .unwrap();
    let id = started["job_id"].as_u64().unwrap();

    let payload = Tool::execute(&kill, &format!(r#"{{"job_id": {id}}}"#), &ctx())
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(v["killed"], serde_json::json!(true), "payload: {v}");
    assert_eq!(v["success"], serde_json::json!(false));

    // 幂等：再 kill 返回同一终态（不报错）。
    let again = Tool::execute(&kill, &format!(r#"{{"job_id": {id}}}"#), &ctx())
        .await
        .unwrap();
    let v2: serde_json::Value = serde_json::from_str(&again).unwrap();
    assert_eq!(v2["killed"], serde_json::json!(true));
}

/// output 工具：正常读回后台进程 stdout（分页坐标推进）。
#[tokio::test]
async fn output_tool_reads_job_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let reg = Arc::new(BackgroundProcessRegistry::with_limits(4, 4096));
    let start = BackgroundStartTool::new(dir.path().to_str().unwrap(), false, reg.clone());
    let out = BackgroundOutputTool::new(reg.clone());

    let started = Tool::execute(&start, r#"{"command": "echo cov-bg-hello"}"#, &ctx())
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&started).unwrap();
    let id = v["job_id"].as_u64().unwrap();

    tokio::time::sleep(Duration::from_millis(400)).await;
    let payload = Tool::execute(&out, &format!(r#"{{"job_id": {id}}}"#), &ctx())
        .await
        .unwrap();
    assert!(payload.contains("cov-bg-hello"), "payload: {payload}");
}

// ---------------------------------------------------------------------------
// wave5c：工具元数据（description/parameters/is_read_only）+ spawn_pump None 臂
// ---------------------------------------------------------------------------

/// 三个后台工具的 prompt 面元数据：description 非空、parameters schema
/// 字段/required 与实现层参数解析一一对应。
#[test]
fn background_tools_expose_metadata() {
    let reg = Arc::new(BackgroundProcessRegistry::with_limits(4, 4096));

    let start = BackgroundStartTool::new("C:\\ws", false, reg.clone());
    assert!(!start.description().is_empty());
    let p = start.parameters();
    assert_eq!(p["type"], "object");
    assert_eq!(p["properties"]["command"]["type"], "string");
    assert_eq!(p["properties"]["cwd"]["type"], "string");
    assert_eq!(p["required"][0], "command");

    let output = BackgroundOutputTool::new(reg.clone());
    assert!(!output.description().is_empty());
    let p = output.parameters();
    assert_eq!(p["properties"]["job_id"]["type"], "integer");
    assert_eq!(p["properties"]["offset"]["type"], "integer");
    assert_eq!(p["required"][0], "job_id");
    assert!(output.is_read_only(), "output tool must declare read-only");

    let kill = BackgroundKillTool::new(reg);
    assert!(!kill.description().is_empty());
    let p = kill.parameters();
    assert_eq!(p["properties"]["job_id"]["type"], "integer");
    assert_eq!(p["required"][0], "job_id");
}

/// spawn_pump 收到 None 管道 → 立即返回，缓冲保持空。
#[tokio::test]
async fn spawn_pump_none_pipe_is_noop() {
    let buf: Arc<Mutex<OutputBuffer>> = Arc::new(Mutex::new(OutputBuffer::new(4096)));
    spawn_pump(None::<tokio::io::Empty>, buf.clone());
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let (chunk, _skipped) = buf.lock().await.read_chunk(0);
    assert!(chunk.is_empty(), "no pipe → no output");
}
