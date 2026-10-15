//! MCP client.
//!
//! Connects to an external MCP server process via a transport layer, using
//! newline-delimited JSON-RPC framing as defined by the MCP specification.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use crate::transport::{self, Transport, TransportError, TransportRequest};
use crate::types::*;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error type for MCP client operations.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Server returned error: {0}")]
    Server(#[from] JSONRPCError),

    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),

    #[error("Client is not connected")]
    NotConnected,

    #[error("Client has been closed")]
    Closed,

    #[error("Client is not initialized")]
    NotInitialized,

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("MCP list pagination aborted: {0}")]
    PaginationAborted(String),
}

/// Result alias for client operations.
pub type ClientResult<T> = Result<T, ClientError>;

/// J3 (devtool-upgrade 阶段 4)：单次 list 允许的最大翻页数——防御失控
/// 服务器（cursor 永不终止）。正常分页远用不满 32 页。
const MAX_LIST_PAGES: usize = 32;

// ---------------------------------------------------------------------------
// Client trait
// ---------------------------------------------------------------------------

/// Trait defining the MCP client interface.
///
/// This mirrors the Go `Client` interface, allowing different implementations
/// for production (real transport) and testing (mock transport).
#[async_trait]
pub trait Client: Send + Sync {
    /// Perform the MCP initialization handshake.
    async fn initialize(&mut self) -> ClientResult<InitializeResult>;

    /// List available tools on the server.
    async fn list_tools(&mut self) -> ClientResult<Vec<McpTool>>;

    /// Invoke a tool on the server.
    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> ClientResult<ToolCallResult>;

    /// List available resources on the server.
    async fn list_resources(&mut self) -> ClientResult<Vec<Resource>>;

    /// Read a resource from the server.
    async fn read_resource(&mut self, uri: &str) -> ClientResult<ResourceContent>;

    /// List available prompts on the server.
    async fn list_prompts(&mut self) -> ClientResult<Vec<Prompt>>;

    /// Get a populated prompt from the server.
    async fn get_prompt(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> ClientResult<PromptResult>;

    /// Close the connection.
    async fn close(&mut self) -> ClientResult<()>;

    /// Return the server info obtained during initialization.
    fn server_info(&self) -> Option<&ServerInfo>;

    /// Return `true` if the client is connected and initialized.
    fn is_connected(&self) -> bool;
}

// ---------------------------------------------------------------------------
// McpClient
// ---------------------------------------------------------------------------

/// MCP client that communicates with a server via a pluggable transport.
///
/// The protocol uses newline-delimited JSON-RPC (each message is a single
/// line terminated by `\n`). Request/response correlation is done via
/// monotonically increasing integer IDs.
pub struct McpClient {
    /// The underlying transport (stdio, mock, etc.).
    transport: Box<dyn Transport>,
    /// Monotonic request id counter.
    next_id: AtomicU64,
    /// Whether the client has been closed.
    closed: bool,
    /// Whether initialization handshake has completed.
    initialized: bool,
    /// Server info obtained from the initialize handshake.
    server_info: Option<ServerInfo>,
    /// Capabilities obtained from the initialize handshake.
    capabilities: Option<ServerCapabilities>,
    /// Protocol version negotiated during initialization.
    protocol_version: Option<String>,
}

impl McpClient {
    /// Create a new client with the given transport.
    ///
    /// The client is not connected until `initialize()` is called.
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self {
            transport,
            next_id: AtomicU64::new(1),
            closed: false,
            initialized: false,
            server_info: None,
            capabilities: None,
            protocol_version: None,
        }
    }

    /// Create a new client from a server configuration using stdio transport.
    ///
    /// Convenience factory that builds a `StdioTransport` from the config and
    /// wraps it in a client.
    pub fn from_config(config: &ServerConfig) -> ClientResult<Self> {
        if config.command.is_empty() {
            return Err(ClientError::InvalidConfig(
                "server command cannot be empty".into(),
            ));
        }
        let stdio = crate::stdio_transport::StdioTransport::from_config(config);
        Ok(Self::new(Box::new(stdio)))
    }

    /// Create a client from a server config, dispatching on `transport_type`.
    ///
    /// config→transport 分派的单一真相源（McpManager::discover_tools 与 CLI
    /// `mcp test/tools/resources/prompts` 共用）：
    /// - `"http"` / `"streamable_http"` / `"streamable-http"`（大小写敏感）→
    ///   [`HttpTransport`](crate::http_transport::HttpTransport)（headers
    ///   "KEY: VALUE" 行消费化，值支持 vault:/env:/yaml: 引用现查）；
    /// - 手写条目兼容（2026-10-15 第六轮复查修复）：`transport_type` 缺省
    ///   且无 `command` 且有 `url` → 按 HTTP 意图处理——stdio 无 url 语义，
    ///   此前这类条目掉进 stdio 臂空 command 必败，而 CLI 诊断面
    ///   （`server_entry_is_http`）按 HTTP 报告，诊断与行为分裂；
    /// - 其他 → `StdioTransport`，且 `command` 空而 `url` 非空时以 `url`
    ///   充当命令（2026-10-15 第八轮复查修复）：Dashboard MCP 管理页的
    ///   stdio 约定是命令存 `url` 字段（前端 payload 不发 `command`，
    ///   web `server_add` 也硬编码 `command` 为空）——运行期只读
    ///   `command`，无此回落则 Dashboard 新建的 stdio 条目 spawn 空
    ///   命令必败。存量盘上条目一并痊愈。
    ///
    /// 未知 `transport_type`（非空且不在 stdio/http 已知集，如 Dashboard
    /// 曾可写的 `"sse"`）loud 拒绝（2026-10-15 第九轮复查修复）：静默掉
    /// stdio 臂会把 url 当程序名 spawn，报出与实际问题无关的错误；与
    /// protocol/stream 归一「未知值 loud 拒绝」的项目惯例对齐。空串不拒
    /// （手写条目兼容形态）。
    pub fn from_server_config(config: &ServerConfig) -> ClientResult<Self> {
        let is_http = matches!(
            config.transport_type.as_str(),
            "http" | "streamable_http" | "streamable-http"
        ) || (config.transport_type.is_empty()
            && config.command.is_empty()
            && !config.url.is_empty());
        if !is_http && !config.transport_type.is_empty() && config.transport_type != "stdio" {
            return Err(ClientError::InvalidConfig(format!(
                "MCP server '{}' has unsupported transport_type '{}' \
                 (supported: stdio, http, streamable_http, streamable-http)",
                config.name, config.transport_type
            )));
        }
        let transport: Box<dyn Transport> = if is_http {
            if config.url.is_empty() {
                return Err(ClientError::InvalidConfig(format!(
                    "MCP server '{}' uses http transport but has no url",
                    config.name
                )));
            }
            Box::new(crate::http_transport::HttpTransport::with_headers(
                config.url.clone(),
                config.headers.clone(),
            ))
        } else {
            let mut cfg = config.clone();
            if cfg.command.is_empty() && !cfg.url.is_empty() {
                cfg.command = cfg.url.clone();
            }
            Box::new(crate::stdio_transport::StdioTransport::from_config(&cfg))
        };
        Ok(Self::new(transport))
    }

    // -- Connection lifecycle ------------------------------------------------

    /// Connect the transport and perform the MCP initialization handshake.
    async fn do_initialize(&mut self) -> ClientResult<InitializeResult> {
        if self.closed {
            return Err(ClientError::Closed);
        }
        if self.initialized {
            return Err(ClientError::InvalidConfig(
                "client already initialized".into(),
            ));
        }

        // Connect the transport (starts subprocess for stdio).
        self.transport.connect().await?;

        // Build initialize request.
        let init_params = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {},
                "resources": {},
                "prompts": {}
            },
            "clientInfo": {
                "name": "nemesis-mcp-client",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let resp = self.send_request("initialize", Some(init_params)).await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        // Parse initialize result.
        let result: InitializeResult = resp
            .result
            .map(serde_json::from_value)
            .transpose()?
            .ok_or_else(|| ClientError::InvalidConfig("empty initialize result".into()))?;

        // Update client state.
        self.protocol_version = Some(result.protocol_version.clone());
        self.server_info = Some(result.server_info.clone());
        self.capabilities = Some(result.capabilities.clone());
        self.initialized = true;

        // Send initialized notification (no response expected).
        self.send_notification("notifications/initialized", None)
            .await?;

        Ok(result)
    }

    // -- Low-level transport -------------------------------------------------

    /// Allocate the next request id.
    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Build a `TransportRequest` without sending it (for testing).
    pub fn build_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> TransportRequest {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::Number(id.into())),
            method: method.to_string(),
            params,
        }
    }

    /// Send a JSON-RPC request via the transport and return the response.
    async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> ClientResult<JSONRPCResponse> {
        if self.closed {
            return Err(ClientError::Closed);
        }

        let id = self.alloc_id();
        let request = TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(serde_json::Value::Number(id.into())),
            method: method.to_string(),
            params,
        };

        let transport_resp = self.transport.send(&request, 30_000).await?;
        let mcp_resp = transport::from_transport_response(&transport_resp);

        Ok(mcp_resp)
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> ClientResult<()> {
        if self.closed {
            return Err(ClientError::Closed);
        }

        let notification = TransportRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None, // notifications have no id
            method: method.to_string(),
            params,
        };

        // Send with a short timeout; we don't expect a response.
        // Some transports may return EOF or a timeout, which is fine for notifications.
        let result = self.transport.send(&notification, 1_000).await;
        // Ignore errors for notifications — they are fire-and-forget.
        let _ = result;

        Ok(())
    }

    /// Parse a JSON-RPC response from a raw string (for testing).
    pub fn parse_response(raw: &str) -> ClientResult<JSONRPCResponse> {
        Ok(serde_json::from_str(raw)?)
    }

    /// J3 (devtool-upgrade 阶段 4)：分页列表通用内核。
    ///
    /// 逐页发送 `{method, {"cursor": cur}}`，拼贴 `items_key` 字段直到响应
    /// 缺 `nextCursor`。旧实现只发一页（params=None）且只读首页——实现
    /// Cursor 分页的服务器（MCP 标准形态）工具表会静默缺页。
    ///
    /// 防失控双保险：已见 cursor 集合去重（同一 cursor 第二次出现=环）+
    /// 页数上限 [`MAX_LIST_PAGES`]。触发即整体报错（`PaginationAborted`）：
    /// 返回部分清单会让上层把「缺工具」误当「服务器就这些工具」，比失败
    /// 更难排查。
    async fn list_page<T: serde::de::DeserializeOwned>(
        &mut self,
        method: &str,
        items_key: &str,
    ) -> ClientResult<Vec<T>> {
        let mut items: Vec<T> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        loop {
            let params = cursor.as_ref().map(|c| serde_json::json!({ "cursor": c }));
            let resp = self.send_request(method, params).await?;
            if let Some(err) = &resp.error {
                return Err(ClientError::Server(err.clone()));
            }
            let result = resp.result.unwrap_or(serde_json::Value::Null);
            if let Some(batch) = result.get(items_key) {
                items.append(&mut serde_json::from_value(batch.clone()).unwrap_or_default());
            }
            let next = result
                .get("nextCursor")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            match next {
                Some(c) => {
                    if !visited.insert(c.clone()) {
                        return Err(ClientError::PaginationAborted(format!(
                            "cursor '{c}' repeated while paging {method} (server pagination loop)"
                        )));
                    }
                    if visited.len() > MAX_LIST_PAGES {
                        return Err(ClientError::PaginationAborted(format!(
                            "{method} did not terminate within {MAX_LIST_PAGES} pages"
                        )));
                    }
                    cursor = Some(c);
                }
                None => return Ok(items),
            }
        }
    }
}

#[async_trait]
impl Client for McpClient {
    async fn initialize(&mut self) -> ClientResult<InitializeResult> {
        self.do_initialize().await
    }

    async fn list_tools(&mut self) -> ClientResult<Vec<McpTool>> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }
        self.list_page("tools/list", "tools").await
    }

    async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> ClientResult<ToolCallResult> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }

        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });

        let resp = self.send_request("tools/call", Some(params)).await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        let result: ToolCallResult = resp
            .result
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();

        Ok(result)
    }

    async fn list_resources(&mut self) -> ClientResult<Vec<Resource>> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }
        self.list_page("resources/list", "resources").await
    }

    async fn read_resource(&mut self, uri: &str) -> ClientResult<ResourceContent> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }

        let params = serde_json::json!({ "uri": uri });

        let resp = self.send_request("resources/read", Some(params)).await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        let content: ResourceContent = resp
            .result
            .and_then(|r| r.get("contents").and_then(|c| c.get(0)).cloned())
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();

        Ok(content)
    }

    async fn list_prompts(&mut self) -> ClientResult<Vec<Prompt>> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }
        self.list_page("prompts/list", "prompts").await
    }

    async fn get_prompt(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> ClientResult<PromptResult> {
        if !self.initialized {
            return Err(ClientError::NotInitialized);
        }

        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });

        let resp = self.send_request("prompts/get", Some(params)).await?;

        if let Some(err) = &resp.error {
            return Err(ClientError::Server(err.clone()));
        }

        let result: PromptResult = resp
            .result
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();

        Ok(result)
    }

    async fn close(&mut self) -> ClientResult<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.initialized = false;

        self.transport.close().await?;

        Ok(())
    }

    fn server_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }

    fn is_connected(&self) -> bool {
        !self.closed && self.initialized && self.transport.is_connected()
    }
}

impl Default for McpClient {
    fn default() -> Self {
        Self::new(Box::new(crate::transport::MockTransport::new()))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
