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
    prune_impl(
        result,
        tool_name,
        hint_subagent,
        MAX_TOOL_RESULT_INLINE_CHARS,
        PRUNE_HEAD_CHARS,
        PRUNE_TAIL_CHARS,
    )
}

/// B30（2026-11-06 三方对比整改）：cap 参数化形态——`agents.tool_result_cap`
/// 的 dispatch 后整形入口（gate 调用；`cap` 来自 AgentLoop 世代快照读，
/// fresh-read 改键下轮生效）。
///
/// - **0 = 关**（拍板③）：返回 `None`，结果原样放行（跨工具兜底封顶关
///   闭；spill 层不受影响——那是可恢复性机制，不是封顶闸）。
/// - 头/尾预算从 cap 推导：`cap` 足够大（≥ 头尾常量 + marker 预算）时
///   与缺省形态逐字节同形（头尾各 3600）；cap 更小时按预算收缩，保证
///   输出 ≤ cap（诚实注记始终在场；cap 小到连注记都装不下时注记优先
///   ——诚实性高于字节预算）。
/// - 重算路径（types.rs 纯投影）保持 [`prune_tool_result`] 的 const 缺省
///   口径不变——gate 侧裁剪结果与重算**同构**时（缺省 cap 下 >8192 深超限）
///   不记投影（X1 纯重算照常，[`cap_prune_matches_recompute`] 判定），
///   不同构时（灰区 ≤8192 / 头尾预算收缩）以 `tool_result_projection`
///   记录（cap 是 dispatch 时点状态，不可由 history 状态重导出），X1
///   重放字节稳定不受本键影响。
pub fn prune_tool_result_with_cap(
    result: &str,
    tool_name: &str,
    hint_subagent: bool,
    cap: usize,
) -> Option<String> {
    if cap == 0 {
        return None;
    }
    let half = cap_head_budget(cap, hint_subagent);
    prune_impl(result, tool_name, hint_subagent, cap, half, half)
}

/// cap → 头/尾预算（[`prune_tool_result_with_cap`] 与
/// [`cap_prune_matches_recompute`] 单源，两处不得漂移；`reserve` = marker
/// 固定文案上界 + tool_name/数字余量 + 可选 spawn 提示）。头尾对半吃剩余
/// 预算，封顶 3600（与缺省形态同形）。
fn cap_head_budget(cap: usize, hint_subagent: bool) -> usize {
    let reserve = 256
        + if hint_subagent {
            SUBAGENT_HINT_SUFFIX.chars().count()
        } else {
            0
        };
    ((cap.saturating_sub(reserve)) / 2).min(PRUNE_HEAD_CHARS)
}

/// B30：gate 侧 cap 裁剪（unhinted 形态）产物是否与 types.rs 纯重算路径
/// （[`prune_tool_result`]，const 8192/3600/3600）**同构**。
///
/// - `cap=0` 或 `total_chars ≤ cap` → gate 未裁，走重算即所见 → `true`；
/// - gate 已裁时：输出逐字节同构 ⟺ 头尾预算未收缩（half==3600）**且**
///   重算也裁（total > 8192——否则重算返回 `None`，全文泄漏）。
///
/// 同构 ⇒ gate 不记 `tool_result_projection`（X1 纯重算语义照常，ledger
/// 面对老会话/replay 零漂移）；不同构 ⇒ gate 必须记录（cap 是 dispatch
/// 时点状态，不可由 history 重导出）。hinted 形态不适用（调用方
/// `prune_hinted` 恒记）。
pub fn cap_prune_matches_recompute(cap: usize, total_chars: usize) -> bool {
    if cap == 0 || total_chars <= cap {
        return true;
    }
    let half = cap_head_budget(cap, false);
    half == PRUNE_HEAD_CHARS && total_chars > MAX_TOOL_RESULT_INLINE_CHARS
}

/// 共享实现（缺省入口与 cap 参数化入口同源；`head`/`tail` 由调用方按
/// 各自预算给出，其余行为逐字节一致）。
fn prune_impl(
    result: &str,
    tool_name: &str,
    hint_subagent: bool,
    threshold: usize,
    head_chars: usize,
    tail_chars: usize,
) -> Option<String> {
    let total_chars = result.chars().count();
    if total_chars <= threshold {
        return None;
    }
    let omitted = total_chars - head_chars - tail_chars;
    let head: String = result.chars().take(head_chars).collect();
    let tail: String = result.chars().skip(total_chars - tail_chars).collect();
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
