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
    /// G2：浏览器 profile 显式覆盖（gateway agent loop 装配经 McpManager
    /// 注入；None = 连接时回落进程 env `NEMESISBOT_BROWSER_PROFILE`——执行体
    /// 子进程上下文标记）。
    browser_profile: Option<String>,
    /// G2：注入 opt-in 闸（server 配置 `browser_profile`）。false = 永不
    /// 注入（普通 MCP 零变化）；裸 `new()` 构造恒 false（一次性探测不塞
    /// 身份）。
    browser_profile_opt_in: bool,
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
            browser_profile: None,
            browser_profile_opt_in: false,
        }
    }

    /// Create from a `ServerConfig`（profile 走进程 env 回落——一次性探测
    /// 与执行体子进程上下文形态）。
    pub fn from_config(config: &crate::types::ServerConfig) -> Self {
        Self::from_config_with_profile(config, None)
    }

    /// G2：从配置构造 + 显式 per-agent profile 覆盖（gateway agent loop
    /// 装配路径——共享 gateway 进程 env 无法 per-agent）。opt-in 闸仍来自
    /// 配置（`browser_profile: false` 的 server 即使给了显式目录也不注入）。
    pub fn from_config_with_profile(
        config: &crate::types::ServerConfig,
        profile: Option<String>,
    ) -> Self {
        let mut t = Self::new(&config.command, config.args.clone(), config.env.clone());
        t.browser_profile_opt_in = config.browser_profile;
        t.browser_profile = profile;
        t
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
    /// BUG-20261025-56：server→client 请求（`id` + `method` 并存——
    /// JSON-RPC 响应永不携带 `method`，sampling/roots/elicitation 等形态）。
    /// 客户端未宣告这些能力，无法应答；只能跳过（吞成响应会让 id 撞车的
    /// server 请求变成我方工具调用的「空成功」）。
    ServerRequest(String),
    /// 非 JSON 行（服务器往 stdout 打的 banner / 日志）。
    Garbage,
}

/// 按行内容分类。判定规则：JSON 且带 `id` 无 `method` = 响应；JSON 带
/// `method` 无 `id` = 通知（JSON-RPC 通知就是无 id 请求）；`id` + `method`
/// 并存 = server→client 请求（BUG-20261025-56，此前误归响应）；其余 =
/// 垃圾行。
fn classify_line(line: &str) -> LineKind {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return LineKind::Garbage;
    };
    if v.get("id").is_some() && v.get("method").is_some() {
        let method = v
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown")
            .to_string();
        return LineKind::ServerRequest(method);
    }
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
// P3 T6 + G2: per-agent browser profile injection
// ---------------------------------------------------------------------------

/// 浏览器 profile 注入（P3 T6 引入，G2 2026-10-18 批四修正）。
///
/// 两条生产路径，同一 helper 消费：
/// - **gateway agent loop 路径**（G2）：selfbuilt agent 的常驻 loop 携带
///   agent home 的 browser-profile/ 目录，经 `McpManager` 显式注入
///   transport——共享 gateway 进程 env 无法 per-agent，走显式覆盖。
/// - **执行体子进程路径**（P3 T6 语义保留）：执行体监督器 spawn 子进程时
///   设 `NEMESISBOT_BROWSER_PROFILE`（nemesisbot `agents/runtime.rs` 的
///   `base_executor_env`），是「本进程处于 agent 执行体上下文」的标记；
///   transport 连接时 env 回落读取。
///
/// **opt-in 闸（G2）**：注入只对配置显式 `browser_profile: true` 的
/// server 生效——不按服务名猜，普通 MCP 默认零变化（共享 server 不被塞
/// 身份）。server args 已显式带 `--user-data-dir`（空格或 `=` 形态）=
/// 用户配置优先，不重复注入。纯函数便于单测（进程级 env 无法在并行测试
/// 里安全改写）。
pub(crate) fn args_with_browser_profile(
    args: &[String],
    profile: Option<&str>,
    opt_in: bool,
) -> Vec<String> {
    let Some(dir) = profile.filter(|d| !d.trim().is_empty()).filter(|_| opt_in) else {
        return args.to_vec();
    };
    if args
        .iter()
        .any(|a| a == "--user-data-dir" || a.starts_with("--user-data-dir="))
    {
        return args.to_vec();
    }
    let mut v = args.to_vec();
    v.push("--user-data-dir".to_string());
    v.push(dir.to_string());
    v
}

#[async_trait]
impl Transport for StdioTransport {
    async fn connect(&mut self) -> Result<(), TransportError> {
        if self.connected {
            return Ok(());
        }

        // G2：profile 解析——显式覆盖（agent loop 装配注入）优先，回落进程
        // env 标记（执行体子进程上下文）；opt-in 闸不满足时恒 None（普通
        // MCP 零变化）。
        let profile = if self.browser_profile_opt_in {
            self.browser_profile
                .clone()
                .or_else(|| std::env::var("NEMESISBOT_BROWSER_PROFILE").ok())
        } else {
            None
        };
        let mut cmd = Command::new(&self.command);
        cmd.args(args_with_browser_profile(
            &self.args,
            profile.as_deref(),
            self.browser_profile_opt_in,
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

        // 通知（id=None）按 JSON-RPC 语义无响应可等——写入即返回，不进读
        // 循环空转满 deadline（BUG-20261025-87：notifications/initialized
        // 旧实现每次连接固定烧满 timeout（client 侧 1s）才 Err(timeout)，
        // 错误虽被 send_notification 吞掉，纯延迟照常落地——启动全量挂载
        // 逐 server 串行放大成 N×1s，mcp_discover 每次探测同付 1s）。
        // 返回合成占位响应：id=None 的唯一生产调用方是
        // client::send_notification（fire-and-forget 丢弃结果），占位值
        // 无其他消费者。
        if request.id.is_none() {
            return Ok(TransportResponse {
                jsonrpc: "2.0".to_string(),
                id: serde_json::Value::Null,
                result: Some(serde_json::json!({})),
                error: None,
            });
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
                    LineKind::Response => {
                        // R39：id 匹配闸。超时/重试后，上一请求的迟到响应仍挂
                        // 在 stdout 缓冲里——无闸会把旧响应错配给新请求（工具
                        // 结果张冠李戴）。id 不符 = 陈旧响应，trace 后跳过继续
                        // 读；id=None（通知形态，无响应可匹配）与解析失败保持
                        // 旧行为（首个响应行采纳 / 既有错误路径诚实报错）。
                        match serde_json::from_str::<TransportResponse>(buf.trim()) {
                            Ok(resp) if Some(&resp.id) == request.id.as_ref() => break buf,
                            Ok(resp) => {
                                tracing::trace!(
                                    expected = ?request.id,
                                    got = ?resp.id,
                                    "[StdioTransport] stale response skipped (id mismatch)"
                                );
                            }
                            Err(_) => break buf,
                        }
                    }
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
                    LineKind::ServerRequest(method) => {
                        // BUG-20261025-56：server→client 请求（id+method 并存）
                        // 不是本次请求的响应——旧形态归 Response，id 撞车时被
                        // 当响应吞掉（result=None 空成功）。客户端未宣告
                        // sampling/roots 能力，无法应答，跳过（server 侧自行
                        // 超时）。
                        tracing::trace!(
                            method = %method,
                            "[StdioTransport] server-to-client request skipped (capability not implemented)"
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
