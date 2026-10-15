//! MCP Streamable HTTP transport.
//!
//! Implements the `Transport` trait for HTTP-based MCP servers using the
//! Streamable HTTP protocol (POST JSON-RPC requests, receive JSON or SSE
//! responses).

use async_trait::async_trait;
use tokio::time::Duration;

use crate::transport::{Transport, TransportError, TransportRequest, TransportResponse};

// ---------------------------------------------------------------------------
// HttpTransport
// ---------------------------------------------------------------------------

/// HTTP-based MCP transport using the Streamable HTTP protocol.
///
/// Each `send()` call issues a POST request to the configured URL.
/// The server may respond with either a direct JSON-RPC response
/// (Content-Type: application/json) or an SSE stream
/// (Content-Type: text/event-stream).
///
/// Session ID handling: if the server returns a `Mcp-Session-Id` header,
/// it is stored and included in subsequent requests.
///
/// Configured headers（收官批 B 段消费化，2026-10-15）: `McpServerConfig.headers`
/// 的 `"KEY: VALUE"` 行在 `connect()` 时解析并现查（值支持 `vault:` / `env:` /
/// `yaml:` 引用，语义同 stdio env——每次现查无缓存）；非法行 loud WARN 跳过。
/// 解析后的值只进 HTTP 请求，Debug 掩码（不进日志）。
pub struct HttpTransport {
    /// The MCP server endpoint URL (e.g. `http://localhost:8080/mcp`).
    url: String,
    /// Reusable HTTP client (connection pooling).
    client: reqwest::Client,
    /// Session ID assigned by the server during initialize.
    session_id: Option<String>,
    /// Whether the transport is logically connected.
    connected: bool,
    /// 配置的头行原文（"KEY: VALUE"；值可含 vault:/env:/yaml: 引用）。
    header_lines: Vec<String>,
    /// connect() 时解析好的头（send() 应用；close() 清空——会话级现查）。
    resolved_headers: Vec<(String, String)>,
}

impl HttpTransport {
    /// Create a new HTTP transport targeting the given URL.
    pub fn new(url: impl Into<String>) -> Self {
        Self::with_headers(url, Vec::new())
    }

    /// Create with configured header lines（"KEY: VALUE"，值可含秘密引用）。
    /// 解析在 `connect()` 现查——同一 transport 重连会拿到轮换后的新值。
    pub fn with_headers(url: impl Into<String>, header_lines: Vec<String>) -> Self {
        Self {
            url: url.into(),
            client: reqwest::Client::new(),
            session_id: None,
            connected: false,
            header_lines,
            resolved_headers: Vec::new(),
        }
    }
}

/// 解析 "KEY: VALUE" 头行 → (name, resolved_value)。
///
/// - 非法行 loud WARN + 跳过：缺冒号 / 头名为空 / 头名含空白或控制字符；
/// - `Mcp-Session-Id` 是协议保留头（server 指派、transport 自管）——配置
///   里写它只有伪造会话身份一种效果，WARN 跳过；
/// - 值经 `nemesis_config::resolve_secret_field` 现查（vault:/env:/yaml:
///   引用 + 字面量原样），解析失败 WARN + 跳过该行（不毒化其余行）；
/// - 同名头（大小写不敏感）**后行胜**：reqwest `.header()` 是追加语义，
///   鉴权头出现两行必坏——先在列表里归一。
pub(crate) fn resolved_header_pairs(lines: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in lines {
        let Some((raw_name, raw_value)) = line.split_once(':') else {
            tracing::warn!("[mcp.http] 头行缺冒号，跳过（原文不落日志）");
            continue;
        };
        let name = raw_name.trim();
        if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
            tracing::warn!("[mcp.http] 头名非法（空 / 含空白或控制字符），跳过（原文不落日志）");
            continue;
        }
        if name.eq_ignore_ascii_case("mcp-session-id") {
            tracing::warn!("[mcp.http] Mcp-Session-Id 是协议保留头（server 指派），配置行跳过");
            continue;
        }
        let field = format!("mcp.headers.{name}");
        match nemesis_config::resolve_secret_field(raw_value.trim(), &field) {
            Ok(value) => {
                if let Some(slot) = out.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
                    slot.1 = value;
                } else {
                    out.push((name.to_string(), value));
                }
            }
            Err(e) => {
                tracing::warn!("[mcp.http] 头 {name}: 值解析失败: {e}（头未注入）");
            }
        }
    }
    out
}

/// 掩码 Debug：头的值永不进 Debug/日志（秘密引用解析后的真值只进 HTTP 请求）。
impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let masked: Vec<(String, String)> = self
            .resolved_headers
            .iter()
            .map(|(n, v)| (n.clone(), format!("<redacted:{} bytes>", v.len())))
            .collect();
        f.debug_struct("HttpTransport")
            .field("url", &self.url)
            .field("session_id", &self.session_id)
            .field("connected", &self.connected)
            .field("header_lines", &self.header_lines.len())
            .field("resolved_headers", &masked)
            .finish()
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn connect(&mut self) -> Result<(), TransportError> {
        // HTTP is stateless — just mark as connected. 头行现查解析（值可含
        // vault:/env: 引用，每次连接取最新值；失败行 WARN 跳过不毒化）。
        self.resolved_headers = resolved_header_pairs(&self.header_lines);
        self.connected = true;
        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.connected = false;
        self.session_id = None;
        // 解析值（可能含凭据真值）随手清空，不挂在长命 transport 上。
        self.resolved_headers.clear();
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

        let effective_timeout = if timeout_ms == 0 {
            30_000u64
        } else {
            timeout_ms
        };

        let mut req = self
            .client
            .post(&self.url)
            .timeout(Duration::from_millis(effective_timeout))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        // 配置头（connect() 已解析现查；同名头已在解析期归一为后行胜，
        // reqwest 的追加语义不会产出重复鉴权头）。
        for (name, value) in &self.resolved_headers {
            req = req.header(name.as_str(), value.as_str());
        }

        // 会话 ID 最后设——server 指派的会话身份不可被配置头覆盖。
        if let Some(ref sid) = self.session_id {
            req = req.header("Mcp-Session-Id", sid);
        }

        let response = req
            .json(request)
            .send()
            .await
            .map_err(|e| TransportError::send_failed(format!("HTTP request failed: {}", e)))?;

        // Store session ID from response headers.
        if let Some(sid) = response.headers().get("mcp-session-id")
            && let Ok(s) = sid.to_str()
        {
            self.session_id = Some(s.to_string());
        }

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        // 202 Accepted — typical for notifications.
        if status.as_u16() == 202 {
            let id = request.id.clone().unwrap_or(serde_json::Value::Null);
            return Ok(TransportResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(serde_json::json!({})),
                error: None,
            });
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(TransportError::send_failed(format!(
                "HTTP {} from MCP server: {}",
                status,
                body.trim()
            )));
        }

        if content_type.contains("text/event-stream") {
            parse_sse_response(response).await
        } else {
            let body = response.text().await.map_err(|e| {
                TransportError::send_failed(format!("Failed to read response: {}", e))
            })?;
            let trimmed = body.trim();
            if trimmed.is_empty() {
                let id = request.id.clone().unwrap_or(serde_json::Value::Null);
                return Ok(TransportResponse {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result: Some(serde_json::json!({})),
                    error: None,
                });
            }
            serde_json::from_str(trimmed).map_err(|e| {
                TransportError::send_failed(format!("Failed to parse JSON response: {}", e))
            })
        }
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn name(&self) -> &str {
        "http"
    }
}

// ---------------------------------------------------------------------------
// SSE parsing
// ---------------------------------------------------------------------------

/// Read the first SSE event from an HTTP response and extract the JSON-RPC
/// data payload.
async fn parse_sse_response(
    mut response: reqwest::Response,
) -> Result<TransportResponse, TransportError> {
    let mut buffer = String::new();

    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|e| TransportError::send_failed(format!("SSE read error: {}", e)))?;

        match chunk {
            Some(bytes) => {
                buffer.push_str(&String::from_utf8_lossy(&bytes));

                // Complete event = data block followed by blank line.
                if let Some(idx) = buffer.find("\n\n") {
                    let event_text = &buffer[..idx];
                    return extract_sse_data(event_text);
                }
            }
            None => {
                let trimmed = buffer.trim();
                if !trimmed.is_empty() {
                    return extract_sse_data(trimmed);
                }
                return Err(TransportError::send_failed("SSE stream ended without data"));
            }
        }
    }
}

/// Extract the JSON-RPC response from an SSE event text.
///
/// Handles both single-line and multi-line `data:` fields (concatenated with
/// `\n`). Ignores `event:`, `id:`, `retry:`, and comment lines.
fn extract_sse_data(event_text: &str) -> Result<TransportResponse, TransportError> {
    let mut data_lines: Vec<&str> = Vec::new();

    for line in event_text.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if !data.is_empty() {
                data_lines.push(data);
            }
        }
        // Silently ignore event:/id:/retry:/comment lines.
    }

    if data_lines.is_empty() {
        return Err(TransportError::send_failed(
            "SSE event contains no data field",
        ));
    }

    let json_str = data_lines.join("\n");
    serde_json::from_str(&json_str)
        .map_err(|e| TransportError::send_failed(format!("Failed to parse SSE data: {}", e)))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
