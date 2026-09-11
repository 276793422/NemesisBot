//! Agent loop: core execution engine that drives LLM + tool interactions.
//!
//! The loop processes messages through these stages:
//!
//! 1. Build context from conversation history
//! 2. Call the LLM provider
//! 3. If the response contains tool calls, execute them and append results
//! 4. Repeat until a plain text response is produced or `max_turns` is reached
//!
//! # Bus-integrated mode
//!
//! The `AgentLoop` can be used in two ways:
//!
//! - **Standalone mode**: Direct calls via `run()`, `process_direct()`, etc.
//! - **Bus-integrated mode**: Continuous consumption from a message bus via
//!   `run_bus_arc()`.
//!
//! In bus-integrated mode, the loop connects to an `mpsc` inbound/outbound
//! channel pair and handles the full Go `AgentLoop` lifecycle including
//! system message routing, history requests, cluster continuation, slash
//! commands, session busy management, summarization, and startup info.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::context::RequestContext;
use crate::hooks::HookToolCall;
use crate::instance::AgentInstance;
use crate::registry::AgentRegistry;
use crate::session::{SessionStore, estimate_tokens_for_turns_projected};
use crate::types::{AgentConfig, AgentEvent, ToolCallInfo, ToolCallResult};
use nemesis_routing::{AgentDef, RouteConfig, RouteInput as RoutingRouteInput, RouteResolver};

/// Grace-round nudge injected when the tool-call budget is exhausted (②).
/// The model gets one extra round to synthesize a final answer from the work
/// already done, instead of hard-stopping with "Max iterations reached". This
/// is a TRANSIENT system message — appended to the built message list for the
/// grace round only, never persisted to instance history or session_log.
const GRACE_ROUND_NUDGE: &str = "工具调用预算已用尽，不要再调用任何工具。请基于已完成的工作给出最终答复：总结完成了什么、还有什么没做、需要用户做哪些决定。";

/// Max retries for transient LLM errors (network / stream / 5xx) before giving
/// up (③). Retries do NOT consume the `turns_used` budget — the increment at
/// the end of an iteration happens once regardless of how many retries it took
/// to get a successful response.
const MAX_TRANSIENT_RETRIES: u32 = 3;

/// ⑩ Per-session compaction tracking for graded tiers + stuck self-check.
/// Keyed by session; lives on `AgentLoop`.
#[derive(Default)]
struct CompactState {
    /// Whether the soft-tier (50%) notice has already been emitted this session
    /// (one-shot; do not nag).
    soft_noticed: bool,
    /// Token estimate at the time of the last summarization. The next
    /// summarization-triggered call compares against this to detect whether
    /// summarization is keeping up.
    last_summary_tokens: usize,
    /// Consecutive summarizations that failed to meaningfully reduce the
    /// prompt. At [`COMPACT_STUCK_LIMIT`] we pause auto-summarization.
    consecutive_failures: u32,
    /// Auto-summarization paused (stuck). Re-checked each call; cleared once
    /// the prompt drops back below the summarize threshold.
    stuck: bool,
}

/// ⑩ Soft tier: prompt at this fraction of context_window emits a one-shot
/// info notice (no summarization, cache-stable prefix intact).
const COMPACT_SOFT_RATIO: usize = 50; // % of context_window
/// ⑩ Summarize tier: at this fraction, trigger summarization.
const COMPACT_SUMMARIZE_RATIO: usize = 75; // % (unchanged from legacy behavior)
/// ⑩ Stuck: a summarization counts as ineffective when the prompt afterwards
/// is still at least this fraction of its pre-summarization size.
const COMPACT_STUCK_PLATEAU_RATIO: usize = 90; // %

/// N1（devtool-upgrade 阶段 1）：context_window 三级解析链的最后一级兜底。
/// 历史 32000 对现代 128k+ 模型意味着 24000 token 就触发压缩（编码场景过早
/// 失忆）——未配置且价目表未命中时按 128000 猜；猜大了也有既有溢出应急链
/// （`context_length_exceeded` → `force_compression` → 重试）自愈。
pub const FALLBACK_CONTEXT_WINDOW: usize = 128_000;

/// N1：三级 context_window 解析（纯函数，`AgentLoop` 与 CLI `model list` /
/// `model probe` 共用同一真相源）。返回 `(窗口, 来源)`：
/// - L1 config 显式 → `(Some(w), "config")`
/// - L2 价目表命中（`max_input_tokens > 0`，custom > downloaded > embedded
///   分层 + bare-suffix 匹配，`zhipu/glm-4.7`→`glm-4.7`）→ `(Some(w), "catalog")`
/// - L3 都没有 → `(None, "fallback-128k")`，调用方用
///   [`FALLBACK_CONTEXT_WINDOW`] 兜底。
pub fn resolve_context_window_tiered(
    cfg: Option<&serde_json::Value>,
    alias: &str,
    pricing: Option<&nemesis_data::PricingStore>,
) -> (Option<usize>, &'static str) {
    if let Some(cfg) = cfg
        && let Some(w) = nemesis_types::capability::resolve_context_window(cfg, alias)
    {
        return (Some(w as usize), "config");
    }
    if let Some(store) = pricing {
        // L2 第一轮：直接用活动别名查（standalone 形态下 alias 即 full
        // model 名，bare-suffix 匹配在 store.lookup 内部完成）。
        if let Some(w) = store
            .lookup(alias)
            .and_then(|p| p.max_input_tokens)
            .filter(|w| *w > 0)
        {
            return (Some(w as usize), "catalog");
        }
        // L2 第二轮：config 条目把别名映射到 full model 名（如
        // `glm`→`zhipu/glm-4.7`）时，用 full 名再查一轮——bare-suffix
        // `glm-4.7` 命中目录条目。
        if let Some(cfg) = cfg {
            let full = cfg
                .get("model_list")
                .and_then(|v| v.as_array())
                .and_then(|arr| {
                    arr.iter().find(|m| {
                        let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                        let full_model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                        name == alias || full_model == alias
                    })
                })
                .and_then(|m| m.get("model"))
                .and_then(|v| v.as_str());
            if let Some(full) = full
                && let Some(w) = store
                    .lookup(full)
                    .and_then(|p| p.max_input_tokens)
                    .filter(|w| *w > 0)
            {
                return (Some(w as usize), "catalog");
            }
        }
    }
    (None, "fallback-128k")
}

/// ⑩ Stuck limit: after this many consecutive ineffective summarizations,
/// pause auto-summarization and warn.
const COMPACT_STUCK_LIMIT: u32 = 2;

/// Target number of trailing messages kept verbatim (not summarized) when the
/// summary cache advances. The tail `history[C..]` is held at ~`K_TARGET`
/// messages by `maybe_update_summary` (C = len - K_TARGET). Small enough that
/// the LLM always sees recent context verbatim, large enough to ride out a
/// few tool-call rounds between summarizations.
const K_TARGET: usize = 6;

/// Aggressive verbatim-tail size used by `force_compression` (the last-resort
/// retry path on LLM context errors). Smaller than `K_TARGET` because the
/// situation is already an emergency — prefer a larger summarized prefix over
/// failing the request. A second compression folds everything into the summary
/// (tail → 0).
const SMALL_K_FORCE: usize = 2;

/// Session-keyed state maps on `AgentLoop` (`compact_state`, `summarizing`) are
/// size-bounded: when one exceeds this many entries we clear it wholesale. These
/// maps are best-effort — losing an entry just re-learns the state on the next
/// relevant call — and a wholesale clear under size pressure avoids the
/// unbounded-growth anti-pattern (one entry per session, never evicted, leaking
/// for the whole bot-process lifetime).
pub(crate) const SESSION_STATE_MAX_ENTRIES: usize = 512;

/// ⑩ Pure predicate: did a summarization fail to meaningfully reduce the
/// prompt? True when there was a prior summarization (`last_summary_tokens > 0`)
/// AND the current prompt is still at least [`COMPACT_STUCK_PLATEAU_RATIO`]% of
/// the pre-summarization size. Extracted so the threshold logic is unit-testable.
fn summarize_was_ineffective(last_summary_tokens: usize, current_tokens: usize) -> bool {
    last_summary_tokens > 0
        && current_tokens >= last_summary_tokens * COMPACT_STUCK_PLATEAU_RATIO / 100
}

/// Voice-playback suffix appended to the last user message when voice mode is
/// on (transient — never persisted to instance history / session_log).
/// T8 (U9 ②): hoisted from an inline literal so the replay ledger records the
/// exact same bytes the provider saw (single source of truth).
pub(crate) const VOICE_PLAYBACK_SUFFIX: &str = "（语音播报模式已开启，请用简洁、便于口语播报的方式回复，避免使用代码块、表格等不适合语音的内容。）";

/// Fold the summary cache over the history + enforce tool-pair consistency —
/// the persisted-derived projection every main-loop LLM request starts from.
///
/// T8 (U9 ②): extracted from `build_messages_with_memory` so the replay
/// ledger (`crate::replay`) rebuilds requests through the SAME code path —
/// production build and audit replay cannot drift apart (single source of
/// truth; same lesson as the T7 memory-tool schema-drift fix).
///
/// `summary` is `(text, covers_up_to)`; `None` sends the history verbatim.
/// `covers_up_to` indexes the full history vector including the system prompt
/// at index 0. The system prompt is never summarized — it is rebuilt as the
/// leading system message with the summary appended (so the cached prefix
/// stays stable between summary updates).
pub(crate) fn project_history_for_request(
    history: &[crate::types::ConversationTurn],
    summary: Option<(&str, usize)>,
) -> Vec<crate::types::ConversationTurn> {
    let mut turns: Vec<crate::types::ConversationTurn> = if let Some((text, covers_up_to)) = summary
    {
        let c_idx = covers_up_to.min(history.len());
        // Verbatim tail starts at c_idx but never re-includes the system
        // prompt at index 0 (it is rebuilt as the leading message below).
        let tail_start = c_idx.max(1).min(history.len());
        let summary_block = format!("\n\n## Summary of Previous Conversation\n\n{}", text);

        let mut out: Vec<crate::types::ConversationTurn> =
            Vec::with_capacity(history.len() - tail_start + 1);
        if history.first().is_some_and(|t| t.role == "system") {
            // Merge the summary into the configured system prompt (history[0]).
            let mut sys = history[0].clone();
            sys.content.push_str(&summary_block);
            out.push(sys);
        } else {
            // No system prompt at history[0]: emit the summary as a
            // dedicated leading system turn so the provider still sees it.
            out.push(crate::types::ConversationTurn {
                role: "system".to_string(),
                content: summary_block,
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: chrono::Local::now().to_rfc3339(),
                reasoning_content: None,
                tool_name: None,
                tool_result_projection: None,
                image_refs: Vec::new(),
            });
        }
        out.extend(history[tail_start..].iter().cloned());
        out
    } else {
        history.to_vec()
    };

    // X1 (U3 projection prune): tool results fold to their bounded
    // model-facing form HERE — history keeps the originals (recoverable
    // mid-sections, branchable history), the provider never sees an
    // oversized tool result. Recorded override wins (spill locator / guard
    // nudges — not recomputable); else the pure prune recompute. Idempotent:
    // prune output stays under the inline threshold, so old sessions whose
    // tool content is already pruned pass through byte-untouched. Because
    // replay rebuilds through this same function, the fold automatically
    // applies to audit replay too (no injection-ledger entry needed — the
    // transform is a pure function of the history state).
    for turn in &mut turns {
        if turn.role == "tool" {
            turn.content = turn.model_facing_content().into_owned();
        }
    }

    // Enforce tool-pair consistency at the LLM boundary. Upstream paths
    // (summarization, session save/load) can leave an assistant tool_call
    // whose result was dropped — or vice versa — and providers then reject
    // the whole request with 400 "insufficient tool messages following
    // tool_calls". Every main-loop LLM call's messages flow through here,
    // so repairing this local copy is the universal guarantee: the
    // provider never sees an inconsistent sequence, regardless of which
    // upstream path produced the history. (Non-destructive: the instance's
    // own history is untouched; only the outgoing view is cleaned.)
    crate::types::repair_tool_message_pairs(&mut turns);
    turns
}

/// A simplified LLM message used for building requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCallInfo>>,
    pub tool_call_id: Option<String>,
    /// Reasoning content from thinking-mode models, passed back to the API.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
    /// T5/T6（多模态，goal 2026-09-03）：本条消息携带的图片（已水合的
    /// base64 字节）。历史存储只留路径引用（`ConversationTurn.image_refs`），
    /// build_messages 组包时经 `hydrate_image_refs` 每轮重读填充；文件已删
    /// → 占位文本进 content。旧请求重放/下游适配器忽略此字段即纯文本路径，
    /// `#[serde(default)]` 保旧快照兼容。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<crate::image_attach::LlmImage>,
}

/// F-J（2026-09-04 四轮盲审）：observer 事件里 images[].data 的省略标记。
pub const OBSERVER_IMAGE_DATA_MARKER: &str =
    "[图片字节已省略：observer 事件不落 base64，路径见 path]";

/// observer 事件的 messages 序列化（F-J 单一真相源）。
///
/// build_messages 水合后的 `LlmMessage.images[].data` 是完整图片 base64
/// （单图 ≤25MB、每消息 ≤8 张）。逐轮 `serde_json::to_value` 若原样发射，
/// 字节会被复制进 observer 事件下游（to_callback_json /
/// request_logger_observer 的 01.AI.Request.raw.json），8×25MB 图 ≈ 每轮
/// ~270MB JSON 的磁盘/内存放大。此处在**发射源**把非空 `data` 替换为
/// [`OBSERVER_IMAGE_DATA_MARKER`]（保留 path/media_type 供排障定位）——
/// 下游消费方（loop_executor::to_callback_json / request_logger_observer）
/// 序列化的是事件里的值，源头干净则全链干净。真实 LLM 请求不受影响
/// （只改序列化副本，`messages` 本体原样上行）。
pub fn observer_msg_values(msgs: &[LlmMessage]) -> Vec<serde_json::Value> {
    msgs.iter()
        .filter_map(|m| {
            let mut v = serde_json::to_value(m).ok()?;
            if let Some(images) = v.get_mut("images").and_then(|i| i.as_array_mut()) {
                for img in images.iter_mut() {
                    let has_data = img
                        .get("data")
                        .and_then(|d| d.as_str())
                        .is_some_and(|s| !s.is_empty());
                    if let (true, Some(obj)) = (has_data, img.as_object_mut()) {
                        obj.insert(
                            "data".to_string(),
                            serde_json::Value::String(OBSERVER_IMAGE_DATA_MARKER.to_string()),
                        );
                    }
                }
            }
            Some(v)
        })
        .collect()
}

/// A simplified LLM response.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// Text content of the response. May be empty if tool_calls are present.
    pub content: String,
    /// Tool calls requested by the LLM, if any.
    pub tool_calls: Vec<ToolCallInfo>,
    /// Whether the LLM indicated it is finished (no more tool calls).
    pub finished: bool,
    /// Reasoning content from thinking-mode models.
    pub reasoning_content: Option<String>,
    /// Token usage from the provider response.
    pub usage: Option<crate::loop_executor::ObserverUsageInfo>,
    /// Raw HTTP request body (for raw logging mode).
    pub raw_request_body: Option<serde_json::Value>,
    /// Raw HTTP response body (for raw logging mode).
    pub raw_response_body: Option<String>,
}

/// Trait for LLM providers used by the agent loop.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat request and return the response, or an error if the call fails.
    ///
    /// The agent loop uses the `Err` variant to detect context-window errors
    /// (token limit, context length exceeded, etc.) and trigger history compression.
    ///
    /// The `options` parameter controls generation parameters (temperature, max_tokens, etc.).
    /// Pass `None` to use provider defaults.
    ///
    /// The `tools` parameter provides tool definitions for function calling.
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String>;
}

/// A previewable file change, used by the checkpoint (edit safety net) to
/// snapshot a file's pre-edit state so a `/rewind` can restore it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileChange {
    /// Path the tool will modify (as given in args; resolved against workspace
    /// root at snapshot/restore time).
    pub path: String,
    /// Kind of change — determines how a rewind restores it.
    pub kind: FileChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileChangeKind {
    /// File did not exist before the edit; rewind deletes it.
    Create,
    /// File existed and is being modified; rewind restores old content.
    Modify,
    /// File existed and is being deleted; rewind restores old content.
    Delete,
}

/// Trait for tools that can be executed by the agent loop.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Execute the tool with the given arguments, returning a string result.
    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String>;

    /// Set the execution context (channel + chat_id) for context-aware tools.
    ///
    /// This is called before each LLM iteration to inject the current channel
    /// and chat_id into tools that need them for routing (e.g., message, spawn,
    /// cluster_rpc). The default implementation is a no-op; tools that need
    /// context should override this method.
    fn set_context(&self, _channel: &str, _chat_id: &str) {}

    /// G2 (devtool-upgrade 阶段 3): notify the tool of the invocation's
    /// sub-agent nesting depth (0 = top-level agent, N = N levels deep).
    /// Called by `handle_tool_call_at_depth` right before execute, same
    /// injection pattern as `set_context`. Default no-op; depth-aware tools
    /// (spawn — `agents.subagent.max_depth` enforcement) override.
    fn set_invocation_depth(&self, _depth: usize) {}

    /// Return a human-readable description of this tool for the LLM.
    /// Mirrors Go's Tool.Description() string.
    fn description(&self) -> String {
        String::new()
    }

    /// Return the JSON schema for this tool's parameters.
    /// Mirrors Go's Tool.Parameters() map[string]interface{}.
    /// Should return a serde_json::Value representing an OpenAI-compatible
    /// JSON Schema object (e.g., {"type": "object", "properties": {...}}).
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    /// Preview the file change this call would make, for checkpointing (the edit
    /// safety net). Synchronous — parse `args` (the same JSON string passed to
    /// `execute`) to determine the target path and change kind only; the
    /// checkpoint store reads the file's current content separately (async).
    /// Returns `None` for read-only tools or non-file tools (default), so only
    /// writer tools opt in. Never panic on malformed args — return `None`.
    fn preview(&self, _args: &str) -> Option<FileChange> {
        None
    }

    /// A7（2026-09-06）：checkpoint 预检多点版——默认与 [`Tool::preview`]
    /// 等价（单文件）；multiedit 覆盖为逐文件清单，让检查点安全网在一次
    /// dispatch 内快照全部待改文件。调用点（K1a 瀑布）只消费本方法。
    fn preview_all(&self, args: &str) -> Vec<FileChange> {
        self.preview(args).into_iter().collect()
    }

    /// U5 (sixth batch): whether this tool is a pure read with no side effects
    /// (filesystem read, list, search, web fetch — safe to run concurrently
    /// with other read-only calls in the same tool batch). Default `false`
    /// — FAIL-CLOSED: a tool that has not declared itself read-only never
    /// joins the parallel pool, so a latent writer can't slip in. Writer
    /// tools and `exec` (even `cat`) stay `false`.
    fn is_read_only(&self) -> bool {
        false
    }

    /// G3 (devtool-upgrade 阶段 6): whether this tool may join the U5
    /// parallel pre-execution pool. Default = [`Self::is_read_only`] —
    /// pure reads stay the only automatic joiners. Override to `true` only
    /// for tools whose concurrent execution is safe by construction: the
    /// spawn tool opts in because (a) every pool dispatch injects the
    /// instance's sub-agent depth via `handle_tool_call_at_depth` (no-op
    /// for tools that don't override `set_invocation_depth`), (b) G0's own
    /// semaphore caps real concurrent sub-agents (`agents.subagent.
    /// max_concurrent`) on top of the pool's 4-permit limiter, (c) the
    /// full dispatch waterfall (estop/hidden/Plan/security/hooks) still
    /// runs inside each `handle_tool_call_at_depth` call, and (d) detached
    /// sub-agents own isolated instances/sessions — no shared mutable
    /// state with sibling spawns. Writers must NOT opt in: a `false` here
    /// keeps the whole batch serial (fail-closed, same as U5).
    fn is_parallel_safe(&self) -> bool {
        self.is_read_only()
    }
}

// ---------------------------------------------------------------------------
// Internal channel detection
// ---------------------------------------------------------------------------

/// Check if a channel is internal (not user-facing).
pub fn is_internal_channel(channel: &str) -> bool {
    matches!(channel, "cli" | "system" | "subagent")
}

// ---------------------------------------------------------------------------
// Session busy state management
// ---------------------------------------------------------------------------

/// Busy message returned when session is busy.
pub const BUSY_MESSAGE: &str =
    "\u{23f3} AI is processing a previous request, please try again later";

/// Concurrent request handling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConcurrentMode {
    /// Reject new messages when session is busy (default; legacy behavior).
    #[default]
    Reject,
    /// Queue messages when session is busy — processed after the current turn.
    Queue,
    /// Queue + steer: `!`-prefixed messages are injected into the RUNNING
    /// turn before its next LLM call (I1 / U7).
    Steer,
}

/// V5 (2026-08-23): outcome of the synchronous inbound gate (`gate_inbound`).
#[derive(Debug)]
enum GateOutcome {
    /// `cluster_continuation` marker — the pump handles it inline via
    /// `dispatch_continuation` (serial, unchanged from the legacy loop).
    Continuation(String),
    /// Short-circuit reply (busy receipt / busy bounce / queue-full / slash
    /// command response). No session was acquired.
    Immediate { agent_id: String, response: String },
    /// E6 (2026-09-05): `/compact` / `/clear` — 会话维护命令。gate 的同步
    /// 短路点拿不到 instance（LLM 摘要是 async），但会话已在此获取（防
    /// 并发回合与压缩互相踩摘要推进）；pump/serial tail 派发到
    /// `handle_maintenance`，回执经 `finish_message` 走正常出站路径。
    Maintenance {
        kind: SessionMaintenance,
        session_key: String,
        receipt: String,
    },
    /// K4 (devtool-upgrade 阶段 7): 用户直发远程编码任务派发——
    /// `/build <task> repo:<node>`。会话已在 gate 获取（Maintenance 同款，
    /// 防并发回合踩会话）；tail 走 `process_user_dispatch`：经
    /// `handle_tool_call` 全管线（安全 8 层 / estop / Plan 闸）提交
    /// `cluster_rpc`，ACK 后存自足续行快照，完成回调复用
    /// `handle_cluster_continuation` 呈现结果并回原通道。
    UserDispatch {
        task_text: String,
        node_id: String,
        session_key: String,
    },
    /// System (non-continuation) / history-request passthrough — async
    /// handling, no session semantics.
    Ungated,
    /// Normal chat message: the session is already acquired and the cancel
    /// token minted. The tail (`process_admitted`) owns release + drain.
    Admitted(TurnAdmission),
}

/// V5: admission minted by the gate — everything the turn tail needs that
/// the gate acquired on the message's behalf.
#[derive(Debug)]
struct TurnAdmission {
    agent_id: String,
    session_key: String,
    cancel_token: tokio_util::sync::CancellationToken,
    /// E3：本消息在 `turn_preamble` 里 begin 的 checkpoint turn 序号（无
    /// store 挂载时 None）。随 admission 穿针到行落盘点——不能落盘时反查
    /// `cur`（整 turn 的 await 间隙里可能被其他会话/steer 的 begin 翻掉）。
    cp_turn: Option<usize>,
}

/// E6: 会话维护命令种类（`/compact` 手动压缩 / `/clear` 清空会话）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMaintenance {
    /// 手动 compaction：推进摘要覆盖、收缩逐字尾巴（`force_compression`）。
    Compact,
    /// 清空会话历史（chat_log 截断 + SessionStore 清除，key 保留可用）。
    Clear,
}

/// E6: parse a session-maintenance slash command (`/compact` / `/clear`).
/// Returns (kind, pending receipt). Extra args are ignored (no-arg commands);
/// `None` for anything else — the caller falls through to normal handling.
fn parse_maintenance_command(content: &str) -> Option<(SessionMaintenance, String)> {
    let name = content.split_whitespace().next()?;
    match name {
        "/compact" => Some((SessionMaintenance::Compact, "⏳ 正在压缩会话…".to_string())),
        "/clear" => Some((SessionMaintenance::Clear, "⏳ 正在清空会话…".to_string())),
        _ => None,
    }
}

/// K4 (devtool-upgrade 阶段 7): 用户直发远程编码任务派发语法——
/// `/build <task> repo:<node>`（`/plan` 前缀同接受）。
///
/// 解析规则（可预测优先）：
/// - 前缀必须是 `/build ` 或 `/plan `（带参数形态）；裸 `/build`（F1 模式
///   切换）因无尾随空格天然不匹配，互不干扰；
/// - 目标节点 = **最后一个** `repo:` 前缀的空白分隔 token；其余部分为
///   任务文本；
/// - 任务文本为空 / 无 `repo:` token / token 后为空 → `None`（落普通轮，
///   不猜不做）。
///
/// 返回 (task_text, node_id)。
fn parse_user_dispatch(content: &str) -> Option<(String, String)> {
    let trimmed = content.trim();
    let rest = trimmed
        .strip_prefix("/build ")
        .or_else(|| trimmed.strip_prefix("/plan "))?
        .trim();
    if rest.is_empty() {
        return None;
    }
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let last = tokens.last()?;
    let node_id = last.strip_prefix("repo:").filter(|s| !s.is_empty())?;
    let task_text = tokens[..tokens.len() - 1].join(" ");
    if task_text.is_empty() {
        return None;
    }
    Some((task_text, node_id.to_string()))
}

/// K4: 构造用户直派发的自足续行快照消息。
///
/// 与 LLM 发起 `cluster_rpc` 的 `__ASYNC__` 路径同构，但快照不来自
/// instance 历史（用户派发没有 LLM 轮），而是直接合成最小合法序列：
/// `[user(任务文本), assistant(tool_calls: [同一 tc_id])]`。完成回调到达时
/// `merge_real_tool_result` 找不到既有 tool 槽位 → push 到末尾，得到
/// `[user, assistant(tc), tool(result)]` ——严格 provider 接受的标准序列
/// （tool 消息紧跟其 assistant tool_calls 消息）。续行 LLM 以自己的口吻
/// 向用户呈现远程执行结果，还可继续多步工具链。
fn build_dispatch_snapshot_messages(
    task_text: &str,
    target_node: &str,
    tool_call_id: &str,
) -> Vec<LlmMessage> {
    let args = serde_json::json!({
        "target": target_node,
        "message": task_text,
    })
    .to_string();
    vec![
        LlmMessage {
            role: "user".to_string(),
            content: task_text.to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        },
        LlmMessage {
            role: "assistant".to_string(),
            content: String::new(),
            tool_calls: Some(vec![ToolCallInfo {
                id: tool_call_id.to_string(),
                name: "cluster_rpc".to_string(),
                arguments: args,
            }]),
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        },
    ]
}

/// K4 (c): B 端 peer_chat 任务完成回报的变化摘要段。
///
/// B 端执行节点处理 `cluster_rpc:` 会话的最后一轮时，把本轮声明式文件
/// 工具变更以紧凑列表追加到最终回复（chat_log 落盘与出站发布共用同一
/// 字符串，日志与通道看到的完全一致）。上限 [`CHANGES_SUMMARY_CAP`] 条，
/// 溢出诚实注记（不静默截断）；顺序 = drain 顺序（确定性，重放字节稳定）。
/// 诚实边界：只覆盖**最终轮**——B 端多轮续行的历史轮变更不聚合（M3 会话
/// 级聚合是远期项）。
const CHANGES_SUMMARY_CAP: usize = 20;

fn render_turn_changes_summary(changes: &[FileChange]) -> Option<String> {
    if changes.is_empty() {
        return None;
    }
    let kind_str = |k: FileChangeKind| match k {
        FileChangeKind::Create => "create",
        FileChangeKind::Modify => "modify",
        FileChangeKind::Delete => "delete",
    };
    let mut s = format!("\n\n---\n📁 变更文件（本轮，共 {} 个）：", changes.len());
    for c in changes.iter().take(CHANGES_SUMMARY_CAP) {
        s.push_str(&format!("\n- {} ({})", c.path, kind_str(c.kind)));
    }
    if changes.len() > CHANGES_SUMMARY_CAP {
        s.push_str(&format!(
            "\n- …另有 {} 个文件未列出",
            changes.len() - CHANGES_SUMMARY_CAP
        ));
    }
    Some(s)
}

/// K4 (b): IM 审批卡回执语法——`/approve <id>` / `/deny <id>`（id 为审批
/// 卡给出的短编号，6~12 位十六进制字符）。命中返回静态确认文案（gate
/// Immediate 短路，不进 agent）；格式不符返回 None 落普通轮。真实裁决与
/// 无效 id 的诚实提示由 gateway 审批回执 watcher 负责。
fn parse_approval_reply(content: &str) -> Option<String> {
    let trimmed = content.trim();
    let (verb, id) = if let Some(rest) = trimmed.strip_prefix("/approve ") {
        ("approve", rest.trim())
    } else if let Some(rest) = trimmed.strip_prefix("/deny ") {
        ("deny", rest.trim())
    } else {
        return None;
    };
    let id = id.trim();
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(match verb {
        "approve" => format!("✓ 已收到审批回复（同意 {id}），结果另行通知。"),
        _ => format!("✓ 已收到审批回复（拒绝 {id}），结果另行通知。"),
    })
}

/// E3（devtool-upgrade 阶段 5）：每会话 undo 栈深度上限。超限丢最旧——
/// 内存态栈只服务本进程内的连续 undo/redo，不设界会话泄漏无界内存。
const REWIND_UNDO_STACK_CAP: usize = 8;

/// E3：一条消息级回退的 redo 依据（`rewind_to_message` 压栈，`redo_rewind`
/// 弹栈反向）。被截断的 jsonl 行 VERBATIM 随行保存（原时间戳/标记不动），
/// 影子 tree 支撑文件态的前向恢复与陈旧性守卫。
#[derive(Debug, Clone)]
struct RewindUndoEntry {
    /// 被 `rewind_to_message` 截掉的行（原样，`rows[cut..]`）。
    removed_rows: Vec<serde_json::Value>,
    /// 回退后剩余行数（= cut）。redo 守卫：期间该会话无任何新行。
    kept_count: usize,
    /// 截断点之后第一个 checkpoint turn（恢复/截断索引用的那个）。
    restore_turn: Option<usize>,
    /// undo 时刻的最新影子 tree——redo 的文件前向恢复目标。
    forward_tree: Option<String>,
    /// undo 完成后（`truncate_from` 之后的索引里）剩的最新 tree——redo 的
    /// 陈旧性守卫基线（期间任何新 checkpoint turn 都会改变它）。
    post_tree: Option<String>,
}

// -------------------------------------------------------------------------
// K3（devtool-upgrade 阶段 4）：`` !`cmd` `` shell 注入
// -------------------------------------------------------------------------

/// K3：`` !`cmd` `` 注入的超时（秒）——模板注入是"顺手拿点上下文"，不是
/// 长任务入口；卡死命令 3s 收尸。
const SHELL_INJECTION_TIMEOUT_SECS: u64 = 3;
/// K3：注入 stdout 的字符上限（超出按字符边界截断——byte 切片会在多字节
/// 字符上 panic，见 str-slice 教训）。
const SHELL_INJECTION_OUTPUT_CAP: usize = 4096;

/// K3：扫描文本中所有 `` !`cmd` `` token，返回（token 全长 span, 命令原文
/// trim 后）。扫描按 byte 找 `!` + `` ` ``（都是单字节 ASCII，不会落在多字
/// 节序列内部，切片安全）；无闭合反引号的 `!` 按字面保留（不进列表）。
fn shell_injection_spans(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'!'
            && bytes[i + 1] == b'`'
            && let Some(j) = text[i + 2..].find('`')
        {
            let end = i + 2 + j + 1;
            out.push((i..end, text[i + 2..end - 1].trim()));
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

/// K3：提取去重后的注入命令清单（首次出现序）。
fn extract_shell_injections(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, cmd) in shell_injection_spans(text) {
        if !cmd.is_empty() && !out.iter().any(|c| c == cmd) {
            out.push(cmd.to_string());
        }
    }
    out
}

/// K3：把每个 `` !`cmd` `` token 替换为 `resolve(cmd)` 的产物（纯函数，
/// 测试对象）。空命令 token 与未闭合 token 保持字面不动（与
/// [`extract_shell_injections`] 的跳过语义对齐，两边永不分歧）。
fn substitute_shell_injections(text: &str, resolve: &dyn Fn(&str) -> String) -> String {
    let spans = shell_injection_spans(text);
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (span, cmd) in spans {
        out.push_str(&text[last..span.start]);
        if cmd.is_empty() {
            out.push_str(&text[span.start..span.end]);
        } else {
            out.push_str(&resolve(cmd));
        }
        last = span.end;
    }
    out.push_str(&text[last..]);
    out
}

/// K3：执行单个注入命令并产出替换文本。成功（exit 0）→ stdout 去尾随空白
/// 并截断（空输出给 `(no output)` 占位，让 LLM/模板有确定内容）；spawn 失败、
/// 超时或非零退出 → `[command failed: <cmd>: <原因>]` 注记（stderr 尾部
/// 随注记带上，≤200 字符，便于人诊断）。
async fn exec_shell_injection(cmd: &str, cwd: Option<&std::path::Path>) -> String {
    let (code, stdout, stderr, timed_out) =
        crate::loop_tools::run_one_stage(cmd, cwd, SHELL_INJECTION_TIMEOUT_SECS).await;
    if timed_out {
        return format!("[command failed: {cmd}: timed out after {SHELL_INJECTION_TIMEOUT_SECS}s]");
    }
    match code {
        Some(0) => {
            let trimmed = stdout.trim_end();
            if trimmed.is_empty() {
                "(no output)".to_string()
            } else {
                cap_injection_output(trimmed)
            }
        }
        Some(c) => {
            let reason = tail_line(&stderr, 200);
            if reason.is_empty() {
                format!("[command failed: {cmd}: exit {c}]")
            } else {
                format!("[command failed: {cmd}: exit {c}: {reason}]")
            }
        }
        None => {
            let reason = tail_line(&stderr, 200);
            format!("[command failed: {cmd}: {reason}]")
        }
    }
}

/// K3：注入输出按字符数截断（头 4096 字符 + 截断注记）。
fn cap_injection_output(s: &str) -> String {
    if s.chars().count() <= SHELL_INJECTION_OUTPUT_CAP {
        return s.to_string();
    }
    let mut out: String = s.chars().take(SHELL_INJECTION_OUTPUT_CAP).collect();
    out.push_str("…[truncated]");
    out
}

/// K3：取 stderr 的最后一个非空行并按字符数截断（注记保持单行可读）。
fn tail_line(s: &str, cap: usize) -> String {
    let line = s.lines().rev().map(str::trim).find(|l| !l.is_empty());
    let line = line.unwrap_or("");
    if line.chars().count() <= cap {
        line.to_string()
    } else {
        let mut out: String = line.chars().take(cap).collect();
        out.push('…');
        out
    }
}

/// Parse the config string. Unknown values fall back to Reject (fail-safe
/// to legacy behavior) with a warn at the call site.
pub fn parse_concurrent_mode(s: &str) -> ConcurrentMode {
    match s.trim().to_lowercase().as_str() {
        "queue" => ConcurrentMode::Queue,
        "steer" => ConcurrentMode::Steer,
        _ => ConcurrentMode::Reject,
    }
}

/// Tracks busy state for sessions.
pub struct SessionBusyTracker {
    busy: dashmap::DashSet<String>,
    #[allow(dead_code)] // Reserved for future concurrent-mode-aware queue logic
    mode: ConcurrentMode,
    #[allow(dead_code)] // Reserved for future concurrent-mode-aware queue logic
    queue_size: usize,
}

impl SessionBusyTracker {
    /// Create a new tracker with the given mode.
    pub fn new(mode: ConcurrentMode, queue_size: usize) -> Self {
        Self {
            busy: dashmap::DashSet::new(),
            mode,
            queue_size,
        }
    }

    /// Try to acquire a session for processing. Returns false if busy and mode is Reject.
    pub fn try_acquire(&self, session_key: &str) -> bool {
        if self.busy.contains(session_key) {
            return false;
        }
        self.busy.insert(session_key.to_string());
        true
    }

    /// Release a session after processing.
    pub fn release(&self, session_key: &str) {
        self.busy.remove(session_key);
    }

    /// Check whether a session is currently busy.
    pub fn is_busy(&self, session_key: &str) -> bool {
        self.busy.contains(session_key)
    }
}

// ---------------------------------------------------------------------------
// ProcessOptions -- options for how a message is processed
// ---------------------------------------------------------------------------

/// Configuration for how a message is processed through the agent loop.
#[derive(Debug, Clone)]
pub struct ProcessOptions {
    /// Session identifier for history/context.
    pub session_key: String,
    /// Target channel for tool execution.
    pub channel: String,
    /// Target chat ID for tool execution.
    pub chat_id: String,
    /// User message content.
    pub user_message: String,
    /// Response when LLM returns empty.
    pub default_response: String,
    /// Whether to trigger summarization.
    pub enable_summary: bool,
    /// Whether to send response via bus.
    pub send_response: bool,
    /// If true, don't load session history (for heartbeat).
    pub no_history: bool,
    /// Trace ID for observer events.
    pub trace_id: String,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            session_key: String::new(),
            channel: String::new(),
            chat_id: String::new(),
            user_message: String::new(),
            default_response: "I've completed processing but have no response to give.".to_string(),
            enable_summary: true,
            send_response: false,
            no_history: false,
            trace_id: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-session busy state with queue length
// ---------------------------------------------------------------------------

/// Per-session busy state with queue length.
#[derive(Debug, Clone, Default)]
struct SessionBusyState {
    busy: bool,
    queue_length: usize,
}

// ---------------------------------------------------------------------------
// MessageTool sent-in-round tracking (mirrors Go's MessageTool.HasSentInRound)
// ---------------------------------------------------------------------------

/// Tracks whether a message has already been sent in the current LLM round.
/// This prevents double-sending when the agent loop also publishes outbound.
#[derive(Debug, Default)]
struct SentInRoundTracker {
    /// session_key -> whether a tool already sent a message this round.
    sent: parking_lot::Mutex<std::collections::HashSet<String>>,
}

impl SentInRoundTracker {
    fn new() -> Self {
        Self::default()
    }

    /// Mark that a message was sent for the given session key this round.
    fn mark_sent(&self, session_key: &str) {
        self.sent.lock().insert(session_key.to_string());
    }

    /// Check if a message was already sent for the given session key.
    fn has_sent_in_round(&self, session_key: &str) -> bool {
        self.sent.lock().contains(session_key)
    }

    /// Clear the sent flag for a session (start of new round).
    fn clear(&self, session_key: &str) {
        self.sent.lock().remove(session_key);
    }

    /// Clear all sent flags.
    #[allow(dead_code)]
    fn clear_all(&self) {
        self.sent.lock().clear();
    }
}

// ---------------------------------------------------------------------------
// AgentLoop -- core execution engine
// ---------------------------------------------------------------------------

/// The core agent execution loop.
///
/// In standalone mode, this wraps a single LLM provider, tool registry,
/// and agent config. In bus-integrated mode, it additionally owns a
/// registry of agent instances, a message bus adapter, summarizer,
/// and session busy tracker.
///
/// U5 (sixth batch): one parallel-executed read-only call's result, captured
/// for serial guard replay in source order. `validation_failed` lets the
/// serial loop replay the `validation_failures` counter exactly as the serial
/// path would (Invalid → +1; Valid/Fixed → reset to 0).
struct PrecomputedTool {
    result: String,
    validation_failed: bool,
    duration_ms: u64,
}

/// G0 (devtool-upgrade 阶段 3)：`run_detached` 选项。
///
/// 构造用 `Default`（全空 = 全继承主 loop）。语义详见
/// [`AgentLoop::run_detached`]。
#[derive(Default)]
pub struct DetachedOpts<'a> {
    /// 子代理可用工具白名单（工具名集合）。`None` / 空 = 继承主 loop
    /// 全量供给（仍经 tier 过滤）。由 `effective_tool_defs` 消费。
    pub allowed_tools: Option<&'a [&'a str]>,
    /// 嵌套深度（0 = 顶层 spawn）。G2 消费两处：`run_detached` 把它写到
    /// instance（`detached_depth`），SpawnTool 层执行
    /// `agents.subagent.max_depth` 深度限制（`parent+1 > max_depth` 拒绝）；
    /// G4 后台化续行时同值透传。
    pub depth: usize,
    /// 模型覆盖。**v1 边界：暂未实现切换**——子代理沿用主 loop 当前
    /// active model（provider 选择在 run_llm_loop 深处，覆盖需独立
    /// 通道；字段先占位，实现留 G4/后续）。
    pub model: Option<String>,
    /// 工具轮预算（>0 时作为 `turn_budget` 传入，REPLACE
    /// `config.max_turns`；0 = 用主配置默认）。
    pub max_turns: u32,
    /// Swarm M1（裸提示词模式）：替换主 loop 人格 system_prompt（本回合
    /// 专用）。None = 继承主 loop 人格。供 board planner / 验收 / 主持人等
    /// 结构化子系统使用——它们需要无人格污染的纯净调用，不是"另一个
    /// 人格的 agent"。
    pub system_prompt: Option<&'a str>,
    /// Swarm M1：零工具供给。true = 本回合不向模型暴露任何工具定义
    /// （`effective_tool_defs` 直返空）；false = 正常供给链
    /// （tier → 白名单 → hidden）。与 max_turns=1 组合即纯文本单轮调用。
    pub no_tools: bool,
    /// Swarm M1：会话标签，注入 session_key（`subagent:{label}:{uuid}`）与
    /// trace_id，供日志检索与可观测（G13）。None = 维持 `subagent:{uuid}`。
    pub label: Option<&'a str>,
}

pub struct AgentLoop {
    // --- Standalone fields (always present) ---
    /// LLM provider for generating responses.
    /// Wrapped in `RwLock<Arc<...>>` for runtime provider swapping (model switch).
    /// Spawned tasks clone the Arc (cheap), so in-flight requests finish with the
    /// old provider while new requests use the updated one.
    provider: parking_lot::RwLock<Arc<dyn LlmProvider>>,
    /// Active model name, kept in sync with the provider above.
    /// Separated from `config.model` so runtime swaps don't need `&mut self`.
    active_model: parking_lot::RwLock<String>,
    /// Tool registry: name -> tool implementation.
    /// Each tool is wrapped in `Arc` so the map can be cloned and shared
    /// with spawned tasks without requiring `Box` clone support.
    /// Wrapped in `RwLock` for interior mutability — MCP hot-reload needs
    /// to register new tools from `&self` methods (inside the run loop).
    tools: parking_lot::RwLock<HashMap<String, Arc<dyn Tool>>>,
    /// Agent configuration.
    config: AgentConfig,

    // --- Bus-integrated fields (optional) ---
    /// Outbound message sender for bus mode.
    outbound_tx: Option<tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>>,
    /// Agent registry for multi-agent routing.
    registry: Option<Arc<AgentRegistry>>,
    /// State manager for recording last channel/chat ID (persistent on disk).
    state_manager: Option<Arc<nemesis_state::workspace_state::WorkspaceStateManager>>,
    /// Session store for persistent history.
    session_store: Option<Arc<SessionStore>>,
    /// Running flag for the bus consumption loop.
    running: AtomicBool,
    /// Per-session busy state with queue length tracking.
    session_busy: parking_lot::Mutex<HashMap<String, SessionBusyState>>,
    /// Session busy check: mode-aware I1 (U7).
    /// Concurrent request handling mode.
    concurrent_mode: ConcurrentMode,
    /// Re-injection sender into the agent's own inbound mpsc (round-5 review
    /// fix). The queue-drain path uses it to hand the queued head back to the
    /// normal `run_bus_*` consumer instead of recursing inline — so the reply
    /// gets the SAME post-processing as any other message (rpc correlation
    /// prefix, sent_in_round check+clear, error→"Error processing message"
    /// conversion + capture flush, meta.model). Deliberately NOT the bus
    /// broadcast: re-publishing on the bus would re-match workflow
    /// message-triggers (double firing). `None` in standalone mode (no drain
    /// consumer exists) and until the adapter wires it.
    reinject_tx: parking_lot::RwLock<
        Option<tokio::sync::mpsc::Sender<nemesis_types::channel::InboundMessage>>,
    >,
    /// Configured queue size for queue mode. Stored for config/logging parity
    /// but NOT read: busy-queueing lives in `crate::inbox` (capacity-bounded
    /// FIFO per session), not in the session_busy map (see
    /// `try_start_session`'s comment for why the old counter path was
    /// removed). Remove this field (+ the `new_bus` param + call sites) if
    /// queue mode is permanently retired.
    #[allow(dead_code)]
    queue_size: usize,
    /// Maximum concurrent cluster continuation tasks.
    /// 0 = inline execution in the main loop (no spawn, serialized).
    /// >0 = spawn with semaphore-controlled concurrency.
    max_continuation_permits: usize,
    /// Semaphore for limiting concurrent continuation spawns.
    /// `None` when `max_continuation_permits == 0` (inline mode).
    continuation_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// Tracks which sessions are currently being summarized.
    /// Wrapped in `Arc` so the flag can be cleared from a spawned task
    /// after summarization completes (mirrors Go's `defer al.summarizing.Delete()`).
    summarizing: Arc<parking_lot::Mutex<HashMap<String, bool>>>,
    /// ⑩ Per-session compaction state for graded tiers (soft/summarize) and
    /// stuck self-check. See `maybe_summarize`.
    compact_state: Arc<parking_lot::Mutex<HashMap<String, CompactState>>>,
    /// Channel manager reference (for channel listing commands).
    channel_manager_channels: parking_lot::Mutex<Vec<String>>,
    /// Tracks whether a message tool already sent a response this round.
    /// Mirrors Go's MessageTool.HasSentInRound() / alreadySent check.
    sent_in_round: SentInRoundTracker,
    /// Route resolver for multi-agent message routing.
    /// Mirrors Go's al.registry (RouteResolver). When set, process_inbound_message
    /// uses the full 7-level priority cascade instead of the default-agent fallback.
    route_resolver: Option<RouteResolver>,
    /// Optional observer event callback (mirrors Go's observerMgr).
    /// Called at conversation_start, conversation_end, llm_request, llm_response, tool_call.
    observer_callback: Option<Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>>,
    /// Continuation manager for cluster RPC async callbacks.
    continuation_manager: Option<Arc<crate::loop_continuation::ContinuationManager>>,
    /// Cluster reference for cross-node communication.
    /// Stored as `Arc<dyn Any + Send + Sync>` to avoid a circular dependency
    /// on the `nemesis-cluster` crate. The caller can downcast to the concrete
    /// cluster type. Mirrors Go's `AgentLoop.cluster`.
    cluster: Option<Arc<dyn std::any::Any + Send + Sync>>,
    /// Observer manager for Phase 5 event emission.
    /// Mirrors Go's `AgentLoop.observerMgr`.
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
    /// Security plugin for pre-execution tool safety checks.
    /// Mirrors Go's SecurityPlugin registered via PluginManager.
    #[cfg(feature = "security")]
    security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    security_plugin: Option<()>,
    /// MCP Manager for dynamic tool discovery and hot-reload.
    mcp_manager: Option<std::sync::Mutex<nemesis_mcp::manager::McpManager>>,
    /// Snapshot of registered MCP tool names and descriptions.
    /// Shared with McpListTool so it can list MCP tools without accessing the full tool registry.
    mcp_tool_snapshot: Arc<parking_lot::RwLock<Vec<(String, String)>>>,
    /// Optional data store for recording LLM usage statistics.
    data_store: Option<Arc<nemesis_data::DataStore>>,
    /// Forge instance for experience collection during tool execution.
    #[cfg(feature = "forge")]
    forge: Option<Arc<nemesis_forge::forge::Forge>>,
    #[cfg(not(feature = "forge"))]
    #[allow(dead_code)] // placeholder when forge feature is off
    forge: Option<()>,
    /// Per-session cancellation tokens. When a user requests cancellation,
    /// the token for the corresponding session is cancelled, causing the
    /// LLM loop to break at the next check point.
    cancel_tokens: dashmap::DashMap<String, tokio_util::sync::CancellationToken>,
    /// Checkpoint store for the edit safety net. When attached, every writer
    /// tool call snapshots the file's pre-edit content before execution, so a
    /// rewind can restore it. RwLock so it can be attached from `&self` (the
    /// gateway sets it after construction).
    checkpoint_store: parking_lot::RwLock<Option<Arc<crate::checkpoint::CheckpointStore>>>,
    /// Monotonic turn counter for checkpoints (one per inbound message). Global
    /// across sessions in this MVP — adequate for single-session deployments;
    /// multi-session isolation is a documented follow-up.
    turn_counter: std::sync::atomic::AtomicUsize,
    /// D3（devtool-upgrade 阶段 5）：本 turn 声明式文件工具的变更流水，按
    /// session_key 分桶。dispatch 瀑布（`preview_all` 处）顺带收集（独立于
    /// checkpoint 是否挂载）；`run_agent_loop_internal` 在 assistant 最终
    /// 回复落盘时 drain+去重，写进 chat_log jsonl 行的 `file_changes` 字段
    /// （消息↔文件变更映射；M3 会话级 diff 查看器的数据源）。Arc 外壳：
    /// dispatch 瀑布闭包是 `Fn`（'static），须经 Arc 捕获共享。清理时点 =
    /// drain（写后即清）+ turn 开始兜底清（上轮异常短路未走到落盘也不残留
    /// 到本轮）；steer 注入不重入 `run_agent_loop_internal`，正在跑的 turn
    /// 缓冲不受影响。
    turn_file_changes: Arc<parking_lot::Mutex<HashMap<String, Vec<FileChange>>>>,
    /// E3（devtool-upgrade 阶段 5）：消息级回退的 undo 栈，按 session_key
    /// 分栈（会话间互不干扰，栈内严格 LIFO）。`rewind_to_message` 压栈，
    /// `redo_rewind` 弹栈反向。内存态（重启即失——redo 只对本次进程内的
    /// undo 有效，诚实边界）；每栈上限 [`REWIND_UNDO_STACK_CAP`]，超限丢
    /// 最旧。
    rewind_undo_stacks:
        parking_lot::Mutex<HashMap<String, std::collections::VecDeque<RewindUndoEntry>>>,
    /// K1a (U14): user tool hooks — pre runs after the fixed security gate,
    /// post runs after execute and before Forge. RwLock so hooks can be
    /// registered from `&self` post-construction (K2 hooks.json wiring).
    /// See `crate::hooks` module doc for the full 布点图.
    tool_hooks: parking_lot::RwLock<crate::hooks::ToolHookManager>,
    /// K1b (U14): LLM-call-level hooks — pre may append reminder messages
    /// (visible in request_log), post may allow/replace/retry/block the
    /// response. See `crate::hooks` module doc（LLM 调用级布点）.
    llm_hooks: parking_lot::RwLock<crate::hooks::LlmHookManager>,
    /// K2 (U14): prompt/turn lifecycle hooks — on_user_prompt runs in
    /// `run_with_trace` BEFORE the message enters history (blocked prompts
    /// are never seen by the model), on_turn_end runs after the final
    /// answer is accepted, before the turn ends. Primary consumer: the
    /// hooks.json dialect bridge (`crate::cc_hooks`).
    lifecycle_hooks: parking_lot::RwLock<crate::hooks::LifecycleHookManager>,
    /// Memory tool executor reference, so the gateway can attach an approval
    /// gate post-construction (memory_store/forget require interactive approval).
    #[cfg(feature = "memory")]
    memory_executor:
        parking_lot::RwLock<Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)] // placeholder when memory feature is off
    memory_executor: parking_lot::RwLock<Option<()>>,
    /// P3.1 (sixth batch): memory manager for the AUTO-INJECT channel —
    /// read-only retrieval (top-K vector search over the current user
    /// message) feeding the `# Memory Context` snapshot section. Deliberately
    /// SEPARATE from `memory_executor`: that one gates store/forget behind
    /// interactive approval; auto-inject is pure retrieval and must not trip
    /// the approval gate. `None` (default) disables injection entirely.
    #[cfg(feature = "memory")]
    memory_inject_manager: parking_lot::RwLock<Option<Arc<nemesis_memory::manager::MemoryManager>>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)] // placeholder when memory feature is off
    memory_inject_manager: parking_lot::RwLock<Option<()>>,
    /// P3.1: auto_inject flag + top_k loaded from config.enhanced_memory.json
    /// by the factory (`set_memory_inject`). Tuple so both values travel
    /// together on the one setter. Default (false, 3) = feature off.
    memory_inject_cfg: parking_lot::RwLock<(bool, usize)>,
    /// Capability tier (small-model-tool-robustness plan, Phase 4a). Resolved at
    /// construction from the active model's `model_tier` config (see
    /// [`nemesis_types::capability`]). Drives tool-set size (Phase 3),
    /// validation-retry budget (Phase 2), and format-repair gating (Phase 5).
    /// `RwLock` so it can be re-resolved if the active model switches at runtime.
    tier: parking_lot::RwLock<nemesis_types::capability::ModelTier>,
    /// F1（devtool-upgrade 阶段 4）：plan/build 工作模式。Build=默认全量；
    /// Plan=只读白名单供给（build_tool_defs 第三过滤层）+ 分发端写类拦截
    /// （`handle_tool_call_at_depth` 入口，防 MCP/未知写工具与陈旧 defs
    /// 漏网）+ plans/ 写放行。运行时态不持久化（重启回 Build）；
    /// `/plan` `/build` 与 WSAPI `chat.set_mode` 双入口改同一份状态。
    mode: parking_lot::RwLock<crate::types::AgentMode>,
    /// F1：M1a 事件广播发送端——模式切换后发布 `ModeChanged`（前端徽标
    /// 实时刷新；chat_id 缺省空 = 只进 SSE EventHub，不路由具体会话）。
    /// `None`（standalone / 未注入）→ 发布静默跳过（观察者通道空转是常态）。
    agent_event_tx: parking_lot::RwLock<
        Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
    >,
    /// Path to config.json — the single source of truth for per-model
    /// `model_tier`. `None` in standalone mode (no config.json to watch). Set
    /// by `agent_factory`; used by `refresh_active_tier` / `check_config_reload`
    /// so dashboard-added models and CLI `model set-tier` are picked up live,
    /// with no stale snapshot.
    config_path: parking_lot::RwLock<Option<std::path::PathBuf>>,
    /// N1（devtool-upgrade 阶段 1）：分层价目表（workspace/data）。三级
    /// context_window 解析链的第 L2 级——config 未显式配置 `context_window`
    /// 时按价目表 `max_input_tokens` 猜。`None`（未注入/打开失败）→ 直接落到
    /// L3 fallback（[`FALLBACK_CONTEXT_WINDOW`]）。
    pricing_store: parking_lot::RwLock<Option<std::sync::Arc<nemesis_data::PricingStore>>>,
    /// C3（devtool-upgrade 阶段 2）：共享 LspManager 单例（与 LspTool 同一
    /// 实例，见 SharedResources.lsp_manager / C5）。编辑后诊断回灌（修复
    /// 闭环）用它同步文档 + 等诊断。`None`（未注入 / standalone）→ 反馈
    /// 静默跳过。Set via `set_lsp_manager` by the agent factory.
    lsp_manager: parking_lot::RwLock<Option<Arc<nemesis_lsp::LspManager>>>,
    /// 自定义 slash 命令表路径（`config.commands.json`；主 agent 专用，集群
    /// agent 不接——命令不该跨节点复制，同 hooks 挂账决策）。
    /// 自定义命令表热重载器（HotReloader 统一收编，2026-08-29：原
    /// path/mtime/cache 三字段手写 mtime 模式收编为一行声明）。
    commands_hot:
        parking_lot::RwLock<Option<nemesis_config::HotReloader<nemesis_config::CommandsConfig>>>,
    /// hooks 方言桥（2026-08-29 T3）：PreCompact/PostCompact 触发用。
    /// SessionEnd 经 SessionEndHookManager（factory 清理点直接调桥）。
    cc_bridge: parking_lot::RwLock<Option<std::sync::Arc<crate::cc_hooks::CcHookBridge>>>,
    /// G4 (U4): root directory for tool-result spill files
    /// (`<home>/logs/spill`). `None` disables spilling (results fall back to
    /// the G3 prune tier). Set via `set_spill_root` by the agent factory.
    spill_root: parking_lot::RwLock<Option<std::path::PathBuf>>,
    /// H3 (P2.2): skills loader for the catalog digest. `None` disables the
    /// digest injection entirely. Set via `set_skills_loader`.
    skills_loader: parking_lot::RwLock<Option<Arc<nemesis_skills::loader::SkillsLoader>>>,
    /// H3 (P2.2): digest emission handle. Round-5: stateless under I2
    /// merged-snapshot semantics (sections re-render from disk every build;
    /// the old per-session hash map gated nothing and was removed — see
    /// skills_digest.rs module doc).
    skills_digest_state: std::sync::Arc<crate::skills_digest::DigestState>,
    /// I1 (U7): per-session message inbox for Queue/Steer modes. Unused in
    /// Reject mode (kept anyway — trivial cost, simplifies mode switching).
    inbox: crate::inbox::SharedInbox,
    /// V5 (2026-08-23): abort handles of the Queue/Steer pump's spawned turn
    /// tasks. The pump itself is aborted by the adapter on stop; without
    /// tracking, those spawned turns would be orphaned and keep publishing
    /// replies after the "stop". `stop()` aborts them (mirrors the serial
    /// pump where aborting the one task killed the in-flight turn). Reject
    /// mode never spawns — stays empty.
    turn_task_handles: parking_lot::Mutex<Vec<tokio::task::AbortHandle>>,
    /// H5 (U18): workspace root for the AGENTS.md/CLAUDE.md instruction
    /// chain. `None` disables the instructions section of the merged
    /// context digest.
    workspace_root: parking_lot::RwLock<Option<std::path::PathBuf>>,
    /// I1 (devtool-upgrade 阶段 3): fs-watcher keep-alive handle. `None` =
    /// not started / disabled / failed (warn-once). Callbacks hold a
    /// Weak<AgentLoop> (handle lives INSIDE the loop — Arc would cycle).
    fs_watcher: parking_lot::RwLock<Option<crate::fs_watcher::WatcherHandle>>,
    /// I1: externally-changed workspace files observed since the last
    /// build_messages drain (capped; one-shot — drained on next build).
    external_changes: parking_lot::Mutex<Vec<String>>,
    /// I1: agent-authored writes (write_file/edit_file) for the self-write
    /// window — watcher events for these paths within
    /// [`crate::fs_watcher::SELF_WRITE_WINDOW`] are dropped (the agent knows
    /// what it just wrote). Normalized workspace-relative lowercase.
    // I1: pub(crate) so sibling test modules (fs_watcher/tests.rs) can age
    // entries past the self-write window without sleeping.
    pub(crate) recent_self_writes:
        parking_lot::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    /// Full-review M4: context-snapshot message role ("user" default;
    /// "system" restores the pre-I2 shape for strict chat templates that
    /// reject adjacent user/user pairs).
    snapshot_role: parking_lot::RwLock<String>,
    /// X2 (U8 refinement): whether interactive approval (desktop popup
    /// adapter wired to the auditor by the gateway) is reachable. Rendered
    /// into the merged context snapshot's `# Runtime Policy` section. The
    /// guardian line reads the security plugin's live judge; the tier line
    /// reads the live capability tier — all three are state (no clocks), so
    /// the section renders deterministically: same state ⇒ same bytes.
    interactive_approval: parking_lot::RwLock<bool>,
    /// I5（devtool-upgrade 阶段 7）：当前轮客户端上报的「当前打开文件」路径
    /// （WSAPI `chat.send` data.open_files → metadata["open_files"] → bus 全链
    /// 路透传，解析走 nemesis-types::channel::open_files_from_metadata 单点）。
    /// Ephemeral per-turn：`process_admitted` 进轮前 set、出轮后 clear（词法
    /// 配对防泄漏——cron/heartbeat/continuation 等非 process_admitted 路径
    /// 天然读不到，无陈旧泄漏）。build_messages 渲染进 merged digest section
    /// （空 = 无 section，字节稳定）；digest 内容随既有 InjectionRecord 台账
    /// 落账 → 字节级回放免费一致。只存路径不读内容（annotation 语义）。
    pending_open_files: parking_lot::RwLock<Vec<String>>,
    /// Y1 (Phase4-a): per-tool description embedding cache (tool name →
    /// (description bytes, vector)) for semantic doc folding. Entries
    /// re-embed only when a tool's description text changes, so after the
    /// first round folding adds no embed calls beyond the query itself.
    /// Read only on the memory-feature path (the embed backend lives in
    /// nemesis-memory); `allow(dead_code)` keeps the no-default-features
    /// build warning-clean.
    #[cfg_attr(not(feature = "memory"), allow(dead_code))]
    tool_vec_cache: parking_lot::RwLock<std::collections::HashMap<String, (String, Vec<f32>)>>,
    /// Last-seen mtime of config.json; `check_config_reload` compares against
    /// this each round to detect on-disk changes without re-reading every turn.
    config_mtime: parking_lot::RwLock<Option<std::time::SystemTime>>,
    /// 全局急停状态（kill switch）。触发后，循环在每轮顶部 break、并在工具
    /// 分发前拒绝调用。`None`（standalone/测试）时永不阻塞 = 零行为变化。
    /// 以 `Option<Arc<...>>` 形态持有，工厂每次重建 loop 时从
    /// `SharedResources.estop` 重新绑定到**同一个** Arc——所以急停状态在
    /// agent 重启后自动保持。
    estop: parking_lot::RwLock<Option<Arc<crate::estop::EstopState>>>,
    /// N2 (devtool-upgrade 阶段 4)：小模型专职杂务通道（`agents.small_model`）。
    /// 手动 compact（E6 `/compact`）的摘要调用优先走它（省 token——摘要不需
    /// 要旗舰档智力）；未配置 = `None`，诚实回退主模型。自动压缩（质量敏感）
    /// 刻意不消费本字段，维持主模型。工厂在 loop 构造后从 config 解析装配；
    /// 运行期改配置需重启 Agent（与 lsp_tool 等启动期装配项同一约定）。
    small_model: parking_lot::RwLock<Option<SmallModelSlot>>,
    /// M7 (devtool-upgrade 阶段 5)：审批响应端（`ApprovalResponder`）。
    /// gateway 装配 WebApprovalManager 后挂在这里，WSAPI `approval.respond` /
    /// `approval.pending` 经 AppState 的 agent_loop 槽触达（不经 security 依赖）。
    /// 未装配 = `None`，approval handler 诚实报「未装配」。
    approval_responder:
        parking_lot::RwLock<Option<Arc<dyn nemesis_types::agent::ApprovalResponder>>>,
    /// F7 (devtool-upgrade 阶段 5)：结构化提问响应端（`QuestionResponder`）。
    /// gateway 装配 WebQuestionBroker 后挂在这里，WSAPI `question.respond` /
    /// `question.pending` 经 AppState 的 agent_loop 槽触达（同审批先例）。
    /// 未装配 = `None`，question handler 诚实报「未装配」。
    question_responder:
        parking_lot::RwLock<Option<Arc<dyn nemesis_types::agent::QuestionResponder>>>,
    /// J5 (devtool-upgrade 阶段 6)：doom-loop 审批卡的提问发起端
    /// （`QuestionAsker`）——gateway 注入与 F7 responder 同源的
    /// WebQuestionBroker Arc 的另一半 trait。escalation 触发且
    /// `agents.doom_loop_approval` 开时经此发卡问用户「继续吗？」。
    /// 未装配 = `None` = 审批通路缺失，escalation 直接走现行为（停轮）。
    question_asker: parking_lot::RwLock<Option<Arc<dyn nemesis_types::agent::QuestionAsker>>>,
}

/// N2: 小模型槽位——provider 与其模型名绑在一起换，避免两者失配。
#[derive(Clone)]
struct SmallModelSlot {
    provider: Arc<dyn LlmProvider>,
    name: String,
}

/// E7: 自动标题的输入上限（首条 user 消息截断，计划原文 ≤500 字）。
pub(crate) const E7_TITLE_INPUT_MAX_CHARS: usize = 500;
/// E7: 自动标题的长度上限（计划原文 ≤24 字）。
pub(crate) const E7_TITLE_MAX_CHARS: usize = 24;

/// E7: 一次性 LLM 标题生成（无工具、非流式）。失败/空输出 → None（调用
/// 方诚实跳过，下轮回复再试）。
pub(crate) async fn generate_title_from_first_message(
    provider: &dyn LlmProvider,
    model: &str,
    first_user: &str,
) -> Option<String> {
    let prompt = format!(
        "根据以下用户请求，生成一个不超过{}字的会话标题。直接输出标题本身：不要引号、不要解释、不要换行。\n\n用户请求：{}",
        E7_TITLE_MAX_CHARS, first_user
    );
    let resp = provider
        .chat(
            model,
            vec![LlmMessage {
                role: "user".to_string(),
                content: prompt,
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
                images: Vec::new(),
            }],
            None,
            Vec::new(),
        )
        .await
        .ok()?;
    sanitize_generated_title(&resp.content)
}

/// E7: 标题清洗——取首行、剥首尾引号/反引号/空白、截到 [`E7_TITLE_MAX_CHARS`]
/// 字符（char 边界安全，CJK 不劈半）。清洗后为空 → None。
pub(crate) fn sanitize_generated_title(raw: &str) -> Option<String> {
    let first_line = raw.lines().next().unwrap_or("").trim();
    let unquoted = first_line.trim_matches(|c| {
        matches!(
            c,
            '"' | '\'' | '`' | '\u{201c}' | '\u{201d}' | '\u{300c}' | '\u{300d}'
        )
    });
    let cleaned = unquoted.trim();
    if cleaned.is_empty() {
        return None;
    }
    Some(cleaned.chars().take(E7_TITLE_MAX_CHARS).collect())
}

impl AgentLoop {
    /// Switch the active model by alias (resolved via `config.models`) or literal
    /// model id. Returns the resolved model id. Unknown aliases are used as-is
    /// (so `/model deepseek-v4-pro` works even without an alias entry).
    pub fn set_active_model(&self, alias_or_model: &str) -> String {
        let model = self
            .config
            .models
            .get(alias_or_model)
            .cloned()
            .unwrap_or_else(|| alias_or_model.to_string());
        *self.active_model.write() = model.clone();
        info!(
            "[AgentLoop] Active model set to {} (via '{}')",
            model, alias_or_model
        );
        // Phase 4a: re-resolve capability tier for the new model.
        self.refresh_active_tier();
        model
    }

    /// Available model aliases (from config.models), for `/model` listing.
    pub fn model_aliases(&self) -> Vec<String> {
        self.config.models.keys().cloned().collect()
    }
    /// Create a new agent loop with the given provider and configuration (standalone mode).
    pub fn new(provider: Box<dyn LlmProvider>, config: AgentConfig) -> Self {
        let model = config.model.clone();
        info!("[AgentLoop] Created in standalone mode, model={}", model);
        Self {
            provider: parking_lot::RwLock::new(Arc::from(provider)),
            active_model: parking_lot::RwLock::new(config.model.clone()),
            tools: parking_lot::RwLock::new(HashMap::new()),
            config,
            outbound_tx: None,
            registry: None,
            state_manager: None,
            session_store: None,
            running: AtomicBool::new(false),
            session_busy: parking_lot::Mutex::new(HashMap::new()),
            concurrent_mode: ConcurrentMode::Reject,
            reinject_tx: parking_lot::RwLock::new(None),
            queue_size: crate::inbox::DEFAULT_QUEUE_SIZE,
            max_continuation_permits: 0,
            continuation_semaphore: None,
            summarizing: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            compact_state: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            channel_manager_channels: parking_lot::Mutex::new(Vec::new()),
            sent_in_round: SentInRoundTracker::new(),
            route_resolver: None,
            observer_callback: None,
            continuation_manager: None,
            cluster: None,
            observer_manager: None,
            security_plugin: None,
            mcp_manager: None,
            mcp_tool_snapshot: Arc::new(parking_lot::RwLock::new(Vec::new())),
            data_store: None,
            forge: None,
            cancel_tokens: dashmap::DashMap::new(),
            checkpoint_store: parking_lot::RwLock::new(None),
            turn_counter: std::sync::atomic::AtomicUsize::new(0),
            turn_file_changes: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            rewind_undo_stacks: parking_lot::Mutex::new(HashMap::new()),
            tool_hooks: parking_lot::RwLock::new(crate::hooks::ToolHookManager::new()),
            llm_hooks: parking_lot::RwLock::new(crate::hooks::LlmHookManager::new()),
            lifecycle_hooks: parking_lot::RwLock::new(crate::hooks::LifecycleHookManager::new()),
            memory_executor: parking_lot::RwLock::new(None),
            #[cfg(feature = "memory")]
            memory_inject_manager: parking_lot::RwLock::new(None),
            #[cfg(not(feature = "memory"))]
            memory_inject_manager: parking_lot::RwLock::new(None),
            memory_inject_cfg: parking_lot::RwLock::new((false, 3)),
            tier: parking_lot::RwLock::new(nemesis_types::capability::ModelTier::Big),
            mode: parking_lot::RwLock::new(crate::types::AgentMode::Build),
            agent_event_tx: parking_lot::RwLock::new(None),
            config_path: parking_lot::RwLock::new(None),
            pricing_store: parking_lot::RwLock::new(None),
            lsp_manager: parking_lot::RwLock::new(None),
            commands_hot: parking_lot::RwLock::new(None),
            cc_bridge: parking_lot::RwLock::new(None),
            spill_root: parking_lot::RwLock::new(None),
            skills_loader: parking_lot::RwLock::new(None),
            skills_digest_state: std::sync::Arc::new(crate::skills_digest::DigestState::new()),
            inbox: std::sync::Arc::new(crate::inbox::Inbox::new(crate::inbox::DEFAULT_QUEUE_SIZE)),
            turn_task_handles: parking_lot::Mutex::new(Vec::new()),
            workspace_root: parking_lot::RwLock::new(None),
            fs_watcher: parking_lot::RwLock::new(None),
            external_changes: parking_lot::Mutex::new(Vec::new()),
            recent_self_writes: parking_lot::Mutex::new(std::collections::HashMap::new()),
            snapshot_role: parking_lot::RwLock::new("user".to_string()),
            interactive_approval: parking_lot::RwLock::new(false),
            pending_open_files: parking_lot::RwLock::new(Vec::new()),
            tool_vec_cache: parking_lot::RwLock::new(std::collections::HashMap::new()),
            config_mtime: parking_lot::RwLock::new(None),
            estop: parking_lot::RwLock::new(None),
            small_model: parking_lot::RwLock::new(None),
            approval_responder: parking_lot::RwLock::new(None),
            question_responder: parking_lot::RwLock::new(None),
            question_asker: parking_lot::RwLock::new(None),
        }
    }

    /// Attach a checkpoint store for the edit safety net. When set, every writer
    /// tool call (write_file/edit_file/append_file/delete_file) snapshots the
    /// file's pre-edit content before execution, so a rewind can restore it.
    pub fn set_checkpoint_store(&self, store: Arc<crate::checkpoint::CheckpointStore>) {
        *self.checkpoint_store.write() = Some(store);
    }

    /// L6++（2026-09-08）：checkpoint store 只读口——项目 loop 工厂测试用
    /// 它断言影子库落在主 workspace（`logs/project_checkpoints/{pid}`）而非
    /// 用户项目目录。
    pub fn checkpoint_store(&self) -> Option<Arc<crate::checkpoint::CheckpointStore>> {
        self.checkpoint_store.read().clone()
    }

    /// 绑定全局急停状态。工厂每次重建 loop 都调一次，所以急停状态在 agent
    /// 重启后自动保持（状态本体在 `SharedResources` 上，不在 loop 上）。
    pub fn set_estop(&self, estop: Arc<crate::estop::EstopState>) {
        *self.estop.write() = Some(estop);
    }

    /// N2：装配小模型杂务通道（工厂从 `agents.small_model` 解析后调用；
    /// 重复调用覆盖前值）。传 `None` 显式清除（= 回退主模型）。
    pub fn set_small_model(&self, provider: Option<(Arc<dyn LlmProvider>, String)>) {
        *self.small_model.write() = provider.map(|(p, name)| SmallModelSlot { provider: p, name });
    }

    /// N2：小模型槽位只读访问（E7 自动标题消费；未配置 = `None`，诚实跳过
    /// ——不烧主模型 token 生成标题）。
    fn small_model_slot(&self) -> Option<(Arc<dyn LlmProvider>, String)> {
        self.small_model
            .read()
            .as_ref()
            .map(|s| (s.provider.clone(), s.name.clone()))
    }

    /// E7 (devtool-upgrade 阶段 5)：会话标题自动生成——assistant 回复落盘
    /// 后触发；后台任务取首条 user 消息（≤500 字）→ 小模型生成 ≤24 字标题
    /// → 写 sidecar meta（只填「无标题/仅占位符」的会话，手动改名永不覆盖，
    /// 见 `chat_log::write_session_meta_auto_title`）。未配置
    /// `agents.small_model` / 无资格 / 首条消息缺失 → 不 spawn（返回 None）。
    /// 返回 JoinHandle 供测试 await；生产调用点丢弃句柄。
    fn spawn_session_title_job(&self, session_key: &str) -> Option<tokio::task::JoinHandle<()>> {
        let (provider, model) = self.small_model_slot()?;
        if !crate::chat_log::auto_title_eligible(session_key) {
            return None;
        }
        let first = crate::chat_log::first_user_message(session_key, E7_TITLE_INPUT_MAX_CHARS)?;
        let key = session_key.to_string();
        Some(tokio::spawn(async move {
            if let Some(title) =
                generate_title_from_first_message(provider.as_ref(), &model, &first).await
            {
                crate::chat_log::write_session_meta_auto_title(&key, &title);
            }
        }))
    }

    /// M7：装配审批响应端（gateway 把 WebApprovalManager 挂上来；重复调用
    /// 覆盖前值）。WSAPI approval handler 经 `approval_responder()` 触达。
    pub fn set_approval_responder(
        &self,
        responder: Arc<dyn nemesis_types::agent::ApprovalResponder>,
    ) {
        *self.approval_responder.write() = Some(responder);
    }

    /// M7：取审批响应端（未装配 = `None`——handler 诚实报「未装配」）。
    pub fn approval_responder(&self) -> Option<Arc<dyn nemesis_types::agent::ApprovalResponder>> {
        self.approval_responder.read().clone()
    }

    /// F7：挂结构化提问响应端（gateway 装配 WebQuestionBroker 后调用）。
    pub fn set_question_responder(
        &self,
        responder: Arc<dyn nemesis_types::agent::QuestionResponder>,
    ) {
        *self.question_responder.write() = Some(responder);
    }

    /// F7：取提问响应端（未装配 = `None`——handler 诚实报「未装配」）。
    pub fn question_responder(&self) -> Option<Arc<dyn nemesis_types::agent::QuestionResponder>> {
        self.question_responder.read().clone()
    }

    /// J5：装配 doom-loop 审批提问端（gateway 与 F7 responder 注入同一
    /// WebQuestionBroker Arc——同一 broker 的两个 trait 各挂一槽）。
    /// 未装配 = escalation 审批化不可用（诚实回退现行为）。
    pub fn set_question_asker(&self, asker: Arc<dyn nemesis_types::agent::QuestionAsker>) {
        *self.question_asker.write() = Some(asker);
    }

    /// J5：取审批提问端（未装配 = `None`）。
    pub(crate) fn question_asker(&self) -> Option<Arc<dyn nemesis_types::agent::QuestionAsker>> {
        self.question_asker.read().clone()
    }

    /// N2：手动压缩（`compact_session`）的摘要供给解析——小模型已配置则用
    /// 之；未配置（或 `prefer_small=false`）诚实回退主模型。锁序固定
    /// small_model → provider → active_model，全库唯此一处取这三把锁。
    fn resolve_summary_provider(&self, prefer_small: bool) -> (Arc<dyn LlmProvider>, String) {
        if prefer_small {
            if let Some(slot) = self.small_model.read().as_ref() {
                return (slot.provider.clone(), slot.name.clone());
            }
            debug!(
                "[AgentLoop] agents.small_model not configured; manual compact falls back to the main model"
            );
        }
        (
            self.provider.read().clone(),
            self.active_model.read().clone(),
        )
    }

    /// K1a (U14): 注册一个用户工具钩子。pre 在固定 security 闸之后、工具
    /// 执行之前运行；post 在执行之后、Forge 记录之前运行（详见
    /// `crate::hooks` 模块文档）。RwLock 注册——运行中随时可挂。
    pub fn add_tool_hook(&self, hook: Arc<dyn crate::hooks::ToolHook>) {
        self.tool_hooks.write().add(hook);
    }

    /// K1b (U14): 注册一个 LLM 调用级钩子。pre 在 messages 组装后、
    /// LlmRequest observer 事件前运行（可 Append 提醒消息 / 拦下本轮）；
    /// post 在响应错误恢复后、LlmResponse observer 事件前运行（可
    /// Allow/Replace/有限 Retry/Block）。详见 `crate::hooks` 模块文档。
    pub fn add_llm_hook(&self, hook: Arc<dyn crate::hooks::LlmHook>) {
        self.llm_hooks.write().add(hook);
    }

    /// K2 (U14): 注册一个 prompt/turn 生命周期钩子。on_user_prompt 在
    /// `run_with_trace` 顶部、消息进 history **之前**运行（拦截则模型
    /// 永远看不到该消息）；on_turn_end 在最终答案被接受后、Done 事件前
    /// 运行（可注入 feedback 要求再答一轮，预算封顶 fail-open）。
    pub fn add_lifecycle_hook(&self, hook: Arc<dyn crate::hooks::LifecycleHook>) {
        self.lifecycle_hooks.write().add(hook);
    }

    /// Wire the re-injection sender for the queue-drain path (round-5 review
    /// fix). The adapter passes a clone of the SAME mpsc sender that feeds
    /// `run_bus_arc`'s receiver, so a drained queued head re-enters the normal
    /// consumer loop and its reply gets full post-processing (rpc prefix,
    /// sent_in_round, error conversion). Call after the channel pair is
    /// created, before `run_bus_*` starts consuming.
    pub fn set_reinject_tx(
        &self,
        tx: tokio::sync::mpsc::Sender<nemesis_types::channel::InboundMessage>,
    ) {
        *self.reinject_tx.write() = Some(tx);
    }

    /// Stash the memory tool executor so the gateway can later attach an approval
    /// gate via `set_memory_approval_gate`. Called by the factory after building
    /// the shared tool config.
    #[cfg(feature = "memory")]
    pub fn set_memory_executor(&self, exec: Arc<nemesis_memory::memory_tools::MemoryToolExecutor>) {
        *self.memory_executor.write() = Some(exec);
    }

    /// P3.1 (sixth batch): wire the auto-inject channel — the memory manager
    /// (read-only retrieval) plus the `auto_inject`/`top_k` flags read from
    /// `config.enhanced_memory.json`. Passing `auto_inject=false` (the
    /// default everywhere) keeps the loop byte-identical to pre-P3.1.
    /// The `manager` param is cfg-gated: absent (and so not suppressable)
    /// under `--no-default-features` — memory injection is a no-op there.
    #[cfg(feature = "memory")]
    pub fn set_memory_inject(
        &self,
        manager: Option<Arc<nemesis_memory::manager::MemoryManager>>,
        auto_inject: bool,
        top_k: usize,
    ) {
        *self.memory_inject_manager.write() = manager;
        *self.memory_inject_cfg.write() = (auto_inject, top_k);
    }

    /// P3.1 stub (memory feature off): accepts only the flags (the manager
    /// param doesn't exist without the memory crate). Injection stays off —
    /// `memory_inject_cfg` records `(false, _)` from the real builder, but
    /// `prefetch_memory_context` returns None regardless (no manager).
    #[cfg(not(feature = "memory"))]
    pub fn set_memory_inject(&self, auto_inject: bool, top_k: usize) {
        *self.memory_inject_cfg.write() = (auto_inject, top_k);
    }

    /// Attach an approval gate to the memory executor (if one was stashed). After
    /// this, agent `memory_store`/`memory_forget` calls require approval.
    #[cfg(feature = "memory")]
    pub fn set_memory_approval_gate(
        &self,
        gate: Arc<dyn nemesis_memory::memory_tools::MemoryApprovalGate>,
    ) {
        if let Some(ref exec) = *self.memory_executor.read() {
            exec.set_approval_gate(gate);
        }
    }

    /// Rewind the workspace to the start of turn `from_turn`: restores every file
    /// changed at or after that turn to its pre-edit content (the edit safety
    /// net). Returns `(written, deleted)` paths. Errors if no checkpoint store is
    /// attached. Conversation rewinding (truncating session history) is handled
    /// by the caller — this only restores code.
    pub async fn rewind(&self, from_turn: usize) -> Result<(Vec<String>, Vec<String>), String> {
        let cp = self
            .checkpoint_store
            .read()
            .as_ref()
            .cloned()
            .ok_or("checkpoint store not attached")?;
        Ok(cp.restore_code(from_turn).await)
    }

    /// List checkpoint turns (for a rewind picker UI). Empty if no store attached.
    pub fn checkpoint_list(&self) -> Vec<crate::checkpoint::CheckpointMeta> {
        match self.checkpoint_store.read().as_ref() {
            Some(cp) => cp.list_meta(),
            None => Vec::new(),
        }
    }

    /// E3 消息级回退：会话截断到 `message_index` 所在 turn 结束（该消息
    /// 及其 turn 的回复**保留**，其后所有行截掉），文件恢复到其后第一个
    /// checkpoint turn 开始时的状态，并压入 undo 栈供 [`Self::redo_rewind`]
    /// 反向。
    ///
    /// 定位契约：`message_index` 是 [`crate::chat_log::read_chat_log`]（=
    /// `sessions.export` / `logs.session_detail` 的行序，同一 jsonl）里该
    /// 会话消息数组的下标。行→turn 精确定位靠 user 行的 `checkpoint_turn`
    /// 标记（本 turn begin 序号随 admission 穿针写入）；无标记的旧行退化为
    /// 「只截断对话不回滚文件」。
    ///
    /// 返回回执 JSON（kept/removed 计数 + 恢复文件清单 + redoable）。
    pub async fn rewind_to_message(
        &self,
        session_key: &str,
        message_index: usize,
    ) -> Result<serde_json::Value, String> {
        if self.is_session_busy(session_key) {
            return Err("会话正在处理消息，请等当前回合完成后再回退".to_string());
        }
        // 全量读（回退是一次性管理操作，非热路径——fork 同款）。
        let (rows, _total, _, _) = crate::chat_log::read_chat_log(session_key, usize::MAX, None);
        if rows.is_empty() {
            return Err("会话没有可回退的消息".to_string());
        }
        if message_index >= rows.len() {
            return Err(format!(
                "message_index {} 超出范围（会话共 {} 条消息）",
                message_index,
                rows.len()
            ));
        }

        // 对话截断点：index 之后第一个 user 行（turn 边界——assistant 行
        // 属于所在 turn，必须随 turn 一起保留/截掉）。
        let cut = rows[message_index + 1..]
            .iter()
            .position(|r| r.get("role").and_then(|v| v.as_str()) == Some("user"))
            .map(|p| p + message_index + 1)
            .unwrap_or(rows.len());
        // 文件恢复锚：cut 起第一个带 checkpoint_turn 标记的行（第一个仍在
        // 的 checkpoint turn）——恢复到它 begin 之前 = index 所在 turn 完成
        // 时的文件态。None = 之后没有 checkpoint turn（或全是无标记旧行）。
        let restore_turn = rows[cut.min(rows.len())..].iter().find_map(|r| {
            r.get("checkpoint_turn")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
        });
        let removed: Vec<serde_json::Value> = rows[cut..].to_vec();

        // undo 依据在任何突变前采集（truncate_from 会清掉 turn ≥ restore 的
        // 索引，tree 值要趁索引还在时读）。
        let (forward_tree, post_tree) = match self.attached_checkpoint() {
            Some(cp) => {
                // redo 前向恢复目标 = rewind 时刻的实时 tree（begin 树只到
                // 各 turn 开始态，最后一个 turn 的变更只在实况里）。仅文件
                // 恢复路径活跃（有 restore_turn 锚）时采集实时树；无锚时保
                // 持 begin 树语义（可能 None），redo 只回填行、文件步诚实跳
                // 过，陈旧性守卫基线不变。
                let forward = if restore_turn.is_some() {
                    cp.current_tree_hex().or_else(|| cp.latest_tree_hex())
                } else {
                    cp.latest_tree_hex()
                };
                // post 基线 = truncate 之后索引里剩的最新 tree（restore 无
                // 锚 = 索引不动，基线就是现状）。
                let post = match restore_turn {
                    Some(t) => cp.tree_hex_before(t),
                    None => forward.clone(),
                };
                (forward, post)
            }
            None => (None, None),
        };

        // 1) 文件恢复（先于 jsonl 截断——中途崩溃时对话完好，可重试）。
        let mut file_restore = "skipped";
        let mut file_note: Option<String> = None;
        let mut written: Vec<String> = Vec::new();
        let mut deleted: Vec<String> = Vec::new();
        if let (Some(t), Some(cp)) = (restore_turn, self.attached_checkpoint()) {
            let (w, d) = cp.restore_code(t).await;
            written = w;
            deleted = d;
            cp.truncate_from(t);
            file_restore = "applied";
        } else if restore_turn.is_some() {
            file_note = Some("checkpoint store 未挂载，文件未回滚".to_string());
        } else if cut < rows.len() {
            file_note = Some(
                "之后没有 checkpoint 标记（旧行或无 store），只截断对话不回滚文件".to_string(),
            );
        }

        // 2) jsonl 截断（tmp+rename 原子重写；verbatim 保留保留侧行）。
        let n = crate::chat_log::truncate_chat_log_rows(session_key, &rows[..cut]);
        if n != cut {
            warn!(
                "[AgentLoop] rewind 截断写回不完整（期望 {cut} 行写 {n} 行）——jsonl 保留原样，回退未生效"
            );
            return Err("会话日志写回失败，回退未生效（原会话完好）".to_string());
        }

        // 3) SessionStore 丢缓存（jsonl 是单一真相源，下次 get_or_create
        // 从截断后的 jsonl 自愈重建——sessions.delete/clear 同款纪律）。
        if let Some(store) = self.session_store() {
            store.clear_session(session_key);
        }

        // 4) 压 undo 栈（截断成功后才压——失败路径不留脏条目）。
        let redoable = !removed.is_empty();
        if redoable {
            let mut stacks = self.rewind_undo_stacks.lock();
            let stack = stacks.entry(session_key.to_string()).or_default();
            stack.push_back(RewindUndoEntry {
                removed_rows: removed,
                kept_count: cut,
                restore_turn,
                forward_tree,
                post_tree,
            });
            while stack.len() > REWIND_UNDO_STACK_CAP {
                stack.pop_front();
            }
        }

        Ok(serde_json::json!({
            "session_key": session_key,
            "message_index": message_index,
            "kept_count": cut,
            "removed_count": rows.len() - cut,
            "restore_turn": restore_turn,
            "file_restore": file_restore,
            "file_restore_note": file_note,
            "restored_files": { "written": written, "deleted": deleted },
            "redoable": redoable,
        }))
    }

    /// E3 redo：弹本会话 undo 栈顶，反向恢复——被截断的行 VERBATIM 回填
    /// jsonl，文件恢复到 undo 时刻的影子 tree。行数/tree 基线双重陈旧性
    /// 守卫：undo 之后该会话有任何新消息、或任何地方有新 checkpoint turn
    /// （工作区变过），诚实拒绝（弹出的条目作废）。
    ///
    /// 诚实边界：redo 不重建 checkpoint 索引（undo 时 turn-*.json 已清），
    /// redo 过的区间无法再次 rewind；文件只恢复工作区内（tree 覆盖范围）。
    pub async fn redo_rewind(&self, session_key: &str) -> Result<serde_json::Value, String> {
        if self.is_session_busy(session_key) {
            return Err("会话正在处理消息，请等当前回合完成后再重做".to_string());
        }
        let entry = {
            let mut stacks = self.rewind_undo_stacks.lock();
            stacks.get_mut(session_key).and_then(|s| s.pop_back())
        }
        .ok_or_else(|| "没有可重做的回退（先执行一次消息级回退）".to_string())?;

        // 守卫 1：会话行数未变（undo 后没发过新消息）。
        let (rows, _total, _, _) = crate::chat_log::read_chat_log(session_key, usize::MAX, None);
        if rows.len() != entry.kept_count {
            return Err(format!(
                "回退后产生了新消息，redo 已失效（期望 {} 行，实际 {} 行）",
                entry.kept_count,
                rows.len()
            ));
        }
        // 守卫 2：工作区 tree 基线未变（undo 后没有新 checkpoint turn）。
        let cur_tree = self
            .attached_checkpoint()
            .and_then(|cp| cp.latest_tree_hex());
        if cur_tree != entry.post_tree {
            return Err("回退后工作区发生了新的变化，redo 已失效".to_string());
        }

        // 1) 文件前向恢复（best-effort——失败只注记，行回填照常进行）。
        let mut file_restore = "skipped";
        let mut file_note: Option<String> = None;
        let mut written: Vec<String> = Vec::new();
        let mut deleted: Vec<String> = Vec::new();
        match (entry.forward_tree.as_deref(), self.attached_checkpoint()) {
            (Some(hex), Some(cp)) => match cp.restore_to_tree(hex) {
                Ok((w, d)) => {
                    written = w;
                    deleted = d;
                    file_restore = "applied";
                }
                Err(e) => file_note = Some(e),
            },
            (Some(_), None) => file_note = Some("checkpoint store 未挂载，文件未恢复".to_string()),
            (None, _) => {
                file_note = Some("无影子 tree（JSON 形态或该区间无变更），文件未恢复".to_string())
            }
        }

        // 2) 行回填（verbatim——原时间戳/标记原样回来）。
        let mut all = rows;
        all.extend(entry.removed_rows.iter().cloned());
        let expected = all.len();
        let n = crate::chat_log::truncate_chat_log_rows(session_key, &all);
        if n != expected {
            warn!("[AgentLoop] redo 回填写回不完整（期望 {expected} 行写 {n} 行）——jsonl 保留原样");
            return Err("会话日志写回失败，重做未生效（会话保持回退态）".to_string());
        }

        // 3) SessionStore 丢缓存（同 rewind——从回填后的 jsonl 重建）。
        if let Some(store) = self.session_store() {
            store.clear_session(session_key);
        }

        Ok(serde_json::json!({
            "session_key": session_key,
            "restored_count": entry.removed_rows.len(),
            "kept_count": entry.kept_count,
            "restore_turn": entry.restore_turn,
            "file_restore": file_restore,
            "file_restore_note": file_note,
            "restored_files": { "written": written, "deleted": deleted },
        }))
    }

    /// E3：checkpoint store 已挂载时克隆出 Arc（`rewind_to_message` /
    /// `redo_rewind` 的共享读法）。
    fn attached_checkpoint(&self) -> Option<Arc<crate::checkpoint::CheckpointStore>> {
        self.checkpoint_store.read().as_ref().cloned()
    }

    /// M3（devtool-upgrade 阶段 5）：会话级文件 diff——某文件「最早
    /// checkpoint 基线（pre-edit 态）vs 现盘」的 unified diff。
    ///
    /// 基线 = checkpoint 索引里首个声明过该文件的条目（git 形态读影子
    /// tree 的 blob；JSON 形态用快照 content；索引 store 全局——多会话
    /// 共改同一文件时基线取更早 turn，语义是「最早已知 pre-edit 态」）。
    /// 文件已不在磁盘 → head 视为空串（diff 呈全删）+ `head_on_disk:
    /// false` 诚实标注；diff 为空 → note 说明（可能已被回退/手动恢复）。
    /// `session_key` v1 只用于将来按会话收窄基线（现留参保调用形状稳定）。
    pub async fn session_file_diff(
        &self,
        _session_key: &str,
        path: &str,
    ) -> Result<serde_json::Value, String> {
        let cp = self
            .attached_checkpoint()
            .ok_or_else(|| "checkpoint 未挂载，无法对比会话文件变更".to_string())?;
        let base = cp.base_for_path(path).ok_or_else(|| {
            "该文件无 checkpoint 基线（未在本会话声明变更，或基线已被回退清除）".to_string()
        })?;

        // 基线内容：git 形态从影子 tree 读 blob；JSON 形态用快照 content。
        let base_content = match &base.tree {
            Some(tree) => cp.read_file_from_tree(tree, path)?.unwrap_or_default(),
            None => base.content.clone().unwrap_or_default(),
        };

        // 现盘内容：缺文件 = 空 head（diff 呈全删）+ head_on_disk=false。
        let abs = cp.root().join(path);
        let (head_content, head_on_disk) = match tokio::fs::read(&abs).await {
            Ok(bytes) => (String::from_utf8_lossy(&bytes).into_owned(), true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
            Err(e) => return Err(format!("读取现盘文件失败: {e}")),
        };

        let diff = crate::loop_tools::edit_hint::unified_diff(path, &base_content, &head_content);
        let note = if !head_on_disk {
            "文件已不在磁盘（对照基线呈全删除）".to_string()
        } else if diff.is_empty() {
            "当前内容与基线无差异（可能已被回退或手动恢复）".to_string()
        } else {
            String::new()
        };
        Ok(serde_json::json!({
            "path": path,
            "backend": if base.tree.is_some() { "git" } else { "json" },
            "base_turn": base.turn,
            "diff": diff,
            "head_on_disk": head_on_disk,
            "note": note,
        }))
    }

    /// Create a new agent loop in bus-integrated mode.
    ///
    /// This mirrors Go's `NewAgentLoop()`. It sets up:
    /// - Agent registry with a default "main" agent
    /// - Session store for persistent history
    /// - Outbound channel for publishing responses
    /// - Session busy tracker
    /// - Route resolver with a default single-agent configuration
    pub fn new_bus(
        provider: Box<dyn LlmProvider>,
        config: AgentConfig,
        outbound_tx: tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>,
        concurrent_mode: ConcurrentMode,
        queue_size: usize,
        max_continuation_permits: usize,
    ) -> Self {
        let registry = Arc::new(AgentRegistry::with_default(config.clone()));
        let session_store = Arc::new(SessionStore::new_in_memory());

        // Build a default route resolver with a single "main" agent.
        // This can be overridden via set_route_resolver() for multi-agent setups.
        let default_route_config = RouteConfig {
            bindings: Vec::new(),
            agents: vec![AgentDef {
                id: "main".to_string(),
                is_default: true,
            }],
            dm_scope: "main".to_string(),
        };

        let continuation_semaphore = if max_continuation_permits > 0 {
            Some(Arc::new(tokio::sync::Semaphore::new(
                max_continuation_permits,
            )))
        } else {
            None
        };

        let model = config.model.clone();
        info!(
            "[AgentLoop] Created in bus mode, model={}, concurrent_mode={:?}, queue_size={}, max_continuation_permits={}",
            model, concurrent_mode, queue_size, max_continuation_permits
        );

        Self {
            provider: parking_lot::RwLock::new(Arc::from(provider)),
            active_model: parking_lot::RwLock::new(config.model.clone()),
            tools: parking_lot::RwLock::new(HashMap::new()),
            config,
            outbound_tx: Some(outbound_tx),
            registry: Some(registry),
            state_manager: None,
            session_store: Some(session_store),
            running: AtomicBool::new(false),
            session_busy: parking_lot::Mutex::new(HashMap::new()),
            concurrent_mode,
            reinject_tx: parking_lot::RwLock::new(None),
            queue_size,
            max_continuation_permits,
            continuation_semaphore,
            summarizing: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            compact_state: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            channel_manager_channels: parking_lot::Mutex::new(Vec::new()),
            sent_in_round: SentInRoundTracker::new(),
            route_resolver: Some(RouteResolver::new(default_route_config)),
            observer_callback: None,
            continuation_manager: None,
            cluster: None,
            observer_manager: None,
            security_plugin: None,
            mcp_manager: None,
            mcp_tool_snapshot: Arc::new(parking_lot::RwLock::new(Vec::new())),
            data_store: None,
            forge: None,
            cancel_tokens: dashmap::DashMap::new(),
            checkpoint_store: parking_lot::RwLock::new(None),
            turn_counter: std::sync::atomic::AtomicUsize::new(0),
            turn_file_changes: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            rewind_undo_stacks: parking_lot::Mutex::new(HashMap::new()),
            tool_hooks: parking_lot::RwLock::new(crate::hooks::ToolHookManager::new()),
            llm_hooks: parking_lot::RwLock::new(crate::hooks::LlmHookManager::new()),
            lifecycle_hooks: parking_lot::RwLock::new(crate::hooks::LifecycleHookManager::new()),
            memory_executor: parking_lot::RwLock::new(None),
            #[cfg(feature = "memory")]
            memory_inject_manager: parking_lot::RwLock::new(None),
            #[cfg(not(feature = "memory"))]
            memory_inject_manager: parking_lot::RwLock::new(None),
            memory_inject_cfg: parking_lot::RwLock::new((false, 3)),
            tier: parking_lot::RwLock::new(nemesis_types::capability::ModelTier::Big),
            mode: parking_lot::RwLock::new(crate::types::AgentMode::Build),
            agent_event_tx: parking_lot::RwLock::new(None),
            config_path: parking_lot::RwLock::new(None),
            pricing_store: parking_lot::RwLock::new(None),
            lsp_manager: parking_lot::RwLock::new(None),
            commands_hot: parking_lot::RwLock::new(None),
            cc_bridge: parking_lot::RwLock::new(None),
            spill_root: parking_lot::RwLock::new(None),
            skills_loader: parking_lot::RwLock::new(None),
            skills_digest_state: std::sync::Arc::new(crate::skills_digest::DigestState::new()),
            inbox: std::sync::Arc::new(crate::inbox::Inbox::new(queue_size.max(1))),
            turn_task_handles: parking_lot::Mutex::new(Vec::new()),
            workspace_root: parking_lot::RwLock::new(None),
            fs_watcher: parking_lot::RwLock::new(None),
            external_changes: parking_lot::Mutex::new(Vec::new()),
            recent_self_writes: parking_lot::Mutex::new(std::collections::HashMap::new()),
            snapshot_role: parking_lot::RwLock::new("user".to_string()),
            interactive_approval: parking_lot::RwLock::new(false),
            pending_open_files: parking_lot::RwLock::new(Vec::new()),
            tool_vec_cache: parking_lot::RwLock::new(std::collections::HashMap::new()),
            config_mtime: parking_lot::RwLock::new(None),
            estop: parking_lot::RwLock::new(None),
            small_model: parking_lot::RwLock::new(None),
            approval_responder: parking_lot::RwLock::new(None),
            question_responder: parking_lot::RwLock::new(None),
            question_asker: parking_lot::RwLock::new(None),
        }
    }

    // -----------------------------------------------------------------------
    // Continuation dispatch
    // -----------------------------------------------------------------------

    /// Dispatch a cluster continuation: inline (permits=0) or spawned (permits>0).
    /// Called from both `run_bus_owned` (test) and `run_bus_arc` (production).
    async fn dispatch_continuation(
        &self,
        task_id: String,
        msg: &nemesis_types::channel::InboundMessage,
    ) {
        let task_response = msg.content.clone();
        let task_metadata = msg.metadata.clone();
        let task_failed = task_metadata
            .get("status")
            .map(|s| s == "error")
            .unwrap_or(false);

        if self.max_continuation_permits == 0 {
            // Inline: process directly in the main loop (no spawn).
            // The main loop is blocked until continuation completes,
            // ensuring serialized execution with no resource contention.
            let task_error = task_metadata.get("error").map(|s| s.as_str());
            if let Some(ref mgr) = self.continuation_manager
                && let Some(ref tx) = self.outbound_tx
            {
                // Clone data from RwLock guards before .await — guards are !Send
                // and cannot be held across yield points in an async fn.
                let provider = self.provider.read().clone();
                let model = self.active_model.read().clone();
                let tools = self.tools.read().clone();

                crate::loop_continuation::handle_cluster_continuation(
                    mgr.as_ref(),
                    &task_id,
                    &task_response,
                    task_failed,
                    task_error,
                    provider.as_ref(),
                    &model,
                    &tools,
                    tx,
                    self.observer_manager.clone(),
                    self.session_store.as_ref().map(|v| v.as_ref()),
                    // F-F：active 模型 vision 解析（config.json 唯一真相源）。
                    self.current_vision().supported,
                )
                .await;
            }
        } else {
            // Spawn with semaphore-controlled concurrency.
            let task_error = task_metadata.get("error").cloned();
            let provider = self.provider.read().clone();
            let model = self.active_model.read().clone();
            let tools = self.tools.read().clone();
            let outbound_tx = self.outbound_tx.clone();
            let continuation_manager = self.continuation_manager.clone();
            let observer_manager = self.observer_manager.clone();
            let session_store = self.session_store.clone();
            let semaphore = self.continuation_semaphore.clone().unwrap();
            // F-F：vision 解析在 spawn 前取值（闭包外，self 不可 move 进去）。
            let vision_supported = self.current_vision().supported;

            tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();
                if let Some(ref mgr) = continuation_manager
                    && let Some(ref tx) = outbound_tx
                {
                    crate::loop_continuation::handle_cluster_continuation(
                        mgr.as_ref(),
                        &task_id,
                        &task_response,
                        task_failed,
                        task_error.as_deref(),
                        provider.as_ref(),
                        &model,
                        &tools,
                        tx,
                        observer_manager,
                        session_store.as_ref().map(|v| v.as_ref()),
                        vision_supported,
                    )
                    .await;
                }
            });
        }
    }

    // -----------------------------------------------------------------------
    // Registration methods
    // -----------------------------------------------------------------------

    /// Register a tool with the agent loop (standalone mode).
    pub fn register_tool(&mut self, name: String, tool: Box<dyn Tool>) {
        debug!("[AgentLoop] Registered tool: {}", name);
        self.tools.write().insert(name, Arc::from(tool));
    }

    /// Register a tool across all agents in the registry (bus mode).
    /// Mirrors Go's `AgentLoop.RegisterTool()`.
    pub fn register_tool_shared(&mut self, name: String, tool: Box<dyn Tool>) {
        debug!("[AgentLoop] Registered shared tool: {}", name);
        self.tools.write().insert(name, Arc::from(tool));
    }

    /// J3 (devtool-upgrade 阶段 4)：注册 MCP 工具——同名不再静默覆盖。
    /// 旧路径直接 HashMap insert：sanitize 撞名的两个工具（同 server 内
    /// `search-file`/`search_file`，或两 server 名 sanitize 后相同）后者
    /// 无声顶掉前者，无任何痕迹。已存在时 warn 并依序改注册为
    /// `{name}_2`、`{name}_3`…——build_tool_defs 用注册键做 LLM 可见名，
    /// 改名后两个工具都诚实可见、可被调用（bridge 内部原始名只用于
    /// description/parameters，不受影响）。
    fn register_mcp_tool(&self, tool: Box<dyn nemesis_mcp::adapter::Tool>) {
        let base = tool.definition().name.clone();
        let mut tools = self.tools.write();
        let name = if tools.contains_key(&base) {
            let mut n = 2;
            while tools.contains_key(&format!("{base}_{n}")) {
                n += 1;
            }
            let renamed = format!("{base}_{n}");
            warn!(
                "[AgentLoop] MCP tool name collision: '{}' already registered; \
                 duplicate registered as '{renamed}'",
                base
            );
            renamed
        } else {
            base
        };
        tools.insert(
            name,
            Arc::from(Box::new(crate::mcp_bridge::McpToolBridge::new(tool)) as Box<dyn Tool>),
        );
    }

    // [ClusterService-Full] 完整方案预留：动态移除工具
    // 当前未启用，原因：避免影响 LLM 提示词缓存命中率
    // 启用条件：当 LLM 提供商支持按工具分组缓存或工具定义独立缓存时
    /// Remove a tool by name from the registry.
    /// Returns true if the tool was found and removed.
    pub fn remove_tool_shared(&mut self, name: &str) -> bool {
        if self.tools.write().remove(name).is_some() {
            debug!("[AgentLoop] Removed shared tool: {}", name);
            true
        } else {
            debug!("[AgentLoop] Tool '{}' not found, nothing to remove", name);
            false
        }
    }

    /// Return the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.read().len()
    }

    /// Return the names of all registered tools.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.read().keys().cloned().collect()
    }

    /// L6++（2026-09-08）：按名取注册工具的 `parameters()` JSON。项目 loop
    /// 工厂测试用它对两 loop 的共有工具逐同名比对 schema 字节一致（prompt
    /// cache 契约——同名工具 schema 漂移会让切会话组的请求缓存全失效）。
    /// None = 未注册。
    pub fn tool_parameters(&self, name: &str) -> Option<serde_json::Value> {
        self.tools.read().get(name).map(|t| t.parameters())
    }

    /// Enable automatic MCP tool reload via mtime-based change detection.
    ///
    /// Creates an `McpManager` for the given config path, discovers tools from
    /// all currently configured servers, and registers them. On each LLM round,
    /// the manager checks if the config file changed and loads new servers.
    pub fn enable_mcp_reload(&mut self, config_path: std::path::PathBuf) {
        let mgr = nemesis_mcp::manager::McpManager::new(config_path);
        if mgr.is_enabled() {
            for server in mgr.list_servers().to_vec() {
                let server_name = server.name.clone();
                match tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(mgr.discover_tools(&server))
                }) {
                    Ok(tools) => {
                        let count = tools.len();
                        for tool in tools {
                            self.register_mcp_tool(tool);
                        }
                        info!(
                            "[AgentLoop] MCP: registered {} tools from '{}'",
                            count, server_name
                        );
                    }
                    Err(e) => {
                        warn!(
                            "[AgentLoop] MCP: server '{}' discovery failed: {}",
                            server_name, e
                        );
                    }
                }
            }
            self.mcp_manager = Some(std::sync::Mutex::new(mgr));
            info!("[AgentLoop] MCP dynamic reload enabled (mtime-based)");
        } else {
            // Store manager even when disabled so we can detect future enable via config change
            self.mcp_manager = Some(std::sync::Mutex::new(mgr));
            info!("[AgentLoop] MCP config disabled; reload watcher active for future changes");
        }
        self.refresh_mcp_snapshot();
    }

    /// Check MCP config for changes and register tools from new servers.
    /// Uses interior mutability since the run loop borrows `&self`.
    fn check_mcp_reload(&self) {
        let mgr = match self.mcp_manager.as_ref() {
            Some(m) => m,
            None => return,
        };

        // 热重载收编登记（2026-08-29）：本处是 manager 重建 + 工具注册副作用
        // （重语义），非纯数据加载——保留独立实现并登记（同 tier 注记）。
        let changed = {
            match mgr.lock() {
                Ok(mut m) => m.check_config_changed(),
                Err(_) => return,
            }
        };

        if !changed {
            return;
        }

        // Collect existing MCP server prefixes to detect what's new.
        // J3：先取 mgr（锁序 mgr → tools 与下方发现循环一致），配置 server
        // 名 + 工具键快照后交给纯函数 [`registered_server_prefixes`] 正推。
        let (configured_servers, tool_keys) = {
            let configured: Vec<String> = match mgr.lock() {
                Ok(m) => m.list_servers().iter().map(|s| s.name.clone()).collect(),
                Err(_) => return,
            };
            let keys: Vec<String> = self.tools.read().keys().cloned().collect();
            (configured, keys)
        };
        let registered = registered_server_prefixes(&configured_servers, &tool_keys);

        let new_servers: Vec<_> = {
            match mgr.lock() {
                Ok(m) => m
                    .find_new_servers(&registered)
                    .into_iter()
                    .cloned()
                    .collect(),
                Err(_) => return,
            }
        };

        for server in new_servers {
            let server_name = server.name.clone();
            let tools = match mgr.lock() {
                Ok(m) => tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(m.discover_tools(&server))
                }),
                Err(_) => continue,
            };

            match tools {
                Ok(tools) => {
                    let count = tools.len();
                    for tool in tools {
                        self.register_mcp_tool(tool);
                    }
                    info!(
                        "[AgentLoop] MCP reload: registered {} tools from '{}'",
                        count, server_name
                    );
                }
                Err(e) => {
                    warn!(
                        "[AgentLoop] MCP reload: server '{}' failed: {}",
                        server_name, e
                    );
                }
            }
        }
        self.refresh_mcp_snapshot();
    }

    /// Refresh the MCP tool snapshot from the tool registry.
    fn refresh_mcp_snapshot(&self) {
        let snapshot: Vec<(String, String)> = self
            .tools
            .read()
            .iter()
            .filter(|(name, _)| name.starts_with("mcp_"))
            .map(|(name, tool)| (name.clone(), tool.description()))
            .collect();
        *self.mcp_tool_snapshot.write() = snapshot;
    }

    /// Return a shared reference to the MCP tool snapshot.
    /// Used to wire up McpListTool.
    pub fn mcp_tool_snapshot(&self) -> Arc<parking_lot::RwLock<Vec<(String, String)>>> {
        self.mcp_tool_snapshot.clone()
    }

    /// Set the channel manager reference for listing enabled channels.
    /// Mirrors Go's `SetChannelManager()`.
    pub fn set_channel_manager(&self, enabled_channels: Vec<String>) {
        *self.channel_manager_channels.lock() = enabled_channels;
    }

    /// Set the state manager for recording last channel/chat ID.
    /// Mirrors Go's `state.NewManager(workspace)`.
    pub fn set_state_manager(
        &mut self,
        mgr: Arc<nemesis_state::workspace_state::WorkspaceStateManager>,
    ) {
        self.state_manager = Some(mgr);
        debug!("[AgentLoop] State manager configured");
    }

    /// Set the observer callback for event emission.
    /// Mirrors Go's `SetObserverManager()`.
    pub fn set_observer_callback(
        &mut self,
        cb: Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>,
    ) {
        self.observer_callback = Some(cb);
        debug!("[AgentLoop] Observer callback configured");
    }

    /// Set the route resolver for multi-agent message routing.
    /// Mirrors Go's `AgentLoop.registry` (RouteResolver).
    /// When set, `process_inbound_message` uses the full 7-level priority
    /// cascade to determine agent and session key.
    pub fn set_route_resolver(&mut self, resolver: RouteResolver) {
        self.route_resolver = Some(resolver);
        info!("[AgentLoop] Route resolver configured");
    }

    /// Set the cluster reference.
    ///
    /// Accepts an `Arc<dyn Any + Send + Sync>` to avoid a compile-time dependency
    /// on the `nemesis-cluster` crate. The concrete cluster instance should be
    /// wrapped with `Arc::new(cluster) as Arc<dyn Any + Send + Sync>`.
    /// Mirrors Go's `AgentLoop.cluster` field assignment.
    pub fn set_cluster(&mut self, cluster: Arc<dyn std::any::Any + Send + Sync>) {
        self.cluster = Some(cluster);
    }

    /// Get the cluster reference, if set.
    ///
    /// Returns `Option<&Arc<dyn Any + Send + Sync>>`. The caller is responsible
    /// for downcasting to the concrete cluster type. Mirrors Go's `GetCluster()`.
    pub fn get_cluster(&self) -> Option<&Arc<dyn std::any::Any + Send + Sync>> {
        self.cluster.as_ref()
    }

    /// Set the observer manager for Phase 5 event emission.
    /// Mirrors Go's `SetObserverManager()`.
    pub fn set_observer_manager(&mut self, mgr: Arc<nemesis_observer::Manager>) {
        self.observer_manager = Some(mgr);
    }

    /// Set the security plugin for pre-execution tool safety checks.
    /// Mirrors Go's SecurityPlugin registered via PluginManager.
    #[cfg(feature = "security")]
    pub fn set_security_plugin(&mut self, plugin: Arc<nemesis_security::pipeline::SecurityPlugin>) {
        self.security_plugin = Some(plugin);
    }

    /// Set the session store, replacing the default in-memory store.
    /// Call this to enable disk-persisted conversation history.
    pub fn set_session_store(&mut self, store: Arc<crate::session::SessionStore>) {
        self.session_store = Some(store);
    }

    /// Get the session store, if one is configured.
    ///
    /// Used by callers outside the main agent loop (e.g. cluster_agent) that need
    /// to read/write history via the same SessionStore the loop would use.
    pub fn session_store(&self) -> Option<&Arc<crate::session::SessionStore>> {
        self.session_store.as_ref()
    }

    /// Set the continuation manager for async cluster RPC callbacks.
    ///
    /// When set, `cluster_continuation` messages intercepted by the bus loop
    /// will trigger snapshot loading and LLM resumption.
    pub fn set_continuation_manager(
        &mut self,
        manager: Arc<crate::loop_continuation::ContinuationManager>,
    ) {
        self.continuation_manager = Some(manager);
    }

    /// G4 (devtool-upgrade 阶段 3)：列出重启时遗留在盘上的后台 subagent
    /// pending 快照（`bg_` 前缀，只读不删）。gateway 适配器启动期以这些 id
    /// 注入诚实丢失回执（后台任务是进程内 tokio 任务，gateway 重启即终止、
    /// 无对端可 poll）——回灌走正常续行路径，快照由 resume 自清。
    pub fn list_stale_bg_spawn_task_ids(&self) -> Vec<String> {
        self.continuation_manager
            .as_ref()
            .map(|m| m.list_bg_spawn_pending_sync())
            .unwrap_or_default()
    }

    /// Set the data store for recording LLM usage statistics.
    pub fn set_data_store(&mut self, store: Arc<nemesis_data::DataStore>) {
        self.data_store = Some(store);
    }

    /// 用量记账存储只读访问（全自动流转 P5/E1 二期 token 回传：cluster
    /// agent 执行完 peer_chat 任务后，以 session 聚合差值提取本轮 token
    /// 用量并随回调回传 master 记账）。未装配 = None。
    pub fn data_store(&self) -> Option<Arc<nemesis_data::DataStore>> {
        self.data_store.clone()
    }

    /// P2B（2026-09-12 NB-15 根修配套）：工具参数校验结果落账（DataStore
    /// 天×模型 upsert，`models.health` 数据源——worker 端 validation_budget
    /// 失败沉淀为可观测的长期趋势，供 tier 校准决策）。无 data_store 静默
    /// 跳过；写失败只 warn——审计是增值动作，不反压轮次。
    fn record_tool_validation_stats(&self, failed: bool) {
        if let Some(ref ds) = self.data_store {
            let model = self.active_model.read().clone();
            if let Err(e) = ds.record_tool_validation(&model, failed) {
                tracing::warn!("[AgentLoop] record_tool_validation failed: {e}");
            }
        }
    }

    /// Set the Forge instance for experience collection.
    #[cfg(feature = "forge")]
    pub fn set_forge(&mut self, forge: Arc<nemesis_forge::forge::Forge>) {
        self.forge = Some(forge);
    }

    /// Swap the LLM provider and model at runtime. Takes effect immediately
    /// for the next LLM call. In-flight requests continue with the old provider.
    pub fn set_provider_and_model(&self, provider: Arc<dyn LlmProvider>, model: String) {
        *self.provider.write() = provider;
        *self.active_model.write() = model;
        tracing::info!("[AgentLoop] Provider swapped at runtime");
        // Phase 4a: re-resolve capability tier for the new model.
        self.refresh_active_tier();
    }

    /// Get the observer manager, if set.
    /// Mirrors Go's `GetObserverManager()`.
    pub fn get_observer_manager(&self) -> Option<&Arc<nemesis_observer::Manager>> {
        self.observer_manager.as_ref()
    }

    /// Get the agent registry (bus mode).
    pub fn get_registry(&self) -> Option<&Arc<AgentRegistry>> {
        self.registry.as_ref()
    }

    /// Get a clone of the provider Arc.
    pub fn provider_arc(&self) -> Arc<dyn LlmProvider> {
        self.provider.read().clone()
    }

    /// Get a mutable reference to the agent config.
    pub fn config_mut(&mut self) -> &mut AgentConfig {
        &mut self.config
    }

    // -----------------------------------------------------------------------
    // Bus-integrated main loop
    // -----------------------------------------------------------------------

    /// Run the main bus consumption loop (takes ownership of the receiver).
    ///
    /// This is the preferred entry point for bus-integrated mode.
    /// Mirrors Go's `AgentLoop.Run(ctx)`. Continuously consumes inbound
    /// messages, processes them, and publishes outbound responses.
    /// Stops when `stop()` is called or the inbound channel closes.
    ///
    /// Test-only variant; production code uses `run_bus_arc`.
    /// Post-turn finish (V5: extracted verbatim from the original run_bus
    /// bodies so the serial pump and spawned turn tasks share one tail):
    /// error funnel + capture flush, sent-in-round check, RPC correlation
    /// prefix, outbound publish.
    ///
    /// `check_sent_in_round`: Reject (serial — no turn can overlap) keeps the
    /// historical check+clear. Queue/Steer Immediate replies (busy receipts,
    /// slash responses) pass false: they can run while the session's turn is
    /// in flight, and touching that turn's sent-in-round flag mid-flight
    /// would corrupt its end-of-turn publish decision.
    async fn finish_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
        response: String,
        err: Option<String>,
        check_sent_in_round: bool,
    ) {
        let response = match err {
            Some(e) => {
                // [capture] Agent error funnel: the full error
                // becomes the user-visible response. Flush the
                // session's captured evidence + the complete error
                // text (the user sees a short "Error: ..."; this
                // keeps the full source string for root-causing).
                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                    sink.flush(&msg.session_key, "agent_error", None, Some(e.as_str()));
                }
                format!("Error processing message: {}", e)
            }
            None => response,
        };

        if response.is_empty() {
            return;
        }

        if check_sent_in_round {
            // Check if a tool (e.g., MessageTool) already sent a response for this
            // session in the current round. Mirrors Go's alreadySent check.
            let already_sent = self.sent_in_round.has_sent_in_round(&msg.session_key);
            // Only clear this session's flag, not all sessions.
            // Go clears per-tool-instance state, so clearing only the current
            // session preserves other sessions' sent-in-round tracking.
            self.sent_in_round.clear(&msg.session_key);

            if already_sent {
                debug!(
                    "[AgentLoop] Skipping outbound publish: message tool already sent response for session {}",
                    msg.session_key
                );
                return;
            }
        }

        if let Some(ref tx) = self.outbound_tx {
            // For RPC channel, add correlation ID prefix if not already present.
            let final_content = if msg.channel == "rpc"
                && !msg.correlation_id.is_empty()
                && !response.starts_with(&format!("[rpc:{}]", msg.correlation_id))
            {
                format!("[rpc:{}] {}", msg.correlation_id, response)
            } else {
                response
            };

            info!(
                "[AgentLoop] Response message     to {}:{}: {}",
                msg.channel,
                msg.chat_id,
                truncate(&final_content, 80)
            );

            let outbound = nemesis_types::channel::OutboundMessage {
                channel: msg.channel.clone(),
                chat_id: msg.chat_id.clone(),
                content: final_content,
                message_type: String::new(),
                meta: nemesis_types::channel::OutboundMeta {
                    model: Some(self.current_display_model()),
                    // L2：会话键随行——web 通道 chat_event_log 按会话（非连接）
                    // 记录，断线重连后 chat.sync 才能寻址。
                    session_key: (!msg.session_key.is_empty()).then(|| msg.session_key.clone()),
                },
            };
            if let Err(e) = tx.send(outbound).await {
                warn!("[AgentLoop] Failed to send outbound message: {}", e);
            }
        }
    }

    /// Spawn a turn task (Queue/Steer modes) and track its abort handle so
    /// `stop()` can cancel in-flight turns. Finished handles are pruned on
    /// each insert, keeping the vec bounded by live turns.
    fn spawn_turn_task<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(fut);
        let mut handles = self.turn_task_handles.lock();
        handles.retain(|h| !h.is_finished());
        handles.push(handle.abort_handle());
    }

    /// Shared bus pump (V5): mode-dependent dispatch.
    ///
    /// Reject (default): serial — byte-identical to the historical loop;
    /// each turn completes before the next message is processed.
    ///
    /// Queue/Steer: the synchronous gate (`gate_inbound`) runs inline in the
    /// pump (routing + busy check + inbox parking + receipts), then each
    /// admitted turn runs as a tracked spawned task so a long turn cannot
    /// starve the gate — this is what makes the U7 inbox reachable from
    /// production channels. Same-session ordering holds because the gate
    /// acquires the session BEFORE spawning; cross-session turns run
    /// concurrently. Continuations stay inline (serialized with the pump).
    async fn run_bus_impl(
        self: Arc<Self>,
        mut inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        self.running.store(true, Ordering::Release);
        info!("[AgentLoop] Bus consumption loop started");

        while self.running.load(Ordering::Acquire) {
            match inbound_rx.recv().await {
                Some(msg) => match self.concurrent_mode {
                    ConcurrentMode::Reject => {
                        let (agent_id, response, err) = self.process_inbound_message(&msg).await;

                        // Check for cluster continuation marker.
                        if agent_id == "__continuation__" {
                            let task_id = response;
                            info!(
                                "[AgentLoop] Handling cluster continuation for task {} (permits={})",
                                task_id, self.max_continuation_permits
                            );
                            self.dispatch_continuation(task_id, &msg).await;
                            continue;
                        }

                        self.finish_message(&msg, response, err, true).await;
                    }
                    ConcurrentMode::Queue | ConcurrentMode::Steer => {
                        match self.gate_inbound(&msg) {
                            GateOutcome::Continuation(task_id) => {
                                info!(
                                    "[AgentLoop] Handling cluster continuation for task {} (permits={})",
                                    task_id, self.max_continuation_permits
                                );
                                self.dispatch_continuation(task_id, &msg).await;
                            }
                            GateOutcome::Immediate {
                                agent_id: _,
                                response,
                            } => {
                                // Busy receipt / slash reply — publish inline.
                                // Never touches sent_in_round (may overlap the
                                // session's running turn; see finish_message).
                                self.finish_message(&msg, response, None, false).await;
                            }
                            GateOutcome::Maintenance {
                                kind,
                                session_key,
                                receipt,
                            } => {
                                // E6: 维护命令 — 会话已在 gate 获取，派独立
                                // task 执行（LLM 摘要可达分钟级，不得堵泵）；
                                // task 尾部释放会话。同串行路径：⏳ 先发、✓
                                // 后发，均不碰 sent_in_round。
                                let this = self.clone();
                                let m = msg.clone();
                                self.spawn_turn_task(async move {
                                    this.finish_message(&m, receipt, None, false).await;
                                    let response =
                                        this.handle_maintenance(kind, &session_key).await;
                                    this.release_session(&session_key);
                                    this.finish_message(&m, response, None, false).await;
                                });
                            }
                            GateOutcome::UserDispatch {
                                task_text,
                                node_id,
                                session_key,
                            } => {
                                // K4: 用户直发远程编码任务派发 — 会话已在
                                // gate 获取，派独立 task 执行（RPC ACK 等待
                                // 可达分钟级，不得堵泵）；task 尾部释放会话
                                // （process_user_dispatch 内部负责）。
                                let this = self.clone();
                                let m = msg.clone();
                                self.spawn_turn_task(async move {
                                    this.process_user_dispatch(
                                        &m,
                                        &task_text,
                                        &node_id,
                                        &session_key,
                                    )
                                    .await;
                                });
                            }
                            GateOutcome::Ungated => {
                                let this = self.clone();
                                let m = msg.clone();
                                self.spawn_turn_task(async move {
                                    let (_, response, err) = this.process_ungated(&m).await;
                                    this.finish_message(&m, response, err, true).await;
                                });
                            }
                            GateOutcome::Admitted(admission) => {
                                let this = self.clone();
                                let m = msg.clone();
                                self.spawn_turn_task(async move {
                                    let (_, response, err) =
                                        this.process_admitted(&m, admission).await;
                                    this.finish_message(&m, response, err, true).await;
                                });
                            }
                        }
                    }
                },
                None => {
                    // Channel closed.
                    break;
                }
            }
        }

        info!("[AgentLoop] Bus consumption loop stopped");
        self.running.store(false, Ordering::Release);
    }

    /// Run the main bus consumption loop (takes ownership of the receiver).
    ///
    /// This is the preferred entry point for bus-integrated mode.
    /// Mirrors Go's `AgentLoop.Run(ctx)`. Continuously consumes inbound
    /// messages, processes them, and publishes outbound responses.
    /// Stops when `stop()` is called or the inbound channel closes.
    ///
    /// Test-only variant; production code uses `run_bus_arc`.
    #[cfg(test)]
    pub async fn run_bus_owned(
        self,
        inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        std::sync::Arc::new(self).run_bus_impl(inbound_rx).await;
    }

    /// Same as `run_bus_owned` but takes `Arc<Self>` so the AgentLoop can be
    /// shared with other components (e.g. heartbeat handler) while the bus
    /// loop is running.
    pub async fn run_bus_arc(
        self: Arc<Self>,
        inbound_rx: tokio::sync::mpsc::Receiver<nemesis_types::channel::InboundMessage>,
    ) {
        self.run_bus_impl(inbound_rx).await;
    }

    /// Stop the bus consumption loop.
    /// Mirrors Go's `AgentLoop.Stop()`.
    pub fn stop(&self) {
        info!("[AgentLoop] Stop requested");
        self.running.store(false, Ordering::Release);
        // V5 (2026-08-23): kill Queue/Steer mode's spawned turn tasks. The
        // adapter aborts the pump task itself; without this the spawned turns
        // would be orphaned (holding Arc clones) and keep running/publishing
        // after the stop.
        let mut handles = self.turn_task_handles.lock();
        for h in handles.drain(..) {
            h.abort();
        }
    }

    /// Check whether the loop is currently running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Clear all session busy states.
    ///
    /// Called after a forced stop (task abort) to release sessions that were
    /// mid-processing when the agent was killed.  Without this, those sessions
    /// remain permanently locked ("busy") and all subsequent messages for them
    /// are rejected.
    pub fn clear_session_busy(&self) {
        let mut map = self.session_busy.lock();
        let count = map.len();
        map.clear();
        if count > 0 {
            tracing::warn!(
                "[AgentLoop] Cleared {} session busy states (agent was stopped mid-processing)",
                count
            );
        }
    }

    // -----------------------------------------------------------------------
    // Observer event emission helpers
    // -----------------------------------------------------------------------

    /// Emit an observer event synchronously (for conversation start/end).
    ///
    /// Forwards to both the Phase 5 observer manager and the legacy
    /// `observer_callback`.
    async fn emit_observer_sync(&self, event: crate::loop_executor::ObserverEvent) {
        if let Some(ref mgr) = self.observer_manager {
            let conv_event = event.to_conversation_event();
            mgr.emit_sync(conv_event).await;
        }
        if let Some(ref cb) = self.observer_callback {
            let (event_type, data) = event.to_callback_json();
            cb(event_type, &data);
        }
    }

    // -----------------------------------------------------------------------
    // Cluster continuation handling
    // -----------------------------------------------------------------------

    /// Handle a cluster continuation by loading the snapshot, resuming the LLM
    /// loop, and sending the final response.
    ///
    /// NOTE: The main run_bus_owned loop calls the free function
    /// `crate::loop_continuation::handle_cluster_continuation` directly instead
    /// of this method. Similarly, maybe_summarize calls the standalone
    /// `summarize_history_owned` / `summarize_multipart_owned` / `summarize_batch_owned`
    /// free functions. These self methods are kept as reference implementations
    /// matching the Go AgentLoop method signatures.
    #[allow(dead_code)]
    async fn handle_cluster_continuation(
        &self,
        task_id: &str,
        original_msg: &nemesis_types::channel::InboundMessage,
    ) {
        if let Some(ref mgr) = self.continuation_manager {
            let task_response = &original_msg.content;
            let task_failed = original_msg
                .metadata
                .get("status")
                .map(|s| s == "error")
                .unwrap_or(false);
            let task_error = original_msg.metadata.get("error").map(|s| s.as_str());

            // Clone provider and model before .await (RwLock guards are not Send).
            let cont_provider = self.provider.read().clone();
            let cont_model = self.active_model.read().clone();
            if let Some(ref tx) = self.outbound_tx {
                crate::loop_continuation::handle_cluster_continuation(
                    mgr.as_ref(),
                    task_id,
                    task_response,
                    task_failed,
                    task_error,
                    cont_provider.as_ref(),
                    &cont_model,
                    &self.tools,
                    tx,
                    self.observer_manager.clone(),
                    self.session_store.as_ref().map(|v| v.as_ref()),
                    // F-F：active 模型 vision 解析。
                    self.current_vision().supported,
                )
                .await;
            }
        } else {
            warn!(
                "[AgentLoop] No continuation manager configured, cannot handle continuation for task_id={}",
                task_id
            );
        }
    }

    // -----------------------------------------------------------------------
    // Direct processing (bypass bus)
    // -----------------------------------------------------------------------

    /// Process a direct message without the bus.
    /// Mirrors Go's `ProcessDirect()`.
    pub async fn process_direct(&self, content: &str, session_key: &str) -> Result<String, String> {
        self.process_direct_with_channel(content, session_key, "cli", "direct")
            .await
    }

    /// Process a direct message with explicit channel/chat ID.
    /// Mirrors Go's `ProcessDirectWithChannel()`.
    pub async fn process_direct_with_channel(
        &self,
        content: &str,
        session_key: &str,
        channel: &str,
        chat_id: &str,
    ) -> Result<String, String> {
        let trace_id = format!(
            "direct-{}-{}",
            session_key,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let start_time = std::time::Instant::now();

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "direct".to_string(),
            content: content.to_string(),
        })
        .await;

        let instance = self.get_or_create_instance(session_key);
        let context = RequestContext::new(channel, chat_id, "cron", session_key);

        let token = tokio_util::sync::CancellationToken::new();
        let events = self
            .run_with_trace(
                &instance,
                content,
                &context,
                &trace_id,
                false,
                &token,
                None,
                &[],
            )
            .await;

        // Extract final response for the conversation end event.
        let final_response = events
            .iter()
            .rev()
            .find_map(|e| {
                if let AgentEvent::Done(msg) = e {
                    Some(msg.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        })
        .await;

        // Extract final response from events.
        for event in events.iter().rev() {
            if let AgentEvent::Done(msg) = event {
                return Ok(msg.clone());
            }
        }
        for event in events.iter().rev() {
            if let AgentEvent::Error(msg) = event {
                return Err(msg.clone());
            }
        }
        Ok(String::new())
    }

    /// Process a heartbeat request without session history.
    /// Each heartbeat is independent and doesn't accumulate context.
    /// Mirrors Go's `ProcessHeartbeat()`.
    pub async fn process_heartbeat(
        &self,
        content: &str,
        channel: &str,
        chat_id: &str,
    ) -> Result<String, String> {
        let trace_id = format!(
            "heartbeat-{}-{}",
            chat_id,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let start_time = std::time::Instant::now();

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: "heartbeat".to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "heartbeat".to_string(),
            content: content.to_string(),
        })
        .await;

        // Heartbeat uses a fresh temporary instance, no history.
        let config = AgentConfig {
            model: self.active_model.read().clone(),
            system_prompt: self.config.system_prompt.clone(),
            max_turns: self.config.max_turns,
            tools: self.config.tools.clone(),
            models: self.config.models.clone(),
        };
        let instance = AgentInstance::new(config);
        let context = RequestContext::new(channel, chat_id, "heartbeat", "heartbeat");

        let token = tokio_util::sync::CancellationToken::new();
        let events = self
            .run_with_trace(
                &instance,
                content,
                &context,
                &trace_id,
                false,
                &token,
                None,
                &[],
            )
            .await;

        // Extract final response for the conversation end event.
        let final_response = events
            .iter()
            .rev()
            .find_map(|e| {
                if let AgentEvent::Done(msg) = e {
                    Some(msg.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: "heartbeat".to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        })
        .await;

        for event in events.iter().rev() {
            if let AgentEvent::Done(msg) = event {
                return Ok(msg.clone());
            }
        }
        Ok("I've completed processing but have no response to give.".to_string())
    }

    // -----------------------------------------------------------------------
    // Inbound message processing (bus mode)
    // -----------------------------------------------------------------------

    /// Process an inbound message from the bus.
    ///
    /// Returns (agent_id, response_content, optional_error).
    /// Mirrors Go's `processMessage()`.
    async fn process_inbound_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, String, Option<String>) {
        // 自定义 slash 命令改写（改写型，见 rewrite_custom_command）：在闸门
        // 之前原地展开为提示词，让消息以最终形态走正常会话/LLM 流程。内置
        // 命令不受影响（rewrite 跳过内置名，gate 内的短路检查照常先行）。
        // K3：async 化（`` !`cmd` `` 注入 + 技能回落），模板注入命令在闸门
        // 前执行完毕——进闸的消息永远是最终形态。
        let mut msg = msg.clone();
        self.rewrite_custom_command(&mut msg).await;
        // V5 (2026-08-23): gate first (sync classification + session
        // acquire), then the matching tail. Reject mode's serial pump and
        // all direct callers (heartbeat, tests, inline queue-drain fallback)
        // still enter here — behavior identical to the pre-split monolith.
        match self.gate_inbound(&msg) {
            GateOutcome::Continuation(task_id) => ("__continuation__".to_string(), task_id, None),
            GateOutcome::Immediate { agent_id, response } => (agent_id, response, None),
            GateOutcome::Maintenance {
                kind,
                session_key,
                receipt,
            } => {
                // 串行路径内联执行：⏳ 回执先发（会话已获取），干活，✓ 回执
                // 随后；返回空串让调用方的 finish_message 早退（不重复发布）。
                self.finish_message(&msg, receipt, None, false).await;
                let response = self.handle_maintenance(kind, &session_key).await;
                self.release_session(&session_key);
                self.finish_message(&msg, response, None, false).await;
                (String::new(), String::new(), None)
            }
            GateOutcome::UserDispatch {
                task_text,
                node_id,
                session_key,
            } => {
                // K4: 用户直发远程编码任务派发——串行路径内联执行（ACK 等
                // 待可阻塞泵，与 Maintenance 的 LLM 摘要同理由）。收据/快照/
                // 错误回复都在 process_user_dispatch 内部完成；返回空串让
                // 调用方的 finish_message 早退。
                self.process_user_dispatch(&msg, &task_text, &node_id, &session_key)
                    .await;
                (String::new(), String::new(), None)
            }
            GateOutcome::Ungated => self.process_ungated(&msg).await,
            GateOutcome::Admitted(admission) => self.process_admitted(&msg, admission).await,
        }
    }

    /// Turn preamble shared by every gated path (V5: extracted verbatim from
    /// the head of the pre-split `process_inbound_message`, so gate-time
    /// short-circuits — busy receipts, slash replies — behave exactly as
    /// before: the monolith opened the checkpoint turn and logged BEFORE any
    /// classification).
    ///
    /// E3：返回本消息 begin 的 checkpoint turn 序号（无 store 挂载时
    /// None）——gate 捕获后装进 [`TurnAdmission`]，随链穿针到行落盘点做
    /// `checkpoint_turn` 行标记（消息级回退的行→turn 定位锚）。
    fn turn_preamble(&self, msg: &nemesis_types::channel::InboundMessage) -> Option<usize> {
        // Open a checkpoint turn for the edit safety net (so writer-tool changes
        // during this message can be rewound). No-op when no store is attached.
        let cp_turn = self
            .turn_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let attached = {
            let cp = self.checkpoint_store.read().as_ref().cloned();
            if let Some(cp) = cp {
                cp.begin(cp_turn, &msg.content);
                true
            } else {
                false
            }
        };

        info!(
            "[AgentLoop] Processing message from {}:{}: {}",
            msg.channel,
            msg.sender_id,
            truncate(&msg.content, 80)
        );
        attached.then_some(cp_turn)
    }

    /// D3：drain 本 session 的 turn 文件变更缓冲（取走即清 + 按 path 去重，
    /// kind 取最后声明——投影规则见 `chat_log::dedup_file_changes`）。
    /// assistant 最终回复落盘前调用。
    fn drain_turn_file_changes(&self, session_key: &str) -> Vec<FileChange> {
        let drained = self
            .turn_file_changes
            .lock()
            .remove(session_key)
            .unwrap_or_default();
        crate::chat_log::dedup_file_changes(drained)
    }

    /// Resolve (agent_id, session_key) for a message (V5: extracted verbatim
    /// from the routing block of the pre-split `process_inbound_message`).
    fn route_message(&self, msg: &nemesis_types::channel::InboundMessage) -> (String, String) {
        // Resolve agent and session via route resolver.
        // Mirrors Go's processMessage: al.registry.ResolveRoute(RouteInput{...})
        let (agent_id, session_key) = if let Some(ref resolver) = self.route_resolver {
            // Build the routing input from message metadata, matching Go's extractPeer/extractParentPeer.
            let peer_kind = msg.metadata.get("peer_kind").cloned();
            let peer_id = msg.metadata.get("peer_id").cloned().or_else(|| {
                // Fallback: if peer_kind is "direct" use sender_id, else use chat_id
                if let Some(kind) = &peer_kind {
                    if kind == "direct" {
                        Some(msg.sender_id.clone())
                    } else {
                        Some(msg.chat_id.clone())
                    }
                } else {
                    None
                }
            });
            let parent_peer_kind = msg.metadata.get("parent_peer_kind").cloned();
            let parent_peer_id = msg.metadata.get("parent_peer_id").cloned();

            let route_input = RoutingRouteInput {
                channel: msg.channel.clone(),
                account_id: msg.metadata.get("account_id").cloned().unwrap_or_default(),
                peer_kind,
                peer_id,
                parent_peer_kind,
                parent_peer_id,
                guild_id: msg.metadata.get("guild_id").cloned(),
                team_id: msg.metadata.get("team_id").cloned(),
                identity_links: std::collections::HashMap::new(),
            };
            let route = resolver.resolve(&route_input);

            // Use routed session key, but honor pre-set agent-scoped keys
            // (mirrors Go's logic for ProcessDirect/cron).
            let session_key =
                if !msg.session_key.is_empty() && msg.session_key.starts_with("agent:") {
                    msg.session_key.clone()
                } else {
                    route.session_key.clone()
                };

            info!(
                "[AgentLoop] Routed message: agent_id={}, session_key={}, matched_by={}",
                route.agent_id, session_key, route.matched_by
            );

            (route.agent_id, session_key)
        } else {
            // Fallback when no route resolver is configured (standalone mode).
            let agent_id = self
                .registry
                .as_ref()
                .and_then(|r| r.default_agent_id())
                .unwrap_or_else(|| "main".to_string());

            let peer = extract_peer(msg);
            let session_key =
                if !msg.session_key.is_empty() && msg.session_key.starts_with("agent:") {
                    msg.session_key.clone()
                } else {
                    format!("{}:{}", msg.channel, peer)
                };

            info!(
                "[AgentLoop] Routed message (no resolver): agent_id={}, session_key={}",
                agent_id, session_key
            );

            (agent_id, session_key)
        };
        (agent_id, session_key)
    }

    /// Synchronous inbound gate (V5 / B5 真机揭的接线 bug，2026-08-23):
    /// 生产 pump（`run_bus_*`）原是纯串行消费者——`process_inbound_message`
    /// 把整个回合 await 到底才 recv 下一条消息，busy 闸门对用户消息永远
    /// 不可达（U7 inbox 因此在真机上从未生效，只有单测手动占住 session
    /// 验证过）。Queue/Steer 模式下 pump 现在同步跑这个闸门（同 session
    /// 保序：忙时的第二条消息必见 busy → 入队/回执），回合在独立 task
    /// 里并发跑；Reject 模式维持原串行路径（行为零变化）。
    ///
    /// 闸门只做同步分类 + session 获取；async 处理（system/history/回合
    /// 本体）留给 tail。slash 命令在这里同步执行并短路（原路径在 busy
    /// 检查前同步返回；且命令可能有副作用，tail 不得重跑）。
    fn gate_inbound(&self, msg: &nemesis_types::channel::InboundMessage) -> GateOutcome {
        // E3：begin 的 turn 序号在此捕获（admitted 链穿针用；其余 outcome
        // 不产生带 checkpoint_turn 标记的行，值自然丢弃）。
        let cp_turn = self.turn_preamble(msg);

        // Route system messages.
        if msg.channel == "system" {
            // Cluster continuation — the pump handles via dispatch_continuation.
            if msg
                .sender_id
                .starts_with(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
            {
                let task_id =
                    &msg.sender_id[nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX.len()..];
                debug!(
                    "[AgentLoop] Cluster continuation message intercepted, task_id={}",
                    task_id
                );
                return GateOutcome::Continuation(task_id.to_string());
            }
            // G4 (devtool-upgrade 阶段 3)：后台 subagent 完成回灌 —— 与集群
            // 续行同构：同走 dispatch_continuation → handle_cluster_continuation
            // （快照加载 + 续行 + 持久化 + 自清，全复用零新路径）。
            if msg
                .sender_id
                .starts_with(nemesis_types::constants::SUBAGENT_CONTINUATION_PREFIX)
            {
                let task_id =
                    &msg.sender_id[nemesis_types::constants::SUBAGENT_CONTINUATION_PREFIX.len()..];
                debug!(
                    "[AgentLoop] Background subagent continuation intercepted, task_id={}",
                    task_id
                );
                return GateOutcome::Continuation(task_id.to_string());
            }
            return GateOutcome::Ungated;
        }

        // History request.
        if let Some(request_type) = msg.metadata.get("request_type")
            && request_type == "history"
        {
            return GateOutcome::Ungated;
        }

        // E6 (2026-09-05): /compact /clear — 会话维护命令。gate 同步短路点
        // 拿不到 instance（摘要 LLM 是 async），所以 mint 一个 Maintenance
        // outcome 交给 tail；会话在此获取（维护期间并发用户消息走 busy
        // 闸），尾巴负责释放。先于普通 slash 检查（维护命令有副作用，不是
        // 静态回复）。
        if let Some((kind, receipt)) = parse_maintenance_command(&msg.content) {
            let (_, session_key) = self.route_message(msg);
            if !self.try_acquire_session(&session_key) {
                return GateOutcome::Immediate {
                    agent_id: String::new(),
                    response: "⏳ 会话正在处理消息，本次 /compact /clear 已忽略，请稍后再试"
                        .to_string(),
                };
            }
            return GateOutcome::Maintenance {
                kind,
                session_key,
                receipt,
            };
        }

        // K4 (devtool-upgrade 阶段 7): 用户直发远程编码任务派发——
        // `/build <task> repo:<node>`。先于 F1 精确匹配（带参形态与裸
        // `/build` 天然互斥）；会话在此获取（Maintenance 同款，防并发回
        // 合踩会话）。session_key 用 route_message 现取。
        if let Some((task_text, node_id)) = parse_user_dispatch(&msg.content) {
            let (_, session_key) = self.route_message(msg);
            if !self.try_acquire_session(&session_key) {
                return GateOutcome::Immediate {
                    agent_id: String::new(),
                    response: "⏳ 会话正在处理消息，本次远程派发已忽略，请稍后再试".to_string(),
                };
            }
            return GateOutcome::UserDispatch {
                task_text,
                node_id,
                session_key,
            };
        }

        // K4 (b) (devtool-upgrade 阶段 7): IM 审批卡回执短路——
        // `/approve <id>` / `/deny <id>`。真实裁决由 gateway 的审批回执
        // watcher（bus 订阅方）完成；loop 侧只做静态确认，**吞掉**这条
        // 消息不让 agent 对「同意 xxx」式回执起一轮无意义对话（裁决结果
        // 由 watcher 以显式消息回执到同一对话）。loop 无 pending 注册表，
        // 无效/过期 id 的诚实提示由 watcher 负责（「未找到待审批请求」）。
        if let Some(response) = parse_approval_reply(&msg.content) {
            return GateOutcome::Immediate {
                agent_id: String::new(),
                response,
            };
        }

        // F1 (devtool-upgrade 阶段 4): /plan /build — 模式切换命令。与普通
        // slash 同为 gate 同步短路（改的是 loop 级 RwLock，无 async），但要
        // 先于 handle_command_with_context（那是静态回复表，不带 chat_id，
        // 而模式切换要发布带路由信息的 ModeChanged 事件）。session_key 用
        // route_message 现取（/compact 臂同款）；无订阅者时空转是常态。
        if msg.content.trim() == "/plan" || msg.content.trim() == "/build" {
            let want = if msg.content.trim() == "/plan" {
                crate::types::AgentMode::Plan
            } else {
                crate::types::AgentMode::Build
            };
            let (agent_id, session_key) = self.route_message(msg);
            self.set_mode_with_event(want, &session_key, &msg.chat_id);
            let response = match want {
                crate::types::AgentMode::Plan => "✓ 已切换到 Plan 模式：文件修改类工具已停用（plans/ 目录写放行）。请以文本形式呈现计划；完成规划后用 /build 切回。".to_string(),
                crate::types::AgentMode::Build => "✓ 已切换回 Build 模式：全量工具恢复。".to_string(),
            };
            return GateOutcome::Immediate { agent_id, response };
        }

        // Slash commands.
        if let Some(response) = self.handle_command_with_context(&msg.content, &msg.channel) {
            return GateOutcome::Immediate {
                agent_id: String::new(),
                response,
            };
        }

        let (agent_id, session_key) = self.route_message(msg);

        // Session busy check — I1 (U7) mode-aware:
        //   Reject (default): legacy BUSY_MESSAGE bounce, byte-identical.
        //   Queue/Steer: park the message in the session inbox instead of
        //   bouncing. The running turn claims it (steer: next LLM call; queue:
        //   turn end starts a new one with the head). Capacity-bounded with a
        //   clear receipt so the rule is discoverable.
        if !self.try_acquire_session(&session_key) {
            match self.concurrent_mode {
                ConcurrentMode::Reject => {
                    warn!(
                        "[AgentLoop] Session busy, returning busy message: session_key={}, mode={:?}",
                        session_key, self.concurrent_mode
                    );
                    return GateOutcome::Immediate {
                        agent_id,
                        response: BUSY_MESSAGE.to_string(),
                    };
                }
                ConcurrentMode::Queue | ConcurrentMode::Steer => {
                    let queued = crate::inbox::QueuedMessage {
                        msg: nemesis_types::channel::InboundMessage {
                            channel: msg.channel.clone(),
                            sender_id: msg.sender_id.clone(),
                            chat_id: msg.chat_id.clone(),
                            content: msg.content.clone(),
                            media: msg.media.clone(),
                            session_key: msg.session_key.clone(),
                            correlation_id: msg.correlation_id.clone(),
                            metadata: msg.metadata.clone(),
                            voice_playback: msg.voice_playback,
                        },
                        timestamp: chrono::Local::now().to_rfc3339(),
                    };
                    // Second-pass review fix: the `!`-prefix steer channel
                    // exists ONLY in Steer mode. Queue mode is pure
                    // queueing — enqueue_for_mode routes everything to
                    // next_turn when steer is disabled.
                    match self.inbox.enqueue_for_mode(
                        &session_key,
                        queued,
                        self.concurrent_mode == ConcurrentMode::Steer,
                    ) {
                        crate::inbox::EnqueueOutcome::QueuedForNextTurn => {
                            info!(
                                "[AgentLoop] Session busy — message queued for next turn: session_key={}",
                                session_key
                            );
                            return GateOutcome::Immediate {
                                agent_id,
                                response: "⏳ 当前正在处理上一条消息。你的消息已排队，将在本轮结束后继续处理。".to_string(),
                            };
                        }
                        crate::inbox::EnqueueOutcome::QueuedForNextStep => {
                            info!(
                                "[AgentLoop] Session busy — steer message queued for in-turn injection: session_key={}",
                                session_key
                            );
                            return GateOutcome::Immediate {
                                agent_id,
                                response: "⚡ 已接收为紧急插话（消息以 ! 开头），将在 AI 的下一步思考前注入。非紧急消息请去掉 ! 前缀排队等待。".to_string(),
                            };
                        }
                        crate::inbox::EnqueueOutcome::Rejected => {
                            warn!(
                                "[AgentLoop] Session busy and inbox full — message refused: session_key={}",
                                session_key
                            );
                            return GateOutcome::Immediate {
                                agent_id,
                                response: "⏳ 排队已满，消息未能接收。请等当前任务完成后再发。"
                                    .to_string(),
                            };
                        }
                    }
                }
            }
        }

        // Create a cancellation token for this session (V5: minted in the
        // gate so the spawned turn tail starts with everything the
        // pre-split monolith had at this point).
        let cancel_token = self.create_cancel_token(&session_key);
        GateOutcome::Admitted(TurnAdmission {
            agent_id,
            session_key,
            cancel_token,
            cp_turn,
        })
    }

    /// Ungated tail: system (non-continuation) + history-request handling
    /// (V5: extracted verbatim from the pre-split monolith's early returns).
    async fn process_ungated(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, String, Option<String>) {
        if msg.channel == "system" {
            let (resp, err) = self.process_system_message(msg).await;
            return (String::new(), resp, err);
        }

        // History request.
        if let Some(request_type) = msg.metadata.get("request_type")
            && request_type == "history"
        {
            self.handle_history_request(msg).await;
            return (String::new(), String::new(), None);
        }

        // The gate classified everything else; unreachable in practice.
        (String::new(), String::new(), None)
    }

    /// Admitted turn tail (V5): preprocess → run turn → cleanup/release →
    /// inbox drain. The session is ALREADY acquired and the cancel token
    /// minted by `gate_inbound`; this fn owns release + reinject (the drain
    /// block runs unconditionally after the turn so an admitted session can
    /// never leak busy).
    async fn process_admitted(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
        admission: TurnAdmission,
    ) -> (String, String, Option<String>) {
        let TurnAdmission {
            agent_id,
            session_key,
            cancel_token,
            cp_turn,
        } = admission;

        let voice_playback = msg.voice_playback.unwrap_or(false);
        // I2（@文件引用）：内联被引用文件内容。相对路径以 workspace 根为基准
        // 解析（与 read_file 同源，不再是 cwd 漂移基准）；安全闸与图片附加
        // 同一套管线（挂真实 SecurityPlugin 时 Layer 7 block_in_place，
        // multi_thread runtime 为生产前提——同 T5 注释）。
        let at_base = self
            .workspace_root
            .read()
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let processed_content = crate::message_preprocess::expand_at_files(
            &msg.content,
            &at_base,
            &msg.channel,
            #[cfg(feature = "security")]
            self.security_plugin.as_deref(),
            #[cfg(not(feature = "security"))]
            None,
        );
        // T5（多模态，goal 2026-09-03）：统一图片附加——文本点名路径（T4 检测）
        // + media 引用（T7/T8/T9 落盘产物）统一过安全闸（8 层管线全跑）产出
        // **路径引用**；失败/拒绝诚实注明合并进轮次文本。注意：挂了真实
        // SecurityPlugin 时管线 Layer 7 会 block_in_place，须 multi_thread
        // runtime（gateway 生产 runtime 即是）。
        // T9：URL media 先经 fetch_url_media 预取（SSRF 闸 + http-pool 下载 →
        // uploads 落盘改写为本地路径引用），再统一走同步附加链；失败注记随
        // 附加注记一并合并进轮次文本。
        let ws_for_uploads = self.workspace_root.read().clone();
        let uploads_base =
            ws_for_uploads.unwrap_or_else(|| nemesis_path::default_path_manager().workspace());
        let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(&uploads_base);
        let (media_for_attach, url_notes) = crate::image_attach::fetch_url_media(
            &msg.media,
            &uploads_dir,
            #[cfg(feature = "security")]
            self.security_plugin.as_deref().and_then(|p| p.ssrf_guard()),
            #[cfg(not(feature = "security"))]
            None,
        )
        .await;
        // J6：超限图片降采样开关（默认开）——产物落同一 uploads 目录（TTL 复用）。
        let downscale_dir = self.current_image_downscale().then(|| uploads_dir.clone());
        let attach = crate::image_attach::attach_turn_images(
            &processed_content,
            &media_for_attach,
            self.workspace_root.read().as_deref(),
            downscale_dir.as_deref(),
            &msg.channel,
            #[cfg(feature = "security")]
            self.security_plugin.as_deref(),
            #[cfg(not(feature = "security"))]
            None,
        );
        let processed_content = attach.merge_into_text(processed_content);
        let processed_content = crate::image_attach::AttachOutcome {
            attached: Vec::new(),
            notes: url_notes,
        }
        .merge_into_text(processed_content);
        let image_refs = attach.ref_strings();
        let cron_job_id = msg.metadata.get("cron_job_id").map(|s| s.as_str());
        let cron_job_name = msg.metadata.get("cron_job_name").map(|s| s.as_str());
        // T3 (U12): per-fire tool-round budget set by the gateway cron fire
        // handler from the job's max_rounds payload. Absent/unparsable → None
        // → the turn runs under the global max_turns.
        let cron_max_rounds = msg
            .metadata
            .get("cron_max_rounds")
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|v| *v > 0);
        // I5：本轮客户端上报的打开文件进 per-turn 状态（渲染在 build_messages
        // 的 merged digest；出轮即清——与下方 run 后的 clear 词法配对，cron/
        // heartbeat/continuation 等非 process_admitted 路径天然读不到）。
        *self.pending_open_files.write() =
            nemesis_types::channel::open_files_from_metadata(&msg.metadata);
        let result = self
            .run_agent_loop_internal(
                &session_key,
                &processed_content,
                &msg.channel,
                &msg.chat_id,
                voice_playback,
                &cancel_token,
                cron_job_id,
                cron_job_name,
                cron_max_rounds,
                &image_refs,
                cp_turn,
            )
            .await;

        // Clean up cancellation token and release session.
        self.remove_cancel_token(&session_key);
        self.release_session(&session_key);
        // I5：轮结束清打开文件状态（与上方 set 词法配对，防跨轮陈旧泄漏）。
        self.pending_open_files.write().clear();

        // I1 (U7) post-turn inbox handling:
        //   - Unconsumed next-step (steer) messages ALWAYS transfer back to
        //     the next-turn queue — both for cancelled turns AND for turns
        //     that finished normally with a steer arriving too late for the
        //     escape hatch (second-pass review fix: the cancelled-only
        //     transfer left stale steers in next_step, which a LATER turn
        //     would claim out of context). Transferred messages become the
        //     next turn's input — nothing is lost, nothing arrives stale.
        //   - Completed turn with queued next-turn messages: requeue the head
        //     through the normal inbound path (fresh busy acquire),
        //     serialized behind this turn because the session was already
        //     released.
        self.inbox.transfer_next_step_to_next_turn(&session_key);
        if matches!(
            self.concurrent_mode,
            ConcurrentMode::Queue | ConcurrentMode::Steer
        ) {
            if let Some(head) = self.inbox.claim_next_turn_head(&session_key) {
                info!(
                    "[AgentLoop] processing queued next-turn message: session_key={}, len={}",
                    session_key,
                    head.msg.content.len()
                );
                // ROUND-5 REVIEW FIX: re-inject the queued head into the
                // agent's own inbound mpsc (the same one `run_bus_*`
                // consumes) instead of recursing inline. The normal consumer
                // then applies the SAME post-processing as any other message:
                // [rpc:{cid}] correlation prefix, sent_in_round check+clear,
                // error → "Error processing message" + capture flush, and
                // meta.model. The previous inline recursion published the
                // reply raw — an rpc-channel queued reply lost its prefix
                // (RPCChannel::send drops unprefixed replies) and a FAILED
                // queued turn produced no user-visible output at all.
                // H3 (full review): the queued message wraps the ORIGINAL
                // InboundMessage whole (round-5), so replaying is a clone
                // with only the session_key normalized to this queue's key.
                //
                // NOT bus.publish_inbound (broadcast): that would re-match
                // workflow message-triggers for an already-matched message
                // (double firing). The agent's private mpsc has no other
                // subscriber.
                //
                // try_send (never a blocking await on a full channel from
                // inside the sole consumer task): capacity 1024 vs inbox cap
                // 8/session makes a full channel unreachable in practice; on
                // the theoretical overflow the message is logged and dropped
                // rather than deadlocking the loop.
                let mut inbound = head.msg.clone();
                inbound.session_key = session_key.to_string();
                let reinject = self.reinject_tx.read().clone();
                match reinject {
                    Some(tx) => {
                        if let Err(e) = tx.try_send(inbound) {
                            // No consumer (standalone/tests without run_bus)
                            // or channel full — surface loudly either way.
                            warn!(
                                "[AgentLoop] failed to re-inject queued message (session_key={}, err={}) — message dropped",
                                session_key, e
                            );
                        }
                    }
                    None => {
                        // Standalone mode / tests without the mpsc wired:
                        // process inline as before so the message is not
                        // silently lost, with the error path at least
                        // converted to a user-visible reply.
                        warn!(
                            "[AgentLoop] reinject_tx not set — processing queued message inline (post-processing skipped), session_key={}",
                            session_key
                        );
                        let fut = Box::pin(self.process_inbound_message(&inbound));
                        let (_id2, resp2, err2) = fut.await;
                        let content = match err2 {
                            Some(e) => format!("Error processing message: {}", e),
                            None => resp2,
                        };
                        if !content.is_empty()
                            && let Some(ref tx) = self.outbound_tx
                        {
                            let outbound = nemesis_types::channel::OutboundMessage {
                                channel: head.msg.channel.clone(),
                                chat_id: head.msg.chat_id.clone(),
                                content,
                                message_type: String::new(),
                                meta: nemesis_types::channel::OutboundMeta {
                                    model: None,
                                    session_key: Some(session_key.clone()),
                                },
                            };
                            let _ = tx.send(outbound).await;
                        }
                    }
                }
            }
            // Drop empty queues (housekeeping; keeps the map bounded).
            if self.inbox.pending(&session_key) == (0, 0) {
                self.inbox.clear(&session_key);
            }
        }

        match result {
            Ok(response) => (agent_id, response, None),
            Err(e) => (agent_id, String::new(), Some(e)),
        }
    }

    // -----------------------------------------------------------------------
    // System message routing
    // -----------------------------------------------------------------------

    /// Process a system message.
    /// Mirrors Go's `processSystemMessage()`.
    async fn process_system_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, Option<String>) {
        if msg.channel != "system" {
            return (
                String::new(),
                Some(format!(
                    "processSystemMessage called with non-system channel: {}",
                    msg.channel
                )),
            );
        }

        info!(
            "[AgentLoop] Processing system message: sender_id={}, chat_id={}",
            msg.sender_id, msg.chat_id
        );

        // Parse origin channel from chat_id (format: "channel:chat_id").
        let (origin_channel, origin_chat_id) = if let Some(idx) = msg.chat_id.find(':') {
            (&msg.chat_id[..idx], msg.chat_id[idx + 1..].to_string())
        } else {
            ("cli", msg.chat_id.clone())
        };

        // Skip internal channels.
        if is_internal_channel(origin_channel) {
            info!(
                "[AgentLoop] Subagent completed (internal channel): content_len={}",
                msg.content.len()
            );
            return (String::new(), None);
        }

        // Use default agent session key.
        let session_key = build_agent_main_session_key("main");

        // Extract subagent result from message content.
        // Format: "Task 'label' completed.\n\nResult:\n<actual content>"
        // Mirrors Go's: if idx := strings.Index(content, "Result:\n"); idx >= 0 { content = content[idx+8:] }
        let content = if let Some(idx) = msg.content.find("Result:\n") {
            &msg.content[idx + 8..]
        } else {
            &msg.content
        };

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cron_job_id = msg.metadata.get("cron_job_id").map(|s| s.as_str());
        let cron_job_name = msg.metadata.get("cron_job_name").map(|s| s.as_str());
        let result = self
            .run_agent_loop_internal(
                &session_key,
                &format!("[System: {}] {}", msg.sender_id, content),
                origin_channel,
                &origin_chat_id,
                false,
                &cancel_token,
                cron_job_id,
                cron_job_name,
                None,
                &[],
                None, // E3：system 直调不经 gate/preamble，无 checkpoint 标记
            )
            .await;

        match result {
            Ok(response) => (response, None),
            Err(e) => (String::new(), Some(e)),
        }
    }

    // -----------------------------------------------------------------------
    // History request handling
    // -----------------------------------------------------------------------

    /// Handle a history request by reading from session and publishing response.
    /// Mirrors Go's `handleHistoryRequest()`.
    async fn handle_history_request(&self, msg: &nemesis_types::channel::InboundMessage) {
        #[derive(Deserialize)]
        struct HistoryRequest {
            #[serde(default)]
            request_id: String,
            #[serde(default)]
            limit: Option<usize>,
            before_index: Option<usize>,
        }

        let req: HistoryRequest = match serde_json::from_str(&msg.content) {
            Ok(r) => r,
            Err(e) => {
                error!("[AgentLoop] Failed to parse history request: {}", e);
                self.publish_history_response(
                    &msg.chat_id,
                    "",
                    &Vec::<serde_json::Value>::new(),
                    false,
                    0,
                    0,
                )
                .await;
                return;
            }
        };

        let limit = req.limit.unwrap_or(20);
        let agent_id = self
            .registry
            .as_ref()
            .and_then(|r| r.default_agent_id())
            .unwrap_or_else(|| "main".to_string());
        // Multi-session: if the client sent a session_id, derive
        // `agent:main:session:{sid}` (agent: prefix → adopted by loop.rs:1623,
        // bypasses routing). Otherwise fall back to the default "legacy"
        // conversation — MUST match server.rs process_messages' fallback so
        // history-read and chat-write share the same key (else the default
        // conversation's history wouldn't reload).
        let session_key = match msg.metadata.get("session_id") {
            Some(sid) if !sid.is_empty() => format!(
                "agent:main:session:{}",
                crate::session::SessionStore::sanitize_session_id(sid)
            ),
            _ => format!("agent:{}:session:legacy", agent_id),
        };

        // Read history from chat log (separate from session store).
        let (page, total_count, has_more, oldest_index) =
            crate::chat_log::read_chat_log(&session_key, limit, req.before_index);

        self.publish_history_response(
            &msg.chat_id,
            &req.request_id,
            &page,
            has_more,
            oldest_index,
            total_count,
        )
        .await;
    }

    /// Publish a history response via the outbound channel.
    /// Mirrors Go's `publishHistoryResponse()`.
    async fn publish_history_response(
        &self,
        chat_id: &str,
        request_id: &str,
        messages: &[serde_json::Value],
        has_more: bool,
        oldest_index: usize,
        total_count: usize,
    ) {
        let response_data = serde_json::json!({
            "request_id": request_id,
            "messages": messages,
            "has_more": has_more,
            "oldest_index": oldest_index,
            "total_count": total_count,
        });

        let content = match serde_json::to_string(&response_data) {
            Ok(c) => c,
            Err(e) => {
                error!("[AgentLoop] Failed to marshal history response: {}", e);
                return;
            }
        };

        if let Some(ref tx) = self.outbound_tx {
            let outbound = nemesis_types::channel::OutboundMessage {
                channel: "web".to_string(),
                chat_id: chat_id.to_string(),
                content,
                message_type: "history".to_string(),
                meta: Default::default(),
            };
            if let Err(e) = tx.send(outbound).await {
                warn!("[AgentLoop] Failed to send history response: {}", e);
            }
        } else {
            warn!("[AgentLoop] publish_history_response: no outbound_tx available");
        }

        debug!(
            "[AgentLoop] History response published: chat_id={}, request_id={}, total_count={}, has_more={}",
            chat_id, request_id, total_count, has_more
        );
    }

    // -----------------------------------------------------------------------
    // State recording
    // -----------------------------------------------------------------------

    /// Record the last active channel for crash recovery.
    /// Mirrors Go's `state.Manager.SetLastChannel()`.
    pub fn record_last_channel(&self, channel: &str) {
        if let Some(ref mgr) = self.state_manager
            && let Err(e) = mgr.set_last_channel(channel)
        {
            tracing::warn!("[AgentLoop] Failed to persist last channel: {}", e);
        }
    }

    /// Record the last active chat ID for crash recovery.
    /// Mirrors Go's `state.Manager.SetLastChatID()`.
    pub fn record_last_chat_id(&self, chat_id: &str) {
        if let Some(ref mgr) = self.state_manager
            && let Err(e) = mgr.set_last_chat_id(chat_id)
        {
            tracing::warn!("[AgentLoop] Failed to persist last chat ID: {}", e);
        }
    }

    // -----------------------------------------------------------------------
    // Session busy state management
    // -----------------------------------------------------------------------

    /// Get or create the busy state for a session.
    /// Mirrors Go's `getSessionBusyState()`.
    pub fn get_session_busy_state(&self, session_key: &str) -> (bool, usize) {
        let map = self.session_busy.lock();
        match map.get(session_key) {
            Some(state) => (state.busy, state.queue_length),
            None => (false, 0),
        }
    }

    /// Try to acquire a session for processing.
    /// Returns true if acquired, false if busy (and queue is full in queue mode).
    /// Mirrors Go's `tryAcquireSession()`.
    pub fn try_acquire_session(&self, session_key: &str) -> bool {
        let mut map = self.session_busy.lock();
        let state = map.entry(session_key.to_string()).or_default();

        if !state.busy {
            state.busy = true;
            return true;
        }

        // Session is busy — reject HERE. Queueing is NOT done at this layer:
        // the old Queue path incremented queue_length WITHOUT storing the
        // message, and release_session kept busy while queue_length>0 → the
        // session could deadlock (no turn ever acquires to drain the
        // counter). Busy-queueing lives in `crate::inbox` (FIFO per session),
        // driven by `gate_inbound`'s busy branch — reachable since V5's
        // gate-in-pump restructure (the gate runs before the turn task is
        // spawned, so a busy session is detected while its turn is still
        // running). (queue_length stays 0, so release_session naturally sets
        // busy=false.)
        let _ = self.concurrent_mode;
        false
    }

    /// Release a session after processing.
    /// Returns true if there are queued requests remaining.
    /// Mirrors Go's `releaseSession()`.
    pub fn release_session(&self, session_key: &str) -> bool {
        let mut map = self.session_busy.lock();
        if let Some(state) = map.get_mut(session_key) {
            if state.queue_length > 0 {
                state.queue_length -= 1;
                // Keep busy since there are queued requests.
                return true;
            }
            state.busy = false;
        }
        false
    }

    /// Check whether a session is currently busy.
    pub fn is_session_busy(&self, session_key: &str) -> bool {
        let map = self.session_busy.lock();
        map.get(session_key).is_some_and(|s| s.busy)
    }

    /// Get the queue length for a session.
    pub fn session_queue_length(&self, session_key: &str) -> usize {
        let map = self.session_busy.lock();
        map.get(session_key).map_or(0, |s| s.queue_length)
    }

    /// U7 dashboard visibility (G1): one-call snapshot of a session's inbox
    /// state for the `agent.inbox_status` WSAPI command. Read-only — claims
    /// and transfers stay exclusively in the turn lifecycle paths.
    pub fn inbox_status(&self, session_key: &str) -> crate::inbox::InboxStatus {
        let (next_turn, next_step) = self.inbox.pending(session_key);
        crate::inbox::InboxStatus {
            next_turn,
            next_step,
            capacity: self.inbox.capacity(),
            busy: self.is_session_busy(session_key),
            mode: match self.concurrent_mode {
                ConcurrentMode::Reject => "reject",
                ConcurrentMode::Queue => "queue",
                ConcurrentMode::Steer => "steer",
            },
        }
    }

    // -----------------------------------------------------------------------
    // Session cancellation
    // -----------------------------------------------------------------------

    /// Cancel an in-progress session by session_key.
    ///
    /// If the session is currently being processed by the LLM loop, this
    /// triggers the cancellation token, causing the loop to break at the
    /// next check point (after the current LLM call or tool execution).
    ///
    /// Returns true if a cancellation token was found and cancelled.
    pub fn cancel_session(&self, session_key: &str) -> bool {
        if let Some(token) = self.cancel_tokens.get(session_key) {
            token.cancel();
            info!(
                "[AgentLoop] Session cancellation requested: {}",
                session_key
            );
            true
        } else {
            debug!("[AgentLoop] No active session to cancel: {}", session_key);
            false
        }
    }

    /// Cancel all in-progress sessions.
    ///
    /// Returns the number of sessions that were cancelled.
    pub fn cancel_all_sessions(&self) -> usize {
        let mut count = 0;
        for entry in self.cancel_tokens.iter() {
            entry.value().cancel();
            count += 1;
        }
        if count > 0 {
            info!("[AgentLoop] Cancelled {} active session(s)", count);
        }
        count
    }

    /// Create and store a cancellation token for a session.
    /// Returns the token for the caller to pass into the processing pipeline.
    fn create_cancel_token(&self, session_key: &str) -> tokio_util::sync::CancellationToken {
        let token = tokio_util::sync::CancellationToken::new();
        self.cancel_tokens
            .insert(session_key.to_string(), token.clone());
        token
    }

    /// Remove the cancellation token for a session after processing completes.
    fn remove_cancel_token(&self, session_key: &str) {
        self.cancel_tokens.remove(session_key);
    }

    // -----------------------------------------------------------------------
    // Summarization
    // -----------------------------------------------------------------------

    /// Advance the summary cache if the verbatim tail is over the context
    /// threshold.
    ///
    /// Inline-summarization pipeline (replaces Go's `maybeSummarize`). The
    /// summary cache covers `history[..covers_up_to]`; `build_messages` sends
    /// `history[covers_up_to..]` verbatim. Token pressure is therefore on the
    /// *tail* (what the LLM actually receives), so the threshold is evaluated
    /// against the tail, not the full history. When the tail exceeds the
    /// threshold and is longer than `K_TARGET`, the cache advances to
    /// `covers_up_to = len - K_TARGET` and the newly-covered prefix is folded
    /// into the summary. History is never mutated (append-only); bounding is
    /// the session store's job.
    ///
    /// Persistence: this updates the in-memory cache on the instance. The save
    /// path persists the cache alongside the full history (see S3.3).
    async fn maybe_update_summary(
        &self,
        instance: &AgentInstance,
        session_key: &str,
        channel: &str,
        chat_id: &str,
    ) {
        let history = instance.get_history();
        // U16 (sixth batch) + N1 (devtool-upgrade 阶段 1): prefer the active
        // model's context_window via the three-tier chain (config explicit →
        // pricing catalog → instance default, now 128_000) over the
        // historical 32000. Falls back to the instance value when
        // unset/standalone.
        let context_window = self
            .current_context_window()
            .unwrap_or_else(|| instance.context_window());

        // C indexes the full history (system prompt at index 0); the verbatim
        // tail build_messages sends is history[C..]. Clamp to history length
        // for safety against a stale cache index.
        let cache = instance.get_summary_cache();
        let c = cache
            .as_ref()
            .map(|c| c.covers_up_to)
            .filter(|&c| c >= 1)
            .unwrap_or(0)
            .min(history.len());
        let existing_summary = cache.as_ref().map(|c| c.text.as_str()).unwrap_or("");

        // Token pressure is on the tail (system + summary + history[C..] is
        // what build_messages emits). The covered prefix is already folded into
        // the summary, so it does not count toward pressure. X1: measured over
        // the MODEL-FACING projection — history keeps tool originals since the
        // size gates moved to build_messages, so the raw estimate would count
        // a 70KB original the provider only ever sees as a bounded locator.
        let tail_tokens = estimate_tokens_for_turns_projected(&history[c..]);
        let tail_len = history.len().saturating_sub(c);
        let soft = context_window * COMPACT_SOFT_RATIO / 100;
        let threshold = context_window * COMPACT_SUMMARIZE_RATIO / 100;
        // Summarize runs only when the tail is over threshold AND long enough to
        // shrink (more than K_TARGET messages past C). A short tail that is huge
        // in tokens (a large system prompt or an early oversized tool result)
        // can't be helped by advancing C — leave it to force_compression.
        let will_summarize = tail_tokens >= threshold && tail_len > K_TARGET;

        // ⑩ Graded tiers (soft / summarize) + stuck self-check on the tail.
        // Soft is info-log-only. The stuck counter only ticks when summarize
        // will ACTUALLY run — otherwise a chronically-over-threshold tail that
        // is too short to summarize (e.g. a big system prompt with few turns)
        // would tick the counter without ever attempting a summarize and pause
        // summarization before it gets the chance.
        let mut paused_stuck = false;
        {
            let mut states = self.compact_state.lock();
            if states.len() > SESSION_STATE_MAX_ENTRIES {
                states.clear();
            }
            let st = states.entry(session_key.to_string()).or_default();

            if tail_tokens >= soft && !st.soft_noticed {
                st.soft_noticed = true;
                info!(
                    "[AgentLoop] context tail at ~{}% of window ({} / {}); summarization will trigger at {}%",
                    tail_tokens * 100 / context_window.max(1),
                    tail_tokens,
                    context_window,
                    COMPACT_SUMMARIZE_RATIO
                );
            }

            if will_summarize {
                if summarize_was_ineffective(st.last_summary_tokens, tail_tokens) {
                    st.consecutive_failures += 1;
                } else {
                    st.consecutive_failures = 0;
                }
                if st.consecutive_failures >= COMPACT_STUCK_LIMIT {
                    if !st.stuck {
                        warn!(
                            "[AgentLoop] compaction stuck: summarization has not reduced the tail {} times in a row; pausing auto-summarization (raise context_window or reduce tool output)",
                            st.consecutive_failures
                        );
                        st.stuck = true;
                    }
                    paused_stuck = true;
                } else {
                    st.last_summary_tokens = tail_tokens;
                }
            } else if tail_tokens < threshold {
                // Breathing room — clear the stuck latch.
                st.consecutive_failures = 0;
                st.stuck = false;
            }
        }
        if paused_stuck {
            return;
        }

        if !will_summarize {
            return;
        }

        let summarize_key = format!("main:{}", session_key);
        {
            let mut map = self.summarizing.lock();
            if map.len() > SESSION_STATE_MAX_ENTRIES {
                map.clear();
            }
            if map.contains_key(&summarize_key) {
                return;
            }
            map.insert(summarize_key.clone(), true);
        }

        // New boundary: cover everything except the last K_TARGET messages
        // (the verbatim tail kept for continuity). new_C > c is guaranteed by
        // the tail_len > K_TARGET check above, so we always advance and never
        // re-summarize the same prefix. Adjust so the tail doesn't start mid
        // tool_call/result pair (keeps the pair verbatim, not dropped by repair).
        let new_c = tool_safe_boundary(&history, history.len() - K_TARGET);

        let provider = self.provider.read().clone();
        let model = self.active_model.read().clone();
        let outbound_tx = self.outbound_tx.clone();
        let summarizing_flag = self.summarizing.clone();
        let observer_mgr = self.observer_manager.clone();
        let channel_owned = channel.to_string();
        let chat_id_owned = chat_id.to_string();
        let clear_key = summarize_key.clone();

        if !is_internal_channel(&channel_owned)
            && let Some(ref tx) = outbound_tx
        {
            let outbound = nemesis_types::channel::OutboundMessage {
                channel: channel_owned.clone(),
                chat_id: chat_id_owned.clone(),
                content: "Memory threshold reached. Optimizing conversation history...".to_string(),
                message_type: String::new(),
                meta: nemesis_types::channel::OutboundMeta {
                    model: None,
                    session_key: Some(clear_key.clone()),
                },
            };
            let _ = tx.send(outbound).await;
        }

        // 方言 PreCompact（观察型，2026-08-29 三段化扩展）：exit 2 不阻止压缩
        // （稳定性机制）。先 clone Arc 再 await（不持锁跨 await）。
        let bridge_pre = self.cc_bridge.read().as_ref().cloned();
        if let Some(bridge) = bridge_pre {
            bridge.run_compact_hooks("auto", "pre").await;
        }

        // Fold the prefix history[..new_c] into the summary, merged with the
        // existing summary (which already covers history[..c]). summarize the
        // FULL prefix from source each time (no "keep last N" — that would
        // leave a gap between the summary and the verbatim tail).
        let prefix_refs: Vec<&crate::types::ConversationTurn> = history[..new_c].iter().collect();
        let summary = summarize_prefix_owned(
            &prefix_refs,
            existing_summary,
            context_window,
            self.current_summarizer_prefix_reuse(),
            provider.as_ref(),
            &model,
            observer_mgr,
        )
        .await;

        if let Some(summary) = summary {
            instance.set_summary_cache(Some(crate::instance::SummaryCache {
                covers_up_to: new_c,
                text: summary,
            }));
        } else {
            // 2026-08-25 摘要静默失败修复：失败（或无可摘要内容）时绝不推进
            // covers —— 否则被折叠的上下文会静默丢失。Loud warn，不静默。
            warn!(
                "[AgentLoop] auto-summarization produced no summary for {} (LLM failure or no valid content); keeping full history, covers_up_to unchanged",
                session_key
            );
        }

        // 方言 PostCompact（观察型）：压缩尝试结束（成败皆触发）。
        let bridge_post = self.cc_bridge.read().as_ref().cloned();
        if let Some(bridge) = bridge_post {
            bridge.run_compact_hooks("auto", "post").await;
        }

        {
            let mut map = summarizing_flag.lock();
            map.remove(&clear_key);
        }
    }

    /// Force-compress by aggressively advancing the summary cache.
    ///
    /// Last-resort path used when the LLM reports a context error and the
    /// caller retries. Does NOT mutate history (history is append-only); it
    /// shrinks what `build_messages` emits by advancing `covers_up_to` and
    /// recomputing the summary over the larger covered prefix.
    ///
    /// Progressive: each call shrinks the verbatim tail further. The first call
    /// reduces the tail to [`SMALL_K_FORCE`] messages; if that still isn't
    /// enough (the caller will retry), the next call folds everything into the
    /// summary (tail → 0). Bounded by the caller's retry limit.
    pub async fn force_compression(&self, instance: &AgentInstance) {
        self.force_compression_opts(instance, false).await;
    }

    /// N2：`force_compression` 的参数化形态——`prefer_small=true` 时手动
    /// 入口（E6 `/compact`）优先用 `agents.small_model` 跑摘要；自动压缩
    /// 路径维持 `false`（主模型，质量敏感不降档）。
    pub async fn force_compression_opts(&self, instance: &AgentInstance, prefer_small: bool) {
        let history = instance.get_history();
        let cache = instance.get_summary_cache();
        let current_c = cache
            .as_ref()
            .map(|c| c.covers_up_to)
            .filter(|&c| c >= 1)
            .unwrap_or(0)
            .min(history.len());
        let existing_summary = cache.as_ref().map(|c| c.text.as_str()).unwrap_or("");

        // Shrink the verbatim tail: to SMALL_K_FORCE if it's still large,
        // otherwise cover everything (tail → 0) as the final resort.
        let current_tail_len = history.len().saturating_sub(current_c);
        let raw_c = if current_tail_len > SMALL_K_FORCE {
            history.len() - SMALL_K_FORCE
        } else {
            history.len()
        };
        // Keep tool_call/result pairs intact in the tail (don't let the boundary
        // drop an orphan result that the summary can't capture).
        let new_c = tool_safe_boundary(&history, raw_c);
        // Must advance and have a non-empty prefix to summarize.
        if new_c <= current_c || new_c == 0 {
            return;
        }

        let (provider, model) = self.resolve_summary_provider(prefer_small);
        let observer_mgr = self.observer_manager.clone();

        // Fold the prefix history[..new_c] into the summary, merged with the
        // existing summary (which covers history[..current_c]).
        let prefix_refs: Vec<&crate::types::ConversationTurn> = history[..new_c].iter().collect();
        let summary = summarize_prefix_owned(
            &prefix_refs,
            existing_summary,
            // U16: per-model context_window when declared (same preference
            // order as the threshold computation above).
            self.current_context_window()
                .unwrap_or_else(|| instance.context_window()),
            // T4: per-model prefix-reuse switch (false → old bare shape).
            self.current_summarizer_prefix_reuse(),
            provider.as_ref(),
            &model,
            observer_mgr,
        )
        .await;

        if let Some(summary) = summary {
            instance.set_summary_cache(Some(crate::instance::SummaryCache {
                covers_up_to: new_c,
                text: summary,
            }));
            info!(
                "[AgentLoop] Force-compressed: covers_up_to {} -> {} (verbatim tail {} -> {} messages)",
                current_c,
                new_c,
                current_tail_len,
                history.len() - new_c
            );
        } else {
            // 2026-08-25 摘要静默失败修复：force 路径同样绝不推进 covers。
            // 调用方的有界重试会放弃并向上报错（响亮失败优于静默失忆）。
            warn!(
                "[AgentLoop] force-compression produced no summary (LLM failure or no valid content); covers_up_to stays {}, the caller's bounded retry will surface the error",
                current_c
            );
        }
    }

    // -----------------------------------------------------------------------
    // E6: manual session maintenance (/compact /clear)
    // -----------------------------------------------------------------------

    /// Dispatch a maintenance command to its implementation (shared by the
    /// serial tail and the spawned pump task).
    async fn handle_maintenance(&self, kind: SessionMaintenance, session_key: &str) -> String {
        match kind {
            SessionMaintenance::Compact => match self.compact_session(session_key).await {
                Ok(receipt) => receipt,
                Err(e) => format!("⚠ 压缩未完成：{e}"),
            },
            SessionMaintenance::Clear => match self.clear_session(session_key).await {
                Ok(receipt) => receipt,
                Err(e) => format!("⚠ 清空未完成：{e}"),
            },
        }
    }

    // -----------------------------------------------------------------------
    // K4 (devtool-upgrade 阶段 7): 用户直发远程编码任务派发
    // -----------------------------------------------------------------------

    /// 处理 `/build <task> repo:<node>` 用户直派发（serial tail 与 spawned
    /// pump task 共用）。会话已在 gate 获取；本 fn 结束前负责释放。
    ///
    /// 流程：
    /// ① ⏳ 收据先发（ACK 等待可达分钟级，不晾用户）；
    /// ② 会话一致性——user 行落 chat_log + session store（原命令原文），
    ///    续行端只追加 assistant 行，两半对称；
    /// ③ 合成 `cluster_rpc` 工具调用走 `handle_tool_call` 全管线——estop /
    ///    hidden / Plan 闸 / 安全 8 层 / guardian 全部生效（9 层安全零降级，
    ///    这是直接调工具对象被否决的原因）；
    /// ④ `__ASYNC__` ACK → 存自足续行快照（`build_dispatch_snapshot_messages`
    ///    合成最小合法序列），完成回调复用 `handle_cluster_continuation`
    ///    全套（续行 LLM / 持久化 / 回原通道 / G5 崩溃恢复）；
    /// ⑤ 同步结果或 Err（集群未启用 / 节点不存在 / 自调用守卫）→ 直接回复。
    async fn process_user_dispatch(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
        task_text: &str,
        node_id: &str,
        session_key: &str,
    ) {
        // ① 收据先发。
        let receipt = format!("⏳ 已把编码任务派发给节点 {node_id}，完成后自动回复结果。");
        self.finish_message(msg, receipt, None, false).await;

        // ② 会话一致性：user 行落盘（原命令原文——派发上下文可追溯）。
        crate::chat_log::append_chat_log_meta(
            session_key,
            "user",
            &msg.content,
            &crate::chat_log::ChatLogMeta {
                model: None,
                cron_job_id: None,
                cron_job_name: None,
                images: &[],
                file_changes: &[],
                checkpoint_turn: None,
            },
        );
        if let Some(ref store) = self.session_store {
            store.add_message(session_key, "user", &msg.content);
        }

        // ③ 合成工具调用 → 全管线提交。
        let tc_id = format!("ud-{}", uuid::Uuid::new_v4().simple());
        let tool_call = ToolCallInfo {
            id: tc_id.clone(),
            name: "cluster_rpc".to_string(),
            arguments: serde_json::json!({
                "target": node_id,
                "message": task_text,
            })
            .to_string(),
        };
        let context = RequestContext::new(&msg.channel, &msg.chat_id, &msg.sender_id, session_key);
        let result = self.handle_tool_call(&tool_call, &context).await;

        // ④ ACK → 存自足续行快照。
        if let Some(rest) = result.strip_prefix("__ASYNC__:") {
            let parts: Vec<&str> = rest.splitn(4, ':').collect();
            if parts.len() >= 2 {
                let task_id = parts[0];
                let target_id = parts[1];
                if let Some(ref mgr) = self.continuation_manager {
                    let messages = build_dispatch_snapshot_messages(task_text, node_id, &tc_id);
                    // G5：对端 ID 随快照落盘——A 侧重启后恢复轮询知道问谁。
                    mgr.save_continuation_with_images(
                        task_id,
                        messages,
                        &tc_id,
                        &msg.channel,
                        &msg.chat_id,
                        session_key,
                        target_id,
                        &[],
                    )
                    .await;
                    info!(
                        "[AgentLoop] User dispatch accepted: task_id={}, target={}, session={}",
                        task_id, target_id, session_key
                    );
                } else {
                    // 无续行管理器（测试/独立模式）：结果无法回灌，诚实告知。
                    warn!(
                        "[AgentLoop] User dispatch ACK but no continuation manager; result cannot be delivered"
                    );
                    self.finish_message(
                        msg,
                        "⚠ 任务已提交但本实例不支持续行回灌（无 continuation manager），结果无法自动回复。".to_string(),
                        None,
                        true,
                    )
                    .await;
                }
                self.release_session(session_key);
                return;
            }
            // 格式异常（段数不足）→ 落到下方按普通文本回复。
        }

        // ⑤ 同步结果或错误：直接回复（handle_tool_call 把错误编码在返回串）。
        self.finish_message(msg, result, None, true).await;
        self.release_session(session_key);
    }

    /// E6: 手动 compaction 入口。取（从 store 重建的）instance，调
    /// `force_compression` 推进摘要覆盖，再把新摘要持久化回 store（顺序同
    /// 回合末：先 summary+covers 后 history，见回合末块注释）。
    ///
    /// 成功回执带覆盖数（摘要覆盖前 N 条，保留近 M 条）；摘要 LLM 失败或
    /// 无可压缩内容时返回 Err（covers 不推进——2026-08-25 静默失忆修复的
    /// 同一契约），历史保持不变。
    pub async fn compact_session(&self, session_key: &str) -> Result<String, String> {
        let instance = self.get_or_create_instance(session_key);
        let history_len = instance.get_history().len();
        if history_len == 0 {
            return Err("会话为空，无需压缩".to_string());
        }
        let before = instance
            .get_summary_cache()
            .map(|c| c.covers_up_to)
            .unwrap_or(0);

        // 方言 PreCompact（观察型）：手动压缩尝试开始。
        let bridge_pre = self.cc_bridge.read().as_ref().cloned();
        if let Some(bridge) = bridge_pre {
            bridge.run_compact_hooks("manual", "pre").await;
        }

        // N2：手动 /compact 是 `agents.small_model` 的唯一消费点——摘要用小
        // 省钱；自动压缩路径（context_length_exceeded 等）维持主模型。
        self.force_compression_opts(&instance, true).await;

        // 方言 PostCompact（观察型）：压缩尝试结束（成败皆触发），与 auto 路径对称。
        let bridge_post = self.cc_bridge.read().as_ref().cloned();
        if let Some(bridge) = bridge_post {
            bridge.run_compact_hooks("manual", "post").await;
        }

        let Some(cache) = instance
            .get_summary_cache()
            .filter(|c| c.covers_up_to > before && !c.text.is_empty())
        else {
            return Err("摘要生成失败（LLM 调用失败或无有效内容），历史保持不变".to_string());
        };

        // Persist: summary + covers BEFORE history（set_history 的
        // trim_to_limit 依赖 covers 判定哪些最旧消息可落盘丢弃，顺序错了会
        // 相互踩——同回合末持久化块的既有纪律）。
        if let Some(ref store) = self.session_store {
            store.get_or_create(session_key);
            store.set_summary(session_key, &cache.text);
            store.set_summary_covers_up_to(session_key, Some(cache.covers_up_to));
            let stored: Vec<crate::session::StoredMessage> = instance
                .get_history()
                .iter()
                .map(crate::session::StoredMessage::from)
                .collect();
            store.set_history(session_key, stored);
            if let Err(e) = store.save(session_key) {
                warn!(
                    "[AgentLoop] Failed to persist compacted session {}: {}",
                    session_key, e
                );
            }
        }

        let kept = history_len - cache.covers_up_to;
        info!(
            "[AgentLoop] Manual compact for {}: summary covers {} msgs, verbatim tail {} msgs",
            session_key, cache.covers_up_to, kept
        );
        Ok(format!(
            "✓ 已压缩：摘要覆盖前 {} 条，保留近 {} 条",
            cache.covers_up_to, kept
        ))
    }

    /// E6: 手动清空会话入口。chat_log 截断（jsonl-first，令 rebuild 路径
    /// 无旧内容可复活——sessions.clear 同纪律）+ SessionStore 清除（内存
    /// 条目 + 磁盘 json，下一回合 get_or_create 重建空会话）。key 保留，
    /// 会话继续可用。实例按回合从 store 重建，无驻留内存态需要清。
    pub async fn clear_session(&self, session_key: &str) -> Result<String, String> {
        crate::chat_log::clear_chat_log(session_key);
        if let Some(ref store) = self.session_store {
            store.clear_session(session_key);
        }
        info!("[AgentLoop] Manual clear for {}", session_key);
        Ok("✓ 已清空会话历史".to_string())
    }

    /// M5（devtool-upgrade 阶段 3）：会话级 context 占用快照（只读）。
    ///
    /// 口径与 `maybe_update_summary` 的压缩压力测量**同一公式**：
    /// `used` = 摘要覆盖点之后的逐字尾部（build_messages 实际发给 LLM 的
    /// 部分）按 MODEL-FACING 投影估算的 token 数；`window` = 三级解析链
    /// （config `context_window` → 价目表 → 实例默认）给出的窗口。
    /// 这样 Dashboard 显示的百分比就是「下一轮真实占用」而不是一个
    /// 另一套口径的近似值。
    ///
    /// 实例按回合从 store 重建（同 [`Self::compact_session`] 的读路径），
    /// 无驻留状态；store 未挂（standalone 简测）时按空历史计。
    pub fn session_context_status(&self, session_key: &str) -> serde_json::Value {
        let instance = self.get_or_create_instance(session_key);
        let history = instance.get_history();
        let window = self
            .current_context_window()
            .unwrap_or_else(|| instance.context_window());
        let cache = instance.get_summary_cache();
        let covers = cache
            .as_ref()
            .map(|c| c.covers_up_to)
            .filter(|&c| c >= 1)
            .unwrap_or(0)
            .min(history.len());
        let used = estimate_tokens_for_turns_projected(&history[covers..]);
        let pct = (used * 100).checked_div(window).unwrap_or(0).min(100);
        serde_json::json!({
            "session_key": session_key,
            "used_tokens": used,
            "window": window,
            "pct": pct,
            "history_len": history.len(),
            "covers_up_to": covers,
            "summarized": cache.is_some(),
        })
    }

    // -----------------------------------------------------------------------
    // Internal agent loop execution
    // -----------------------------------------------------------------------

    /// Get or create an AgentInstance for the given session key.
    fn get_or_create_instance(&self, session_key: &str) -> AgentInstance {
        let config = AgentConfig {
            model: self.active_model.read().clone(),
            system_prompt: self.config.system_prompt.clone(),
            max_turns: self.config.max_turns,
            tools: self.config.tools.clone(),
            models: self.config.models.clone(),
        };
        let instance = AgentInstance::new(config);

        // Restore history + summary cache from session store if available.
        // Mirrors Go's `agent.Sessions.Get(sessionKey)` in `getOrCreateInstance`.
        if let Some(ref store) = self.session_store {
            let stored = store.get_or_create(session_key);
            let existing_summary = store.get_summary(session_key);
            let covers = store.get_summary_covers_up_to(session_key);
            if !stored.messages.is_empty() {
                let history: Vec<crate::types::ConversationTurn> =
                    stored.messages.into_iter().map(|m| m.into()).collect();
                instance.set_history(history);
            }
            // Restore the summary cache. covers_up_to indexes the full history
            // (system prompt at index 0). For new-format files it is stored
            // explicitly; for legacy files (pre-refactor, field absent → None)
            // we map it so build_messages sends ALL loaded messages verbatim
            // while injecting the summary as floating context for older content
            // that was truncated out under the old regime.
            if !existing_summary.is_empty() {
                let history = instance.get_history();
                if !history.is_empty() {
                    let c = match covers {
                        Some(c) => c.clamp(1, history.len()),
                        None => history
                            .iter()
                            .take_while(|t| t.role == "system")
                            .count()
                            .max(1),
                    };
                    instance.set_summary_cache(Some(crate::instance::SummaryCache {
                        covers_up_to: c,
                        text: existing_summary,
                    }));
                }
            }
        }

        instance
    }

    /// Run the agent loop for a specific session.
    /// Mirrors Go's `runAgentLoop()`.
    ///
    /// T5（多模态）：`image_refs` 是本 turn 已通过安全闸的图片路径引用
    /// （process_admitted 统一附加产出），随 user turn 进
    /// `ConversationTurn.image_refs`；无图调用方传 `&[]`。
    async fn run_agent_loop_internal(
        &self,
        session_key: &str,
        user_message: &str,
        channel: &str,
        chat_id: &str,
        voice_playback: bool,
        cancel_token: &tokio_util::sync::CancellationToken,
        cron_job_id: Option<&str>,
        cron_job_name: Option<&str>,
        turn_budget: Option<u32>,
        image_refs: &[String],
        cp_turn: Option<usize>,
    ) -> Result<String, String> {
        // Round-5 fix: cron-originated turns are exempt from boundary events,
        // same as heartbeat. A recurring cron job targeting a persistent
        // session would grow its boundary sidecar (3+ rows per fire) without
        // bound — the exact unbounded-growth failure the heartbeat exemption
        // exists to prevent. The gateway cron handler (gateway.rs) delivers
        // via the bus with metadata cron_job_id, which flows here through
        // the caller; the CronTool path passes user="cron" (detected below).
        let is_cron_turn = cron_job_id.is_some();
        // Generate trace ID and emit conversation_start event.
        let trace_id = format!(
            "{}-{}",
            session_key,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let start_time = std::time::Instant::now();

        // D3：本 turn 文件变更缓冲从空开始（正常路径上轮 drain 已清；这里
        // 兜底上轮异常短路未走到 assistant 落盘的残留）。
        self.turn_file_changes.lock().remove(session_key);

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "agent".to_string(),
            content: user_message.to_string(),
        })
        .await;

        // Record last channel (skip internal channels).
        if !channel.is_empty() && !chat_id.is_empty() && !is_internal_channel(channel) {
            let channel_key = format!("{}:{}", channel, chat_id);
            self.record_last_channel(&channel_key);
        }

        let instance = self.get_or_create_instance(session_key);
        let mut context = RequestContext::new(channel, chat_id, "agent", session_key);
        if is_cron_turn {
            // Round-5 fix: propagate cron origin so run_llm_loop's boundary
            // gating can exempt it (see log_boundaries).
            context.user = "cron".to_string();
        }

        let events = self
            .run_with_trace(
                &instance,
                user_message,
                &context,
                &trace_id,
                voice_playback,
                cancel_token,
                turn_budget,
                image_refs,
            )
            .await;

        // Maybe trigger summarization.
        self.maybe_update_summary(&instance, session_key, channel, chat_id)
            .await;

        // Persist to session store. Post-refactor (inline-summarization): the
        // store holds the FULL instance history (system + user + assistant +
        // tool calls/results), not just a user/assistant log, so the summary
        // cache's covers_up_to index stays coherent across turns (the instance
        // is rebuilt from the store each turn). The user-facing conversation
        // log is written separately to chat_log below.
        //
        // (Pre-refactor this appended only user + final assistant, mirroring
        // Go's runAgentLoop. That left tool context out of the session file
        // and made a persistent summary cache incoherent — the root cause of
        // the "summary not injected / silent amnesia" bug this refactor fixes.)

        // Extract final response once (shared by session store, chat log, and observer).
        // K4 (c)：`mut`——`cluster_rpc:` 会话（B 端执行节点）本轮有文件变更
        // 时，完成回报会在下方追加变更摘要段（落盘与出站共用同一字符串）。
        let mut final_response = events
            .iter()
            .rev()
            .find_map(|e| {
                if let AgentEvent::Done(msg) = e {
                    Some(msg.clone())
                } else if let AgentEvent::Error(msg) = e {
                    Some(msg.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        if let Some(ref store) = self.session_store {
            // Ensure session exists in store.
            store.get_or_create(session_key);

            // Persist the summary cache (text + covers_up_to) BEFORE the
            // history: set_history's trim_to_limit reads covers_up_to to decide
            // which oldest messages are safe to drop (only from the covered
            // prefix) and adjusts it downward by the number dropped. Setting
            // covers first means the trim operates on this turn's correct value
            // and leaves a coherent store (messages + covers aligned). Setting
            // it AFTER would clobber the trim's adjustment and, for long
            // conversations (>MAX_STORED_MESSAGES), leave covers_up_to too large
            // so build_messages drops the verbatim tail.
            match instance.get_summary_cache() {
                Some(cache) => {
                    store.set_summary(session_key, &cache.text);
                    store.set_summary_covers_up_to(session_key, Some(cache.covers_up_to));
                }
                None => {
                    store.set_summary(session_key, "");
                    store.set_summary_covers_up_to(session_key, None);
                }
            }

            // Persist the full instance history (the store is the single source
            // of truth the next turn's get_or_create_instance reloads). trim runs
            // inside, bounding to MAX_STORED_MESSAGES and adjusting covers_up_to.
            let stored: Vec<crate::session::StoredMessage> = instance
                .get_history()
                .iter()
                .map(crate::session::StoredMessage::from)
                .collect();
            store.set_history(session_key, stored);

            if let Err(e) = store.save(session_key) {
                warn!(
                    "[AgentLoop] Failed to persist session history for {}: {}",
                    session_key, e
                );
            }
        }

        // Append to chat log (independent of session store).
        // T6（多模态）：user 行带图片路径引用（只存路径不落字节）。
        // E3：user 行带 `checkpoint_turn` 标记（本 turn begin 的序号随
        // admission 穿针而来）——消息级回退的行→turn 定位锚。
        crate::chat_log::append_chat_log_meta(
            session_key,
            "user",
            user_message,
            &crate::chat_log::ChatLogMeta {
                model: None,
                cron_job_id,
                cron_job_name,
                images: image_refs,
                file_changes: &[],
                checkpoint_turn: cp_turn,
            },
        );
        // D3：本 turn 声明式文件工具变更随 assistant 行落盘（消息↔文件
        // 变更映射；M3 会话级 diff 查看器的数据源）。drain 即清（下 turn
        // 从空开始）；去重规则见 `chat_log::dedup_file_changes`。
        let turn_changes = self.drain_turn_file_changes(session_key);
        // K4 (c) (devtool-upgrade 阶段 7): B 端 peer_chat 任务完成回报带
        // 变更摘要——`cluster_rpc:` 会话（B 端执行节点）本轮有声明式文件
        // 变更时，把紧凑列表追加进最终回复。落盘与出站共用同一字符串：
        // chat_log、channel 回执（含 rpc correlation 前缀包裹）看到的完全
        // 一致，A 端用户在原通道直接看到执行节点改了哪些文件。渲染规则
        // （上限/溢出注记/确定性顺序）见 `render_turn_changes_summary`。
        if session_key.starts_with("cluster_rpc:")
            && let Some(summary) = render_turn_changes_summary(&turn_changes)
        {
            final_response.push_str(&summary);
        }
        crate::chat_log::append_chat_log_meta(
            session_key,
            "assistant",
            &final_response,
            &crate::chat_log::ChatLogMeta {
                model: Some(&self.current_display_model()),
                cron_job_id,
                cron_job_name,
                images: &[],
                file_changes: &turn_changes,
                checkpoint_turn: None, // E3：标记只在 user 行（turn 开始锚）
            },
        );

        // E7：会话标题自动生成（assistant 回复落盘后；无资格/未配置小模型
        // 内部诚实跳过。通常首轮触发——meta 已有标题后不再重复）。
        let _ = self.spawn_session_title_job(session_key);

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        })
        .await;

        // Extract final response.
        for event in events.iter().rev() {
            if let AgentEvent::Done(msg) = event {
                return Ok(msg.clone());
            }
        }
        for event in events.iter().rev() {
            if let AgentEvent::Error(msg) = event {
                return Err(msg.clone());
            }
        }

        Ok("I've completed processing but have no response to give.".to_string())
    }

    // -----------------------------------------------------------------------
    // Standalone run loop
    // -----------------------------------------------------------------------

    /// Run the agent loop to process a user message (standalone mode).
    ///
    /// Returns a vector of events produced during execution.
    pub async fn run(
        &self,
        instance: &AgentInstance,
        user_message: &str,
        context: &RequestContext,
    ) -> Vec<AgentEvent> {
        let trace_id = format!(
            "run-{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let token = tokio_util::sync::CancellationToken::new();
        self.run_with_trace(
            instance,
            user_message,
            context,
            &trace_id,
            false,
            &token,
            None,
            &[],
        )
        .await
    }

    /// Run the agent loop with a specific trace ID for observer event correlation.
    ///
    /// This is the actual implementation that emits observer events for:
    /// - LLM request (before calling the provider)
    /// - LLM response (after receiving the response)
    /// - Tool call (after each tool execution)
    pub async fn run_with_trace(
        &self,
        instance: &AgentInstance,
        user_message: &str,
        context: &RequestContext,
        trace_id: &str,
        voice_playback: bool,
        cancel_token: &tokio_util::sync::CancellationToken,
        turn_budget: Option<u32>,
        image_refs: &[String],
    ) -> Vec<AgentEvent> {
        // K2 (U14): prompt-level lifecycle hooks (dialect SessionStart +
        // UserPromptSubmit dialect events). Runs BEFORE `add_user_message`
        // so a blocked prompt NEVER enters history — the model doesn't see
        // it, matching the dialect's block semantics. `resume_execution` does not
        // pass through here (no new user prompt → no event), by design.
        {
            let lifecycle = self.lifecycle_hooks.read().snapshot();
            if !lifecycle.is_empty() {
                let prompt = crate::hooks::HookPrompt {
                    session_key: context.session_key.clone(),
                    channel: context.channel.clone(),
                    chat_id: context.chat_id.clone(),
                    prompt: user_message.to_string(),
                };
                if let Some(reason) = crate::hooks::run_user_prompt_hooks(&lifecycle, &prompt).await
                {
                    warn!(
                        "[AgentLoop] prompt hook blocked the message for session '{}': {}",
                        context.session_key, reason
                    );
                    return vec![AgentEvent::Done(format!(
                        "⛔ HOOK BLOCKED [layer:hook|policy:prompt_hook] {} — A registered hook denied this prompt. Adjust the hook policy and resend.",
                        reason
                    ))];
                }
            }
        }

        // Add user message to instance history.
        // T5（多模态）：带图片路径引用的变体（引用进 ConversationTurn.image_refs，
        // build_messages 每轮水合；无图时与 add_user_message 等价）。
        instance.add_user_message_with_images(user_message, image_refs);
        instance.set_state(crate::types::AgentState::Thinking);

        self.run_llm_loop(
            instance,
            context,
            trace_id,
            voice_playback,
            cancel_token,
            turn_budget,
        )
        .await
    }

    /// G0 (devtool-upgrade 阶段 3)：派生一个**分离执行**的子代理回合。
    ///
    /// 与主路径的本质关系：不走 bus、不写 session store / chat_log（会话
    /// 只存在于本次派生的临时 instance，跑完即弃），但**复用
    /// `run_with_trace` 全链路**——工具调度、安全 8 层、guardian、hook、
    /// tier 过滤、args 校验、spill、turn_guard、C3 诊断回灌全部同源生效
    /// （这是与 legacy `nemesis-tools` 异步版的本质差异：子代理拥有与主
    /// loop 相同的全套治理）。最终 assistant 文本从 `Done` 事件提取。
    ///
    /// 由 `SpawnTool` 的 spawn_slot 闭包（agent_factory 组装后注入，持
    /// `Weak<AgentLoop>` 防 Arc 环）调用。
    pub async fn run_detached(&self, task: &str, opts: DetachedOpts<'_>) -> Result<String, String> {
        let events = self.run_detached_events(task, opts).await;
        // Done 优先；Error 次之；两者皆无 = 诚实报错（不编造输出）。
        let mut done: Option<String> = None;
        let mut error: Option<String> = None;
        for e in events {
            match e {
                AgentEvent::Done(m) => done = Some(m),
                AgentEvent::Error(e) => error = Some(e),
                _ => {}
            }
        }
        match (done, error) {
            (Some(m), _) => Ok(m),
            (None, Some(e)) => Err(e),
            (None, None) => Err("sub-agent produced no output".to_string()),
        }
    }

    /// Swarm M1：detached 会话键构造。`label` 注入段（`subagent:{label}:{uuid}`）
    /// 供日志检索与可观测（G13）；None 维持 `subagent:{uuid}` 现状。
    fn detached_session_key(label: Option<&str>) -> String {
        match label {
            Some(l) => format!("subagent:{}:{}", l, uuid::Uuid::new_v4()),
            None => format!("subagent:{}", uuid::Uuid::new_v4()),
        }
    }

    /// K1/K2（devtool-upgrade 阶段 4）：[`AgentLoop::run_detached`] 的**事件
    /// 全集**变体。同一条 `run_with_trace` 链路（工具调度、安全 8 层、
    /// guardian、tier、spill、turn_guard 全部同源），但把事件 `Vec` 原样
    /// 交还调用方——headless `run` 文本模式折 Done/Error，NDJSON 模式（K2）
    /// 逐事件序列化。会话语义同 run_detached：临时 instance 跑完即弃。
    pub async fn run_detached_events(&self, task: &str, opts: DetachedOpts<'_>) -> Vec<AgentEvent> {
        // Swarm M1（裸提示词模式）：system_prompt 覆盖须在 AgentInstance::new
        // 之前——构造函数把它注入为 history 首轮 system turn。
        let mut cfg = self.config.clone();
        if let Some(sp) = opts.system_prompt {
            cfg.system_prompt = Some(sp.to_string());
        }
        let session_key = Self::detached_session_key(opts.label);
        let instance = AgentInstance::new(cfg);
        if let Some(allowed) = opts.allowed_tools
            && !allowed.is_empty()
        {
            instance
                .set_detached_allowed_tools(Some(allowed.iter().map(|s| s.to_string()).collect()));
        }
        if opts.no_tools {
            instance.set_detached_no_tools(true);
        }
        // G2: record the sub-agent nesting depth on the instance — the serial
        // dispatch reads it back (instance.detached_depth()) and injects it
        // into depth-aware tools so `agents.subagent.max_depth` is enforced.
        instance.set_detached_depth(opts.depth);
        // 注：workspace 继承由 agent_factory 的 SpawnConfig 提供（与主
        // instance 同目录）；standalone loop（无 workspace 概念）留空。
        let context = RequestContext {
            channel: "subagent".to_string(),
            chat_id: session_key.clone(),
            user: "subagent".to_string(),
            session_key: session_key.clone(),
            correlation_id: None,
            async_callback: None,
        };
        let trace_id = format!("subagent-{}", uuid::Uuid::new_v4().simple());
        // Swarm G13：detached 路径补合成 ConversationStart/End。run_llm_loop
        // 只发 LlmRequest/LlmResponse，而请求日志观察者（RequestLoggerObserver
        // / ClusterRequestLoggerObserver）的 active 表以 start 事件注册
        // trace_id——缺 start 则全部事件被静默丢弃，评审/子代理/无头任务的
        // LLM 调用不可回放。与 cluster_agent worker 任务同一处置。
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.clone(),
            channel: "subagent".to_string(),
            chat_id: session_key.clone(),
            sender_id: "subagent".to_string(),
            content: task.to_string(),
        })
        .await;
        let start_time = std::time::Instant::now();
        let events = self
            .run_with_trace(
                &instance,
                task,
                &context,
                &trace_id,
                false,
                &tokio_util::sync::CancellationToken::new(),
                (opts.max_turns > 0).then_some(opts.max_turns),
                &[],
            )
            .await;
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        // Done 优先、Error 次之、皆无为空串——与 run_detached 的提取一致。
        let final_response = events
            .iter()
            .rev()
            .find_map(|e| match e {
                AgentEvent::Done(m) => Some(m.clone()),
                _ => None,
            })
            .or_else(|| {
                events.iter().rev().find_map(|e| match e {
                    AgentEvent::Error(e) => Some(e.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id,
            session_key,
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: "subagent".to_string(),
            chat_id: context.chat_id,
        })
        .await;
        events
    }

    /// Resume execution from a previously saved conversation state.
    ///
    /// Unlike `run_with_trace()`, this does NOT inject a user message.
    /// The instance should already have history loaded (via `set_history()`)
    /// and a tool result injected (via `add_tool_result()`).
    pub async fn resume_execution(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
    ) -> Vec<AgentEvent> {
        // 委托带 token 版本（自造一次性 token，行为与旧实现一致）。
        self.resume_execution_with_token(
            instance,
            context,
            trace_id,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    /// `resume_execution` 的带取消令牌变体（W2 P4 per-task cancel）：集群续行
    /// 任务执行前由 cluster_agent 注册 token，cancel_task 下行时可中断 LLM 循环。
    /// 语义与 [`Self::resume_execution`] 相同，仅 token 透传。
    pub async fn resume_execution_with_token(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> Vec<AgentEvent> {
        instance.set_state(crate::types::AgentState::Thinking);
        self.run_llm_loop(instance, context, trace_id, false, cancel_token, None)
            .await
    }

    /// Core LLM loop shared by `run_with_trace()` and `resume_execution()`.
    ///
    /// `turn_budget` (T3/U12): per-turn tool-round override. When set (>0) it
    /// REPLACES `config.max_turns` for this turn — the per-fire budget of a
    /// cron continuation. Exhaustion reuses the grace-round semantics (one
    /// finalize round, then a resumable stop with reason `budget_exhausted`).
    async fn run_llm_loop(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        voice_playback: bool,
        cancel_token: &tokio_util::sync::CancellationToken,
        turn_budget: Option<u32>,
    ) -> Vec<AgentEvent> {
        let mut events = Vec::new();

        // max_tokens: per-model `max_output_tokens` from config if declared
        // (each model's real output ceiling, not a blanket 8192 — large files
        // write in one shot instead of truncating); else 8192. temperature 0.7.
        let chat_opts = crate::types::ChatOptions {
            max_tokens: Some(self.current_max_tokens().unwrap_or(8192)),
            temperature: Some(0.7),
            // H4 (U16 half): per-model reasoning effort from config.json.
            reasoning_effort: self.current_reasoning_effort(),
            ..Default::default()
        };

        // I3 (U9): durable turn boundary markers. Heartbeat sessions are
        // exempt — they run periodically and would grow heartbeat.jsonl
        // without bound (3+ boundary lines per beat, forever).
        // 4th-pass fix: the heartbeat exemption keys on the CONTEXT user
        // marker (set by process_heartbeat), not the session_key — a real
        // user session could legitimately be named "agent:heartbeat" and
        // would have been silently exempted from boundary logging.
        // Round-5 fix: cron-originated turns are exempt too (user=="cron",
        // set by run_agent_loop_internal when cron_job_id metadata is
        // present) — a recurring cron on a persistent session grows the
        // boundary sidecar unboundedly otherwise.
        let log_boundaries = context.user != "heartbeat"
            && context.user != "cron"
            && !is_internal_channel(&context.channel);
        if log_boundaries {
            crate::chat_log::append_boundary_event(&context.session_key, "turn_start", "");
        }
        let mut turns_used = 0u32;
        // Phase 2 (small-model-tool-robustness): per-request consecutive
        // validation-failure counter. Reset on any successful (valid or
        // auto-fixed) tool call; incremented on each schema violation. When it
        // reaches the tier budget the loop stops, preventing a struggling model
        // from burning max_turns on the same malformed call.
        let mut validation_failures = 0u32;
        // Continue-generation budget for max_tokens truncation: when output
        // hits the token cap it's cut mid-way (often mid tool-call JSON).
        // Instead of routing the broken call through the validation budget
        // (which force-stops with a misleading "args invalid" error — Big tier
        // = 0 retries), append partial content + a "continue" prompt and
        // re-loop.
        let mut length_continuations = 0u32;
        const MAX_LENGTH_CONTINUATIONS: u32 = 5;
        // ② Grace-round latch. When the tool-call budget is exhausted we grant
        // one extra round (with GRACE_ROUND_NUDGE injected) so the model can
        // synthesize a final answer from completed work; a second hit stops
        // resumably instead of hard-crashing with "Max iterations reached".
        let mut grace_round = false;
        // Turn-scoped guards (⑥ alternating loop, ⑦ degenerate output). Fresh
        // per request — no state crosses requests.
        let mut turn_guard = crate::turn_guard::TurnGuard::new();
        // ⑦ Degenerate-answer nudge awaiting re-injection. Transient — kept out
        // of instance history / session_log; re-applied after each build_messages
        // until the model gives a visible answer or the retry budget runs out.
        let mut degenerate_nudge_pending: Option<String> = None;
        // ⑧ Pending cross-round prose-repetition nudge (same transient pattern:
        // re-applied after each build_messages, never persisted to history).
        let mut repetition_nudge_pending: Option<String> = None;
        // I1 (U7): one-shot escape-hatch latch (see the Accept branch).
        let mut steer_escape_used = false;
        // L2 (full review): terminal reason recorded AT the break site
        // instead of post-hoc string sniffing (a model reply containing
        // the paused-after wording would have been misclassified).
        let mut terminal_reason: Option<&'static str> = None;
        // K2 (U14): turn-end hook (dialect Stop) continue budget. Each
        // `Continue` demand injects the hook feedback as a user message and
        // grants one more round; exhausted → stop anyway (fail-open, same
        // discipline as MAX_LLM_HOOK_RETRIES).
        let mut turn_end_continues: u32 = 0;

        // K1b (U14): labeled so the LLM post-hook retry loop (deep inside,
        // around the guarded re-call) can abort the turn with `break 'turn`.
        // Bare `break`s elsewhere keep targeting their nearest loop — this
        // label only ADDS a way to name the turn loop, changing nothing else.
        'turn: loop {
            // Auto-reload MCP tools if config file changed.
            self.check_mcp_reload();
            // Phase 4a: re-resolve capability tier if config.json changed on
            // disk (dashboard model add, CLI `model set-tier` while running).
            self.check_config_reload();

            // Check cancellation at the top of each iteration.
            if cancel_token.is_cancelled() {
                info!(
                    "[AgentLoop] LLM loop cancelled at top of iteration, turns_used={}",
                    turns_used
                );
                events.push(AgentEvent::Done("已取消".to_string()));
                break;
            }

            // 全局急停检查：触发则立刻结束当前轮。未接线（None）时整块跳过。
            let estop_engaged = self
                .estop
                .read()
                .as_ref()
                .map(|e| e.is_engaged())
                .unwrap_or(false);
            if estop_engaged {
                info!(
                    "[AgentLoop] E-stop engaged at top of iteration, turns_used={}",
                    turns_used
                );
                events.push(AgentEvent::Done(
                    "⛔ 已急停 (E-STOP) — 已停止当前任务。发送 `nemesisbot estop --release` 恢复。"
                        .to_string(),
                ));
                break;
            }

            // ①/② max_turns cap + grace round. max_turns == 0 means unlimited
            // (opt-in). T3 (U12): when a per-turn budget override is set
            // (cron continuation's max_rounds), it REPLACES the global cap for
            // this turn. On the first hit we grant one grace round (with
            // GRACE_ROUND_NUDGE injected below) so the model can finalize from
            // completed work; a second hit stops resumably — no work is lost.
            let effective_max_turns = turn_budget.unwrap_or(self.config.max_turns);
            if effective_max_turns > 0 && turns_used >= effective_max_turns {
                if !grace_round {
                    grace_round = true;
                    info!(
                        "[AgentLoop] max_turns ({}) reached after {} turns; granting one grace round to finalize",
                        effective_max_turns, turns_used
                    );
                    // Fall through: this iteration runs as the grace round.
                } else if turn_budget.is_some() {
                    warn!(
                        "[AgentLoop] paused after {} tool-call rounds (per-turn budget exhausted, grace round spent)",
                        effective_max_turns
                    );
                    // T3 (U12): budget-driven stop. The job that fired this
                    // turn is NOT deleted — the next fire re-budgets, so the
                    // message says so instead of suggesting a config change.
                    terminal_reason = Some("budget_exhausted");
                    events.push(AgentEvent::Done(format!(
                        "已在定时任务预算 {} 轮工具调用后暂停，已完成的工作已保存。定时任务未被删除，下次触发时会重新获得预算。",
                        effective_max_turns
                    )));
                    break;
                } else {
                    warn!(
                        "[AgentLoop] paused after {} tool-call rounds (grace round exhausted)",
                        effective_max_turns
                    );
                    terminal_reason = Some("max_turns");
                    events.push(AgentEvent::Done(format!(
                        "已在 {} 轮工具调用后暂停，已完成的工作已保存。发送下一条消息可继续，或调大 max_tool_iterations（设为 0 表示不限）。",
                        effective_max_turns
                    )));
                    break;
                }
            }

            // I1 (U7): inbox claim — before EVERY LLM call of this turn, take
            // all pending steer messages (next-step) into history as real user
            // messages (persisted: they ARE genuine user input). Placement
            // after the existing history = same position as the time/env
            // injection's protected prefix zone (appended user turn), so the
            // provider prefix stays stable.
            //
            // ROUND-5 EFFICIENCY FIX: claim BEFORE build_messages (it used to
            // run after, so every steered round built the full message list
            // twice — skills catalog scan + instruction-chain file IO + 2
            // sha256s — and threw the first build away). One build, always.
            let steer_batch = self.inbox.claim_next_step(&context.session_key);
            if !steer_batch.is_empty() {
                for m in &steer_batch {
                    // L4 (full review) + round-5: strip the marker via the
                    // SINGLE shared rule (inbox::strip_steer_marker) — it is a
                    // ROUTING signal, not content, and the same message must
                    // arrive marker-free whether injected in-turn (here) or
                    // replayed post-turn (drain path).
                    let content = crate::inbox::strip_steer_marker(&m.msg.content).to_string();
                    // I2：steer 消息与首轮同源（B1 原则延伸）——@文件引用同样
                    // 展开（同基准/同安全闸），不因注入时点而异。
                    let at_base = self
                        .workspace_root
                        .read()
                        .clone()
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    let content = crate::message_preprocess::expand_at_files(
                        &content,
                        &at_base,
                        &m.msg.channel,
                        #[cfg(feature = "security")]
                        self.security_plugin.as_deref(),
                        #[cfg(not(feature = "security"))]
                        None,
                    );
                    // B1（2026-09-03 二次回归）：steer 消息与首轮同源——同样可能
                    // 携带图片（media 引用 + 文本点名路径）。走与 process_admitted
                    // 同一附加链（URL 预取 + 统一附加 + 诚实注记 + image_refs），
                    // 不静默丢图。
                    let ws_for_uploads = self.workspace_root.read().clone();
                    let uploads_base = ws_for_uploads
                        .unwrap_or_else(|| nemesis_path::default_path_manager().workspace());
                    let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(&uploads_base);
                    let (media_for_attach, url_notes) = crate::image_attach::fetch_url_media(
                        &m.msg.media,
                        &uploads_dir,
                        #[cfg(feature = "security")]
                        self.security_plugin.as_deref().and_then(|p| p.ssrf_guard()),
                        #[cfg(not(feature = "security"))]
                        None,
                    )
                    .await;
                    // J6：同 process_admitted——降采样开关 fresh-read，产物落 uploads。
                    let downscale_dir = self.current_image_downscale().then(|| uploads_dir.clone());
                    let attach = crate::image_attach::attach_turn_images(
                        &content,
                        &media_for_attach,
                        self.workspace_root.read().as_deref(),
                        downscale_dir.as_deref(),
                        &m.msg.channel,
                        #[cfg(feature = "security")]
                        self.security_plugin.as_deref(),
                        #[cfg(not(feature = "security"))]
                        None,
                    );
                    let content = attach.merge_into_text(content);
                    let content = crate::image_attach::AttachOutcome {
                        attached: Vec::new(),
                        notes: url_notes,
                    }
                    .merge_into_text(content);
                    instance.add_user_message_with_images(&content, &attach.ref_strings());
                    // L3（2026-09-04 四轮盲审）：steer 的 chat_log 行也要带图片
                    // 路径引用——首轮路径用 append_chat_log_full_with_images，
                    // steer 路径却用 3 参变体丢掉 images → 会话浏览器/self-heal
                    // 重建/fork 时 steer 轮的图片凭空消失（instance 里有图、
                    // 落盘行无图，两套存储分叉）。
                    crate::chat_log::append_chat_log_full_with_images(
                        &context.session_key,
                        "user",
                        &format!("[steer] {}", content),
                        None,
                        None,
                        None,
                        &attach.ref_strings(),
                    );
                    info!(
                        "[AgentLoop] steer message injected before LLM call: session_key={}, len={}",
                        context.session_key,
                        m.msg.content.len()
                    );
                    if log_boundaries {
                        crate::chat_log::append_boundary_event(
                            &context.session_key,
                            "steer_injected",
                            &format!("len={}", m.msg.content.len()),
                        );
                    }
                }
            }

            // P3.1 (sixth batch): auto-inject memory prefetch — async search
            // against the CURRENT (latest) user message, done OUTSIDE
            // build_messages (which is sync; search is async). Per round: the
            // latest user message changes when steer messages land, so
            // re-prefetching per LLM round keeps the section in sync with
            // what the model is about to see. Off (default) ⇒ None ⇒ the
            // build is byte-identical to pre-P3.1.
            let memory_hits: Option<Vec<String>> = self.prefetch_memory_context(instance).await;

            // Build the message list from instance history (AFTER the steer
            // claim so injected turns are already included).
            //
            // T8 (U9 ②): the annotated build + the injection records below
            // form this round's projection ledger — everything a later
            // byte-exact replay needs beyond the session store (the
            // transient injections are never persisted). See `crate::replay`.
            let (mut messages, build_annotation) =
                self.build_messages_with_memory_annotated(instance, memory_hits.as_deref());
            let mut replay_injections: Vec<crate::replay::InjectionRecord> = Vec::new();
            if let Some(idx) = build_annotation.digest_index {
                replay_injections.push(crate::replay::InjectionRecord {
                    index: idx,
                    role: messages[idx].role.clone(),
                    source: crate::replay::INJECTION_CONTEXT_DIGEST.to_string(),
                    content: messages[idx].content.clone(),
                });
            }
            let mut replay_voice: Option<crate::replay::VoiceAppend> = None;

            // Voice playback prompt injection: append to last user message (not stored in history).
            if voice_playback && let Some(pos) = messages.iter().rposition(|m| m.role == "user") {
                messages[pos].content.push_str(VOICE_PLAYBACK_SUFFIX);
                replay_voice = Some(crate::replay::VoiceAppend {
                    index: pos,
                    suffix: VOICE_PLAYBACK_SUFFIX.to_string(),
                });
            }

            // ② Grace-round nudge. Transient — NOT persisted to instance history
            // or session_log; only this turn's message list carries it.
            if grace_round {
                messages.push(LlmMessage {
                    role: "system".to_string(),
                    content: GRACE_ROUND_NUDGE.to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                    images: Vec::new(),
                });
                replay_injections.push(crate::replay::InjectionRecord {
                    index: messages.len() - 1,
                    role: "system".to_string(),
                    source: crate::replay::INJECTION_GRACE_NUDGE.to_string(),
                    content: GRACE_ROUND_NUDGE.to_string(),
                });
            }

            // ⑦ Re-inject a pending degenerate-answer nudge (transient, like the
            // grace nudge — never persisted to instance history / session_log).
            if let Some(nudge) = &degenerate_nudge_pending {
                messages.push(LlmMessage {
                    role: "user".to_string(),
                    content: nudge.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                    images: Vec::new(),
                });
                replay_injections.push(crate::replay::InjectionRecord {
                    index: messages.len() - 1,
                    role: "user".to_string(),
                    source: crate::replay::INJECTION_DEGENERATE_NUDGE.to_string(),
                    content: nudge.clone(),
                });
            }

            // ⑧ Re-inject a pending prose-repetition nudge (transient).
            if let Some(nudge) = &repetition_nudge_pending {
                messages.push(LlmMessage {
                    role: "system".to_string(),
                    content: nudge.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                    images: Vec::new(),
                });
                replay_injections.push(crate::replay::InjectionRecord {
                    index: messages.len() - 1,
                    role: "system".to_string(),
                    source: crate::replay::INJECTION_REPETITION_NUDGE.to_string(),
                    content: nudge.clone(),
                });
            }

            debug!("[AgentLoop] Sending {} messages to LLM", messages.len());

            // K1b (U14): LLM-call-level pre hooks. Runs AFTER messages are
            // built (nudges included) and BEFORE the LlmRequest observer
            // event — appended messages land in request_log and in the T8
            // replay ledger (byte-exact replay keeps holding).
            {
                let llm_hooks = self.llm_hooks.read().snapshot();
                if !llm_hooks.is_empty() {
                    let hook_call = crate::hooks::HookLlmCall {
                        model: self.active_model.read().clone(),
                        session_key: context.session_key.clone(),
                        round: turns_used as usize + 1,
                    };
                    match crate::hooks::run_llm_pre_hooks(&llm_hooks, &hook_call, &messages).await {
                        Ok(appended) => {
                            for m in appended {
                                replay_injections.push(crate::replay::InjectionRecord {
                                    index: messages.len(),
                                    role: m.role.clone(),
                                    source: crate::replay::INJECTION_LLM_HOOK.to_string(),
                                    content: m.content.clone(),
                                });
                                messages.push(m);
                            }
                        }
                        Err(reason) => {
                            warn!(
                                "[AgentLoop] LLM hook blocked the call, turns_used={}: {}",
                                turns_used, reason
                            );
                            events.push(AgentEvent::Done(format!(
                                "⛔ HOOK BLOCKED [layer:hook|policy:llm_hook] {} — A registered LLM hook denied this round. Do NOT retry unless the user changes the hook policy.",
                                reason
                            )));
                            break;
                        }
                    }
                }
            }

            // Build tool definitions from registered tools for LLM function calling.
            // Mirrors Go's ToolRegistry.ToProviderDefs() which calls tool.Description() and tool.Parameters().
            // Sort by name so the order is stable across runs — a deterministic
            // tool order gives reproducible behaviour and avoids unnecessary prompt
            // variation between requests.
            // Y1 (Phase4-a): fold AFTER the tier filter — description text only,
            // byte-identical passthrough whenever folding is off/degrades.
            let tool_defs: Vec<crate::types::ToolDefinition> = self.effective_tool_defs(instance);
            debug!(
                "[AgentLoop] Sending {} tool definitions to LLM",
                tool_defs.len()
            );

            // Emit LLM request observer event.
            // F-J：经 observer_msg_values 剥掉图片 base64（见其 doc）。
            let msg_values: Vec<serde_json::Value> = observer_msg_values(&messages);
            let tool_values: Vec<serde_json::Value> = tool_defs
                .iter()
                .filter_map(|t| serde_json::to_value(t).ok())
                .collect();
            // Extract model string before emit so RwLockReadGuard doesn't span the await.
            let active_model = self.active_model.read().clone();
            self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmRequest {
                trace_id: trace_id.to_string(),
                round: turns_used + 1,
                model: active_model.clone(),
                messages_count: messages.len(),
                tools_count: tool_defs.len(),
                messages: msg_values,
                tools: tool_values,
                provider_name: String::new(),
                api_key: String::new(),
                api_base: String::new(),
            })
            .await;

            // Call LLM.
            instance.set_state(crate::types::AgentState::Thinking);
            let round_start = std::time::Instant::now();
            // Clone provider Arc so RwLock guard is dropped before .await.
            let active_provider = self.provider.read().clone();

            // 订阅急停状态——LLM 调用进行中若触发急停，能即时打断：select 命中
            // estop arm 后，chat future 被 drop → reqwest 取消在途 HTTP 请求。
            // `None`（未接线）时该 arm 永不 resolve（pending），等价于没这条 arm。
            // 注意：subscribe() 返回的是 owned Receiver（不借用 guard），所以这里
            // 拿完就能放掉 estop 的读锁。
            let mut estop_rx = self.estop.read().as_ref().map(|e| e.subscribe());

            // I3 (U9): durable llm_request marker (model + size estimate,
            // no bodies). Heartbeat/internal-channel exemption (see
            // turn_start).
            //
            // T8 (U9 ②): projection-ledger sidecar for this round — the
            // durable record of every non-persisted injection (full bodies),
            // enabling byte-exact replay from the session store. Same
            // cron/heartbeat/internal exemption as the marker above: those
            // turns recur forever and would grow the ledger unboundedly.
            if log_boundaries {
                crate::chat_log::append_boundary_event(
                    &context.session_key,
                    "llm_request",
                    &format!(
                        "model={} messages={} turns_used={}",
                        self.active_model.read(),
                        messages.len(),
                        turns_used
                    ),
                );
                crate::replay::append_projection_record(&crate::replay::RequestProjectionRecord {
                    trace_id: trace_id.to_string(),
                    session_key: context.session_key.clone(),
                    round: turns_used as usize + 1,
                    ts: crate::replay::now_rfc3339(),
                    messages_count: messages.len(),
                    roles: messages.iter().map(|m| m.role.clone()).collect(),
                    history_len_at_build: build_annotation.history_len,
                    injections: replay_injections,
                    voice_append: replay_voice,
                    summary_as_of: build_annotation.summary_as_of.clone(),
                    vision_projected: build_annotation.vision_projected,
                });
            }

            // T10（多模态 D4 ③）：provider 兜底提示的判定输入——最终请求里
            // 是否真的带了图片字节（vision=yes/默认放行时图才会进来；nudge/
            // hook 注入消息恒无图）。
            let request_had_images = messages.iter().any(|m| !m.images.is_empty());

            // Use tokio::select! to allow cancellation / e-stop during the LLM call.
            // P3B 撞墙检测起点：首次调用的失败时长供 transient 重试链比对。
            let first_call_start = std::time::Instant::now();
            let chat_result = tokio::select! {
                result = active_provider.chat(&active_model, messages, Some(chat_opts.clone()), tool_defs) => result,
                _ = cancel_token.cancelled() => {
                    info!("[AgentLoop] LLM call cancelled while waiting for response, turns_used={}", turns_used);
                    events.push(AgentEvent::Done("已取消".to_string()));
                    break;
                }
                // 急停：watch 翻成 engaged 才 resolve。（K1b 起该等待逻辑抽成
                // wait_estop_engaged 共享——hook 重呼路径用同一段，防漂移。）
                _ = Self::wait_estop_engaged(estop_rx.as_mut()) => {
                    info!(
                        "[AgentLoop] E-stop engaged during LLM call, turns_used={}",
                        turns_used
                    );
                    events.push(AgentEvent::Done(
                        "⛔ 已急停 (E-STOP) — LLM 调用已中断。发送 `nemesisbot estop --release` 恢复。"
                            .to_string(),
                    ));
                    break;
                }
            };

            let mut response = match chat_result {
                Ok(resp) => resp,
                Err(err) => {
                    // 首次调用从发起到失败的时长（Err 臂入口即失败点）。
                    let first_call_failed_after = first_call_start.elapsed();
                    let err_lower = err.to_lowercase();
                    let is_context_error = ["token", "context", "length", "invalid"]
                        .iter()
                        .any(|keyword| err_lower.contains(keyword));

                    if is_context_error {
                        // Mirrors Go's retry-with-compression logic (loop_executor.go).
                        // Attempt up to 2 retries with progressive history compression.
                        let mut retry_count = 0u32;
                        let max_retries = 2u32;
                        let mut retry_err = err.clone();
                        let mut got_response = None;

                        // Notify user about compression.
                        info!(
                            "[AgentLoop] LLM context error, attempting compression and retry: {}",
                            retry_err
                        );

                        while retry_count < max_retries {
                            retry_count += 1;

                            // Force-compress: advance the summary cache (tail → SMALL_K_FORCE,
                            // then → 0 on a second pass). History is not mutated.
                            self.force_compression(instance).await;

                            // Rebuild messages from compressed history.
                            let mut compressed_messages = self.build_messages(instance);

                            // Re-apply voice playback prompt after compression.
                            if voice_playback
                                && let Some(last_user) = compressed_messages
                                    .iter_mut()
                                    .rev()
                                    .find(|m| m.role == "user")
                            {
                                last_user.content.push_str("（语音播报模式已开启，请用简洁、便于口语播报的方式回复，避免使用代码块、表格等不适合语音的内容。）");
                            }
                            debug!(
                                "[AgentLoop] Retry {}: sending {} messages after compression",
                                retry_count,
                                compressed_messages.len()
                            );

                            let retry_tool_defs: Vec<crate::types::ToolDefinition> = self
                                .tools
                                .read()
                                .iter()
                                .map(|(name, tool)| crate::types::ToolDefinition {
                                    tool_type: "function".to_string(),
                                    function: crate::types::ToolFunctionDef {
                                        name: name.clone(),
                                        description: tool.description(),
                                        parameters: tool.parameters(),
                                    },
                                })
                                .collect();

                            match active_provider
                                .chat(
                                    &active_model,
                                    compressed_messages,
                                    Some(chat_opts.clone()),
                                    retry_tool_defs,
                                )
                                .await
                            {
                                Ok(resp) => {
                                    got_response = Some(resp);
                                    break;
                                }
                                Err(e) => {
                                    retry_err = e;
                                    warn!(
                                        "[AgentLoop] LLM retry {} failed: {}",
                                        retry_count, retry_err
                                    );
                                }
                            }
                        }

                        match got_response {
                            Some(resp) => resp,
                            None => {
                                warn!("[AgentLoop] All LLM retries exhausted: {}", retry_err);
                                let error_round = turns_used + 1;
                                let error_duration = round_start.elapsed();
                                self.emit_observer_sync(
                                    crate::loop_executor::ObserverEvent::LlmResponse {
                                        trace_id: trace_id.to_string(),
                                        round: error_round,
                                        duration_ms: error_duration.as_millis() as u64,
                                        has_tool_calls: false,
                                        content: format!("Error: {}", retry_err),
                                        tool_calls: vec![],
                                        tool_calls_count: 0,
                                        finish_reason: Some("error".to_string()),
                                        usage: None,
                                        raw_request_body: None,
                                        raw_response_body: None,
                                    },
                                )
                                .await;
                                instance.add_assistant_message(
                                    &format!("Error: {}", retry_err),
                                    Vec::new(),
                                    None,
                                );
                                // [capture] LLM retries exhausted (context-error
                                // retry path). Flush the full retry_err — the raw
                                // provider error, likely source of the user-visible
                                // "tools" wording — plus trace_id to correlate with
                                // request_logs/' now-complete failed round (组1).
                                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                                    sink.flush(
                                        &context.session_key,
                                        "llm_retry_exhausted",
                                        Some(trace_id),
                                        Some(retry_err.as_str()),
                                    );
                                }
                                // T10（多模态 D4 ③）：兜底提示只进用户可见
                                // 文案；retry_err 原文继续走 history /
                                // observer / capture（诊断不掺提示）。
                                let formatted =
                                    context.format_rpc_message(&append_vision_fallback_hint(
                                        format!("Error: {}", retry_err),
                                        request_had_images,
                                    ));
                                events.push(AgentEvent::Error(formatted));
                                break;
                            }
                        }
                    } else {
                        // ③ Transient-error retry (network / stream / 5xx). Retries
                        // do NOT consume turns_used — the per-iteration increment
                        // below happens once regardless of how many retries it took
                        // to get a successful response. Messages + tool_defs are
                        // rebuilt fresh because the first-attempt values were moved
                        // into the failed call.
                        let is_transient_error = [
                            "timeout",
                            "timed out",
                            "connection reset",
                            "broken pipe",
                            "connect error",
                            "connection refused",
                            "temporarily unavailable",
                            "reset by peer",
                            "502",
                            "503",
                            "504",
                            "service unavailable",
                        ]
                        .iter()
                        .any(|k| err_lower.contains(k));

                        let mut last_err = err.clone();
                        let mut maybe_resp: Option<LlmResponse> = None;

                        if is_transient_error {
                            info!(
                                "[AgentLoop] LLM transient error, retrying up to {} times: {}",
                                MAX_TRANSIENT_RETRIES, last_err
                            );
                            // P3B 撞墙检测（2026-09-12）：prev 带入首次调用的失败
                            // 时长——相邻两次失败几乎同时长（±1s）= 大概率固定
                            // 超时阈值拦截（provider timeout / 上游网关 / CC
                            // Switch 类代理的固定超时），继续盲重试只是重复烧墙。
                            // 双端真机 S2 实证：评审 LLM 连续 4 次精确 120.01s
                            // 超时——根因 anthropic lane 默认 120s（已另行根修
                            // 为全 lane 600s + per-model timeout_secs 覆盖）。
                            let mut prev_fail_after = Some(first_call_failed_after);
                            let mut retries = 0u32;
                            while retries < MAX_TRANSIENT_RETRIES {
                                retries += 1;
                                let r_msgs = self.build_messages(instance);
                                let r_tools: Vec<crate::types::ToolDefinition> = self
                                    .tools
                                    .read()
                                    .iter()
                                    .map(|(name, tool)| crate::types::ToolDefinition {
                                        tool_type: "function".to_string(),
                                        function: crate::types::ToolFunctionDef {
                                            name: name.clone(),
                                            description: tool.description(),
                                            parameters: tool.parameters(),
                                        },
                                    })
                                    .collect();
                                let attempt_start = std::time::Instant::now();
                                match active_provider
                                    .chat(&active_model, r_msgs, Some(chat_opts.clone()), r_tools)
                                    .await
                                {
                                    Ok(resp) => {
                                        maybe_resp = Some(resp);
                                        break;
                                    }
                                    Err(e) => {
                                        last_err = e;
                                        let failed_after = attempt_start.elapsed();
                                        if let Some(prev) = prev_fail_after
                                            && prev.abs_diff(failed_after)
                                                <= std::time::Duration::from_secs(1)
                                        {
                                            warn!(
                                                "[AgentLoop] ⚠ 疑似固定超时阈值拦截：相邻两次失败时长几乎相同（前次 {:.1}s / 本次 {:.1}s），重试大概率无效——检查模型条目 timeout_secs / provider 超时与上游可用性，而不是继续重试。最后错误: {}",
                                                prev.as_secs_f32(),
                                                failed_after.as_secs_f32(),
                                                last_err
                                            );
                                        }
                                        prev_fail_after = Some(failed_after);
                                        warn!(
                                            "[AgentLoop] transient retry {}/{} failed: {}",
                                            retries, MAX_TRANSIENT_RETRIES, last_err
                                        );
                                    }
                                }
                            }
                        }

                        if let Some(resp) = maybe_resp {
                            resp
                        } else {
                            // Non-transient error, or transient retries exhausted.
                            warn!("[AgentLoop] LLM call failed: {}", last_err);
                            let error_round = turns_used + 1;
                            let error_duration = round_start.elapsed();
                            self.emit_observer_sync(
                                crate::loop_executor::ObserverEvent::LlmResponse {
                                    trace_id: trace_id.to_string(),
                                    round: error_round,
                                    duration_ms: error_duration.as_millis() as u64,
                                    has_tool_calls: false,
                                    content: format!("Error: {}", last_err),
                                    tool_calls: vec![],
                                    tool_calls_count: 0,
                                    finish_reason: Some("error".to_string()),
                                    usage: None,
                                    raw_request_body: None,
                                    raw_response_body: None,
                                },
                            )
                            .await;
                            instance.add_assistant_message(
                                &format!("Error: {}", last_err),
                                Vec::new(),
                                None,
                            );
                            // [capture] Non-transient error or transient retries
                            // exhausted. Flush full last_err + trace_id.
                            if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                                sink.flush(
                                    &context.session_key,
                                    "llm_call_failed",
                                    Some(trace_id),
                                    Some(last_err.as_str()),
                                );
                            }
                            // T10（多模态 D4 ③）：同上——提示只进用户可见
                            // 文案，last_err 原文继续走 history / observer /
                            // capture。
                            let formatted =
                                context.format_rpc_message(&append_vision_fallback_hint(
                                    format!("Error: {}", last_err),
                                    request_had_images,
                                ));
                            events.push(AgentEvent::Error(formatted));
                            break;
                        }
                    }
                }
            };

            // K1b (U14): LLM-call-level post hooks — the「拦思考」layer. Runs
            // AFTER the built-in error recovery, BEFORE the LlmResponse
            // observer event / turns_used increment, so every downstream
            // consumer (observer, usage, tool execution) sees the final
            // decision. Retry re-calls carry the same cancel/e-stop select
            // guard and emit their own LlmRequest observer event (visible in
            // request_log; no extra T8 ledger record — same shape as the
            // built-in transient retries).
            {
                let llm_hooks = self.llm_hooks.read().snapshot();
                if !llm_hooks.is_empty() {
                    let mut hook_retries: u32 = 0;
                    loop {
                        let hook_call = crate::hooks::HookLlmCall {
                            model: active_model.clone(),
                            session_key: context.session_key.clone(),
                            round: turns_used as usize + 1,
                        };
                        // Pass a clone: on fail-open paths (budget exhausted /
                        // retry call failed) `response` still holds the
                        // original and needs no restore.
                        match crate::hooks::run_llm_post_hooks(
                            &llm_hooks,
                            &hook_call,
                            response.clone(),
                        )
                        .await
                        {
                            crate::hooks::PostLlmOutcome::Allow(final_resp) => {
                                response = final_resp;
                                break;
                            }
                            crate::hooks::PostLlmOutcome::Block { reason } => {
                                warn!(
                                    "[AgentLoop] LLM hook blocked the response, turns_used={}: {}",
                                    turns_used, reason
                                );
                                events.push(AgentEvent::Done(format!(
                                    "⛔ HOOK BLOCKED [layer:hook|policy:llm_hook] {} — A registered LLM hook terminated this round. Inform the user.",
                                    reason
                                )));
                                break 'turn;
                            }
                            crate::hooks::PostLlmOutcome::Retry { reason } => {
                                if hook_retries >= crate::hooks::MAX_LLM_HOOK_RETRIES {
                                    warn!(
                                        "[AgentLoop] LLM hook retry budget exhausted ({}), allowing the previous response",
                                        crate::hooks::MAX_LLM_HOOK_RETRIES
                                    );
                                    break;
                                }
                                hook_retries += 1;
                                // Regenerate: rebuild messages (the first
                                // attempt's were moved into the call), append
                                // the hook feedback, re-call under the same
                                // cancel/e-stop guard.
                                let mut r_msgs = self.build_messages(instance);
                                r_msgs.push(LlmMessage {
                                    role: "system".to_string(),
                                    content: format!("# Hook feedback: {}", reason),
                                    tool_calls: None,
                                    tool_call_id: None,
                                    reasoning_content: None,
                                    images: Vec::new(),
                                });
                                // Y1 (Phase4-a): fold the retry call's defs with
                                // the same gates/rendering as the main call —
                                // same query ⇒ same fold bytes.
                                let r_tools = self.effective_tool_defs(instance);
                                self.emit_observer_sync(
                                    crate::loop_executor::ObserverEvent::LlmRequest {
                                        trace_id: trace_id.to_string(),
                                        round: turns_used + 1,
                                        model: active_model.clone(),
                                        messages_count: r_msgs.len(),
                                        tools_count: r_tools.len(),
                                        messages: observer_msg_values(&r_msgs),
                                        tools: r_tools
                                            .iter()
                                            .filter_map(|t| serde_json::to_value(t).ok())
                                            .collect(),
                                        provider_name: String::new(),
                                        api_key: String::new(),
                                        api_base: String::new(),
                                    },
                                )
                                .await;
                                let mut r_estop_rx =
                                    self.estop.read().as_ref().map(|e| e.subscribe());
                                let r = tokio::select! {
                                    res = active_provider.chat(&active_model, r_msgs, Some(chat_opts.clone()), r_tools) => res,
                                    _ = cancel_token.cancelled() => {
                                        info!("[AgentLoop] Hook retry call cancelled, turns_used={}", turns_used);
                                        events.push(AgentEvent::Done("已取消".to_string()));
                                        break 'turn;
                                    }
                                    _ = Self::wait_estop_engaged(r_estop_rx.as_mut()) => {
                                        info!(
                                            "[AgentLoop] E-stop engaged during hook retry call, turns_used={}",
                                            turns_used
                                        );
                                        events.push(AgentEvent::Done(
                                            "⛔ 已急停 (E-STOP) — LLM 调用已中断。发送 `nemesisbot estop --release` 恢复。"
                                                .to_string(),
                                        ));
                                        break 'turn;
                                    }
                                };
                                match r {
                                    Ok(resp) => {
                                        info!(
                                            "[AgentLoop] Hook retry {} succeeded, re-checking response",
                                            hook_retries
                                        );
                                        response = resp;
                                        continue;
                                    }
                                    Err(e) => {
                                        // Fail-open: keep the response the hook
                                        // rejected — a failed re-call must not
                                        // lose the round's only answer.
                                        warn!(
                                            "[AgentLoop] Hook retry call failed, keeping previous response: {}",
                                            e
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            turns_used += 1;

            // Emit LLM response observer event.
            let round_duration = round_start.elapsed();
            let tc_values: Vec<serde_json::Value> = response
                .tool_calls
                .iter()
                .filter_map(|tc| serde_json::to_value(tc).ok())
                .collect();
            let tc_count = response.tool_calls.len();
            self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmResponse {
                trace_id: trace_id.to_string(),
                round: turns_used,
                duration_ms: round_duration.as_millis() as u64,
                has_tool_calls: !response.tool_calls.is_empty(),
                content: response.content.clone(),
                tool_calls: tc_values,
                tool_calls_count: tc_count,
                finish_reason: if response.finished {
                    Some("stop".to_string())
                } else {
                    None
                },
                usage: response.usage.clone(),
                raw_request_body: response.raw_request_body.take(),
                raw_response_body: response.raw_response_body.take(),
            })
            .await;

            // Record usage statistics if data store is available.
            if let Some(ref ds) = self.data_store
                && let Some(ref usage) = response.usage
            {
                let model_name = self.active_model.read().clone();
                let cache_creation = usage.cache_creation_tokens.unwrap_or(0);
                let cache_read = usage.cache_read_tokens.or(usage.cached_tokens).unwrap_or(0);
                // A3：分项计价 + 实际命中条目名（未命中 → 空名 + 全 0，
                // 明细行可区分「未命中」与「命中免费条目」）。
                let breakdown = ds.compute_cost_breakdown(
                    &model_name,
                    usage.prompt_tokens,
                    usage.completion_tokens,
                    cache_creation,
                    cache_read,
                );
                let empty = nemesis_data::CostBreakdown::default();
                let bd = breakdown.as_ref().unwrap_or(&empty);
                let log = nemesis_data::RequestLog {
                    id: 0,
                    trace_id: trace_id.to_string(),
                    model: model_name.clone(),
                    provider_type: String::new(),
                    input_tokens: usage.prompt_tokens,
                    output_tokens: usage.completion_tokens,
                    cache_creation_tokens: cache_creation,
                    cache_read_tokens: cache_read,
                    total_cost_usd: bd.total_cost_usd,
                    latency_ms: round_duration.as_millis() as i64,
                    status_code: if response.content.starts_with("Error:") {
                        500
                    } else {
                        200
                    },
                    error_message: None,
                    is_streaming: false,
                    created_at: chrono::Local::now().timestamp(),
                    pricing_model: bd.pricing_model.clone(),
                    input_cost_usd: bd.input_cost_usd,
                    output_cost_usd: bd.output_cost_usd,
                    cache_creation_cost_usd: bd.cache_creation_cost_usd,
                    cache_read_cost_usd: bd.cache_read_cost_usd,
                    // provider trait 无流式通路，TTFT 无从测量（列留 NULL）。
                    first_token_ms: None,
                    session_key: context.session_key.clone(),
                };
                if let Err(e) = ds.insert_request_log(&log) {
                    tracing::warn!("[AgentLoop] Failed to record usage: {e}");
                }
            }

            // Continue-generation on max_tokens truncation.
            // When completion hits the cap, output is cut mid-way —
            // often mid tool-call JSON, which args_validator would report as
            // "Arguments are not valid JSON" and burn the validation budget
            // (Big tier = 0 retries → instant force-stop with a misleading
            // error). Detect it here: drop the truncated tool calls (executing
            // them would write a partial file / run half-formed args), keep
            // partial content, append a "continue" prompt, re-loop. Bounded so
            // a genuinely too-large file surfaces a clear error, not a loop.
            // NOTE: detection assumes chat_opts.max_tokens is Some — the agent
            // always sets Some(8192) when building chat_opts. If that ever
            // becomes None (provider's own default cap), gate on is_some():
            // the 8192 fallback could false-positive against a higher cap.
            let token_cap = chat_opts.max_tokens.unwrap_or(8192) as u64;
            let hit_cap = response
                .usage
                .as_ref()
                .map(|u| (u.completion_tokens as u64) >= token_cap)
                .unwrap_or(false);
            if hit_cap {
                if length_continuations < MAX_LENGTH_CONTINUATIONS {
                    length_continuations += 1;
                    warn!(
                        "[AgentLoop] response truncated at max_tokens cap ({token_cap}); \
                         continue-generation {length_continuations}/{MAX_LENGTH_CONTINUATIONS}"
                    );
                    instance.add_assistant_message(
                        &response.content,
                        Vec::new(),
                        response.reasoning_content.clone(),
                    );
                    instance.add_user_message(
                        "Output limit reached. Continue exactly where you left off — \
                         no recap, no apology. If you were writing a large file, \
                         break it into smaller writes.",
                    );
                    continue;
                }
                // Budget exhausted: clear, non-misleading error.
                warn!(
                    "[AgentLoop] length-continuation budget exhausted; \
                     output keeps exceeding max_tokens ({token_cap})"
                );
                let notice = format!(
                    "输出反复超过 max_tokens 上限（{token_cap}）被截断，文件可能太大。\
                     请调大 max_tokens，或让我分段写入。"
                );
                instance.add_assistant_message(&notice, Vec::new(), None);
                events.push(AgentEvent::Error(context.format_rpc_message(&notice)));
                break;
            }
            // Complete (non-truncated) response — reset the counter.
            length_continuations = 0;

            // ⑧ Cross-round prose repetition: if the model's content is
            // near-identical to the previous round's, queue a transient nudge
            // for the next build. Catches "saying the same thing while churning
            // tools" — a loop ⑥ cannot see (it watches tool results, not prose).
            if let Some(nudge) = turn_guard.check_text_repetition(&response.content) {
                info!("[AgentLoop] loop guard: response content repeating across rounds; nudging");
                repetition_nudge_pending = Some(nudge);
            } else {
                repetition_nudge_pending = None;
            }

            if response.tool_calls.is_empty() || response.finished {
                // No tool calls: candidate final response. ⑦ Check for degenerate
                // (empty / whitespace-only / reasoning-only) content and nudge
                // the model to retry before accepting. Skipped for heartbeat —
                // an empty heartbeat response means "nothing to do", a valid
                // outcome, not a broken answer.
                let content = response.content.clone();
                if context.user == "heartbeat" {
                    instance.add_assistant_message(
                        &content,
                        Vec::new(),
                        response.reasoning_content.clone(),
                    );
                    let formatted = context.format_rpc_message(&content);
                    events.push(AgentEvent::Done(formatted));
                    break;
                }
                match turn_guard.check_final_answer(&content) {
                    crate::turn_guard::FinalAnswerVerdict::Accept => {
                        instance.add_assistant_message(
                            &content,
                            Vec::new(),
                            response.reasoning_content.clone(),
                        );
                        // I1 (U7) turn escape hatch: the model is about to
                        // finish, but an unclaimed steer message arrived in
                        // the last moments — hand it to the model for one
                        // more round instead of answering past it — pending
                        // next-step input keeps the turn open. At most once
                        // per turn
                        // (steer_escape_used) so `!`-spam cannot loop the
                        // turn forever.
                        if !steer_escape_used
                            && self.inbox.has_next_step(&context.session_key)
                            && self.concurrent_mode == ConcurrentMode::Steer
                        {
                            steer_escape_used = true;
                            info!(
                                "[AgentLoop] escape hatch: pending steer at turn end, one more round"
                            );
                            // Loop again — the claim at the top of the next
                            // iteration injects the steer message(s).
                            continue;
                        }
                        // K2 (U14): turn-end lifecycle hooks (Stop
                        // dialect event). Runs after the assistant message
                        // is recorded, before the Done event. `Continue`
                        // injects the hook feedback as a user message and
                        // grants one more round, bounded by
                        // MAX_TURN_END_CONTINUES (exhausted → stop anyway,
                        // fail-open). Only the normal Accept path fires —
                        // heartbeat (its own branch above), GiveUp and
                        // error/stop paths do not.
                        {
                            let lifecycle = self.lifecycle_hooks.read().snapshot();
                            if !lifecycle.is_empty() {
                                let end = crate::hooks::HookTurnEnd {
                                    session_key: context.session_key.clone(),
                                    channel: context.channel.clone(),
                                    chat_id: context.chat_id.clone(),
                                    final_content: content.clone(),
                                    stop_hook_active: turn_end_continues > 0,
                                };
                                if let crate::hooks::TurnEndDecision::Continue { feedback } =
                                    crate::hooks::run_turn_end_hooks(&lifecycle, &end).await
                                {
                                    if turn_end_continues < crate::hooks::MAX_TURN_END_CONTINUES {
                                        turn_end_continues += 1;
                                        info!(
                                            "[AgentLoop] turn-end hook blocked stopping \
                                             ({}/{}, session '{}') — one more round",
                                            turn_end_continues,
                                            crate::hooks::MAX_TURN_END_CONTINUES,
                                            context.session_key
                                        );
                                        instance.add_user_message(&feedback);
                                        continue 'turn;
                                    }
                                    warn!(
                                        "[AgentLoop] turn-end hook keeps blocking stop after {} \
                                         continues; stopping anyway (fail-open), session '{}'",
                                        crate::hooks::MAX_TURN_END_CONTINUES,
                                        context.session_key
                                    );
                                }
                            }
                        }
                        let formatted = context.format_rpc_message(&content);
                        events.push(AgentEvent::Done(formatted));
                        break;
                    }
                    crate::turn_guard::FinalAnswerVerdict::RetryWithNudge(nudge) => {
                        warn!(
                            "[AgentLoop] degenerate final answer (empty/no visible text); nudging retry"
                        );
                        // Record the empty attempt in history, then queue the
                        // nudge for transient re-injection on the next build.
                        instance.add_assistant_message(
                            &content,
                            Vec::new(),
                            response.reasoning_content.clone(),
                        );
                        degenerate_nudge_pending = Some(nudge);
                        continue;
                    }
                    crate::turn_guard::FinalAnswerVerdict::GiveUp(notice) => {
                        warn!(
                            "[AgentLoop] degenerate final answer retry budget exhausted; giving up"
                        );
                        instance.add_assistant_message(&notice, Vec::new(), None);
                        let formatted = context.format_rpc_message(&notice);
                        events.push(AgentEvent::Done(formatted));
                        break;
                    }
                }
            }

            // Model produced tool calls → it is making progress. Clear any
            // pending degenerate-answer nudge (⑦) so it stops nagging while the
            // model works — tool work is the opposite of a degenerate empty
            // final answer.
            degenerate_nudge_pending = None;

            // Record the assistant's response with tool calls.
            let tool_calls = response.tool_calls.clone();
            let assistant_content = response.content.clone();
            instance.add_assistant_message(
                &assistant_content,
                tool_calls.clone(),
                response.reasoning_content.clone(),
            );
            events.push(AgentEvent::ToolCall(tool_calls.clone()));

            // Execute each tool call.
            instance.set_state(crate::types::AgentState::ExecutingTool);
            let mut hit_async = false;
            // Outer-scope turn-stop latch. Set inside the tool-call for-loop
            // (where `break` can only exit the batch, not the outer LLM loop) by
            // ⑥ escalation OR validation-budget exhaustion. Checked after the
            // for-loop to actually end the turn. Without this two-step, those
            // `break`s only stopped the current batch and the model was called
            // again — escalation fired every round without stopping (observed
            // 43× in a deployed test), and "validation stopping loop" was a lie.
            let mut force_stop: Option<AgentEvent> = None;
            // U5 (sixth batch): precompute execution for an ALL-parallel-safe
            // batch (≥2 calls, every tool read-only OR explicitly opted in —
            // G3: spawn). The for-loop then replays the serial guards on the
            // precomputed results in source order — the audit chain stays
            // ordered = model source order (roadmap risk 3). cluster_rpc/exec
            // /writers are never parallel-safe → this stays None for those
            // batches → the loop below runs byte-identical to pre-U5.
            // `None` also when a cancel/estop is already engaged at batch
            // start (the for-loop's per-item check handles that case
            // unchanged).
            let precomputed: Option<Vec<PrecomputedTool>> = if tool_calls.len() >= 2
                && !cancel_token.is_cancelled()
                && !self
                    .estop
                    .read()
                    .as_ref()
                    .map(|e| e.is_engaged())
                    .unwrap_or(false)
                && tool_calls
                    .iter()
                    .all(|tc| self.tool_is_parallel_safe(&tc.name))
            {
                let pc = self
                    .precompute_parallel_batch(&tool_calls, context, instance.detached_depth())
                    .await;
                Some(pc)
            } else {
                None
            };
            // U5: in the parallel path, cancel/estop are NOT re-checked per item
            // — the batch was checkpointed non-cancelled above and runs to
            // completion (goal §四 documented semantic: a cancel arriving during
            // the parallel window takes effect on the NEXT turn, not mid-batch).
            // The serial path (precomputed.is_none()) keeps the per-item checks
            // byte-identical.
            let skip_cancel_estop = precomputed.is_some();
            for (batch_idx, tc) in tool_calls.iter().enumerate() {
                // Check cancellation before each tool execution.
                if !skip_cancel_estop && cancel_token.is_cancelled() {
                    info!(
                        "[AgentLoop] LLM loop cancelled before tool execution: {}, turns_used={}",
                        tc.name, turns_used
                    );
                    events.push(AgentEvent::Done("已取消".to_string()));
                    break;
                }

                // 全局急停检查：触发则拒绝后续工具调用并结束当前轮。
                let estop_engaged = self
                    .estop
                    .read()
                    .as_ref()
                    .map(|e| e.is_engaged())
                    .unwrap_or(false);
                if estop_engaged {
                    info!(
                        "[AgentLoop] E-stop engaged before tool execution: {}, turns_used={}",
                        tc.name, turns_used
                    );
                    events.push(AgentEvent::Done(
                        "⛔ 已急停 (E-STOP) — 工具调用已拒绝。发送 `nemesisbot estop --release` 恢复。"
                            .to_string(),
                    ));
                    break;
                }

                let tool_start = std::time::Instant::now();
                // Phase 2 (small-model-tool-robustness): validate args against
                // the tool's schema before dispatch. Catches B-class failures;
                // auto-fixes high-confidence field-name typos (edit distance ≤2);
                // otherwise bounces a structured error back to the model so it
                // can self-correct on the next round.
                //
                // U5 (sixth batch): when `precomputed` is Some, the execution
                // already ran concurrently (above) — replay its result + the
                // `validation_failures` counter increment here, then fall
                // through to the SAME serial guards (observer/capture/
                // turn_guard/spill/escalation). Guards run in source order
                // because join_all preserves iteration order. `tool_duration`
                // carries the REAL per-task wall time (measured in the pool),
                // not this near-zero clone.
                let (result, tool_duration_ms) = if let Some(ref pc) = precomputed {
                    let p = &pc[batch_idx];
                    if p.validation_failed {
                        validation_failures += 1;
                        self.record_tool_validation_stats(true);
                    } else {
                        validation_failures = 0;
                        self.record_tool_validation_stats(false);
                    }
                    (p.result.clone(), p.duration_ms)
                } else {
                    let r = match self.check_tool_args(tc) {
                        crate::args_validator::Outcome::Valid => {
                            validation_failures = 0;
                            self.record_tool_validation_stats(false);
                            // G2: dispatch at this instance's sub-agent depth so
                            // depth-aware tools (spawn) enforce max_depth.
                            self.handle_tool_call_at_depth(tc, context, instance.detached_depth())
                                .await
                        }
                        crate::args_validator::Outcome::Fixed(fixed_args) => {
                            validation_failures = 0;
                            self.record_tool_validation_stats(false);
                            info!(
                                "[AgentLoop] Auto-fixed args for tool '{}' (id={})",
                                tc.name, tc.id
                            );
                            let mut fixed = tc.clone();
                            fixed.arguments = fixed_args;
                            self.handle_tool_call_at_depth(
                                &fixed,
                                context,
                                instance.detached_depth(),
                            )
                            .await
                        }
                        crate::args_validator::Outcome::Invalid { message, class } => {
                            validation_failures += 1;
                            self.record_tool_validation_stats(true);
                            warn!(
                                "[AgentLoop] Arg validation failed for tool '{}' (id={}, class={}): {}",
                                tc.name, tc.id, class, message
                            );
                            format!("Tool error: {}", message)
                        }
                    };
                    (r, tool_start.elapsed().as_millis() as u64)
                };
                let tool_duration = std::time::Duration::from_millis(tool_duration_ms);
                let tool_success =
                    !result.starts_with("Error:") && !result.starts_with("Tool error:");

                // Emit tool call observer event.
                self.emit_observer_sync(crate::loop_executor::ObserverEvent::ToolCall {
                    trace_id: trace_id.to_string(),
                    tool_name: tc.name.clone(),
                    success: tool_success,
                    duration_ms: tool_duration.as_millis() as u64,
                    round: turns_used,
                    arguments: tc.arguments.clone(),
                    result: result.clone(),
                })
                .await;

                // [capture] Record the full pre-truncation tool result. loop.rs
                // does NOT truncate tool results before they enter the context,
                // so this is what catches a bloated output blowing out the
                // context window (the suspected bug trigger). No-op unless
                // capture is enabled; flushed only on a later failure signal.
                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                    sink.record_tool(
                        &context.session_key,
                        crate::capture_sink::ToolCapture {
                            tool_name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                            result: result.clone(),
                            success: tool_success,
                            duration_ms: tool_duration.as_millis() as u64,
                            error: if tool_success {
                                String::new()
                            } else {
                                result.clone()
                            },
                            llm_round: turns_used as usize,
                            ts: String::new(),
                        },
                    );
                }

                // Check for async cluster_rpc result — save continuation snapshot.
                //
                // Plan C (template-based UX): the cluster_rpc tool encodes the
                // peer's display name as the 4th part of the marker so we can
                // render a human-friendly "waiting" message here without an
                // extra cluster lookup (this crate can't depend on
                // nemesis-cluster). The full LLM-generated persona response
                // was deferred — it would double cross-node latency and
                // complicate the continuation snapshot. See loop_tools.rs
                // for the encoding site.
                //
                // Format: `__ASYNC__:{task_id}:{target_id}:{target_name}`
                // Older senders may omit the name part (3-segment format),
                // in which case we fall back to the bare target_id.
                if result.starts_with("__ASYNC__:") {
                    let parts: Vec<String> = result.splitn(4, ':').map(|s| s.to_string()).collect();
                    if parts.len() >= 3 {
                        let task_id = parts[1].clone();
                        let target_id = parts[2].clone();
                        let target_name = parts
                            .get(3)
                            .cloned()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| target_id.clone());
                        if let Some(ref mgr) = self.continuation_manager {
                            // Get messages up to this point (including the assistant's tool_call).
                            // We use build_messages() to convert history → LlmMessage format.
                            let messages = self.build_messages(instance);
                            let channel = context.channel.clone();
                            let chat_id = context.chat_id.clone();
                            let session_key = context.session_key.clone();
                            // T6（多模态）：快照只落图片路径引用（磁盘侧剥离
                            // base64 字节）。引用取自 instance 历史最后一条
                            // user turn（即本 turn 的 image_refs，:3468 写入）。
                            let image_refs: Vec<String> = instance
                                .get_history()
                                .iter()
                                .rev()
                                .find(|t| t.role == "user")
                                .map(|t| t.image_refs.clone())
                                .unwrap_or_default();

                            // Save continuation snapshot (spawns a tokio task for disk write)
                            let mgr = mgr.clone();
                            let tc_id = tc.id.clone();
                            let msgs = messages.clone();
                            let task_id_spawn = task_id.clone();
                            // G5: 对端 ID 随快照落盘 —— A 侧重启后 first_start
                            // 恢复 pending 任务时才知道 poll 该问谁。
                            let peer_id_spawn = target_id.clone();
                            tokio::spawn(async move {
                                mgr.save_continuation_with_images(
                                    &task_id_spawn,
                                    msgs,
                                    &tc_id,
                                    &channel,
                                    &chat_id,
                                    &session_key,
                                    &peer_id_spawn,
                                    &image_refs,
                                )
                                .await;
                            });

                            info!(
                                "[AgentLoop] Continuation saved for async cluster_rpc: task_id={}, tool_call_id={}",
                                task_id, tc.id
                            );
                        }

                        // Return an intermediate message to the user and stop processing.
                        // The continuation will resume when the callback arrives.
                        //
                        // NOTE: `is_async_done` in nemesisbot/src/cluster_agent.rs detects
                        // this async path via the `__CLUSTER_ASYNC__` marker in conversation
                        // history, NOT by matching this message text. So the wording here
                        // is free to change without breaking multi-hop (A→B→C) detection.
                        //
                        // The template is deliberately persona-agnostic — this code has
                        // no knowledge of which AI identity is currently loaded (IDENTITY.md
                        // is applied at the LLM layer, not here). Address terms like "老爷"
                        // belong in the persona file, not in hardcoded system messages.
                        // The task_id is omitted from user-visible copy — it's an internal
                        // correlation ID with no meaning to the user.
                        let intermediate = format!("已经联系 {} 了，稍等~", target_name);
                        instance.add_tool_result(&tc.id, &format!(
                            "Request accepted by {}. Task ID: {} | __CLUSTER_ASYNC__{{\"task_id\":\"{}\",\"target\":\"{}\"}}",
                            target_id, task_id, task_id, target_id
                        ));

                        let formatted = context.format_rpc_message(&intermediate);
                        events.push(AgentEvent::Done(formatted));
                        hit_async = true;
                        break;
                    }
                }

                // G4 (devtool-upgrade 阶段 3)：后台 subagent —— spawn 闭包
                // （background=true）已在闭包侧把任务转入后台 tokio 任务并
                // 立即返回此 marker。这里与 __ASYNC__（集群）同构：存续行
                // 快照 + 中间消息收尾本回合；任务完成时闭包侧向 bus 发布
                // `subagent_continuation:{task_id}`，gate_inbound 拦截后走
                // dispatch_continuation → handle_cluster_continuation 全复用
                // （单飞闸认领 + 快照加载 + 续行 + 持久化 + finish_handling
                // 收口自清——磁盘快照保留到最终回复持久化后，发现 F 2026-09-11）。
                //
                // 快照保存必须 **inline await**（不能像 __ASYNC__ 那样
                // spawn）：后台子代理毫秒级即可完成并回灌，spawn 式保存要
                // 等下一个 await 点才落内存，load 端扑空即静默且回复
                // （handle_cluster_continuation 的 debug-skip 分支）。inline
                // 消除该竞态；一次小盘写的延迟可忽略。
                //
                // 格式：`__BG_SPAWN__:{task_id}`（编码端 = agent_factory 注入
                // 的 spawn 闭包）。
                if let Some(bg_task_id) = result.strip_prefix("__BG_SPAWN__:") {
                    let bg_task_id = bg_task_id.trim().to_string();
                    if let Some(ref mgr) = self.continuation_manager {
                        let messages = self.build_messages(instance);
                        let channel = context.channel.clone();
                        let chat_id = context.chat_id.clone();
                        let session_key = context.session_key.clone();
                        // T6（多模态）：快照只落图片路径引用。引用取自
                        // instance 历史最后一条 user turn（同 __ASYNC__ 路径）。
                        let image_refs: Vec<String> = instance
                            .get_history()
                            .iter()
                            .rev()
                            .find(|t| t.role == "user")
                            .map(|t| t.image_refs.clone())
                            .unwrap_or_default();
                        // peer_id 留空：后台任务是进程内 tokio 任务，无对端
                        // 可 poll —— cluster first_start 恢复循环对空 peer_id
                        // 快照本就跳过（留给 TTL 清扫）；重启丢失场景由适配器
                        // 启动期的诚实丢失注入兜底（见 nemesisbot adapters）。
                        mgr.save_continuation_with_images(
                            &bg_task_id,
                            messages,
                            &tc.id,
                            &channel,
                            &chat_id,
                            &session_key,
                            "",
                            &image_refs,
                        )
                        .await;
                        info!(
                            "[AgentLoop] Continuation saved for background subagent: task_id={}, tool_call_id={}",
                            bg_task_id, tc.id
                        );
                    }

                    // 工具结果 marker：与 __CLUSTER_ASYNC__ 刻意不同名，两套
                    // 异步路径在会话历史里互不误判。
                    instance.add_tool_result(&tc.id, &format!(
                        "Background sub-agent accepted. Task ID: {} | __BG_ASYNC__{{\"task_id\":\"{}\"}}",
                        bg_task_id, bg_task_id
                    ));

                    let intermediate =
                        "已派后台子代理任务，完成后结果会自动带回本会话~".to_string();
                    let formatted = context.format_rpc_message(&intermediate);
                    events.push(AgentEvent::Done(formatted));
                    hit_async = true;
                    break;
                }

                let tool_result = ToolCallResult {
                    tool_name: tc.name.clone(),
                    result: result.clone(),
                    is_error: false,
                };
                events.push(AgentEvent::ToolResult(tool_result));

                // ⑤/⑥ Loop guards — mutually exclusive per call (success vs error).
                // Use the shared helper so ExecTool's `Ok("Exit code: N")` for
                // non-zero exits is detected as a failure — otherwise build
                // loops look like success and the guards never fire.
                let tool_succeeded = !crate::turn_guard::tool_result_indicates_error(&result);

                // ⑤ Repeat-success guard: a write-like tool succeeding with
                // identical args is a no-op / write loop → append a nudge.
                //
                // NOTE: keys on `tc.arguments` (the model's ORIGINAL args), not
                // the validator's auto-fixed args. Intentional — if the model
                // keeps re-sending the same (typo'd) args, that IS the repeat
                // we want to catch, regardless of the per-call auto-fix.
                // Detection stays consistent; the signature is the pre-fix form.
                // X1 (U3 projection prune): the pure tool output, kept for
                // history. The ⑤/⑤′/⑥ guard nudges below decorate only the
                // MODEL-facing text; `guard_nudged` marks that a decoration
                // fired (the decorated form cannot be recomputed from the
                // original later, so it must be recorded as the projection
                // override — see the gate block below).
                let original_result = result.clone();
                let mut guard_nudged = false;
                let result = if tool_succeeded {
                    match turn_guard.record_write_success(&tc.name, &tc.arguments) {
                        Some(nudge) => {
                            info!(
                                "[AgentLoop] loop guard: '{}' repeated an identical write; nudging",
                                tc.name
                            );
                            guard_nudged = true;
                            format!("{}\n{}", result, nudge)
                        }
                        None => result,
                    }
                } else {
                    result
                };

                // ⑤′ Read-side repeat guard: a non-write tool succeeding with
                // identical args repeatedly is a re-query loop — the model is
                // not consuming the result it already has. Advisory nudge only
                // (never blocks). Same NOTE as ⑤ above: keys on the model's
                // ORIGINAL args, not the auto-fixed form.
                let result = if tool_succeeded {
                    match turn_guard.record_read_success(&tc.name, &tc.arguments) {
                        Some(nudge) => {
                            info!(
                                "[AgentLoop] loop guard: '{}' repeated an identical read; nudging",
                                tc.name
                            );
                            guard_nudged = true;
                            format!("{}\n{}", result, nudge)
                        }
                        None => result,
                    }
                } else {
                    result
                };

                // C3 (devtool-upgrade 阶段 2) 编辑后诊断回灌：write_file /
                // edit_file 成功后，若该路径语言有已安装 LSP 且
                // `agents.defaults.diagnostics_loop.enabled`，同步文档 →
                // 等 ERROR → 把 ≤max_errors 条追加到工具结果尾部（"please
                // fix"），让模型同轮自纠——修复闭环。插在 ⑤′ 与 spill/gate
                // 之间：反馈与工具结果同走一条模型可见管线（gate/spill/
                // projection 都作用于装饰后的文本）。失败路径全部静默
                // （开关关 / 非 write|edit / 无 manager / 无服务器 /
                // 同步失败 / 无 ERROR）——永不拖垮工具调用。
                let result = if tool_succeeded
                    && matches!(tc.name.as_str(), "write_file" | "edit_file")
                    && let Ok(args_val) = serde_json::from_str::<serde_json::Value>(&tc.arguments)
                    && let Some(path_str) = args_val.get("path").and_then(|v| v.as_str())
                {
                    self.diagnostics_feedback(&tc.name, path_str, &result).await
                } else {
                    result
                };

                // G3 (U3) + G4 (U4) + X1 (U3 projection prune): model-free
                // size gates, computed here but applied at the PROJECTION
                // (build_messages), not at history-write time. History keeps
                // `original_result` (the mid-section stays recoverable for
                // history replay / session branching); `gate_text` is the
                // bounded model-facing form the OLD code stored in history —
                // byte-identical to pre-X1 behavior:
                //   >= SPILL_THRESHOLD_CHARS  → spill whole text to disk, keep
                //                               preview + locator (readable back
                //                               via read_file offset/limit).
                //   > MAX_TOOL_RESULT_INLINE_CHARS (and no spill) → head +
                //                               marker + tail prune.
                // Spill is best-effort: a storage failure falls through to the
                // prune tier (a spill failure must never lose a successful
                // tool call's content outright).
                //
                // B3 (devtool-upgrade 阶段 3): when the registry HAS a spawn
                // tool, both gate texts append the sub-agent hint (the elided
                // content is unreadable inline — a spawned sub-agent can read
                // it and return a digest). Hinted prune text is registry-
                // state-dependent and NOT recomputable by the pure projection
                // path, so `prune_hinted` forces the projection override
                // (same ledger as spill/nudges); replay recomputes without
                // hint only for unhinted turns, which are byte-stable.
                let hint_subagent = self.tools.read().contains_key("spawn");
                let mut spill_applied = false;
                let mut prune_hinted = false;
                let gate_text: String = {
                    let spill_root = self.spill_root.read().clone();
                    let spilled = spill_root.as_ref().and_then(|root| {
                        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S%3f").to_string();
                        match crate::spill::spill_tool_result(
                            &result,
                            &tc.name,
                            root,
                            &context.session_key,
                            &stamp,
                            &tc.id,
                            hint_subagent,
                        ) {
                            crate::spill::SpillOutcome::Spilled(text) => Some(text),
                            crate::spill::SpillOutcome::SpillFailed => {
                                warn!(
                                    "[AgentLoop] tool result spill failed for '{}' — falling back to prune tier",
                                    tc.name
                                );
                                None
                            }
                            crate::spill::SpillOutcome::BelowThreshold => None,
                        }
                    });
                    match spilled {
                        Some(text) => {
                            info!(
                                "[AgentLoop] tool result spilled to disk: '{}' result >= {} chars",
                                tc.name,
                                crate::spill::SPILL_THRESHOLD_CHARS
                            );
                            spill_applied = true;
                            text
                        }
                        None => {
                            match crate::prune::prune_tool_result(&result, &tc.name, hint_subagent)
                            {
                                Some(pruned) => {
                                    info!(
                                        "[AgentLoop] tool result pruned: '{}' result exceeded {} chars",
                                        tc.name,
                                        crate::prune::MAX_TOOL_RESULT_INLINE_CHARS
                                    );
                                    prune_hinted = hint_subagent;
                                    pruned
                                }
                                None => result,
                            }
                        }
                    }
                };

                // ⑥ Alternating-loop guard: per-turn (tool, error) failure
                // frequency, NOT reset by intervening successes (also handles ④
                // storm — consecutive identical — internally). On a repeated
                // failure, append a nudge so the model sees it in the error.
                // Signature input is the post-gate text — same as pre-X1.
                let error_for_guard: Option<&str> = if tool_succeeded {
                    None
                } else {
                    Some(&gate_text)
                };
                let nudge6 = turn_guard
                    .record_tool_outcome(&tc.name, error_for_guard)
                    .inspect(|_nudge| {
                        info!(
                            "[AgentLoop] loop guard: '{}' repeating the same failure within this turn; nudging",
                            tc.name
                        );
                    });

                // X1: the recorded projection override — only when the final
                // model-facing text cannot be recomputed from the original
                // later: the spill tier (locator path embeds a wall-clock
                // stamp), any guard-nudge decoration (⑤/⑤′/⑥ — dynamic
                // per-turn state), or B3's spawn-hinted prune (the hint flag
                // is registry state, not turn data). Otherwise None and
                // build_messages recomputes the pure prune (deterministic,
                // ledger-free).
                let projection: Option<String> =
                    if spill_applied || guard_nudged || nudge6.is_some() || prune_hinted {
                        Some(match &nudge6 {
                            Some(nudge) => format!("{}\n{}", gate_text, nudge),
                            None => gate_text.clone(),
                        })
                    } else {
                        None
                    };
                instance.add_tool_result_projected(&tc.id, &original_result, &tc.name, projection);

                // H5 (U18): touch-driven instruction-chain invalidation. A
                // successful read_file/write_file/edit_file may have touched
                // a file on the workspace instruction chain — invalidate the
                // context digests so the next build re-reads the chain.
                // (File-level check only: the re-read happens at injection
                // time. Rare + cheap: only fires for these three tools and
                // only when the path matches a chain file name.)
                if tool_succeeded
                    && matches!(tc.name.as_str(), "read_file" | "write_file" | "edit_file")
                    && let Ok(args_val) = serde_json::from_str::<serde_json::Value>(&tc.arguments)
                    && let Some(path_str) = args_val.get("path").and_then(|v| v.as_str())
                {
                    let touched = std::path::PathBuf::from(path_str);
                    // I1 (devtool-upgrade 阶段 3): record agent-authored
                    // writes so the fs watcher's event for the same path is
                    // dropped inside the self-write window (the agent knows
                    // what it just wrote — must not surface as "外部修改").
                    if matches!(tc.name.as_str(), "write_file" | "edit_file") {
                        self.note_self_write(path_str);
                    }
                    // I3 (devtool-upgrade 阶段 3): lazy sub-directory
                    // instruction discovery on successful reads — the file's
                    // directory may carry AGENTS.md/CLAUDE.md that was never
                    // injected (root chain only, by default). Queued here,
                    // injected one-shot at the next build.
                    if tc.name == "read_file" {
                        self.note_read_for_instructions(instance, path_str);
                    }
                    let ws_root = self.workspace_root.read().clone();
                    if let Some(ref root) = ws_root {
                        // Chain files are <dir>/AGENTS.md or CLAUDE.md
                        // under the workspace — check by file name to
                        // avoid re-reading the whole chain on every
                        // file op (the full path_is_on_chain check
                        // happens against the loaded chain at
                        // injection; here the name match is the
                        // conservative trigger).
                        let name = touched
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if name == "AGENTS.md" || name == "CLAUDE.md" {
                            let chain =
                                crate::workspace_instructions::load_instruction_chain(root, root);
                            if crate::workspace_instructions::path_is_on_chain(&chain, &touched) {
                                info!(
                                    "[AgentLoop] instruction-chain file touched: {} — context digest invalidated",
                                    touched.display()
                                );
                                self.invalidate_context_digests();
                            }
                        }
                    }
                }

                // ⑥ Escalation: same (tool, error) failed past the hard-stop
                // threshold → nudges are being ignored. Latch a stop event and
                // break the tool batch; the outer-scope check after this for-loop
                // ends the turn (a bare `break` here only exits the batch, not
                // the LLM loop).
                //
                // J5 (devtool-upgrade 阶段 6)：`agents.doom_loop_approval` 开且
                // question asker 已装配时，先发提问卡问用户「继续吗？」——
                // approve = 清签名计数继续；deny / 超时 / 通路缺失 / 开关关 =
                // 现行为（停轮）。turn_guard 现行为是安全底座，开关默认关。
                if let Some((sig, count)) = turn_guard.escalating_signature() {
                    let approved = self.current_doom_loop_approval()
                        && self
                            .ask_doom_loop_approval(&sig, count, context)
                            .await
                            .unwrap_or(false);
                    if approved {
                        warn!(
                            "[AgentLoop] loop guard escalation on '{}' (x{}) — user approved, clearing signature count and continuing",
                            sig.split('\x00').next().unwrap_or("tool"),
                            count
                        );
                        turn_guard.clear_signature(&sig);
                    } else {
                        warn!(
                            "[AgentLoop] loop guard escalation: stopping turn to avoid burning max_turns on a stuck loop"
                        );
                        // P1（2026-09-11 真机日志分析）：升级停轮也是失败终止——
                        // 记终端原因，turn_end 边界标记不再谎报 "done"。
                        terminal_reason = Some("escalation");
                        force_stop = Some(AgentEvent::Done(context.format_rpc_message(
                            &crate::turn_guard::TurnGuard::escalation_message(&sig, count),
                        )));
                        break;
                    }
                }

                // Phase 2: bound consecutive validation failures so a struggling
                // model cannot burn the whole max_turns budget on the same
                // malformed arguments. Same latch pattern as escalation — a bare
                // `break` here used to only exit the batch while the outer LLM
                // loop kept calling the model (the "stopping loop" log was a lie,
                // observed in a deployed cluster test). Now it actually ends the
                // turn, giving the model exactly `validation_retry_budget` retries.
                if validation_failures >= self.validation_retry_budget() {
                    warn!(
                        "[AgentLoop] Validation retry budget exhausted ({}); stopping turn.",
                        validation_failures
                    );
                    // P1（2026-09-11 真机日志分析）：校验预算耗尽的 turn 是失败
                    // 终止（B 端曾把它包装成 success 回调 → 空交付结构性缺陷）。
                    // 记终端原因 + 末事件为 Error——cluster_agent 据此发 error 回调。
                    terminal_reason = Some("validation_exhausted");
                    force_stop = Some(AgentEvent::Error(format!(
                        "工具参数校验连续失败 {} 次，已停止重试。最近工具：'{}'。",
                        validation_failures, tc.name
                    )));
                    break;
                }
            }

            if hit_async {
                break;
            }

            // Outer-scope turn stop latched from inside the tool-call for-loop
            // (⑥ escalation OR validation-budget exhaustion). A bare `break` in
            // that for-loop only exits the batch; this actually ends the turn,
            // emitting a single terminal event.
            if let Some(ev) = force_stop {
                events.push(ev);
                break;
            }
        }

        instance.set_state(crate::types::AgentState::Idle);

        // I3 (U9): durable turn_end marker with the terminal reason.
        // L2 (full review): the reason comes from the break-site latch
        // (terminal_reason), not from sniffing the Done text — a model
        // reply containing the paused-after wording can no longer be
        // misclassified as max_turns.
        let end_reason = if cancel_token.is_cancelled() {
            "cancelled"
        } else {
            terminal_reason.unwrap_or("done")
        };
        if log_boundaries {
            crate::chat_log::append_boundary_event(&context.session_key, "turn_end", end_reason);
        } else if terminal_reason == Some("budget_exhausted") {
            // T3 (U12): cron turns are exempt from per-turn boundary events
            // (a recurring job would grow the sidecar unboundedly), but a
            // budget-exhausted stop is a rare, one-shot terminal fact worth
            // exactly one marker — the budget's observability requirement.
            crate::chat_log::append_boundary_event(&context.session_key, "turn_end", end_reason);
        }

        events
    }

    // -----------------------------------------------------------------------
    // Tool handling
    // -----------------------------------------------------------------------

    /// U5 (sixth batch), G3-extended: is `name` eligible for the parallel
    /// pool? Looks up the agent-side registry and asks the tool's
    /// `is_parallel_safe()` (= `is_read_only()` by default; spawn opts in
    /// explicitly). Fail-closed: unknown tools and writer tools return
    /// false → never join the parallel pool. When executor separation is
    /// ON, the MOVE_TOOLS (incl. read_file/list_dir/grep) are
    /// `RemoteExecutorTool` instances whose `is_read_only()` is the default
    /// `false` — so an executor-separated batch naturally falls back to
    /// serial here, no separate check needed.
    fn tool_is_parallel_safe(&self, name: &str) -> bool {
        match self.tools.read().get(name) {
            Some(t) => t.is_parallel_safe(),
            None => false,
        }
    }

    /// U5: the result of one parallel-executed call, captured for serial
    /// guard replay in source order. `validation_failed` lets the serial
    /// loop replay the `validation_failures` counter exactly as the serial
    /// path would (Invalid → +1; Valid/Fixed → reset to 0).
    ///
    /// U5/G3: concurrently execute an ALL-parallel-safe batch (validated
    /// read-only-or-opted-in by the caller). Each task runs the SAME
    /// execution path as the serial loop's match (`check_tool_args` →
    /// `handle_tool_call_at_depth` or synthesize an Invalid error), gated
    /// by a 4-permit semaphore. Every dispatch goes through the depth-aware
    /// variant with the invoking instance's `detached_depth` — behaviorally
    /// identical to the old depth-0 dispatch for read-only tools (their
    /// `set_invocation_depth` is the trait's default no-op) and exactly
    /// what spawn needs for `max_depth` enforcement (G2/G3). Returns
    /// results in SOURCE ORDER (`join_all` preserves iteration order) so
    /// the serial guard-replay keeps the audit chain ordered = model
    /// source order (roadmap risk 3 hard constraint). cluster_rpc/exec/
    /// writers are never here (not parallel-safe) → the `__ASYNC__`
    /// continuation and executor paths are structurally excluded; spawn's
    /// own `__BG_SPAWN__` marker is handled by the unchanged inline-await
    /// replay in the for-loop.
    async fn precompute_parallel_batch(
        &self,
        tool_calls: &[ToolCallInfo],
        context: &RequestContext,
        depth: usize,
    ) -> Vec<PrecomputedTool> {
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let futs = tool_calls.iter().map(|tc| {
            let sem = sem.clone();
            let tc = tc.clone();
            async move {
                let _permit = sem.acquire().await.ok();
                let start = std::time::Instant::now();
                let (result, validation_failed) = match self.check_tool_args(&tc) {
                    crate::args_validator::Outcome::Valid => (
                        self.handle_tool_call_at_depth(&tc, context, depth).await,
                        false,
                    ),
                    crate::args_validator::Outcome::Fixed(fixed_args) => {
                        info!(
                            "[AgentLoop] Auto-fixed args for tool '{}' (id={})",
                            tc.name, tc.id
                        );
                        let mut fixed = tc.clone();
                        fixed.arguments = fixed_args;
                        (
                            self.handle_tool_call_at_depth(&fixed, context, depth).await,
                            false,
                        )
                    }
                    crate::args_validator::Outcome::Invalid { message, .. } => {
                        warn!(
                            "[AgentLoop] Arg validation failed for tool '{}' (id={}): {}",
                            tc.name, tc.id, message
                        );
                        (format!("Tool error: {}", message), true)
                    }
                };
                PrecomputedTool {
                    result,
                    validation_failed,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }
        });
        futures::future::join_all(futs).await
    }

    /// Build the LLM-visible tool definitions from the registry.
    ///
    /// Extracted verbatim from `run_llm_loop` (K1b, U14) so the post-LLM
    /// hook retry re-call rebuilds them from the same single source instead
    /// of duplicating the tier-filter/sort block (the transient-retry path
    /// predates the extraction and keeps its inline copy — untouched).
    ///
    /// Mirrors Go's ToolRegistry.ToProviderDefs() which calls
    /// tool.Description() and tool.Parameters(). Sort by name so the order is
    /// stable across runs — a deterministic tool order gives reproducible
    /// behaviour and avoids unnecessary prompt variation between requests.
    fn build_tool_defs(&self) -> Vec<crate::types::ToolDefinition> {
        let tools_guard = self.tools.read();
        // Phase 3 (small-model-tool-robustness): tier-based toolset.
        // Empty allowed-list (Big/Auto) = show everything; Mini/Normal
        // see a restricted set to reduce small-model cognitive load.
        let allowed = nemesis_types::capability::tier_allowed_tools(*self.tier.read());
        // F8 (devtool-upgrade 阶段 3): third filter layer — config
        // `agents.hidden_tools` removes tools from the model's supply
        // entirely (dispatch re-checks the same list: double gate). Read
        // FRESH each call so dashboard/CLI edits apply from the next turn.
        let hidden = self.current_hidden_tools();
        // F1 (devtool-upgrade 阶段 4): Plan 模式第四过滤层——只读白名单
        // ∩（MCP 前缀工具与 spawn 豁免：MCP 语义未知不猜；spawn 保留供给、
        // 分发闸兜底）。与 tier 过滤正交（Mini+Plan 取交集，验收项）。
        let plan_mode = *self.mode.read() == crate::types::AgentMode::Plan;
        let mut names: Vec<&String> = tools_guard.keys().collect();
        names.sort();
        names
            .into_iter()
            .filter(|name| allowed.is_empty() || allowed.contains(&name.as_str()))
            .filter(|name| !tool_name_matches_hidden(&hidden, name))
            .filter(|name| {
                !plan_mode
                    || Self::PLAN_MODE_TOOLS.contains(&name.as_str())
                    || name.starts_with("mcp_")
                    || name.as_str() == "spawn"
            })
            .filter_map(|name| tools_guard.get(name).map(|tool| (name, tool)))
            .map(|(name, tool)| crate::types::ToolDefinition {
                tool_type: "function".to_string(),
                function: crate::types::ToolFunctionDef {
                    name: name.clone(),
                    description: tool.description(),
                    parameters: tool.parameters(),
                },
            })
            .collect()
    }

    /// G0 (devtool-upgrade 阶段 3)：本回合生效的工具 defs 供给链——
    /// 全量注册 → tier 过滤 → **子代理白名单收窄**（`run_detached` 派生的
    /// instance 才带 `detached_allowed_tools`，普通回合 None 直通）→ 文档折叠。
    /// instance 级白名单（非 loop 级全局槽）：并发 detached 回合互不串扰。
    fn effective_tool_defs(&self, instance: &AgentInstance) -> Vec<crate::types::ToolDefinition> {
        // Swarm M1（裸提示词模式）：零工具供给直返空——纯文本单轮调用，
        // 模型看不到任何工具定义（优先级高于 tier/白名单/hidden 全链）。
        if instance.detached_no_tools() {
            return Vec::new();
        }
        let mut defs = self.build_tool_defs();
        if let Some(allowed) = instance.detached_allowed_tools()
            && !allowed.is_empty()
        {
            defs.retain(|d| allowed.contains(&d.function.name));
        }
        // 空白名单 = 不设限（与 run_detached 的 `!allowed.is_empty()` 守卫
        // 同语义——否则 Some(空) 会把供给收窄到零工具，两端语义劈叉）。
        self.apply_tool_doc_folding(defs, instance)
    }

    /// Y1 (Phase4-a): read `agents.tool_doc_folding` from config.json FRESH
    /// each call (same pattern as [`current_summarizer_prefix_reuse`] — the
    /// dashboard/CLI can flip it while the gateway runs). Absent section,
    /// unreadable file, or a standalone loop (no config_path) →
    /// `(false, default)` — folding off, tool defs byte-identical.
    pub(crate) fn current_tool_doc_folding(&self) -> (bool, usize) {
        const OFF: (bool, usize) = (false, crate::tool_doc_folding::DEFAULT_EXPAND_TOP_N);
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return OFF,
        };
        let v = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let Some(v) = v else {
            return OFF;
        };
        let sec = v.get("agents").and_then(|a| a.get("tool_doc_folding"));
        let enabled = sec
            .and_then(|s| s.get("enabled"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        let top_n = sec
            .and_then(|s| s.get("expand_top_n"))
            .and_then(|n| n.as_u64())
            .map(|n| n as usize)
            .unwrap_or(crate::tool_doc_folding::DEFAULT_EXPAND_TOP_N);
        (enabled, top_n)
    }

    /// F8 (devtool-upgrade 阶段 3): read `agents.hidden_tools` from
    /// config.json FRESH each call (same pattern as
    /// [`Self::current_tool_doc_folding`] — dashboard/CLI edits take effect
    /// from the next turn without restarting the loop). Absent key,
    /// unreadable file, or a standalone loop (no config_path) → empty list
    /// (nothing hidden).
    pub(crate) fn current_hidden_tools(&self) -> Vec<String> {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let v = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        v.and_then(|v| {
            v.get("agents")
                .and_then(|a| a.get("hidden_tools"))
                .and_then(|h| h.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| e.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<String>>()
                })
        })
        .unwrap_or_default()
    }

    /// J5 (devtool-upgrade 阶段 6)：`agents.doom_loop_approval` fresh-read
    /// （F8 `current_hidden_tools` 同款模式——config.json 是唯一真相源，每次
    /// escalation 现读，运行时改键下一轮生效，无需重启）。无 config_path /
    /// 解析失败 = `false`（安全底座方向）。
    pub(crate) fn current_doom_loop_approval(&self) -> bool {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return false,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("agents")
                    .and_then(|a| a.get("doom_loop_approval"))
                    .and_then(|b| b.as_bool())
            })
            .unwrap_or(false)
    }

    /// J6：超限图片自动降采样开关（F8 模式 fresh-read config.json；默认**开**，
    /// 极性与 doom_loop_approval 相反——读不到/键缺失按开处理，保持降采样
    /// 能力可用；读取失败不阻塞轮次）。
    pub(crate) fn current_image_downscale(&self) -> bool {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return true,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("agents")
                    .and_then(|a| a.get("image_downscale"))
                    .and_then(|b| b.as_bool())
            })
            .unwrap_or(true)
    }

    /// J5：doom-loop 审批卡。经 [`Self::question_asker`]（F7 同源 broker 的
    /// ask 端）发结构化提问「继续吗？」，阻塞等用户作答。
    ///
    /// 返回：`Some(true)` = 用户选「继续」（调用方清签名计数继续）；
    /// `Some(false)` = 拒绝或超时（调用方走现行为停轮）；`None` = asker
    /// 未装配或 ask 通路异常（同现行为，fail-closed）。
    async fn ask_doom_loop_approval(
        &self,
        sig: &str,
        count: u32,
        context: &RequestContext,
    ) -> Option<bool> {
        let asker = self.question_asker()?;
        let tool = sig.split('\x00').next().unwrap_or("tool").to_string();
        let request = nemesis_types::agent::QuestionRequest {
            question_id: crate::loop_tools::next_question_id(),
            question: format!(
                "循环守卫拦截：{} 已连续 {} 次报相同错误且未响应纠正提示，继续执行吗？",
                tool, count
            ),
            options: vec!["继续执行".to_string(), "停止".to_string()],
            multi: false,
            chat_id: context.chat_id.clone(),
            session_key: context.session_key.clone(),
            timeout_secs: crate::loop_tools::QUESTION_DEFAULT_TIMEOUT_SECS,
        };
        // ask 是同步阻塞（最长 120s）——与 question 工具同形：spawn_blocking
        // 出 worker 线程，broker 内部按上下文决定 block_in_place/直等。
        let outcome = tokio::task::spawn_blocking(move || asker.ask(request))
            .await
            .ok()
            .and_then(|r| r.ok());
        match outcome {
            Some(nemesis_types::agent::QuestionOutcome::Answered(selected)) => {
                Some(selected.iter().any(|s| s == "继续执行"))
            }
            _ => Some(false),
        }
    }

    /// Y1 (Phase4-a): semantic tool-documentation folding, applied AFTER the
    /// tier filter and ORTHOGONAL to tool supply — folded tools stay
    /// callable with their full parameter schema; only the description text
    /// shrinks. Returns the defs byte-unchanged unless EVERY gate opens:
    /// config enabled, tier != Mini (the 13-tool core set has nothing to
    /// save), a wired memory manager (P3.1 embed backend), a non-empty
    /// latest user message, and successful embeddings for the query and
    /// every tool description (all-or-nothing — a tool that cannot be ranked
    /// must not be folded). Deterministic for a given (descriptions, query,
    /// embed backend) triple, so the two call sites (main call + hook retry
    /// re-call) and any rebuild that re-derives defs render the same bytes.
    fn apply_tool_doc_folding(
        &self,
        defs: Vec<crate::types::ToolDefinition>,
        instance: &AgentInstance,
    ) -> Vec<crate::types::ToolDefinition> {
        #[cfg(feature = "memory")]
        let (enabled, top_n) = self.current_tool_doc_folding();
        #[cfg(not(feature = "memory"))]
        let (enabled, _) = self.current_tool_doc_folding();
        if !enabled {
            return defs;
        }
        if *self.tier.read() == nemesis_types::capability::ModelTier::Mini {
            return defs;
        }
        #[cfg(feature = "memory")]
        {
            // Latest USER message is the ranking signal (same choice as
            // `prefetch_memory_context`).
            let query = instance
                .get_history()
                .iter()
                .rev()
                .find(|t| t.role == "user")
                .map(|t| t.content.clone())
                .unwrap_or_default();
            if query.trim().is_empty() {
                return defs;
            }
            let manager = match self.memory_inject_manager.read().clone() {
                Some(m) => m,
                None => {
                    debug!(
                        "[AgentLoop] tool_doc_folding enabled but no memory manager (embed \
                         backend) wired — leaving docs unfolded"
                    );
                    return defs;
                }
            };
            let query_vec = match manager.embed_text(&query) {
                Some(v) => v,
                None => {
                    debug!(
                        "[AgentLoop] tool_doc_folding: query embedding failed — leaving docs \
                         unfolded"
                    );
                    return defs;
                }
            };
            let mut sims: std::collections::HashMap<String, f32> =
                std::collections::HashMap::with_capacity(defs.len());
            {
                // Cache guard held only for the embed-and-rank pass (the fold
                // render itself touches no shared state).
                let mut cache = self.tool_vec_cache.write();
                for d in &defs {
                    let cached = cache.get(&d.function.name);
                    let vec = match cached {
                        Some((desc, v)) if desc == &d.function.description => v.clone(),
                        _ => match manager.embed_text(&d.function.description) {
                            Some(v) => {
                                cache.insert(
                                    d.function.name.clone(),
                                    (d.function.description.clone(), v.clone()),
                                );
                                v
                            }
                            None => {
                                debug!(
                                    "[AgentLoop] tool_doc_folding: embedding failed for tool \
                                     {} — leaving docs unfolded",
                                    d.function.name
                                );
                                return defs;
                            }
                        },
                    };
                    let sim = crate::tool_doc_folding::cosine(&query_vec, &vec);
                    sims.insert(d.function.name.clone(), sim);
                }
            }
            crate::tool_doc_folding::fold_tool_defs(defs, &sims, top_n)
        }
        #[cfg(not(feature = "memory"))]
        {
            // No embed backend compiled in — folding cannot rank; passthrough.
            let _ = instance;
            defs
        }
    }

    /// Wait until the e-stop watch flips to engaged, or forever if no
    /// receiver is wired (`None` → pending, i.e. the select arm never fires).
    ///
    /// Extracted verbatim from the estop arm of the primary LLM-call select
    /// (K1b, U14) — the post-LLM hook retry re-call needs the identical
    /// subscribe-window-safe wait (if already engaged at subscribe time,
    /// return immediately instead of waiting for the NEXT change).
    async fn wait_estop_engaged(rx: Option<&mut tokio::sync::watch::Receiver<bool>>) {
        match rx {
            Some(rx) => {
                // 订阅时已经 engaged（checkpoint A → subscribe 之间的窗口）
                // → 立刻 return，否则 changed() 会干等下一次变化、漏掉这次。
                if *rx.borrow() {
                    return;
                }
                while rx.changed().await.is_ok() {
                    if *rx.borrow() {
                        return;
                    }
                }
            }
            None => {
                std::future::pending::<()>().await;
            }
        }
    }

    /// Execute a single tool call at the top-level depth (0).
    ///
    /// G2 thin wrapper: external callers (hooks/cc_hooks/loop tests, any
    /// non-subagent dispatch) mean "top-level agent invoked this" — they
    /// stay untouched and get depth 0. The production serial dispatch in
    /// `run_llm_loop` calls [`Self::handle_tool_call_at_depth`] with the
    /// instance's detached depth instead.
    pub async fn handle_tool_call(
        &self,
        tool_call: &ToolCallInfo,
        context: &RequestContext,
    ) -> String {
        self.handle_tool_call_at_depth(tool_call, context, 0).await
    }

    /// G2: depth-aware variant of [`Self::handle_tool_call`]. `depth` is the
    /// invoking instance's sub-agent nesting depth (instance.detached_depth());
    /// it is injected into depth-aware tools (spawn) before execution so the
    /// `agents.subagent.max_depth` limit can reject over-deep spawns.
    pub async fn handle_tool_call_at_depth(
        &self,
        tool_call: &ToolCallInfo,
        context: &RequestContext,
        depth: usize,
    ) -> String {
        info!(
            "[AgentLoop] Executing tool: {} (id={})",
            tool_call.name, tool_call.id
        );

        // 全局急停：触发时拒绝所有工具分发。这是 handle_tool_call 公开入口级
        // 的防御深度——与 run_llm_loop 里的批次检查点互补，任何调用方都吃到。
        let estop_engaged = self
            .estop
            .read()
            .as_ref()
            .map(|e| e.is_engaged())
            .unwrap_or(false);
        if estop_engaged {
            warn!(
                "[AgentLoop] E-stop engaged — tool {} refused.",
                tool_call.name
            );
            return "⛔ ESTOP: 已急停 — 工具调用已被拒绝 (e-stop engaged). 不要重试；告知用户当前处于急停状态，等待释放。"
                .to_string();
        }

        // F8 (devtool-upgrade 阶段 3): dispatch-side hidden gate — the second
        // half of the `agents.hidden_tools` double gate. Supply-side filtering
        // (build_tool_defs) keeps hidden tools out of the defs, but a stale
        // prompt cache / in-flight request can still carry an old defs snapshot;
        // re-checking the same list here (same matcher, same fresh read) makes
        // the hidden state authoritative at execution time too. Runs BEFORE the
        // security pipeline so a hidden tool costs no judge/scanner work.
        {
            let hidden = self.current_hidden_tools();
            if tool_name_matches_hidden(&hidden, &tool_call.name) {
                warn!(
                    "[AgentLoop] Hidden tool {} refused (agents.hidden_tools).",
                    tool_call.name
                );
                return format!(
                    "Error: Tool '{}' is hidden by configuration (agents.hidden_tools) and cannot be executed. Inform the user; do NOT retry unless they un-hide it.",
                    tool_call.name
                );
            }
        }

        // F1 (devtool-upgrade 阶段 4): Plan 模式分发闸——供给侧
        // （build_tool_defs 白名单过滤）之外的第二道闸，与 F8 同一防御模型：
        // 陈旧 prompt cache / 子代理 full 档（其 defs 也被 loop 级过滤，但
        // 未知 MCP 写工具不在过滤范围）都从这里兜底。深度无关：子代理
        // （depth ≥ 1）同样被拦——Plan 语义是「这个 loop 现在不许改文件」，
        // 委托子代理不能成为旁路。位置在 security 之前：模式级拒绝不必
        // 白付 judge/scanner 成本（F8 同理由）。唯一写放行 = write_file
        // 且路径落在 `<workspace>/plans/`（计划产物落盘）。C7：lsp 的
        // rename op 是跨文件改内容——同受 Plan 拦截（其余 lsp op 只读）。
        if *self.mode.read() == crate::types::AgentMode::Plan
            && ((Self::PLAN_MODE_WRITE_TOOLS.contains(&tool_call.name.as_str())
                && !self.plan_mode_write_allowed(&tool_call.name, &tool_call.arguments))
                || Self::is_lsp_write_call(&tool_call.name, &tool_call.arguments))
        {
            warn!(
                "[AgentLoop] Plan mode: write-class tool {} refused.",
                tool_call.name
            );
            return "Plan mode: file modifications are denied. Present your plan as text, or ask the user to switch with /build. (Exception: write_file under <workspace>/plans/ is allowed for saving the plan itself.)"
                .to_string();
        }

        // Pre-execution security check (mirrors Go's PluginableTool.Execute → PluginManager → SecurityPlugin).
        #[cfg(feature = "security")]
        {
            if let Some(ref security) = self.security_plugin {
                let args_value = serde_json::from_str::<serde_json::Value>(&tool_call.arguments)
                    .unwrap_or(serde_json::Value::Null);
                let invocation = nemesis_security::types::ToolInvocation {
                    tool_name: tool_call.name.clone(),
                    args: args_value,
                    user: String::new(),
                    source: context.channel.clone(),
                    // K4 (b)（devtool-upgrade 阶段 7）：审批来源上下文随行——
                    // pipeline 层 3 提取后传给审批管理器，IM 通道的审批卡
                    // 路由回发起对话（web/dashboard 走原路径不受影响）。
                    metadata: {
                        let mut m = std::collections::HashMap::new();
                        m.insert("approval_chat_id".to_string(), context.chat_id.clone());
                        m.insert("approval_sender_id".to_string(), context.user.clone());
                        m
                    },
                };
                let (allowed, deny) = security.execute(&invocation);
                if !allowed {
                    // F5: 结构化 deny 反馈——回灌带 layer/policy/suggestion
                    // 三要素（summary 是各层原文），并明示模型不要原样重试。
                    let info = deny.unwrap_or_else(|| nemesis_security::types::DenyInfo {
                        layer: "unknown",
                        policy: "security_pipeline".to_string(),
                        summary: "operation denied by security policy".to_string(),
                        suggestion: None,
                    });
                    warn!(
                        "[AgentLoop] Security blocked tool {}: [{}:{}] {}",
                        tool_call.name, info.layer, info.policy, info.summary
                    );
                    // Use a very explicit prefix so the LLM cannot misinterpret this
                    // as a generic error (e.g. "file not found"). The LLM must
                    // understand that the USER or SECURITY POLICY blocked the action.
                    let suggestion_line = info
                        .suggestion
                        .as_deref()
                        .map(|s| format!("\n建议：{s}"))
                        .unwrap_or_default();
                    return format!(
                        "⛔ SECURITY BLOCKED [layer:{}|policy:{}] {}\nDo NOT retry the same call unchanged. Inform the user that the operation was rejected.{}",
                        info.layer, info.policy, info.summary, suggestion_line
                    );
                }
                // P5: guardian (LLM safety judge) review for CRITICAL tools. Runs only
                // after the rule layers allow, and only for CRITICAL operations (cost
                // bounded). A Deny verdict blocks; errors/Allow proceed (the guardian
                // only escalates — rules already denied cases returned above).
                if security.is_critical_tool(&tool_call.name)
                    && let Some(judge) = security.judge()
                {
                    let req = nemesis_security::guardian::JudgeRequest {
                        action: tool_call.name.clone(),
                        risk_level: "critical".to_string(),
                        transcript: tool_call.arguments.clone(),
                    };
                    if let Ok(v) = judge.judge(&req).await
                        && v.outcome == nemesis_security::guardian::JudgeOutcome::Deny
                    {
                        warn!(
                            "[AgentLoop] Guardian denied critical tool {}: {}",
                            tool_call.name, v.rationale
                        );
                        return format!(
                            "⛔ GUARDIAN DENIED [layer:guardian|policy:llm_judge] {} — The safety judge flagged this critical operation as unsafe. Do NOT retry. Inform the user.",
                            v.rationale
                        );
                    }
                }
            }
        }

        // K1a (U14): user tool hooks. Pre hooks run here — AFTER the fixed
        // security gate above (which stays inline as the de-facto pre[0]; see
        // crate::hooks module doc for why it wasn't converted to a trait
        // object) and BEFORE context injection / checkpoint / execute.
        // Ordered, first Block wins. Fires on every dispatch attempt,
        // including unknown tool names (a hook may deny what the model
        // *tried* to call — mirrors the dialect's PreToolUse).
        let hook_call = crate::hooks::HookToolCall {
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.clone(),
            channel: context.channel.clone(),
            chat_id: context.chat_id.clone(),
            session_key: context.session_key.clone(),
        };
        {
            let hooks = self.tool_hooks.read().snapshot();
            if let Some(reason) = crate::hooks::run_pre_hooks(&hooks, &hook_call).await {
                warn!(
                    "[AgentLoop] Hook blocked tool {}: {}",
                    tool_call.name, reason
                );
                return format!(
                    "⛔ HOOK BLOCKED [layer:hook|policy:tool_hook] {} — A registered hook denied this operation. Do NOT retry unless the user changes the hook policy. Inform the user if this keeps blocking.",
                    reason
                );
            }
        }

        // Inject channel/chat_id into context-aware tools before execution.
        // Mirrors loop_executor.rs:1634 which calls set_context for AgentLoopExecutor.
        // G2: also inject the invocation depth for depth-aware tools (spawn) —
        // the accepted set-then-read race on a shared SpawnTool only marginally
        // misattributes depth between concurrent dispatches (same class as the
        // pre-existing set_context race).
        {
            let guard = self.tools.read();
            if let Some(tool) = guard.get(&tool_call.name) {
                tool.set_context(&context.channel, &context.chat_id);
                tool.set_invocation_depth(depth);
            }
        }

        #[cfg(feature = "forge")]
        let tool_start = std::time::Instant::now();
        // K1a 三段化（2026-08-29）：around 链——scoped
        // hooks 逆序包装真实执行，Err 分支走 post_tool_use_failure 变体
        // （PostToolUseFailure 语义挂点）。作用域过滤：主 agent 只接 None。
        let scoped_hooks: Vec<std::sync::Arc<dyn crate::hooks::ToolHook>> = {
            let hooks = self.tool_hooks.read().snapshot();
            hooks.into_iter().filter(|h| h.scope().is_none()).collect()
        };
        let ctx_arc = std::sync::Arc::new(context.clone());
        let tools_snapshot = std::sync::Arc::new(self.tools.read().clone());
        let checkpoint_arc = std::sync::Arc::new(self.checkpoint_store.read().as_ref().cloned());
        let hooks_arc = std::sync::Arc::new(scoped_hooks.clone());
        // A6：format-on-save 的 config.json 路径快照（每次 dispatch 新鲜读，
        // 同 C3 current_diagnostics_loop 模式——运行中可翻转开关）。
        let format_cfg_path = self.config_path.read().clone();
        // D3：本 turn 声明式文件变更的收集桶（Arc 捕获进 'static Fn 闭包）。
        let turn_fc = Arc::clone(&self.turn_file_changes);
        type NextExec = std::sync::Arc<
            dyn Fn(
                    HookToolCall,
                )
                    -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
                + Send
                + Sync,
        >;
        let mut chain: NextExec = {
            let ctx = ctx_arc.clone();
            let tools = tools_snapshot.clone();
            let cp = checkpoint_arc.clone();
            let hooks = hooks_arc.clone();
            let format_cfg_path = format_cfg_path.clone();
            let turn_fc = turn_fc.clone();
            std::sync::Arc::new(move |call: HookToolCall| {
                // Fn 闭包：捕获的 Arc 每次调用克隆一份（不能 move 出 Fn）。
                let ctx = ctx.clone();
                let tools = tools.clone();
                let cp = cp.clone();
                let hooks = hooks.clone();
                let format_cfg_path = format_cfg_path.clone();
                let turn_fc = turn_fc.clone();
                Box::pin(async move {
                    let tool_opt = tools.get(&call.name).cloned();
                    let tool_was_registered = tool_opt.is_some();
                    // A7：preview_all = checkpoint 预检多点版（multiedit
                    // 一次 dispatch 快照全部待改文件；单文件工具恰好一条，
                    // 与旧 preview 行为逐字节等价）。
                    // D3：预检清单同时喂消息级收集桶（session_key 分桶）——
                    // 收集独立于 checkpoint 是否挂载（消息↔文件变更映射不
                    // 依赖安全网开关）；快照仍只在挂载时发生。
                    let previewed = tool_opt
                        .as_ref()
                        .map(|tool| tool.preview_all(&call.arguments))
                        .unwrap_or_default();
                    for change in previewed {
                        turn_fc
                            .lock()
                            .entry(ctx.session_key.clone())
                            .or_default()
                            .push(change.clone());
                        if let Some(cp) = cp.as_ref() {
                            cp.snapshot(&change).await;
                        }
                    }
                    match tool_opt {
                        Some(tool) => match tool.execute(&call.arguments, &ctx).await {
                            Ok(result) => {
                                debug!(
                                    "[AgentLoop] Tool {} returned: {} bytes",
                                    call.name,
                                    result.len()
                                );
                                // A6（devtool-upgrade 阶段 5）format-on-save：
                                // write_file / edit_file / multiedit（A7 批量
                                // 版，逐文件去重依次格式化）成功后、PostToolUse
                                // hooks **之前**跑——用户自定义 hook 看到/
                                // 拿到的是格式化后的文件（计划明文的正交语
                                // 义）；外层 C3 诊断带在瀑布之后，诊断同样作
                                // 用于格式化后文件。失败/超时/开关关/无匹配
                                // 格式化器全部静默——永不拖垮工具调用。
                                let format_paths: Vec<String> =
                                    if !crate::turn_guard::tool_result_indicates_error(&result)
                                        && let Ok(args_val) =
                                            serde_json::from_str::<serde_json::Value>(
                                                &call.arguments,
                                            )
                                    {
                                        match call.name.as_str() {
                                            "write_file" | "edit_file" => args_val
                                                .get("path")
                                                .and_then(|v| v.as_str())
                                                .map(|p| vec![p.to_string()])
                                                .unwrap_or_default(),
                                            "multiedit" => {
                                                // 去重保序：同文件多条编辑只
                                                // 格式化一次（重复跑也是 no-op，
                                                // 省子进程）。
                                                let mut seen = std::collections::HashSet::new();
                                                args_val
                                                    .get("edits")
                                                    .and_then(|v| v.as_array())
                                                    .map(|arr| {
                                                        arr.iter()
                                                            .filter_map(|e| {
                                                                e.get("path")
                                                                    .and_then(|p| p.as_str())
                                                            })
                                                            .filter(|p| {
                                                                seen.insert((*p).to_string())
                                                            })
                                                            .map(|p| p.to_string())
                                                            .collect()
                                                    })
                                                    .unwrap_or_default()
                                            }
                                            _ => Vec::new(),
                                        }
                                    } else {
                                        Vec::new()
                                    };
                                let mut result = result;
                                for path_str in format_paths {
                                    result = crate::formatter::format_on_save(
                                        format_cfg_path.clone(),
                                        &path_str,
                                        &result,
                                    )
                                    .await;
                                }
                                if tool_was_registered {
                                    crate::hooks::run_post_hooks(&hooks, &call, result).await
                                } else {
                                    result
                                }
                            }
                            Err(err) => {
                                warn!("[AgentLoop] Tool {} error: {}", call.name, err);
                                if tool_was_registered {
                                    crate::hooks::run_post_failure_hooks(&hooks, &call, &err).await
                                } else {
                                    format!("Tool error: {err}")
                                }
                            }
                        },
                        None => {
                            warn!("[AgentLoop] Unknown tool: {}", call.name);
                            format!("Error: Unknown tool '{}'", call.name)
                        }
                    }
                })
            })
        };
        for hook in scoped_hooks.iter().rev() {
            let hook = std::sync::Arc::clone(hook);
            let next = std::sync::Arc::clone(&chain);
            chain = std::sync::Arc::new(move |call: HookToolCall| {
                // async move 捕获 owned Arc → future 'static（摆脱 &self 借用）。
                let hook = std::sync::Arc::clone(&hook);
                let next = std::sync::Arc::clone(&next);
                Box::pin(async move { hook.around_tool_use(call, next).await })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
            }) as NextExec;
        }
        let hook_call_owned = hook_call.clone();
        let result = chain(hook_call_owned).await;

        // Record experience for Forge self-learning (non-blocking).
        #[cfg(feature = "forge")]
        {
            if let Some(ref forge) = self.forge {
                // Gate on the runtime master switch — without this, experiences
                // are recorded on every tool call even when forge.enabled=false.
                if forge.is_enabled() {
                    // Truncate payloads: despite the "summary" field name the
                    // previous code stored the FULL args/result (read_file/exec
                    // could be megabytes). 500 chars is enough for reflection
                    // stats. Note: the dedup hash below still uses the full
                    // parsed `args`, so hashing is unaffected.
                    let trunc = |s: &str| -> String { s.chars().take(500).collect() };
                    let exp = nemesis_types::forge::Experience {
                        id: uuid::Uuid::new_v4().to_string(),
                        tool_name: tool_call.name.clone(),
                        input_summary: trunc(&tool_call.arguments),
                        output_summary: trunc(&result),
                        success: !result.contains("SECURITY BLOCKED")
                            && !result.contains("Tool error:"),
                        duration_ms: tool_start.elapsed().as_millis() as u64,
                        timestamp: chrono::Local::now().to_rfc3339(),
                        session_key: format!("{}:{}", context.channel, context.chat_id),
                    };
                    let args = serde_json::from_str(&tool_call.arguments)
                        .unwrap_or(serde_json::Value::Null);
                    let _ = forge.collector().record_with_args(exp, &args).await;
                }
            }
        }

        result
    }

    /// Phase 2: check a tool call's arguments against the registered tool's
    /// schema. Returns Valid / Fixed / Invalid. Unknown tools return Valid so
    /// the existing unknown-tool path in `handle_tool_call` reports them
    /// (class C, not a schema failure).
    fn check_tool_args(&self, tool_call: &ToolCallInfo) -> crate::args_validator::Outcome {
        let schema_opt = self
            .tools
            .read()
            .get(&tool_call.name)
            .map(|t| t.parameters());
        match schema_opt {
            Some(schema) => crate::args_validator::check(&schema, &tool_call.arguments),
            None => crate::args_validator::Outcome::Valid,
        }
    }

    /// Phase 2: per-request consecutive-validation-failure budget, tier-aware.
    /// Mini models get 3, Normal/Big 2（P2C：Big 1→2，并行工具批首个坏调用
    /// 即烧光 budget=1，模型没机会自纠；见 capability.rs 注释）.
    fn validation_retry_budget(&self) -> u32 {
        (*self.tier.read()).validation_retry_budget()
    }

    /// Phase 4a: capability tier currently in effect (small-model-tool-robustness).
    pub fn tier(&self) -> nemesis_types::capability::ModelTier {
        *self.tier.read()
    }

    /// Phase 4a: override the capability tier (e.g. after resolving it from the
    /// active model config at construction, or after `model set-tier`).
    pub fn set_tier(&self, tier: nemesis_types::capability::ModelTier) {
        info!("[AgentLoop] Capability tier set: {}", tier);
        *self.tier.write() = tier;
    }

    /// F1（devtool-upgrade 阶段 4）：当前工作模式。
    pub fn mode(&self) -> crate::types::AgentMode {
        *self.mode.read()
    }

    /// F1：注入 M1a 事件广播发送端（`SharedResources.agent_event_tx`）。
    /// 模式切换发布 `ModeChanged` 用；`None` = 发布静默跳过。
    pub fn set_agent_event_tx(
        &self,
        tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
    ) {
        *self.agent_event_tx.write() = tx;
    }

    /// F1：翻转工作模式并广播 `ModeChanged`（session_key/chat_id 供 web
    /// pump 路由到会话徽标；传空串 = 只进 SSE）。重复设置同一模式仍是
    /// 幂等无害（徽标确认刷新），事件照发——调用方（/plan /build 臂）已经
    /// 在语义上表达「我要切到 X」而非「X 变了」。
    pub fn set_mode_with_event(
        &self,
        mode: crate::types::AgentMode,
        session_key: &str,
        chat_id: &str,
    ) {
        *self.mode.write() = mode;
        info!("[AgentLoop] Agent mode set: {}", mode.as_str());
        let event = nemesis_types::agent::AgentEvent::ModeChanged {
            session_key: session_key.to_string(),
            chat_id: chat_id.to_string(),
            mode: mode.as_str().to_string(),
        };
        if let Some(tx) = self.agent_event_tx.read().as_ref() {
            // 无订阅者（CLI / 无人在线）= 观察者通道空转，静默忽略。
            let _ = tx.send(event);
        }
    }

    /// F1：Plan 模式只读供给白名单。命中者才进 tool_defs（MCP 前缀工具与
    /// spawn 除外——MCP 语义未知不猜，采取「plan 只 deny 已知写」
    /// 立场；spawn 保留供给，分发闸在子代理深度同样生效=纵深）。
    /// 曾前瞻列入的 question（F7）/multiedit（A7）均已落码（2026-09-06），
    /// 本表沿用语义不变。
    const PLAN_MODE_TOOLS: &'static [&'static str] = &[
        "read_file",
        "list_dir",
        "grep",
        "git",
        "web_fetch",
        "lsp",
        "mcp_list",
        "memory_search",
        "memory_list",
        "skills_list",
        "skills_info",
        "find_skills",
        "cli_reference",
        "cron",
        "sleep",
        "message",
        "history_search",
        "todowrite",
        "question",
    ];

    /// F1：分发端写类集合（Plan 模式拦截对象）。**有意偏离计划字面**
    /// （MOVE_TOOLS 全量并集会把 read_file/grep/list_dir/git 四个只读工具
    /// 也拦掉——与计划自己的供给白名单矛盾）：取「可变更外部状态」语义集
    /// = MOVE_TOOLS 去掉四个只读 + multiedit（A7 已落码，2026-09-06）。
    /// exec/run_script 可落盘必拦；git 的写子命令由安全 8 层管线与 D1 自
    /// 管，模式层不重复。
    const PLAN_MODE_WRITE_TOOLS: &'static [&'static str] = &[
        "exec",
        "run_script",
        // C8（2026-09-06）：cargo/npm/go 构建落 target//node_modules/——
        // 「可变更外部状态」同语义，plan 模式一并拦。
        "run_checks",
        "write_file",
        "edit_file",
        "append_file",
        "delete_file",
        "create_dir",
        "delete_dir",
        "multiedit",
    ];

    /// C7：lsp 工具的 `rename` op 是写类（跨文件改内容）——plan 模式必须
    /// 拦。lsp 不在 [`Self::PLAN_MODE_WRITE_TOOLS`] 里（其余 op 只读，整表
    /// 拦截会误伤），故按 args 探测 op 字段特判。解析失败从严当写拦
    /// （plan 模式拒绝成本不对称：误拒可让用户切 /build，误放行改了文件）。
    fn is_lsp_write_call(tool_name: &str, arguments: &str) -> bool {
        if tool_name != "lsp" {
            return false;
        }
        serde_json::from_str::<serde_json::Value>(arguments)
            .ok()
            .and_then(|v| v.get("op").and_then(|o| o.as_str()).map(String::from))
            .map(|op| op == "rename")
            .unwrap_or(true)
    }

    /// F1：Plan 模式分发端唯一写放行——`write_file` 且目标路径落在
    /// `<workspace>/plans/` 前缀内。路径解析与 [`crate::loop_tools::validate_workspace_path`]
    /// 同构（相对路径 join 工作区根 + 双侧 `canonicalize_for_compare` 防
    /// 8.3 短名/大小写失配；目标不存在按最长存在祖先解析）。workspace_root
    /// 未注入（standalone）→ 无放行锚点，一律拦截（诚实从严）。
    fn plan_mode_write_allowed(&self, tool_name: &str, arguments: &str) -> bool {
        if tool_name != "write_file" {
            return false;
        }
        let Some(root) = self.workspace_root.read().clone() else {
            return false;
        };
        let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments) else {
            return false;
        };
        let Some(raw_path) = args.get("path").and_then(|v| v.as_str()) else {
            return false;
        };
        let target = std::path::Path::new(raw_path);
        let candidate = if target.is_absolute() {
            target.to_path_buf()
        } else {
            root.join(target)
        };
        let plans_dir = root.join("plans");
        let resolved = nemesis_path::paths::canonicalize_for_compare(&candidate);
        let plans_canon = nemesis_path::paths::canonicalize_for_compare(&plans_dir);
        resolved.starts_with(&plans_canon)
    }

    /// Phase 4a: set the config.json path. After this, the tier is re-resolved
    /// live from config.json on every model switch and whenever the file's mtime
    /// changes (dashboard model add, CLI `model set-tier`). config.json is the
    /// single source of truth — there is no stale per-model snapshot to keep in
    /// sync. Called by `agent_factory` at gateway startup.
    pub fn set_config_path(&self, path: std::path::PathBuf) {
        *self.config_path.write() = Some(path);
    }

    /// N1 (devtool-upgrade 阶段 1): inject the shared layered pricing store
    /// (workspace/data). L2 of the three-tier `context_window` resolution —
    /// when config.json has no explicit `context_window` for the active
    /// model, `PricingStore::lookup` (custom > downloaded > embedded, with
    /// bare-suffix matching) supplies `max_input_tokens`. `None` (not
    /// injected / open failed) skips straight to L3 fallback.
    pub fn set_pricing_store(&self, store: std::sync::Arc<nemesis_data::PricingStore>) {
        *self.pricing_store.write() = Some(store);
    }

    /// C3（devtool-upgrade 阶段 2）：注入共享 LspManager 单例（与 LspTool
    /// 同一实例——SharedResources.lsp_manager，见 C5）。编辑后诊断回灌
    /// （修复闭环）消费；`None`（未注入 / standalone）→ 反馈静默跳过。
    pub fn set_lsp_manager(&self, mgr: Arc<nemesis_lsp::LspManager>) {
        *self.lsp_manager.write() = Some(mgr);
    }

    /// C3：读 `agents.defaults.diagnostics_loop`（config.json 每次新鲜读，
    /// 同 [`Self::current_tool_doc_folding`] 模式——dashboard/CLI 可在网关
    /// 运行中翻转开关）。缺段 / standalone → 全默认（enabled=false）。
    pub(crate) fn current_diagnostics_loop(&self) -> nemesis_config::DiagnosticsLoopConfig {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return Default::default(),
        };
        let v = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let Some(v) = v else {
            return Default::default();
        };
        v.get("agents")
            .and_then(|a| a.get("defaults"))
            .and_then(|d| d.get("diagnostics_loop"))
            .and_then(|s| {
                serde_json::from_value::<nemesis_config::DiagnosticsLoopConfig>(s.clone()).ok()
            })
            .unwrap_or_default()
    }

    /// C3：编辑后诊断回灌核心（修复闭环：「落盘 → 触发诊断 → 等 ERROR →
    /// please fix」）。调用点在工具结果
    /// 进 spill/gate 管线**之前**——反馈与工具结果同走一条模型可见管线。
    ///
    /// 全部失败/未命中路径**静默原样返回**（开关关 / 非 write|edit / 无
    /// manager / 语言无服务器 / 同步失败 / 无 ERROR）——诊断永不拖垮工具
    /// 调用。ERROR 级取 ≤`max_errors` 条追加：
    /// `"\n\n[LSP] {n} error(s) detected in {path}, please fix:"` + 每条
    /// `"- L{line}:{col} {message} ({source})"`（1-based 显示，LSP 0-based
    /// 内部转换）。
    ///
    /// 文档同步用 [`nemesis_lsp::LspManager::touch_file`]（读盘下发）而非
    /// `notify_change`：edit_file 的最终内容无法从 args 重建，磁盘是唯一
    /// 真相源；write_file 读盘等价（execute 返回即写完）。
    pub(crate) async fn apply_diagnostics_feedback(
        mgr: Option<&nemesis_lsp::LspManager>,
        cfg: nemesis_config::DiagnosticsLoopConfig,
        tool_name: &str,
        path: &str,
        result: &str,
    ) -> String {
        if !cfg.enabled || !matches!(tool_name, "write_file" | "edit_file") {
            return result.to_string();
        }
        let Some(mgr) = mgr else {
            return result.to_string();
        };
        let p = std::path::Path::new(path);
        // 未注册语言 / 该语言无已安装服务器 → 原样（探测是纯 PATH 查找，
        // 不 spawn 进程）。
        let Some(lang) = nemesis_lsp::registry::lang_for_path(p) else {
            return result.to_string();
        };
        if !nemesis_lsp::registry::server_available(lang) {
            return result.to_string();
        }
        // 服务器同步失败 → 原样（best-effort）。
        if mgr.touch_file(p).await.is_err() {
            return result.to_string();
        }
        let diags = mgr.wait_for_diagnostics(p, 150, cfg.wait_max_ms).await;
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == 1)
            .take(cfg.max_errors)
            .collect();
        if errors.is_empty() {
            return result.to_string();
        }
        let mut out = String::with_capacity(result.len() + 96 * errors.len());
        out.push_str(result);
        out.push_str(&format!(
            "\n\n[LSP] {} error(s) detected in {}, please fix:",
            errors.len(),
            path
        ));
        for d in errors {
            out.push_str(&format!(
                "\n- L{}:{} {} ({})",
                d.range_start.0 + 1,
                d.range_start.1 + 1,
                d.message,
                d.source.as_deref().unwrap_or("lsp")
            ));
        }
        out
    }

    /// C3：dispatch 现场包装——读共享 manager + 新鲜 config 后委托核心。
    pub(crate) async fn diagnostics_feedback(
        &self,
        tool_name: &str,
        path: &str,
        result: &str,
    ) -> String {
        let mgr = self.lsp_manager.read().clone();
        Self::apply_diagnostics_feedback(
            mgr.as_deref(),
            self.current_diagnostics_loop(),
            tool_name,
            path,
            result,
        )
        .await
    }

    /// 内置 slash 命令名（与 [`Self::handle_command_with_context`] 的 match 臂
    /// **同步维护**）：自定义命令表命中这些名字时跳过改写——内置优先。
    /// `compact`/`clear` 是 E6 会话维护命令（gate 内 `parse_maintenance_command`
    /// 拦截，有副作用，同样不许被自定义命令表遮蔽）；`plan`/`build` 是 F1
    /// 模式切换（gate 内独立臂，要先拿 chat_id 发布 ModeChanged）。
    /// K3：清单本体收敛到 `nemesis_types::constants::BUILTIN_SLASH_COMMANDS`
    /// （dashboard `commands.list` 的 `builtins` 下发同源），这里只做转发。
    const BUILTIN_SLASH_COMMANDS: &'static [&'static str] =
        nemesis_types::constants::BUILTIN_SLASH_COMMANDS;

    /// 自定义 slash 命令表路径（主 agent 专用；集群 agent 不接——命令不该
    /// 跨节点复制，同 hooks 挂账决策）。设置时立即加载一次；mtime 变化在
    /// `rewrite_custom_command` 的 check() 里自动重载（HotReloader 统一收编，
    /// dashboard/CLI 改命令表后无需重启）。
    pub fn set_commands_path(&self, path: std::path::PathBuf) {
        *self.commands_hot.write() = Some(nemesis_config::HotReloader::new(
            path,
            nemesis_config::load_commands_config,
        ));
    }

    /// hooks 方言桥注入（PreCompact/PostCompact 触发用；工具/生命周期钩子走
    /// 各自注册表，与此并存）。
    pub fn set_cc_hooks_bridge(&self, bridge: std::sync::Arc<crate::cc_hooks::CcHookBridge>) {
        *self.cc_bridge.write() = Some(bridge);
    }

    /// 桥的只读访问（Dashboard 删除会话时触发方言 SessionEnd 用）。
    pub fn cc_hooks_bridge(&self) -> Option<std::sync::Arc<crate::cc_hooks::CcHookBridge>> {
        self.cc_bridge.read().clone()
    }

    /// 触发 SessionEnd 钩子（会话 TTL 过期清理/显式删除时由装配点调用）。
    /// 直接走方言桥（唯一实现者；无桥 = no-op）。
    pub async fn run_session_end_hooks(&self, session_key: &str, reason: &str) {
        // 复用 cc_hooks_bridge()（锁内 clone Arc 出来），读 guard 不得跨 await——
        // 否则 on_session_end 整个回调期间 cc_bridge 写锁全部阻塞。
        if let Some(bridge) = self.cc_hooks_bridge() {
            bridge.on_session_end(session_key, reason).await;
        }
    }

    /// 自定义 slash 命令改写（改写型，区别于内置命令的短路型）。K3 补齐后
    /// 三段解析链（每段只在上一段未命中时生效）：
    ///
    /// 1. **自定义命令**：`/name args` → 命令表模板中的 `$ARGUMENTS` 替换为
    ///    `args`（模板无占位符且带参数 → 追加为独立段）。
    /// 2. **`` !`cmd` `` 注入**：对展开后的模板文本 regex 扫 `` !`cmd` `` →
    ///    同步执行（复用 C8 exec 内核 `run_one_stage`，3s 超时，workspace 为
    ///    cwd）→ stdout（≤4KB，字符边界截断）替换进模板；失败注记
    ///    `[command failed: ...]`。相同命令一段模板内只执行一次（结果复用）。
    ///    信任边界（诚实声明）：这是用户自己配置的模板/参数 = 用户本机终端
    ///    同级信任，**不**过 9 层安全管线（管线管的是 LLM 工具调用）；且只在
    ///    命令模板路径生效——普通消息不做注入（不放大攻击面）。
    /// 3. **技能回落**：`/name` 未命中命令表时查已装 skills（workspace →
    ///    global → builtin），命中则改写为 `Use the {name} skill to handle:
    ///    {args}`（无参数则 `Use the {name} skill.`）——技能斜杠化；
    ///    内置名/自定义命令优先级恒高于技能名。
    ///
    /// 未命中/内置名/非 slash 一律不动。每消息做一次 mtime 检查（一次 stat，
    /// 可忽略）。async 的原因只有 `` !`cmd` `` 执行；调用点 `process_inbound_
    /// message` 本就是 async，无锁跨 await（命令表读锁在 exec 前释放）。
    pub async fn rewrite_custom_command(&self, msg: &mut nemesis_types::channel::InboundMessage) {
        let content = msg.content.trim();
        if !content.starts_with('/') {
            return;
        }
        let rest = &content[1..]; // '/' 为 1 字节 ASCII，字节切片安全
        // 拷贝为 owned：name/args 借用自 msg.content，而技能回落要可变借
        // msg（E0502）——slash 消息本就低频，两次小分配无所谓。
        let (name, args) = match rest.split_once(char::is_whitespace) {
            Some((n, a)) => (n.to_string(), a.trim().to_string()),
            None => (rest.to_string(), String::new()),
        };
        if name.is_empty() || Self::BUILTIN_SLASH_COMMANDS.contains(&name.as_str()) {
            return;
        }
        // 命令表读锁 scope 严格限制在查表——技能回落走 fs 扫描，不得持锁
        // （return 走出本块时 guard 自动释放）。
        let expanded = {
            let hot_guard = self.commands_hot.read();
            let Some(hot) = hot_guard.as_ref() else {
                return self.rewrite_skill_fallback(&name, &args, msg);
            };
            hot.check();
            let commands = hot.get();
            let Some(cmd) = commands.commands.iter().find(|c| c.name == name) else {
                return self.rewrite_skill_fallback(&name, &args, msg);
            };
            // $ARGUMENTS 占位替换；模板无占位符且带参数 → 追加为独立段（对
            // 用户更友好：模板忘写占位符时参数不至于被吞）。
            if cmd.prompt.contains("$ARGUMENTS") {
                cmd.prompt.replace("$ARGUMENTS", &args)
            } else if !args.is_empty() {
                format!("{}\n\n{}", cmd.prompt, args)
            } else {
                cmd.prompt.clone()
            }
        };
        // K3：`` !`cmd` `` 注入（展开后的模板文本；模板与参数都可能携带）。
        let expanded = self.expand_shell_injections(expanded).await;
        info!(
            "[AgentLoop] custom command /{} expanded (prompt {} chars)",
            name,
            expanded.len()
        );
        msg.content = expanded;
    }

    /// K3：技能回落（只在命令表未命中时被 [`Self::rewrite_custom_command`]
    /// 调用）。命中已装技能 → 改写为技能驱动提示词；否则不动。
    fn rewrite_skill_fallback(
        &self,
        name: &str,
        args: &str,
        msg: &mut nemesis_types::channel::InboundMessage,
    ) {
        let Some(loader) = self.skills_loader.read().clone() else {
            return;
        };
        let known = loader.list_skills().iter().any(|s| s.name == name);
        if !known {
            return;
        }
        msg.content = if args.is_empty() {
            format!("Use the {name} skill.")
        } else {
            format!("Use the {name} skill to handle: {args}")
        };
        info!("[AgentLoop] /{name} resolved to installed skill");
    }

    /// K3：对文本中的 `` !`cmd` `` 逐个执行并替换（相同命令只执行一次）。
    /// 无注入时零开销直返；cwd 取 workspace_root（未设则继承进程 cwd）。
    async fn expand_shell_injections(&self, text: String) -> String {
        let cmds = extract_shell_injections(&text);
        if cmds.is_empty() {
            return text;
        }
        let cwd = self.workspace_root.read().clone();
        let mut results: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for c in &cmds {
            let replaced = exec_shell_injection(c, cwd.as_deref()).await;
            results.insert(c.clone(), replaced);
        }
        substitute_shell_injections(&text, &|c| results.get(c).cloned().unwrap_or_default())
    }

    /// G4 (U4): enable tool-result spill with the given root directory
    /// (expected `<home>/logs/spill`). Results above the spill threshold are
    /// written whole under this root and the conversation keeps a bounded
    /// preview + locator. Without a call, spilling stays disabled and results
    /// use only the G3 prune tier.
    pub fn set_spill_root(&self, root: std::path::PathBuf) {
        *self.spill_root.write() = Some(root);
    }

    /// D2 (2026-08-24 arch review): read back the configured spill root, if
    /// any. Diagnostics/test seam for verifying factory wiring — both the
    /// main and cluster factories point at `<home>/logs/spill`.
    pub fn spill_root_path(&self) -> Option<std::path::PathBuf> {
        self.spill_root.read().clone()
    }

    /// H3 (P2.2): enable skills-catalog digest injection by providing the
    /// loader. The digest is injected (same point as the time/env hint) only
    /// when the catalog changed since the last injection for this session.
    pub fn set_skills_loader(&self, loader: Arc<nemesis_skills::loader::SkillsLoader>) {
        *self.skills_loader.write() = Some(loader);
    }

    /// H5 (U18): enable the workspace instruction-chain section by giving
    /// the workspace root (chain root = this dir; the chain itself is read
    /// per-injection).
    pub fn set_workspace_root(&self, root: std::path::PathBuf) {
        *self.workspace_root.write() = Some(root);
    }

    /// I1 (devtool-upgrade 阶段 3): start the workspace fs watcher. Call
    /// AFTER the `Arc<AgentLoop>` is finalized — the watcher handle is
    /// stored INSIDE the loop, so its callbacks hold a `Weak<AgentLoop>`
    /// (an Arc would cycle and leak). Requires `set_workspace_root` first;
    /// startup failure warns once and leaves the watcher disabled (never
    /// retries, never blocks the loop).
    pub fn start_fs_watcher(
        loop_arc: &std::sync::Arc<Self>,
        cfg: &nemesis_config::FsWatcherConfig,
    ) -> Result<(), String> {
        let root = match loop_arc.workspace_root.read().clone() {
            Some(r) => r,
            None => {
                tracing::info!("[AgentLoop] fs watcher skipped: no workspace root set");
                return Ok(());
            }
        };
        let weak = std::sync::Arc::downgrade(loop_arc);
        let on_instruction_change: crate::fs_watcher::Callback = {
            let weak = weak.clone();
            std::sync::Arc::new(move || {
                if let Some(l) = weak.upgrade() {
                    // Round-5 note: digest state is stateless (sections
                    // re-read from disk every build) so external instruction
                    // edits already surface next build — this call is the
                    // kept anchor (same shape as the dispatch path's
                    // touch-driven H5 call) in case change-gating returns.
                    l.invalidate_context_digests();
                }
            })
        };
        let on_external_change: crate::fs_watcher::PathCallback = {
            let weak = weak.clone();
            std::sync::Arc::new(move |p| {
                if let Some(l) = weak.upgrade() {
                    l.push_external_change(p);
                }
            })
        };
        match crate::fs_watcher::start(&root, cfg, on_instruction_change, on_external_change) {
            Ok(Some(handle)) => {
                tracing::info!(
                    root = %root.display(),
                    "[AgentLoop] fs watcher started (external changes surface next turn)"
                );
                *loop_arc.fs_watcher.write() = Some(handle);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(e) => {
                tracing::warn!(
                    "[AgentLoop] fs watcher failed to start (disabled, will not retry): {}",
                    e
                );
                Err(e)
            }
        }
    }

    /// I1: buffer an externally-changed workspace file (workspace-relative).
    /// Drops self-inflicted writes (the agent's own write_file/edit_file
    /// within [`crate::fs_watcher::SELF_WRITE_WINDOW`]) and caps the buffer.
    pub fn push_external_change(&self, rel_path: &str) {
        let norm = crate::fs_watcher::normalize_workspace_rel(rel_path);
        {
            let mut writes = self.recent_self_writes.lock();
            writes.retain(|_, t| t.elapsed() < crate::fs_watcher::SELF_WRITE_WINDOW);
            if writes.contains_key(&norm) {
                tracing::debug!(
                    "[AgentLoop] fs watcher event for self-written file dropped: {}",
                    rel_path
                );
                return;
            }
        }
        let mut buf = self.external_changes.lock();
        if buf.len() < crate::fs_watcher::MAX_BUFFERED_CHANGES {
            buf.push(rel_path.to_string());
        }
    }

    /// I1: one-shot drain for the build_messages `<external_changes>` section.
    pub fn drain_external_changes(&self) -> Vec<String> {
        std::mem::take(&mut *self.external_changes.lock())
    }

    /// I1: record an agent-authored write (dispatch path, successful
    /// write_file/edit_file) so the watcher's own event for the same path is
    /// dropped inside the self-write window.
    pub fn note_self_write(&self, path: &str) {
        let mut writes = self.recent_self_writes.lock();
        // Prune expired entries opportunistically (map stays tiny).
        writes.retain(|_, t| t.elapsed() < crate::fs_watcher::SELF_WRITE_WINDOW);
        writes.insert(
            crate::fs_watcher::normalize_workspace_rel(path),
            std::time::Instant::now(),
        );
    }

    /// I3 (devtool-upgrade 阶段 3): lazy sub-directory instruction discovery.
    /// Called on a SUCCESSFUL read_file whose path resolves inside the
    /// workspace: if the file's directory (≠ workspace root — the root chain
    /// is always injected) carries AGENTS.md/CLAUDE.md and was never claimed
    /// this session, queue its contents for the next build's one-shot
    /// injection (same channel shape as the I1 external_changes section).
    ///
    /// Directories without instruction files stay UNCLAIMED so instructions
    /// added later (agent- or user-authored) are still discovered on a later
    /// read. Relative paths resolve against the workspace root (read_file
    /// accepts them); outside-root paths are silently ignored. Loop-level
    /// state is not needed — the instance owns claims+buffer (per-session
    /// dedup, drained at build).
    fn note_read_for_instructions(&self, instance: &AgentInstance, path: &str) {
        use nemesis_path::paths::canonicalize_for_compare;
        let root = match self.workspace_root.read().clone() {
            Some(r) => r,
            None => return,
        };
        let p = std::path::Path::new(path);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        };
        let Some(parent) = abs.parent() else {
            return;
        };
        let parent_canon = canonicalize_for_compare(parent);
        let root_canon = canonicalize_for_compare(&root);
        // Outside the workspace, or the root itself → not a lazy subdir.
        if !parent_canon.starts_with(&root_canon) || parent_canon == root_canon {
            return;
        }
        if instance.instruction_dir_claimed(&parent_canon) {
            return;
        }
        let files = crate::workspace_instructions::load_dir_instruction_files(parent);
        if files.is_empty() {
            return;
        }
        instance.claim_instruction_dir(parent_canon);
        instance.queue_pending_instructions(files);
        info!(
            "[AgentLoop] I3 lazy instructions: sub-directory {} queued for next build",
            parent.display()
        );
    }

    /// Full-review M4: set the context-snapshot message role ("user" |
    /// "system"). Anything unrecognized stays/becomes "user" (default).
    pub fn set_snapshot_role(&self, role: &str) {
        let r = if role.eq_ignore_ascii_case("system") {
            "system"
        } else {
            "user"
        };
        *self.snapshot_role.write() = r.to_string();
    }

    /// X2 (U8 refinement): tell the loop whether interactive approval
    /// (desktop popup) is wired. Called by the gateway after it attaches
    /// the approval adapter; renders into the `# Runtime Policy` snapshot
    /// section. Default `false` (standalone / non-desktop builds).
    pub fn set_interactive_approval(&self, enabled: bool) {
        *self.interactive_approval.write() = enabled;
    }

    /// H5 (U18): touch-driven digest invalidation. Called by the dispatch
    /// path when a read_file/write_file/edit_file call touched a file that
    /// is on the session's instruction chain — the next build_messages
    /// re-reads the chain and re-injects. `session_key` is the stable key
    /// build_messages derived (first-user-content hash); the dispatch path
    /// does not know it, so this drops the WHOLE digest state entry list is
    /// not viable — instead we clear all session keys whose chain contains
    /// the touched path. Simplest correct form given the keying: clear ALL
    /// entries (re-inject once for every live session on the next build) —
    /// file touches on the chain are rare, so over-invalidation is cheap.
    pub fn invalidate_context_digests(&self) {
        self.skills_digest_state.clear_all();
    }

    /// Phase 4a: re-resolve the capability tier against the currently active
    /// model by reading config.json live. Per-model `model_tier`/`real_name`/
    /// `model_size_b` are honoured; a missing/unreadable config falls back to
    /// the name heuristic. Called after every model switch and on config change.
    // 热重载收编登记（2026-08-29）：此处非纯数据加载（读 config 后写多个
    // AgentLoop 状态），HotReloader<T> 的 fn(&Path)->T 形状装不下——保留独立
    // 实现并登记（计划原文"诚实优于强行统一"）。收编需 trait 形状的 reloader。
    fn refresh_active_tier(&self) {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return, // standalone mode — keep the startup tier
        };
        let active = self.active_model.read().clone();
        let tier = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| nemesis_types::capability::resolve_active_tier(&v, &active))
            .unwrap_or_else(|| {
                nemesis_types::capability::detect_tier(&nemesis_types::capability::TierHint {
                    full_model: Some(active.clone()),
                    real_name: None,
                    size_b: None,
                })
            });
        if *self.tier.read() != tier {
            info!(
                "[AgentLoop] Active model '{}' → capability tier {} (re-resolved from config.json)",
                active, tier
            );
            *self.tier.write() = tier;
        }
    }

    /// Resolve the active model's display id (`provider/name`, e.g.
    /// `deepseek/deepseek-v4-flash`) for the per-message "供应商·模型名"
    /// badge. Reads config.json fresh each call (called once per assistant
    /// turn, negligible cost) — no cached field, so it can never go stale when
    /// the model switches. Falls back to the bare `active_model` when config
    /// is unavailable (standalone mode) or the entry isn't found.
    pub(crate) fn current_display_model(&self) -> String {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return active,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| nemesis_types::capability::resolve_display_model(&v, &active))
            .unwrap_or(active)
    }

    /// Resolve the active model's per-model output token cap (`max_output_tokens`)
    /// from config.json, used as the chat request's `max_tokens`. Reads config
    /// fresh each call (like `current_display_model`). `None` when config is
    /// unavailable (standalone mode) or the field is absent — caller falls back
    /// to the 8192 default. Lets each model declare its real output ceiling so
    /// large files write in one shot instead of truncating at a blanket cap.
    /// H4 (U16 half): the active model's reasoning-effort tier from
    /// config.json (`model set-effort`). None when unset/"off"/standalone.
    pub(crate) fn current_reasoning_effort(&self) -> Option<String> {
        let active = self.active_model.read().clone();
        let path = self.config_path.read().clone()?;
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| nemesis_types::capability::resolve_reasoning_effort(&v, &active))
    }

    pub(crate) fn current_max_tokens(&self) -> Option<u32> {
        let active = self.active_model.read().clone();
        let path = self.config_path.read().clone()?;
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| nemesis_types::capability::resolve_max_output_tokens(&v, &active))
            .map(|n| n as u32)
        // ↑ i64 (from JSON) → u32; max_output_tokens is a non-negative count
    }

    /// U16 (sixth batch) + N1 (devtool-upgrade 阶段 1)：active 模型的
    /// context_window（input token capacity）**三级解析链**：
    /// L1 config.json 显式 `context_window`（用户最大）→ L2 价目表
    /// `max_input_tokens`（`PricingStore::lookup`，含 bare-suffix 匹配）→
    /// L3 [`FALLBACK_CONTEXT_WINDOW`]（128k，由调用方
    /// `instance.context_window()` 兜底）。读 config 每次新鲜（同
    /// `current_max_tokens` 模式）。This closes the S1-S7 leftover: the
    /// compaction thresholds in `maybe_summarize` were computed against a
    /// hardcoded 32000 regardless of the model's real window (a
    /// 200K-window model compacted 6× too early) — and N1 replaces that
    /// fallback itself (32k → 128k + catalog lookup).
    pub(crate) fn current_context_window(&self) -> Option<usize> {
        self.current_context_window_with_source().0
    }

    /// [`Self::current_context_window`] 带来源标记（N1 可观测）：
    /// `Some((窗口, "config" | "catalog"))`；L3 未命中 → `(None, "fallback-128k")`
    /// ——调用方用 `instance.context_window()`（= [`FALLBACK_CONTEXT_WINDOW`]）。
    pub(crate) fn current_context_window_with_source(&self) -> (Option<usize>, &'static str) {
        let active = self.active_model.read().clone();
        let cfg = self.config_path.read().clone().and_then(|path| {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        });
        let pricing = self.pricing_store.read().clone();
        resolve_context_window_tiered(cfg.as_ref(), &active, pricing.as_deref())
    }

    /// T10（多模态 goal）：active 模型的 vision 能力（读 config.json 新鲜
    /// 值，同 [`current_max_tokens`] 模式——config.json 是唯一真相源，模型
    /// 中途切换 / 磁盘改动都自然生效）。standalone / 条目缺失 → 默认放行。
    pub(crate) fn current_vision(&self) -> nemesis_types::capability::VisionResolution {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return nemesis_types::capability::vision_default_allow(),
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| nemesis_types::capability::resolve_active_vision(&v, &active))
            .unwrap_or_else(nemesis_types::capability::vision_default_allow)
    }

    /// T4 (U1): per-model summarizer prefix-reuse switch from config.json
    /// (`summarizer_prefix_reuse`, default true — the main model keeps the
    /// G1 prefix-reuse summary shape). `false` → the summary request falls
    /// back to the pre-G1 shape (single bare user message with
    /// `role: content` text concatenation), for cheap summarizer models that
    /// break the assumed warm KV prefix. Reads config fresh each call (same
    /// pattern as [`current_max_tokens`]); standalone (no config_path) →
    /// default true.
    pub(crate) fn current_summarizer_prefix_reuse(&self) -> bool {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return true,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| nemesis_types::capability::resolve_summarizer_prefix_reuse(&v, &active))
            .unwrap_or(true)
    }

    /// Phase 4a: detect config.json on-disk changes (by mtime) and re-resolve
    /// the active model's tier if it changed. Runs once per LLM round, next to
    /// `check_mcp_reload`. Picks up dashboard model additions and CLI
    /// `model set-tier` while the gateway is running.
    pub(crate) fn check_config_reload(&self) {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return,
        };
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        {
            let mut last = self.config_mtime.write();
            if mtime == *last {
                return; // unchanged since last check
            }
            *last = mtime;
        }
        debug!("[AgentLoop] config.json mtime changed; re-resolving capability tier");
        self.refresh_active_tier();
    }

    /// P3.1 (sixth batch): retrieve relevant long-term memories for the
    /// LATEST user message (vector top-K, score-thresholded, deduped by
    /// pairwise cosine > 0.92 — goal §三 semantics). `None` whenever the
    /// feature is off (default), the manager/vector store is absent, or no
    /// hit clears the bar — `None` keeps build_messages byte-identical.
    #[cfg_attr(not(feature = "memory"), allow(unused_variables))]
    async fn prefetch_memory_context(&self, instance: &AgentInstance) -> Option<Vec<String>> {
        #[cfg(feature = "memory")]
        {
            let (auto, top_k) = *self.memory_inject_cfg.read();
            if !auto {
                return None;
            }
            // Latest USER message is the retrieval signal.
            let history = instance.get_history();
            let query = history
                .iter()
                .rev()
                .find(|t| t.role == "user")
                .map(|t| t.content.clone())?;
            if query.trim().is_empty() {
                return None;
            }
            let mgr = self.memory_inject_manager.read().clone()?;
            // AUTO_INJECT_MIN_SCORE (0.35) gates INSIDE search_auto_inject —
            // the plain search() runs the store's 0.7 bar (memory_search
            // tuning), which silently emptied loosely-related injection hits
            // (2026-08-29 根因).
            let result = mgr
                .search_auto_inject(&query, top_k.max(1) + 2)
                .await
                .ok()?;
            let mut scored: Vec<(f64, String)> = result
                .entries
                .into_iter()
                .map(|e| {
                    let content = e.entry.content;
                    let cut = content
                        .char_indices()
                        .nth(300)
                        .map(|(i, _)| i)
                        .unwrap_or(content.len());
                    (e.score, content[..cut].to_string())
                })
                .collect();
            // Sort best-first, cap at top_k.
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            // Pairwise dedup: drop a weaker hit whose cosine with a kept hit
            // exceeds 0.92. Cheap at top_k+2 candidates.
            let mut kept: Vec<(f64, String)> = Vec::new();
            'cand: for cand in scored {
                for k in &kept {
                    if textwise_similar(&cand.1, &k.1) > 0.92 {
                        continue 'cand;
                    }
                }
                kept.push(cand);
                if kept.len() >= top_k.max(1) {
                    break;
                }
            }
            if kept.is_empty() {
                return None;
            }
            Some(kept.into_iter().map(|(_, c)| c).collect())
        }
        #[cfg(not(feature = "memory"))]
        {
            None
        }
    }

    /// Build the LLM message list from the instance conversation history.
    ///
    /// Injects an ephemeral "# Current Time / # Environment" system message
    /// immediately before the latest user message. The historical prefix (system
    /// prompt + earlier turns) stays byte-identical across requests, preserving
    /// prompt-cache hits; only the trailing user message and the dynamic marker
    /// are billed at the cache-miss rate. The platform/shell hint steers the
    /// model away from interactive commands that hang the exec tool (e.g. bare
    /// Windows `date` vs `date /t`) — small-model-tool-robustness plan Phase 1.
    ///
    /// P3.1 (sixth batch): `memory_hits` (caller-side async prefetch — search
    /// is async, this fn is sync) renders a `# Memory Context` section into
    /// the merged snapshot. `None`/empty = no section, byte-identical to
    /// pre-P3.1 output.
    pub fn build_messages(&self, instance: &AgentInstance) -> Vec<LlmMessage> {
        self.build_messages_with_memory(instance, None)
    }

    /// P3.1 companion: same as [`build_messages`] plus an optional
    /// pre-fetched memory-hit section.
    pub fn build_messages_with_memory(
        &self,
        instance: &AgentInstance,
        memory_hits: Option<&[String]>,
    ) -> Vec<LlmMessage> {
        self.build_messages_with_memory_annotated(instance, memory_hits)
            .0
    }

    /// T8 (U9 ②) companion: [`build_messages_with_memory`] plus a
    /// [`crate::replay::BuildAnnotation`] recording everything a later
    /// byte-exact replay needs that is NOT derivable from the final session
    /// file — the digest injection's final-vec position, the folded history
    /// length, and the summary-cache state AS OF this build (the session
    /// file's final summary may have advanced later in the same turn).
    /// Byte-identical output to the unannotated build; the annotation rides
    /// alongside and never feeds the provider.
    pub fn build_messages_with_memory_annotated(
        &self,
        instance: &AgentInstance,
        memory_hits: Option<&[String]>,
    ) -> (Vec<LlmMessage>, crate::replay::BuildAnnotation) {
        let history = instance.get_history();

        // Inline-summary pipeline. When a summary cache is active, its `text`
        // folds the covered prefix `history[..covers_up_to]` into the leading
        // system message, and `history[covers_up_to..]` is sent verbatim. Every
        // message is either summarized (in `text`) or verbatim — no gap, no
        // overlap. With no active cache this degrades to sending the entire
        // history verbatim, byte-identical to pre-refactor behavior.
        let cache = instance.get_summary_cache();
        let active_cache = cache
            .as_ref()
            .filter(|c| !c.text.is_empty() && c.covers_up_to >= 1);

        let turns = project_history_for_request(
            &history,
            active_cache.map(|c| (c.text.as_str(), c.covers_up_to)),
        );

        // T10（多模态）：最新 user 注入点的判定提前——vision 投影与后面的
        // digest 注入共用同一个 index（纯 turns 派生，与注入内容无关）。
        let last_user_idx = turns
            .iter()
            .rposition(|t| t.role == "user")
            .filter(|&i| i > 0)
            .filter(|_| turns.first().is_some_and(|t| t.role == "system"));

        // T10（多模态 D4）：vision=no 时把 image_refs 投影为稳定占位文本
        // （当前轮拒绝注明 / 历史轮已省略）。supported（默认放行含）零改动
        // ——纯文本请求字节不受影响（字节兼容验收 #2）。投影发生在
        // turn → 请求消息转换**之前**，图片字节根本不进请求；占位写入的是
        // 投影视图，历史持久化不动（切回 vision 模型图片原样回来）。投影
        // 标志随投影台账落盘，replay 用同一共享纯函数重放。
        let mut turns = turns;
        let vision_projected = !self.current_vision().supported;
        if vision_projected {
            project_turns_for_no_vision(&mut turns, last_user_idx);
        }

        let mut annotation = crate::replay::BuildAnnotation {
            digest_index: None,
            history_len: turns.len(),
            summary_as_of: active_cache.map(|c| crate::replay::SummaryAsOf {
                covers_up_to: c.covers_up_to,
                text: c.text.clone(),
            }),
            vision_projected,
        };

        // I2 (U8): time/env becomes the FIRST section of the merged context
        // snapshot (was a standalone system-role dyn_msg). Minute granularity:
        // the timestamp truncates to the minute so a burst of calls within
        // the same minute does not churn the digest (runtime-context
        // snapshot discipline: identical content ⇒ no re-injection).
        let now = chrono::Local::now()
            .format("%Y-%m-%d %H:%M (%A)")
            .to_string();
        #[cfg(target_os = "windows")]
        let env_hint = "platform: windows\ndefault_shell: cmd\ntime_cmd: use `date /t` or `echo %date% %time%` or PowerShell `Get-Date`";
        #[cfg(not(target_os = "windows"))]
        let env_hint = "platform: unix\ndefault_shell: sh\ntime_cmd: use `date`";
        let snapshot_section = format!(
            "# Current Time / Environment snapshot\n{}\n# Environment\n{}\n(本快照取代之前的时间/环境快照)",
            now, env_hint
        );

        // T5/T6（多模态）：turn → 请求消息统一走 `turn_to_request_message`
        // （与 replay 重建共用；image_refs 每轮水合重读，失效 → 占位文本）。
        let turn_to_msg = |turn: &crate::types::ConversationTurn| turn_to_request_message(turn);

        // Inject dyn_msg just before the last user message, but only when there
        // is a system prompt at turns[0] to protect (otherwise there's no
        // cached prefix to preserve).
        //
        // H3 (P2.2) + H5 (U18) + I2 (U8): the skills-catalog digest, the
        // workspace instruction chain, AND the time/env snapshot ride ONE
        // injection point as a single MERGED message (sections inside one
        // <system-reminder> wrapper), with the same prefix-protection
        // condition. Re-emitted on EVERY build (not persisted in history) —
        // deterministic rendering keeps it byte-identical while nothing
        // changed, which is what preserves the provider prefix. Sections
        // re-read from disk each build, so file touches are picked up
        // naturally (H5's invalidate call is a structural no-op).
        let context_digest_msg: Option<LlmMessage> = {
            let loader = self.skills_loader.read().clone();
            let ws_root = self.workspace_root.read().clone();
            // Build the merged content: time/env snapshot (I2) + skills
            // section (if any) + workspace instructions section (if any).
            // The snapshot is ALWAYS present (time always renders), so the
            // merged message exists for every session.
            let mut sections: Vec<String> = vec![snapshot_section.clone()];
            if let Some(ref l) = loader {
                let infos = l.list_skills();
                if !infos.is_empty() {
                    let catalog = crate::skills_digest::catalog_from_skills_infos(&infos);
                    let rendered = crate::skills_digest::render_skills_digest(&catalog);
                    sections.push(crate::skills_digest::digest_message(&rendered));
                }
            }
            if let Some(ref root) = ws_root {
                let cwd = root.clone(); // workspace root ≈ conversation cwd
                let chain = crate::workspace_instructions::load_instruction_chain(root, &cwd);
                let rendered = crate::workspace_instructions::render_instructions_section(&chain);
                if !rendered.is_empty() {
                    sections.push(rendered);
                }
            }
            // I3 (devtool-upgrade 阶段 3): lazily discovered sub-directory
            // instructions — one-shot section drained from the instance
            // (same channel shape as the external_changes section below;
            // empty between discoveries keeps the merged message
            // byte-stable). Claims live on the instance, so dedup is
            // session-scoped.
            let pending_instructions = instance.drain_pending_instructions();
            if !pending_instructions.is_empty() {
                sections.push(
                    crate::workspace_instructions::render_new_instructions_section(
                        &pending_instructions,
                    ),
                );
            }
            // P3.1 (sixth batch): pre-fetched memory hits as a section. The
            // caller (run_llm_loop) did the async search against the CURRENT
            // user message; here we only render. Empty/None ⇒ no section ⇒
            // byte-identical output to auto_inject=false.
            if let Some(hits) = memory_hits
                && !hits.is_empty()
            {
                let body = hits
                    .iter()
                    .map(|h| format!("- {}", h))
                    .collect::<Vec<_>>()
                    .join("\n");
                sections.push(format!(
                        "# Memory Context\n{body}\n\n(以上是自动检索到的相关长期记忆，可能与当前对话有关，也可能无关——自行判断取舍。)"
                    ));
            }
            // I1 (devtool-upgrade 阶段 3): externally-changed workspace
            // files since the last build — one-shot section (drained here;
            // absent when nothing changed, keeping the message byte-stable
            // between turns). The watcher-side ignore table + self-write
            // window keep this quiet; ≤10 entries per flush, ≤32 buffered.
            let external_changes = self.drain_external_changes();
            if !external_changes.is_empty() {
                sections.push(crate::fs_watcher::render_external_changes_section(
                    &external_changes,
                ));
            }
            // I5（devtool-upgrade 阶段 7）：当前轮客户端上报的打开文件路径
            // —— per-turn ephemeral 状态（process_admitted set / 出轮 clear）。
            // 只渲染路径不读内容（annotation 语义；agent 真正读文件仍走
            // read_file 工具与安全 8 层，零新增信任面）。空 = 无 section
            // （字节稳定）；turn 内多迭代状态不变 → 每次构建字节一致；
            // 回放走 InjectionRecord 台账记录的 digest 内容（transient
            // 注入既有机制，零额外回放工作）。
            let open_files = self.pending_open_files.read().clone();
            if !open_files.is_empty() {
                let body = open_files
                    .iter()
                    .map(|p| format!("- {}", p))
                    .collect::<Vec<_>>()
                    .join("\n");
                sections.push(format!(
                    "# Open Files (client-reported)\n{body}\n\n(以上是客户端上报的当前打开文件路径，顺序=上报顺序，仅供参考——读取文件仍受安全策略约束。)"
                ));
            }
            // X2 (U8 refinement): runtime policy facts as the LAST section.
            // All three inputs are plain state rendered without clocks —
            // deterministic (same state ⇒ identical bytes, so the merged
            // message stays byte-stable between turns and the historical
            // prefix before it is untouched either way):
            //   approval — live wiring flag (gateway sets after attaching
            //     the desktop popup adapter);
            //   guardian — live judge presence on the security plugin
            //     (feature-off builds render "off");
            //   model_tier — the live capability tier (auto re-resolves on
            //     config reload; the snapshot picks the new value up at the
            //     next build).
            #[cfg(feature = "security")]
            let guardian_on = self
                .security_plugin
                .as_ref()
                .is_some_and(|p| p.judge().is_some());
            #[cfg(not(feature = "security"))]
            let guardian_on = false;
            let approval_on = *self.interactive_approval.read();
            let tier_now = *self.tier.read();
            // F1（devtool-upgrade 阶段 4）：模式进快照——Plan 时模型当轮就
            // 能读到「不许改文件」，与供给/分发双闸互补（提醒是软约束）。
            let mode_now = self.mode();
            let mode_line = match mode_now {
                crate::types::AgentMode::Plan => "plan（PLAN mode: do not modify files. Present your plan as text; write_file under plans/ is the only write allowed. User can switch back with /build.）".to_string(),
                crate::types::AgentMode::Build => "build（正常全量工具）".to_string(),
            };
            sections.push(format!(
                "# Runtime Policy\napproval: {}\nguardian: {}\nmodel_tier: {}\nmode: {}\n(当前审批/守护/模型档位/工作模式运行时策略快照；策略变更后下一次构建生效。)",
                if approval_on {
                    "interactive（ask 规则触发弹窗审批）"
                } else {
                    "off（无交互审批，ask 规则按默认策略处理）"
                },
                if guardian_on {
                    "on（CRITICAL 操作语义二审）"
                } else {
                    "off"
                },
                tier_now,
                mode_line,
            ));
            if sections.is_empty() {
                None
            } else {
                let merged = format!(
                    "<system-reminder>\n{}\n</system-reminder>",
                    sections.join("\n\n")
                );
                self.skills_digest_state
                    .should_inject("", &merged) // stateless since round-5
                    .map(|m| LlmMessage {
                        // I2 (U8): user-role snapshot (was system) — the
                        // system prompt stays byte-frozen; dynamic facts
                        // arrive as conversation messages. M4: role is
                        // configurable for strict chat templates that
                        // reject adjacent user/user pairs.
                        role: self.snapshot_role.read().clone(),
                        content: m,
                        tool_calls: None,
                        tool_call_id: None,
                        reasoning_content: None,
                        images: Vec::new(),
                    })
            }
        };

        match last_user_idx {
            Some(idx) => {
                let mut messages: Vec<LlmMessage> =
                    Vec::with_capacity(turns.len() + 1 + context_digest_msg.is_some() as usize);
                messages.extend(turns[..idx].iter().map(turn_to_msg));
                if let Some(d) = context_digest_msg {
                    messages.push(d);
                    // T8 (U9 ②): final-vec position of the digest injection —
                    // recorded because it is rebuilt-on-every-request and never
                    // persisted, so replay must re-insert it at this position.
                    annotation.digest_index = Some(messages.len() - 1);
                }
                messages.extend(turns[idx..].iter().map(turn_to_msg));
                (messages, annotation)
            }
            None => (turns.iter().map(turn_to_msg).collect(), annotation),
        }
    }

    // -----------------------------------------------------------------------
    // Slash command handling
    // -----------------------------------------------------------------------

    /// Process an inbound message through routing and slash command handling (standalone).
    ///
    /// Returns (agent_id, response_content, handled).
    pub fn process_message(
        &self,
        content: &str,
        context: &RequestContext,
    ) -> (String, String, bool) {
        // Check for cluster continuation prefix.
        if context.channel == "system"
            && content.starts_with(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
        {
            debug!(
                "[AgentLoop] Cluster continuation message intercepted: {}",
                content
            );
            return (String::new(), String::new(), true);
        }

        // Check for slash commands.
        if let Some(response) = self.handle_command(content) {
            return (String::new(), response, true);
        }

        (String::new(), String::new(), false)
    }

    /// Handle slash commands embedded in message content (standalone, no context).
    pub fn handle_command(&self, content: &str) -> Option<String> {
        self.handle_command_with_context(content, "")
    }

    /// Handle slash commands with optional channel context.
    /// Mirrors Go's `handleCommand()`.
    fn handle_command_with_context(&self, content: &str, current_channel: &str) -> Option<String> {
        let content = content.trim();
        if !content.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        match parts[0] {
            "/help" => Some(
                "Commands: /show [model|channel|agents], /list [tools|models], /model <alias>, /plan (计划模式: 停用文件修改), /build (切回构建模式), /compact (压缩会话上下文), /clear (清空会话历史), /help".to_string(),
            ),
            "/model" => {
                if parts.len() < 2 {
                    let current = self.active_model.read().clone();
                    let aliases = self.model_aliases();
                    Some(format!(
                        "Current model: {}\nAliases: {} (or pass any model id)",
                        current,
                        if aliases.is_empty() {
                            "(none configured)".to_string()
                        } else {
                            aliases.join(", ")
                        }
                    ))
                } else {
                    let new_model = self.set_active_model(parts[1]);
                    Some(format!("✓ Model switched to: {}", new_model))
                }
            }
            "/show" => {
                if parts.len() < 2 {
                    return Some("Usage: /show [model|channel|agents]".to_string());
                }
                match parts[1] {
                    "model" => Some(format!("Current model: {}", self.active_model.read())),
                    "channel" => Some(format!("Current channel: {}", current_channel)),
                    "agents" => {
                        let agent_ids = self
                            .registry
                            .as_ref()
                            .map(|r| r.list_agent_ids())
                            .unwrap_or_default();
                        if agent_ids.is_empty() {
                            let guard = self.tools.read();
                            let tool_names: Vec<&str> =
                                guard.keys().map(|s| s.as_str()).collect();
                            Some(format!("Registered agents (tools): {}", tool_names.join(", ")))
                        } else {
                            Some(format!("Registered agents: {}", agent_ids.join(", ")))
                        }
                    }
                    _ => Some(format!("Unknown show target: {}", parts[1])),
                }
            }
            "/list" => {
                if parts.len() < 2 {
                    return Some("Usage: /list [models|channels|agents|tools]".to_string());
                }
                match parts[1] {
                    "tools" => {
                        let guard = self.tools.read();
                        let tool_names: Vec<&str> =
                            guard.keys().map(|s| s.as_str()).collect();
                        Some(format!("Available tools: {}", tool_names.join(", ")))
                    }
                    "model" | "models" => Some(format!(
                        "Current model: {} (configured in config.json)",
                        self.active_model.read()
                    )),
                    "channels" => {
                        let channels = self.channel_manager_channels.lock();
                        if channels.is_empty() {
                            Some("No channels enabled".to_string())
                        } else {
                            Some(format!("Enabled channels: {}", channels.join(", ")))
                        }
                    }
                    "agents" => {
                        let agent_ids = self
                            .registry
                            .as_ref()
                            .map(|r| r.list_agent_ids())
                            .unwrap_or_default();
                        if agent_ids.is_empty() {
                            let guard = self.tools.read();
                            let tool_names: Vec<&str> =
                                guard.keys().map(|s| s.as_str()).collect();
                            Some(format!("Registered agents: {}", tool_names.join(", ")))
                        } else {
                            Some(format!("Registered agents: {}", agent_ids.join(", ")))
                        }
                    }
                    _ => Some(format!("Unknown list target: {}", parts[1])),
                }
            }
            "/switch" => {
                if parts.len() < 4 || parts[2] != "to" {
                    return Some("Usage: /switch [model|channel] to <name>".to_string());
                }
                let target = parts[1];
                let value = parts[3];

                match target {
                    "model" => {
                        let old_model = self.active_model.read().clone();
                        Some(format!(
                            "Model switch requested: {} -> {} (restart required for persistent change)",
                            old_model, value
                        ))
                    }
                    "channel" => Some(format!("Target channel switched to: {}", value)),
                    _ => Some(format!("Unknown switch target: {}", target)),
                }
            }
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Startup info
    // -----------------------------------------------------------------------

    /// Get startup information about the agent loop for logging.
    /// Mirrors Go's `GetStartupInfo()`.
    pub fn get_startup_info(&self) -> serde_json::Value {
        let guard = self.tools.read();
        let tool_names: Vec<&str> = guard.keys().map(|s| s.as_str()).collect();

        let agent_ids = self
            .registry
            .as_ref()
            .map(|r| r.list_agent_ids())
            .unwrap_or_default();

        serde_json::json!({
            "tools": {
                "count": tool_names.len(),
                "names": tool_names,
            },
            "agents": {
                "count": agent_ids.len(),
                "ids": agent_ids,
            },
            "model": self.active_model.read().to_string(),
            "max_turns": self.config.max_turns,
            "system_prompt_configured": self.config.system_prompt.is_some(),
        })
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Returns a reference to the tool registry.
    pub fn tools(&self) -> parking_lot::RwLockReadGuard<'_, HashMap<String, Arc<dyn Tool>>> {
        self.tools.read()
    }

    /// Returns a reference to the agent config.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Mark that a message was sent for the given session in the current round.
    /// Used by tools like MessageTool to prevent double-sending. Mirrors Go's
    /// MessageTool.sentInRound.
    pub fn mark_sent_in_round(&self, session_key: &str) {
        self.sent_in_round.mark_sent(session_key);
    }

    /// Check if a message was already sent in the current round for a session.
    pub fn has_sent_in_round(&self, session_key: &str) -> bool {
        self.sent_in_round.has_sent_in_round(session_key)
    }
}

// ---------------------------------------------------------------------------
// Standalone summarization helpers (usable from spawned tasks)
// ---------------------------------------------------------------------------

/// Adjust a summarize boundary so the verbatim tail `history[new_c..]` doesn't
/// START in the middle of a tool_call/result pair.
///
/// `summarize_prefix_owned` only folds user/assistant `content` into the
/// summary — tool_calls and tool results are not summarized. If the boundary
/// landed between an assistant tool_call (covered by the summary only as text
/// content, which is often empty for a pure tool-call turn) and its tool
/// result, `repair_tool_message_pairs` would drop the orphan result from the
/// tail and the whole interaction would vanish from the LLM's view. Backing
/// `new_c` up past any leading tool messages moves the parent assistant (and
/// sibling results) into the verbatim tail, keeping the pair intact. (The old
/// `truncate_with_tool_pairs` did the equivalent by prepending the parent.)
///
/// The summary still covers `history[..returned_new_c]` and the tail is
/// `history[returned_new_c..]` — the gap-free invariant holds; the tail just
/// grows slightly past `K_TARGET` when a pair straddles the boundary.
fn tool_safe_boundary(history: &[crate::types::ConversationTurn], mut new_c: usize) -> usize {
    while new_c > 0 && new_c < history.len() && history[new_c].role == "tool" {
        new_c -= 1;
    }
    new_c
}

/// Summarize a contiguous prefix of the conversation, merging any existing
/// summary.
///
/// Summarizes **all** of `messages` (no internal "keep last N" step) — the
/// caller has already chosen the verbatim tail boundary (`K_TARGET`), so every
/// message passed in is meant to be folded into the summary. Keeping a "last N"
/// here would leave a gap between the summary and the verbatim tail. Reuses the
/// multipart/batch machinery; merges `existing_summary` (which covers messages
/// before this prefix) into the result.
///
/// G1 (U1) prefix-reuse: the summary request is built as
/// `[system, ...original covered messages..., instruction]` — the same leading
/// messages the main loop sends (byte-equal per message), so the provider's
/// warm KV prefix from the last routed request is REUSED rather than
/// invalidated ("genuine prefix" principle). The old
/// form (single bare user message with `role: content` text concatenation)
/// shared no prefix with real requests and destroyed structure (tool_calls
/// flattened to text).
///
/// Returns `Some(summary)` if a non-empty summary was produced, `None`
/// otherwise. **None 的两种含义都不允许调用方推进 covers_up_to**：
/// (a) 前缀里没有可摘要的 user/assistant 内容；(b) 摘要 LLM 调用失败
/// （2026-08-25 摘要静默失败修复：此前 batch 失败返回空字符串，multipart
/// 拿两段空串去 merge，merge 模型回"你没把摘要贴给我"，这段**失败回复**
/// 被当摘要存进 store、covers 照常推进——被折叠的上下文静默丢失。现在
/// 失败一路传播为 None，调用方保持原 history 不折叠并 warn。）
///
/// T4 (U1) per-model switch: `prefix_reuse == false` falls back to the
/// pre-G1 shape (`summarize_bare_concat_owned`) — per-model config
/// `summarizer_prefix_reuse: false`, for cheap summarizer models that break
/// the assumed warm KV prefix. Default (true) keeps the prefix-reuse shape.
async fn summarize_prefix_owned(
    messages: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    context_window: usize,
    prefix_reuse: bool,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    // Oversized message guard.
    let max_msg_tokens = context_window / 2;
    let mut valid_messages: Vec<&crate::types::ConversationTurn> = Vec::new();
    let mut omitted = false;

    for m in messages {
        if m.role != "user" && m.role != "assistant" {
            continue;
        }
        let msg_tokens = crate::session::estimate_tokens(&m.content);
        if msg_tokens > max_msg_tokens {
            omitted = true;
            continue;
        }
        valid_messages.push(m);
    }

    if valid_messages.is_empty() {
        return None;
    }

    let final_summary = if !prefix_reuse {
        // T4 (U1): old shape — single bare user message, no structure.
        summarize_bare_concat_owned(
            &valid_messages,
            existing_summary,
            provider,
            model,
            observer_manager,
        )
        .await
    } else {
        // G1: the system prompt anchoring the prefix. `messages` is
        // history[..new_c]; history[0] is the system turn — include it verbatim
        // (WITHOUT the summary block the main loop appends: that would leak the
        // old summary into the prefix and change it between rounds).
        let system_msg: Option<LlmMessage> = messages
            .first()
            .filter(|m| m.role == "system")
            .map(|m| conversation_turn_to_llm_message(m));

        if valid_messages.len() > 10 {
            summarize_multipart_owned(
                system_msg.as_ref(),
                &valid_messages,
                existing_summary,
                provider,
                model,
                observer_manager,
            )
            .await
        } else {
            summarize_batch_owned(
                system_msg.as_ref(),
                &valid_messages,
                existing_summary,
                provider,
                model,
                observer_manager,
            )
            .await
        }
    };

    let final_summary = match final_summary {
        Some(s) if omitted && !s.is_empty() => Some(format!(
            "{}\n[Note: Some oversized messages were omitted from this summary for efficiency.]",
            s
        )),
        other => other,
    };

    final_summary.filter(|s| !s.is_empty())
}

/// G1 (U1): build an LLM wire message from a ConversationTurn preserving the
/// original structure (role/content/tool_calls/tool_call_id/reasoning) — the
/// same projection `build_messages`'s `turn_to_msg` performs. Shared by the
/// summarizers so the summary request's prefix messages are byte-equal to the
/// main loop's.
///
/// T6（多模态）：摘要请求**不发图片字节**（摘要模型可能是不支持视觉的廉价
/// 档，且图片字节会破坏 G1 前缀复用的字节等价前提）——历史 turn 的
/// `image_refs` 在此不水合，`images` 恒空。失效图片的占位文本也只出现在
/// 主循环请求（`turn_to_request_message`）里，摘要请求保持纯文本。
fn conversation_turn_to_llm_message(turn: &crate::types::ConversationTurn) -> LlmMessage {
    LlmMessage {
        role: turn.role.clone(),
        content: turn.content.clone(),
        tool_calls: if turn.tool_calls.is_empty() {
            None
        } else {
            Some(turn.tool_calls.clone())
        },
        tool_call_id: turn.tool_call_id.clone(),
        reasoning_content: turn.reasoning_content.clone(),
        images: Vec::new(),
    }
}

/// T5/T6（多模态）：主循环请求的 history turn → 请求消息转换（`build_messages`
/// 与 replay 重建共用，水合语义单点）。路径引用**每轮重读**：验真通过 →
/// base64 字节进 `LlmMessage.images`；文件已删/失效 → 占位文本行
/// `[图片已失效: <path>]` 追加进 content（诚实降级，不静默、不炸）。
pub(crate) fn turn_to_request_message(turn: &crate::types::ConversationTurn) -> LlmMessage {
    let mut msg = conversation_turn_to_llm_message(turn);
    if turn.image_refs.is_empty() {
        return msg;
    }
    let (images, placeholders) = crate::image_attach::hydrate_image_refs(&turn.image_refs);
    if !placeholders.is_empty() {
        msg.content.push('\n');
        msg.content.push_str(&placeholders.join("\n"));
    }
    msg.images = images;
    msg
}

/// T10（多模态 D4）：vision=no 模型的 image_refs 确定性投影（纯函数，生产
/// build 与 replay 重建共用）。带引用的 turn 一律清空 `image_refs` 并把占位
/// 文本行追加进 content：
/// - 最新 user 轮（本轮要发的图）→ `[图片未发送: 当前模型 vision=no（不支持
///   图像输入），图片已忽略]`（拒绝注明，文本照常处理，与 T5 失败路径同构）；
/// - 其余（历史）轮 → `[图片已省略: 当前模型仅支持文本]`（模型中途切换不
///   砖会话）。
///
/// 同输入同输出（无时钟/无 IO）：历史前缀字节稳定保 prompt cache；占位写入
/// 的是**投影视图**——历史持久化的 image_refs 不动，切回 vision 模型图片
/// 原样回来。`last_user_idx` 用与 build 注入点判定完全相同的表达式。
pub(crate) fn project_turns_for_no_vision(
    turns: &mut [crate::types::ConversationTurn],
    last_user_idx: Option<usize>,
) {
    for (i, turn) in turns.iter_mut().enumerate() {
        if turn.image_refs.is_empty() {
            continue;
        }
        turn.image_refs.clear();
        let note = if Some(i) == last_user_idx {
            "[图片未发送: 当前模型 vision=no（不支持图像输入），图片已忽略]"
        } else {
            "[图片已省略: 当前模型仅支持文本]"
        };
        if turn.content.trim().is_empty() {
            turn.content.push_str(note);
        } else {
            turn.content.push('\n');
            turn.content.push_str(note);
        }
    }
}

/// T10（多模态 D4 ③）：provider 4xx 兜底——请求带图且本轮调用最终失败时，
/// 把逃生通道（probe 实测 / vision=no）附加到**用户可见**错误文案。措辞条件
/// 化（"若该错误与图像输入有关"），对无关失败（网络超时等）不误导；历史、
/// request_log 与 observer 事件里的原始错误保持干净（不带提示）。
fn append_vision_fallback_hint(err_text: String, request_had_images: bool) -> String {
    if !request_had_images {
        return err_text;
    }
    format!(
        "{}\n[提示: 本次请求携带了图片。若该错误与图像输入有关（如模型不支持视觉），\
         可运行 `model probe <模型名>` 实测视觉能力，或在 config.json 对应模型条目\
         设置 \"vision\": \"no\" 后重试。]",
        err_text
    )
}

/// The trailing instruction for a G1 prefix-reuse summary request. Kept in one
/// place so the batch and multipart paths emit the identical instruction.
const SUMMARIZE_INSTRUCTION: &str =
    "请对以上对话片段做一份简明摘要，保留核心上下文与关键要点，供后续对话作为前情提要使用。";

/// T4 (U1): pre-G1 summary shape, restored as the per-model fallback
/// (`summarizer_prefix_reuse: false`).
///
/// This is the OLD request form the G1 refactor replaced: a single bare user
/// message whose content is the covered messages flattened as
/// `role: content` text lines, plus the instruction (and any existing-summary
/// context). It shares NO prefix with real requests and destroys structure
/// (tool_calls flatten to text) — which is exactly why it is NOT the default.
/// It remains useful for cheap summarizer models whose warm-KV-prefix
/// assumption G1 relies on does not hold (different tokenizer, no prompt
/// caching): a shape-neutral single message is the lowest-common-denominator
/// request those models handle reliably. The G1 prefix-reuse path
/// (summarize_multipart_owned / summarize_batch_owned) stays the default for
/// the main model; this function is invoked ONLY when the per-model switch
/// opts out.
/// Returns `Some(summary)` on a non-empty LLM reply; `None` on LLM failure or
/// empty reply (failure must propagate — never fold history behind a failed
/// summary; see the 2026-08-25 fix note on `summarize_prefix_owned`).
async fn summarize_bare_concat_owned(
    messages: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mut content = String::new();
    if !existing_summary.is_empty() {
        content.push_str(&format!(
            "Existing context (summary of the earlier conversation, merge with the new summary): {}\n\n",
            existing_summary
        ));
    }
    for m in messages {
        content.push_str(&format!("{}: {}\n", m.role, m.content));
    }
    content.push_str(SUMMARIZE_INSTRUCTION);

    let llm_messages = vec![LlmMessage {
        role: "user".to_string(),
        content,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }];

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-bare-concat",
        model,
        provider.chat(model, llm_messages, None, vec![]),
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => None,
        Some(Err(e)) => {
            warn!(
                "[AgentLoop] summarize_bare_concat_owned LLM call failed (summary NOT produced, history stays unfolded): {}",
                e
            );
            None
        }
        None => None,
    }
}

/// Multi-part summarization (standalone, works in spawned task).
///
/// G1 (U1): each part is summarized as `[system?, ...part messages,
/// instruction]` — a part is a contiguous slice of the covered prefix, so its
/// message list is a true ordered prefix subset of the main request's history
/// (prefix-cache friendly in the same way).
///
/// Returns `Some` only when BOTH parts produced real summaries. If either
/// part's LLM call fails → `None` (whole summarization fails; the caller must
/// keep the history unfolded — a half-empty merge prompt makes the merge model
/// answer with a "you didn't paste the summaries" complaint, and storing that
/// complaint as the summary silently amnesiates the covered prefix; this is
/// the exact production failure found in the legacy session 2026-08-25).
async fn summarize_multipart_owned(
    system_msg: Option<&LlmMessage>,
    messages: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mid = messages.len() / 2;
    let part1 = &messages[..mid];
    let part2 = &messages[mid..];

    let s1 = summarize_batch_owned(
        system_msg,
        part1,
        existing_summary,
        provider,
        model,
        observer_manager.clone(),
    )
    .await;
    let s2 = summarize_batch_owned(
        system_msg,
        part2,
        "",
        provider,
        model,
        observer_manager.clone(),
    )
    .await;

    let (s1, s2) = match (s1, s2) {
        (Some(a), Some(b)) => (a, b),
        (failed, _) => {
            warn!(
                "[AgentLoop] multipart summarization aborted: one part failed to summarize ({}); summary NOT produced, history stays unfolded",
                if failed.is_none() { "part 1" } else { "part 2" }
            );
            return None;
        }
    };

    // Merge via LLM.
    let merge_prompt = format!(
        "Merge these two conversation summaries into one cohesive summary:\n\n1: {}\n\n2: {}",
        s1, s2
    );

    let llm_messages = vec![LlmMessage {
        role: "user".to_string(),
        content: merge_prompt,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }];

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-multipart-merge",
        model,
        provider.chat(model, llm_messages, None, vec![]),
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => {
            // Empty merge reply: both parts are validated non-empty —
            // concatenate them instead of losing the coverage.
            Some(format!("{}\n\n{}", s1, s2))
        }
        _ => {
            // Merge call failed: same fallback — both parts hold real
            // summaries, so concatenation preserves the information (the
            // next compaction round will re-fold them into a merged one).
            warn!(
                "[AgentLoop] multipart merge LLM call failed; falling back to concatenating the two (validated) part summaries"
            );
            Some(format!("{}\n\n{}", s1, s2))
        }
    }
}

/// Single-batch summarization (standalone, works in spawned task).
///
/// G1 (U1): the request is `[system?, ...covered messages (original
/// structure), instruction]` — a genuine prefix of the conversation plus the
/// trailing instruction, replacing the old single bare user message with
/// `role: content` text concatenation.
///
/// Returns `Some` on a non-empty LLM reply; `None` on LLM failure or empty
/// reply (failure propagates — see the 2026-08-25 fix note on
/// `summarize_prefix_owned`).
async fn summarize_batch_owned(
    system_msg: Option<&LlmMessage>,
    batch: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mut messages: Vec<LlmMessage> = Vec::with_capacity(batch.len() + 2);
    if let Some(sys) = system_msg {
        messages.push(sys.clone());
    }
    for m in batch {
        messages.push(conversation_turn_to_llm_message(m));
    }
    // Trailing instruction (merged with any existing-summary context so the
    // fold still carries prior coverage).
    let mut instruction = String::new();
    if !existing_summary.is_empty() {
        instruction.push_str(&format!(
            "Existing context (summary of the earlier conversation, merge with the new summary): {}\n\n",
            existing_summary
        ));
    }
    instruction.push_str(SUMMARIZE_INSTRUCTION);
    messages.push(LlmMessage {
        role: "user".to_string(),
        content: instruction,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    });

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-batch",
        model,
        provider.chat(model, messages, None, vec![]),
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => None,
        Some(Err(e)) => {
            warn!(
                "[AgentLoop] summarize_batch_owned LLM call failed (summary NOT produced, history stays unfolded): {}",
                e
            );
            None
        }
        None => None,
    }
}

/// Emit observer events (ConversationStart, LlmRequest, LlmResponse, ConversationEnd)
/// around a synchronous LLM call closure. Used by standalone summarization functions.
async fn emit_observer_events_around_llm<Fut>(
    observer_manager: Option<&Arc<nemesis_observer::Manager>>,
    label: &str,
    model: &str,
    llm_call: Fut,
) -> Option<Result<LlmResponse, String>>
where
    Fut: std::future::Future<Output = Result<LlmResponse, String>>,
{
    use crate::loop_executor::ObserverEvent;

    let trace_id = format!(
        "{}-{}",
        label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );

    // Emit ConversationStart + LlmRequest before the call (await, no block_in_place).
    if let Some(mgr) = observer_manager {
        let start_event = ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: label.to_string(),
            channel: String::new(),
            chat_id: String::new(),
            sender_id: "summarizer".to_string(),
            content: String::new(),
        };
        mgr.emit(start_event.to_conversation_event()).await;

        let request_event = ObserverEvent::LlmRequest {
            trace_id: trace_id.clone(),
            round: 0,
            model: model.to_string(),
            messages: vec![],
            tools: vec![],
            messages_count: 0,
            tools_count: 0,
            provider_name: String::new(),
            api_key: String::new(),
            api_base: String::new(),
        };
        mgr.emit(request_event.to_conversation_event()).await;
    }

    // Execute the LLM call (async, no block_on).
    let start = std::time::Instant::now();
    let mut response = llm_call.await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (response_content, raw_req, raw_resp) = match &mut response {
        Ok(r) => {
            let content = r.content.clone();
            let req = r.raw_request_body.take();
            let resp = r.raw_response_body.take();
            (content, req, resp)
        }
        Err(_) => (String::new(), None, None),
    };

    // Emit LlmResponse + ConversationEnd after the call (await, sequential —
    // LlmResponse fully processed before ConversationEnd, as the old emit_sync intended).
    if let Some(mgr) = observer_manager {
        let response_event = ObserverEvent::LlmResponse {
            trace_id: trace_id.clone(),
            round: 0,
            duration_ms,
            has_tool_calls: false,
            content: response_content.clone(),
            tool_calls: vec![],
            tool_calls_count: 0,
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_request_body: raw_req,
            raw_response_body: raw_resp,
        };
        mgr.emit(response_event.to_conversation_event()).await;

        let end_event = ObserverEvent::ConversationEnd {
            trace_id,
            session_key: label.to_string(),
            total_rounds: 1,
            duration_ms,
            content: response_content,
            channel: String::new(),
            chat_id: String::new(),
        };
        mgr.emit(end_event.to_conversation_event()).await;
    }

    Some(response)
}

// ---------------------------------------------------------------------------
// Cluster integration helpers
// ---------------------------------------------------------------------------

/// Extract the task ID from a cluster continuation sender ID.
///
/// The format is `cluster_continuation:{taskID}`.
#[cfg(test)]
pub fn extract_continuation_task_id(sender_id: &str) -> Option<&str> {
    sender_id.strip_prefix(nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX)
}

/// Extract a peer identifier from an inbound message.
///
/// Looks at metadata fields to determine the originating peer.
/// Mirrors Go's `extractPeer`:
/// - If `peer_kind` is set, uses `peer_id` (falls back to sender_id for "direct", chat_id otherwise)
/// - If no metadata, returns sender_id
pub fn extract_peer(msg: &nemesis_types::channel::InboundMessage) -> String {
    if let Some(peer_kind) = msg.metadata.get("peer_kind")
        && !peer_kind.is_empty()
    {
        let peer_id = msg.metadata.get("peer_id").cloned().unwrap_or_else(|| {
            if peer_kind == "direct" {
                msg.sender_id.clone()
            } else {
                msg.chat_id.clone()
            }
        });
        return format!("{}:{}", peer_kind, peer_id);
    }
    msg.sender_id.clone()
}

/// Extract the parent peer identifier from an inbound message.
///
/// Used for routing in nested or forwarded messages.
/// Mirrors Go's `extractParentPeer`.
#[cfg(test)]
pub fn extract_parent_peer(msg: &nemesis_types::channel::InboundMessage) -> Option<String> {
    let parent_kind = msg.metadata.get("parent_peer_kind")?;
    let parent_id = msg.metadata.get("parent_peer_id")?;
    if parent_kind.is_empty() || parent_id.is_empty() {
        return None;
    }
    Some(format!("{}:{}", parent_kind, parent_id))
}

/// Route input for agent resolution.
///
/// This is a legacy compatibility type. For new code, use
/// [`nemesis_routing::RouteInput`] directly with [`RouteResolver`].
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct RouteInput {
    pub channel: String,
    pub account_id: Option<String>,
    pub peer: String,
    pub parent_peer: Option<String>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
}

/// Resolved route for a message.
///
/// This is a legacy compatibility type. For new code, use
/// [`nemesis_routing::ResolvedRoute`] directly.
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct RouteOutput {
    pub agent_id: String,
    pub session_key: String,
    pub matched_by: String,
}

/// Resolve the route for a message to determine which agent and session to use.
///
/// Uses the full `RouteResolver` with a default single-agent configuration.
/// The peer field is parsed from the format "kind:id" to extract peer_kind and peer_id.
/// Mirrors Go's `al.registry.ResolveRoute(routing.RouteInput{...})`.
#[cfg(test)]
pub fn resolve_route(input: &RouteInput) -> RouteOutput {
    // Parse peer from "kind:id" format (as produced by extract_peer).
    let (peer_kind, peer_id) = if let Some(colon_pos) = input.peer.find(':') {
        let kind = input.peer[..colon_pos].to_string();
        let id = input.peer[colon_pos + 1..].to_string();
        (Some(kind), Some(id))
    } else {
        // Treat as just an ID with no kind
        (None, Some(input.peer.clone()))
    };

    // Parse parent_peer from "kind:id" format.
    let (parent_peer_kind, parent_peer_id) = input
        .parent_peer
        .as_ref()
        .and_then(|pp| {
            pp.find(':').map(|colon_pos| {
                (
                    Some(pp[..colon_pos].to_string()),
                    Some(pp[colon_pos + 1..].to_string()),
                )
            })
        })
        .unwrap_or((None, None));

    let route_input = RoutingRouteInput {
        channel: input.channel.clone(),
        account_id: input.account_id.clone().unwrap_or_default(),
        peer_kind,
        peer_id,
        parent_peer_kind,
        parent_peer_id,
        guild_id: input.guild_id.clone(),
        team_id: input.team_id.clone(),
        identity_links: std::collections::HashMap::new(),
    };

    // Build a default resolver with a single "main" agent and no bindings.
    let config = RouteConfig {
        bindings: Vec::new(),
        agents: vec![AgentDef {
            id: "main".to_string(),
            is_default: true,
        }],
        dm_scope: "main".to_string(),
    };
    let resolver = RouteResolver::new(config);
    let route = resolver.resolve(&route_input);

    RouteOutput {
        agent_id: route.agent_id,
        session_key: route.session_key,
        matched_by: route.matched_by,
    }
}

/// Build an agent-scoped main session key.
///
/// Format: `agent:{agent_id}:main`
pub fn build_agent_main_session_key(agent_id: &str) -> String {
    format!("agent:{}:main", agent_id)
}

// ---------------------------------------------------------------------------
// Message formatting utilities
// ---------------------------------------------------------------------------

/// Format messages for log output, truncating long content.
///
/// Returns a human-readable multi-line representation of the message list
/// suitable for debug logging.
#[cfg(test)]
pub fn format_messages_for_log(messages: &[LlmMessage]) -> String {
    if messages.is_empty() {
        return "[]".to_string();
    }

    let mut result = String::from("[\n");
    for (i, msg) in messages.iter().enumerate() {
        result.push_str(&format!("  [{}] Role: {}\n", i, msg.role));

        if let Some(ref tool_calls) = msg.tool_calls {
            result.push_str("  ToolCalls:\n");
            for tc in tool_calls {
                let args_preview = truncate(&tc.arguments, 200);
                result.push_str(&format!("    - ID: {}, Name: {}\n", tc.id, tc.name));
                result.push_str(&format!("      Arguments: {}\n", args_preview));
            }
        }

        if !msg.content.is_empty() {
            let content_preview = truncate(&msg.content, 200);
            result.push_str(&format!("  Content: {}\n", content_preview));
        }

        if let Some(ref tcid) = msg.tool_call_id {
            result.push_str(&format!("  ToolCallID: {}\n", tcid));
        }

        result.push('\n');
    }
    result.push(']');
    result
}

/// Format tools for log output.
#[cfg(test)]
pub fn format_tools_for_log(tools: &[ToolCallInfo]) -> String {
    if tools.is_empty() {
        return "[]".to_string();
    }
    let mut result = String::from("[\n");
    for tc in tools {
        let args_preview = truncate(&tc.arguments, 200);
        result.push_str(&format!(
            "  - ID: {}, Name: {}, Args: {}\n",
            tc.id, tc.name, args_preview
        ));
    }
    result.push(']');
    result
}

/// Truncate a string to a maximum byte length, appending "..." if truncated.
/// UTF-8 safe: finds the nearest char boundary before slicing.
pub fn truncate(s: &str, max_len: usize) -> String {
    nemesis_types::utils::truncate(s, max_len)
}

/// P3.1 (sixth batch): cheap text similarity (char-bigram Jaccard) for the
/// auto-inject dedup pass. Approximates "same memory, near-identical text"
/// without another embedding call — the dedup bar (0.92) is deliberately
/// high so only true near-duplicates are dropped.
// Only called from the `#[cfg(feature = "memory")]` dedup pass — same gate
// pattern as `prefetch_memory_context` above, so memory-off builds stay
// warning-free.
#[cfg_attr(not(feature = "memory"), allow(dead_code))]
fn textwise_similar(a: &str, b: &str) -> f64 {
    let bigrams = |s: &str| -> std::collections::HashSet<(char, char)> {
        let t: Vec<char> = s.chars().collect();
        t.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let (ba, bb) = (bigrams(a), bigrams(b));
    if ba.is_empty() && bb.is_empty() {
        return 1.0; // both empty (or single-char) → identical
    }
    if ba.is_empty() || bb.is_empty() {
        return 0.0;
    }
    let inter = ba.intersection(&bb).count() as f64;
    let union = ba.union(&bb).count() as f64;
    inter / union
}

/// F8 (devtool-upgrade 阶段 3): wildcard-aware membership test for
/// `agents.hidden_tools`. An entry matches a tool name either exactly or,
/// when it ends with `*`, as a prefix (`mcp_*` hides every MCP tool). A bare
/// `*` hides everything. Matching is case-sensitive (tool names are
/// lowercase by convention). Shared by BOTH gates — the supply side
/// ([`AgentLoop::build_tool_defs`]) and the dispatch side
/// ([`AgentLoop::handle_tool_call_at_depth`]) — so the two can never
/// disagree about what is hidden.
pub(crate) fn tool_name_matches_hidden(entries: &[String], name: &str) -> bool {
    entries.iter().any(|entry| {
        if let Some(prefix) = entry.strip_suffix('*') {
            name.starts_with(prefix)
        } else {
            entry == name
        }
    })
}

/// J3 (devtool-upgrade 阶段 4)：从「配置的 server 名 × 已注册工具键」正推
/// 已注册前缀。与 [`nemesis_mcp::manager::McpManager::find_new_servers`] 的
/// 前缀计算同源（`mcp_<sanitize(server)>_`），再看工具表里有没有以它开头
/// 的键。旧实现从注册键反推切分（数下划线取 [2]）：`underscores[2]` 是
/// 第 3 个下划线，注释声称 `mcp_<srv>_` 实则多切一段（server 或工具名含
/// 下划线时切错 → 热重载误判 server 未注册而重复发现）；恰好 2 个下划线
/// 的键还会越界 panic（guard `len() >= 2` 但索引 [2]）。正推天然无歧义。
fn registered_server_prefixes(configured_servers: &[String], tool_keys: &[String]) -> Vec<String> {
    configured_servers
        .iter()
        .map(|s| format!("mcp_{}_", nemesis_mcp::adapter::sanitize_name(s)))
        .filter(|prefix| tool_keys.iter().any(|k| k.starts_with(prefix.as_str())))
        .collect()
}

#[cfg(test)]
mod inbox_tests;
// 自定义 slash 命令改写（2026-08-29）：rewrite_custom_command 决策表测试。
#[cfg(test)]
mod commands_tests;
// N1 (devtool-upgrade 阶段 1)：三级 context_window 解析链测试。
#[cfg(test)]
mod context_window_tests;
// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
// G4 (devtool-upgrade 阶段 3)：subagent 后台化 + 完成回灌测试。
#[cfg(test)]
mod g4_background_spawn_tests;
// E6 (devtool-upgrade 阶段 2)：手动会话维护（/compact /clear）测试。
#[cfg(test)]
mod e6_maintenance_tests;
// C3 (devtool-upgrade 阶段 2)：编辑后诊断回灌测试（fake LSP server）。
#[cfg(test)]
mod diagnostics_feedback_tests;
// G0 (devtool-upgrade 阶段 3)：SpawnTool 生产化 + run_detached 测试。
#[cfg(test)]
mod spawn_detached_tests;
// F8 (devtool-upgrade 阶段 3)：hidden_tools 双闸（供给过滤 + dispatch 拦截）测试。
#[cfg(test)]
mod f8_hidden_tools_tests;
// M5 (devtool-upgrade 阶段 3)：会话级 context 占用快照测试。
#[cfg(test)]
mod m5_context_status_tests;
// I3 (devtool-upgrade 阶段 3)：子目录指令懒注入（发现 + 会话去重 + 一次性注入）测试。
#[cfg(test)]
mod i3_lazy_instructions_tests;
// F1 (devtool-upgrade 阶段 4)：plan/build 双模式（供给过滤 + dispatch 闸 +
// plans/ 写放行 + slash 切换 + ModeChanged 事件）测试。
#[cfg(test)]
mod f1_plan_mode_tests;
// J3 (devtool-upgrade 阶段 4)：MCP 客户端补齐 agent 侧测试（前缀前向推导
// 回归锁 + 同名冲突改名）。
#[cfg(test)]
mod mcp_reload_tests;
// N2 (devtool-upgrade 阶段 4)：`agents.small_model` 小模型杂务通道测试
// （摘要路由小模型 / 主模型零调用 / 自动压缩路径不受影响）。
#[cfg(test)]
mod n2_small_model_tests;
// D3 (devtool-upgrade 阶段 5)：消息↔文件变更映射的 agent 侧测试
// （dispatch 瀑布收集独立于 checkpoint 挂载 / drain 即清 + 去重）。
#[cfg(test)]
mod d3_tests;
// E3 (devtool-upgrade 阶段 5)：消息级回退/重做编排测试
// （turn 对齐截断 + 文件恢复 + redo 回填 + 陈旧性守卫）。
#[cfg(test)]
mod e3_tests;
// M3 (devtool-upgrade 阶段 5)：会话级 diff 查看器测试（git/JSON 双形态
// 基线对比 + 未挂载/未知路径诚实报错）。
#[cfg(test)]
mod m3_tests;
// E7 (devtool-upgrade 阶段 5)：会话标题自动生成测试（清洗规则 / 端到端
// spawn 落盘 / 无小模型与手动改名跳过）。
#[cfg(test)]
mod e7_tests;
// I5 (devtool-upgrade 阶段 7)：打开文件上下文测试（digest section 渲染
// 有/无两形态字节稳定 + per-turn set/clear 生命周期 + 解析纯函数联测）。
#[cfg(test)]
mod i5_open_files_tests;
// K4 (devtool-upgrade 阶段 7)：IM 编码入口测试（派发语法解析 + 自足续行
// 快照构造的合法消息序列 + B 端变更摘要渲染封顶/溢出注记）。
#[cfg(test)]
mod k4_user_dispatch_tests;
#[cfg(test)]
mod tests;
