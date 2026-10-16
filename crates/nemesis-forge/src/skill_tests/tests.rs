//! skill_tests 单测：schema 解析（合法/非法/超限）、纯断言求值、judge 解析。

use super::*;

fn case_with(expect: TestExpectations) -> TestCase {
    TestCase {
        name: "t".into(),
        turns: vec![TestTurn {
            user: "do it".into(),
        }],
        fixtures: vec![],
        expect,
        judge: None,
    }
}

fn transcript_ok() -> ExecutionTranscript {
    ExecutionTranscript {
        turns_executed: 2,
        duration_secs: 10,
        tools_called: vec!["present_plan".into(), "read_file".into()],
        finals: vec!["这是重构计划，请确认".into()],
        errors: vec![],
        files: vec!["plan/方案.md".into()],
    }
}

// ---- 解析 ----

#[test]
fn parse_no_tests_section_returns_none() {
    let content = "---\nname: demo\ndescription: d\n---\nbody";
    assert!(parse_skill_tests(content).unwrap().is_none());
}

#[test]
fn parse_no_frontmatter_returns_none() {
    assert!(parse_skill_tests("just body, no fm").unwrap().is_none());
}

#[test]
fn parse_valid_spec() {
    // tests 段必须在 frontmatter 围栏**内部**（闭合 --- 之前）。
    let content = "---\nname: demo\ndescription: demo skill\ntests:\n  - name: 计划先行\n    turns:\n      - user: 重构登录\n    expect:\n      tools_called: [present_plan]\n      max_llm_turns: 4\n---\nbody";
    let spec = parse_skill_tests(content).unwrap().expect("some");
    assert_eq!(spec.tests.len(), 1);
    assert_eq!(spec.tests[0].expect.tools_called, vec!["present_plan"]);
    assert_eq!(spec.tests[0].expect.max_llm_turns, Some(4));
}

#[test]
fn parse_malformed_tests_section_is_err() {
    let content = "---\nname: d\ntests: \"not-a-list\"\n---\nbody";
    assert!(parse_skill_tests(content).is_err());
}

#[test]
fn parse_rejects_unknown_expect_fields() {
    // deny_unknown_fields：拼错的断言键必须 loud 报错，不能静默吞掉。
    let content =
        "---\nname: d\ntests:\n  - name: t\n    expect:\n      contains_typo: [x]\n---\nbody";
    assert!(parse_skill_tests(content).is_err());
}

#[test]
fn parse_rejects_case_without_any_assertion() {
    let content = "---\nname: d\ntests:\n  - name: t\n    turns:\n      - user: x\n---\nbody";
    let err = parse_skill_tests(content).unwrap_err();
    assert!(err.contains("no assertions"), "{err}");
}

#[test]
fn parse_rejects_turn_overflow() {
    let mut turns = String::from("    turns:\n");
    for i in 0..7 {
        turns.push_str(&format!("      - user: m{i}\n"));
    }
    let content = format!(
        "---\nname: d\ntests:\n  - name: t\n{turns}    expect:\n      max_llm_turns: 3\n---\nbody"
    );
    let err = parse_skill_tests(&content).unwrap_err();
    assert!(err.contains("turns"), "{err}");
}

#[test]
fn parse_rejects_oversized_fixture() {
    let big = "x".repeat(FIXTURE_MAX_BYTES + 1);
    let content = format!(
        "---\nname: d\ntests:\n  - name: t\n    fixtures:\n      - path: a.txt\n        content: {big}\n    expect:\n      max_llm_turns: 1\n---\nbody"
    );
    let err = parse_skill_tests(&content).unwrap_err();
    assert!(err.contains("exceeds"), "{err}");
}

#[test]
fn parse_rejects_fixture_path_traversal() {
    let content = "---\nname: d\ntests:\n  - name: t\n    fixtures:\n      - path: ../evil.txt\n        content: x\n    expect:\n      max_llm_turns: 1\n---\nbody";
    let err = parse_skill_tests(content).unwrap_err();
    assert!(err.contains("invalid fixture path"), "{err}");
}

#[test]
fn parse_rejects_fixture_path_windows_drive_and_root_forms() {
    // R18 收紧的拒绝面：`C:evil` 盘符相对形态 is_absolute()==false 且无
    // `..`，但 join 整段换基座（夹具写出沙盒）；绝对路径与根相对
    //（`\x`、`/x`）同理拒。
    for bad in ["C:evil.txt", "/abs.txt", "\\root.txt", "a/../b.txt"] {
        let content = format!(
            "---\nname: d\ntests:\n  - name: t\n    fixtures:\n      - path: {bad}\n        content: x\n    expect:\n      max_llm_turns: 1\n---\nbody"
        );
        let err = parse_skill_tests(&content).unwrap_err();
        assert!(err.contains("invalid fixture path"), "{bad}: {err}");
    }
    // 合法相对段不受影响。
    let ok = "---\nname: d\ntests:\n  - name: t\n    fixtures:\n      - path: data/input.txt\n        content: x\n    expect:\n      max_llm_turns: 1\n---\nbody";
    assert!(parse_skill_tests(ok).is_ok());
}

#[test]
fn parse_judge_threshold_optional_and_range_checked() {
    // threshold 缺省 = None（执行层配置默认注入）。
    let content = "---\nname: d\ntests:\n  - name: t\n    judge:\n      rubric: 判据\n    expect:\n      max_llm_turns: 1\n---\nbody";
    let spec = parse_skill_tests(content).unwrap().expect("some");
    assert_eq!(spec.tests[0].judge.as_ref().unwrap().threshold, None);
    // 显式超范围 = 解析错误。
    let bad = "---\nname: d\ntests:\n  - name: t\n    judge:\n      rubric: 判据\n      threshold: 1.5\n    expect:\n      max_llm_turns: 1\n---\nbody";
    let err = parse_skill_tests(bad).unwrap_err();
    assert!(err.contains("outside [0,1]"), "{err}");
}

// ---- 求值 ----

#[test]
fn evaluate_passes_when_all_assertions_hold() {
    let case = case_with(TestExpectations {
        tools_called: vec!["present_plan".into()],
        tools_forbidden: vec!["write_file".into()],
        contains: vec!["重构计划".into()],
        not_contains: vec!["已经完成".into()],
        files_exist: vec!["plan/方案.md".into()],
        max_llm_turns: Some(3),
        max_duration_secs: Some(60),
        ..Default::default()
    });
    let r = evaluate_case(&case, &transcript_ok());
    assert!(r.passed, "failures: {:?}", r.failures);
    assert!(r.failures.is_empty());
}

#[test]
fn evaluate_fails_each_violation_with_detail() {
    let case = case_with(TestExpectations {
        tools_called: vec!["missing_tool".into()],
        tools_forbidden: vec!["read_file".into()],
        contains: vec!["不存在串".into()],
        not_contains: vec!["重构计划".into()],
        max_llm_turns: Some(1),
        max_duration_secs: Some(5),
        ..Default::default()
    });
    let r = evaluate_case(&case, &transcript_ok());
    assert!(!r.passed);
    assert_eq!(r.failures.len(), 6, "{:?}", r.failures);
    assert!(r.failures.iter().any(|f| f.contains("tools_called")));
    assert!(r.failures.iter().any(|f| f.contains("tools_forbidden")));
    assert!(r.failures.iter().any(|f| f.contains("contains")));
    assert!(r.failures.iter().any(|f| f.contains("not_contains")));
    assert!(r.failures.iter().any(|f| f.contains("max_llm_turns")));
    assert!(r.failures.iter().any(|f| f.contains("max_duration_secs")));
}

#[test]
fn evaluate_execution_errors_fail_the_case() {
    let case = case_with(TestExpectations::default());
    let mut t = transcript_ok();
    t.errors = vec!["spawn failed".into()];
    let r = evaluate_case(&case, &t);
    assert!(!r.passed);
    assert!(r.failures[0].contains("execution error"));
}

#[test]
fn evaluate_regex_assertion() {
    let case = case_with(TestExpectations {
        matches_regex: vec!["重构.{0,20}确认".into()],
        ..Default::default()
    });
    assert!(evaluate_case(&case, &transcript_ok()).passed);
    let case_bad = case_with(TestExpectations {
        matches_regex: vec!["不存在的模式xyz".into()],
        ..Default::default()
    });
    assert!(!evaluate_case(&case_bad, &transcript_ok()).passed);
}

// ---- judge ----

#[test]
fn judge_prompt_contains_rubric_and_transcript() {
    let mut case = case_with(TestExpectations::default());
    case.judge = Some(TestJudge {
        rubric: "应给出方案".into(),
        threshold: Some(0.7),
    });
    let p = judge_prompt(&case, &transcript_ok());
    assert!(p.contains("应给出方案"));
    assert!(p.contains("present_plan"));
    assert!(p.contains("score"));
}

#[test]
fn parse_judge_response_valid() {
    let o = parse_judge_response(r#"{"score": 0.8, "summary": "好"}"#, 0.7).unwrap();
    assert!(o.passed);
    assert_eq!(o.score, 0.8);
    assert_eq!(o.summary, "好");
}

#[test]
fn parse_judge_response_below_threshold_fails() {
    let o = parse_judge_response(r#"{"score": 0.5, "summary": ""}"#, 0.7).unwrap();
    assert!(!o.passed);
}

#[test]
fn parse_judge_response_garbage_is_err() {
    assert!(parse_judge_response("no json here", 0.7).is_err());
    assert!(parse_judge_response(r#"{"score": "high"}"#, 0.7).is_err());
    assert!(parse_judge_response(r#"{"score": 1.5}"#, 0.7).is_err());
}
