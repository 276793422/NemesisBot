//! Model-free tool-result pruning (U3).
//!
//! Oversized plain-text tool results are replaced, before they are fed back
//! into the conversation, with a bounded head + omission marker + bounded
//! tail. This is the cheap first gate before the expensive LLM summarization
//! tier: a giant `grep`/`exec` output no longer blows up the context window
//! and the summary request that follows it.
//!
//! Design note (documented deliberately): an alternative shape keeps the FULL
//! original in an append-only event log and only rewrites the projection.
//! Here there is no separate projection layer — the history IS the storage —
//! so the pruned form is what history keeps. Consequence: the elided middle is not
//! recoverable from history (models that need it must re-run the tool with a
//! narrower query, or use the U4 spill path above the spill threshold).
//! Marker wording tells the model exactly that.

/// Max characters (not bytes) of a tool result kept inline in the
/// conversation. Results at or below this pass through untouched.
pub const MAX_TOOL_RESULT_INLINE_CHARS: usize = 8192;

/// How many characters of head/tail each to keep when pruning (the marker
/// takes the rest of the budget).
const PRUNE_HEAD_CHARS: usize = 3600;
const PRUNE_TAIL_CHARS: usize = 3600;

/// B3 (devtool-upgrade 阶段 3): appended to prune/spill gate text when the
/// registry HAS a spawn tool — the elided content is unrecoverable from the
/// pruned form, and a sub-agent can read the full source (spill file via
/// read_file, or re-run the tool narrowly) and return only the relevant
/// digest without burdening the main session's context.
pub const SUBAGENT_HINT_SUFFIX: &str =
    "也可派子代理（spawn 工具）读取该内容并只回传相关摘要，避免主会话占用上下文。";

/// Prune an oversized tool result to head + marker + tail. Char-based (not
/// byte-based) so multi-byte text can never panic on a slice boundary — see
/// the `str-slice-multibyte-panic` incident class. `tool_name` is included in
/// the marker so the model knows which call produced the elision.
///
/// `hint_subagent` (B3): appends the spawn hint — true only when the tool
/// registry contains a spawn tool. NOTE: the hinted text is NOT recomputable
/// by the pure projection path (the flag is registry state, not turn data),
/// so callers that apply the hint must record it as the turn's
/// `tool_result_projection` (loop.rs does); recompute sites (types.rs) pass
/// `false`.
///
/// Returns the pruned string, or `None` when the result is within budget
/// (caller keeps the original).
pub fn prune_tool_result(result: &str, tool_name: &str, hint_subagent: bool) -> Option<String> {
    let total_chars = result.chars().count();
    if total_chars <= MAX_TOOL_RESULT_INLINE_CHARS {
        return None;
    }
    let omitted = total_chars - PRUNE_HEAD_CHARS - PRUNE_TAIL_CHARS;
    let head: String = result.chars().take(PRUNE_HEAD_CHARS).collect();
    let tail: String = result
        .chars()
        .skip(total_chars - PRUNE_TAIL_CHARS)
        .collect();
    let hint = if hint_subagent {
        SUBAGENT_HINT_SUFFIX
    } else {
        ""
    };
    Some(format!(
        "{head}\n[结果过长已截断：{} 共 {} 字符，中间省略约 {} 字符。截断后内容不可从历史恢复；如需完整输出请缩小范围重试该工具（如指定更精确的路径/模式或 offset/limit）。{}]\n...{tail}",
        tool_name, total_chars, omitted, hint
    ))
}

#[cfg(test)]
mod tests;
