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
