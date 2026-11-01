//! flow 引擎单测：状态机纯函数、FlowStore 投影、模板校验、取数解析、
//! board 绑定（真 BoardStore 临时库）。引擎本体（tick/publish/report）依赖
//! 常驻 AgentLoop，不在单测面（P4 e2e 覆盖）。

use std::path::PathBuf;
use std::sync::Arc;

use super::*;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn temp_ws(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "flow-engine-tests-{}-{}-{tag}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).expect("temp ws");
    dir
}

fn make_store(ws: &std::path::Path) -> Arc<BoardStore> {
    Arc::new(BoardStore::open(&ws.join("board").join("board.db"), "NB").expect("store"))
}

/// 构造实例：kinds = [(step_id, kind, gate, on_fail)]。
fn inst_with_steps(template: &str, kinds: &[(&str, &str, &str, &str)]) -> FlowInstance {
    let now = now_ms();
    FlowInstance {
        template: template.to_string(),
        instance: "I-1".to_string(),
        ext_key: "EXT-1".to_string(),
        title: "样例实例".to_string(),
        current_step: 0,
        steps: kinds
            .iter()
            .map(|(id, kind, gate, on_fail)| StepRun {
                id: (*id).to_string(),
                kind: (*kind).to_string(),
                gate: (*gate).to_string(),
                on_fail: (*on_fail).to_string(),
                ..Default::default()
            })
            .collect(),
        entry: serde_json::json!({ "key": "EXT-1", "title": "样例实例" }),
        gaps: Vec::new(),
        parked: None,
        created_at_ms: now,
        updated_at_ms: now,
    }
}

fn board_issue_for(store: &Arc<BoardStore>, title: &str, ext_key: &str) -> nemesis_board::Issue {
    store
        .create_issue(nemesis_board::models::NewIssue {
            title: title.to_string(),
            description: String::new(),
            creator: nemesis_board::Actor::system("flow-engine-tests"),
            origin: Some(nemesis_board::models::TaskOrigin {
                origin_type: ORIGIN_TYPE.to_string(),
                origin_id: format!("tpl/{ext_key}"),
            }),
            ..Default::default()
        })
        .expect("create issue")
}

// ---------------------------------------------------------------------------
// 纯函数：on_fail / 状态投影
// ---------------------------------------------------------------------------

#[test]
fn parse_on_fail_branches() {
    assert!(matches!(parse_on_fail("continue"), OnFail::Continue));
    assert!(matches!(parse_on_fail("retry:3"), OnFail::Retry(3)));
    assert!(matches!(parse_on_fail("retry:1"), OnFail::Retry(1)));
    assert!(matches!(parse_on_fail("retry:10"), OnFail::Retry(10)));
    // 越界/非法一律 park_human（防御默认）。
    assert!(matches!(parse_on_fail("retry:0"), OnFail::ParkHuman));
    assert!(matches!(parse_on_fail("retry:11"), OnFail::ParkHuman));
    assert!(matches!(parse_on_fail("retry:x"), OnFail::ParkHuman));
    assert!(matches!(parse_on_fail("park_human"), OnFail::ParkHuman));
    assert!(matches!(parse_on_fail(""), OnFail::ParkHuman));
}

#[test]
fn state_of_priority_branches() {
    // parked 最高优先。
    let mut inst = inst_with_steps("tpl", &[("s", KIND_FIXLOOP, "", "")]);
    inst.parked = Some("x".to_string());
    assert_eq!(state_of(&inst), "parked");

    // 游标到底 = done。
    let mut inst = inst_with_steps("tpl", &[("s", KIND_FIXLOOP, "", "")]);
    inst.current_step = 1;
    assert_eq!(state_of(&inst), "done");

    // manual gate 未确认 = awaiting_gate。
    let inst = inst_with_steps("tpl", &[("s", KIND_PUBLISH, "manual", "")]);
    assert_eq!(state_of(&inst), "awaiting_gate");

    // gate 已确认 = 活动态。
    let mut inst = inst_with_steps("tpl", &[("s", KIND_PUBLISH, "manual", "")]);
    inst.steps[0].gate_confirmed = true;
    assert_eq!(state_of(&inst), "delivering");

    // Running 的观察步 = fixing。
    let mut inst = inst_with_steps("tpl", &[("s", KIND_FIXLOOP, "", "")]);
    inst.steps[0].status = StepStatus::Running;
    assert_eq!(state_of(&inst), "fixing");

    // 未知 kind = working。
    let inst = inst_with_steps("tpl", &[("s", "mystery", "", "")]);
    assert_eq!(state_of(&inst), "working");
}

#[test]
fn activity_name_vocabulary() {
    assert_eq!(activity_name(KIND_FETCH), "fetching");
    assert_eq!(activity_name(KIND_LAND), "landing");
    assert_eq!(activity_name(KIND_FIXLOOP), "fixing");
    assert_eq!(activity_name(KIND_REGRESS), "regressing");
    assert_eq!(activity_name(KIND_PUBLISH), "delivering");
    assert_eq!(activity_name(KIND_REPORT), "reporting");
    assert_eq!(activity_name("other"), "working");
}

// ---------------------------------------------------------------------------
// 步骤记账
// ---------------------------------------------------------------------------

#[test]
fn complete_step_advances_cursor_and_records_artifact() {
    let flows = FlowStore::new(&temp_ws("complete"));
    let mut inst = inst_with_steps(
        "tpl",
        &[("a", KIND_LAND, "", ""), ("b", KIND_FIXLOOP, "", "")],
    );
    complete_step(
        &flows,
        &mut inst,
        &serde_json::json!({ "board_issue": "NB-7" }),
    );
    assert_eq!(inst.steps[0].status, StepStatus::Ok);
    assert_eq!(inst.steps[0].artifacts["board_issue"], "NB-7");
    assert!(inst.steps[0].finished_at_ms.is_some());
    assert_eq!(inst.current_step, 1);
}

#[test]
fn fail_step_park_human_parks() {
    let flows = FlowStore::new(&temp_ws("park"));
    let mut inst = inst_with_steps("tpl", &[("a", KIND_LAND, "", "park_human")]);
    fail_step(&flows, &mut inst, "项目不存在: nope");
    assert_eq!(inst.steps[0].status, StepStatus::Failed);
    assert!(inst.parked.is_some());
    assert!(
        inst.parked
            .as_deref()
            .unwrap_or_default()
            .contains("项目不存在")
    );
}

#[test]
fn fail_step_retry_caps_then_parks() {
    let flows = FlowStore::new(&temp_ws("retry"));
    let mut inst = inst_with_steps("tpl", &[("a", KIND_LAND, "", "retry:2")]);
    // 第 1 次失败：attempts 1 < 2 → 回 Pending 等重试。
    fail_step(&flows, &mut inst, "e1");
    assert_eq!(inst.steps[0].status, StepStatus::Pending);
    assert_eq!(inst.steps[0].attempts, 1);
    assert!(inst.parked.is_none());
    // 第 2 次失败：attempts 2 >= 2 → 停车。
    fail_step(&flows, &mut inst, "e2");
    assert_eq!(inst.steps[0].attempts, 2);
    assert!(inst.parked.is_some());
}

#[test]
fn fail_step_continue_records_gap_and_advances() {
    let flows = FlowStore::new(&temp_ws("continue"));
    let mut inst = inst_with_steps(
        "tpl",
        &[("a", KIND_LAND, "", "continue"), ("b", KIND_REPORT, "", "")],
    );
    fail_step(&flows, &mut inst, "建单失败但不停车");
    assert_eq!(inst.current_step, 1);
    assert_eq!(inst.gaps.len(), 1);
    assert_eq!(inst.gaps[0]["step"], "a");
    // 步骤保持 Failed 态留痕。
    assert_eq!(inst.steps[0].status, StepStatus::Failed);
}

#[test]
fn fail_step_still_running_sentinel_is_silent() {
    let flows = FlowStore::new(&temp_ws("sentinel"));
    let mut inst = inst_with_steps("tpl", &[("a", KIND_FIXLOOP, "", "")]);
    fail_step(&flows, &mut inst, FLOW_STILL_RUNNING);
    assert_ne!(inst.steps[0].status, StepStatus::Failed);
    assert!(inst.parked.is_none());
    assert!(inst.steps[0].error.is_none());
}

// ---------------------------------------------------------------------------
// 模板校验 + 取数解析
// ---------------------------------------------------------------------------

fn tpl(id: &str, kinds: &[&str]) -> FlowTemplate {
    FlowTemplate {
        id: id.to_string(),
        interval_secs: 0,
        steps: kinds
            .iter()
            .enumerate()
            .map(|(i, k)| FlowStep {
                id: format!("s{i}"),
                kind: (*k).to_string(),
                gate: String::new(),
                when: String::new(),
                on_fail: String::new(),
                args: Default::default(),
            })
            .collect(),
    }
}

#[test]
fn validate_template_rules() {
    assert!(validate_template(&tpl("t", &[KIND_FETCH, KIND_LAND])).is_ok());
    // 恰好一个 fetch。
    assert!(validate_template(&tpl("t", &[])).is_err());
    assert!(validate_template(&tpl("t", &[KIND_LAND])).is_err());
    assert!(validate_template(&tpl("t", &[KIND_FETCH, KIND_FETCH])).is_err());
    // fetch 必须首步。
    assert!(validate_template(&tpl("t", &[KIND_LAND, KIND_FETCH])).is_err());
    // 未知 kind。
    assert!(validate_template(&tpl("t", &[KIND_FETCH, "warp"])).is_err());
}

#[test]
fn parse_entries_shapes() {
    // 数组直收。
    let arr = parse_entries(r#"[{"key":"A"},{"key":"B"}]"#).unwrap();
    assert_eq!(arr.len(), 2);
    // 包裹形态解包（四键表）。
    for wrap in ["issues", "items", "entries", "rows"] {
        let text = format!(r#"{{"{wrap}":[{{"key":"A"}}]}}"#);
        assert_eq!(parse_entries(&text).unwrap().len(), 1, "wrap={wrap}");
    }
    // 非法形态诚实报错。
    assert!(parse_entries(r#"{"key":"A"}"#).is_err());
    assert!(parse_entries("not json").is_err());
}

// ---------------------------------------------------------------------------
// FlowStore：落账 + 投影 + confirm/resume
// ---------------------------------------------------------------------------

#[test]
fn flow_store_roundtrip_list_and_corrupted_skip() {
    let ws = temp_ws("store");
    let flows = FlowStore::new(&ws);
    assert!(!flows.exists("tpl", "A"));
    let mut a = inst_with_steps("tpl", &[("s", KIND_LAND, "", "")]);
    a.instance = "A".to_string();
    let mut b = inst_with_steps("tpl", &[("s", KIND_LAND, "", "")]);
    b.instance = "B".to_string();
    flows.save(&a).unwrap();
    flows.save(&b).unwrap();
    assert!(flows.exists("tpl", "A"));

    let loaded = flows.load("tpl", "A").unwrap();
    assert_eq!(loaded.instance, "A");
    assert_eq!(loaded.ext_key, "EXT-1");

    // 损坏文件诚实跳过（不炸整列）。
    std::fs::write(flows.instance_path("tpl", "BAD"), "{ not json").unwrap();
    let all = flows.list_all("tpl");
    assert_eq!(all.len(), 2);
    let mut names: Vec<&str> = all.iter().map(|i| i.instance.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["A", "B"]);
}

#[test]
fn pick_latest_by_updated() {
    let ws = temp_ws("pick");
    let flows = FlowStore::new(&ws);
    let mut a = inst_with_steps("tpl", &[("s", KIND_LAND, "", "")]);
    a.instance = "A".to_string();
    a.updated_at_ms = 100;
    let mut b = inst_with_steps("tpl", &[("s", KIND_LAND, "", "")]);
    b.instance = "B".to_string();
    b.updated_at_ms = 200;
    flows.save(&a).unwrap();
    flows.save(&b).unwrap();
    // 缺省 = 最近更新。
    assert_eq!(flows.pick("tpl", None).unwrap().instance, "B");
    // 显式点名优先。
    assert_eq!(flows.pick("tpl", Some("A")).unwrap().instance, "A");
    assert!(flows.pick("tpl", Some("GHOST")).is_err());
}

#[test]
fn answer_confirm_gate_semantics() {
    let ws = temp_ws("confirm");
    let flows = FlowStore::new(&ws);
    let inst = inst_with_steps("tpl", &[("deliver", KIND_PUBLISH, "manual", "")]);
    flows.save(&inst).unwrap();

    // 无 gate 步骤 / 非 Pending / 已完成 / 停车 都诚实报错。
    let mut nogate = inst_with_steps("tpl", &[("s", KIND_LAND, "", "")]);
    nogate.instance = "NG".to_string();
    flows.save(&nogate).unwrap();
    assert!(flows.answer_confirm("tpl", Some("NG")).is_err());

    // 正常确认。
    let out = flows.answer_confirm("tpl", Some("I-1")).unwrap();
    assert_eq!(out["ok"], true);
    assert!(flows.load("tpl", "I-1").unwrap().steps[0].gate_confirmed);
    // 重复确认幂等（步骤仍 Pending 直到引擎推进）。
    let again = flows.answer_confirm("tpl", Some("I-1")).unwrap();
    assert_eq!(again["ok"], true);

    // 停车实例拒绝确认。
    let mut parked = inst_with_steps("tpl", &[("deliver", KIND_PUBLISH, "manual", "")]);
    parked.instance = "P".to_string();
    parked.parked = Some("失败".to_string());
    flows.save(&parked).unwrap();
    assert!(flows.answer_confirm("tpl", Some("P")).is_err());
}

#[test]
fn answer_resume_clears_parking() {
    let ws = temp_ws("resume");
    let flows = FlowStore::new(&ws);
    let mut inst = inst_with_steps("tpl", &[("a", KIND_LAND, "", "park_human")]);
    inst.parked = Some("步骤 a 失败: x".to_string());
    inst.steps[0].status = StepStatus::Failed;
    flows.save(&inst).unwrap();

    // 未停车实例诚实拒绝。
    let mut running = inst_with_steps("tpl", &[("s", KIND_LAND, "", "")]);
    running.instance = "R".to_string();
    flows.save(&running).unwrap();
    assert!(flows.answer_resume("tpl", Some("R")).is_err());

    flows.answer_resume("tpl", Some("I-1")).unwrap();
    let resumed = flows.load("tpl", "I-1").unwrap();
    assert!(resumed.parked.is_none());
    assert_eq!(resumed.steps[0].status, StepStatus::Pending);
    assert!(resumed.steps[0].error.is_none());
}

#[test]
fn answer_list_projection_fields() {
    let ws = temp_ws("list");
    let flows = FlowStore::new(&ws);
    let mut inst = inst_with_steps(
        "tpl",
        &[("land", KIND_LAND, "", ""), ("fix", KIND_FIXLOOP, "", "")],
    );
    complete_step(
        &flows,
        &mut inst,
        &serde_json::json!({ "board_issue": "NB-9" }),
    );
    let out = flows.answer_list("tpl").unwrap();
    let row = &out["rows"][0];
    assert_eq!(row["instance"], "I-1");
    assert_eq!(row["ext_key"], "EXT-1");
    assert_eq!(row["board_issue"], "NB-9");
    assert_eq!(row["state"], "fixing");
}

#[test]
fn answer_timeline_orders_events() {
    let ws = temp_ws("timeline");
    let flows = FlowStore::new(&ws);
    let mut inst = inst_with_steps(
        "tpl",
        &[("a", KIND_LAND, "", ""), ("b", KIND_FIXLOOP, "", "")],
    );
    complete_step(&flows, &mut inst, &serde_json::json!({}));
    fail_step(&flows, &mut inst, "boom");
    let out = flows.answer_timeline("tpl", Some("I-1")).unwrap();
    let events = out["events"].as_array().unwrap();
    // a: started + ok，b: started + failed（boom）→ ≥4 条，时间升序。
    assert!(events.len() >= 4);
    let mut last = 0i64;
    for e in events {
        let ts = e["ts_ms"].as_i64().unwrap();
        assert!(ts >= last);
        last = ts;
    }
    assert!(out.to_string().contains("boom"));
}

// ---------------------------------------------------------------------------
// token 替换 + 条目取值
// ---------------------------------------------------------------------------

#[test]
fn substitute_tokens_resolution() {
    let ws = temp_ws("tokens");
    let flows = FlowStore::new(&ws);
    let mut inst = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);
    complete_step(
        &flows,
        &mut inst,
        &serde_json::json!({ "board_issue": "NB-42" }),
    );
    // report_path 是保留 token（先读步骤产物账面，账面空回落 extra；两处
    // 皆无 = 原样保留）。extra 只供非保留键。
    let extra = serde_json::json!({ "jira_base": "https://jira.example" });

    let out = substitute_tokens(
        "{{instance}}/{{ext_key}}:{{title}} -> {{board_issue}} @ {{jira_base}} ?{{unknown}}",
        &inst,
        &extra,
    );
    assert_eq!(
        out,
        "I-1/EXT-1:样例实例 -> NB-42 @ https://jira.example ?{{unknown}}"
    );
    // 保留键账面+extra 皆缺 → 原样保留（模板残留可见）。
    assert_eq!(
        substitute_tokens("[{{report_path}}]", &inst, &extra),
        "[{{report_path}}]"
    );
    // report 步执行中产物未入账，extra 是活源（P4 e2e 实录根修：旧实现
    // 账面空串遮蔽 extra，{{report_path}} 恒空）。
    assert_eq!(
        substitute_tokens(
            "[{{report_path}}]",
            &inst,
            &serde_json::json!({ "report_path": "D:\\logs\\dossiers\\NB-42" })
        ),
        "[D:\\logs\\dossiers\\NB-42]"
    );
}

#[test]
fn substitute_tokens_value_tree() {
    let ws = temp_ws("tokens_tree");
    let flows = FlowStore::new(&ws);
    let mut inst = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);
    complete_step(
        &flows,
        &mut inst,
        &serde_json::json!({ "board_issue": "NB-42" }),
    );
    // 树内替换：Windows 路径反斜杠由 serde 统一转义，序列化后仍是合法 JSON。
    let args = serde_json::json!({
        "key": "{{ext_key}}",
        "body": "完成「{{title}}」（看板单 {{board_issue}}）\n报告：{{report_path}}",
        "nested": { "keep": 7, "list": ["{{instance}}", null] }
    });
    let out = substitute_tokens_value(
        &args,
        &inst,
        &serde_json::json!({ "report_path": "C:\\Users\\t\\logs\\dossiers\\D" }),
    );
    assert_eq!(out["key"], "EXT-1");
    assert_eq!(
        out["body"],
        "完成「样例实例」（看板单 NB-42）\n报告：C:\\Users\\t\\logs\\dossiers\\D"
    );
    assert_eq!(out["nested"]["keep"], 7);
    assert_eq!(out["nested"]["list"][0], "I-1");
    assert!(out["nested"]["list"][1].is_null());
    // 序列化往返仍合法（真实消费形态：connector 请求体）。
    let round = serde_json::to_string(&out).unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&round).is_ok());
}

#[test]
fn entry_string_shapes() {
    let entry = serde_json::json!({ "key": "A-1", "num": 7, "null": null });
    assert_eq!(entry_string(&entry, "key").as_deref(), Some("A-1"));
    assert_eq!(entry_string(&entry, "num").as_deref(), Some("7"));
    assert_eq!(entry_string(&entry, "null"), None);
    assert_eq!(entry_string(&entry, "missing"), None);
}

// ---------------------------------------------------------------------------
// board 绑定（真 BoardStore 临时库）
// ---------------------------------------------------------------------------

#[test]
fn board_land_creates_and_is_idempotent() {
    let ws = temp_ws("land");
    let store = make_store(&ws);
    let mut inst = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);

    let first = start_board_land(&store, &mut inst, None).unwrap();
    assert_eq!(first["landed"], "created");
    let number = first["board_issue"].as_str().unwrap().to_string();

    // 幂等：同 origin 二次 land = existing 且同单号。
    let second = start_board_land(&store, &mut inst, None).unwrap();
    assert_eq!(second["landed"], "existing");
    assert_eq!(second["board_issue"].as_str(), Some(number.as_str()));
}

#[test]
fn board_land_missing_project_err() {
    let ws = temp_ws("land-proj");
    let store = make_store(&ws);
    let mut inst = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);
    let err = start_board_land(&store, &mut inst, Some("不存在项目")).unwrap_err();
    assert!(err.contains("项目不存在"), "got: {err}");
}

#[test]
fn start_fixloop_dispatch_and_observe() {
    let ws = temp_ws("fixloop");
    let store = make_store(&ws);
    let issue = board_issue_for(&store, "修 bug", "FX-1");
    let mut inst = inst_with_steps("tpl", &[("fix", KIND_FIXLOOP, "", "")]);
    inst.ext_key = "FX-1".to_string();

    // Backlog + 派发失败 → Err。
    let err = start_fixloop(&store, &mut inst, &|_: &nemesis_board::Issue| false).unwrap_err();
    assert!(err.contains("派发未成行"), "got: {err}");

    // Backlog + 派发成功 → Started 语义（Ok）。
    assert!(start_fixloop(&store, &mut inst, &|_: &nemesis_board::Issue| true).is_ok());

    // Done → 观察态直通。
    store
        .transition_issue(
            issue.id,
            nemesis_board::models::IssueStatus::Done,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    let out = start_fixloop(&store, &mut inst, &|_: &nemesis_board::Issue| false).unwrap();
    assert_eq!(out["observing"], true);
}

#[test]
fn poll_fixloop_states() {
    let ws = temp_ws("poll-fix");
    let store = make_store(&ws);
    let issue = board_issue_for(&store, "修 bug", "PF-1");
    let mut inst = inst_with_steps("tpl", &[("fix", KIND_FIXLOOP, "", "")]);
    inst.ext_key = "PF-1".to_string();

    // 执行中 → StillRunning（backlog 不可直达 in_review，走 in_progress）。
    store
        .transition_issue(
            issue.id,
            nemesis_board::models::IssueStatus::InProgress,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    assert!(matches!(
        poll_fixloop(&store, &inst),
        PollOutcome::StillRunning
    ));
    // InReview = 工作交付完成（fixloop 两段式分界）。
    store
        .transition_issue(
            issue.id,
            nemesis_board::models::IssueStatus::InReview,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    match poll_fixloop(&store, &inst) {
        PollOutcome::Completed(v) => assert_eq!(v["delivered"], true),
        other => panic!("expect completed, got {other:?}"),
    }
    // Blocked = 失败。
    store
        .transition_issue(
            issue.id,
            nemesis_board::models::IssueStatus::Blocked,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    assert!(matches!(
        poll_fixloop(&store, &inst),
        PollOutcome::Failed(_)
    ));
}

#[test]
fn regress_outcome_states() {
    let ws = temp_ws("regress");
    let store = make_store(&ws);
    let issue = board_issue_for(&store, "修 bug", "RG-1");
    let mut inst = inst_with_steps("tpl", &[("reg", KIND_REGRESS, "", "")]);
    inst.ext_key = "RG-1".to_string();

    // InReview 继续等评审（验收只有 Done 才成立）。
    store
        .transition_issue(
            issue.id,
            nemesis_board::models::IssueStatus::InProgress,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    store
        .transition_issue(
            issue.id,
            nemesis_board::models::IssueStatus::InReview,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    assert!(matches!(
        regress_outcome(&store, &inst),
        PollOutcome::StillRunning
    ));
    // Done = accepted。
    store
        .transition_issue(
            issue.id,
            nemesis_board::models::IssueStatus::Done,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    match regress_outcome(&store, &inst) {
        PollOutcome::Completed(v) => assert_eq!(v["verdict"], "accepted"),
        other => panic!("expect completed, got {other:?}"),
    }
    // Done 是终态（不可再转）——Blocked 腿用第二张单验。
    let issue2 = board_issue_for(&store, "修 bug 二", "RG-2");
    let mut inst2 = inst_with_steps("tpl", &[("reg", KIND_REGRESS, "", "")]);
    inst2.ext_key = "RG-2".to_string();
    store
        .transition_issue(
            issue2.id,
            nemesis_board::models::IssueStatus::Blocked,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    assert!(matches!(
        regress_outcome(&store, &inst2),
        PollOutcome::Failed(_)
    ));
}

// ---------------------------------------------------------------------------
// 实例物化
// ---------------------------------------------------------------------------

#[test]
fn materialize_instances_creates_skips_and_records_fetch() {
    let ws = temp_ws("materialize");
    let flows = FlowStore::new(&ws);
    let def = FlowTemplateDef {
        slug: "sample".to_string(),
        template: tpl("tpl", &[KIND_FETCH, KIND_LAND]),
    };
    let entries = vec![
        serde_json::json!({ "key": "K1", "title": "第一单" }),
        serde_json::json!({ "key": "K1", "title": "重复键" }), // 幂等跳过
        serde_json::json!({ "nokey": true }),                  // 缺键诚实跳过
        serde_json::json!({ "key": "K2" }),                    // 无标题回落 key
    ];
    let created = materialize_instances(&flows, &def, &entries).unwrap();
    assert_eq!(created, 2);
    // 幂等：重跑不新建。
    assert_eq!(materialize_instances(&flows, &def, &entries).unwrap(), 0);

    let k1 = flows.load("tpl", "K1").unwrap();
    assert_eq!(k1.ext_key, "K1");
    assert_eq!(k1.title, "第一单");
    assert_eq!(k1.current_step, 1); // fetch 已完成
    assert_eq!(k1.steps[0].status, StepStatus::Ok);
    assert_eq!(k1.steps[0].artifacts["count"], 4);
    // 无标题条目回落 key。
    let k2 = flows.load("tpl", "K2").unwrap();
    assert_eq!(k2.title, "K2");
}

// ---------------------------------------------------------------------------
// 模板扫描（真实 plugins_wasm 目录形态）
// ---------------------------------------------------------------------------

#[test]
fn scan_templates_filters_switches_and_kinds() {
    // 空目录 → 空清单（不炸）。
    let ws = temp_ws("scan");
    let plugins = ws.join("plugins_wasm");
    std::fs::create_dir_all(&plugins).unwrap();
    let out = scan_templates(&plugins, &ws.join("config"));
    assert!(out.is_empty());

    // 真 manifest：panel kind + 合法模板 → 进清单；switch 关 → 不进。
    let dir = plugins.join("scan-sample");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        "api-version = 1\nslug = \"scan-sample\"\nkind = \"panel\"\nname = \"扫描样例\"\nversion = \"0.1.0\"\n\n[[ui.page]]\nroute = \"scan\"\ntitle = \"扫描\"\n\n[[flow.template]]\nid = \"tpl\"\ninterval-secs = 0\n\n[[flow.template.steps]]\nid = \"fetch\"\nkind = \"connector_fetch\"\n",
    )
    .unwrap();
    // 开关缺省 false = 不加载。
    assert!(scan_templates(&plugins, &ws.join("config")).is_empty());
    // 开关开 → 命中（开关文件形态：{"plugins": {slug: {enabled}}}）。
    let cfg = ws.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.plugin.wasm.json"),
        r#"{ "plugins": { "scan-sample": { "enabled": true } } }"#,
    )
    .unwrap();
    let out = scan_templates(&plugins, &cfg);
    // 诊断：manifest 解析/校验错误现场化（解析失败 = 断言前可见）。
    if out.is_empty() {
        if let Err(e) = PluginManifest::load_from_dir(&dir) {
            panic!("scan 空清单，manifest 加载失败: {e:?}");
        }
        panic!(
            "scan 空清单，manifest 可解析——switch 面未命中: {:?}",
            std::fs::read_to_string(cfg.join("config.plugin.wasm.json"))
        );
    }
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].slug, "scan-sample");
    assert_eq!(out[0].template.id, "tpl");
}

/// 缺陷 A（2026-11-01）回归锁：enable/disable 只写 config.plugin.wasm.json
/// 不碰载荷目录——缓存戳必须覆盖开关文件，否则禁用后 tick 照跑、重启用
/// 不恢复。载荷目录侧入戳（旧行为）一并钉住。
#[test]
fn template_stamp_covers_switch_file() {
    let ws = temp_ws("stamp");
    let plugins = ws.join("plugins_wasm");
    std::fs::create_dir_all(&plugins).unwrap();
    let cfg = ws.join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    let switch = cfg.join("config.plugin.wasm.json");
    std::fs::write(&switch, r#"{ "plugins": { "s": { "enabled": true } } }"#).unwrap();
    let before = template_stamp(&plugins, &cfg);

    // 载荷目录不动，只翻开关文件 → 戳必须变。（NTFS/ext4 mtime 精度
    // 均 ≥ 毫秒，50ms 间隔足以分辨。）
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&switch, r#"{ "plugins": { "s": { "enabled": false } } }"#).unwrap();
    let after = template_stamp(&plugins, &cfg);
    assert_ne!(before, after, "开关文件翻转必须改变缓存戳");

    // 开关文件删除 → 戳同样变化。
    std::fs::remove_file(&switch).unwrap();
    let gone = template_stamp(&plugins, &cfg);
    assert_ne!(after, gone, "开关文件删除必须改变缓存戳");

    // 载荷目录条目数仍入戳（旧行为不回退）。
    std::fs::create_dir_all(plugins.join("new-slug")).unwrap();
    let bumped = template_stamp(&plugins, &cfg);
    assert_ne!(gone.1, bumped.1, "载荷目录条目数必须入戳");
}

// ---------------------------------------------------------------------------
// entry_string 点路径
// ---------------------------------------------------------------------------

#[test]
fn entry_string_dot_path_and_literal_fallback() {
    // 真实 Jira 形态：标题在嵌套层。
    let entry: Value = serde_json::json!({"key": "PROJ-1", "fields": {"summary": "登录页 500"}});
    assert_eq!(
        entry_string(&entry, "fields.summary").as_deref(),
        Some("登录页 500")
    );
    // 顶层键照旧。
    assert_eq!(entry_string(&entry, "key").as_deref(), Some("PROJ-1"));
    // 数字字段。
    let num: Value = serde_json::json!({"id": 42, "n": {"c": 7}});
    assert_eq!(entry_string(&num, "id").as_deref(), Some("42"));
    assert_eq!(entry_string(&num, "n.c").as_deref(), Some("7"));
    // 点路径中途断裂 → None（不 panic）。
    assert_eq!(entry_string(&entry, "fields.nested.deep"), None);
    // 字面量键兜底：键本身含点且顶层恰有该字段。
    let lit: Value = serde_json::json!({"a.b": "literal"});
    assert_eq!(entry_string(&lit, "a.b").as_deref(), Some("literal"));
    // 非串/非数叶子（对象/数组/布尔）→ None。
    let obj: Value = serde_json::json!({"fields": {"summary": {"x": 1}}, "flag": true});
    assert_eq!(entry_string(&obj, "fields.summary"), None);
    assert_eq!(entry_string(&obj, "flag"), None);
}

// ---------------------------------------------------------------------------
// 决策 B：when 条件步（eval_when / skip_step / steps.<id>.<field> 投影）
// ---------------------------------------------------------------------------

#[test]
fn eval_when_semantics() {
    // 数值对按数值比。
    assert!(eval_when("10 >= 9").unwrap());
    assert!(!eval_when("9 > 10").unwrap());
    assert!(eval_when("3.14 == 3.14").unwrap());
    assert!(eval_when("2 != 3").unwrap());
    assert!(eval_when("1 <= 1").unwrap());
    assert!(eval_when("1 < 2").unwrap());
    // 非数值对按字符串比（== 全等 / > 字典序）。
    assert!(eval_when("done == done").unwrap());
    assert!(eval_when("'done' == done").unwrap(), "成对引号剥离");
    assert!(eval_when("\"failed\" != ok").unwrap());
    assert!(eval_when("beta > alpha").unwrap());
    assert!(
        !eval_when("9 > abc").unwrap(),
        "混对退字符串字典序（'9' < 'a'）"
    );
    // 同位时长 op 先中（>= / <= 不被单字符误拆）。
    assert!(eval_when("5 >= 5").unwrap());
    assert!(eval_when("5 <= 5").unwrap());
    // 左值含比较符形态：最左命中是确定性文档化行为。
    assert!(!eval_when("a >= b >= c").unwrap());
    // 畸形 → Err（调用方按失败走 on_fail，不静默）。
    assert!(eval_when("just text").is_err(), "无比较符");
    assert!(eval_when("== right").is_err(), "左空");
    assert!(eval_when("left ==").is_err(), "右空");
}

#[test]
fn skip_step_marks_skipped_and_advances() {
    let flows = FlowStore::new(&temp_ws("skip"));
    let mut inst = inst_with_steps(
        "tpl",
        &[
            ("cond", KIND_LAND, "", "park_human"),
            ("after", KIND_REPORT, "", ""),
        ],
    );
    skip_step(&flows, &mut inst, "1 == 2");
    assert_eq!(inst.steps[0].status, StepStatus::Skipped);
    assert_eq!(inst.steps[0].artifacts["skipped_when"], "1 == 2");
    assert!(inst.steps[0].finished_at_ms.is_some());
    assert_eq!(inst.current_step, 1);
    assert!(inst.parked.is_none(), "跳过不走 on_fail（跳过不是失败）");
    assert!(inst.gaps.is_empty(), "跳过不是缺口");
}

#[test]
fn substitute_tokens_steps_projection() {
    let mut inst = inst_with_steps(
        "tpl",
        &[("land", KIND_LAND, "", ""), ("fix", KIND_FIXLOOP, "", "")],
    );
    inst.steps[0].status = StepStatus::Ok;
    inst.steps[1].attempts = 2;
    inst.steps[1].error = Some("第一次派发失败".to_string());

    let out = substitute_tokens(
        "{{steps.land.status}}/{{steps.fix.attempts}}/{{steps.fix.error}}",
        &inst,
        &serde_json::json!({}),
    );
    assert_eq!(out, "ok/2/第一次派发失败");
    // 未知 id / 未知 field = 原样保留（诚实不猜，模板写错可见）。
    assert_eq!(
        substitute_tokens("[{{steps.ghost.status}}]", &inst, &serde_json::json!({})),
        "[{{steps.ghost.status}}]"
    );
    assert_eq!(
        substitute_tokens("[{{steps.land.mystery}}]", &inst, &serde_json::json!({})),
        "[{{steps.land.mystery}}]"
    );
}

#[test]
fn when_pipeline_interpolate_then_eval() {
    // advance_template 接线形态（纯函数级）：先内插（含 steps 投影 + 账面
    // 产物）再求值——true 推进 / false 跳过 / Err 失败三分支的数据基础。
    let mut inst = inst_with_steps(
        "tpl",
        &[("land", KIND_LAND, "", ""), ("reg", KIND_REGRESS, "", "")],
    );
    inst.steps[0].status = StepStatus::Ok;
    inst.steps[0].artifacts = serde_json::json!({ "board_issue": "NB-9" });
    inst.current_step = 1;

    // 条件引用前步投影 → true。
    let rendered = substitute_tokens("{{steps.land.status}} == ok", &inst, &serde_json::json!({}));
    assert!(eval_when(&rendered).unwrap());
    // 条件引用账面产物 + 字符串比较（board_issue 非数值形态）。
    let rendered = substitute_tokens("{{board_issue}} >= NB-1", &inst, &serde_json::json!({}));
    assert!(eval_when(&rendered).unwrap());
    // false 分支：状态不匹配。
    let rendered = substitute_tokens(
        "{{steps.land.status}} == running",
        &inst,
        &serde_json::json!({}),
    );
    assert!(!eval_when(&rendered).unwrap());
    // 畸形（token 不在账面且无值语义）原样保留后仍可判 → false 而非 Err。
    let rendered = substitute_tokens("{{unknown_token}} == x", &inst, &serde_json::json!({}));
    assert!(
        !eval_when(&rendered).unwrap(),
        "残留 token 按字符串比较，确定性 false"
    );
}
