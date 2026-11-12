//! [`crate::mcp_serve`] 单元测试：工具清单契约 / 协议通知判定 / 轻量记忆
//! 检索 / 装配结构断言（security on/off 行为差异——安全同源证据之一，
//! 端到端行为证据见 `tests/mcp_serve_stdio.rs` 审计链用例）。

use super::*;

// ---------------------------------------------------------------------------
// 工具清单契约
// ---------------------------------------------------------------------------

#[test]
fn tool_definitions_has_four_v1_tools() {
    let defs = tool_definitions();
    let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["board_issue_query", "memory_search", "run", "sessions_list"],
        "v1 工具面必须是这四个"
    );
    for d in &defs {
        assert!(!d.name.is_empty());
        assert!(
            d.description
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "工具 {} 必须带描述（外部 MCP 客户端靠它选型）",
            d.name
        );
        assert_eq!(
            d.input_schema["type"], "object",
            "{} schema 顶层 object",
            d.name
        );
    }
}

#[test]
fn run_and_memory_search_declare_required_args() {
    let defs = tool_definitions();
    let run = defs.iter().find(|d| d.name == "run").unwrap();
    assert_eq!(
        run.input_schema["required"],
        serde_json::json!(["task"]),
        "run 必填 task"
    );
    let mem = defs.iter().find(|d| d.name == "memory_search").unwrap();
    assert_eq!(
        mem.input_schema["required"],
        serde_json::json!(["query"]),
        "memory_search 必填 query"
    );
}

// ---------------------------------------------------------------------------
// 协议通知判定
// ---------------------------------------------------------------------------

#[test]
fn is_notification_distinguishes_frames() {
    // 带 id 的请求 → 不是通知（要回帧）。
    assert!(!is_notification(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#
    ));
    assert!(!is_notification(
        r#"{"jsonrpc":"2.0","id":"abc","method":"tools/list"}"#
    ));
    // 无 id 帧（notifications/initialized）→ 通知（不回帧）。
    assert!(is_notification(
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
    ));
    // id 显式为 null = JSON-RPC 里仍算「有 id」位置……按无 id 字段语义判定
    // 为通知——get("id") 返回 Some(Value::Null)？不：serde_json 的 get 对
    // "id": null 返回 Some(Null)，所以带 null id 的帧不算通知（会回帧，
    // handle_raw 的响应 id 也是 Null——两端对得上）。
    assert!(!is_notification(
        r#"{"jsonrpc":"2.0","id":null,"method":"x"}"#
    ));
    // 垃圾行 → 不是通知（走 handle_raw 的 -32700 回帧路径）。
    assert!(!is_notification("not json at all"));
}

// ---------------------------------------------------------------------------
// 轻量记忆检索（workspace/memory 文本回落路径）
// ---------------------------------------------------------------------------

fn write_file(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn search_memory_files_finds_case_insensitive_with_line_numbers() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write_file(
        &ws.join("memory").join("MEMORY.md"),
        "# Memory\n\n用户偏好深色主题。\nDeployment uses blue-green.\n",
    );
    write_file(
        &ws.join("memory").join("notes").join("sub.md"),
        "deployment checklist: run tests\n",
    );

    let hits = search_memory_files(ws, "deployment", 10);
    assert_eq!(hits.len(), 2, "大小写不敏感命中两个文件");
    assert_eq!(hits[0].file, "memory/MEMORY.md");
    assert_eq!(hits[0].line_no, 4);
    assert_eq!(hits[0].line, "Deployment uses blue-green.");
    assert_eq!(hits[1].file, "memory/notes/sub.md");
    assert_eq!(hits[1].line_no, 1);
}

#[test]
fn search_memory_files_respects_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write_file(
        &ws.join("memory").join("MEMORY.md"),
        "alpha\nalpha\nalpha\nalpha\nalpha\n",
    );
    let hits = search_memory_files(ws, "alpha", 3);
    assert_eq!(hits.len(), 3, "limit 截断");
}

#[test]
fn collect_markdown_files_walks_in_sorted_order() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    // ≤200 截断发生在收集过程中（提前 return），收集序
    // 曾跟随 read_dir 平台序（NTFS 字典序 vs ext4 哈希序）——大记忆库下
    // 哪些文件参与轻量检索会跨平台漂移。每层按文件名排序后必须确定。
    write_file(&ws.join("memory").join("zeta.md"), "z");
    write_file(&ws.join("memory").join("alpha.md"), "a");
    write_file(&ws.join("memory").join("notes").join("mid.md"), "m");
    write_file(&ws.join("memory").join("skip.txt"), "not md");

    let mut out = Vec::new();
    collect_markdown_files(&ws.join("memory"), 0, &mut out);
    let names: Vec<String> = out
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["alpha.md", "mid.md", "zeta.md"],
        "collect must walk name-sorted (deterministic truncation set) and skip non-md: {names:?}"
    );
}

#[test]
fn search_memory_files_missing_dir_is_empty_not_error() {
    let tmp = tempfile::tempdir().unwrap();
    let hits = search_memory_files(tmp.path(), "anything", 10);
    assert!(hits.is_empty(), "memory 目录不存在 = 空结果");
}

#[test]
fn search_memory_files_miss_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        &tmp.path().join("memory").join("MEMORY.md"),
        "完全无关内容\n",
    );
    assert!(search_memory_files(tmp.path(), "nonexistent-xyz", 10).is_empty());
}

// ---------------------------------------------------------------------------
// 装配结构断言（security on/off 行为差异）
// ---------------------------------------------------------------------------

/// 最小可装配 config（死地址 provider——装配不拨号，与 run/tests.rs 同款）。
fn dead_provider_config() -> serde_json::Value {
    serde_json::json!({
        "agents": {"defaults": {"llm": "fake"}},
        "model_list": [{
            "model_name": "fake",
            "model": "openai/gpt-fake",
            "api_base": "http://127.0.0.1:1",
            "api_key": "k"
        }]
    })
}

/// 造一个带 config.json 的临时 home（调用方再按需改写 security 键）。
fn temp_home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("workspace")).unwrap();
    std::fs::write(
        tmp.path().join("config.json"),
        dead_provider_config().to_string(),
    )
    .unwrap();
    tmp
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemble_missing_config_is_honest_err() {
    let tmp = tempfile::tempdir().unwrap();
    let err = assemble(tmp.path()).await.unwrap_err();
    assert!(err.contains("Configuration not found"), "{err}");
    assert!(err.contains("nemesisbot onboard default"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemble_security_on_reports_pipeline_active() {
    let tmp = temp_home();
    let asm = assemble(tmp.path()).await.expect("装配应成功");
    assert!(
        asm.security_active,
        "security.enabled 缺省 true → 安全 8 层管线必须真实在位"
    );
    assert!(asm.workspace.ends_with("workspace"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemble_security_off_reports_pipeline_absent() {
    let tmp = temp_home();
    // 显式关安全 → 结构断言面翻 false（诚实呈现，绝不谎报）。
    let cfg_path = tmp.path().join("config.json");
    let mut cfg = dead_provider_config();
    cfg["security"] = serde_json::json!({"enabled": false});
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();

    let asm = assemble(tmp.path()).await.expect("装配应成功");
    assert!(
        !asm.security_active,
        "security.enabled=false → security_active 必须为 false"
    );
}

#[cfg(feature = "memory")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemble_memory_flag_gates_executor() {
    // 关（缺省）：无执行器。
    let tmp = temp_home();
    let asm = assemble(tmp.path()).await.expect("装配应成功");
    assert!(asm.memory_executor.is_none(), "memory.enabled 缺省关");

    // 开：执行器在位（gateway ctx.rs 同款构造；无 ONNX 插件自动降级 basic）。
    let tmp2 = temp_home();
    let cfg_path = tmp2.path().join("config.json");
    let mut cfg = dead_provider_config();
    cfg["memory"] = serde_json::json!({"enabled": true});
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();
    let asm2 = assemble(tmp2.path()).await.expect("装配应成功");
    assert!(
        asm2.memory_executor.is_some(),
        "memory.enabled=true → 执行器在位"
    );
}

// ---------------------------------------------------------------------------
// stdio 成帧循环（BUG-20261113-133：入站字节闸 + 非 UTF-8 lossy 收敛——
// BUG-20261029-160 家族在 mcp-serve 方向的漏网站点）
// ---------------------------------------------------------------------------

/// 最小 stdio 测试客户端（acp TestClient 同款）：持久 Lines（防每次新建
/// BufReader 丢缓冲字节），`send_raw` 支持非 UTF-8 原始字节。
struct FrameClient {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
}

impl FrameClient {
    fn new(stream: tokio::io::DuplexStream) -> Self {
        use tokio::io::AsyncBufReadExt as _;
        let (read, write) = tokio::io::split(stream);
        Self {
            write,
            lines: tokio::io::BufReader::new(read).lines(),
        }
    }

    async fn send_raw(&mut self, bytes: &[u8]) {
        use tokio::io::AsyncWriteExt as _;
        self.write.write_all(bytes).await.unwrap();
        self.write.flush().await.unwrap();
    }

    async fn send_frame(&mut self, line: &str) {
        self.send_raw(format!("{line}\n").as_bytes()).await;
    }

    /// 读下一帧（10s 超时防挂死）。
    async fn recv_frame(&mut self) -> Value {
        let line = tokio::time::timeout(std::time::Duration::from_secs(10), self.lines.next_line())
            .await
            .expect("timeout waiting for frame")
            .expect("server side closed unexpectedly")
            .expect("read line failed");
        serde_json::from_str(&line).expect("frame not JSON")
    }
}

/// 起一个带 cap 的成帧循环 server；收尾靠 drop 客户端 → EOF → serve 返回。
async fn start_framed_server(
    cap: usize,
) -> (FrameClient, tokio::task::JoinHandle<Result<(), String>>) {
    let server = Arc::new(McpServer::new("mcp-serve-test", "0.0.0"));
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (sr, sw) = tokio::io::split(server_side);
    let task = tokio::spawn(serve_frames(sr, sw, server, cap));
    (FrameClient::new(client_side), task)
}

/// 非 UTF-8 帧不再杀 server。旧 `BufReader::lines()` 形态：next_line
/// Err("stream did not contain valid UTF-8") → serve_frames 返回 Err →
/// server 退出（②的超时断言当场抓）。新契约：lossy 替换 → -32700 → 存活。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_frames_invalid_utf8_yields_parse_error_and_survives() {
    let (mut c, task) = start_framed_server(MAX_FRAME_BYTES).await;
    c.send_raw(&[0xff, 0xfe, b'g', b'a', b'r', b'b', b'a', b'g', b'e', b'\n'])
        .await;
    let frame = c.recv_frame().await;
    assert_eq!(
        frame["error"]["code"], -32700,
        "非 UTF-8 帧必须 lossy 收敛为 parse error，而非杀进程: {frame}"
    );
    // 服务存活：同一连接上后续请求照常应答。
    c.send_frame(r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#)
        .await;
    let frame = c.recv_frame().await;
    assert_eq!(frame["id"], 7, "坏帧后服务必须存活并继续响应");
    assert!(frame.get("error").is_none(), "合法 ping 不应报错: {frame}");
    drop(c);
    let _ = task.await.unwrap();
}

/// 超限帧回 -32700 并继续服务。可证伪锁的关键：超限帧**不带换行**发送
/// ——LineReader 在无换行累积 >cap 时即刻结算响应；旧裸 `lines()` 形态
/// 要等满一行才处理，recv_frame 超时当场抓（acp
/// `oversized_frame_is_rejected_and_server_survives` 同款契约）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_frames_oversized_frame_rejected_and_survives() {
    // cap=256：300B 无换行累积即超限；后续合法 ping 帧远小于 cap 必须放行。
    let (mut c, task) = start_framed_server(256).await;
    let padded = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"pad":"{}"}}}}"#,
        "x".repeat(200)
    );
    assert!(padded.len() > 256);
    // 不带换行发送——超限闸必须在换行到达前就结算。
    c.send_raw(padded.as_bytes()).await;
    let frame = c.recv_frame().await;
    assert_eq!(
        frame["error"]["code"], -32700,
        "超限帧必须回 parse error（id 未知 → null）: {frame}"
    );
    // 收尾换行终止超限行，随后合法请求正常响应（服务存活）。
    c.send_raw(b"\n").await;
    c.send_frame(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#)
        .await;
    let frame = c.recv_frame().await;
    assert_eq!(frame["id"], 2, "超限帧后服务必须存活并继续响应");
    assert!(frame.get("error").is_none(), "合法 ping 不应报错: {frame}");
    drop(c);
    let _ = task.await.unwrap();
}

/// 通知（无 id 帧）在成帧循环里不产生任何帧（若误回帧，首个到达帧的
/// id 校验当场抓到——server.rs 对通知会构造 id:null 响应，靠消费侧
/// is_notification 过滤，这是该纪律的循环级证据）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_frames_notification_silent_then_request_answered() {
    let (mut c, task) = start_framed_server(MAX_FRAME_BYTES).await;
    c.send_frame(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .await;
    c.send_frame(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#)
        .await;
    let frame = c.recv_frame().await;
    assert_eq!(
        frame["id"], 3,
        "通知不得产生任何响应帧（首帧必须是 ping 的响应）: {frame}"
    );
    drop(c);
    let _ = task.await.unwrap();
}
