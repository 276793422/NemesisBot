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
        origin_type: String::new(),
        origin_id: String::new(),
        origin_meta: Default::default(),
        body: String::new(),
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
                origin_meta: Default::default(),
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
        // 身份映射声明是模板合法性要件（域剥离批 C-1）：夹具默认给一条
        // 可渲染声明（token 表 {connector, key, arg.*}）。
        origin_type: "test".to_string(),
        origin_id: "test/{{key}}".to_string(),
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
        ..Default::default()
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
// 外循环域剥离批 C 回归锁（计划 §六：模板映射契约 / 回写表 / publish 声明）
// ---------------------------------------------------------------------------

/// §六①：模板映射契约 fetch→land 全链（bugfix-flow 0.2.0 声明逐字段镜像）。
/// Gitea 形 entries（顶层数组、number 整数）→ materialize（身份解析）→
/// board_land（board 级 TaskOrigin 三件套透传 + 整串精确去重）。
#[test]
fn gitea_shape_full_chain_materialize_to_land() {
    let ws = temp_ws("gitea-chain");
    let store = make_store(&ws);
    let flows = FlowStore::new(&ws);

    let mut template = tpl("bugfix-pipeline", &[KIND_FETCH, KIND_LAND]);
    template.origin_type = "gitea".to_string();
    template.origin_id = "{{connector}}/{{arg.owner}}/{{arg.repo}}#{{key}}".to_string();
    template.origin_meta = BTreeMap::from([
        ("owner".to_string(), "p0admin".to_string()),
        ("repo".to_string(), "durfmt".to_string()),
        ("number".to_string(), "{{key}}".to_string()),
    ]);
    template.steps[0].args = BTreeMap::from([
        ("connector".to_string(), "gitea".to_string()),
        ("op".to_string(), "issues_list".to_string()),
        (
            "args_json".to_string(),
            r#"{"owner": "p0admin", "repo": "durfmt", "state": "open"}"#.to_string(),
        ),
        ("key_field".to_string(), "number".to_string()),
        ("title_field".to_string(), "title".to_string()),
        ("body_field".to_string(), "body".to_string()),
    ]);
    let def = FlowTemplateDef {
        slug: "bugfix-flow".to_string(),
        template,
    };

    // Gitea issues_list 真实响应形态：顶层数组，number 整数。
    let entries = vec![
        serde_json::json!({
            "number": 12,
            "title": "修复持续率格式化",
            "state": "open",
            "body": "复现步骤……\n\n[CHECK] artifacts/durfmt.md 存在"
        }),
        serde_json::json!({ "number": 13, "title": "缺正文单", "state": "open" }),
    ];

    // 全链前半：取数结果 → 实例物化（身份 token 表 {connector,key,arg.*}）。
    assert_eq!(materialize_instances(&flows, &def, &entries).unwrap(), 2);
    let inst = flows.load("bugfix-pipeline", "12").unwrap();
    assert_eq!(inst.ext_key, "12");
    assert_eq!(inst.title, "修复持续率格式化");
    assert_eq!(inst.origin_type, "gitea");
    assert_eq!(inst.origin_id, "gitea/p0admin/durfmt#12");
    assert_eq!(
        inst.origin_meta.get("owner").map(String::as_str),
        Some("p0admin")
    );
    assert_eq!(
        inst.origin_meta.get("repo").map(String::as_str),
        Some("durfmt")
    );
    assert_eq!(
        inst.origin_meta.get("number").map(String::as_str),
        Some("12")
    );
    assert!(inst.body.contains("复现步骤"));
    assert_eq!(inst.current_step, 1);
    assert_eq!(inst.steps[0].status, StepStatus::Ok);

    // 全链后半：board_land 建单 + origin 三件套透传（board 级 TaskOrigin）。
    let mut inst = flows.load("bugfix-pipeline", "12").unwrap();
    let out = start_board_land(&store, &mut inst, None).unwrap();
    assert_eq!(out["landed"], "created");
    let number = out["board_issue"].as_str().unwrap().to_string();

    let landed = store
        .list_issues_by_origin("gitea", "gitea/p0admin/durfmt#12", 1)
        .unwrap();
    assert_eq!(landed.len(), 1);
    let issue = &landed[0];
    assert_eq!(issue.number, number);
    let origin = issue.origin.as_ref().expect("landed 单必带 origin");
    assert_eq!(origin.origin_type, "gitea");
    assert_eq!(origin.origin_id, "gitea/p0admin/durfmt#12");
    assert_eq!(
        origin.origin_meta.get("number").map(String::as_str),
        Some("12")
    );
    assert!(
        issue
            .description
            .contains("来源: gitea gitea/p0admin/durfmt#12")
    );
    assert!(issue.description.contains("外部条目: 12"));
    assert!(
        issue
            .acceptance_criteria
            .as_ref()
            .is_some_and(|v| !v.is_empty()),
        "[CHECK] 行须提取为 AC"
    );

    // 整串精确去重：同 origin 二次 land = existing。
    let second = start_board_land(&store, &mut inst, None).unwrap();
    assert_eq!(second["landed"], "existing");
}

/// §六②：回写表 origin_meta 优先——坐标键挂 `origin_meta.*`，parse 声明
/// 即使在场也不消费（`origin_id.*` 不进表）。
#[test]
fn build_writeback_table_origin_meta_preferred() {
    let hook = FlowTransition {
        op: "issue_comment".to_string(),
        args: BTreeMap::from([("index".to_string(), "{{origin_meta.number}}".to_string())]),
        parse_origin_id: BTreeMap::from([("number".to_string(), "after '#'".to_string())]),
    };
    let origin = nemesis_board::models::TaskOrigin {
        origin_type: "gitea".to_string(),
        origin_id: "gitea/p0admin/durfmt#12".to_string(),
        origin_meta: BTreeMap::from([
            ("owner".to_string(), "p0admin".to_string()),
            ("repo".to_string(), "durfmt".to_string()),
            ("number".to_string(), "12".to_string()),
        ]),
    };
    let WritebackTableOutcome::Ready(table) =
        build_writeback_table(&origin, &hook, "NB-31", "in_review", "todo")
    else {
        panic!("origin_meta 在场必须 Ready");
    };
    assert_eq!(table["transition.status"], "in_review");
    assert_eq!(table["transition.from"], "todo");
    assert_eq!(table["transition.number"], "NB-31");
    assert!(table["transition.text"].contains("NB-31"));
    assert_eq!(table["origin_meta.number"], "12");
    assert!(!table.contains_key("origin_id.number"));
    assert_eq!(
        render_writeback_tokens("{{origin_meta.number}}", &table).unwrap(),
        "12"
    );
}

/// §六④：v1 旧单回写——origin_meta 缺席时 parse-origin-id 现场分解，
/// 结果**双前缀镜像**落表（`origin_id.*` + `origin_meta.*`，模板 args
/// 一套前缀两态通用；v1 字节格式 `gitea/p0admin/durfmt#12`）；形态不符 =
/// Retry（游标保留）；无 origin_meta 也无 parse 声明 = 永久跳过。
#[test]
fn build_writeback_table_v1_parse_fallback_skip_and_retry() {
    let hook = FlowTransition {
        op: "issue_comment".to_string(),
        args: BTreeMap::from([("index".to_string(), "{{origin_id.number}}".to_string())]),
        parse_origin_id: BTreeMap::from([
            ("owner".to_string(), "segment '/' 1".to_string()),
            ("repo".to_string(), "before '#' | segment '/' 2".to_string()),
            ("number".to_string(), "after '#'".to_string()),
        ]),
    };
    let v1 = nemesis_board::models::TaskOrigin {
        origin_type: "gitea".to_string(),
        origin_id: "gitea/p0admin/durfmt#12".to_string(),
        origin_meta: Default::default(),
    };
    let WritebackTableOutcome::Ready(table) =
        build_writeback_table(&v1, &hook, "NB-31", "done", "in_review")
    else {
        panic!("parse 声明在场必须 Ready");
    };
    assert_eq!(table["origin_id.owner"], "p0admin");
    assert_eq!(table["origin_id.repo"], "durfmt");
    assert_eq!(table["origin_id.number"], "12");
    assert_eq!(
        render_writeback_tokens("{{origin_id.number}}", &table).unwrap(),
        "12"
    );
    // 镜像断言：v1 兜底分解键同步落 `origin_meta.*`——模板 args 静态声明
    // 一套前缀（`{{origin_meta.*}}`）必须 v1 旧单同样可渲染（2026-11-02
    // NB-2 cancel 回帖 Retry 循环实证：只落 origin_id.* 时 meta 前缀模板
    // 永久失败）。
    assert_eq!(table["origin_meta.owner"], "p0admin");
    assert_eq!(table["origin_meta.repo"], "durfmt");
    assert_eq!(table["origin_meta.number"], "12");
    assert_eq!(
        render_writeback_tokens("{{origin_meta.number}}", &table).unwrap(),
        "12"
    );

    // origin_id 形态不符 = Retry（游标保留，下 tick 重试）。
    let bad = nemesis_board::models::TaskOrigin {
        origin_type: "gitea".to_string(),
        origin_id: "no-hash-form".to_string(),
        origin_meta: Default::default(),
    };
    assert!(matches!(
        build_writeback_table(&bad, &hook, "NB-32", "done", "in_review"),
        WritebackTableOutcome::Retry(_)
    ));

    // 无 origin_meta 也无 parse-origin-id = 永久跳过（调用方游标落盘）。
    let hook_no_parse = FlowTransition {
        op: "issue_comment".to_string(),
        args: Default::default(),
        parse_origin_id: Default::default(),
    };
    assert!(matches!(
        build_writeback_table(&v1, &hook_no_parse, "NB-31", "done", "in_review"),
        WritebackTableOutcome::PermanentSkip
    ));
}

/// §六⑤：publish 声明缺失/畸形诚实报错（域知识只在模板，宿主不猜）。
#[test]
fn parse_publish_step_protocol_honest_errors() {
    let noop_render = |tpl: &str| tpl.to_string();
    let base: BTreeMap<String, String> = BTreeMap::from([
        ("create_op".to_string(), "create_pull".to_string()),
        ("create_args".to_string(), r#"{"head":"h"}"#.to_string()),
        ("result_ref".to_string(), "$.number".to_string()),
        ("reconcile_op".to_string(), "pr_list".to_string()),
        (
            "reconcile_args".to_string(),
            r#"{"state":"all"}"#.to_string(),
        ),
        ("reconcile_match".to_string(), "$.head.ref == h".to_string()),
    ]);
    // 六个必填键逐个缺席，各自响（消息指名缺哪个）。
    for missing in [
        "create_op",
        "create_args",
        "result_ref",
        "reconcile_op",
        "reconcile_args",
        "reconcile_match",
    ] {
        let get = |key: &str| -> Option<String> {
            if key == missing {
                None
            } else {
                base.get(key).cloned()
            }
        };
        let err = parse_publish_step_protocol(&get, &noop_render).unwrap_err();
        assert!(
            err.contains(&format!("publish 步缺 {missing} 声明")),
            "missing={missing}, got: {err}"
        );
    }

    // 非法 JSON。
    let get = |key: &str| -> Option<String> {
        Some(if key == "create_args" {
            "{not json".to_string()
        } else {
            base.get(key).cloned().unwrap_or_default()
        })
    };
    let err = parse_publish_step_protocol(&get, &noop_render).unwrap_err();
    assert!(err.contains("create_args 非法 JSON"), "got: {err}");

    // JSON 但非对象。
    let get = |key: &str| -> Option<String> {
        Some(if key == "create_args" {
            "[1,2]".to_string()
        } else {
            base.get(key).cloned().unwrap_or_default()
        })
    };
    let err = parse_publish_step_protocol(&get, &noop_render).unwrap_err();
    assert!(err.contains("create_args 必须是 JSON 对象"), "got: {err}");

    // 值非字符串（token 模板必须是串）。
    let get = |key: &str| -> Option<String> {
        Some(if key == "create_args" {
            r#"{"head": 7}"#.to_string()
        } else {
            base.get(key).cloned().unwrap_or_default()
        })
    };
    let err = parse_publish_step_protocol(&get, &noop_render).unwrap_err();
    assert!(
        err.contains("create_args.head 值必须是字符串"),
        "got: {err}"
    );
}

/// §六③：publish 声明契约——bugfix-flow 0.2.0 声明镜像（字段名/取数期
/// token 渲染/缺省值 limit·page·20·5）。
#[test]
fn parse_publish_step_protocol_gitea_declaration_contract() {
    let args: BTreeMap<String, String> = BTreeMap::from([
        ("connector".to_string(), "gitea".to_string()),
        ("owner".to_string(), "p0admin".to_string()),
        ("repo".to_string(), "durfmt".to_string()),
        ("create_op".to_string(), "create_pull".to_string()),
        (
            "create_args".to_string(),
            r#"{"owner":"p0admin","repo":"durfmt","head":"{{branch}}","base":"main","title":"[{{board_issue}}] {{title}}"}"#.to_string(),
        ),
        ("result_ref".to_string(), "$.number".to_string()),
        ("result_url".to_string(), "$.html_url".to_string()),
        ("reconcile_op".to_string(), "pr_list".to_string()),
        (
            "reconcile_args".to_string(),
            r#"{"owner":"p0admin","repo":"durfmt","state":"all"}"#.to_string(),
        ),
        ("reconcile_match".to_string(), "$.head.ref == {{branch}}".to_string()),
    ]);
    let get = |key: &str| args.get(key).cloned();
    // 取数期渲染上下文（publish_protocol 的 parse-time token 侧）。
    let render = |tpl: &str| {
        tpl.replace("{{branch}}", "nemesis/task-T9")
            .replace("{{board_issue}}", "NB-31")
            .replace("{{title}}", "修复持续率")
    };
    let p = parse_publish_step_protocol(&get, &render).unwrap();
    assert_eq!(p.create_op, "create_pull");
    assert_eq!(p.create_args["head"], "nemesis/task-T9");
    assert_eq!(p.create_args["title"], "[NB-31] 修复持续率");
    assert_eq!(p.create_args["base"], "main");
    assert_eq!(p.result_ref, "$.number");
    assert_eq!(p.result_url.as_deref(), Some("$.html_url"));
    assert_eq!(p.reconcile_op, "pr_list");
    assert_eq!(p.reconcile_args["state"], "all");
    // match 比较式是运行期渲染（publish.rs rt() 补 branch 等运行时 token），
    // parse 期保留原始模板——这是契约不是遗漏。
    assert_eq!(p.reconcile_match, "$.head.ref == {{branch}}");
    // 缺省值契约。
    assert_eq!(p.reconcile_limit_key, "limit");
    assert_eq!(p.reconcile_page_key, "page");
    assert_eq!(p.reconcile_limit, 20);
    assert_eq!(p.reconcile_max_pages, 5);
}

/// publish 声明可选键覆盖 + 非法值回落缺省（max_pages ≤0 视作未声明）。
#[test]
fn parse_publish_step_protocol_optional_overrides() {
    let noop = |tpl: &str| tpl.to_string();
    let args: BTreeMap<String, String> = BTreeMap::from([
        ("create_op".to_string(), "create_pull".to_string()),
        ("create_args".to_string(), r#"{"head":"h"}"#.to_string()),
        ("result_ref".to_string(), "$.number".to_string()),
        ("reconcile_op".to_string(), "pr_list".to_string()),
        (
            "reconcile_args".to_string(),
            r#"{"state":"all"}"#.to_string(),
        ),
        ("reconcile_match".to_string(), "$.head.ref == h".to_string()),
        ("reconcile_limit_key".to_string(), "per_page".to_string()),
        ("reconcile_page_key".to_string(), "p".to_string()),
        ("reconcile_limit".to_string(), "50".to_string()),
        ("reconcile_max_pages".to_string(), "3".to_string()),
    ]);
    let get = |key: &str| args.get(key).cloned();
    let p = parse_publish_step_protocol(&get, &noop).unwrap();
    assert_eq!(p.reconcile_limit_key, "per_page");
    assert_eq!(p.reconcile_page_key, "p");
    assert_eq!(p.reconcile_limit, 50);
    assert_eq!(p.reconcile_max_pages, 3);

    // 非法数值 → 缺省（诚实不猜）。
    let mut bad = args.clone();
    bad.insert("reconcile_limit".to_string(), "abc".to_string());
    bad.insert("reconcile_max_pages".to_string(), "0".to_string());
    let get_bad = |key: &str| bad.get(key).cloned();
    let p_bad = parse_publish_step_protocol(&get_bad, &noop).unwrap();
    assert_eq!(p_bad.reconcile_limit, 20);
    assert_eq!(p_bad.reconcile_max_pages, 5);
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
        "api-version = 1\nslug = \"scan-sample\"\nkind = \"panel\"\nname = \"扫描样例\"\nversion = \"0.1.0\"\n\n[[ui.page]]\nroute = \"scan\"\ntitle = \"扫描\"\n\n[[flow.template]]\nid = \"tpl\"\ninterval-secs = 0\norigin-id = \"demo/{{key}}\"\n\n[[flow.template.steps]]\nid = \"fetch\"\nkind = \"connector_fetch\"\n",
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

// ---------------------------------------------------------------------------
// 域剥离批 C-1/C-2：身份映射 + 回写原语（纯函数面）
// ---------------------------------------------------------------------------

fn fetch_step(connector: &str, args_json: &str) -> FlowStep {
    FlowStep {
        id: "fetch".to_string(),
        kind: KIND_FETCH.to_string(),
        gate: String::new(),
        when: String::new(),
        on_fail: String::new(),
        args: [
            ("connector".to_string(), connector.to_string()),
            ("args_json".to_string(), args_json.to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

#[test]
fn resolve_instance_origin_v1_byte_compat() {
    // v1 origin_id 字节兼容：`gitea/{owner}/{repo}#{number}`。
    // P0 已导入三单的去重键就是这个形态——模板声明必须逐字节复现。
    let mut t = tpl("tpl", &[KIND_FETCH, KIND_LAND]);
    t.origin_type = String::new(); // 空 = 回落 connector
    t.origin_id = "{{connector}}/{{arg.owner}}/{{arg.repo}}#{{key}}".to_string();
    t.origin_meta.insert("owner".into(), "{{arg.owner}}".into());
    t.origin_meta.insert("number".into(), "{{key}}".into());
    let step = fetch_step("gitea", r#"{"owner":"p0admin","repo":"durfmt"}"#);
    let (ot, oid, meta) = resolve_instance_origin(&t, &step, "12").unwrap();
    assert_eq!(ot, "gitea", "origin_type 空 → 回落 fetch connector");
    assert_eq!(oid, "gitea/p0admin/durfmt#12");
    assert_eq!(meta.get("owner").map(String::as_str), Some("p0admin"));
    assert_eq!(meta.get("number").map(String::as_str), Some("12"));
}

#[test]
fn resolve_instance_origin_missing_origin_id_is_err() {
    let mut t = tpl("tpl", &[KIND_FETCH, KIND_LAND]);
    t.origin_id = String::new();
    let step = fetch_step("gitea", "{}");
    let e = resolve_instance_origin(&t, &step, "1").unwrap_err();
    assert!(e.contains("origin_id 声明"), "{e}");
}

#[test]
fn resolve_instance_origin_unknown_token_is_err() {
    // 严格渲染：声明里的未知 token = Err（身份串错了必须响，不静默留残）。
    let mut t = tpl("tpl", &[KIND_FETCH, KIND_LAND]);
    t.origin_id = "{{connector}}/{{nope}}".to_string();
    let step = fetch_step("gitea", "{}");
    assert!(resolve_instance_origin(&t, &step, "1").is_err());
}

#[test]
fn render_writeback_tokens_strict_and_no_rescan() {
    let mut table = std::collections::BTreeMap::new();
    table.insert("a".to_string(), "A{{b}}".to_string()); // 值内容合法含 {{
    table.insert("b".to_string(), "B".to_string());
    // 命中键插值；值中的 {{b}} 不回扫（插值结果不是新 token 源）。
    assert_eq!(
        render_writeback_tokens("x={{a}}", &table).unwrap(),
        "x=A{{b}}"
    );
    // 未知键 = Err。
    let e = render_writeback_tokens("{{zzz}}", &table).unwrap_err();
    assert!(e.contains("未知键"), "{e}");
    // 未闭合 {{ = Err。
    assert!(render_writeback_tokens("oops {{a", &table).is_err());
}

#[test]
fn eval_split_expr_doc_example() {
    // 模块注释的工作样例：`gitea/p0admin/durfmt#12` 三段分解。
    let oid = "gitea/p0admin/durfmt#12";
    assert_eq!(eval_split_expr(oid, "segment '/' 1").unwrap(), "p0admin");
    assert_eq!(
        eval_split_expr(oid, "before '#' | segment '/' 2").unwrap(),
        "durfmt"
    );
    assert_eq!(eval_split_expr(oid, "after '#'").unwrap(), "12");
    // 双引号分隔子串同认。
    assert_eq!(eval_split_expr(oid, "after \"#\"").unwrap(), "12");
}

#[test]
fn eval_split_expr_error_branches() {
    // 分隔子串未命中 = Err。
    assert!(eval_split_expr("abc", "after '#'").is_err());
    // 段序越界 = Err。
    assert!(eval_split_expr("a/b", "segment '/' 5").is_err());
    // 结果为空 = Err。
    assert!(eval_split_expr("abc", "after 'c'").is_err());
    // 步骤缺参数 / 参数后多余内容 = Err。
    assert!(eval_split_expr("a#b", "after").is_err());
    assert!(eval_split_expr("a#b", "after '#' junk").is_err());
}

#[test]
fn render_writeback_text_three_states() {
    let r = render_writeback_text("NB-7", "todo", "in_review");
    assert!(r.contains("NB-7") && r.contains("todo → in_review"), "{r}");
    let d = render_writeback_text("NB-7", "in_review", "done");
    assert!(d.contains("NB-7") && d.contains("in_review → done"), "{d}");
    let c = render_writeback_text("NB-7", "todo", "cancelled");
    assert!(c.contains("NB-7") && c.contains("cancelled"), "{c}");
    // 非三态 = 空串（调用方据空跳过 compose）。
    assert_eq!(render_writeback_text("NB-7", "x", "mystery"), "");
}

#[test]
fn writeback_decision_three_states() {
    assert_eq!(writeback_decision(None, "todo"), WritebackDecision::Adopt);
    assert_eq!(
        writeback_decision(Some("in_review"), "in_review"),
        WritebackDecision::Noop
    );
    assert_eq!(
        writeback_decision(Some("todo"), "done"),
        WritebackDecision::Compose {
            from: "todo".to_string()
        }
    );
}

#[test]
fn transition_state_roundtrip_and_corruption() {
    let flows = FlowStore::new(&temp_ws("wb-state"));
    // 缺席 = None（Adopt 面）。
    assert_eq!(flows.load_transition_state("tpl", "gitea/o/r#12"), None);
    flows
        .save_transition_state("tpl", "gitea/o/r#12", "in_review")
        .unwrap();
    assert_eq!(
        flows.load_transition_state("tpl", "gitea/o/r#12"),
        Some("in_review".to_string())
    );
    // 含 `/`/`#` 的外部键文件名安全化后仍可回读（同键同路径）。
    let path = flows.transition_state_path("tpl", "gitea/o/r#12");
    assert!(path.is_file(), "游标文件在场: {}", path.display());
    // 损坏文件 = None（诚实重踩 adopt，不炸轮询）。
    let flows2 = FlowStore::new(&temp_ws("wb-corrupt"));
    let corrupt = flows2.transition_state_path("tpl", "x");
    std::fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
    std::fs::write(&corrupt, "{ not json").unwrap();
    assert_eq!(flows2.load_transition_state("tpl", "x"), None);
}

#[test]
fn extract_check_criteria_semantics() {
    // [CHECK] 行提取：容忍缩进 + `- ` 前缀；行内非行首不认；无命中 = None。
    let body = "说明文字\n- [CHECK] 登录成功\n  [CHECK] 列表为空\n中间 [CHECK] 不认\n";
    assert_eq!(
        extract_check_criteria(body).unwrap(),
        "[CHECK] 登录成功\n[CHECK] 列表为空"
    );
    assert!(extract_check_criteria("没有检查项").is_none());
}

#[test]
fn salvage_json_objects_truncated_array() {
    // 截断页打捞：完整对象保留（数组尾 `]` 缺了不算残尾）、对象自身残尾丢弃；
    // 干净 JSON 走 parse_entries 不打捞。
    let (v, ok) = parse_entries_resilient(r#"[{"key":"A"},{"key":"B"}"#);
    assert!(!ok, "截断 = parse 不干净");
    assert_eq!(v.len(), 2, "B 的对象已完整（闭花括号在场），打捞 A+B");
    assert_eq!(v[0]["key"], "A");
    assert_eq!(v[1]["key"], "B");
    // 对象自身残尾（闭花括号都没了）= 丢弃。
    let (v1b, _) = parse_entries_resilient(r#"[{"key":"A"},{"key":"B"#);
    assert_eq!(v1b.len(), 1, "残尾对象 B 丢弃，只打捞出 A");
    assert_eq!(v1b[0]["key"], "A");
    let (v2, ok2) = parse_entries_resilient(r#"[{"key":"A"}]"#);
    assert!(ok2);
    assert_eq!(v2.len(), 1);
    // 字符串里的花括号不迷惑状态机（A 的值含 `}` 不提前终结对象，C 照常打捞）。
    let (v3, _) = parse_entries_resilient(r#"[{"key":"a}b"},{"key":"C"}"#);
    assert_eq!(v3.len(), 2);
    assert_eq!(v3[0]["key"], "a}b");
    assert_eq!(v3[1]["key"], "C");
}

#[test]
fn labels_any_match_semantics() {
    let e = serde_json::json!({"labels": [{"name": "bug"}, {"name": "ui"}]});
    // 空 want = 全过。
    assert!(labels_any_match(&e, &[]));
    // 任一命中 = 过。
    assert!(labels_any_match(&e, &["ui"]));
    assert!(!labels_any_match(&e, &["feat"]));
    // 无 labels 数组 = 不过（非空 want）。
    assert!(!labels_any_match(
        &serde_json::json!({"key": "x"}),
        &["bug"]
    ));
}

#[test]
fn eval_filter_json_path_and_literal() {
    let e = serde_json::json!({"state": "open", "n": 3});
    // $. 侧取条目标量，另一侧字面量。
    assert!(eval_filter(&e, "$.state == open"));
    assert!(!eval_filter(&e, "$.state == closed"));
    // 数值比较同源。
    assert!(eval_filter(&e, "$.n >= 3"));
    // 缺席路径 → "null" 可比较。
    assert!(eval_filter(&e, "$.ghost == null"));
    // 字面两侧直比。
    assert!(eval_filter(&e, "a == a"));
    // 畸形 = false（fetch 前置校验已拦，此处为兜底）。
    assert!(!eval_filter(&e, "no-op"));
}

// ---------------------------------------------------------------------------
// plugin 步骤（guest step，2026-11-03 批）：出参四态解析 + 入参投影 +
// 模板校验 plugin 引用面（自由函数级）。投递链路真组件 e2e 归
// nemesis-plugins-wasm 的 stepdemo 夹具（tests/e2e_guest.rs）；estop 冻结
// 由 flow tick loop 天然继承（无新闸），不另设用例。
// ---------------------------------------------------------------------------

#[test]
fn plugin_step_outcome_four_states() {
    // done：status 剥离，其余负载 = artifacts。
    let o = plugin_step_outcome(r#"{"status":"done","report_path":"/x/r.md","n":3}"#);
    assert_eq!(
        o,
        PluginOutcome::Done(serde_json::json!({"report_path":"/x/r.md","n":3}))
    );
    // 纯 status 出参 = 空对象负载。
    assert_eq!(
        plugin_step_outcome(r#"{"status":"done"}"#),
        PluginOutcome::Done(json!({}))
    );
    // running：无负载语义（下轮重投）。
    assert_eq!(
        plugin_step_outcome(r#"{"status":"running"}"#),
        PluginOutcome::Running
    );
    // failed：错误文本取负载 error 字段。
    let o = plugin_step_outcome(r#"{"status":"failed","error":"上游 502"}"#);
    assert_eq!(o, PluginOutcome::Failed("上游 502".to_string()));
    // failed 无 error 明细 = 缺省文案（诚实不猜）。
    let o = plugin_step_outcome(r#"{"status":"failed"}"#);
    assert!(
        matches!(o, PluginOutcome::Failed(ref m) if m.contains("guest 自报 failed")),
        "o: {o:?}"
    );
    // skipped：负载 = artifacts（when 假语义同款推进由 skip_step_with_payload 做）。
    let o = plugin_step_outcome(r#"{"status":"skipped","why":"已存在"}"#);
    assert_eq!(o, PluginOutcome::Skipped(json!({"why":"已存在"})));
}

#[test]
fn plugin_step_outcome_malformed_faces() {
    // 非 JSON。
    let o = plugin_step_outcome("not-json-at-all");
    assert!(
        matches!(o, PluginOutcome::Malformed(ref m) if m.contains("非 JSON")),
        "o: {o:?}"
    );
    // 缺 status。
    let o = plugin_step_outcome(r#"{"payload":1}"#);
    assert!(
        matches!(o, PluginOutcome::Malformed(ref m) if m.contains("缺 status")),
        "o: {o:?}"
    );
    // status 非字符串。
    let o = plugin_step_outcome(r#"{"status":3}"#);
    assert!(matches!(o, PluginOutcome::Malformed(_)), "o: {o:?}");
    // 未知 status（词表外诚实失败，不静默归类）。
    let o = plugin_step_outcome(r#"{"status":"wip"}"#);
    assert!(
        matches!(o, PluginOutcome::Malformed(ref m) if m.contains("未知 status: wip")),
        "o: {o:?}"
    );
    // 畸形长出参截断（ERR_SNIPPET 同长度，回灌不炸账本）。
    let long = format!("x{}", "y".repeat(ERR_SNIPPET * 3));
    let o = plugin_step_outcome(&long);
    assert!(
        matches!(o, PluginOutcome::Malformed(ref m) if m.chars().count() < ERR_SNIPPET + 60),
        "截断失败: {o:?}"
    );
}

#[test]
fn plugin_step_input_projection_shape() {
    let mut inst = inst_with_steps(
        "bugfix",
        &[
            ("fetch", KIND_FETCH, "", ""),
            ("run", "plugin:stepdemo:echo", "", ""),
        ],
    );
    // 前一步已完成带 board_issue 产物；当前 plugin 步带 args（token 渲染）。
    inst.steps[0].status = StepStatus::Ok;
    inst.steps[0].attempts = 1;
    inst.steps[0].error = Some("曾失败一次".to_string());
    // board_land 真实产物形态：Issue.number 是 String（账面字符串，投影时数字化）。
    inst.steps[0].artifacts = json!({"board_issue": "42"});
    inst.steps[1].args = [("who".to_string(), "{{ext_key}}".to_string())]
        .into_iter()
        .collect();
    inst.origin_type = "gitea".to_string();
    inst.origin_id = "acme/repo#7".to_string();

    let input = plugin_step_input(&inst, &inst.steps[1], "echo", "flow:bugfix/I-1");
    let v: Value = serde_json::from_str(&input).expect("input json");
    assert_eq!(v["op"], "echo");
    // args 经 token 渲染（ext_key → EXT-1）。
    assert_eq!(v["args"]["who"], "EXT-1");
    // 实例身份投影。
    assert_eq!(v["instance"]["template"], "bugfix");
    assert_eq!(v["instance"]["instance"], "I-1");
    assert_eq!(v["instance"]["ext_key"], "EXT-1");
    assert_eq!(v["instance"]["origin_type"], "gitea");
    assert_eq!(v["instance"]["origin_id"], "acme/repo#7");
    // steps 状态投影（id → {status,error,attempts}）。
    assert_eq!(v["steps"]["fetch"]["status"], "ok");
    assert_eq!(v["steps"]["fetch"]["error"], "曾失败一次");
    assert_eq!(v["steps"]["fetch"]["attempts"], 1);
    assert_eq!(v["steps"]["run"]["status"], "pending");
    // board_issue 锚点（board_land 产物数字化）。
    assert_eq!(v["board_issue"], 42);
    // attempts = 步账本 +1（宿主 Retry 账本记失败，投递不双计）。
    assert_eq!(v["attempts"], 1);
    assert_eq!(v["session_key"], "flow:bugfix/I-1");
    // board_issue 缺席 = null（诚实不造 0）。
    let bare = inst_with_steps("bugfix", &[("run", "plugin:stepdemo:echo", "", "")]);
    let v: Value =
        serde_json::from_str(&plugin_step_input(&bare, &bare.steps[0], "echo", "k")).unwrap();
    assert!(v["board_issue"].is_null(), "v: {v}");
    assert_eq!(v["attempts"], 1, "首次投递 = 账本 0 + 1");
}

#[test]
fn validate_template_accepts_plugin_refs() {
    // plugin 引用形态合法即过声明面校验（词表对账在安装期 + 运行期路由闸）。
    assert!(validate_template(&tpl("t", &[KIND_FETCH, "plugin:stepdemo:echo"])).is_ok());
    // op 段允许冒号（专业参数化同款语法；slug 词表 [a-z0-9-] 无点号）。
    assert!(validate_template(&tpl("t", &[KIND_FETCH, "plugin:acme-jira:issue:fetch"])).is_ok());
    // 坏形态照拒（无 op 段 / 坏 slug / 坏 op / 缺前缀）。
    assert!(validate_template(&tpl("t", &[KIND_FETCH, "plugin:stepdemo"])).is_err());
    assert!(validate_template(&tpl("t", &[KIND_FETCH, "plugin:Bad_Slug:echo"])).is_err());
    assert!(validate_template(&tpl("t", &[KIND_FETCH, "plugin:stepdemo:has space"])).is_err());
    assert!(validate_template(&tpl("t", &[KIND_FETCH, "stepdemo:echo"])).is_err());
    // plugin 引用不作 fetch（fetch 唯一性/首步约束不受影响）。
    assert!(validate_template(&tpl("t", &["plugin:stepdemo:fetch"])).is_err());
    assert!(validate_template(&tpl("t", &[KIND_FETCH, KIND_LAND, "plugin:x:y"])).is_ok());
}
