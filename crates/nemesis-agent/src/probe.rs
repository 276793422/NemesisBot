//! Capability probe (small-model-tool-robustness plan, Phase 4b).
//!
//! Sends a fixed battery of 7 short tool-use prompts to a model and scores the
//! responses on three axes — format (did it use the `tool_calls` channel?),
//! selection (did it pick the right tool?), and schema (did the args validate?).
//! The aggregate scores map to a [`ModelTier`], giving a direct measurement of
//! tool-calling ability that complements the name/size heuristic.
//!
//! Scoring is pure and unit-tested; the LLM-call boundary is the async [`run`]
//! function. The probe is invoked only by the user (CLI `model probe` or
//! `--probe`) — never automatically injected into a live conversation.

use nemesis_types::capability::ModelTier;
use serde_json::Value;

use crate::r#loop::{LlmMessage, LlmProvider, LlmResponse};

/// A single probe task: a prompt, the tool we hope the model picks, and that
/// tool's parameter schema (used both to build the tool definition sent to the
/// model and to score the returned arguments).
#[derive(Debug, Clone)]
pub struct ProbeTask {
    pub prompt: &'static str,
    pub expected_tool: &'static str,
    pub schema: Value,
}

/// Per-axis score in `[0.0, 1.0]`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProbeScore {
    pub format: f64,
    pub selection: f64,
    pub schema: f64,
}

/// Aggregate probe report.
#[derive(Debug, Clone)]
pub struct ProbeReport {
    pub format_score: f64,
    pub selection_score: f64,
    pub schema_score: f64,
    pub tier: ModelTier,
    pub per_task: Vec<(String, ProbeScore)>,
}

/// The fixed 7-task battery. Tool names match the production tools so the
/// scored behaviour reflects real tool-use ability. The cluster task is included
/// deliberately (cluster is a project highlight) even though small models often
/// struggle with it — that's exactly what selection_score measures.
pub fn probe_tasks() -> Vec<ProbeTask> {
    vec![
        ProbeTask {
            prompt: "现在几点了？请用一个工具获取当前时间。",
            expected_tool: "exec",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"]
            }),
        },
        ProbeTask {
            prompt: "请读取 README.md 这个文件的内容。",
            expected_tool: "read_file",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        },
        ProbeTask {
            prompt: "请创建一个名叫 test 的目录。",
            expected_tool: "create_dir",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }),
        },
        ProbeTask {
            prompt: "请在工作区里搜索字符串 TODO。",
            expected_tool: "grep",
            schema: serde_json::json!({
                "type": "object",
                "properties": {"pattern": {"type": "string"}},
                "required": ["pattern"]
            }),
        },
        ProbeTask {
            prompt: "请把刚刚读到的东西写进 note.md 里。",
            expected_tool: "write_file",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
        },
        ProbeTask {
            prompt: "请把 note.md 里的 foo 替换成 bar。",
            expected_tool: "edit_file",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_text": {"type": "string"},
                    "new_text": {"type": "string"}
                },
                "required": ["path", "old_text", "new_text"]
            }),
        },
        ProbeTask {
            prompt: "请通过集群把消息「你好」转发给另一个节点。",
            expected_tool: "cluster_rpc",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "target_node": {"type": "string"},
                    "message": {"type": "string"}
                },
                "required": ["target_node", "message"]
            }),
        },
    ]
}

/// Build the tool-definition list sent to the model for the battery. Each task
/// contributes its own (expected_tool, schema); duplicates are deduped by name.
pub fn probe_tool_defs() -> Vec<crate::types::ToolDefinition> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<&'static str, Value> = BTreeMap::new();
    for t in probe_tasks() {
        map.entry(t.expected_tool).or_insert(t.schema);
    }
    map.into_iter()
        .map(|(name, schema)| crate::types::ToolDefinition {
            tool_type: "function".to_string(),
            function: crate::types::ToolFunctionDef {
                name: name.to_string(),
                description: format!("Probe tool: {}", name),
                parameters: schema,
            },
        })
        .collect()
}

/// Score one model response against the task.
pub fn score_response(resp: &LlmResponse, task: &ProbeTask) -> ProbeScore {
    if resp.tool_calls.is_empty() {
        // No tool call emitted (class A format failure).
        return ProbeScore::default();
    }
    let format = 1.0;
    let tc = &resp.tool_calls[0];
    let selection = if tc.name == task.expected_tool { 1.0 } else { 0.0 };
    let schema = match crate::args_validator::check(&task.schema, &tc.arguments) {
        crate::args_validator::Outcome::Valid => 1.0,
        crate::args_validator::Outcome::Fixed(_) => 0.5,
        crate::args_validator::Outcome::Invalid { .. } => 0.0,
    };
    ProbeScore { format, selection, schema }
}

/// Map aggregate axis scores to a capability tier.
pub fn tier_from_scores(format_score: f64, selection_score: f64, schema_score: f64) -> ModelTier {
    if format_score >= 0.8 && selection_score >= 0.8 && schema_score >= 0.8 {
        ModelTier::Big
    } else if schema_score >= 0.6 && format_score >= 0.5 && selection_score >= 0.5 {
        ModelTier::Normal
    } else {
        ModelTier::Mini
    }
}

/// Run the probe battery against `provider`/`model`. One LLM call per task.
///
/// **Cost**: 7 short chat completions. The caller MUST be the user (CLI) — never
/// invoke this automatically inside a live conversation.
pub async fn run(provider: &dyn LlmProvider, model: &str) -> Result<ProbeReport, String> {
    let tasks = probe_tasks();
    let tool_defs = probe_tool_defs();
    let opts = crate::types::ChatOptions::default();

    let mut per_task: Vec<(String, ProbeScore)> = Vec::with_capacity(tasks.len());
    let mut fmt_sum = 0.0;
    let mut sel_sum = 0.0;
    let mut sch_sum = 0.0;

    for task in &tasks {
        let messages = vec![
            LlmMessage {
                role: "system".to_string(),
                content: "You are a helpful assistant. When the user asks for an action, \
                          use the appropriate tool. Respond concisely."
                    .to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
            },
            LlmMessage {
                role: "user".to_string(),
                content: task.prompt.to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
            },
        ];
        let resp = provider
            .chat(model, messages, Some(opts.clone()), tool_defs.clone())
            .await
            .map_err(|e| format!("LLM chat failed on task '{}': {}", task.expected_tool, e))?;
        let score = score_response(&resp, task);
        fmt_sum += score.format;
        sel_sum += score.selection;
        sch_sum += score.schema;
        per_task.push((task.expected_tool.to_string(), score));
    }

    let n = tasks.len() as f64;
    let format_score = fmt_sum / n;
    let selection_score = sel_sum / n;
    let schema_score = sch_sum / n;
    let tier = tier_from_scores(format_score, selection_score, schema_score);

    Ok(ProbeReport {
        format_score,
        selection_score,
        schema_score,
        tier,
        per_task,
    })
}

#[cfg(test)]
mod tests;
