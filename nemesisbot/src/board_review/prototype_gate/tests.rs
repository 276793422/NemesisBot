//! 触点体系 P3b：原型机器行为闸纯函数面单测（出题解析 / 任务提示词 /
//! 报告裁决 / 报告摘要 / 里程碑目标判定）。
//!
//! 闸的 I/O 链路（派发/定案/重派/熔断/sweep）由 cluster-uat 端到端覆盖；
//! 此处钉死解析与裁决的全部边界组合，防报告契约回归。

use super::*;

// ---------- parse_playtest_quests ----------

#[test]
fn quests_parse_valid_scenarios() {
    let raw = r#"{"scenarios":[
        {"id":"S1","name":"开局","steps":["点击开始","等待加载"],"expect":"进入主城"},
        {"id":"S2","name":"建造","steps":["选空地","点民居"],"expect":"人口+1"}
    ]}"#;
    let quests = parse_playtest_quests(raw).expect("parse ok");
    assert_eq!(quests.len(), 2);
    assert_eq!(quests[0].id, "S1");
    assert_eq!(quests[0].steps.len(), 2);
    assert_eq!(quests[1].expect, "人口+1");
}

#[test]
fn quests_parse_rejects_non_json_and_missing_array() {
    assert!(parse_playtest_quests("不是 JSON").is_err());
    assert!(parse_playtest_quests(r#"{"items":[]}"#).is_err());
    assert!(parse_playtest_quests(r#"{"scenarios":[]}"#).is_err());
}

#[test]
fn quests_parse_rejects_over_limit_and_empty_id() {
    let mut s = String::from(r#"{"scenarios":["#);
    for i in 0..9 {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            r#"{{"id":"S{i}","name":"n","steps":[],"expect":"e"}}"#
        ));
    }
    s.push_str("]}");
    let err = parse_playtest_quests(&s).unwrap_err();
    assert!(err.contains("超上限"), "got: {err}");
    assert!(parse_playtest_quests(r#"{"scenarios":[{"name":"无id"}]}"#).is_err());
}

#[test]
fn quests_parse_dedups_repeated_ids() {
    let raw = r#"{"scenarios":[
        {"id":"S1","name":"a","steps":[],"expect":""},
        {"id":"S1","name":"b","steps":[],"expect":""},
        {"id":"S2","name":"c","steps":[],"expect":""}
    ]}"#;
    let quests = parse_playtest_quests(raw).expect("parse ok");
    assert_eq!(quests.len(), 2);
    assert_eq!(quests[0].name, "a");
}

// ---------- build_playtest_task_prompt ----------

#[test]
fn task_prompt_carries_marker_scenarios_and_contract() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-prototype-gate-prompt-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("store");
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "原型：城市天际线".into(),
            description: "首个可运行构建".into(),
            acceptance_criteria: Some("能开局、能建造".into()),
            quality: "experience".into(),
            milestone: true,
            ..Default::default()
        })
        .expect("new issue");
    let quests = parse_playtest_quests(
        r#"{"scenarios":[{"id":"S1","name":"开局","steps":["点开始"],"expect":"进主城"}]}"#,
    )
    .unwrap();
    let prompt = build_playtest_task_prompt(&issue, &quests);
    assert!(
        prompt.starts_with(&format!(
            "{}{}]",
            nemesis_cluster::PLAYTEST_PROMPT_MARKER,
            issue.number
        )),
        "marker head 失守：{}",
        &prompt[..80.min(prompt.len())]
    );
    assert!(prompt.contains("S1"), "场景清单缺失");
    assert!(prompt.contains("开局"));
    assert!(prompt.contains("进主城"));
    for key in ["scenarios", "redteam", "screenshots", "console_tail"] {
        assert!(prompt.contains(key), "报告契约缺 {key}");
    }
    assert!(prompt.contains("红队"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- evaluate_playtest_report ----------

#[test]
fn report_all_pass_verdict_pass() {
    let raw = r#"{"scenarios":[
        {"id":"S2","pass":true,"evidence":"ok"},
        {"id":"S1","pass":true,"evidence":"ok"}
    ],"redteam":[{"severity":"high","where":"首页","confusion":"按钮太小"}]}"#;
    assert!(matches!(
        evaluate_playtest_report(raw),
        PlaytestVerdict::Pass
    ));
}

#[test]
fn report_any_fail_verdict_fail_sorted_dedup() {
    let raw = r#"{"scenarios":[
        {"id":"S3","pass":true,"evidence":"ok"},
        {"id":"S2","pass":false,"evidence":"崩溃"},
        {"id":"S1","pass":false,"evidence":"黑屏"},
        {"id":"S2","pass":false,"evidence":"重复行"}
    ]}"#;
    match evaluate_playtest_report(raw) {
        PlaytestVerdict::Fail(ids) => assert_eq!(ids, vec!["S1", "S2"]),
        other => panic!("expect Fail, got {other:?}"),
    }
}

#[test]
fn report_missing_pass_defaults_to_fail() {
    // pass 键缺席 = 未证明通过（fail-closed，不按 pass 处理）。
    let raw = r#"{"scenarios":[{"id":"S1","evidence":"跑完了"}]}"#;
    match evaluate_playtest_report(raw) {
        PlaytestVerdict::Fail(ids) => assert_eq!(ids, vec!["S1"]),
        other => panic!("expect Fail, got {other:?}"),
    }
}

#[test]
fn report_unparseable_is_infra_fail_not_fail() {
    assert!(matches!(
        evaluate_playtest_report("worker 崩溃堆栈"),
        PlaytestVerdict::InfraFail(_)
    ));
    assert!(matches!(
        evaluate_playtest_report(r#"{"redteam":[]}"#),
        PlaytestVerdict::InfraFail(_)
    ));
    assert!(matches!(
        evaluate_playtest_report(r#"{"scenarios":[]}"#),
        PlaytestVerdict::InfraFail(_)
    ));
}

#[test]
fn report_redteam_never_blocks_pass() {
    // 红队发现是触点卡展示材料，不改变机器闸裁决（计划 :36 否决=人工）。
    let raw = r#"{"scenarios":[{"id":"S1","pass":true,"evidence":"ok"}],
        "redteam":[{"severity":"high","where":"x","confusion":"y"}]}"#;
    assert!(matches!(
        evaluate_playtest_report(raw),
        PlaytestVerdict::Pass
    ));
}

// ---------- render_gate_report_brief ----------

#[test]
fn report_brief_lists_scenarios_and_redteam_count() {
    let raw = r#"{"scenarios":[{"id":"S1","pass":true,"evidence":"开局成功"}],
        "redteam":[{"severity":"low","where":"a","confusion":"b"}],
        "console_tail":"listening on :3000"}"#;
    let brief = render_gate_report_brief(raw);
    assert!(brief.contains("S1") && brief.contains("开局成功"));
    assert!(brief.contains("红队发现 1 条"));
    assert!(brief.contains("listening on :3000"));
}

// ---------- is_prototype_gate_target ----------

fn fixture_issue(
    name: &str,
    quality: &str,
    milestone: bool,
) -> (std::path::PathBuf, nemesis_board::Issue) {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-prototype-gate-target-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("store");
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "里程碑单".into(),
            description: "d".into(),
            quality: quality.to_string(),
            milestone,
            ..Default::default()
        })
        .expect("new issue");
    (dir, issue)
}

#[test]
fn gate_target_requires_milestone_and_experience() {
    let (dir, issue) = fixture_issue("yes", "experience", true);
    assert!(is_prototype_gate_target(&issue));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gate_target_rejects_standard_quality_and_non_milestone() {
    let (dir, issue) = fixture_issue("quality", "standard", true);
    assert!(!is_prototype_gate_target(&issue));
    let _ = std::fs::remove_dir_all(&dir);

    let (dir, issue) = fixture_issue("nomilestone", "experience", false);
    assert!(!is_prototype_gate_target(&issue));
    let _ = std::fs::remove_dir_all(&dir);
}

// 宽容口径钉（BUG-20261109-010 漂移修复）：档位判定走
// `is_experience_quality` 单一真相源（trim + 大小写不敏感）——与规格闸/
// milestone 归一同口径；此前裸 `== "experience"` 对 `"Experience"` 档
// 静默落空（挂了规格触点却丢原型闸）。
#[test]
fn gate_target_accepts_experience_case_and_space_variants() {
    for (name, quality) in [
        ("upper", "Experience"),
        ("shout", "EXPERIENCE"),
        ("padded", "  experience  "),
    ] {
        let (dir, issue) = fixture_issue(name, quality, true);
        assert!(
            is_prototype_gate_target(&issue),
            "quality {quality:?} must hit the prototype gate"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
