// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

#[cfg(feature = "workflow")]
use super::*;

/// Agent tool that invokes a registered workflow synchronously.
///
/// Mirrors Go's `workflow_run` tool. The agent supplies a workflow name and
/// an optional input object; the tool calls `WorkflowEngine::run` and
/// returns the execution id / state / aggregated node output as JSON.
///
/// **Recursion depth**: each call increments the depth carried in
/// `TriggerSource::AgentTool`. When the new depth would exceed
/// `MAX_RECURSION_DEPTH`, the call is rejected without dispatching to the
/// engine. This prevents runaway `workflow_run → agent → workflow_run`
/// cycles from stack-allocating unbounded tokio tasks.
#[cfg(feature = "workflow")]
pub struct WorkflowRunTool {
    engine: Arc<nemesis_workflow::engine::WorkflowEngine>,
    /// Depth attributed to the call this tool is about to make. Top-level
    /// agent calls pass 0; nested calls (when the tool is itself invoked
    /// from inside a sub_workflow that was triggered by an AgentTool) read
    /// this from the enclosing execution's trigger source. The tool always
    /// increments by 1 before dispatching.
    starting_depth: u32,
}

#[cfg(feature = "workflow")]
impl WorkflowRunTool {
    pub fn new(engine: Arc<nemesis_workflow::engine::WorkflowEngine>) -> Self {
        Self {
            engine,
            starting_depth: 0,
        }
    }

    /// Construct with a non-zero starting depth. Used when the tool is
    /// invoked from within a workflow that was itself triggered by an
    /// AgentTool (so the depth chains correctly across nestings).
    pub fn with_starting_depth(
        engine: Arc<nemesis_workflow::engine::WorkflowEngine>,
        starting_depth: u32,
    ) -> Self {
        Self {
            engine,
            starting_depth,
        }
    }
}

#[cfg(feature = "workflow")]
#[async_trait]
impl Tool for WorkflowRunTool {
    fn description(&self) -> String {
        "Run a registered workflow by name. Returns the workflow's execution id, final state, and aggregated node output as JSON. Use this when the user wants to execute a multi-step predefined process (a workflow) rather than ad-hoc tool calls.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "workflow": {
                    "type": "string",
                    "description": "Name of the registered workflow to run."
                },
                "input": {
                    "type": "object",
                    "description": "Optional input variables for the workflow. Keys become workflow variables accessible to nodes.",
                    "additionalProperties": true
                }
            },
            "required": ["workflow"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let args_value: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {}", e))?;

        let workflow_name = args_value
            .get("workflow")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "parameter 'workflow' (non-empty string) is required".to_string())?
            .to_string();

        let input: std::collections::HashMap<String, serde_json::Value> =
            match args_value.get("input") {
                Some(serde_json::Value::Object(map)) => {
                    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                }
                Some(serde_json::Value::Null) | None => std::collections::HashMap::new(),
                Some(other) => {
                    return Err(format!(
                        "parameter 'input' must be an object, got {}",
                        other
                    ));
                }
            };

        let new_depth = self.starting_depth.saturating_add(1);
        if new_depth > nemesis_workflow::MAX_RECURSION_DEPTH {
            return Err(format!(
                "workflow_run recursion limit reached: new depth {} would exceed MAX_RECURSION_DEPTH={}",
                new_depth,
                nemesis_workflow::MAX_RECURSION_DEPTH
            ));
        }

        let trigger = nemesis_workflow::types::TriggerSource::AgentTool {
            tool_call_id: uuid::Uuid::new_v4().to_string(),
            recursion_depth: new_depth,
        };

        let execution = self
            .engine
            .run(&workflow_name, input, Some(trigger))
            .await
            .map_err(|e| format!("workflow '{}' failed to execute: {}", workflow_name, e))?;

        let mut output_map = serde_json::Map::new();
        for (node_id, nr) in &execution.node_results {
            output_map.insert(node_id.clone(), nr.output.clone());
        }
        let payload = serde_json::json!({
            "execution_id": execution.id,
            "workflow": execution.workflow_name,
            "state": format!("{:?}", execution.state),
            "started_at": execution.started_at,
            "ended_at": execution.ended_at,
            "node_results": serde_json::Value::Object(output_map),
            "error": execution.error,
        });
        Ok(serde_json::to_string(&payload)
            .map_err(|e| format!("failed to serialize workflow output: {}", e))?)
    }
}

// ===========================================================================
// WorkflowCreateTool / WorkflowCapabilitiesTool — 对话生成 capability core.
// Two-phase: this tool only ever writes DRAFTS (zero effect on registered
// workflows); the human applies them from the workflow page's draft panel.
// ===========================================================================

/// Agent tool that saves an AI-generated workflow definition as a draft.
///
/// The definition is validated (errors reported back for self-correction)
/// and persisted under `{workspace}/workflow/drafts/<name>.yaml`, but NOT
/// registered. `drafts::DraftStore::apply` (human-in-the-loop, from the UI)
/// is the only path that turns a draft into a registered workflow — with an
/// automatic `.history/` backup when it replaces an existing definition.
#[cfg(feature = "workflow")]
pub struct WorkflowCreateTool {
    engine: Arc<nemesis_workflow::engine::WorkflowEngine>,
}

#[cfg(feature = "workflow")]
impl WorkflowCreateTool {
    pub fn new(engine: Arc<nemesis_workflow::engine::WorkflowEngine>) -> Self {
        Self { engine }
    }
}

#[cfg(feature = "workflow")]
#[async_trait]
impl Tool for WorkflowCreateTool {
    fn description(&self) -> String {
        "Save a workflow definition as a DRAFT (对话生成). Nothing is registered until the user applies the draft from the workflow page. Before writing a definition, call workflow_capabilities once to learn the exact node types and config fields. If validation_errors is non-empty in the response, fix the definition and re-save with the same name.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "definition": {
                    "type": "object",
                    "description": "Full workflow definition object: {name, description, triggers: [{trigger_type, config}], nodes: [{id, node_type, config, depends_on?, is_terminal?}], edges: [{from_node, to_node, condition?}]}. Call workflow_capabilities first for the exact field specs.",
                    "additionalProperties": true
                }
            },
            "required": ["definition"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let args_value: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {}", e))?;

        let definition = args_value
            .get("definition")
            .cloned()
            .ok_or_else(|| "parameter 'definition' (object) is required".to_string())?;

        let workflow: nemesis_workflow::types::Workflow = serde_json::from_value(definition)
            .map_err(|e| {
                format!(
                    "definition object does not match the workflow schema: {} — \
                     call workflow_capabilities and check field names/types",
                    e
                )
            })?;

        if workflow.name.trim().is_empty() {
            return Err("definition.name must be a non-empty string".to_string());
        }

        let store =
            nemesis_workflow::drafts::DraftStore::from_engine(&self.engine).ok_or_else(|| {
                "workflow definitions directory is not configured — cannot save drafts \
                 (workflow_defs_dir missing)"
                    .to_string()
            })?;

        let summary = store
            .save(&workflow)
            .map_err(|e| format!("save draft: {}", e))?;
        let existing_definition = self.engine.get_workflow(&workflow.name).is_some();

        let mut hints: Vec<String> = Vec::new();
        if !summary.valid {
            hints.push(
                "definition has validation errors — fix them and re-save with the same name"
                    .to_string(),
            );
        } else {
            hints.push(
                "draft saved. It does NOT run yet — the user must apply it from the \
                 workflow page (对话生成 → 草稿面板 → 应用). Tell the user the draft is ready."
                    .to_string(),
            );
        }
        if existing_definition {
            hints.push(
                "a registered workflow with this name already exists — applying the draft \
                 will replace it (the old definition is backed up to .history/). Confirm \
                 with the user first."
                    .to_string(),
            );
        }
        // 语义 lint（2026-09-23 计划类 C）：warning 不阻断，但经 hints 进
        // 本轮自纠回路——修了重存，或向用户确认属有意配置。
        for w in &summary.warnings {
            hints.push(format!(
                "warning: {w} — fix it and re-save with the same name, or explicitly \
                 confirm with the user that it is intentional"
            ));
        }

        let payload = serde_json::json!({
            "status": "draft_saved",
            "name": summary.name,
            "valid": summary.valid,
            "validation_errors": summary.validation_errors,
            "node_count": summary.node_count,
            "trigger_types": summary.trigger_types,
            "existing_definition": existing_definition,
            "hints": hints,
        });
        serde_json::to_string(&payload).map_err(|e| format!("serialize response: {}", e))
    }
}

/// Agent tool returning the generator capability table (node types, config
/// fields, trigger types, structural rules). Rendered from the static table
/// in `nemesis-workflow::capabilities`, which is pinned to the real executor
/// registry by anti-drift tests.
#[cfg(feature = "workflow")]
pub struct WorkflowCapabilitiesTool;

#[cfg(feature = "workflow")]
#[async_trait]
impl Tool for WorkflowCapabilitiesTool {
    fn description(&self) -> String {
        "Query the available workflow node types, per-node config fields, trigger types, and structural rules. Call this ONCE before writing any workflow definition.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(nemesis_workflow::capabilities::render_for_prompt())
    }
}
