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

    /// ⑥ Escalation: at 5 cumulative identical failures there's only a nudge;
    /// at 6 the hard-stop fires.
    #[test]
    fn alternating_loop_escalates_after_hard_stop_threshold() {
        let mut g = TurnGuard::new();
        let err = "Error: build failed: unresolved import";
        // Failures 1-5: no escalation (nudge fires from 3, tested elsewhere).
        for _ in 0..5 {
            g.record_tool_outcome("exec", Some(err));
            assert!(
                g.escalation_check().is_none(),
                "no escalation before hard-stop threshold"
            );
        }
        // 6th failure → escalation fires.
        g.record_tool_outcome("exec", Some(err));
        let stop = g.escalation_check().expect("escalation at 6th failure");
        assert!(stop.contains("exec"));
        assert!(stop.contains("无法打破"));
    }

    /// ⑥ Escalation survives intervening successes — the cumulative counter
    /// (unlike storm's consecutive counter) is NOT reset, so an alternating
    /// edit(ok)→build(fail) loop still escalates.
    #[test]
    fn escalation_survives_intervening_successes() {
        let mut g = TurnGuard::new();
        let err = "Error: timeout";
        for _ in 0..6 {
            g.record_tool_outcome("exec", Some(err));
            g.record_tool_outcome("edit_file", None); // success, does not reset ⑥
        }
        assert!(g.escalation_check().is_some());
    }

    #[test]
    fn signature_strips_error_prefix() {
        // "Error:" and "Tool error:" prefixes should not leak into the sig
        // (so a retry-emitted error and the original compare equal).
        let s1 = error_signature("exec", "Error: boom: details");
        let s2 = error_signature("exec", "boom: details");
        assert_eq!(s1, s2);
    }

    #[test]
    fn indicates_error_detects_exit_code_prefix() {
        use super::tool_result_indicates_error;
        assert!(tool_result_indicates_error("Error: boom"));
        assert!(tool_result_indicates_error("Tool error: bad args"));
        // ExecTool non-zero-exit format (the original stuck case).
        assert!(tool_result_indicates_error(
            "Exit code: 101\nstdout: \nstderr: error[E0432]: x"
        ));
        // Genuine success.
        assert!(!tool_result_indicates_error("compilation successful"));
        assert!(!tool_result_indicates_error("File edited: /a/b.rs"));
    }

    #[test]
    fn signature_skips_exec_boilerplate_to_actual_error() {
        // Two DIFFERENT build errors must get distinct signatures so that
        // fix-one-expose-next progress is not a false-positive loop. The sig
        // skips "Exit code:" / "stdout:" / "stderr:" headers.
        let s1 = error_signature(
            "exec",
            "Exit code: 101\nstdout: \nstderr: error[E0432]: unresolved import `winapi::user32`",
        );
        let s2 = error_signature(
            "exec",
            "Exit code: 101\nstdout: \nstderr: error[E0425]: cannot find function `MessageBoxA`",
        );
        assert_ne!(s1, s2, "distinct build errors must not collide");
        assert!(s1.contains("E0432"));
        assert!(s2.contains("E0425"));
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

    /// ④ Storm fires on the Nth CONSECUTIVE identical failure, and the nudge is
    /// preferred over the ⑥ alternating nudge when both apply.
    #[test]
    fn storm_fires_on_consecutive_identical_failure() {
        let mut g = TurnGuard::new();
        let err = "Error: connection refused";
        assert!(g.record_tool_outcome("exec", Some(err)).is_none()); // storm 1
        assert!(g.record_tool_outcome("exec", Some(err)).is_none()); // storm 2
        let nudge = g.record_tool_outcome("exec", Some(err));        // storm 3
        assert!(nudge.is_some());
        // Storm nudge mentions "连续" (consecutive), the alternating one does not.
        assert!(nudge.unwrap().contains("连续"));
    }

    /// ④ A success between failures resets the storm run (consecutive broken),
    /// but NOT the ⑥ cumulative counter.
    #[test]
    fn storm_resets_on_success_but_alternating_does_not() {
        let mut g = TurnGuard::new();
        let err = "Error: timeout";
        assert!(g.record_tool_outcome("exec", Some(err)).is_none()); // alt 1, storm 1
        assert!(g.record_tool_outcome("exec", Some(err)).is_none()); // alt 2, storm 2
        assert!(g.record_tool_outcome("exec", None).is_none());      // success → storm reset, alt stays 2
        // 3rd cumulative failure (storm run is only 1 long now after the reset):
        // alt hits 3 → nudge, but storm does not fire.
        let nudge = g.record_tool_outcome("exec", Some(err));        // alt 3, storm 1
        assert!(nudge.is_some());
        // Storm didn't fire (run only 1 long), so the nudge is the alternating
        // one — no "连续" wording.
        assert!(!nudge.unwrap().contains("连续"));
    }

    /// ⑤ Repeat-success: a write-like tool succeeding with identical args
    /// nudges past the threshold; non-write tools are ignored.
    #[test]
    fn repeat_success_nudges_on_identical_writes() {
        let mut g = TurnGuard::new();
        let args = r#"{"path":"a.rs","new_text":"x"}"#;
        // Threshold is 2 → allowed counts are 1 and 2; the 3rd nudges.
        assert!(g.record_write_success("edit_file", args).is_none());
        assert!(g.record_write_success("edit_file", args).is_none());
        let nudge = g.record_write_success("edit_file", args);
        assert!(nudge.is_some());
        assert!(nudge.unwrap().contains("edit_file"));

        // Non-write tools never nudge.
        for _ in 0..5 {
            assert!(g.record_write_success("read_file", args).is_none());
        }
    }

    /// ⑤ Whitespace-only differences in args do not bypass the guard.
    #[test]
    fn repeat_success_canonicalizes_whitespace() {
        let mut g = TurnGuard::new();
        assert!(g.record_write_success("write_file", "{ \"a\": 1 }").is_none());
        assert!(g.record_write_success("write_file", "{\"a\":1}").is_none());
        // Third call (semantically identical) nudges.
        assert!(g
            .record_write_success("write_file", " {\"a\": 1} ")
            .is_some());
    }

    /// ⑧ Two near-identical consecutive contents nudge; a different content resets.
    #[test]
    fn text_repetition_nudges_then_resets() {
        let mut g = TurnGuard::new();
        let a = "我已经检查了文件，发现问题是依赖配置不对，需要修改 Cargo.toml。";
        // First round: establishes baseline, no nudge.
        assert!(g.check_text_repetition(a).is_none());
        // Second round: near-identical → nudge.
        let a2 = "我已经检查了文件，发现问题是依赖配置不对，需要修改 Cargo.toml 哦。";
        assert!(g.check_text_repetition(a2).is_some());
        // A clearly different content resets the streak (no nudge).
        assert!(g.check_text_repetition("完全不同的一句新内容，开始新任务。").is_none());
    }

    /// ⑧ Empty content is ignored (does not establish a baseline).
    #[test]
    fn text_repetition_ignores_empty() {
        let mut g = TurnGuard::new();
        assert!(g.check_text_repetition("").is_none());
        assert!(g.check_text_repetition("   ").is_none());
        // No baseline was set, so the first real content does not nudge.
        assert!(g.check_text_repetition("hello world").is_none());
    }

    /// ⑧ similarity sanity checks.
    #[test]
    fn similarity_basics() {
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity("abcdefg", "abcdefg"), 1.0);
        assert!(similarity("completely different content one", "totally unrelated text two") < 0.4);
    }
}
