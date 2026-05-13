//! NemesisBot - MCP Test Server
//!
//! Minimal MCP protocol server that communicates over stdin/stdout using
//! newline-delimited JSON. Implements: initialize, tools/list, tools/call.
//! Provides two test tools: "echo" and "add".

use std::io::{self, BufRead, Write};

use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "mcp-server", about = "MCP protocol test server")]
struct Args {
    /// Server name reported in initialize response
    #[arg(long, default_value = "test-mcp-server")]
    name: String,
}

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

// ---------------------------------------------------------------------------
// MCP protocol types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    protocol_version: String,
    capabilities: ServerCapabilities,
    server_info: ServerInfo,
}

#[derive(Debug, Serialize)]
struct ServerCapabilities {
    tools: ToolsCapability,
}

#[derive(Debug, Serialize)]
struct ToolsCapability {
    // empty object -- signals tool support
}

#[derive(Debug, Serialize)]
struct ServerInfo {
    name: String,
    version: String,
}

#[derive(Debug, Serialize)]
struct ToolsListResult {
    tools: Vec<ToolInfo>,
}

#[derive(Debug, Serialize)]
struct ToolInfo {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolCallResult {
    content: Vec<ContentBlock>,
    is_error: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

fn echo_tool() -> ToolInfo {
    ToolInfo {
        name: "echo".into(),
        description: "Echoes back the input text".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text to echo back" }
            },
            "required": ["text"]
        }),
    }
}

fn add_tool() -> ToolInfo {
    ToolInfo {
        name: "add".into(),
        description: "Adds two numbers together".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "number", "description": "First number" },
                "b": { "type": "number", "description": "Second number" }
            },
            "required": ["a", "b"]
        }),
    }
}

// ---------------------------------------------------------------------------
// Request handling
// ---------------------------------------------------------------------------

fn handle_request(
    req: &JsonRpcRequest,
    server_name: &str,
) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => {
            let result = InitializeResult {
                protocol_version: "2024-11-05".into(),
                capabilities: ServerCapabilities {
                    tools: ToolsCapability {},
                },
                server_info: ServerInfo {
                    name: server_name.into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
            };
            JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id.clone(),
                result: Some(serde_json::to_value(&result).unwrap()),
                error: None,
            }
        }

        "notifications/initialized" => {
            // Client notification, no response needed (id is typically null).
            JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id.clone(),
                result: Some(Value::Object(serde_json::Map::new())),
                error: None,
            }
        }

        "tools/list" => {
            let result = ToolsListResult {
                tools: vec![echo_tool(), add_tool()],
            };
            JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id.clone(),
                result: Some(serde_json::to_value(&result).unwrap()),
                error: None,
            }
        }

        "tools/call" => {
            let params = req.params.clone().unwrap_or(Value::Null);
            let tool_name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));

            match tool_name {
                "echo" => {
                    let text = arguments
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let result = ToolCallResult {
                        content: vec![ContentBlock::Text {
                            text: text.into(),
                        }],
                        is_error: false,
                    };
                    JsonRpcResponse {
                        jsonrpc: "2.0".into(),
                        id: req.id.clone(),
                        result: Some(serde_json::to_value(&result).unwrap()),
                        error: None,
                    }
                }

                "add" => {
                    let a = arguments
                        .get("a")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let b = arguments
                        .get("b")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let sum = a + b;
                    // Format without trailing .0 for integers
                    let text = if sum.fract() == 0.0 {
                        format!("{}", sum as i64)
                    } else {
                        format!("{sum}")
                    };
                    let result = ToolCallResult {
                        content: vec![ContentBlock::Text { text }],
                        is_error: false,
                    };
                    JsonRpcResponse {
                        jsonrpc: "2.0".into(),
                        id: req.id.clone(),
                        result: Some(serde_json::to_value(&result).unwrap()),
                        error: None,
                    }
                }

                _ => JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: req.id.clone(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!("Unknown tool: {tool_name}"),
                    }),
                },
            }
        }

        "ping" => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: Some(Value::Object(serde_json::Map::new())),
            error: None,
        },

        _ => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {}", req.method),
            }),
        },
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args = Args::parse();

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let reader = stdin.lock();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Error reading stdin: {e}");
                break;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: None,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("Parse error: {e}"),
                    }),
                };
                let out = serde_json::to_string(&resp).unwrap();
                println!("{out}");
                let _ = stdout.flush();
                continue;
            }
        };

        // Skip notifications (no id or id is null) after sending ack for initialized
        let is_notification =
            req.id.is_none() || req.id.as_ref().is_some_and(|v| v.is_null());

        let resp = handle_request(&req, &args.name);

        // For notifications, still respond (some test harnesses expect it)
        if is_notification && req.method != "notifications/initialized" {
            continue;
        }

        let out = serde_json::to_string(&resp).unwrap();
        println!("{out}");
        let _ = stdout.flush();
    }
}
