//! Model-free tool-result pruning (U3, dsh-alignment first batch).
//!
//! Oversized plain-text tool results are replaced, before they are fed back
//! into the conversation, with a bounded head + omission marker + bounded
//! tail. This is the cheap first gate before the expensive LLM summarization
//! tier: a giant `grep`/`exec` output no longer blows up the context window
//! and the summary request that follows it.
//!
//! Trade-off vs dsh (documented deliberately): dsh keeps the FULL original in
//! an append-only event log and only rewrites the projection. After S1-S7 we
//! have no separate projection layer — the history IS the storage — so the
//! pruned form is what history keeps. Consequence: the elided middle is not
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

/// Prune an oversized tool result to head + marker + tail. Char-based (not
/// byte-based) so multi-byte text can never panic on a slice boundary — see
/// the `str-slice-multibyte-panic` incident class. `tool_name` is included in
/// the marker so the model knows which call produced the elision.
///
/// Returns the pruned string, or `None` when the result is within budget
/// (caller keeps the original).
pub fn prune_tool_result(result: &str, tool_name: &str) -> Option<String> {
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
    Some(format!(
        "{head}\n[结果过长已截断：{} 共 {} 字符，中间省略约 {} 字符。截断后内容不可从历史恢复；如需完整输出请缩小范围重试该工具（如指定更精确的路径/模式或 offset/limit）。]\n...{tail}",
        tool_name, total_chars, omitted
    ))
}

#[cfg(test)]
mod tests;
