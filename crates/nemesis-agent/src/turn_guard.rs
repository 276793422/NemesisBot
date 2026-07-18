//! Turn-scoped guards that detect a stuck / no-progress agent loop and nudge
//! the model toward recovery *before* the hard `max_turns` backstop fires.
//!
//! All state here is per-`process_message` (one user turn): construct a fresh
//! [`TurnGuard`] at the top of each request. Nothing carries across requests.
//!
//! Design principle: signatures key on `(tool, error)` — **not** on arguments.
//! A stuck model reworks the arguments cosmetically while failing identically,
//! so arg-based matching misses the loop. See the plan doc
//! `docs/PLAN/2026-07-06_agent-loop-turn-management-and-stuck-detection.md`
//! (§4⑥) for the rationale.

use std::collections::{HashMap, HashSet};

/// ⑥ Per-turn threshold for the alternating-loop guard. When the same
/// `(tool, error_signature)` fails this many times within one user turn —
/// counting **across intervening successes**, unlike a consecutive-only storm
/// counter — we nudge the model to change approach. This is what catches
/// `edit(ok) → build(fail) → edit(ok) → build(fail)` loops.
pub const ALTERNATING_LOOP_THRESHOLD: u32 = 3;

/// ⑥ Escalation: when the same `(tool, error_signature)` fails this many times
/// in a turn we stop hard — nudges are being ignored. Bounds the worst-case
/// cost of a stuck model that would otherwise run all the way to `max_turns`.
/// 2× the nudge threshold, so the model gets three nudged chances to recover
/// (failures 3, 4, 5) before we cut it off at failure 6.
pub const ALTERNATING_LOOP_HARD_STOP: u32 = 6;

/// ④ Per-turn threshold for the storm guard: the same `(tool, error)` failing
/// this many times **consecutively** (no intervening success or different
/// error) is a death-spiral — nudge. Lower priority than ⑥ (consecutive is a
/// subset of cumulative), but the nudge is more specific ("no progress at all"
/// vs "root cause elsewhere"), so both run.
pub const STORM_THRESHOLD: u32 = 3;

/// ⑤ Per-turn threshold for the repeat-success guard: a write-like tool
/// succeeding this many times with identical args within one turn is a no-op /
/// write loop — nudge. (Post-execution nudge, not pre-execution block: the
/// write is idempotent, the point is to break the loop.)
pub const REPEAT_SUCCESS_THRESHOLD: u32 = 2;

/// ⑧ Similarity (4-gram Jaccard) at which two consecutive response contents
/// count as a repeat.
pub const TEXT_REPETITION_SIM_THRESHOLD: f64 = 0.8;

/// ⑦ Max degenerate (empty / whitespace-only / reasoning-only) final answers
/// before giving up. Below this, retry with a nudge.
pub const MAX_EMPTY_FINAL_RETRIES: u32 = 3;

/// ⑤ Tools whose success is "write-like" — repeating an identical successful
/// call is almost always a no-op loop. Conservative list; extend as needed.
const WRITE_LIKE_TOOLS: &[&str] = &[
    "edit_file", "write_file", "create_file", "save_file", "patch_file", "multi_edit",
];

/// ⑦ Verdict from checking a candidate final answer.
#[derive(Debug)]
pub enum FinalAnswerVerdict {
    /// Answer has visible content — accept as the final response.
    Accept,
    /// Degenerate answer — inject this nudge and let the model retry.
    RetryWithNudge(String),
    /// Degenerate-answer retry budget exhausted — finalize with this notice.
    GiveUp(String),
}

/// Per-turn guard state. Construct fresh per request; no cross-request state.
#[derive(Default)]
pub struct TurnGuard {
    /// ⑥ `(tool, error_signature) → cumulative failure count this turn`.
    /// Deliberately NOT reset on intervening successes — that is the whole
    /// point: it catches alternating success/fail loops that a consecutive-only
    /// counter misses.
    fail_freq: HashMap<String, u32>,
    /// ④ Storm: the last consecutive `(tool, error)` signature and its run
    /// length. Reset on any success or a different error signature.
    storm_sig: Option<String>,
    storm_count: u32,
    /// ⑤ `(write tool, canonical args) → success count this turn`.
    success_counts: HashMap<String, u32>,
    /// ⑦ Consecutive degenerate final answers this turn.
    empty_final_count: u32,
    /// ⑧ Last non-empty response content, for cross-round prose repetition.
    last_content: Option<String>,
    /// ⑧ Consecutive similar-content rounds this turn.
    repeat_text_count: u32,
}

impl TurnGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// ④/⑥ Record a tool outcome. On failure, increment BOTH:
    /// - ⑥ alternating: per-turn cumulative count (never reset by success);
    /// - ④ storm: consecutive-run count (reset on any success or different
    ///   error signature).
    /// When either reaches its threshold, return a nudge to append to the tool
    /// result fed back to the model. The ④ storm nudge (more specific: "no
    /// progress at all") is preferred when both fire. Returns `None` for
    /// successes and below-threshold failures.
    ///
    /// `error` should be `None` on success, or the error text on failure
    /// (typically the tool result string starting with `Error:` / `Tool error:`).
    pub fn record_tool_outcome(&mut self, tool: &str, error: Option<&str>) -> Option<String> {
        let error = match error {
            None => {
                // Success: ④ storm run is broken. ⑥ fail_freq is intentionally
                // NOT reset (that is what catches alternating loops).
                self.storm_sig = None;
                self.storm_count = 0;
                return None;
            }
            Some(e) => e,
        };
        let sig = error_signature(tool, error);

        // ⑥ Alternating — cumulative across the whole turn.
        let alt_count = self.fail_freq.entry(sig.clone()).or_insert(0);
        *alt_count += 1;
        let alt_nudge = (*alt_count >= ALTERNATING_LOOP_THRESHOLD).then(|| {
            format!(
                "\n[loop guard] {} 在本任务中已 {} 次报相同错误。中间的修改并没有消除这个错误——根因可能在别处。请换方向：检查依赖配置 / 构建配置 / 环境依赖是否正确安装，或换一条完全不同的实现路径；若实在无法解决，请在最终答复里说明阻塞点。",
                tool, *alt_count
            )
        });

        // ④ Storm — consecutive identical failure only.
        let storm_nudge = if Some(&sig) == self.storm_sig.as_ref() {
            self.storm_count += 1;
            if self.storm_count >= STORM_THRESHOLD {
                Some(format!(
                    "\n[loop guard] {} 已连续 {} 次以完全相同的方式失败，且中间没有任何其他有效操作。换个说法重发同样没用。请换工具、换思路，或在最终答复里说明阻塞点。",
                    tool, self.storm_count
                ))
            } else {
                None
            }
        } else {
            self.storm_sig = Some(sig);
            self.storm_count = 1;
            None
        };

        storm_nudge.or(alt_nudge)
    }

    /// ⑥ Escalation: returns a hard-stop message if any single `(tool, error)`
    /// has failed at least [`ALTERNATING_LOOP_HARD_STOP`] times this turn — the
    /// model is ignoring the nudges, so stop the turn to avoid burning the whole
    /// `max_turns` budget. The caller breaks the loop and surfaces the message.
    pub fn escalation_check(&self) -> Option<String> {
        self.fail_freq.iter().find_map(|(sig, count)| {
            if *count >= ALTERNATING_LOOP_HARD_STOP {
                let tool = sig.split('\x00').next().unwrap_or("tool");
                Some(format!(
                    "检测到循环无法打破：{} 在本任务中已 {} 次报相同错误，多次提示后仍未改变方向。已停止本轮以避免空耗，已完成的工作已保存。请人工介入，或换一种思路后重试。",
                    tool, count
                ))
            } else {
                None
            }
        })
    }

    /// ⑤ Record a successful write-like tool call; returns a nudge if this
    /// exact `(tool, args)` has now succeeded more than `REPEAT_SUCCESS_THRESHOLD`
    /// times this turn (a no-op / write loop). Call AFTER the tool succeeds;
    /// the caller appends the nudge to the tool result. Non-write tools are
    /// ignored.
    pub fn record_write_success(&mut self, tool: &str, args: &str) -> Option<String> {
        if !is_write_like_tool(tool) {
            return None;
        }
        let sig = format!("{}\x00{}", tool, canonical_args(args));
        let count = self.success_counts.entry(sig).or_insert(0);
        *count += 1;
        if *count > REPEAT_SUCCESS_THRESHOLD {
            Some(format!(
                "\n[loop guard] {} 已在本任务中以相同参数成功 {} 次。重复同样的写操作没有意义。请确认结果（读回 / 测试），或换下一步操作，或在最终答复里收尾。",
                tool, *count
            ))
        } else {
            None
        }
    }

    /// ⑧ Cross-round prose repetition. Returns a nudge when the model's
    /// response content is near-identical (≥ [`TEXT_REPETITION_SIM_THRESHOLD`])
    /// to the previous round's content — a "saying the same thing while
    /// churning tools" loop. Empty content is ignored ([check_final_answer]
    /// owns that) and does not update the baseline.
    pub fn check_text_repetition(&mut self, content: &str) -> Option<String> {
        if content.trim().is_empty() {
            return None;
        }
        let similar = self
            .last_content
            .as_ref()
            .map(|prev| similarity(prev, content) >= TEXT_REPETITION_SIM_THRESHOLD)
            .unwrap_or(false);
        if similar {
            self.repeat_text_count += 1;
            // First similar round sets count to 1 and just records the baseline;
            // the SECOND consecutive similar round (count ≥ 1 here means we've
            // now seen two in a row) nudges.
            self.last_content = Some(content.to_string());
            if self.repeat_text_count >= 1 {
                return Some(format!(
                    "\n[loop guard] 你连续两轮给出了几乎相同的内容（相似度 ≥{:.0}%）。请给出新信息、新角度，或直接收尾。",
                    TEXT_REPETITION_SIM_THRESHOLD * 100.0
                ));
            }
        } else {
            self.repeat_text_count = 0;
            self.last_content = Some(content.to_string());
        }
        None
    }

    /// ⑦ Check a candidate final answer. Empty / whitespace-only `content` is
    /// degenerate (this also covers reasoning-only answers, since reasoning
    /// lives in a separate field from `content`).
    pub fn check_final_answer(&mut self, text: &str) -> FinalAnswerVerdict {
        if !text.trim().is_empty() {
            return FinalAnswerVerdict::Accept;
        }
        self.empty_final_count += 1;
        if self.empty_final_count >= MAX_EMPTY_FINAL_RETRIES {
            FinalAnswerVerdict::GiveUp(
                "（模型多次未给出有效答复，已停止重试。请重试或换一种问法。）".to_string(),
            )
        } else {
            FinalAnswerVerdict::RetryWithNudge(
                "你的上一条回复没有可见正文。请直接给出实际答复。".to_string(),
            )
        }
    }
}

/// Build a stable signature for a tool failure. Keys on `(tool, first error
/// line)` — NOT on arguments. The first line is usually the discriminator
/// (e.g. `error[E0432]: unresolved import ...`); taking the first line and
/// truncating ignores volatile line numbers / surrounding output while still
/// distinguishing genuinely-different errors. Char-bounded truncation so it is
/// safe on multi-byte (e.g. Chinese) text.
/// Whether a tool's result string indicates failure. Tools in this codebase
/// signal errors three ways, and the guards must recognize all of them:
/// - `Error: ...` — framework-wrapped (e.g. unknown tool).
/// - `Tool error: ...` — `handle_tool_call` wrapping a tool's `Err(_)` return.
/// - `Exit code: <N>\n...` — `ExecTool` returns this as `Ok(_)` for non-zero
///   exits. A non-zero exit IS a failure even though it's `Ok`, so without this
///   case the most common loop shape — repeated failing builds — would look like
///   a stream of successes and the guards would never fire.
pub fn tool_result_indicates_error(result: &str) -> bool {
    result.starts_with("Error:")
        || result.starts_with("Tool error:")
        || result.starts_with("Exit code:")
}

/// Build a stable signature for a tool failure. Keys on `(tool, first
/// meaningful error line)` — NOT on arguments. Skips `ExecTool` boilerplate
/// lines (`Exit code:` / `stdout:` / `stderr:` headers) so the signature
/// reflects the actual error — otherwise every build failure keys to
/// "Exit code: 101" and distinct errors (fix-one-expose-next progress) would
/// collide into a false-positive loop signal. Char-bounded truncation so it is
/// safe on multi-byte (e.g. Chinese) text.
fn error_signature(tool: &str, error: &str) -> String {
    let stripped = error
        .trim_start_matches("Error:")
        .trim_start_matches("Tool error:")
        .trim();
    // Skip ExecTool's `Exit code:` / `stdout:` / `stderr:` headers. The latter
    // two are INLINE prefixes on the same line as the content (e.g.
    // "stderr: error[E0432]: ..."), so we strip the prefix and look at the
    // remainder — otherwise every build failure keys to "Exit code: N" and
    // distinct errors collide into a false-positive loop signal.
    let meaningful = stripped
        .lines()
        .filter_map(|l| {
            let s = l
                .trim_start_matches("stdout:")
                .trim_start_matches("stderr:")
                .trim();
            if s.is_empty() || s.starts_with("Exit code:") {
                None
            } else {
                Some(s)
            }
        })
        .next()
        .unwrap_or_else(|| stripped.lines().next().unwrap_or(stripped).trim());
    let normalized: String = meaningful.chars().take(120).collect();
    format!("{}\x00{}", tool, normalized)
}

/// ⑤ Whether a tool's success is "write-like" (repeating an identical
/// successful call is a no-op loop). Conservative allow-list.
fn is_write_like_tool(name: &str) -> bool {
    WRITE_LIKE_TOOLS.iter().any(|t| *t == name)
}

/// ⑤ Canonicalize raw tool args for the repeat-success signature. Parses JSON
/// and re-serializes compactly (serde_json sorts object keys by default), so
/// cosmetically-reformatted but semantically-identical args compare equal.
/// Falls back to whitespace-collapsing for non-JSON args.
fn canonical_args(args: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(args) {
        Ok(v) => v.to_string(),
        Err(_) => args.split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

/// ⑧ Jaccard similarity over char-level `n`-grams. Char-level so it is safe on
/// multi-byte (Chinese) text. Returns 1.0 for two empty inputs.
fn similarity(a: &str, b: &str) -> f64 {
    let sa: HashSet<String> = shingles(a, 4).into_iter().collect();
    let sb: HashSet<String> = shingles(b, 4).into_iter().collect();
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    let union = sa.union(&sb).count();
    if union == 0 {
        return 0.0;
    }
    let inter = sa.intersection(&sb).count();
    inter as f64 / union as f64
}

/// Char-level n-grams (shingles). Strings shorter than `n` produce a single
/// shingle equal to the whole string, so two identical short strings stay
/// similar (1.0) and two different short strings stay dissimilar (0.0).
fn shingles(s: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < n {
        return vec![s.to_string()];
    }
    chars.windows(n).map(|w| w.iter().collect()).collect()
}

#[cfg(test)]
mod tests;
