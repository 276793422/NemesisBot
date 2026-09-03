//! Agent types used within the agent engine.
//!
//! These types represent conversation turns, tool results, agent state,
//! and events emitted during the agent loop execution.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tracing::debug;

/// Configuration for an agent instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// The LLM model identifier (e.g. "gpt-4", "claude-sonnet-4-6").
    pub model: String,
    /// System prompt injected at the start of every conversation.
    pub system_prompt: Option<String>,
    /// Maximum number of LLM tool-calling iterations per request.
    pub max_turns: u32,
    /// Names of tools available to this agent.
    pub tools: Vec<String>,
    /// Model alias → model id map for per-turn switching (Flash-first cost control).
    /// e.g. {"flash": "deepseek-v4-flash", "pro": "deepseek-v4-pro"}.
    /// `/model <alias>` resolves via this; an unknown alias is used as a literal model id.
    #[serde(default)]
    pub models: HashMap<String, String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4".to_string(),
            system_prompt: None,
            max_turns: 100,
            tools: Vec::new(),
            models: HashMap::new(),
        }
    }
}

/// A single conversation turn in the agent history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationTurn {
    /// Role: "system", "user", "assistant", or "tool".
    pub role: String,
    /// Text content of the turn.
    ///
    /// X1 (U3 projection prune): for role="tool" turns this is the tool's
    /// ORIGINAL output, un-pruned and un-spilled — the mid-section stays
    /// recoverable (history replay / session branching). The bounded,
    /// model-facing form is derived per-request via
    /// [`ConversationTurn::model_facing_content`]; the provider never sees
    /// more than the prune budget of any tool result.
    pub content: String,
    /// Tool calls issued by the assistant in this turn.
    pub tool_calls: Vec<ToolCallInfo>,
    /// Tool call ID this turn responds to (set for role "tool").
    pub tool_call_id: Option<String>,
    /// Timestamp of the turn (ISO 8601).
    pub timestamp: String,
    /// Reasoning content from thinking-mode models (e.g., DeepSeek R1, GLM).
    /// Stored for passing back to the API in subsequent turns.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
    /// X1 (U3 projection prune): tool name for role="tool" turns. The prune
    /// marker embeds the tool name, and with the gates moved to the
    /// projection the recompute must be self-contained per turn (the name
    /// otherwise only lives on the preceding assistant turn's tool_calls,
    /// which pure per-turn functions cannot see). None on old sessions and
    /// non-tool turns — the marker then falls back to "tool".
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_name: Option<String>,
    /// X1 (U3 projection prune): the COMPLETE model-facing replacement text,
    /// recorded at tool time ONLY when it cannot be recomputed from
    /// `content` later — the spill tier (the locator path embeds a
    /// wall-clock stamp) and any turn-guard nudge decoration (⑤/⑤′/⑥ —
    /// dynamic per-turn state). `None` ⇒ the projection recomputes
    /// `prune_tool_result(content, tool_name)`, a pure deterministic
    /// function, so replay/branch rebuilds recompute rather than consult the
    /// injection ledger. Idempotence: prune output stays under the inline
    /// threshold, and old sessions' tool content is already pruned, so
    /// projecting an already-projected turn is a no-op.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_result_projection: Option<String>,
    /// T5/T6（多模态，goal 2026-09-03）：本 turn 附带的图片路径引用（路径
    /// 注入 + media 落盘引用）。历史只存引用不存字节——build_messages 每轮
    /// 水合重读（文件已删 → 占位文本），provider 字节路径零影响。
    /// 旧会话文件无此键 → `#[serde(default)]` 加载为空。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub image_refs: Vec<String>,
}

impl ConversationTurn {
    /// X1 (U3 projection prune): the content the MODEL must see for this
    /// turn. Tool results keep their original in history; this is the
    /// bounded projection — the recorded override when present (spill
    /// locator / guard nudges — not recomputable), else the pure prune
    /// recompute, else the content as-is. Same input ⇒ same output; both
    /// `project_history_for_request` (request building + replay rebuild)
    /// and the compaction token estimate go through here so they can never
    /// drift from what the provider actually receives.
    pub fn model_facing_content(&self) -> std::borrow::Cow<'_, str> {
        if self.role != "tool" {
            return std::borrow::Cow::Borrowed(&self.content);
        }
        if let Some(ref projection) = self.tool_result_projection {
            return std::borrow::Cow::Borrowed(projection.as_str());
        }
        match crate::prune::prune_tool_result(
            &self.content,
            self.tool_name.as_deref().unwrap_or("tool"),
        ) {
            Some(pruned) => std::borrow::Cow::Owned(pruned),
            None => std::borrow::Cow::Borrowed(&self.content),
        }
    }
}

/// Information about a single tool call within a conversation turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallInfo {
    /// Unique ID assigned by the LLM for this tool call.
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// JSON-encoded arguments for the tool.
    pub arguments: String,
}

/// Result returned after executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Name of the tool that was executed.
    pub tool_name: String,
    /// The output string from the tool.
    pub result: String,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

/// Options for LLM chat completion requests.
///
/// Mirrors the Go `options map[string]interface{}` passed to `Chat()`.
/// These control generation parameters like temperature and max output tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatOptions {
    /// Maximum number of tokens to generate in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0.0 = deterministic, 1.0 = creative).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling threshold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Stop sequences that end generation early.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
    /// H4 (U16 half): reasoning-effort tier ("low"|"medium"|"high"; None =
    /// send nothing). Translated by each provider into its wire format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            max_tokens: Some(8192),
            temperature: Some(0.7),
            top_p: None,
            stop: None,
            reasoning_effort: None,
        }
    }
}

/// Tool definition for LLM function calling.
///
/// Mirrors the OpenAI function calling format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool type (always "function").
    #[serde(rename = "type", default = "default_tool_type")]
    pub tool_type: String,
    /// Function definition.
    pub function: ToolFunctionDef,
}

fn default_tool_type() -> String {
    "function".to_string()
}

impl Default for ToolDefinition {
    fn default() -> Self {
        Self {
            tool_type: "function".to_string(),
            function: ToolFunctionDef::default(),
        }
    }
}

/// Function definition within a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunctionDef {
    /// Function name.
    pub name: String,
    /// Function description.
    pub description: String,
    /// JSON Schema for parameters.
    pub parameters: serde_json::Value,
}

impl Default for ToolFunctionDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }
}

/// Current operational state of an agent instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgentState {
    /// Agent is idle and ready to process a new request.
    #[default]
    Idle,
    /// Agent is waiting for an LLM response.
    Thinking,
    /// Agent is executing one or more tool calls.
    ExecutingTool,
    /// Agent is preparing the final response.
    Responding,
}

/// Events emitted by the agent loop during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    /// A text message was produced (intermediate or final).
    Message(String),
    /// The LLM requested one or more tool calls.
    ToolCall(Vec<ToolCallInfo>),
    /// A tool execution completed.
    ToolResult(ToolCallResult),
    /// An error occurred during execution.
    Error(String),
    /// The agent loop has finished processing.
    Done(String),
}

/// Fix orphaned tool message pairs in-place.
///
/// Guarantees three invariants:
/// 1. No two role="tool" messages share the same tool_call_id (keep the last)
/// 2. Every role="tool" message has a preceding role="assistant" with matching tool_call id
/// 3. Every assistant tool_call id has a corresponding tool response after it
///
/// When violated:
/// - Duplicate tool messages (same tool_call_id) collapse to the last one
/// - Orphaned tool messages are removed
/// - Assistant tool_calls without responses get a SYNTHETIC tool result
///   (see [`TOOL_OUTCOME_UNKNOWN`]) instead of being silently dropped: the
///   model keeps visibility of its own half-executed call and can decide
///   whether a retry is safe.
///
/// This operates on the projection copy built by `build_messages` — it never
/// mutates the stored `AgentInstance` history.
pub const TOOL_OUTCOME_UNKNOWN: &str = "TOOL_OUTCOME_UNKNOWN";

/// Wording for the synthetic tool result injected by
/// [`repair_tool_message_pairs`]. Deliberately ONE tier: unlike dsh (which has
/// a durable `tool/call` event and can distinguish "started, outcome unknown"
/// from "never started"), we have no execution-trace event, so splitting into
/// two tiers would fabricate precision we do not have. The single message
/// tells the model the outcome is unknown and how to decide on a retry.
const TOOL_OUTCOME_UNKNOWN_TEXT: &str = "该工具调用没有记录到结果，其执行结果未知。请根据操作性质决定：仅在操作是只读或幂等时可以直接重试；若可能有副作用（写文件、执行命令、发送消息等），请先验证外部状态（读回文件 / 检查输出）或询问用户，不要盲目重试。";

pub fn repair_tool_message_pairs(messages: &mut Vec<ConversationTurn>) {
    if messages.is_empty() {
        return;
    }

    // Pass 0: deduplicate tool messages by tool_call_id (keep the last occurrence).
    // This handles the case where an async placeholder and a real callback result
    // both exist with the same tool_call_id — LLM APIs reject duplicates.
    {
        let mut seen: HashSet<String> = HashSet::new();
        let mut to_remove: Vec<usize> = Vec::new();
        for (idx, msg) in messages.iter().enumerate().rev() {
            if msg.role == "tool"
                && let Some(ref id) = msg.tool_call_id
                && !seen.insert(id.clone())
            {
                to_remove.push(idx);
            }
        }
        for idx in to_remove {
            debug!(
                "[repair_tool_message_pairs] Removing duplicate tool message at {}",
                idx
            );
            messages.remove(idx);
        }
    }

    // Pass 1: remove orphaned tool messages via retain.
    let mut seen_call_ids: HashSet<String> = HashSet::new();
    messages.retain(|msg| {
        let keep = if msg.role == "tool" {
            match msg.tool_call_id {
                Some(ref id) => seen_call_ids.contains(id),
                None => false,
            }
        } else {
            true
        };

        if msg.role == "assistant" {
            for tc in &msg.tool_calls {
                seen_call_ids.insert(tc.id.clone());
            }
        }

        if !keep {
            debug!("[repair_tool_message_pairs] Removing orphaned tool message");
        }
        keep
    });

    // Pass 2: give every unanswered assistant tool_call a synthetic tool
    // result instead of dropping the call. Dropping hid the model's own
    // half-executed call from it: pairing was preserved but the model had no
    // basis to decide whether a retry was safe. The synthetic result states
    // the outcome is unknown and the retry policy; the call itself stays in
    // `tool_calls` so the provider transcript stays balanced.
    //
    // Insertion must happen right after the assistant turn's own position
    // (before the next assistant), and multiple missing calls insert in the
    // model's own call order. Because we grow `messages` while iterating over
    // indices captured on the ORIGINAL length, process the original range in
    // order and collect (insert_at, turn) pairs first, then apply them
    // back-to-front so earlier indices stay valid.
    let n = messages.len();
    let mut insertions: Vec<(usize, ConversationTurn)> = Vec::new();
    for i in 0..n {
        if messages[i].role == "assistant" && !messages[i].tool_calls.is_empty() {
            let call_ids: Vec<String> = messages[i]
                .tool_calls
                .iter()
                .map(|tc| tc.id.clone())
                .collect();
            let mut found_ids: HashSet<String> = HashSet::new();
            // Scan to the END of the projection, not just to the next
            // assistant: Pass 0 already guarantees at most one tool result
            // per call id, so a result found anywhere after this turn means
            // the call IS answered. Breaking at an intervening assistant here
            // would synthesize a SECOND result for an id that already has one
            // (duplicate tool_call_id — providers reject that).
            for m in messages.iter().take(n).skip(i + 1) {
                if m.role == "tool"
                    && let Some(ref tc_id) = m.tool_call_id
                    && call_ids.contains(tc_id)
                {
                    found_ids.insert(tc_id.clone());
                }
            }
            if found_ids.len() < call_ids.len() {
                for tc in &messages[i].tool_calls {
                    if !found_ids.contains(&tc.id) {
                        debug!(
                            "[repair_tool_message_pairs] Synthesizing unknown-outcome tool result for call {}",
                            tc.id
                        );
                        insertions.push((
                            i + 1,
                            ConversationTurn {
                                role: "tool".to_string(),
                                content: format!(
                                    "[{}] {}",
                                    TOOL_OUTCOME_UNKNOWN, TOOL_OUTCOME_UNKNOWN_TEXT
                                ),
                                tool_calls: Vec::new(),
                                tool_call_id: Some(tc.id.clone()),
                                timestamp: messages[i].timestamp.clone(),
                                reasoning_content: None,
                                tool_name: Some(tc.name.clone()),
                                tool_result_projection: None,
                                image_refs: Vec::new(),
                            },
                        ));
                    }
                }
            }
        }
    }
    // Apply back-to-front. Multiple insertions at the same base index were
    // pushed in call order; inserting back-to-front reverses that, so apply
    // same-index groups front-to-back by iterating the collected pairs in
    // order within equal base indices — simplest correct approach: iterate
    // collected pairs in REVERSE overall, and for equal base indices they
    // were collected in forward call order, so reverse order inserts the LAST
    // call first at position i+1, then the second-to-last at i+1 lands BEFORE
    // it — which restores the model's call order. Verified by test
    // `test_repair_synthesizes_unknown_outcome_tool_result`.
    for (at, turn) in insertions.into_iter().rev() {
        messages.insert(at, turn);
    }
}

#[cfg(test)]
mod tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
