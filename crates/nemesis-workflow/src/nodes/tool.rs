// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 nodes.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

// 跨子文件引用（机械拆分）：helpers 定义在 llm.rs / 本文件
use super::llm::{failed_node_result, resolve_runtime_string};

/// Placeholder executor for `tool` nodes on engines without a tool bridge.
///
/// The real executor is [`RealToolNodeExecutor`] — it dispatches to a shared
/// [`nemesis_tools::registry:ToolRegistry`] and is wired by
/// `WorkflowEngine::new_integrated*` (gateway side). Engines built with
/// `WorkflowEngine::new()` (CLI bare engine) have no registry, so this
/// placeholder answers instead.
///
/// (, quality-hardening goal 冲刺) It previously fabricated
/// `{"tool":…,"status":"success"}` + Completed without executing anything —
/// a silent fake success: `nemesisbot workflow run` reported Completed for
/// tool-node workflows while doing none of the work. It now surfaces an
/// explicit **Failed** NodeResult carrying the reason, mirroring the
/// `execute_inline_node` precedent for unconfigured complex types, so the
/// failure is loud and branchable instead of silent.
pub struct ToolNodeExecutor;

#[async_trait]
impl NodeExecutor for ToolNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();
        // Resolve the configured tool name the same way RealToolNodeExecutor
        // does ("name" preferred, legacy "tool") so the error names what the
        // workflow author asked for.
        let tool_name = node
            .config
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| node.config.get("tool").and_then(|v| v.as_str()))
            .unwrap_or("unknown");
        Ok(failed_node_result(
            &node.id,
            started,
            &format!(
                "tool node '{}' requests tool '{}' but this engine has no tool executor \
                 configured (未配置工具执行器): bare WorkflowEngine::new() cannot execute tools; \
                 wire RealToolNodeExecutor via new_integrated_with_dirs or register an executor \
                 for node type 'tool'",
                node.id, tool_name
            ),
        ))
    }
}

/// Real tool node executor that delegates to `nemesis_tools::ToolRegistry`.
///
/// Looks up the tool by name and invokes it with resolved args. Tool errors
/// are surfaced as a Failed NodeResult (not Err), so the workflow can branch
/// on failure states.
///
/// Config fields:
/// - `name` (preferred) or `tool` (legacy): tool name, required
/// - `args`: JSON object of tool arguments; `{{var}}` placeholders are
///   resolved against the executor context
pub struct RealToolNodeExecutor {
    tools: Arc<nemesis_tools::registry::ToolRegistry>,
}

impl RealToolNodeExecutor {
    pub fn new(tools: Arc<nemesis_tools::registry::ToolRegistry>) -> Self {
        Self { tools }
    }

    pub fn tools(&self) -> &Arc<nemesis_tools::registry::ToolRegistry> {
        &self.tools
    }
}

#[async_trait]
impl NodeExecutor for RealToolNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let started = Local::now();

        let tool_name = node
            .config
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| node.config.get("tool").and_then(|v| v.as_str()));
        let tool_name = match tool_name {
            Some(n) => n,
            None => {
                return Ok(tool_failed_node_result(
                    &node.id,
                    started,
                    "missing required 'name' (or legacy 'tool') field",
                ));
            }
        };

        let raw_args = node
            .config
            .get("args")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let resolved_args = match resolve_template_value(&raw_args, context, "tool.args") {
            Ok(v) => v,
            Err(e) => return Ok(failed_node_result(&node.id, started, &e)),
        };

        let tool_result = self.tools.execute(tool_name, &resolved_args).await;

        Ok(tool_result_to_node_result(
            &node.id,
            started,
            tool_name,
            tool_result,
        ))
    }
}

/// Resolve `{{var}}` placeholders (and whole-value credential references)
/// inside an arbitrary JSON value.
///
/// Strings get [`resolve_runtime_string`]; objects and arrays are recursed
/// (object keys extend `field_ctx` as `parent.key` for error messages);
/// other scalars pass through unchanged. Fallible: a credential reference
/// that fails to resolve aborts with the field path in the error.
pub(super) fn resolve_template_value(
    value: &serde_json::Value,
    context: &HashMap<String, serde_json::Value>,
    field_ctx: &str,
) -> Result<serde_json::Value, String> {
    match value {
        serde_json::Value::String(s) => Ok(serde_json::Value::String(resolve_runtime_string(
            s, context, field_ctx,
        )?)),
        serde_json::Value::Array(arr) => {
            let resolved = arr
                .iter()
                .map(|v| resolve_template_value(v, context, field_ctx))
                .collect::<Result<Vec<_>, String>>()?;
            Ok(serde_json::Value::Array(resolved))
        }
        serde_json::Value::Object(obj) => {
            let mut resolved = serde_json::Map::new();
            for (k, v) in obj {
                let child_ctx = format!("{field_ctx}.{k}");
                resolved.insert(k.clone(), resolve_template_value(v, context, &child_ctx)?);
            }
            Ok(serde_json::Value::Object(resolved))
        }
        other => Ok(other.clone()),
    }
}

/// Resolve the `items` config of a foreach loop into a concrete list.
///
/// Accepts:
/// - A JSON array (literal or templated) — each element template-resolved.
/// - A string that is a single `{{ref}}` placeholder — resolved to the **raw**
///   context value, so an upstream node's array output is NOT stringified
///   (which `resolve_template_value` would do). This is the common case for
///   `items: "{{node.field}}"`.
/// - Any other string — template-resolved, then parsed as a JSON array; if
///   that fails, split into non-empty lines (handy for plain text lists such
///   as one URL per line).
pub(super) fn resolve_items_list(
    raw: &serde_json::Value,
    context: &HashMap<String, serde_json::Value>,
) -> Result<Vec<serde_json::Value>, String> {
    if let Some(arr) = raw.as_array() {
        return arr
            .iter()
            .map(|v| resolve_template_value(v, context, "foreach.items"))
            .collect();
    }
    let s = raw
        .as_str()
        .ok_or_else(|| format!("foreach `items` must be an array or a string, got {}", raw))?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    // Single whole-string reference: pull the raw context value (no
    // stringification) so an array output stays an array.
    if trimmed.starts_with("{{") && trimmed.ends_with("}}") {
        let inner = trimmed[2..trimmed.len() - 2].trim();
        if !inner.is_empty()
            && !inner.contains("{{")
            && !inner.contains(' ')
            && let Some(val) = context.get(inner)
            && let Some(arr) = val.as_array()
        {
            return Ok(arr.clone());
        }
        // Referenced value isn't an array — fall through to string
        // parsing (JSON-array string or newline-separated list).
    }

    // General string: resolve placeholders (or a whole-value credential
    // reference), then JSON-array parse, else lines.
    let resolved = resolve_runtime_string(trimmed, context, "foreach.items")?;
    if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(&resolved)
    {
        return Ok(arr);
    }
    Ok(resolved
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::Value::String(l.to_string()))
        .collect())
}

/// Convert a `ToolResult` into a workflow `NodeResult`.
///
/// Tool success → Completed with `output = {tool, result,...}`.
/// Tool error → Failed with the LLM-facing message in `error`.
fn tool_result_to_node_result(
    node_id: &str,
    started: chrono::DateTime<Local>,
    tool_name: &str,
    result: nemesis_tools::types::ToolResult,
) -> NodeResult {
    let ended = Local::now();
    if result.is_error {
        NodeResult {
            node_id: node_id.to_string(),
            output: serde_json::Value::Null,
            error: Some(format!("tool '{}' error: {}", tool_name, result.for_llm)),
            state: ExecutionState::Failed,
            started_at: started,
            ended_at: ended,
            metadata: HashMap::new(),
        }
    } else {
        NodeResult {
            node_id: node_id.to_string(),
            output: serde_json::json!({
                "tool": tool_name,
                "result": result.for_llm,
                "silent": result.silent,
                "async": result.is_async,
                "task_id": result.task_id,
            }),
            error: None,
            state: ExecutionState::Completed,
            started_at: started,
            ended_at: ended,
            metadata: HashMap::new(),
        }
    }
}

/// Build a Failed NodeResult for tool resolution failures (missing config,
/// template errors, etc.). Used before we ever invoke the tool.
fn tool_failed_node_result(
    node_id: &str,
    started: chrono::DateTime<Local>,
    error: &str,
) -> NodeResult {
    NodeResult {
        node_id: node_id.to_string(),
        output: serde_json::Value::Null,
        error: Some(error.to_string()),
        state: ExecutionState::Failed,
        started_at: started,
        ended_at: Local::now(),
        metadata: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Condition Node
// ---------------------------------------------------------------------------
