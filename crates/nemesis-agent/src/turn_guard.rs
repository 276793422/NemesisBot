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

use std::collections::HashMap;

/// ⑥ Per-turn threshold for the alternating-loop guard. When the same
/// `(tool, error_signature)` fails this many times within one user turn —
/// counting **across intervening successes**, unlike a consecutive-only storm
/// counter — we nudge the model to change approach. This is what catches
/// `edit(ok) → build(fail) → edit(ok) → build(fail)` loops.
pub const ALTERNATING_LOOP_THRESHOLD: u32 = 3;

/// ⑦ Max degenerate (empty / whitespace-only / reasoning-only) final answers
/// before giving up. Below this, retry with a nudge.
pub const MAX_EMPTY_FINAL_RETRIES: u32 = 3;

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
    /// ⑥ `(tool, error_signature) → failure count this turn`. Deliberately NOT
    /// reset on intervening successes — that is the whole point: it catches
    /// alternating success/fail loops that a consecutive-only counter misses.
    fail_freq: HashMap<String, u32>,
    /// ⑦ Consecutive degenerate final answers this turn.
    empty_final_count: u32,
}

impl TurnGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// ⑥ Record a tool outcome. On failure, increment the per-turn counter for
    /// `(tool, error_signature)`; once it reaches [`ALTERNATING_LOOP_THRESHOLD`],
    /// return a nudge to append to the tool result fed back to the model.
    /// Returns `None` for successes and below-threshold failures.
    ///
    /// `error` should be `None` on success, or the error text on failure
    /// (typically the tool result string starting with `Error:` / `Tool error:`).
    pub fn record_tool_outcome(&mut self, tool: &str, error: Option<&str>) -> Option<String> {
        let error = error?;
        let sig = error_signature(tool, error);
        let count = self.fail_freq.entry(sig).or_insert(0);
        *count += 1;
        if *count >= ALTERNATING_LOOP_THRESHOLD {
            Some(format!(
                "\n[loop guard] {} 在本任务中已 {} 次报相同错误。中间的修改并没有消除这个错误——根因可能在别处（依赖、配置、环境）。请换方向：检查 Cargo.toml / 配置 / 依赖，或换一条完全不同的实现路径；若实在无法解决，请在最终答复里说明阻塞点。",
                tool, *count
            ))
        } else {
            None
        }
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
fn error_signature(tool: &str, error: &str) -> String {
    let stripped = error
        .trim_start_matches("Error:")
        .trim_start_matches("Tool error:")
        .trim();
    let first_line = stripped.lines().next().unwrap_or(stripped);
    let normalized: String = first_line.chars().take(120).collect();
    format!("{}\x00{}", tool, normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⑥ The signature counter is NOT reset by intervening successes — this is
    /// the exact scenario the plan calls out (edit→build→edit→build).
    #[test]
    fn alternating_loop_accumulates_across_successes() {
        let mut g = TurnGuard::new();
        let err = "Error: error[E0432]: unresolved import `winapi::user32`";
        assert!(g.record_tool_outcome("exec", Some(err)).is_none()); // count 1
        // Intervening success on a DIFFERENT tool must not reset exec's counter.
        assert!(g.record_tool_outcome("edit_file", None).is_none());
        assert!(g.record_tool_outcome("exec", Some(err)).is_none()); // count 2
        // Third identical failure → nudge.
        let nudge = g.record_tool_outcome("exec", Some(err));
        assert!(nudge.is_some());
        let nudge = nudge.unwrap();
        assert!(nudge.contains("loop guard"));
        assert!(nudge.contains("exec"));
    }

    /// ⑥ Genuinely-different errors do NOT accumulate together — fixing one
    /// lint exposes the next, each is its own signature.
    #[test]
    fn alternating_loop_different_errors_dont_accumulate() {
        let mut g = TurnGuard::new();
        assert!(g.record_tool_outcome("exec", Some("Error: error[E0432]: a")).is_none());
        assert!(g.record_tool_outcome("exec", Some("Error: error[E0425]: b")).is_none());
        // E0432 seen again → its own count is 2 (not 3), still no nudge.
        assert!(g.record_tool_outcome("exec", Some("Error: error[E0432]: a")).is_none());
    }

    #[test]
    fn successes_never_nudge() {
        let mut g = TurnGuard::new();
        for _ in 0..10 {
            assert!(g.record_tool_outcome("read_file", None).is_none());
        }
    }

    #[test]
    fn signature_strips_error_prefix() {
        // "Error:" and "Tool error:" prefixes should not leak into the sig
        // (so a retry-emitted error and the original compare equal).
        let s1 = error_signature("exec", "Error: boom: details");
        let s2 = error_signature("exec", "boom: details");
        assert_eq!(s1, s2);
    }

    /// ⑦ Two retries, then give up on the third degenerate answer.
    #[test]
    fn empty_final_answer_retries_then_gives_up() {
        let mut g = TurnGuard::new();
        assert!(matches!(g.check_final_answer(""), FinalAnswerVerdict::RetryWithNudge(_)));
        assert!(matches!(g.check_final_answer("   \n  "), FinalAnswerVerdict::RetryWithNudge(_)));
        assert!(matches!(g.check_final_answer(""), FinalAnswerVerdict::GiveUp(_)));
    }

    #[test]
    fn non_empty_final_answer_accepted() {
        let mut g = TurnGuard::new();
        assert!(matches!(g.check_final_answer("hello"), FinalAnswerVerdict::Accept));
        assert!(matches!(g.check_final_answer("  x  "), FinalAnswerVerdict::Accept));
        // Accept does not consume a retry budget.
        assert!(matches!(g.check_final_answer("again"), FinalAnswerVerdict::Accept));
    }
}
