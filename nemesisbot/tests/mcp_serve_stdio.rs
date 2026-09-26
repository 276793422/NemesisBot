//! P23 `nemesisbot mcp-serve` stdio 端到端集成测试。
//!
//! spawn 真实二进制（`CARGO_BIN_EXE_nemesisbot`）作为 stdio MCP server，
//! 用手写 ndjson JSON-RPC 客户端（[`McpClient`]，std::process::Command +
//! 管道 + 读线程）走完整握手与工具调用：
//!
//! 1. initialize 握手 + tools/list 四工具 + ping。
//! 2. `sessions_list` 只读工具合法 JSON 响应（预置 session_log）。
//! 3. `memory_search` 轻量回落路径（无 enhanced memory 配置）。
//! 4. `run` 工具全链：MockAiServer 脚本模型驱动 read_file 工具调用 → 最终
//!    回复返回 + `workspace/logs/security_logs/audit_chain.jsonl` 落账——
//!    **安全 8 层在 mcp-serve 的 run 路径真实执行的行为证据**（K1 同源
//!    装配；结构断言见 `src/mcp_serve/tests.rs`）。
//! 5. `security.enabled=false` 对照组：run 照常完成但审计链不落——证明
//!    开关被同源消费（不是硬编码常开）。
//!
//! 运行：`cargo test -p nemesisbot --test mcp_serve_stdio`
//! （子进程为 debug 构建 console 程序，stdio 全管道——无弹窗风险。）

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use test_harness::mock_ai::{MockAiReply, MockAiServer};

/// 指向本次构建的 nemesisbot 二进制（cargo 为同包集成测试注入）。
fn nemesisbot_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_nemesisbot"))
}

/// run 工具走完整 agent loop（装配 + 两轮 LLM + 工具执行），debug 构建
/// 下装配耗时秒级——给足超时，宁慢勿 flaky。
const RUN_TIMEOUT: Duration = Duration::from_secs(300);
const FAST_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// 临时 home（NEMESISBOT_HOME 指向；不用 --local——那会往 stdout 打
// "Local mode enabled" 污染协议流）
// ---------------------------------------------------------------------------

struct TestHome {
    tmp: tempfile::TempDir,
}

impl TestHome {
    /// config.json 指向给定 api_base（装配不拨号，死地址即可）。
    ///
    /// `NEMESISBOT_HOME` 的语义是**父基目录**（home = `{env}/.nemesisbot`，
    /// 与 `~/.nemesisbot` 默认同构）——所以布局是 `<base>/.nemesisbot/{config.json, workspace/}`，
    /// env 指向 `<base>`。不用 `--local`——那会往 stdout 打
    /// "Local mode enabled" 污染协议流。
    fn new(api_base: &str) -> Self {
        let t = Self::empty();
        t.write_config(api_base);
        t
    }

    /// 只建目录不写 config（run 全链用：evidence 文件路径要先于 mock
    /// 脚本确定，config 里才有 mock 的 api_base）。
    fn empty() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace")).expect("mkdir workspace");
        Self { tmp }
    }

    /// 写 config.json（agents.defaults.llm → mock 模型，api_base 指向 mock）。
    fn write_config(&self, api_base: &str) {
        let cfg = json!({
            "agents": {"defaults": {"llm": "mock"}},
            "model_list": [{
                "model_name": "mock",
                "model": "mockprov/mock",
                "api_key": "k",
                "api_base": format!("{api_base}/v1"),
            }]
        });
        std::fs::write(self.home().join("config.json"), cfg.to_string()).expect("write config");
    }

    /// NEMESISBOT_HOME 应指向的基目录（home 在其 `.nemesisbot` 子目录）。
    fn path(&self) -> &Path {
        self.tmp.path()
    }

    /// 实际 home 目录（`.nemesisbot`）。
    fn home(&self) -> PathBuf {
        self.tmp.path().join(".nemesisbot")
    }
}

// ---------------------------------------------------------------------------
// 手写 stdio MCP 客户端（ndjson JSON-RPC；子进程级读写）
// ---------------------------------------------------------------------------

struct McpClient {
    child: Child,
    stdin: std::process::ChildStdin,
    rx: Receiver<String>,
    _stderr: Arc<Mutex<String>>,
    next_id: u64,
}

impl McpClient {
    fn spawn(home: &Path) -> Self {
        let mut child = Command::new(nemesisbot_exe())
            .arg("mcp-serve")
            .env("NEMESISBOT_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn nemesisbot mcp-serve");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let stderr = child.stderr.take().expect("child stderr");

        // stdout 行 → channel（响应按 id 匹配取用）。
        let (tx, rx) = channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // stderr 必须排干（debug 构建日志不少，管道写满会把子进程卡死）；
        // 尾部 64KB 留作失败诊断。
        let keep = Arc::new(Mutex::new(String::new()));
        let keep2 = Arc::clone(&keep);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().flatten() {
                let mut buf = keep2.lock().expect("stderr buf lock");
                if buf.len() < 64 * 1024 {
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
        });

        Self {
            child,
            stdin,
            rx,
            _stderr: keep,
            next_id: 0,
        }
    }

    fn stderr_snapshot(&self) -> String {
        self._stderr.lock().expect("stderr buf lock").clone()
    }

    /// 发请求并等匹配 id 的响应帧（跳过垃圾行/无干帧）；超时或子进程退出
    /// 时携带 stderr 尾巴，诊断不盲。
    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.next_id += 1;
        let id = json!(self.next_id);
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{frame}").map_err(|e| format!("write stdin: {e}"))?;
        self.stdin
            .flush()
            .map_err(|e| format!("flush stdin: {e}"))?;

        let deadline = Instant::now() + timeout;
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                return Err(format!(
                    "timeout waiting response for {method}; stderr tail:\n{}",
                    self.stderr_snapshot()
                ));
            }
            match self.rx.recv_timeout(remain) {
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "timeout waiting response for {method}; stderr tail:\n{}",
                        self.stderr_snapshot()
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!(
                        "child closed stdout while waiting for {method}; stderr tail:\n{}",
                        self.stderr_snapshot()
                    ));
                }
                Ok(line) => {
                    let Ok(v) = serde_json::from_str::<Value>(&line) else {
                        continue; // 容忍非 JSON 行（协议上不该出现，防御）
                    };
                    if v.get("id") == Some(&id) {
                        return Ok(v);
                    }
                    // 其他 id 的帧：继续等（理论上无并发请求，防御性跳过）。
                }
            }
        }
    }

    /// 发通知（无 id 帧，按协议不回帧）。
    fn notify(&mut self, method: &str) {
        let frame = json!({"jsonrpc": "2.0", "method": method});
        let _ = writeln!(self.stdin, "{frame}");
        let _ = self.stdin.flush();
    }

    /// 标准 MCP 握手（initialize + initialized 通知），返回 initialize 结果。
    fn initialize(&mut self) -> Value {
        let resp = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "mcp-serve-stdio-test", "version": "0.0.1"},
                }),
                FAST_TIMEOUT,
            )
            .expect("initialize handshake");
        assert!(resp.get("result").is_some(), "initialize 应成功: {resp}");
        self.notify("notifications/initialized");
        resp["result"].clone()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// tools/call 的便捷封装：断言无 JSON-RPC error 后返回 result。
fn call_tool(
    c: &mut McpClient,
    name: &str,
    arguments: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let resp = c.request(
        "tools/call",
        json!({"name": name, "arguments": arguments}),
        timeout,
    )?;
    if let Some(err) = resp.get("error") {
        return Err(format!("tools/call {name} json-rpc error: {err}"));
    }
    Ok(resp["result"].clone())
}

/// 取工具结果首段 text。
fn result_text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or("")
}

// ---------------------------------------------------------------------------
// 1. 握手 + 工具清单 + ping
// ---------------------------------------------------------------------------

#[test]
fn initialize_handshake_tools_list_and_ping() {
    let home = TestHome::new("http://127.0.0.1:1");
    let mut c = McpClient::spawn(home.path());

    let init = c.initialize();
    assert_eq!(init["protocolVersion"], "2025-06-18", "协议版本");
    assert_eq!(init["serverInfo"]["name"], "nemesisbot", "server 名");
    assert!(
        init["serverInfo"]["version"].is_string(),
        "server 版本字符串在位: {init}"
    );

    let pong = c.request("ping", json!({}), FAST_TIMEOUT).expect("ping");
    assert!(pong.get("result").is_some(), "ping 应回空对象结果: {pong}");

    let tools = c
        .request("tools/list", json!({}), FAST_TIMEOUT)
        .expect("tools/list");
    let arr = tools["result"]["tools"]
        .as_array()
        .expect("tools 数组")
        .clone();
    let names: Vec<String> = arr
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default().to_string())
        .collect();
    for expected in ["run", "sessions_list", "memory_search", "board_issue_query"] {
        assert!(
            names.iter().any(|n| n == expected),
            "tools/list 缺 {expected}（实际 {names:?}）"
        );
    }
    for t in &arr {
        assert_eq!(
            t["inputSchema"]["type"], "object",
            "工具 {} schema 顶层应为 object",
            t["name"]
        );
    }
}

// ---------------------------------------------------------------------------
// 2. sessions_list（只读工具合法 JSON 响应）
// ---------------------------------------------------------------------------

#[test]
fn sessions_list_returns_valid_json() {
    let home = TestHome::new("http://127.0.0.1:1");
    // 预置一个会话日志（chat_log jsonl 行形态：role/content/timestamp）+ 标题 meta。
    let logs = home.home().join("workspace/logs/session_logs");
    std::fs::create_dir_all(&logs).expect("mkdir session_logs");
    std::fs::write(
        logs.join("agent_main_session_demo.jsonl"),
        concat!(
            r#"{"role":"user","content":"MCP 演示第一轮","timestamp":"2026-09-25T10:00:00+08:00"}"#,
            "\n",
            r#"{"role":"assistant","content":"收到","timestamp":"2026-09-25T10:00:05+08:00"}"#,
            "\n"
        ),
    )
    .expect("write jsonl");
    std::fs::write(
        logs.join("agent_main_session_demo.meta.json"),
        r#"{"title":"MCP 演示会话"}"#,
    )
    .expect("write meta");

    let mut c = McpClient::spawn(home.path());
    c.initialize();

    let result =
        call_tool(&mut c, "sessions_list", json!({}), FAST_TIMEOUT).expect("sessions_list 调用");
    assert_eq!(result["isError"], false, "只读工具不应报错: {result}");
    let payload: Value =
        serde_json::from_str(result_text(&result)).expect("工具结果应为合法 JSON 文本");
    assert_eq!(payload["total"], 1, "恰好一个预置会话: {payload}");
    let sessions = payload["sessions"].as_array().expect("sessions 数组");
    assert_eq!(sessions[0]["id"], "agent_main_session_demo");
    assert_eq!(sessions[0]["title"], "MCP 演示会话");
    assert_eq!(sessions[0]["messageCount"], 2);
}

// ---------------------------------------------------------------------------
// 3. memory_search（无 enhanced memory 配置 → 轻量文本回落）
// ---------------------------------------------------------------------------

#[test]
fn memory_search_falls_back_to_workspace_files() {
    let home = TestHome::new("http://127.0.0.1:1");
    let mem = home.home().join("workspace/memory");
    std::fs::create_dir_all(&mem).expect("mkdir memory");
    std::fs::write(
        mem.join("MEMORY.md"),
        "# Memory\n\nThe launch codename is ZEPHYR-9.\n完全无关的一行。\n",
    )
    .expect("write MEMORY.md");

    let mut c = McpClient::spawn(home.path());
    c.initialize();

    let result = call_tool(
        &mut c,
        "memory_search",
        json!({"query": "zephyr"}),
        FAST_TIMEOUT,
    )
    .expect("memory_search 调用");
    assert_eq!(
        result["isError"], false,
        "检索失败也不该是工具错误: {result}"
    );
    let text = result_text(&result);
    assert!(
        text.contains("ZEPHYR-9"),
        "大小写不敏感命中 MEMORY.md 行: {text}"
    );
    assert!(text.contains("memory/MEMORY.md"), "带文件定位: {text}");

    // 未命中 = 诚实空结果，不是错误。
    let miss = call_tool(
        &mut c,
        "memory_search",
        json!({"query": "nonexistent-xyz"}),
        FAST_TIMEOUT,
    )
    .expect("memory_search miss");
    assert_eq!(miss["isError"], false);
    assert!(
        result_text(&miss).contains("No matches"),
        "{}",
        result_text(&miss)
    );
}

// ---------------------------------------------------------------------------
// 4. run 工具全链 + 审计链落账（安全 8 层行为证据）
// ---------------------------------------------------------------------------

#[test]
fn run_tool_executes_task_and_writes_audit_chain() {
    // 脚本：第 1 轮模型发起 read_file 工具调用 → 工具结果回灌 → 第 2 轮收尾文本。
    // 用只读工具而非 exec：缺省安全策略（无规则）对 CRITICAL 操作默认拒绝
    // （ABAC Layer 3 拦截即短路，到不了 Layer 8）——read_file 是 LOW 风险
    // 默认放行，能证明管线全层走通并落 Merkle 账。
    let home = TestHome::empty();

    // 被读的文件（工具真执行成功，不靠报错回灌；绝对路径——工具的相对
    // 路径语义锚定进程 cwd，不锚 workspace，别赌）。
    let notes = home.home().join("workspace/notes");
    std::fs::create_dir_all(&notes).expect("mkdir notes");
    let evidence_path = notes.join("mcp_evidence.txt");
    std::fs::write(&evidence_path, "mcp audit evidence body\n").expect("write evidence file");

    let mock = MockAiServer::start(vec![
        MockAiReply::ToolCall {
            name: "read_file".into(),
            arguments: json!({"path": evidence_path.display().to_string()}).to_string(),
        },
        MockAiReply::Text("MCP_RUN_OK final reply".into()),
    ])
    .expect("mock ai server");
    home.write_config(&mock.base_url());

    // 开审计链（config.security.json 位于 <workspace>/config/，与 gateway
    // 同一解析路径）——run 路径的工具调用必须走安全 8 层并落 Merkle 账。
    // default_action=allow：空规则集下 ABAC 缺省是全拒（无规则匹配走
    // default），显式放行让管线走到 Layer 8。
    let ws_cfg = home.home().join("workspace/config");
    std::fs::create_dir_all(&ws_cfg).expect("mkdir ws/config");
    std::fs::write(
        ws_cfg.join("config.security.json"),
        r#"{"audit_chain_enabled": true, "default_action": "allow"}"#,
    )
    .expect("write config.security.json");

    let mut c = McpClient::spawn(home.path());
    c.initialize();

    let result = call_tool(
        &mut c,
        "run",
        json!({"task": format!(
            "Use the read_file tool to read {}, then finish.",
            evidence_path.display()
        )}),
        RUN_TIMEOUT,
    )
    .expect("run 工具调用");
    assert_eq!(result["isError"], false, "run 应成功收尾: {result}");
    let text = result_text(&result);
    assert!(
        text.contains("MCP_RUN_OK"),
        "最终回复应原样送达 MCP 客户端: {text}"
    );

    // LLM 至少两轮（工具轮 + 收尾轮）——真走了完整 agent loop。
    assert!(
        mock.hits() >= 2,
        "agent loop 应发起 ≥2 次 LLM 请求（实际 {}）",
        mock.hits()
    );

    // 安全 8 层行为证据：read_file 调用经 pipeline → Layer 8 审计链落账。
    let chain = home
        .home()
        .join("workspace/logs/security_logs/audit_chain.jsonl");
    let content = std::fs::read_to_string(&chain).unwrap_or_else(|e| {
        panic!(
            "审计链应存在（{}）：{e}\nstderr tail:\n{}",
            chain.display(),
            c.stderr_snapshot()
        )
    });
    assert!(
        content.lines().count() >= 1,
        "审计链至少一条事件: {content}"
    );
    assert!(
        content.contains("read_file"),
        "审计链应记录 read_file 工具调用: {content}"
    );
}

// ---------------------------------------------------------------------------
// 5. security.enabled=false 对照组：审计链不落（开关被同源消费）
// ---------------------------------------------------------------------------

#[test]
fn run_tool_security_disabled_leaves_no_audit_chain() {
    let mock =
        MockAiServer::start(vec![MockAiReply::Text("DISABLED_OK".into())]).expect("mock ai server");
    let home = TestHome::new(&mock.base_url());

    // 显式关安全：装配结构断言（src/mcp_serve/tests.rs）已覆盖 security_active
    // 翻 false；这里验证行为差异——run 照常完成，但审计链文件不存在。
    let cfg_path = home.home().join("config.json");
    let mut cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).expect("read config"))
            .expect("parse config");
    cfg["security"] = json!({"enabled": false});
    std::fs::write(&cfg_path, cfg.to_string()).expect("write config");

    let mut c = McpClient::spawn(home.path());
    c.initialize();

    let result = call_tool(
        &mut c,
        "run",
        json!({"task": "just say DISABLED_OK and finish."}),
        RUN_TIMEOUT,
    )
    .expect("run 工具调用");
    assert_eq!(result["isError"], false, "run 应成功: {result}");
    assert!(
        result_text(&result).contains("DISABLED_OK"),
        "{}",
        result_text(&result)
    );

    let chain = home
        .home()
        .join("workspace/logs/security_logs/audit_chain.jsonl");
    assert!(
        !chain.exists(),
        "security 关闭时审计链不应落盘: {}",
        chain.display()
    );
}
