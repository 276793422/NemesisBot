// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// 2026-11-06 三合一收编（用户拍板，对齐 forge 单工具多 action 形态）：
// workflow_run / workflow_create / workflow_capabilities 收编为单个 `workflow`
// 工具 + `action` 判别字段（run / create / capabilities）。行为逐分支与旧三
// 工具一致；注册面只插 "workflow" 一个键，旧三名不再单独供给。
// 2026-11-07 增 apply 臂（用户拍板）：草稿登记动作进工具面，与 WSAPI
// workflow.draft_apply 同一 DraftStore::apply 路径；对话内 create→apply→run
// 全链路不再卡人工应用（原「应用由用户在工作流页完成」的闸让位，审计链照常）。
// ---------------------------------------------------------------------------

#[cfg(feature = "workflow")]
use super::*;

/// Agent tool for the workflow subsystem — single tool, action-discriminated.
///
/// - `action = "run"`: invokes a registered workflow synchronously. Mirrors
///   Go's `workflow_run` tool. The agent supplies a workflow name and an
///   optional input object; the tool calls `WorkflowEngine::run` and returns
///   the execution id / state / aggregated node output as JSON.
///
///   **Recursion depth**: each run increments the depth carried in
///   `TriggerSource::AgentTool`. When the new depth would exceed
///   `MAX_RECURSION_DEPTH`, the call is rejected without dispatching to the
///   engine. This prevents runaway `workflow → agent → workflow` cycles from
///   stack-allocating unbounded tokio tasks.
///
/// - `action = "create"`: saves an AI-generated workflow definition as a
///   DRAFT (对话生成). The draft is NOT registered — it cannot run until it
///   is applied.
///
/// - `action = "apply"` (2026-11-07, user-approved): registers a previously
///   saved draft so it can run. Same `DraftStore::apply` path as the workflow
///   page's draft panel (WSAPI `workflow.draft_apply`): re-validate → back up
///   any replaced definition to `.history/` → persist + register → delete the
///   draft. Registration itself executes nothing — nodes only run via
///   `action = "run"`.
///
/// - `action = "capabilities"`: returns the generator capability table
///   (node types, config fields, trigger types, structural rules).
#[cfg(feature = "workflow")]
pub struct WorkflowTool {
    engine: Arc<nemesis_workflow::engine::WorkflowEngine>,
    /// Depth attributed to the run this tool is about to make. Top-level
    /// agent calls pass 0; nested calls (when the tool is itself invoked
    /// from inside a sub_workflow that was triggered by an AgentTool) read
    /// this from the enclosing execution's trigger source. Each run
    /// increments by 1 before dispatching.
    starting_depth: u32,
}

#[cfg(feature = "workflow")]
impl WorkflowTool {
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
impl Tool for WorkflowTool {
    fn description(&self) -> String {
        "Workflow subsystem tool (action-discriminated). action=\"run\": execute a registered workflow by name and return its execution id, final state, and aggregated node output as JSON — use this when the user wants to execute a multi-step predefined process rather than ad-hoc tool calls. action=\"create\": save a workflow definition as a DRAFT (对话生成); the draft cannot run until it is applied — call action=\"capabilities\" once first to learn the exact node types and config fields, and if validation_errors is non-empty in the response, fix the definition and re-save with the same name. action=\"apply\": register a previously saved draft (by name) so it can run — same path as the workflow page's apply button; a replaced same-name definition is backed up to .history/ automatically; applying over an existing registered workflow replaces it, so confirm with the user first in that case. action=\"capabilities\": query the available workflow node types, per-node config fields, trigger types, and structural rules.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["run", "create", "apply", "capabilities"],
                    "description": "Which workflow operation to perform."
                },
                "workflow": {
                    "type": "string",
                    "description": "(action=run|apply) Workflow name: the registered workflow to run (run), or the draft to register (apply)."
                },
                "input": {
                    "type": "object",
                    "description": "(action=run) Optional input variables for the workflow. Keys become workflow variables accessible to nodes.",
                    "additionalProperties": true
                },
                "definition": {
                    "type": "object",
                    "description": "(action=create) Full workflow definition object: {name, description, triggers: [{trigger_type, config}], nodes: [{id, node_type, config, depends_on?, is_terminal?}], edges: [{from_node, to_node, condition?}]}. Call with action=capabilities first for the exact field specs.",
                    "additionalProperties": true
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let args_value: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("invalid JSON args: {}", e))?;

        match args_value
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "parameter 'action' (one of run/create/apply/capabilities) is required".to_string()
            })? {
            "run" => self.run(args_value).await,
            "create" => self.create(args_value).await,
            "apply" => self.apply(args_value).await,
            "capabilities" => Ok(nemesis_workflow::capabilities::render_for_prompt()),
            other => Err(format!(
                "unknown action '{other}' — expected one of run/create/apply/capabilities"
            )),
        }
    }

    /// 三合一的 per-action 操作投影（2026-11-06；2026-11-07 增 apply 臂）：
    /// 静态表无法按 args 分档，投影是管线与工具之间的唯一桥（mcp_discover
    /// 同款机制）。create 分支落草稿 YAML = file_write（旧 workflow_create
    /// 静态档）；run / capabilities / apply = internal（旧 workflow_run/
    /// capabilities 静态档——纯会话机制，工作流节点各自过闸；apply 只做
    /// 登记不执行节点，后果上限就是 run，同档）。args 非对象 / action 缺席
    /// 或未知 = None（落静态表兜底：FileWrite，写能力分支主导的安全过度
    /// 分类）。
    fn operation_override(&self, args: &str) -> Option<OperationOverride> {
        let v: serde_json::Value = serde_json::from_str(args).ok()?;
        let op = match v.get("action")?.as_str()? {
            "create" => "file_write",
            "run" | "capabilities" | "apply" => "internal",
            _ => return None,
        };
        Some(OperationOverride {
            op_type: op.to_string(),
            force_approval: None,
            receipt_ctx: None,
        })
    }
}

#[cfg(feature = "workflow")]
impl WorkflowTool {
    /// action=run（旧 WorkflowRunTool::execute 原样搬运，行为零变更）。
    async fn run(&self, args_value: serde_json::Value) -> Result<String, String> {
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
                "workflow recursion limit reached: new depth {} would exceed MAX_RECURSION_DEPTH={}",
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
        serde_json::to_string(&payload)
            .map_err(|e| format!("failed to serialize workflow output: {}", e))
    }

    /// action=create（旧 WorkflowCreateTool::execute 原样搬运，行为零变更）。
    ///
    /// The definition is validated (errors reported back for self-correction)
    /// and persisted under `{workspace}/workflow/drafts/<name>.yaml`, but NOT
    /// registered — it cannot run until applied. Registration goes through
    /// `drafts::DraftStore::apply` (re-validate → `.history/` backup on
    /// replace → persist + register → delete draft), reachable from this
    /// tool's own `action = "apply"` (2026-11-07) or the workflow page's
    /// draft panel (WSAPI `workflow.draft_apply`) — same single path.
    async fn create(&self, args_value: serde_json::Value) -> Result<String, String> {
        let definition = args_value
            .get("definition")
            .cloned()
            .ok_or_else(|| "parameter 'definition' (object) is required".to_string())?;

        let workflow: nemesis_workflow::types::Workflow = serde_json::from_value(definition)
            .map_err(|e| {
                format!(
                    "definition object does not match the workflow schema: {} — \
                     call this tool with action=capabilities and check field names/types",
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
                "draft saved. It does NOT run yet — register it yourself with \
                 action=apply (or the user can apply it from the workflow page's \
                 draft panel), then run it with action=run."
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

    /// action=apply（2026-11-07 新增，用户拍板）：把既有草稿登记进引擎注册表。
    /// 与 WSAPI `workflow.draft_apply` / 工作流页「应用」按钮同一条
    /// `DraftStore::apply` 路径（单一真相源）：重校验 → 覆盖同名时 `.history/`
    /// 自动备份 → persist_workflow 注册 → 删草稿。登记本身不执行任何节点
    /// （执行只走 action=run）；响应带 `replaced_existing`/`backup_file`，
    /// 覆盖场景由提示词软闸引导模型先向用户确认。
    async fn apply(&self, args_value: serde_json::Value) -> Result<String, String> {
        let name = args_value
            .get("workflow")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "parameter 'workflow' (non-empty string) is required".to_string())?
            .to_string();

        let store =
            nemesis_workflow::drafts::DraftStore::from_engine(&self.engine).ok_or_else(|| {
                "workflow definitions directory is not configured — cannot apply drafts \
                 (workflow_defs_dir missing)"
                    .to_string()
            })?;

        let applied = store
            .apply(&self.engine, &name)
            .map_err(|e| format!("apply draft '{}': {}", name, e))?;

        let hint = if applied.replaced_existing {
            format!(
                "a previously registered workflow named '{}' was replaced (old \
                 definition backed up to .history/ as '{}') — mention this to the user",
                applied.name,
                applied.backup_file.as_deref().unwrap_or("<unknown>")
            )
        } else {
            "registered. You can run it now with action=run".to_string()
        };
        let payload = serde_json::json!({
            "status": "applied",
            "name": applied.name,
            "replaced_existing": applied.replaced_existing,
            "backup_file": applied.backup_file,
            "hint": hint,
        });
        serde_json::to_string(&payload).map_err(|e| format!("serialize response: {}", e))
    }
}
