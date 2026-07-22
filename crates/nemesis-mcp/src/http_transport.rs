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
pub struct HttpTransport {
    /// The MCP server endpoint URL (e.g. `http://localhost:8080/mcp`).
    url: String,
    /// Reusable HTTP client (connection pooling).
    client: reqwest::Client,
    /// Session ID assigned by the server during initialize.
    session_id: Option<String>,
    /// Whether the transport is logically connected.
    connected: bool,
}

impl HttpTransport {
    /// Create a new HTTP transport targeting the given URL.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            client: reqwest::Client::new(),
            session_id: None,
            connected: false,
        }
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn connect(&mut self) -> Result<(), TransportError> {
        // HTTP is stateless — just mark as connected.
        self.connected = true;
        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.connected = false;
        self.session_id = None;
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
