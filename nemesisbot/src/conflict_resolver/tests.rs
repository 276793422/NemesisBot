//! P5/F5 冲突硬解执行体单测：产出契约校验矩阵（范围钉死/二进制择边/
//! 锁文件理由/覆盖完备/围栏容错）+ prompt 静态断言 + 接触时刻表常量。

use super::*;

fn conflict_text(path: &str) -> ConflictFile {
    ConflictFile {
        path: path.into(),
        binary: false,
        ours: Some(b"ours line\n".to_vec()),
        theirs: Some(b"theirs line\n".to_vec()),
        ancestor: Some(b"base line\n".to_vec()),
    }
}

fn conflict_binary(path: &str) -> ConflictFile {
    ConflictFile {
        path: path.into(),
        binary: true,
        ours: Some(vec![0x01, 0x00, 0x02]),
        theirs: Some(vec![0x03, 0x00, 0x04]),
        ancestor: Some(vec![0x00, 0x00]),
    }
}

#[test]
fn parse_ok_full_coverage() {
    let conflicts = [conflict_text("src/a.rs"), conflict_text("src/b.rs")];
    let raw = r#"{"resolutions": [
        {"path": "src/a.rs", "action": "merge", "content": "merged line\n", "reason": "两侧都保留"},
        {"path": "src/b.rs", "action": "theirs", "reason": "对方改动更完整"}
    ]}"#;
    let out = parse_resolutions(raw, &conflicts).expect("合法产出必须通过");
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].action, ResolutionAction::Merge);
    assert_eq!(out[0].content.as_deref(), Some(b"merged line\n".as_slice()));
    assert_eq!(out[1].action, ResolutionAction::Theirs);
    assert!(
        out[1].content.is_none(),
        "择边不带 content（落盘时从冲突三阶段取）"
    );
}

#[test]
fn parse_ok_strips_code_fence_and_prose() {
    let conflicts = [conflict_text("src/a.rs")];
    let raw = "好的，以下是解决方案：\n```json\n{\"resolutions\": [{\"path\": \"src/a.rs\", \"action\": \"ours\", \"reason\": \"我方为准\"}]}\n```\n请查收。";
    let out = parse_resolutions(raw, &conflicts).expect("围栏+前后杂文必须容错");
    assert_eq!(out.len(), 1);
}

#[test]
fn parse_reject_out_of_scope_path() {
    let conflicts = [conflict_text("src/a.rs")];
    let raw = r#"{"resolutions": [{"path": "src/evil.rs", "action": "merge", "content": "x", "reason": "r"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("越界"), "必须拒绝范围外文件：{err}");
}

#[test]
fn parse_reject_binary_merge() {
    let conflicts = [conflict_binary("assets/logo.bin")];
    let raw = r#"{"resolutions": [{"path": "assets/logo.bin", "action": "merge", "content": "abc", "reason": "r"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("二进制"), "二进制必须择边：{err}");
}

#[test]
fn parse_reject_lockfile_reason_without_regen() {
    let conflicts = [conflict_text("Cargo.lock")];
    let raw =
        r#"{"resolutions": [{"path": "Cargo.lock", "action": "ours", "reason": "保留我方版本"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("建议重新生成"), "锁文件理由闸：{err}");
}

#[test]
fn parse_ok_lockfile_reason_with_regen() {
    let conflicts = [conflict_text("package-lock.json")];
    let raw = r#"{"resolutions": [{"path": "package-lock.json", "action": "theirs", "reason": "采信对方，建议重新生成以保证一致性"}]}"#;
    parse_resolutions(raw, &conflicts).expect("锁文件理由含建议重新生成必须通过");
}

#[test]
fn parse_reject_incomplete_coverage() {
    let conflicts = [conflict_text("src/a.rs"), conflict_text("src/b.rs")];
    let raw = r#"{"resolutions": [{"path": "src/a.rs", "action": "ours", "reason": "r"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(
        err.contains("缺少冲突文件处置"),
        "漏一个冲突文件就是假绿：{err}"
    );
}

#[test]
fn parse_reject_duplicate_path() {
    let conflicts = [conflict_text("src/a.rs")];
    let raw = r#"{"resolutions": [
        {"path": "src/a.rs", "action": "ours", "reason": "r"},
        {"path": "src/a.rs", "action": "theirs", "reason": "r"}
    ]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("重复"), "重复处置必须拒绝：{err}");
}

#[test]
fn parse_reject_side_absent_choice() {
    // 我方侧无文件（对方新增、我方删除型冲突）：择边 ours 不可表达（删除）。
    let conflicts = [ConflictFile {
        path: "src/new.rs".into(),
        binary: false,
        ours: None,
        theirs: Some(b"new\n".to_vec()),
        ancestor: None,
    }];
    let raw = r#"{"resolutions": [{"path": "src/new.rs", "action": "ours", "reason": "r"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("不能择边 ours"), "缺席侧择边必须拒绝：{err}");
}

#[test]
fn parse_reject_merge_without_content() {
    let conflicts = [conflict_text("src/a.rs")];
    let raw = r#"{"resolutions": [{"path": "src/a.rs", "action": "merge", "reason": "r"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("content"), "merge 缺内容必须拒绝：{err}");
}

#[test]
fn parse_reject_garbage() {
    let conflicts = [conflict_text("src/a.rs")];
    assert!(parse_resolutions("完全不是 JSON", &conflicts).is_err());
    assert!(parse_resolutions("{\"resolutions\": \"not-array\"}", &conflicts).is_err());
}

#[test]
fn system_prompt_pins_scope_and_lockfile_rule() {
    // 静态断言：范围钉死 + 锁文件规则 + 二进制择边必须写在 prompt 里
    //（prompt 是行为契约的一部分，改丢规则在这里红）。
    assert!(CONFLICT_RESOLVER_SYSTEM_PROMPT.contains("只允许处置列出的冲突文件"));
    assert!(CONFLICT_RESOLVER_SYSTEM_PROMPT.contains("建议重新生成"));
    assert!(CONFLICT_RESOLVER_SYSTEM_PROMPT.contains("择边"));
    assert!(CONFLICT_RESOLVER_SYSTEM_PROMPT.contains("resolutions"));
}

#[test]
fn probe_schedule_is_t0_60_120() {
    // 三轮接触时刻表钉死（goal 钦定 t0/+60s/+120s；具名常量防手滑改掉）。
    assert_eq!(PROBE_SCHEDULE_SECS, &[0, 60, 120]);
}

#[test]
fn user_prompt_carries_intent_and_three_stages() {
    // 本地构造 Issue（只需 title/description/project_id 参与 prompt 组装）。
    let issue = nemesis_board::Issue {
        id: 9,
        number: "NB-9".into(),
        title: "修登录 bug".into(),
        description: "把登录超时改成 30s".into(),
        status: nemesis_board::IssueStatus::InProgress,
        priority: 2,
        assignee: None,
        assignee_id: None,
        creator: nemesis_board::Actor::system("test"),
        parent_issue_id: None,
        project_id: Some(1),
        due_date: None,
        position: 0,
        acceptance_criteria: None,
        origin: None,
        required_role: None,
        required_tags: vec![],
        hidden: false,
        created_at: 0,
        updated_at: 0,
    };
    let conflicts = [
        conflict_text("src/login.rs"),
        conflict_binary("assets/logo.bin"),
    ];
    let p = build_user_prompt(&issue, "task-1", &conflicts);
    assert!(p.contains("修登录 bug"), "任务意图（标题）必须在场");
    assert!(p.contains("把登录超时改成 30s"), "任务说明必须在场");
    assert!(p.contains("src/login.rs"), "冲突路径必须在场");
    assert!(p.contains("ours line"), "我方内容必须在场");
    assert!(p.contains("theirs line"), "对方内容必须在场");
    assert!(p.contains("base line"), "基线内容必须在场");
    assert!(p.contains("assets/logo.bin"), "二进制冲突路径必须在场");
    assert!(
        p.contains("二进制文件"),
        "二进制择边提示语义在场（二进制条目渲染）"
    );
}
