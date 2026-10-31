//! MCP tool adapter.
//!
//! Bridges MCP tools to the NemesisBot tool system. Each MCP tool exposed by
//! a remote server is wrapped in an `McpAdapter` that translates between the
//! NemesisBot `ToolDefinition` format and MCP's JSON-RPC tool invocation
//! protocol.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::client::{Client, ClientError};
use crate::types::*;

// ---------------------------------------------------------------------------
// NemesisBot tool types (local to avoid circular dependency)
// ---------------------------------------------------------------------------
// The adapter defines its own lightweight tool interface rather than depending
// on the full nemesis-types crate. Higher-level code can bridge these types
// to the agent system.

/// A tool definition in NemesisBot format.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name (prefixed with server name to avoid collisions).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema describing the expected input parameters.
    pub parameters: serde_json::Value,
    /// Server 自述行为提示（B3，从 McpTool.annotations 透传；老 server 不带
    /// = None）。消费点：连接器独占镜像基线定档（nemesis-connectors registry）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<super::types::McpToolAnnotations>,
}

/// The result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The text content of the result.
    pub content: String,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

impl ToolResult {
    /// Create a successful result.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    /// Create an error result.
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Trait for a tool that can be executed by the NemesisBot agent.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Return the tool definition.
    fn definition(&self) -> &ToolDefinition;

    /// Execute the tool with the given arguments.
    async fn execute(&self, args: serde_json::Value) -> ToolResult;
}

// ---------------------------------------------------------------------------
// McpAdapter
// ---------------------------------------------------------------------------

/// Adapter that wraps an MCP tool as a NemesisBot `Tool`.
///
/// The tool name is prefixed with the MCP server name (sanitized) to avoid
/// naming collisions when multiple MCP servers are connected.
///
/// Format: `mcp_<server_name>_<tool_name>`
pub struct McpAdapter {
    /// The MCP client used to invoke the tool (Arc<Mutex> for interior mutability).
    client: std::sync::Arc<tokio::sync::Mutex<Box<dyn Client>>>,
    /// The MCP tool definition.
    mcp_tool: McpTool,
    /// The cached NemesisBot tool definition.
    tool_def: ToolDefinition,
    /// Default execution timeout.
    timeout: Duration,
}

impl McpAdapter {
    /// Create a new adapter for the given MCP tool.
    ///
    /// The `client` must already be initialized (server info available).
    pub fn new(client: Box<dyn Client>, mcp_tool: McpTool) -> Self {
        let server_name = client
            .server_info()
            .map(|info| sanitize_name(&info.name))
            .unwrap_or_else(|| "unknown".to_string());

        let tool_name = sanitize_name(&mcp_tool.name);
        let prefixed_name = format!("mcp_{server_name}_{tool_name}");

        let description = match &mcp_tool.description {
            Some(desc) => format!("[MCP:{server_name}] {desc}"),
            None => format!("[MCP:{server_name}] MCP tool: {tool_name}"),
        };

        let parameters = sanitize_schema(mcp_tool.input_schema.clone());

        let tool_def = ToolDefinition {
            name: prefixed_name,
            description,
            parameters,
            annotations: mcp_tool.annotations.clone(),
        };

        Self {
            client: std::sync::Arc::new(tokio::sync::Mutex::new(client)),
            mcp_tool,
            tool_def,
            timeout: Duration::from_secs(30),
        }
    }

    /// Set the execution timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Return a reference to the underlying MCP tool definition.
    pub fn mcp_tool(&self) -> &McpTool {
        &self.mcp_tool
    }
}

#[async_trait]
impl Tool for McpAdapter {
    fn definition(&self) -> &ToolDefinition {
        &self.tool_def
    }

    async fn execute(&self, args: serde_json::Value) -> ToolResult {
        let args_map: HashMap<String, serde_json::Value> = if args.is_object() {
            serde_json::from_value(args).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let args_value = serde_json::to_value(&args_map).unwrap_or(serde_json::json!({}));

        // Execute with timeout.
        let future = async {
            let mut guard = self.client.lock().await;
            guard.call_tool(&self.mcp_tool.name, args_value).await
        };

        let result = match timeout(self.timeout, future).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                return ToolResult::err(format!("MCP tool '{}' error: {}", self.mcp_tool.name, e));
            }
            Err(_) => {
                return ToolResult::err(format!(
                    "MCP tool '{}' timed out after {:?}",
                    self.mcp_tool.name, self.timeout
                ));
            }
        };

        // Check if tool returned an error.
        if result.is_error {
            let err_msg: String = result
                .content
                .iter()
                .filter(|c| c.content_type == "text")
                .filter_map(|c| c.text.as_deref())
                .collect::<Vec<&str>>()
                .join("; ");

            return ToolResult::err(format!(
                "MCP tool '{}' returned error: {}",
                self.mcp_tool.name,
                if err_msg.is_empty() {
                    "unknown error"
                } else {
                    &err_msg
                }
            ));
        }

        // Extract text content from result.
        let text_parts: Vec<String> = result
            .content
            .iter()
            .map(|c| match c.content_type.as_str() {
                "text" => c.text.clone().unwrap_or_default(),
                "image" => {
                    let data = c.text.as_deref().unwrap_or("");
                    format!("[Image: {data}]")
                }
                "resource" => {
                    let data = c.text.as_deref().unwrap_or("");
                    format!("[Resource: {data}]")
                }
                _ => c.text.clone().unwrap_or_default(),
            })
            .collect();

        ToolResult::ok(text_parts.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Factory: create adapters from a connected client
// ---------------------------------------------------------------------------

/// Create NemesisBot tool adapters for all tools available on an MCP server.
///
/// The client must already be initialized. On success, returns a vector of
/// boxed `Tool` trait objects, one for each MCP tool on the server.
pub async fn create_tools_from_client(
    client: Box<dyn Client>,
) -> Result<Vec<Box<dyn Tool>>, ClientError> {
    // We need a mutable reference to call list_tools, but we also need to
    // share the client across all adapters. We'll use a shared wrapper.
    let shared = std::sync::Arc::new(tokio::sync::Mutex::new(client));

    // List tools from the server.
    let tools = {
        let mut guard = shared.lock().await;
        guard.list_tools().await?
    };

    let mut adapters: Vec<Box<dyn Tool>> = Vec::new();

    for mcp_tool in tools {
        // For each tool, create a thin wrapper that clones the Arc.
        let adapter = ArcClientAdapter {
            client: shared.clone(),
            mcp_tool: mcp_tool.clone(),
            tool_def: {
                let guard = shared.lock().await;
                let server_name = guard
                    .server_info()
                    .map(|info| sanitize_name(&info.name))
                    .unwrap_or_else(|| "unknown".to_string());

                let tool_name = sanitize_name(&mcp_tool.name);
                let prefixed_name = format!("mcp_{server_name}_{tool_name}");

                let description = match &mcp_tool.description {
                    Some(desc) => format!("[MCP:{server_name}] {desc}"),
                    None => format!("[MCP:{server_name}] MCP tool: {tool_name}"),
                };

                let parameters = sanitize_schema(mcp_tool.input_schema.clone());

                ToolDefinition {
                    name: prefixed_name,
                    description,
                    parameters,
                    annotations: mcp_tool.annotations.clone(),
                }
            },
            timeout: Duration::from_secs(30),
        };
        adapters.push(Box::new(adapter));
    }

    Ok(adapters)
}

/// Create tool adapters using an explicit server name (instead of server_info).
///
/// Use this when the config name differs from the server's self-reported name.
pub async fn create_tools_from_client_named(
    client: Box<dyn Client>,
    server_name: &str,
    timeout_secs: u64,
) -> Result<Vec<Box<dyn Tool>>, ClientError> {
    let shared = std::sync::Arc::new(tokio::sync::Mutex::new(client));
    let srv = sanitize_name(server_name);

    let tools = {
        let mut guard = shared.lock().await;
        guard.list_tools().await?
    };

    let mut adapters: Vec<Box<dyn Tool>> = Vec::new();

    for mcp_tool in tools {
        let tool_name = sanitize_name(&mcp_tool.name);
        let prefixed_name = format!("mcp_{srv}_{tool_name}");

        let description = match &mcp_tool.description {
            Some(desc) => format!("[MCP:{server_name}] {desc}"),
            None => format!("[MCP:{server_name}] MCP tool: {tool_name}"),
        };

        let parameters = sanitize_schema(mcp_tool.input_schema.clone());

        let tool_def = ToolDefinition {
            name: prefixed_name,
            description,
            parameters,
            annotations: mcp_tool.annotations.clone(),
        };

        let adapter = ArcClientAdapter {
            client: shared.clone(),
            mcp_tool,
            tool_def,
            timeout: Duration::from_secs(if timeout_secs > 0 { timeout_secs } else { 30 }),
        };
        adapters.push(Box::new(adapter));
    }

    Ok(adapters)
}

/// Internal adapter that uses an Arc<Mutex<Client>> for shared access.
struct ArcClientAdapter {
    client: std::sync::Arc<tokio::sync::Mutex<Box<dyn Client>>>,
    mcp_tool: McpTool,
    tool_def: ToolDefinition,
    timeout: Duration,
}

#[async_trait]
impl Tool for ArcClientAdapter {
    fn definition(&self) -> &ToolDefinition {
        &self.tool_def
    }

    async fn execute(&self, args: serde_json::Value) -> ToolResult {
        let args_map: HashMap<String, serde_json::Value> = if args.is_object() {
            serde_json::from_value(args).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let args_value = serde_json::to_value(&args_map).unwrap_or(serde_json::json!({}));

        let future = async {
            let mut guard = self.client.lock().await;
            guard.call_tool(&self.mcp_tool.name, args_value).await
        };

        let result = match timeout(self.timeout, future).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                return ToolResult::err(format!("MCP tool '{}' error: {}", self.mcp_tool.name, e));
            }
            Err(_) => {
                return ToolResult::err(format!(
                    "MCP tool '{}' timed out after {:?}",
                    self.mcp_tool.name, self.timeout
                ));
            }
        };

        if result.is_error {
            let err_msg: String = result
                .content
                .iter()
                .filter(|c| c.content_type == "text")
                .filter_map(|c| c.text.as_deref())
                .collect::<Vec<&str>>()
                .join("; ");

            return ToolResult::err(format!(
                "MCP tool '{}' returned error: {}",
                self.mcp_tool.name,
                if err_msg.is_empty() {
                    "unknown error"
                } else {
                    &err_msg
                }
            ));
        }

        let text_parts: Vec<String> = result
            .content
            .iter()
            .map(|c| match c.content_type.as_str() {
                "text" => c.text.clone().unwrap_or_default(),
                "image" => format!("[Image: {}]", c.text.as_deref().unwrap_or("")),
                "resource" => format!("[Resource: {}]", c.text.as_deref().unwrap_or("")),
                _ => c.text.clone().unwrap_or_default(),
            })
            .collect();

        ToolResult::ok(text_parts.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sanitize a string for use as a tool identifier component.
///
/// Lowercases the input and replaces any character that's not alphanumeric
/// or an underscore with an underscore.
pub fn sanitize_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// 检测 server 名单中 sanitize 后工具前缀相撞的分组（2026-10-16 第十四轮
/// 缺陷 17）：`pet.v2` 与 `pet-v2` 都归一为 `mcp_pet_v2_`——热重载
/// `find_new_servers` 会把第二个当「已注册」静默过滤（永不挂载），启动
/// 全量挂载则工具键互相覆盖（后注册者胜）。检测单一真相源：加载面
/// （`McpManager::load_config`，覆盖启动 + 热重载）与写面（CLI `mcp add`
/// / Dashboard server_add）共用本函数。吃名字迭代器（manager 传类型化
/// 名单、CLI/web 传 raw JSON 名字，免 serde 往返）。返回只含冲突组的
/// `(prefix, colliding names)` 列表。
pub fn sanitize_prefix_collisions<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Vec<(String, Vec<String>)> {
    let mut groups: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for name in names {
        let prefix = format!("mcp_{}_", sanitize_name(name));
        groups.entry(prefix).or_default().push(name.to_string());
    }
    groups
        .into_iter()
        .filter(|(_, names)| names.len() > 1)
        .collect()
}

/// Sanitize an MCP input_schema for use as a function-calling parameters schema.
///
/// Some MCP servers return schemas with constructs that certain LLM providers
/// reject (e.g. `type: ["string", "null"]` as an array). This function ensures
/// the schema is compatible by:
/// - Flattening `type` arrays to the first type
/// - Recursively cleaning nested property schemas
/// - Ensuring top-level has `type: "object"` and `additionalProperties: false`
fn sanitize_schema(mut schema: serde_json::Value) -> serde_json::Value {
    // Ensure top-level is an object
    if !schema.is_object() {
        return serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false});
    }

    // Ensure type is "object" at top level
    schema["type"] = serde_json::Value::String("object".to_string());

    // Ensure additionalProperties is set
    if schema.get("additionalProperties").is_none() {
        schema["additionalProperties"] = serde_json::json!(false);
    }

    // Sanitize nested property schemas
    // 2026-10-25 BUG-20261025-47：flatten_type 递归覆盖 properties/items
    // 全深度（含数组元素）；2026-10-30 BUG-20261030-212 收编复合关键字
    // （anyOf/oneOf/allOf、对象形态 additionalProperties、$defs/definitions、
    // patternProperties/prefixItems）。此处一遍调用即可，无需分层手抄。
    flatten_type(&mut schema);

    schema
}

/// Flatten `type` arrays to the first element (e.g. `["string", "null"]` → `"string"`),
/// recursing at full depth: `properties`/`patternProperties`/`$defs`/`definitions`
/// (map-of-schema), `items`/`prefixItems` (single schema, tuple array), and the
/// composition keywords `anyOf`/`oneOf`/`allOf` plus object-form
/// `additionalProperties`.
///
/// 背景（BUG-20261025-47）：部分 LLM provider 拒绝数组形态的 `type`；旧实现
/// 只处理两层 properties 且完全不碰 `items`——深层嵌套或数组项带
/// `type: ["x","null"]` 时原样透传，provider 侧报 schema 非法。
///
/// 背景（BUG-20261030-212）：BUG-47 的收编面仍漏复合关键字——`anyOf`/
/// `oneOf`/`allOf` 分支（可空联合的常见载体）、对象形态 `additionalProperties`
/// （map 值 schema）、`$defs`/`definitions`（随 `$ref` 一起整体上行给
/// provider）、`patternProperties`/`prefixItems`——这些位置里的数组形态
/// `type` 照样穿透。布尔形态 `additionalProperties`（true/false）不是
/// schema，跳过不碰。
fn flatten_type(schema: &mut serde_json::Value) {
    if let Some(types) = schema.get("type").and_then(|t| t.as_array())
        && let Some(first) = types.first()
    {
        schema["type"] = first.clone();
    }
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    if let Some(props) = obj.get_mut("properties").and_then(|p| p.as_object_mut()) {
        for prop in props.values_mut() {
            flatten_type(prop);
        }
    }
    if let Some(props) = obj
        .get_mut("patternProperties")
        .and_then(|p| p.as_object_mut())
    {
        for prop in props.values_mut() {
            flatten_type(prop);
        }
    }
    if let Some(defs) = obj.get_mut("$defs").and_then(|d| d.as_object_mut()) {
        for def in defs.values_mut() {
            flatten_type(def);
        }
    }
    if let Some(defs) = obj.get_mut("definitions").and_then(|d| d.as_object_mut()) {
        for def in defs.values_mut() {
            flatten_type(def);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = obj.get_mut(key).and_then(|b| b.as_array_mut()) {
            for branch in branches.iter_mut() {
                flatten_type(branch);
            }
        }
    }
    // 对象形态 additionalProperties = map 值的 schema；布尔形态（惯例的
    // 严/松开关）不是 schema，跳过。
    if let Some(ap) = obj.get_mut("additionalProperties")
        && ap.is_object()
    {
        flatten_type(ap);
    }
    // 数组项 schema：items/prefixItems 可能是单个 schema 或按位置排列的
    // schema 数组（tuple validation 形态）。
    for key in ["items", "prefixItems"] {
        match obj.get_mut(key) {
            Some(serde_json::Value::Array(items)) => {
                for item in items.iter_mut() {
                    flatten_type(item);
                }
            }
            Some(item @ serde_json::Value::Object(_)) => flatten_type(item),
            _ => {}
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
