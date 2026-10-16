//! MCP stdio transport.
//!
//! Implements the `Transport` trait for subprocess-based communication using
//! newline-delimited JSON-RPC over stdin/stdout, as defined by the MCP
//! specification.

use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{Duration, timeout};

use crate::transport::{Transport, TransportError, TransportRequest, TransportResponse};

// ---------------------------------------------------------------------------
// StdioTransport
// ---------------------------------------------------------------------------

/// Stdio-based MCP transport.
///
/// Spawns a child process and communicates via its stdin/stdout using
/// newline-delimited JSON-RPC messages.
pub struct StdioTransport {
    /// Command to execute.
    command: String,
    /// Arguments to pass to the command.
    args: Vec<String>,
    /// Environment variables ("KEY=VALUE").
    env: Vec<String>,
    /// The spawned child process.
    child: Option<Child>,
    /// Write handle to child's stdin (protected by mutex for concurrent sends).
    stdin: Option<Mutex<tokio::process::ChildStdin>>,
    /// Read handle to child's stdout (protected by mutex for sequential reads).
    stdout: Option<Mutex<BufReader<tokio::process::ChildStdout>>>,
    /// Whether the transport is currently connected.
    connected: bool,
}

impl StdioTransport {
    /// Create a new stdio transport for the given command.
    ///
    /// The subprocess is not started until `connect()` is called.
    pub fn new(command: impl Into<String>, args: Vec<String>, env: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            env,
            child: None,
            stdin: None,
            stdout: None,
            connected: false,
        }
    }

    /// Create from a `ServerConfig`.
    pub fn from_config(config: &crate::types::ServerConfig) -> Self {
        Self::new(&config.command, config.args.clone(), config.env.clone())
    }
}

/// P0 vault（B3）：解析 "KEY=VALUE" 列表中可含引用的 VALUE
/// （vault:/env:/yaml: 前缀经 nemesis-config 全局解析器现查；字面量原样）。
/// 解析失败的变量跳过（error! 带键名与根因），其余原序返回。
///
/// 2026-10-15 第十三轮：语法规则单一真相源 =
/// `nemesis_config::validate_mcp_env_line`（CLI `mcp add` 写入面与
/// Dashboard server.add·update 同源）——非法行 loud WARN + 跳过（原文不
/// 落日志），不再静默丢弃（此前无 `=` 的行无声消失，用户以为变量已注入）；
/// 值部 trim 后解析（与 headers 侧 `resolved_header_pairs` 对齐，写入面
/// 校验同款 trim-后检查）。
pub(crate) fn resolved_env(env: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(env.len());
    for pair in env {
        if let Err(reason) = nemesis_config::validate_mcp_env_line(pair) {
            tracing::warn!("[mcp] env 行非法，跳过（{reason}；原文不落日志）");
            continue;
        }
        let Some((k, v)) = pair.split_once('=') else {
            continue; // 校验已保证有 '='；防御臂
        };
        match nemesis_config::resolve_secret_field(v.trim(), &format!("mcp.env.{}", k.trim())) {
            Ok(value) => out.push((k.trim().to_string(), value)),
            Err(e) => {
                tracing::error!("[mcp] env {k}: 引用解析失败: {e}（变量未注入）");
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// J3: stdout 行分类（响应 / 通知 / 垃圾行）
// ---------------------------------------------------------------------------

/// `classify_line` 的分类结果。抽成纯函数便于单测——真实的 subprocess
/// 读取循环没法直接构造。
enum LineKind {
    /// 带 `id` 的 JSON-RPC 响应（含错误响应——错误响应也带 id）。
    Response,
    /// progress 通知（`method` 含 "progress"）。
    Progress(String),
    /// 其他通知（log / cancelled / initialized 等）。
    Notification(String),
    /// 非 JSON 行（服务器往 stdout 打的 banner / 日志）。
    Garbage,
}

/// 按行内容分类。判定规则：JSON 且带 `id` = 响应；JSON 无 `id` = 通知
/// （JSON-RPC 通知就是无 id 请求）；其余 = 垃圾行。
fn classify_line(line: &str) -> LineKind {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return LineKind::Garbage;
    };
    if v.get("id").is_some() {
        return LineKind::Response;
    }
    let method = v
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown")
        .to_string();
    if method.contains("progress") {
        LineKind::Progress(method)
    } else {
        LineKind::Notification(method)
    }
}

// ---------------------------------------------------------------------------
// P3 T6: per-agent browser profile injection
// ---------------------------------------------------------------------------

/// P3 T6（浏览器身份）：执行体上下文内的 MCP spawn 注入 per-agent Chromium
/// profile。`NEMESISBOT_BROWSER_PROFILE` 由常驻执行体监督器 spawn 子进程时
/// 设置（nemesisbot `agents/runtime.rs` 的 `base_executor_env`），是「本进程
/// 处于 agent 执行体上下文」的标记：gateway 进程不设它——共享 MCP server
/// 的 spawn 行为零变化；执行体上下文内 spawn 的浏览器类 MCP server
/// （playwright / chrome-devtools 等）由此拿到 per-agent `--user-data-dir`
/// （登录态/cookie 全落 agent home 的 browser-profile/，跨任务跨重建保持、
/// 不出 home——P3 ④ 纪律；Sandboxie 盒路径另有 OpenFilePath 写穿透）。
///
/// server 配置里已显式带 `--user-data-dir` = 用户配置优先，不重复注入。
/// 纯函数便于单测（进程级 env 无法在并行测试里安全改写）。
pub(crate) fn args_with_browser_profile(args: &[String], profile: Option<&str>) -> Vec<String> {
    match profile {
        Some(dir) if !dir.trim().is_empty() && !args.iter().any(|a| a == "--user-data-dir") => {
            let mut v = args.to_vec();
            v.push("--user-data-dir".to_string());
            v.push(dir.to_string());
            v
        }
        _ => args.to_vec(),
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn connect(&mut self) -> Result<(), TransportError> {
        if self.connected {
            return Ok(());
        }

        let mut cmd = Command::new(&self.command);
        cmd.args(args_with_browser_profile(
            &self.args,
            std::env::var("NEMESISBOT_BROWSER_PROFILE").ok().as_deref(),
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        // MCP server 多为 console 程序(node/python 等)；gateway 以托盘/
        // 无控制台运行（release windows 子系统，2026-09-21）时，不压
        // CREATE_NO_WINDOW 会给每个 server 弹一个新控制台窗口。stdio 全走
        // 管道，不受影响。
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        // Inject environment variables.
        // P0 vault（B3，2026-09-22 计划）：env VALUE 支持 vault:/env:/yaml:
        // 引用，spawn 前现查（discover 一次性路径同走本 transport，单一
        // 消费点）。解析失败的变量跳过并 error!——server 侧会以缺凭据
        // 响亮失败，日志带根因。解析逻辑抽纯函数便于单测。
        for (k, value) in resolved_env(&self.env) {
            cmd.env(k, value);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::send_failed(format!("failed to spawn MCP server: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::send_failed("failed to get stdin pipe"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TransportError::send_failed("failed to get stdout pipe"))?;

        // 第十五轮缺陷 18：stderr 管道必须持续排水——此前 `Stdio::piped()`
        // 捕获后无人读：话痨型 server（node/python 的运行日志走 stderr 是
        // 常态）写满 OS 管道缓冲后会阻塞在 stderr 写上，进程整体挂死，
        // 表现为「跑得好好的，一阵之后全部请求神秘超时」。排水任务逐行读
        // stderr 转 debug 日志（默认级别不刷屏，排障时开 debug 可见服务器
        // 侧输出）；EOF / 读错误静默退出（close 杀进程后必然走这）。
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            tracing::debug!(target: "mcp_stdio_stderr", "{}", line.trim_end());
                        }
                    }
                }
            });
        }

        self.child = Some(child);
        self.stdin = Some(Mutex::new(stdin));
        self.stdout = Some(Mutex::new(BufReader::new(stdout)));
        self.connected = true;

        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        if !self.connected {
            return Ok(());
        }
        self.connected = false;

        // Drop stdin to signal EOF.
        self.stdin = None;

        // Kill the child process.
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.child = None;
        self.stdout = None;

        Ok(())
    }

    async fn send(
        &mut self,
        request: &TransportRequest,
        timeout_ms: u64,
    ) -> Result<TransportResponse, TransportError> {
        if !self.connected {
            return Err(TransportError::not_connected());
        }

        // Serialize the request.
        let mut line = serde_json::to_string(request).map_err(|e| {
            TransportError::send_failed(format!("failed to serialize request: {e}"))
        })?;
        line.push('\n');

        // Write to stdin.
        let stdin = self.stdin.as_ref().ok_or(TransportError::not_connected())?;
        {
            let mut writer = stdin.lock().await;
            writer.write_all(line.as_bytes()).await.map_err(|e| {
                TransportError::send_failed(format!("failed to write to stdin: {e}"))
            })?;
            writer
                .flush()
                .await
                .map_err(|e| TransportError::send_failed(format!("failed to flush stdin: {e}")))?;
        }

        // Read response from stdout with timeout.
        let stdout = self
            .stdout
            .as_ref()
            .ok_or(TransportError::not_connected())?;
        let effective_timeout = if timeout_ms == 0 {
            Duration::from_secs(30)
        } else {
            Duration::from_millis(timeout_ms)
        };

        // J3：逐行读直到拿到带 id 的响应行（整体 deadline 不变）。无 id 的
        // JSON-RPC 通知（progress/log 是 MCP 标准行为）不是本次请求的响应，
        // 记 trace 后跳过；非 JSON 行（服务器 banner）同样跳过。旧实现读
        // 一行就当响应解析：服务器在响应前先发通知会让通知行反序列化失败
        // （TransportResponse 要求 id 字段）→ 整个请求报 send_failed。
        let deadline = tokio::time::Instant::now() + effective_timeout;
        let response_line = {
            let mut reader = stdout.lock().await;
            loop {
                let mut buf = String::new();
                let remaining = deadline
                    .checked_duration_since(tokio::time::Instant::now())
                    .unwrap_or_default();
                if remaining.is_zero() {
                    return Err(TransportError::timeout());
                }
                timeout(remaining, reader.read_line(&mut buf))
                    .await
                    .map_err(|_| TransportError::timeout())?
                    .map_err(|e| {
                        TransportError::send_failed(format!("failed to read from stdout: {e}"))
                    })?;

                if buf.is_empty() {
                    return Err(TransportError::send_failed(
                        "connection closed (EOF from MCP server)",
                    ));
                }

                match classify_line(&buf) {
                    LineKind::Response => break buf,
                    LineKind::Progress(method) => {
                        tracing::trace!(
                            method = %method,
                            "[StdioTransport] MCP progress notification: {}",
                            buf.trim()
                        );
                    }
                    LineKind::Notification(method) => {
                        tracing::trace!(
                            method = %method,
                            "[StdioTransport] MCP notification skipped"
                        );
                    }
                    LineKind::Garbage => {
                        tracing::trace!("[StdioTransport] MCP non-JSON stdout line skipped");
                    }
                }
            }
        };

        // Parse the response.
        let response: TransportResponse = serde_json::from_str(response_line.trim())
            .map_err(|e| TransportError::send_failed(format!("failed to parse response: {e}")))?;

        Ok(response)
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn name(&self) -> &str {
        "stdio"
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Best-effort kill the child process on drop.
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

// pub(crate)：http_transport/tests.rs 复用 VAULT_SLOT_LOCK——全局 vault
// 解析器槽位是进程单例，两个测试文件必须共用同一把锁才真的互斥。
#[cfg(test)]
pub(crate) mod tests;
