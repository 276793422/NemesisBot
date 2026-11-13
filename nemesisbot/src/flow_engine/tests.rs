//! flow 引擎单测：状态机纯函数、FlowStore 投影、模板校验、取数解析、
//! board 绑定（真 BoardStore 临时库），以及引擎本体（tick/answer/推进/
//! 回写轮询/report/repo_sync——真 BoardStore + TaskDb + 临时工作区，
//! agent loop 槽置空走停机窗口诚实报错路径）。需要常驻 AgentLoop 的
//! connector 调用成功链路（取数响应解析、publish 交付执行）归 e2e 覆盖。

use std::path::PathBuf;
use std::sync::Arc;

use super::*;

// —— 外评探针翻转收编（2026-11-13 R4/R5/R6 复评整改批 + 正向锁定）——

struct Nov13NoModel;
#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for Nov13NoModel {
    async fn chat(
        &self,
        _: &str,
        _: Vec<nemesis_agent::r#loop::LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        panic!("review must not call a model")
    }
}
struct Nov13Pages {
    pages: Vec<String>,
    calls: Arc<std::sync::Mutex<Vec<Value>>>,
}
#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for Nov13Pages {
    async fn execute(
        &self,
        args: &str,
        _: &nemesis_agent::context::RequestContext,
    ) -> Result<String, String> {
        let v: Value = serde_json::from_str(args).unwrap();
        self.calls.lock().unwrap().push(v.clone());
        let p = v["args"]["page"].as_u64().unwrap_or(1) as usize;
        Ok(self
            .pages
            .get(p - 1)
            .cloned()
            .unwrap_or_else(|| "[]".into()))
    }
    fn operation_override(&self, _: &str) -> Option<nemesis_agent::r#loop::OperationOverride> {
        Some(nemesis_agent::r#loop::OperationOverride {
            op_type: "internal".into(),
            force_approval: None,
            receipt_ctx: None,
        })
    }
}
fn nov13_wire_pages(fx: &EngineFixture, pages: Vec<String>) -> Arc<std::sync::Mutex<Vec<Value>>> {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut lp = AgentLoop::new(
        Box::new(Nov13NoModel),
        nemesis_agent::types::AgentConfig {
            model: "test".into(),
            ..Default::default()
        },
    );
    lp.register_tool(
        crate::connector_tools::INVOKE_TOOL_NAME.into(),
        Box::new(Nov13Pages {
            pages,
            calls: calls.clone(),
        }),
    );
    *fx.engine.agent_loop_ref.write() = Some(Arc::new(lp));
    calls
}
fn nov13_fetch_def() -> FlowTemplateDef {
    let mut def = FlowTemplateDef {
        slug: "sample".into(),
        template: tpl("nov13-fetch", &[KIND_FETCH, KIND_LAND]),
    };
    def.template.steps[0].args = BTreeMap::from([
        ("connector".into(), "test".into()),
        ("op".into(), "list".into()),
        ("page_size".into(), "2".into()),
        ("max_pages".into(), "5".into()),
        ("labels".into(), "agent-ready".into()),
    ]);
    def
}

/// 外评探针收编（正向锁定）：fetch 过滤后首页全被滤掉也继续翻页——
/// page1 全 human 标签被滤、page2 有 agent-ready → 创建 1、调用 [1,2]。
#[tokio::test]
async fn fetch_filtered_first_page_reaches_second_page() {
    let fx = make_engine("fetch-pages-fixed", true);
    let calls = nov13_wire_pages(
        &fx,
        vec![
            json!([{"key":"human1","labels":[{"name":"human"}]},{"key":"human2","labels":[{"name":"human"}]}]).to_string(),
            json!([{"key":"agent1","labels":[{"name":"agent-ready"}]}]).to_string(),
        ],
    );
    let out = fx.engine.fetch_template(&nov13_fetch_def()).await.unwrap();
    println!(
        "NOV13 filtered page fixed={out}; calls={:?}",
        calls.lock().unwrap()
    );
    assert_eq!(out["created"], 1);
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(|v| v["args"]["page"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

/// 外评探针收编（正向锁定）：截断 JSON 页 salvaged——完整前缀对象物化、
/// 截断残尾不炸（created=1、entries=1、ext_key=complete）。
#[tokio::test]
async fn fetch_truncated_page_materializes_salvaged_entry() {
    let fx = make_engine("salvage-fixed", true);
    let calls = nov13_wire_pages(
        &fx,
        vec![format!(
            "[{},{{\"key\":\"large\",\"body\":\"{}",
            json!({"key":"complete","labels":[{"name":"agent-ready"}]}),
            "a".repeat(9000)
        )],
    );
    let out = fx.engine.fetch_template(&nov13_fetch_def()).await.unwrap();
    println!(
        "NOV13 salvage fixed={out}; invocations={}",
        calls.lock().unwrap().len()
    );
    assert_eq!(out["created"], 1);
    assert_eq!(
        out["entries"], 1,
        "only the complete prefix object was salvaged"
    );
    assert_eq!(fx.flows.list_all("nov13-fetch")[0].ext_key, "complete");
}

/// R6 复评整改回归锁（外评探针
/// review_nov13_external_keys_collide_and_drop_second_instance 翻转）：
/// 段安全化碰撞（`a/b` 与 `a_b` 同落实例名 `a_b`）不再静默吞第二条——
/// 既有实例按 ext_key 核对，不等落 FNV 摘要后缀独立实例；两键各自物化、
/// 重扫幂等。
#[test]
fn external_keys_collide_materialize_independent_instances() {
    let ws = temp_ws("key-collision-r6");
    let flows = FlowStore::new(&ws);
    let def = FlowTemplateDef {
        slug: "sample".into(),
        template: tpl("tpl", &[KIND_FETCH, KIND_LAND]),
    };
    let entries = vec![
        json!({"key":"a/b","title":"first distinct issue"}),
        json!({"key":"a_b","title":"second distinct issue"}),
    ];
    let created = materialize_instances(&flows, &def, &entries).unwrap();
    println!(
        "NOV13 R6 created={created}; stored={:?}",
        flows
            .list_all("tpl")
            .iter()
            .map(|i| &i.ext_key)
            .collect::<Vec<_>>()
    );
    assert_eq!(created, 2, "两个键各自物化（不再吞第二条）");
    let mut keys: Vec<String> = flows
        .list_all("tpl")
        .iter()
        .map(|i| i.ext_key.clone())
        .collect();
    keys.sort();
    assert_eq!(keys, vec!["a/b".to_string(), "a_b".to_string()]);
    assert_eq!(
        materialize_instances(&flows, &def, &entries).unwrap(),
        0,
        "重扫幂等（两实例都按 ext_key 认领跳过）"
    );
}

/// R4 复评整改回归锁（外评探针
/// review_nov13_writeback_older_issue_starves_beyond_first_hundred 翻转）：
/// 回写轮询换 updated_at 水位 + **三态过滤下推 SQL**——大量 backlog 单
/// 不再占满 LIMIT 窗口挤掉三态单；老单的 done 转移被真实 attempt
///（旧形态永不触达：游标停 in_review、零失败计数）。
#[tokio::test]
async fn writeback_three_state_filter_reaches_older_issue_beyond_backlog_window() {
    let fx = make_engine("wb-filter-r4", true);
    let def = wb_def(
        "issue_comment",
        BTreeMap::new(),
        BTreeMap::new(),
        Some("jira"),
    );
    let old = wb_issue(
        &fx.store,
        "old",
        BTreeMap::from([("owner".into(), "acme".into())]),
    );
    // R5 孤儿闸要求订阅面单有模板实例——先物化（ext_key=old → fe/old）。
    materialize_instances(&fx.flows, &def, &[json!({"key": "old"})]).unwrap();
    to_review(&fx.store, &old);
    fx.engine.poll_writebacks(&def).await.unwrap();
    assert_eq!(
        fx.flows.load_transition_state("wb", "fe/old").as_deref(),
        Some("in_review")
    );
    // 100 条 backlog 单（非三态）——SQL 下推过滤后不占窗口。
    for i in 0..100 {
        wb_issue(&fx.store, &format!("new-{i}"), BTreeMap::new());
    }
    let selected = fx
        .store
        .list_issues_for_writeback("fe", 0, &["in_review", "done", "cancelled"], 100)
        .unwrap();
    assert!(
        selected.iter().any(|i| i.id == old.id),
        "三态下推：old 不被 backlog 挤出窗口"
    );
    fx.store
        .transition_issue(
            old.id,
            nemesis_board::IssueStatus::Done,
            &nemesis_board::Actor::system("review"),
        )
        .unwrap();
    for _ in 0..3 {
        fx.engine.poll_writebacks(&def).await.unwrap();
    }
    let failures = fx.engine.writeback_failures.lock().unwrap();
    println!("NOV13 R4 failures={failures:?}");
    assert_eq!(
        failures.get("wb/fe/old"),
        Some(&3),
        "done 转移被 attempt（每 poll 恰一次，Retried break 停推不跳批）"
    );
}

/// R5 复评整改回归锁（外评探针
/// review_nov13_writeback_subscribes_to_other_template_origin 翻转）：
/// 回写订阅**孤儿闸**——模板名下无实例的外部单（他项目 origin）确定性
/// 跳过：不采纳游标、不回写（旧形态跨模板误采纳+误投递）。
#[tokio::test]
async fn writeback_skips_issues_without_template_instance() {
    let fx = make_engine("wb-orphan-r5", true);
    let mut def = wb_def(
        "issue_comment",
        BTreeMap::new(),
        BTreeMap::new(),
        Some("jira"),
    );
    def.template.origin_id = "jira/PROJECT-A/{{key}}".into();
    def.template.steps[0].args.insert(
        "args_json".into(),
        json!({"project":"PROJECT-A"}).to_string(),
    );
    let foreign = fx
        .store
        .create_issue(nemesis_board::models::NewIssue {
            title: "belongs to project B and another flow".into(),
            origin: Some(nemesis_board::TaskOrigin {
                origin_type: "fe".into(),
                origin_id: "jira/PROJECT-B/7".into(),
                origin_meta: BTreeMap::from([("project".into(), "PROJECT-B".into())]),
            }),
            ..Default::default()
        })
        .unwrap();
    to_review(&fx.store, &foreign);
    fx.engine.poll_writebacks(&def).await.unwrap();
    assert_eq!(
        fx.flows
            .load_transition_state("wb", "jira/PROJECT-B/7")
            .as_deref(),
        None,
        "孤儿单不采纳游标（R5：模板名下无实例 = 确定性跳过）"
    );
    fx.store
        .transition_issue(
            foreign.id,
            nemesis_board::IssueStatus::Done,
            &nemesis_board::Actor::system("review"),
        )
        .unwrap();
    fx.engine.poll_writebacks(&def).await.unwrap();
    let failures = fx.engine.writeback_failures.lock().unwrap();
    println!("NOV13 R5 orphan failures={failures:?}");
    assert!(
        !failures.keys().any(|k| k.contains("jira/PROJECT-B/7")),
        "孤儿单不投递（不产生回写失败计数）"
    );
}

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
    // 保留键账面+extra 皆缺 → 原样保留（模板残留可）。
    assert_eq!(
        substitute_tokens("[{{report_path}}]", &inst, &extra),
        "[{{report_path}}]"
    );
    // report 步执行中产物未入账，extra 是活源（e2e 根修案例：旧实现
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

    let first = start_board_land(&store, &mut inst, None, None).unwrap();
    assert_eq!(first["landed"], "created");
    let number = first["board_issue"].as_str().unwrap().to_string();

    // 幂等：同 origin 二次 land = existing 且同单号。
    let second = start_board_land(&store, &mut inst, None, None).unwrap();
    assert_eq!(second["landed"], "existing");
    assert_eq!(second["board_issue"].as_str(), Some(number.as_str()));
}

#[test]
fn board_land_missing_project_err() {
    let ws = temp_ws("land-proj");
    let store = make_store(&ws);
    let mut inst = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);
    let err = start_board_land(&store, &mut inst, Some("不存在项目"), None).unwrap_err();
    assert!(err.contains("项目不存在"), "got: {err}");
}

/// body_preamble：流程契约前导确定性注入 description 头部（域模板声明面；
/// worker 读单即读纪律）。空串/全空白 = 不注入（不产生空段落垃圾）。
#[test]
fn board_land_preamble_prepended_to_description() {
    let ws = temp_ws("land-preamble");
    let store = make_store(&ws);
    let mut inst = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);

    let pre = "【修复流程契约】六阶段逐段留证据标记。";
    let out = start_board_land(&store, &mut inst, None, Some(pre)).unwrap();
    let landed = store.list_issues_by_origin("flow", "tpl/EXT-1", 1).unwrap();
    assert_eq!(landed.len(), 1);
    assert!(
        landed[0]
            .description
            .starts_with(&format!("{pre}\n\n来源: flow tpl/EXT-1")),
        "前导必须在 description 头部且紧跟来源行: {}",
        landed[0].description
    );

    // 空白前导 = 不注入（description 无前导空段）。
    let mut inst2 = inst_with_steps("tpl2", &[("land", KIND_LAND, "", "")]);
    start_board_land(&store, &mut inst2, None, Some("   ")).unwrap();
    let landed2 = store
        .list_issues_by_origin("flow", "tpl2/EXT-1", 1)
        .unwrap();
    assert_eq!(landed2.len(), 1);
    assert!(
        landed2[0].description.starts_with("来源: flow tpl2/EXT-1"),
        "空白前导不得注入: {}",
        landed2[0].description
    );
    let _ = out;
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

#[cfg(feature = "cluster")]
#[test]
fn materialize_exec_target_declares_execution_surface() {
    // 执行面声明物化（改版）：`self` = 单机本地执行面字面量，
    // 恒等透传（消费端 dispatch_issue_for_flow 识别后走 dispatch_issue_local
    // ——一等公民，与集群是否装配无关）；显式节点名原样透传；声明钉住
    // 即走，无回退。
    assert_eq!(materialize_exec_target("self"), "self");
    assert_eq!(materialize_exec_target("SELF"), "SELF");
    assert_eq!(materialize_exec_target("jgl-worker"), "jgl-worker");
    // 空白在 args_get 层已过滤（None），此处只钉词法层 trim 语义。
    assert_eq!(materialize_exec_target("self "), "self");
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

/// §六①：模板映射契约 fetch→land 全链（构造模板；外循环域剥离批 C 0.2.0
/// connector_fetch 声明形态的历史镜像——0.3.0 自持批起 bugfix-flow 本体已换
/// plugin 自报步，平台对 connector_fetch 原语的接受度由本测试继续钉住）。
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
    let out = start_board_land(&store, &mut inst, None, None).unwrap();
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
    let second = start_board_land(&store, &mut inst, None, None).unwrap();
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
    // 一套前缀（`{{origin_meta.*}}`）必须 v1 旧单同样可渲染（
    // cancel 回帖 Retry 循环实证：只落 origin_id.* 时 meta 前缀模板
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

/// §六③：publish 声明契约——构造模板（外循环域剥离批 C 0.2.0 connector
/// 原语声明形态的历史镜像——0.3.0 自持批起 bugfix-flow 本体已换 plugin 引用
/// op；connector 原语臂的接受度由本测试继续钉住。字段名/取数期 token 渲染/
/// 缺省值 limit·page·20·5）。
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
    // 诊断：manifest 解析/校验错误现场化（解析失败 = 断言前可）。
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

/// 缺陷 A回归锁：enable/disable 只写 config.plugin.wasm.json
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
// 实例配置 token 装载期渲染（参数化批：{{config.*}} → schema
// 缺省值打底 + 开关文件运行值覆盖；条目级 token 不动）
// ---------------------------------------------------------------------------

/// scan_templates config 渲染夹具：写一个带 [config-schema] 的 panel 插件 +
/// 翻开开关。`fetch_args_json` 参数化 = 待渲染声明；`switch_config` = None
/// （纯缺省）或运行值 JSON 片段。
fn write_cfg_sample(ws: &std::path::Path, fetch_args_json: &str, switch_config: Option<&str>) {
    let dir = ws.join("plugins_wasm").join("cfg-sample");
    std::fs::create_dir_all(&dir).unwrap();
    let switch_body = match switch_config {
        Some(c) => format!(r#"{{ "enabled": true, "config": {c} }}"#),
        None => r#"{ "enabled": true }"#.to_string(),
    };
    std::fs::write(
        ws.join("config").join("config.plugin.wasm.json"),
        format!(r#"{{ "plugins": {{ "cfg-sample": {switch_body} }} }}"#),
    )
    .unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            r#"api-version = 1
slug = "cfg-sample"
kind = "panel"
name = "配置样例"
version = "0.1.0"

[config-schema]
owner = {{ desc = "属主", default = "def-owner" }}
repo = {{ desc = "仓库", default = "def-repo" }}

[[ui.page]]
route = "cfg"
title = "配置"

[[flow.template]]
id = "tpl"
interval-secs = 0
origin-type = "gitea"
origin-id = "{{{{connector}}}}/{{{{arg.owner}}}}/{{{{arg.repo}}}}#{{{{key}}}}"
origin-meta = {{ owner = "{{{{config.owner}}}}", repo = "{{{{config.repo}}}}" }}

[flow.template.on-transition]
op = "issue_comment"
args = {{ owner = "{{{{origin_meta.owner}}}}" }}

[[flow.template.steps]]
id = "fetch"
kind = "connector_fetch"
args = {{ connector = "gitea", args_json = '{fetch_args_json}' }}

[[flow.template.steps]]
id = "land"
kind = "board_land"
args = {{ project = "gitea/{{{{config.owner}}}}/{{{{config.repo}}}}" }}
"#
        ),
    )
    .unwrap();
}

#[test]
fn scan_renders_config_tokens_with_schema_defaults() {
    let ws = temp_ws("cfgscan");
    std::fs::create_dir_all(ws.join("config")).unwrap();
    write_cfg_sample(
        &ws,
        r#"{"owner": "{{config.owner}}", "repo": "{{config.repo}}"}"#,
        None,
    );
    let out = scan_templates(&ws.join("plugins_wasm"), &ws.join("config"));
    assert_eq!(out.len(), 1, "缺省渲染不该跳过模板: {out:?}");
    let t = &out[0].template;
    // 缺省值渲染进步 args（含 JSON 串内）与 origin-meta。
    let fetch = t.steps.iter().find(|s| s.kind == KIND_FETCH).unwrap();
    assert_eq!(
        fetch.args["args_json"],
        r#"{"owner": "def-owner", "repo": "def-repo"}"#
    );
    let land = t.steps.iter().find(|s| s.kind == KIND_LAND).unwrap();
    assert_eq!(land.args["project"], "gitea/def-owner/def-repo");
    assert_eq!(t.origin_meta["owner"], "def-owner");
    assert_eq!(t.origin_meta["repo"], "def-repo");
    // 条目级 token 原样保留（下游逐条目渲染——契约不是遗漏）。
    assert_eq!(
        t.origin_id,
        "{{connector}}/{{arg.owner}}/{{arg.repo}}#{{key}}"
    );
    assert_eq!(t.on_transition.args["owner"], "{{origin_meta.owner}}");
}

#[test]
fn scan_config_tokens_runtime_override_wins_over_default() {
    let ws = temp_ws("cfgover");
    std::fs::create_dir_all(ws.join("config")).unwrap();
    write_cfg_sample(
        &ws,
        r#"{"owner": "{{config.owner}}", "repo": "{{config.repo}}"}"#,
        Some(r#"{ "owner": "real-owner" }"#),
    );
    let out = scan_templates(&ws.join("plugins_wasm"), &ws.join("config"));
    assert_eq!(out.len(), 1);
    let t = &out[0].template;
    let fetch = t.steps.iter().find(|s| s.kind == KIND_FETCH).unwrap();
    // 运行值覆盖 owner；未覆盖的 repo 落缺省。
    assert_eq!(
        fetch.args["args_json"],
        r#"{"owner": "real-owner", "repo": "def-repo"}"#
    );
}

#[test]
fn scan_skips_template_with_undeclared_config_key() {
    let ws = temp_ws("cfgskip");
    std::fs::create_dir_all(ws.join("config")).unwrap();
    // 引用 schema 未声明的 nope 键 → 残留检查 Err → 模板诚实跳过。
    write_cfg_sample(&ws, r#"{"owner": "{{config.nope}}"}"#, None);
    let out = scan_templates(&ws.join("plugins_wasm"), &ws.join("config"));
    assert!(
        out.is_empty(),
        "未声明 config 键的模板必须跳过（不把字面量垃圾送 connector）: {out:?}"
    );
    // 直接断言渲染错误形态。
    let raw = std::fs::read_to_string(ws.join("plugins_wasm/cfg-sample/plugin.toml")).unwrap();
    let (m, _) = PluginManifest::parse(&raw).expect("shipped 形态必须可解析");
    let t = &m.flow.templates[0];
    let cfg = config_table(&m, None);
    let declared: std::collections::BTreeSet<String> = m.config_schema.keys().cloned().collect();
    assert!(render_config_tokens(t, &cfg, &declared).is_err());
}

#[test]
fn render_config_tokens_renders_when_config_but_keeps_step_tokens() {
    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND]);
    t.steps[0].when = "{{steps.s1.count}} > 0".to_string();
    t.steps[1].when = "{{config.owner}} == gitlab".to_string();
    t.steps[0]
        .args
        .insert("a".to_string(), "{{key}}/{{arg.x}}".to_string());
    let cfg = BTreeMap::from([("owner".to_string(), "gitlab".to_string())]);
    let declared = std::collections::BTreeSet::from(["owner".to_string()]);
    let rendered = render_config_tokens(&t, &cfg, &declared).unwrap();
    // when 参与装载期 config 渲染（config 值对实例静态）；steps 投影
    // （`{{steps.*}}`）不属 config 词表，原样保留归运行期。
    assert_eq!(rendered.steps[0].when, "{{steps.s1.count}} > 0");
    assert_eq!(rendered.steps[1].when, "gitlab == gitlab");
    // 条目级 token 原样。
    assert_eq!(rendered.steps[0].args["a"], "{{key}}/{{arg.x}}");
}

#[test]
fn render_config_tokens_declared_unset_renders_empty() {
    // 可选键契约（SC 套件 proxy / gerrit_base_url 二选一形态）：键已声明
    // 但未配置（无缺省无覆盖）= 空串，不再整模板拒绝；未声明 = 仍 Err。
    let mut t = tpl("t", &[KIND_FETCH]);
    t.steps[0]
        .args
        .insert("proxy".to_string(), "{{config.proxy}}".to_string());
    let cfg = BTreeMap::new();
    let declared = std::collections::BTreeSet::from(["proxy".to_string()]);
    let rendered = render_config_tokens(&t, &cfg, &declared).unwrap();
    assert_eq!(rendered.steps[0].args["proxy"], "");
    // 同一引用换未声明键集 = 残留 Err（诚实拒绝不宽和）。
    let undeclared = std::collections::BTreeSet::<String>::new();
    assert!(render_config_tokens(&t, &cfg, &undeclared).is_err());
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
    // 未知 id / 未知 field = 原样保留（诚实不猜，模板写错可）。
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
    // 已导入三单的去重键就是这个形态——模板声明必须逐字节复现。
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
fn writeback_circuit_opens_at_threshold() {
    // 连续失败 < 5 = 未熔断（继续重投）；达到 5 = 熔断停投；超限维持熔断。
    // 钉死 WRITEBACK_MAX_RETRIES = 5（对齐 board 派发 max_send_retries 缺省），
    // 改动阈值时此处必改——防魔数漂移。
    assert_eq!(WRITEBACK_MAX_RETRIES, 5);
    for n in 0..WRITEBACK_MAX_RETRIES {
        assert!(
            !writeback_circuit_open(n),
            "连续失败 {n} 次不应熔断（阈值 {WRITEBACK_MAX_RETRIES}）"
        );
    }
    assert!(writeback_circuit_open(WRITEBACK_MAX_RETRIES));
    assert!(writeback_circuit_open(WRITEBACK_MAX_RETRIES + 1));
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
// plugin 步骤（guest step， 批）：出参四态解析 + 入参投影 +
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
    // plugin 引用不作 fetch 的旧约束已被 plugin fetch 形态取代（见下）。
    assert!(validate_template(&tpl("t", &[KIND_FETCH, KIND_LAND, "plugin:x:y"])).is_ok());
}

#[test]
fn validate_template_plugin_fetch_forms() {
    // plugin fetch（自持样板批）：首步合法 plugin 引用 = guest
    // 专管取数——放行；与 connector_fetch 互斥（取数入口唯一）。
    assert!(validate_template(&tpl("t", &["plugin:stepdemo:fetch", KIND_LAND])).is_ok());
    // op 段允许冒号（专业参数化同款语法）。
    assert!(validate_template(&tpl("t", &["plugin:acme-jira:issue:fetch", KIND_LAND])).is_ok());
    // plugin fetch 首步 + connector_fetch 混用 = 拒（取数入口唯一）。
    assert!(
        validate_template(&tpl("t", &["plugin:stepdemo:fetch", KIND_FETCH, KIND_LAND])).is_err()
    );
    // 首步 plugin 引用坏形态照拒（宽放行只对合法引用）。
    assert!(validate_template(&tpl("t", &["plugin:stepdemo", KIND_LAND])).is_err());
    // plugin fetch 模板禁 connector_fetch 专管身份 token（{{connector}} 渲染
    // 空串 = 身份静默错位；{{arg.*}} 未知 token = 静默零实例）——装载期 loud 拒。
    let mut t = tpl("t", &["plugin:stepdemo:fetch", KIND_LAND]);
    t.origin_id = "{{connector}}/PROJ#{{key}}".to_string();
    assert!(validate_template(&t).is_err(), "{{connector}} 在 origin_id");
    let mut t = tpl("t", &["plugin:stepdemo:fetch", KIND_LAND]);
    t.origin_id = "jira/{{config.project_key}}#{{key}}".to_string();
    assert!(validate_template(&t).is_ok(), "{{config.*}}/{{key}} 合法");
    let mut t = tpl("t", &["plugin:stepdemo:fetch", KIND_LAND]);
    t.origin_meta
        .insert("issue_key".to_string(), "{{arg.jql}}".to_string());
    assert!(validate_template(&t).is_err(), "{{arg.*}} 在 origin_meta");
}

#[test]
fn plugin_fetch_entries_extraction() {
    // 出参契约：done 负载 {"entries":[...]} 键必带且为数组；条目原始形态
    // 透传（物化路径在 materialize_instances，不在此处）。
    let payload =
        serde_json::from_str::<Value>(r#"{"entries":[{"key":"A-1"},{"key":"A-2"}]}"#).unwrap();
    let entries = plugin_fetch_entries(&payload, "t").expect("entries 数组");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["key"], "A-1");
    // 缺 entries 键 = Err（guest 契约坏了必响，不落零条目静默）。
    let payload = serde_json::from_str::<Value>(r#"{"count":0}"#).unwrap();
    assert!(plugin_fetch_entries(&payload, "t").is_err());
    // entries 非数组 = Err。
    let payload = serde_json::from_str::<Value>(r#"{"entries":"nope"}"#).unwrap();
    assert!(plugin_fetch_entries(&payload, "t").is_err());
    // 空数组 = 合法（本轮无新条目）。
    let payload = serde_json::from_str::<Value>(r#"{"entries":[]}"#).unwrap();
    assert_eq!(plugin_fetch_entries(&payload, "t").unwrap().len(), 0);
}

#[test]
fn validate_template_report_plugin_reply_ref() {
    // report 回帖 plugin 臂（自持样板批）：connector =
    // plugin:<slug>:<op> 引用——op 段即引用组成，无需另声明 op。
    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND, KIND_REPORT]);
    let report = t.steps.last_mut().unwrap();
    report.args.insert(
        "connector".to_string(),
        "plugin:jira-fix:comment".to_string(),
    );
    assert!(validate_template(&t).is_ok(), "plugin 引用无需 op 声明");
    // 坏引用 loud 拒。
    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND, KIND_REPORT]);
    let report = t.steps.last_mut().unwrap();
    report
        .args
        .insert("connector".to_string(), "plugin:jira-fix".to_string());
    assert!(validate_template(&t).is_err(), "缺 op 段");
    // connector 原语形态照旧：带 connector 必须声明 op。
    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND, KIND_REPORT]);
    let report = t.steps.last_mut().unwrap();
    report
        .args
        .insert("connector".to_string(), "jira".to_string());
    assert!(validate_template(&t).is_err(), "原语缺 op");
    let report = t.steps.last_mut().unwrap();
    report
        .args
        .insert("op".to_string(), "issue_comment".to_string());
    assert!(validate_template(&t).is_ok(), "原语带 op 照旧");
}

#[test]
fn validate_template_gates_on_transition_plugin_refs() {
    // on_transition 回写路由（自持写通道批）：connector 原语
    // op（自由文本，远端裁决）与空 op（缺省无回写）照旧过；`plugin:` 引
    // 用形态必须过 parse_plugin_step_ref——非法引用 = 声明坏 loud 拒，
    // 装载期诚实跳过模板，不给运行期游标留永久重试黑洞。
    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND]);
    t.on_transition.op = "issue_comment".to_string();
    assert!(validate_template(&t).is_ok(), "connector 原语 op 照旧");

    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND]);
    t.on_transition.op.clear();
    assert!(validate_template(&t).is_ok(), "空 op（无回写）照旧");

    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND]);
    t.on_transition.op = "plugin:issue-import:note".to_string();
    assert!(validate_template(&t).is_ok(), "合法 plugin 引用");

    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND]);
    t.on_transition.op = "plugin:issue-import".to_string();
    assert!(validate_template(&t).is_err(), "缺 op 段");
    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND]);
    t.on_transition.op = "plugin:Bad_Slug:note".to_string();
    assert!(validate_template(&t).is_err(), "坏 slug");
    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND]);
    t.on_transition.op = "plugin:issue-import:has space".to_string();
    assert!(validate_template(&t).is_err(), "坏 op");
}

#[test]
fn shipped_issue_import_template_passes_validate_template() {
    // 入库模板 drift 锁（引擎半边，同 jira-fix/bugfix-flow 锁理由）：install
    // 期 manifest.validate 不含「恰好一个 fetch 且首步」引擎规则——那条在
    // gateway 装载期才跑，drift 到装载期才红太晚；此测试把 shipped 声明钉在
    // CI。0.2.0 自持形态锚点：fetch 打头（plugin: 自报步，非 connector_fetch
    // ——connector 复用已退役）+ origin-id 字面 gitea 前缀（禁
    // {{connector}}）。
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../plugins/wasm/issue-import/plugin.toml");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("shipped issue-import plugin.toml 不可读: {e}"));
    let (m, _) = nemesis_plugins_wasm::manifest::PluginManifest::parse(&raw)
        .expect("shipped 模板必须可解析");
    assert_eq!(m.flow.templates.len(), 1, "单模板");
    for t in &m.flow.templates {
        validate_template(t)
            .unwrap_or_else(|e| panic!("shipped 模板 {} 必须过引擎校验: {e}", t.id));
    }
    // 轻量形态锚点：fetch→land 两步。
    let t = &m.flow.templates[0];
    assert_eq!(t.steps[0].kind, "plugin:issue-import:fetch");
    assert_eq!(t.steps[1].kind, KIND_LAND);
    assert!(
        t.origin_id.starts_with("gitea/") && !t.origin_id.contains("{{connector}}"),
        "origin-id 字面 gitea 前缀（plugin fetch 模板禁 {{connector}}/{{arg.*}}）"
    );
}

#[test]
fn shipped_jira_fix_template_passes_validate_template() {
    // 入库模板 drift 锁（引擎半边，同 issue-import 锁理由）：五步
    // jira-pipeline（无 publish）必须过引擎规则——「恰好一个 fetch 且首步」
    // / origin_id 必声明 / report 带回写协议等在 gateway 装载期才跑。
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../plugins/wasm/jira-fix/plugin.toml");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("shipped jira-fix plugin.toml 不可读: {e}"));
    let (m, _) = nemesis_plugins_wasm::manifest::PluginManifest::parse(&raw)
        .expect("shipped 模板必须可解析");
    assert_eq!(m.flow.templates.len(), 1, "单模板");
    for t in &m.flow.templates {
        validate_template(t)
            .unwrap_or_else(|e| panic!("shipped 模板 {} 必须过引擎校验: {e}", t.id));
    }
    // 五步形态锚点：fetch 打头（0.2.0 自持批 = plugin: 自报步，非
    // connector_fetch），report 收尾；fixloop 带人工闸。
    let t = &m.flow.templates[0];
    assert_eq!(t.steps[0].kind, "plugin:jira-fix:fetch");
    assert_eq!(t.steps[1].kind, KIND_LAND);
    assert_eq!(t.steps.last().unwrap().kind, "report");
    let fixloop = t
        .steps
        .iter()
        .find(|s| s.kind == "agent_fixloop")
        .expect("fixloop step");
    assert_eq!(fixloop.gate, "manual");
}

#[test]
fn shipped_bugfix_flow_template_passes_validate_template() {
    // 3b 自持批入库模板 drift 锁（引擎半边，同 jira-fix/issue-import 锁理由）：
    // 六步 bugfix-pipeline 必须过引擎规则；「恰好一个 fetch 且首步」等在
    // gateway 装载期才跑，drift 到装载期才红太晚。0.3.0 自持形态锚点：
    // fetch 打头（plugin: 自报步）+ report 收尾（connector = plugin 引用）
    // + 双人工闸（fixloop/publish）+ publish 双 op 全 plugin 引用（无
    // connector 原语键）+ origin-id 字面 gitea 前缀（禁 {{connector}}）。
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../plugins/wasm/bugfix-flow/plugin.toml");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("shipped bugfix-flow plugin.toml 不可读: {e}"));
    let (m, _) = nemesis_plugins_wasm::manifest::PluginManifest::parse(&raw)
        .expect("shipped 模板必须可解析");
    assert_eq!(m.flow.templates.len(), 1, "单模板");
    for t in &m.flow.templates {
        validate_template(t)
            .unwrap_or_else(|e| panic!("shipped 模板 {} 必须过引擎校验: {e}", t.id));
    }
    let t = &m.flow.templates[0];
    assert_eq!(t.steps[0].kind, "plugin:bugfix-flow:fetch");
    assert_eq!(t.steps[1].kind, KIND_LAND);
    assert_eq!(t.steps.last().unwrap().kind, "report");
    // 双人工闸：fixloop 与 publish。
    let fixloop = t
        .steps
        .iter()
        .find(|s| s.kind == "agent_fixloop")
        .expect("fixloop step");
    assert_eq!(fixloop.gate, "manual");
    let publish = t
        .steps
        .iter()
        .find(|s| s.kind == "publish")
        .expect("publish step");
    assert_eq!(publish.gate, "manual");
    // publish 双 op 全 plugin 引用（connector 原语键退役——0.3.0 自持形态）。
    assert!(
        !publish.args.contains_key("connector"),
        "publish 无 connector 键"
    );
    assert_eq!(
        publish.args.get("create_op").map(String::as_str),
        Some("plugin:bugfix-flow:create_pull")
    );
    assert_eq!(
        publish.args.get("reconcile_op").map(String::as_str),
        Some("plugin:bugfix-flow:pr_list")
    );
    // 回写与报告回帖 = guest comment op（plugin 引用）。
    assert_eq!(t.on_transition.op, "plugin:bugfix-flow:comment");
    let report = t.steps.last().unwrap();
    assert_eq!(
        report.args.get("connector").map(String::as_str),
        Some("plugin:bugfix-flow:comment")
    );
    // origin-id 字面 gitea 前缀（plugin fetch 模板禁 {{connector}}/{{arg.*}}）。
    assert!(
        t.origin_id.starts_with("gitea/") && !t.origin_id.contains("{{connector}}"),
        "origin-id 必须字面 gitea 前缀: {}",
        t.origin_id
    );
}

#[test]
fn shipped_jira_gitlab_flow_template_passes_validate_template() {
    // 入库模板 drift 锁（引擎半边，同 jira-fix/bugfix-flow 锁理由）：
    // 七步 jira-gitlab-pipeline（Jira 取数 + 宿主 repo_sync 仓副本 + GitLab MR
    // 发布）必须过引擎规则——「首步合法 plugin 引用且无 connector_fetch」/
    // origin_id 必声明 / publish 协议等在 gateway 装载期才跑。
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../plugins/wasm/jira-gitlab-flow/plugin.toml");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("shipped jira-gitlab-flow plugin.toml 不可读: {e}"));
    let (m, _) = nemesis_plugins_wasm::manifest::PluginManifest::parse(&raw)
        .expect("shipped 模板必须可解析");
    assert_eq!(m.flow.templates.len(), 1, "单模板");
    for t in &m.flow.templates {
        validate_template(t)
            .unwrap_or_else(|e| panic!("shipped 模板 {} 必须过引擎校验: {e}", t.id));
    }
    // 七步形态锚点：fetch 打头（plugin: 自报步）+ repo_sync 仓副本（宿主
    // 原语首用入库）+ report 收尾；唯一人工闸 = fixloop（publish 自动）。
    let t = &m.flow.templates[0];
    assert_eq!(t.steps[0].kind, "plugin:jira-gitlab-flow:fetch");
    assert_eq!(t.steps[1].kind, "repo_sync");
    assert_eq!(t.steps.last().unwrap().kind, "report");
    let fixloop = t
        .steps
        .iter()
        .find(|s| s.kind == "agent_fixloop")
        .expect("fixloop step");
    assert_eq!(fixloop.gate, "manual");
    // 执行面显式声明：target = self = 本节点一等执行面——
    // 单机形态不需要第二个节点；无回退（删声明才落匹配器阶梯）。
    assert_eq!(
        fixloop.args.get("target").map(String::as_str),
        Some("self"),
        "fixloop 必须声明本节点执行面"
    );
    let publish = t
        .steps
        .iter()
        .find(|s| s.kind == "publish")
        .expect("publish step");
    assert!(
        publish.gate.is_empty(),
        "publish 无 gate（发布动作自动，修复内容人审）"
    );
    assert_eq!(
        publish.args.get("create_op").map(String::as_str),
        Some("plugin:jira-gitlab-flow:create_mr")
    );
    assert_eq!(
        publish.args.get("reconcile_op").map(String::as_str),
        Some("plugin:jira-gitlab-flow:mr_list")
    );
    // 回写 = guest comment op；origin-id 字面 jira 前缀（禁 {{connector}}）。
    assert_eq!(t.on_transition.op, "plugin:jira-gitlab-flow:comment");
    assert!(
        t.origin_id.starts_with("jira/") && !t.origin_id.contains("{{connector}}"),
        "origin-id 必须字面 jira 前缀: {}",
        t.origin_id
    );
}

// --- BUG-20261108-031：步骤记账落盘失败不再完全静默（warn + 降级登记）。
// 修复前五处 `let _ = flows.save(inst)` 全吞——磁盘满/权限问题运维不可见，
// 断点续跑悄悄失效。创建路径（run_sync_all 的 `save(&inst)?`）错误传播
// 语义不变。 ---

/// save 必失败的工作区：root 挂在一个**已存在的文件**下面——
/// `template_dir` 的 `create_dir_all` 撞 ENOTDIR 跨平台稳定失败。
fn store_under_file(tag: &str) -> FlowStore {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "flow-engine-blocker-{}-{}-{tag}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&dir, b"blocker").expect("create blocker file");
    // FlowStore::new 在 workspace 上拼 logs/flows——这里手工构造同构实例：
    // root = <已有文件>/logs/flows，save 时 create_dir_all 必失败。
    FlowStore {
        root: dir.join("logs").join("flows"),
    }
}

#[test]
fn save_flow_step_failure_is_observed_not_silent() {
    let flows = store_under_file("observed");
    let inst = inst_with_steps("t-observed", &[("s1", "fetch", "", "")]);
    // 修复前：let _ = 全吞，无任何信号。修复后：warn + 降级登记。
    // 不 panic、不阻塞（尽力而为语义保持），且降级表出现 flow_engine 记录。
    save_flow_step(&flows, &inst);
    let snap = nemesis_utils::degraded::snapshot();
    let hit = snap.iter().find(|d| d.kind == "flow_engine");
    assert!(
        hit.is_some(),
        "save 失败必须进降级登记表（kind=flow_engine）"
    );
    let hit = hit.unwrap();
    assert!(
        hit.detail.contains("t-observed"),
        "登记 detail 必须带实例现场: {}",
        hit.detail
    );
}

#[test]
fn save_flow_step_success_leaves_no_degraded_entry() {
    // 对照：正常保存（真实临时工作区）不产生降级登记。
    let ws = temp_ws("happy-save");
    let flows = FlowStore::new(&ws);
    let inst = inst_with_steps("t-happy", &[("s1", "fetch", "", "")]);
    save_flow_step(&flows, &inst);
    assert!(
        !nemesis_utils::degraded::snapshot()
            .iter()
            .any(|d| d.kind == "flow_engine" && d.detail.contains("t-happy")),
        "成功保存不得进降级登记表"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------------------------------------------------------------------------
// 引擎本体（真 BoardStore + TaskDb + 临时工作区；agent loop 槽置空 = 停机
// 窗口诚实报错路径。需要常驻 AgentLoop 的 connector 成功链路归 e2e）
// ---------------------------------------------------------------------------

/// 引擎夹具：独立临时树（base/ws 工作区、base/home 主目录、
/// base/plugins_wasm 载荷目录）。workspace.parent() 即 base——运行期总闸
/// 读 base/config.json，默认缺席 = 总闸开；总闸关的用例显式写 enabled=false。
/// FlowEngine 用结构体直构（测试是子模块）：plugins_dir 指向本用例临时目录，
/// 不碰全局解析面。
struct EngineFixture {
    base: PathBuf,
    ws: PathBuf,
    plugins_dir: PathBuf,
    engine: Arc<FlowEngine>,
    store: Arc<BoardStore>,
    flows: FlowStore,
}

fn make_engine(tag: &str, runtime_on: bool) -> EngineFixture {
    let base = temp_ws(tag);
    let ws = base.join("ws");
    let home = base.join("home");
    let plugins_dir = base.join("plugins_wasm");
    let cfg_dir = ws.join("config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::create_dir_all(&plugins_dir).unwrap();
    if !runtime_on {
        // 总闸关：workspace.parent() 的 config.json 声明 plugins.wasm 关。
        std::fs::write(
            base.join("config.json"),
            r#"{"plugins": {"wasm": {"enabled": false}}}"#,
        )
        .unwrap();
    }
    let store = Arc::new(BoardStore::open(&ws.join("board").join("board.db"), "NB").unwrap());
    let db = nemesis_taskstore::TaskDb::open(&home.join("tasks.db")).unwrap();
    #[cfg(feature = "cluster")]
    let cluster: ClusterOwned = None;
    #[cfg(not(feature = "cluster"))]
    let cluster: ClusterOwned = ();
    let engine = Arc::new(FlowEngine {
        agent_loop_ref: Arc::new(parking_lot::RwLock::new(None)),
        store: Arc::clone(&store),
        db,
        workspace: ws.clone(),
        home,
        plugins_dir: plugins_dir.clone(),
        config_dir: cfg_dir,
        estop: Arc::new(EstopState::new()),
        plugin_manager: None,
        cluster,
        flows: FlowStore::new(&ws),
        actions: tokio::sync::Mutex::new(()),
        templates: tokio::sync::Mutex::new(TemplatesCache {
            stamp: (0, 0),
            templates: Vec::new(),
        }),
        last_fetch: tokio::sync::Mutex::new(HashMap::new()),
        writeback_failures: std::sync::Mutex::new(HashMap::new()),
    });
    let flows = FlowStore::new(&ws);
    EngineFixture {
        base,
        ws,
        plugins_dir,
        engine,
        store,
        flows,
    }
}

/// 写一个最小可装载的插件声明（载荷目录形态同 scan 夹具）。非 panel kind
/// 必须带 wasm-sha256（guest 载荷强制）——`payload` 参数控制是否声明。
fn write_plugin_toml(
    plugins_dir: &Path,
    slug: &str,
    kind: &str,
    payload: bool,
    flow_section: &str,
) {
    let dir = plugins_dir.join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    let mut doc = format!(
        "api-version = 1\nslug = \"{slug}\"\nkind = \"{kind}\"\nname = \"{slug}\"\nversion = \"0.1.0\"\n"
    );
    if payload {
        doc.push_str(&format!("wasm-sha256 = \"{}\"\n", "ab".repeat(32)));
    }
    if kind == "panel" {
        doc.push_str(&format!(
            "\n[[ui.page]]\nroute = \"{slug}\"\ntitle = \"{slug}\"\n"
        ));
    }
    if !flow_section.is_empty() {
        doc.push('\n');
        doc.push_str(flow_section);
    }
    std::fs::write(dir.join("plugin.toml"), doc).unwrap();
}

/// 覆盖开关文件（整文件重写——各用例独立临时 config 目录）。
fn enable_switches(cfg_dir: &Path, body: &str) {
    std::fs::write(cfg_dir.join("config.plugin.wasm.json"), body).unwrap();
}

/// 引擎用面板模板声明（fetch 首步 + 可调 interval）。
fn panel_flow_section(interval_secs: u64) -> String {
    format!(
        "[[flow.template]]\nid = \"tpl\"\ninterval-secs = {interval_secs}\norigin-type = \"fe\"\norigin-id = \"fe/{{{{key}}}}\"\n\n[[flow.template.steps]]\nid = \"fetch\"\nkind = \"connector_fetch\"\n"
    )
}

#[tokio::test]
async fn answer_views_full_cycle() {
    let fx = make_engine("answer", true);
    write_plugin_toml(
        &fx.plugins_dir,
        "ans-sample",
        "panel",
        false,
        &panel_flow_section(0),
    );
    enable_switches(
        &fx.ws.join("config"),
        r#"{ "plugins": { "ans-sample": { "enabled": true } } }"#,
    );

    // gate 待确认 / 停车两类实例。
    let mut gate = inst_with_steps("tpl", &[("deliver", KIND_PUBLISH, "manual", "")]);
    gate.instance = "GA".to_string();
    fx.flows.save(&gate).unwrap();
    let mut parked = inst_with_steps("tpl", &[("a", KIND_LAND, "", "park_human")]);
    parked.instance = "PB".to_string();
    parked.parked = Some("失败".to_string());
    fx.flows.save(&parked).unwrap();

    let list = fx.engine.answer("tpl", "list", &json!({})).await.unwrap();
    assert_eq!(list["rows"].as_array().unwrap().len(), 2);
    let detail = fx
        .engine
        .answer("tpl", "detail", &json!({"instance": "GA"}))
        .await
        .unwrap();
    assert_eq!(detail["instance"], "GA");
    assert_eq!(detail["state"], "awaiting_gate");
    let tl = fx
        .engine
        .answer("tpl", "timeline", &json!({"instance": "PB"}))
        .await
        .unwrap();
    assert!(!tl["events"].as_array().unwrap().is_empty());

    let ok = fx
        .engine
        .answer("tpl", "confirm", &json!({"instance": "GA"}))
        .await
        .unwrap();
    assert_eq!(ok["ok"], true);
    assert!(fx.flows.load("tpl", "GA").unwrap().steps[0].gate_confirmed);
    let ok = fx
        .engine
        .answer("tpl", "resume", &json!({"instance": "PB"}))
        .await
        .unwrap();
    assert_eq!(ok["ok"], true);
    assert!(fx.flows.load("tpl", "PB").unwrap().parked.is_none());

    // 未知 view / 未知模板（含所属插件禁用语义）诚实拒绝。
    let err = fx
        .engine
        .answer("tpl", "nope", &json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("未知 flow view"), "{err}");
    let err = fx
        .engine
        .answer("ghost", "list", &json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "{err}");
}

#[tokio::test]
async fn answer_blocks_when_runtime_disabled() {
    let fx = make_engine("answer-off", false);
    let err = fx
        .engine
        .answer("tpl", "list", &json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("已关闭"), "{err}");
}

#[tokio::test]
async fn flow_bridge_delegates_to_answer() {
    let fx = make_engine("bridge", false);
    use nemesis_web::handlers::plugins_wasm::FlowEngineBridge as _;
    let bridge = FlowBridge::new(Arc::clone(&fx.engine));
    let err = bridge.invoke("tpl", "list", &json!({})).await.unwrap_err();
    assert!(err.contains("已关闭"), "{err}");
}

#[tokio::test]
async fn answer_fetch_missing_template_errors() {
    let fx = make_engine("fetchghost", true);
    let err = fx.engine.answer_fetch("ghost").await.unwrap_err();
    assert!(err.contains("流程模板不存在"), "{err}");
}

#[tokio::test]
async fn tick_is_noop_when_runtime_disabled() {
    let fx = make_engine("tick-off", false);
    fx.engine.tick().await.unwrap();
}

#[tokio::test]
async fn tick_without_templates_is_noop() {
    let fx = make_engine("tick-empty", true);
    fx.engine.tick().await.unwrap();
}

#[tokio::test]
async fn tick_fetch_due_advance_and_writeback_loops() {
    let fx = make_engine("tick", true);
    write_plugin_toml(
        &fx.plugins_dir,
        "tick-sample",
        "panel",
        false,
        &panel_flow_section(60),
    );
    // 只手动模板（interval 0）→ fetch 循环跳过；回写订阅模板但身份声明
    // 引用未知 token → 回写轮询诚实报错（tick 内 warn 不上抛）。
    write_plugin_toml(
        &fx.plugins_dir,
        "tick-idle",
        "panel",
        false,
        &panel_flow_section(0),
    );
    write_plugin_toml(
        &fx.plugins_dir,
        "tick-wb",
        "panel",
        false,
        "[[flow.template]]\nid = \"tpl\"\ninterval-secs = 0\norigin-type = \"fe\"\norigin-id = \"fe/{{arg.missing}}\"\n\n[flow.template.on-transition]\nop = \"issue_comment\"\n\n[[flow.template.steps]]\nid = \"fetch\"\nkind = \"connector_fetch\"\n",
    );
    enable_switches(
        &fx.ws.join("config"),
        r#"{ "plugins": { "tick-sample": { "enabled": true }, "tick-idle": { "enabled": true }, "tick-wb": { "enabled": true } } }"#,
    );
    // 第一轮：到期 → fetch（缺 connector 声明 → warn 不上抛）→ 推进/回写
    // 空转 → Ok。
    fx.engine.tick().await.unwrap();
    // 第二轮：模板缓存命中 + interval 闸未到期 → 零动作 Ok。
    fx.engine.tick().await.unwrap();
    // 手动 fetch 代答：命中模板并撞上声明校验（诚实报错）。
    let err = fx
        .engine
        .answer("tpl", "fetch", &json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("缺 connector/op 声明"), "{err}");
}

// ---------------------------------------------------------------------------
// fetch_template 声明校验 + connector/plugin 取数失败面
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fetch_template_declaration_validation_faces() {
    let fx = make_engine("fetch-decl", true);
    // 无步骤。
    let d0 = FlowTemplateDef {
        slug: "s".to_string(),
        template: tpl("f0", &[]),
    };
    let err = fx.engine.fetch_template(&d0).await.unwrap_err();
    assert!(err.contains("无步骤"), "{err}");
    // 首步不是取数入口。
    let d1 = FlowTemplateDef {
        slug: "s".to_string(),
        template: tpl("f1", &[KIND_LAND]),
    };
    let err = fx.engine.fetch_template(&d1).await.unwrap_err();
    assert!(err.contains("首步必须是"), "{err}");
    // 缺 connector/op 声明。
    let d2 = FlowTemplateDef {
        slug: "s".to_string(),
        template: tpl("f2", &[KIND_FETCH]),
    };
    let err = fx.engine.fetch_template(&d2).await.unwrap_err();
    assert!(err.contains("缺 connector/op"), "{err}");
    // args_json 非法 JSON。
    let mut d3 = FlowTemplateDef {
        slug: "s".to_string(),
        template: tpl("f3", &[KIND_FETCH]),
    };
    d3.template.steps[0].args = BTreeMap::from([
        ("connector".to_string(), "jira".to_string()),
        ("op".to_string(), "list".to_string()),
        ("args_json".to_string(), "{ not json".to_string()),
    ]);
    let err = fx.engine.fetch_template(&d3).await.unwrap_err();
    assert!(err.contains("args_json 非法"), "{err}");
    // 分页形态 args_json 必须是对象。
    let mut d4 = FlowTemplateDef {
        slug: "s".to_string(),
        template: tpl("f4", &[KIND_FETCH]),
    };
    d4.template.steps[0].args = BTreeMap::from([
        ("connector".to_string(), "jira".to_string()),
        ("op".to_string(), "list".to_string()),
        ("args_json".to_string(), "[]".to_string()),
        ("page_size".to_string(), "2".to_string()),
        ("limit_key".to_string(), "per_page".to_string()),
        ("page_key".to_string(), "p".to_string()),
    ]);
    let err = fx.engine.fetch_template(&d4).await.unwrap_err();
    assert!(err.contains("必须是对象"), "{err}");
}

#[tokio::test]
async fn fetch_template_connector_and_plugin_fetch_faces() {
    let fx = make_engine("fetch-conn", true);
    // 单页：connector 调用撞 agent loop 停机窗口 → 诚实 Err。
    let mut d = FlowTemplateDef {
        slug: "s".to_string(),
        template: tpl("fc", &[KIND_FETCH]),
    };
    d.template.steps[0].args = BTreeMap::from([
        ("connector".to_string(), "jira".to_string()),
        ("op".to_string(), "issues".to_string()),
    ]);
    let err = fx.engine.fetch_template(&d).await.unwrap_err();
    assert!(err.contains("agent loop 已停止"), "{err}");
    // 分页形态：第一页同窗口失败上抛。
    d.template.steps[0]
        .args
        .insert("page_size".to_string(), "2".to_string());
    let err = fx.engine.fetch_template(&d).await.unwrap_err();
    assert!(err.contains("agent loop 已停止"), "{err}");
    // plugin fetch：子系统未装配诚实失败。
    let dp = FlowTemplateDef {
        slug: "s".to_string(),
        template: tpl("fp", &["plugin:fe-plugin:fetch"]),
    };
    let err = fx.engine.fetch_template(&dp).await.unwrap_err();
    assert!(err.contains("子系统未装配"), "{err}");
}

// ---------------------------------------------------------------------------
// 推进器：when 条件步 / gate / 发起与观察全路径
// ---------------------------------------------------------------------------

#[tokio::test]
async fn advance_template_when_gate_and_completed_faces() {
    let fx = make_engine("advance", true);
    // 条件成立 → land 同 tick 完成（真 board 单）。
    let mut w1 = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);
    w1.instance = "W1".to_string();
    w1.steps[0].when = "1 == 1".to_string();
    fx.flows.save(&w1).unwrap();
    // 条件不成立 → Skipped 同 tick 推进。
    let mut w2 = inst_with_steps(
        "tpl",
        &[("land", KIND_LAND, "", ""), ("rep", KIND_REPORT, "", "")],
    );
    w2.instance = "W2".to_string();
    w2.steps[0].when = "1 == 2".to_string();
    fx.flows.save(&w2).unwrap();
    // 条件式畸形 → 失败停车（诚实不静默）。
    let mut w3 = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);
    w3.instance = "W3".to_string();
    w3.steps[0].when = "junk".to_string();
    fx.flows.save(&w3).unwrap();
    // 条件成立但 manual gate 未放行 → 原地等待。
    let mut w4 = inst_with_steps("tpl", &[("pub", KIND_PUBLISH, "manual", "")]);
    w4.instance = "W4".to_string();
    w4.steps[0].when = "1 == 1".to_string();
    fx.flows.save(&w4).unwrap();
    // 无条件 gate 未确认 → 原地等待。
    let mut w5 = inst_with_steps("tpl", &[("pub", KIND_PUBLISH, "manual", "")]);
    w5.instance = "W5".to_string();
    fx.flows.save(&w5).unwrap();
    // 已完成 / 停车实例跳过。
    let mut w6 = inst_with_steps("tpl", &[("done", KIND_LAND, "", "")]);
    w6.instance = "W6".to_string();
    w6.steps[0].status = StepStatus::Ok;
    fx.flows.save(&w6).unwrap();
    let mut w7 = inst_with_steps("tpl", &[("p", KIND_LAND, "", "")]);
    w7.instance = "W7".to_string();
    w7.parked = Some("x".to_string());
    fx.flows.save(&w7).unwrap();
    // 观察步 Pending（无 gate）→ 发起转 Running（等后续 tick 观察）。
    let mut w8 = inst_with_steps("tpl", &[("reg", KIND_REGRESS, "", "")]);
    w8.instance = "W8".to_string();
    fx.flows.save(&w8).unwrap();
    // land 带已注册项目归属 → 完成；带未注册项目 → 诚实失败停车。
    // （ext_key 各自独立——同 origin 幂等去重会遮蔽项目查找分支。）
    fx.store
        .create_project("adv-proj", "测试", None, "", "", None, None)
        .unwrap();
    let mut w9 = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);
    w9.instance = "W9".to_string();
    w9.ext_key = "EXT-9".to_string();
    w9.steps[0]
        .args
        .insert("project".to_string(), "adv-proj".to_string());
    fx.flows.save(&w9).unwrap();
    let mut w10 = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);
    w10.instance = "W10".to_string();
    w10.ext_key = "EXT-10".to_string();
    w10.steps[0]
        .args
        .insert("project".to_string(), "adv-ghost".to_string());
    fx.flows.save(&w10).unwrap();

    fx.engine.advance_template("tpl").await.unwrap();
    let w1 = fx.flows.load("tpl", "W1").unwrap();
    assert_eq!(w1.current_step, 1);
    assert_eq!(w1.steps[0].status, StepStatus::Ok);
    assert!(w1.steps[0].artifacts["board_issue"].as_str().is_some());
    let w2 = fx.flows.load("tpl", "W2").unwrap();
    assert_eq!(w2.steps[0].status, StepStatus::Skipped);
    assert_eq!(w2.current_step, 1);
    assert_eq!(w2.steps[0].artifacts["skipped_when"], "1 == 2");
    let w3 = fx.flows.load("tpl", "W3").unwrap();
    assert_eq!(w3.steps[0].status, StepStatus::Failed);
    assert!(
        w3.parked
            .as_deref()
            .unwrap_or_default()
            .contains("条件式畸形")
    );
    let w4 = fx.flows.load("tpl", "W4").unwrap();
    assert_eq!(w4.current_step, 0);
    assert_eq!(w4.steps[0].status, StepStatus::Pending);
    let w5 = fx.flows.load("tpl", "W5").unwrap();
    assert_eq!(w5.current_step, 0);
    // W6/W7 未被推动。
    let w6 = fx.flows.load("tpl", "W6").unwrap();
    assert_eq!(w6.current_step, 0);
    let w8 = fx.flows.load("tpl", "W8").unwrap();
    assert_eq!(w8.steps[0].status, StepStatus::Running);
    assert!(w8.steps[0].started_at_ms.is_some());
    let w9 = fx.flows.load("tpl", "W9").unwrap();
    assert_eq!(w9.steps[0].status, StepStatus::Ok);
    let w10 = fx.flows.load("tpl", "W10").unwrap();
    assert!(
        w10.parked
            .as_deref()
            .unwrap_or_default()
            .contains("项目不存在")
    );
}

#[tokio::test]
async fn start_step_unknown_and_repo_sync_validation_park() {
    let fx = make_engine("start", true);
    // 未知 kind 不可由推进器启动。
    let mut u = inst_with_steps("tpl", &[("m", "mystery", "", "")]);
    u.instance = "U".to_string();
    fx.flows.save(&u).unwrap();
    // repo_sync 缺 project。
    let mut r1 = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    r1.instance = "R1".to_string();
    fx.flows.save(&r1).unwrap();
    // repo_sync 缺 url。
    let mut r2 = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    r2.instance = "R2".to_string();
    r2.steps[0]
        .args
        .insert("project".to_string(), "x".to_string());
    fx.flows.save(&r2).unwrap();
    // fixloop：Backlog + 派发不成行 → 停车。
    let _issue = board_issue_for(&fx.store, "修", "ST-1");
    let mut f1 = inst_with_steps("tpl", &[("fix", KIND_FIXLOOP, "", "")]);
    f1.instance = "F1".to_string();
    f1.ext_key = "ST-1".to_string();
    fx.flows.save(&f1).unwrap();
    // fixloop：Blocked 单不可派发（停车/取消单不自动续跑）。
    let issue2 = board_issue_for(&fx.store, "修二", "ST-2");
    fx.store
        .transition_issue(
            issue2.id,
            nemesis_board::models::IssueStatus::Blocked,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    let mut f2 = inst_with_steps("tpl", &[("fix", KIND_FIXLOOP, "", "")]);
    f2.instance = "F2".to_string();
    f2.ext_key = "ST-2".to_string();
    fx.flows.save(&f2).unwrap();
    // fixloop：单已在执行中 → 观察态直通发起（转 Running 等观察）。
    let issue3 = board_issue_for(&fx.store, "修三", "ST-3");
    fx.store
        .transition_issue(
            issue3.id,
            nemesis_board::models::IssueStatus::InProgress,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    let mut f3 = inst_with_steps("tpl", &[("fix", KIND_FIXLOOP, "", "")]);
    f3.instance = "F3".to_string();
    f3.ext_key = "ST-3".to_string();
    fx.flows.save(&f3).unwrap();

    fx.engine.advance_template("tpl").await.unwrap();
    let u = fx.flows.load("tpl", "U").unwrap();
    assert!(
        u.parked
            .as_deref()
            .unwrap_or_default()
            .contains("不可由推进器启动")
    );
    let r1 = fx.flows.load("tpl", "R1").unwrap();
    assert!(
        r1.parked
            .as_deref()
            .unwrap_or_default()
            .contains("缺 project")
    );
    let r2 = fx.flows.load("tpl", "R2").unwrap();
    assert!(r2.parked.as_deref().unwrap_or_default().contains("缺 url"));
    let f1 = fx.flows.load("tpl", "F1").unwrap();
    assert!(
        f1.parked
            .as_deref()
            .unwrap_or_default()
            .contains("派发未成行")
    );
    let f2 = fx.flows.load("tpl", "F2").unwrap();
    assert!(
        f2.parked
            .as_deref()
            .unwrap_or_default()
            .contains("不可派发")
    );
    let f3 = fx.flows.load("tpl", "F3").unwrap();
    assert_eq!(f3.steps[0].status, StepStatus::Running);
    assert!(f3.parked.is_none());
}

#[tokio::test]
async fn poll_step_observation_completion_and_restart() {
    let fx = make_engine("poll", true);
    let actor = nemesis_board::Actor::system("tests");

    // PL1 fixloop 执行中 → StillRunning 静默保 Running。
    let issue1 = board_issue_for(&fx.store, "修1", "PL-1");
    fx.store
        .transition_issue(
            issue1.id,
            nemesis_board::models::IssueStatus::InProgress,
            &actor,
        )
        .unwrap();
    let mut p1 = inst_with_steps("tpl", &[("fix", KIND_FIXLOOP, "", "")]);
    p1.instance = "PL1".to_string();
    p1.ext_key = "PL-1".to_string();
    p1.steps[0].status = StepStatus::Running;
    fx.flows.save(&p1).unwrap();
    // PL2 fixloop InReview → 工作交付完成。
    let issue2 = board_issue_for(&fx.store, "修2", "PL-2");
    fx.store
        .transition_issue(
            issue2.id,
            nemesis_board::models::IssueStatus::InProgress,
            &actor,
        )
        .unwrap();
    fx.store
        .transition_issue(
            issue2.id,
            nemesis_board::models::IssueStatus::InReview,
            &actor,
        )
        .unwrap();
    let mut p2 = inst_with_steps("tpl", &[("fix", KIND_FIXLOOP, "", "")]);
    p2.instance = "PL2".to_string();
    p2.ext_key = "PL-2".to_string();
    p2.steps[0].status = StepStatus::Running;
    fx.flows.save(&p2).unwrap();
    // PL3 regress Done → 验收成立。
    let issue3 = board_issue_for(&fx.store, "修3", "PL-3");
    fx.store
        .transition_issue(
            issue3.id,
            nemesis_board::models::IssueStatus::InProgress,
            &actor,
        )
        .unwrap();
    fx.store
        .transition_issue(
            issue3.id,
            nemesis_board::models::IssueStatus::InReview,
            &actor,
        )
        .unwrap();
    fx.store
        .transition_issue(issue3.id, nemesis_board::models::IssueStatus::Done, &actor)
        .unwrap();
    let mut p3 = inst_with_steps("tpl", &[("reg", KIND_REGRESS, "", "")]);
    p3.instance = "PL3".to_string();
    p3.ext_key = "PL-3".to_string();
    p3.steps[0].status = StepStatus::Running;
    fx.flows.save(&p3).unwrap();
    // PL4 publish 无派发记录 → 停车。
    let _issue4 = board_issue_for(&fx.store, "发布", "PL-4");
    let mut p4 = inst_with_steps("tpl", &[("pub", KIND_PUBLISH, "", "")]);
    p4.instance = "PL4".to_string();
    p4.ext_key = "PL-4".to_string();
    p4.steps[0].status = StepStatus::Running;
    fx.flows.save(&p4).unwrap();
    // PL5 land Running → 幂等重跑（重启恢复路径）同 tick 完成。
    let mut p5 = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);
    p5.instance = "PL5".to_string();
    p5.steps[0].status = StepStatus::Running;
    fx.flows.save(&p5).unwrap();
    // PL6 未知 kind 观察 → 诚实失败。
    let mut p6 = inst_with_steps("tpl", &[("m", "mystery", "", "")]);
    p6.instance = "PL6".to_string();
    p6.steps[0].status = StepStatus::Running;
    fx.flows.save(&p6).unwrap();
    // PL7/PL8 取数身份找不到 board 单 → 观察步诚实失败（fixloop / regress）。
    let mut p7 = inst_with_steps("tpl", &[("fix", KIND_FIXLOOP, "", "")]);
    p7.instance = "PL7".to_string();
    p7.ext_key = "NOPE-7".to_string();
    p7.steps[0].status = StepStatus::Running;
    fx.flows.save(&p7).unwrap();
    let mut p8 = inst_with_steps("tpl", &[("reg", KIND_REGRESS, "", "")]);
    p8.instance = "PL8".to_string();
    p8.ext_key = "NOPE-8".to_string();
    p8.steps[0].status = StepStatus::Running;
    fx.flows.save(&p8).unwrap();

    fx.engine.advance_template("tpl").await.unwrap();
    let p1 = fx.flows.load("tpl", "PL1").unwrap();
    assert_eq!(p1.steps[0].status, StepStatus::Running);
    assert!(p1.parked.is_none());
    let p2 = fx.flows.load("tpl", "PL2").unwrap();
    assert_eq!(p2.steps[0].status, StepStatus::Ok);
    assert_eq!(p2.steps[0].artifacts["delivered"], true);
    assert_eq!(p2.current_step, 1);
    let p3 = fx.flows.load("tpl", "PL3").unwrap();
    assert_eq!(p3.steps[0].artifacts["verdict"], "accepted");
    let p4 = fx.flows.load("tpl", "PL4").unwrap();
    assert!(
        p4.parked
            .as_deref()
            .unwrap_or_default()
            .contains("无派发记录")
    );
    let p5 = fx.flows.load("tpl", "PL5").unwrap();
    assert_eq!(p5.steps[0].status, StepStatus::Ok);
    assert!(p5.steps[0].artifacts["board_issue"].as_str().is_some());
    let p6 = fx.flows.load("tpl", "PL6").unwrap();
    assert!(
        p6.parked
            .as_deref()
            .unwrap_or_default()
            .contains("未知 step kind")
    );
    let p7 = fx.flows.load("tpl", "PL7").unwrap();
    assert!(
        p7.parked
            .as_deref()
            .unwrap_or_default()
            .contains("未落 board 单")
    );
    let p8 = fx.flows.load("tpl", "PL8").unwrap();
    assert!(
        p8.parked
            .as_deref()
            .unwrap_or_default()
            .contains("未落 board 单")
    );
}

#[tokio::test]
async fn poll_step_publish_with_dispatch_hits_stopped_agent_loop() {
    let fx = make_engine("pubpoll", true);
    let issue = board_issue_for(&fx.store, "发布", "PD-1");
    fx.store
        .insert_dispatch(
            "task-1",
            issue.id,
            "worker-1",
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    let mut p = inst_with_steps("tpl", &[("pub", KIND_PUBLISH, "", "")]);
    p.instance = "PD1".to_string();
    p.ext_key = "PD-1".to_string();
    p.steps[0].status = StepStatus::Running;
    p.steps[0].args = BTreeMap::from([
        ("create_op".to_string(), "create_pull".to_string()),
        ("create_args".to_string(), r#"{"head":"h"}"#.to_string()),
        ("result_ref".to_string(), "$.number".to_string()),
        ("reconcile_op".to_string(), "pr_list".to_string()),
        (
            "reconcile_args".to_string(),
            r#"{"state":"all"}"#.to_string(),
        ),
        ("reconcile_match".to_string(), "$.head.ref == h".to_string()),
        ("connector".to_string(), "gitea".to_string()),
        ("owner".to_string(), "acme".to_string()),
        ("repo".to_string(), "durfmt".to_string()),
    ]);
    fx.flows.save(&p).unwrap();

    fx.engine.advance_template("tpl").await.unwrap();
    let p = fx.flows.load("tpl", "PD1").unwrap();
    assert!(
        p.parked
            .as_deref()
            .unwrap_or_default()
            .contains("agent loop 已停止"),
        "parked: {:?}",
        p.parked
    );
    // 协议声明面已消费：产物未推进（停车在发布执行前）。
    assert_eq!(p.current_step, 0);
}

#[tokio::test]
async fn poll_step_report_export_and_reply_faces() {
    let fx = make_engine("report", true);
    // R1 纯导出 → 完成 + report_path 产物。派发记录在场 → dossier usage
    // 闭包真实求值。
    let issue1 = board_issue_for(&fx.store, "报告", "RP-1");
    fx.store
        .insert_dispatch(
            "task-rp1",
            issue1.id,
            "worker-1",
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
    let mut r1 = inst_with_steps("tpl", &[("rep", KIND_REPORT, "", "")]);
    r1.instance = "R1".to_string();
    r1.ext_key = "RP-1".to_string();
    r1.steps[0].status = StepStatus::Running;
    fx.flows.save(&r1).unwrap();
    // R2 connector 回帖 → agent loop 停机 → 报告已导出但回帖失败。
    let issue2 = board_issue_for(&fx.store, "报告2", "RP-2");
    let mut r2 = inst_with_steps("tpl", &[("rep", KIND_REPORT, "", "")]);
    r2.instance = "R2".to_string();
    r2.ext_key = "RP-2".to_string();
    r2.steps[0].status = StepStatus::Running;
    r2.steps[0].args = BTreeMap::from([
        ("connector".to_string(), "jira".to_string()),
        ("op".to_string(), "issue_comment".to_string()),
        (
            "args_json".to_string(),
            r#"{"body":"done {{report_path}}"}"#.to_string(),
        ),
    ]);
    fx.flows.save(&r2).unwrap();
    // R3 plugin 引用回帖 + 子系统未装配。
    let issue3 = board_issue_for(&fx.store, "报告3", "RP-3");
    let mut r3 = inst_with_steps("tpl", &[("rep", KIND_REPORT, "", "")]);
    r3.instance = "R3".to_string();
    r3.ext_key = "RP-3".to_string();
    r3.steps[0].status = StepStatus::Running;
    r3.steps[0].args.insert(
        "connector".to_string(),
        "plugin:fe-plugin:comment".to_string(),
    );
    fx.flows.save(&r3).unwrap();
    // R4 plugin 引用非法。
    let issue4 = board_issue_for(&fx.store, "报告4", "RP-4");
    let mut r4 = inst_with_steps("tpl", &[("rep", KIND_REPORT, "", "")]);
    r4.instance = "R4".to_string();
    r4.ext_key = "RP-4".to_string();
    r4.steps[0].status = StepStatus::Running;
    r4.steps[0]
        .args
        .insert("connector".to_string(), "plugin:bad".to_string());
    fx.flows.save(&r4).unwrap();

    fx.engine.advance_template("tpl").await.unwrap();
    let r1 = fx.flows.load("tpl", "R1").unwrap();
    assert_eq!(r1.steps[0].status, StepStatus::Ok);
    assert!(
        r1.steps[0].artifacts["report_path"]
            .as_str()
            .unwrap_or_default()
            .contains("dossiers")
    );
    assert!(r1.steps[0].artifacts["issues"].as_i64().unwrap() >= 1);
    let _ = issue1;
    let r2 = fx.flows.load("tpl", "R2").unwrap();
    assert!(
        r2.parked
            .as_deref()
            .unwrap_or_default()
            .contains("报告已导出但回帖失败")
    );
    let _ = issue2;
    let r3 = fx.flows.load("tpl", "R3").unwrap();
    assert!(
        r3.parked.as_deref().unwrap_or_default().contains("未装配"),
        "parked: {:?}",
        r3.parked
    );
    let _ = issue3;
    let r4 = fx.flows.load("tpl", "R4").unwrap();
    assert!(
        r4.parked
            .as_deref()
            .unwrap_or_default()
            .contains("plugin 引用非法")
    );
    let _ = issue4;
}

#[tokio::test]
async fn plugin_step_without_subsystem_fails_honestly() {
    let fx = make_engine("plugstep", true);
    // Pending 发起 → 子系统未装配 → 停车。
    let mut p1 = inst_with_steps("tpl", &[("run", "plugin:fe-plugin:echo", "", "")]);
    p1.instance = "PG1".to_string();
    fx.flows.save(&p1).unwrap();
    // Running 轮询重投 → 同样诚实失败。
    let mut p2 = inst_with_steps("tpl", &[("run", "plugin:fe-plugin:echo", "", "")]);
    p2.instance = "PG2".to_string();
    p2.steps[0].status = StepStatus::Running;
    fx.flows.save(&p2).unwrap();

    fx.engine.advance_template("tpl").await.unwrap();
    let p1 = fx.flows.load("tpl", "PG1").unwrap();
    assert!(
        p1.parked
            .as_deref()
            .unwrap_or_default()
            .contains("投递失败")
    );
    assert!(p1.parked.as_deref().unwrap_or_default().contains("未装配"));
    let p2 = fx.flows.load("tpl", "PG2").unwrap();
    assert!(p2.parked.is_some());
}

#[tokio::test]
async fn repo_sync_clones_local_repository() {
    let fx = make_engine("reposync", true);
    // 本地源仓（git2 真仓夹具；ensure_repo 自带首笔基线 commit）。
    let src = fx.base.join("src-repo");
    std::fs::create_dir_all(&src).unwrap();
    nemesis_board::git_repo::ensure_repo(&src).unwrap();
    std::fs::write(src.join("README.md"), "seed").unwrap();
    let head = nemesis_board::git_repo::commit_worktree(&src, "seed")
        .unwrap()
        .expect("seed commit");
    // 项目注册：directory 指向尚未存在的目标目录。
    let dest = fx.base.join("repo-dest");
    fx.store
        .create_project(
            "fe-proj",
            "测试项目",
            None,
            "",
            "",
            Some(&dest.to_string_lossy()),
            None,
        )
        .unwrap();
    let dest2 = fx.base.join("repo-dest2");
    fx.store
        .create_project(
            "fe-cred",
            "凭据项目",
            None,
            "",
            "",
            Some(&dest2.to_string_lossy()),
            None,
        )
        .unwrap();
    // 无目录项目。
    fx.store
        .create_project("fe-nodir", "无目录", None, "", "", None, None)
        .unwrap();

    // OK：克隆成功（匿名，默认分支 → "(default)"）。
    let mut ok = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    ok.instance = "RSOK".to_string();
    ok.steps[0].args = BTreeMap::from([
        ("project".to_string(), "fe-proj".to_string()),
        ("url".to_string(), src.to_string_lossy().to_string()),
    ]);
    fx.flows.save(&ok).unwrap();
    // 凭据别名缺席 → 匿名降级 → 仍克隆成功。
    let mut cred = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    cred.instance = "RSCRED".to_string();
    cred.steps[0].args = BTreeMap::from([
        ("project".to_string(), "fe-cred".to_string()),
        ("url".to_string(), src.to_string_lossy().to_string()),
        ("cred_secret".to_string(), "plugin/fe/pat".to_string()),
    ]);
    fx.flows.save(&cred).unwrap();
    // 凭据别名在场且 vault 可解析 → 带凭据继续（本地克隆凭据只是挂件）。
    // vault 写入纪律：Linux 默认 argon2id，「缺文件自动建库」需口令 env
    //（无口令 = 诚实 Err 是生产正确语义）；先备 env 再写，值与
    // vault_runtime 测试统一（rt-test-pw），并发交错 set 同值竞态良性。
    crate::vault_runtime::test_restore_passphrase_env();
    let vault_path = nemesis_path::resolve_vault_path_in_workspace(&fx.ws);
    crate::vault_runtime::write_alias_at(&vault_path, "plugin/fe/pat", "pat-value").unwrap();
    // 空值别名 → 凭据解析空 → 匿名降级（诚实不猜）。
    crate::vault_runtime::write_alias_at(&vault_path, "plugin/fe/empty", "").unwrap();
    let dest3 = fx.base.join("repo-dest3");
    fx.store
        .create_project(
            "fe-vault",
            "vault 凭据项目",
            None,
            "",
            "",
            Some(&dest3.to_string_lossy()),
            None,
        )
        .unwrap();
    let mut cred_ok = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    cred_ok.instance = "RSVAULT".to_string();
    cred_ok.steps[0].args = BTreeMap::from([
        ("project".to_string(), "fe-vault".to_string()),
        ("url".to_string(), src.to_string_lossy().to_string()),
        ("cred_secret".to_string(), "plugin/fe/pat".to_string()),
        ("cred_user".to_string(), "bot".to_string()),
    ]);
    fx.flows.save(&cred_ok).unwrap();
    // 空值凭据别名 → 匿名降级 → 克隆成功。
    let dest4 = fx.base.join("repo-dest4");
    fx.store
        .create_project(
            "fe-empty",
            "空凭据项目",
            None,
            "",
            "",
            Some(&dest4.to_string_lossy()),
            None,
        )
        .unwrap();
    let mut cred_empty = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    cred_empty.instance = "RSEMPTY".to_string();
    cred_empty.steps[0].args = BTreeMap::from([
        ("project".to_string(), "fe-empty".to_string()),
        ("url".to_string(), src.to_string_lossy().to_string()),
        ("cred_secret".to_string(), "plugin/fe/empty".to_string()),
    ]);
    fx.flows.save(&cred_empty).unwrap();
    // 声明分支 dev（本地源仓先行建分支）→ 全新目录克隆后检出 → Checkout。
    nemesis_board::git_repo::branch_at_head(&src, "dev").unwrap();
    let dest5 = fx.base.join("repo-dest5");
    fx.store
        .create_project(
            "fe-br",
            "分支项目",
            None,
            "",
            "",
            Some(&dest5.to_string_lossy()),
            None,
        )
        .unwrap();
    let mut br = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    br.instance = "RSBR".to_string();
    br.steps[0].args = BTreeMap::from([
        ("project".to_string(), "fe-br".to_string()),
        ("url".to_string(), src.to_string_lossy().to_string()),
        ("branch".to_string(), "dev".to_string()),
    ]);
    fx.flows.save(&br).unwrap();
    // 远端不可达（无 TLS 后端 → https 传输直接报错）→ 同步失败停车。
    let mut bad = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    bad.instance = "RSBAD".to_string();
    bad.steps[0].args = BTreeMap::from([
        ("project".to_string(), "fe-proj".to_string()),
        ("url".to_string(), "https://nope.invalid/x.git".to_string()),
    ]);
    fx.flows.save(&bad).unwrap();
    // 同目录重跑（远端无新提交）→ UpToDate 零动作。
    let mut ok2 = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    ok2.instance = "RSOK2".to_string();
    ok2.steps[0].args = BTreeMap::from([
        ("project".to_string(), "fe-proj".to_string()),
        ("url".to_string(), src.to_string_lossy().to_string()),
    ]);
    fx.flows.save(&ok2).unwrap();
    // 项目不存在。
    let mut ghost = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    ghost.instance = "RSGHOST".to_string();
    ghost.steps[0]
        .args
        .insert("project".to_string(), "fe-ghost".to_string());
    ghost.steps[0]
        .args
        .insert("url".to_string(), src.to_string_lossy().to_string());
    fx.flows.save(&ghost).unwrap();
    // 项目无目录。
    let mut nodir = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    nodir.instance = "RSNODIR".to_string();
    nodir.steps[0]
        .args
        .insert("project".to_string(), "fe-nodir".to_string());
    nodir.steps[0]
        .args
        .insert("url".to_string(), src.to_string_lossy().to_string());
    fx.flows.save(&nodir).unwrap();

    fx.engine.advance_template("tpl").await.unwrap();
    let ok = fx.flows.load("tpl", "RSOK").unwrap();
    assert_eq!(
        ok.steps[0].status,
        StepStatus::Ok,
        "{:?}",
        ok.steps[0].error
    );
    assert_eq!(ok.steps[0].artifacts["sync_outcome"], "cloned");
    assert_eq!(ok.steps[0].artifacts["head"], head);
    assert_eq!(ok.steps[0].artifacts["branch"], "(default)");
    assert!(dest.join(".git").exists());
    let cred = fx.flows.load("tpl", "RSCRED").unwrap();
    assert_eq!(cred.steps[0].status, StepStatus::Ok);
    assert!(dest2.join(".git").exists());
    let cred_ok = fx.flows.load("tpl", "RSVAULT").unwrap();
    assert_eq!(cred_ok.steps[0].status, StepStatus::Ok);
    assert!(dest3.join(".git").exists());
    let cred_empty = fx.flows.load("tpl", "RSEMPTY").unwrap();
    assert_eq!(cred_empty.steps[0].status, StepStatus::Ok);
    assert!(dest4.join(".git").exists());
    let br = fx.flows.load("tpl", "RSBR").unwrap();
    assert_eq!(br.steps[0].artifacts["sync_outcome"], "checkout");
    assert_eq!(br.steps[0].artifacts["branch"], "dev");
    assert!(dest5.join(".git").exists());
    let ok2 = fx.flows.load("tpl", "RSOK2").unwrap();
    assert_eq!(ok2.steps[0].status, StepStatus::Ok);
    assert_eq!(ok2.steps[0].artifacts["sync_outcome"], "up_to_date");
    let bad = fx.flows.load("tpl", "RSBAD").unwrap();
    assert!(bad.parked.is_some(), "远端不可达必须停车: {:?}", bad.parked);
    let ghost = fx.flows.load("tpl", "RSGHOST").unwrap();
    assert!(
        ghost
            .parked
            .as_deref()
            .unwrap_or_default()
            .contains("项目不存在")
    );
    let nodir = fx.flows.load("tpl", "RSNODIR").unwrap();
    assert!(
        nodir
            .parked
            .as_deref()
            .unwrap_or_default()
            .contains("无目录")
    );

    // 远端推进一笔提交 → 重跑 = 可快进（本地分支推进到远端头）。
    std::fs::write(src.join("README.md"), "second").unwrap();
    let head2 = nemesis_board::git_repo::commit_worktree(&src, "second")
        .unwrap()
        .expect("second commit");
    let mut ff = inst_with_steps("tpl", &[("sync", KIND_REPO_SYNC, "", "")]);
    ff.instance = "RSFF".to_string();
    ff.steps[0].args = BTreeMap::from([
        ("project".to_string(), "fe-proj".to_string()),
        ("url".to_string(), src.to_string_lossy().to_string()),
    ]);
    fx.flows.save(&ff).unwrap();
    fx.engine.advance_template("tpl").await.unwrap();
    let ff = fx.flows.load("tpl", "RSFF").unwrap();
    assert_eq!(ff.steps[0].artifacts["sync_outcome"], "fast_forward");
    assert_eq!(ff.steps[0].artifacts["head"], head2);
}

// ---------------------------------------------------------------------------
// 回写轮询（poll_writebacks / compose_writeback / compose_plugin_writeback）
// ---------------------------------------------------------------------------

/// 回写模板 def：hook.op + hook.args + parse-origin-id + fetch connector。
fn wb_def(
    hook_op: &str,
    hook_args: BTreeMap<String, String>,
    parse: BTreeMap<String, String>,
    connector: Option<&str>,
) -> FlowTemplateDef {
    let mut fetch = FlowStep {
        id: "fetch".to_string(),
        kind: KIND_FETCH.to_string(),
        gate: String::new(),
        when: String::new(),
        on_fail: String::new(),
        args: BTreeMap::new(),
    };
    if let Some(c) = connector {
        fetch.args.insert("connector".to_string(), c.to_string());
    }
    FlowTemplateDef {
        slug: "fe-wb".to_string(),
        template: FlowTemplate {
            id: "wb".to_string(),
            origin_type: "fe".to_string(),
            origin_id: "fe/{{key}}".to_string(),
            steps: vec![fetch],
            on_transition: FlowTransition {
                op: hook_op.to_string(),
                args: hook_args,
                parse_origin_id: parse,
            },
            ..Default::default()
        },
    }
}

/// 回写订阅用 board 单（origin_type "fe"，origin_id "fe/<ext>"）。
fn wb_issue(
    store: &Arc<BoardStore>,
    ext: &str,
    meta: BTreeMap<String, String>,
) -> nemesis_board::Issue {
    store
        .create_issue(nemesis_board::models::NewIssue {
            title: format!("回写 {ext}"),
            description: String::new(),
            creator: nemesis_board::Actor::system("flow-engine-tests"),
            origin: Some(nemesis_board::models::TaskOrigin {
                origin_type: "fe".to_string(),
                origin_id: format!("fe/{ext}"),
                origin_meta: meta,
            }),
            ..Default::default()
        })
        .expect("create wb issue")
}

fn to_review(store: &Arc<BoardStore>, issue: &nemesis_board::Issue) {
    let actor = nemesis_board::Actor::system("tests");
    store
        .transition_issue(
            issue.id,
            nemesis_board::models::IssueStatus::InProgress,
            &actor,
        )
        .unwrap();
    store
        .transition_issue(
            issue.id,
            nemesis_board::models::IssueStatus::InReview,
            &actor,
        )
        .unwrap();
}

fn to_done(store: &Arc<BoardStore>, issue: &nemesis_board::Issue) {
    to_review(store, issue);
    store
        .transition_issue(
            issue.id,
            nemesis_board::models::IssueStatus::Done,
            &nemesis_board::Actor::system("tests"),
        )
        .unwrap();
}

#[tokio::test]
async fn poll_writebacks_adopt_noop_and_compose_bookkeeping() {
    let fx = make_engine("wb1", true);
    let def = wb_def(
        "issue_comment",
        BTreeMap::from([("body".to_string(), "状态 {{transition.status}}".to_string())]),
        BTreeMap::new(),
        Some("jira"),
    );
    // R5 孤儿闸前置：订阅面单必须有模板实例（A/C/D；B 靠 todo 状态天然跳过）。
    materialize_instances(
        &fx.flows,
        &def,
        &[
            json!({"key": "A"}),
            json!({"key": "C"}),
            json!({"key": "D"}),
        ],
    )
    .unwrap();
    // A：meta 在场 → adopt（静默收编）→ noop → done 后 compose。
    let a = wb_issue(
        &fx.store,
        "A",
        BTreeMap::from([("owner".to_string(), "acme".to_string())]),
    );
    to_review(&fx.store, &a);
    fx.engine.poll_writebacks(&def).await.unwrap();
    assert_eq!(
        fx.flows.load_transition_state("wb", "fe/A"),
        Some("in_review".to_string())
    );
    // 块作用域持锁（clippy await_holding_lock 不识别显式 drop——守卫视为
    // 活到作用域末；块收口语义等价且 lint 干净）。
    {
        let map = fx.engine.writeback_failures.lock().unwrap();
        assert!(map.is_empty(), "adopt 不计失败: {map:?}");
    }
    // 二轮：游标 == 当前 → noop。
    fx.engine.poll_writebacks(&def).await.unwrap();
    assert_eq!(
        fx.flows.load_transition_state("wb", "fe/A"),
        Some("in_review".to_string())
    );
    // 三轮：转 done → compose → connector 发送撞停机窗口 → Retried 计数 1。
    to_done(&fx.store, &a);
    fx.engine.poll_writebacks(&def).await.unwrap();
    {
        let map = fx.engine.writeback_failures.lock().unwrap();
        assert_eq!(map.get("wb/fe/A"), Some(&1), "map: {map:?}");
    }
    // R4 契约（ASC 批序 + Retried break）：持续失败的老单恒居批首，后续单
    // 要等它熔断让路（有界 5 tick）才可达——预熔断 A 给 C/D 段让位。
    fx.engine
        .writeback_failures
        .lock()
        .unwrap()
        .insert("wb/fe/A".to_string(), WRITEBACK_MAX_RETRIES);

    // B：todo 状态不订阅三态 → 零动作；无 origin 单同样跳过。
    let _b = wb_issue(&fx.store, "B", BTreeMap::new());
    let _plain = fx
        .store
        .create_issue(nemesis_board::models::NewIssue {
            title: "无 origin".to_string(),
            creator: nemesis_board::Actor::system("flow-engine-tests"),
            ..Default::default()
        })
        .unwrap();
    fx.engine.poll_writebacks(&def).await.unwrap();
    {
        let map = fx.engine.writeback_failures.lock().unwrap();
        assert!(!map.contains_key("wb/fe/B"));
    }

    // C：先 adopt（in_review），预置连续失败 4 次，转 done 触发 compose →
    // 计数到 5（达熔断线）；再轮询 → 停投不再累加。
    let c = wb_issue(
        &fx.store,
        "C",
        BTreeMap::from([("owner".to_string(), "acme".to_string())]),
    );
    to_review(&fx.store, &c);
    fx.engine.poll_writebacks(&def).await.unwrap();
    fx.engine
        .writeback_failures
        .lock()
        .unwrap()
        .insert("wb/fe/C".to_string(), WRITEBACK_MAX_RETRIES - 1);
    to_done(&fx.store, &c);
    fx.engine.poll_writebacks(&def).await.unwrap();
    assert_eq!(
        fx.engine.writeback_failures.lock().unwrap().get("wb/fe/C"),
        Some(&WRITEBACK_MAX_RETRIES)
    );
    fx.engine.poll_writebacks(&def).await.unwrap();
    assert_eq!(
        fx.engine.writeback_failures.lock().unwrap().get("wb/fe/C"),
        Some(&WRITEBACK_MAX_RETRIES),
        "熔断后停投不再累加"
    );

    // D：meta 在场但模板缺 connector 声明 → Retried（游标保留）。
    let d = wb_issue(
        &fx.store,
        "D",
        BTreeMap::from([("owner".to_string(), "acme".to_string())]),
    );
    let def_nc = wb_def("issue_comment", BTreeMap::new(), BTreeMap::new(), None);
    to_review(&fx.store, &d);
    fx.engine.poll_writebacks(&def_nc).await.unwrap();
    to_done(&fx.store, &d);
    fx.engine.poll_writebacks(&def_nc).await.unwrap();
    assert_eq!(
        fx.engine.writeback_failures.lock().unwrap().get("wb/fe/D"),
        Some(&1)
    );
    assert_eq!(
        fx.flows.load_transition_state("wb", "fe/D"),
        Some("in_review".to_string()),
        "compose 失败游标保留"
    );
}

#[tokio::test]
async fn poll_writebacks_permanent_skip_parse_retry_render_fail() {
    let fx = make_engine("wb2", true);
    let def_np = wb_def("issue_comment", BTreeMap::new(), BTreeMap::new(), None);
    // R5 孤儿闸前置：订阅面单必须有模板实例（P/R/F）。
    materialize_instances(
        &fx.flows,
        &def_np,
        &[
            json!({"key": "P"}),
            json!({"key": "R"}),
            json!({"key": "F"}),
        ],
    )
    .unwrap();
    // 永久跳过：先 adopt（in_review），转 done 触发 compose——无 origin_meta
    // 无 parse 声明 → 游标落盘收口（Advanced，不计失败）。
    let p = wb_issue(&fx.store, "P", BTreeMap::new());
    to_review(&fx.store, &p);
    fx.engine.poll_writebacks(&def_np).await.unwrap();
    to_done(&fx.store, &p);
    fx.engine.poll_writebacks(&def_np).await.unwrap();
    assert_eq!(
        fx.flows.load_transition_state("wb", "fe/P"),
        Some("done".to_string())
    );
    assert!(fx.engine.writeback_failures.lock().unwrap().is_empty());

    // 分解失败：无 meta + parse 表达式打不中 → Retry（游标保留计失败）。
    let r = wb_issue(&fx.store, "R", BTreeMap::new());
    to_review(&fx.store, &r);
    fx.engine.poll_writebacks(&def_np).await.unwrap();
    to_done(&fx.store, &r);
    let def_parse = wb_def(
        "issue_comment",
        BTreeMap::new(),
        BTreeMap::from([("num".to_string(), "after '#'".to_string())]),
        None,
    );
    fx.engine.poll_writebacks(&def_parse).await.unwrap();
    assert_eq!(
        fx.engine.writeback_failures.lock().unwrap().get("wb/fe/R"),
        Some(&1)
    );
    assert_eq!(
        fx.flows.load_transition_state("wb", "fe/R"),
        Some("in_review".to_string()),
        "compose 失败游标不推进"
    );
    // R4 契约（ASC 批序 + Retried break）：R 的解析失败恒居批首挡住 F 段
    // ——预熔断 R 给 F 段让位（同 wb1 A 段注记）。
    fx.engine
        .writeback_failures
        .lock()
        .unwrap()
        .insert("wb/fe/R".to_string(), WRITEBACK_MAX_RETRIES);

    // 渲染失败：meta 在场但 args 引用未知 token → Retry。
    let f = wb_issue(
        &fx.store,
        "F",
        BTreeMap::from([("owner".to_string(), "acme".to_string())]),
    );
    to_review(&fx.store, &f);
    fx.engine.poll_writebacks(&def_np).await.unwrap();
    to_done(&fx.store, &f);
    let def_render = wb_def(
        "issue_comment",
        BTreeMap::from([("body".to_string(), "{{bogus_token}}".to_string())]),
        BTreeMap::new(),
        None,
    );
    fx.engine.poll_writebacks(&def_render).await.unwrap();
    assert_eq!(
        fx.engine.writeback_failures.lock().unwrap().get("wb/fe/F"),
        Some(&1)
    );
}

#[tokio::test]
async fn poll_writebacks_plugin_route_faces() {
    let fx = make_engine("wb3", true);
    // 先 adopt（in_review），转 done 触发 compose。
    let def = wb_def(
        "plugin:fe-plugin:comment",
        BTreeMap::from([("body".to_string(), "x".to_string())]),
        BTreeMap::new(),
        None,
    );
    // R5 孤儿闸前置：订阅面单必须有模板实例（A/B）。
    materialize_instances(&fx.flows, &def, &[json!({"key": "A"}), json!({"key": "B"})]).unwrap();
    // 合法 plugin 引用 + 子系统未装配 → Retried。
    let a = wb_issue(
        &fx.store,
        "A",
        BTreeMap::from([("owner".to_string(), "acme".to_string())]),
    );
    to_review(&fx.store, &a);
    fx.engine.poll_writebacks(&def).await.unwrap();
    to_done(&fx.store, &a);
    fx.engine.poll_writebacks(&def).await.unwrap();
    assert_eq!(
        fx.engine.writeback_failures.lock().unwrap().get("wb/fe/A"),
        Some(&1)
    );
    assert_eq!(
        fx.flows.load_transition_state("wb", "fe/A"),
        Some("in_review".to_string())
    );
    // R4 契约（ASC 批序 + Retried break）：A 的子系统缺席失败恒居批首挡住
    // B 段——预熔断 A 给 B 段让位（同 wb1 A 段注记）。
    fx.engine
        .writeback_failures
        .lock()
        .unwrap()
        .insert("wb/fe/A".to_string(), WRITEBACK_MAX_RETRIES);

    // 引用形态非法 → Retried（装载期校验是声明面闸，运行期诚实保底）。
    let b = wb_issue(
        &fx.store,
        "B",
        BTreeMap::from([("owner".to_string(), "acme".to_string())]),
    );
    to_review(&fx.store, &b);
    fx.engine.poll_writebacks(&def).await.unwrap();
    to_done(&fx.store, &b);
    let def_bad = wb_def(
        "plugin:Bad",
        BTreeMap::from([("body".to_string(), "x".to_string())]),
        BTreeMap::new(),
        None,
    );
    fx.engine.poll_writebacks(&def_bad).await.unwrap();
    assert_eq!(
        fx.engine.writeback_failures.lock().unwrap().get("wb/fe/B"),
        Some(&1)
    );
}

#[tokio::test]
async fn poll_writebacks_declaration_faces() {
    let fx = make_engine("wb4", true);
    // 未订阅（hook.op 空）→ 直接 Ok。
    let def_idle = wb_def("", BTreeMap::new(), BTreeMap::new(), None);
    fx.engine.poll_writebacks(&def_idle).await.unwrap();
    // origin_id 声明缺失 → Err（身份映射必声明）。
    let mut t = tpl("wb", &[KIND_FETCH]);
    t.origin_id.clear();
    t.on_transition.op = "issue_comment".to_string();
    let def_no_origin = FlowTemplateDef {
        slug: "s".to_string(),
        template: t,
    };
    let err = fx.engine.poll_writebacks(&def_no_origin).await.unwrap_err();
    assert!(err.contains("origin_id"), "{err}");
    // 无步骤模板 → Err。
    let mut t2 = tpl("wb", &[]);
    t2.on_transition.op = "issue_comment".to_string();
    let def_no_steps = FlowTemplateDef {
        slug: "s".to_string(),
        template: t2,
    };
    let err = fx.engine.poll_writebacks(&def_no_steps).await.unwrap_err();
    assert!(err.contains("无步骤"), "{err}");
}

// ---------------------------------------------------------------------------
// FlowStore 投影缺口补面
// ---------------------------------------------------------------------------

#[test]
fn answer_detail_projection_fields() {
    let ws = temp_ws("detail");
    let flows = FlowStore::new(&ws);
    let mut inst = inst_with_steps(
        "tpl",
        &[
            ("land", KIND_LAND, "", ""),
            ("fix", KIND_FIXLOOP, "manual", ""),
        ],
    );
    complete_step(&flows, &mut inst, &json!({ "board_issue": "NB-3" }));
    inst.steps[1].status = StepStatus::Running;
    inst.steps[1].started_at_ms = Some(1_234_567);
    inst.gaps
        .push(json!({"step": "land", "error": "曾失败", "ts_ms": 5}));
    inst.parked = Some("等待人工".to_string());
    flows.save(&inst).unwrap();

    let out = flows.answer_detail("tpl", Some("I-1")).unwrap();
    assert_eq!(out["instance"], "I-1");
    assert_eq!(out["ext_key"], "EXT-1");
    assert_eq!(out["state"], "parked");
    assert_eq!(out["current_step"], "fix");
    assert_eq!(out["board_issue"], "NB-3");
    let steps = out["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["status"], "ok");
    assert_eq!(steps[0]["attempts"], 0);
    assert_eq!(steps[1]["status"], "running");
    assert_eq!(steps[1]["gate"], "manual");
    assert!(steps[1]["started_at"].as_str().unwrap_or_default().len() >= 10);
    assert!(steps[1]["artifacts"].is_object());
    assert_eq!(out["gaps"].as_array().unwrap().len(), 1);
    assert_eq!(out["parked"], "等待人工");
    // 缺省 = 最近更新的一条。
    assert!(flows.answer_detail("tpl", None).is_ok());
}

#[test]
fn answer_timeline_skip_gap_park_events() {
    let ws = temp_ws("timeline2");
    let flows = FlowStore::new(&ws);
    let mut inst = inst_with_steps(
        "tpl",
        &[
            ("a", KIND_LAND, "", ""),
            ("b", KIND_LAND, "", ""),
            ("c", KIND_LAND, "", "continue"),
            ("d", KIND_LAND, "", ""),
            ("e", KIND_LAND, "", ""),
        ],
    );
    complete_step(&flows, &mut inst, &json!({})); // a ok
    skip_step(&flows, &mut inst, "1 == 2"); // b skipped（条件式入产物）
    fail_step(&flows, &mut inst, "boom"); // c 缺口账 + 带隙续跑
    inst.parked = Some("等待人工".to_string());
    inst.steps[3].status = StepStatus::Skipped;
    inst.steps[3].artifacts = json!({"skipped_when": "0 == 1"});
    inst.steps[3].finished_at_ms = Some(999);
    // e：Pending 但带 finished 时间戳 → 非三态终态不出事件（词表外静默）。
    inst.steps[4].finished_at_ms = Some(12345);
    flows.save(&inst).unwrap();

    let out = flows.answer_timeline("tpl", Some("I-1")).unwrap();
    let events = out["events"].as_array().unwrap();
    assert!(
        events.iter().any(|e| e["state"] == "skipped"
            && e["detail"].as_str().unwrap_or_default().contains("0 == 1")),
        "skipped 事件缺条件式现场: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e["state"] == "error" && e["detail"] == "boom"),
        "缺口事件缺错误文本: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e["state"] == "pending" && e["detail"] == "等待人工"),
        "停车事件缺现场: {events:?}"
    );
}

#[test]
fn answer_confirm_rejects_running_step() {
    let ws = temp_ws("confirm2");
    let flows = FlowStore::new(&ws);
    let mut inst = inst_with_steps("tpl", &[("deliver", KIND_PUBLISH, "manual", "")]);
    inst.steps[0].status = StepStatus::Running;
    flows.save(&inst).unwrap();
    let err = flows.answer_confirm("tpl", Some("I-1")).unwrap_err();
    assert!(err.contains("非 Pending"), "{err}");
}

#[test]
fn list_all_missing_workspace_is_empty() {
    let ws = temp_ws("missing-dir");
    let flows = FlowStore::new(&ws);
    // logs/flows 目录缺席 → 空清单不炸。
    assert_eq!(flows.list_all("tpl").len(), 0);
    assert!(
        flows.answer_list("tpl").unwrap()["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn fail_step_past_cursor_is_noop() {
    let flows = FlowStore::new(&temp_ws("pastend"));
    let mut inst = inst_with_steps("tpl", &[("a", KIND_LAND, "", "")]);
    inst.current_step = 1; // 越界
    fail_step(&flows, &mut inst, "x");
    assert!(inst.parked.is_none());
    assert_eq!(inst.steps[0].status, StepStatus::Pending);
}

#[test]
fn skip_step_with_payload_keeps_guest_payload() {
    let flows = FlowStore::new(&temp_ws("skippl"));
    let mut inst = inst_with_steps("tpl", &[("a", KIND_LAND, "", ""), ("b", KIND_LAND, "", "")]);
    skip_step_with_payload(&flows, &mut inst, json!({"why": "已存在"}));
    assert_eq!(inst.steps[0].status, StepStatus::Skipped);
    assert_eq!(inst.steps[0].artifacts["why"], "已存在");
    assert!(inst.steps[0].finished_at_ms.is_some());
    assert_eq!(inst.current_step, 1);
    assert!(inst.parked.is_none());
    assert!(inst.gaps.is_empty());
}

// ---------------------------------------------------------------------------
// 纯函数缺口补面
// ---------------------------------------------------------------------------

#[test]
fn step_status_as_str_matches_disk_vocabulary() {
    assert_eq!(StepStatus::Pending.as_str(), "pending");
    assert_eq!(StepStatus::Running.as_str(), "running");
    assert_eq!(StepStatus::Ok.as_str(), "ok");
    assert_eq!(StepStatus::Failed.as_str(), "failed");
    assert_eq!(StepStatus::Skipped.as_str(), "skipped");
}

#[test]
fn poll_outcome_maps_three_states() {
    assert_eq!(poll_outcome(PollOutcome::Completed(json!(1))).unwrap(), 1);
    assert_eq!(
        poll_outcome(PollOutcome::Failed("e".to_string())).unwrap_err(),
        "e"
    );
    assert_eq!(
        poll_outcome(PollOutcome::StillRunning).unwrap_err(),
        FLOW_STILL_RUNNING
    );
}

#[test]
fn entry_scalar_json_path_forms() {
    let e = json!({
        "a": { "b": "嵌套" },
        "n": 7,
        "flag": true,
        "obj": { "x": 1 },
        "arr": [1],
    });
    assert_eq!(entry_scalar(&e, "$.a.b").as_deref(), Some("嵌套"));
    assert_eq!(entry_scalar(&e, "$.n").as_deref(), Some("7"));
    assert_eq!(entry_scalar(&e, "$.flag").as_deref(), Some("true"));
    // 对象/数组不配做键/标题/正文（诚实不猜）。
    assert_eq!(entry_scalar(&e, "$.obj"), None);
    assert_eq!(entry_scalar(&e, "$.arr"), None);
    assert_eq!(entry_scalar(&e, "$.ghost"), None);
    // 非 $. 前缀回落点路径。
    assert_eq!(entry_scalar(&e, "a.b").as_deref(), Some("嵌套"));
}

#[test]
fn salvage_json_objects_string_escapes() {
    // 转义引号不打断字符串态、反斜杠转义吞掉后续字符——两腿都不迷惑
    // 对象配对状态机。
    let (v, ok) = parse_entries_resilient(r#"[{"key":"a\"b"},{"path":"C:\\t"},{"key":"C"}"#);
    assert!(!ok);
    assert_eq!(v.len(), 3);
    assert_eq!(v[0]["key"], "a\"b");
    assert_eq!(v[2]["key"], "C");
}

#[test]
fn eval_split_expr_more_malformed_faces() {
    // 分隔子串缺引号包裹。
    assert!(eval_split_expr("a#b", "after #").is_err());
    // 空步骤（管道打头）。
    assert!(eval_split_expr("a", "|after '#'").is_err());
    // 未知步骤名。
    assert!(eval_split_expr("a#b", "frobnicate '#'").is_err());
}

#[test]
fn resolve_instance_origin_arg_table_forms() {
    // 非字符串 args_json 值标量化进 arg 表。
    let mut t = tpl("tpl", &[KIND_FETCH, KIND_LAND]);
    t.origin_type = "gitea".to_string();
    t.origin_id = "{{connector}}/{{arg.n}}/{{key}}".to_string();
    let step = fetch_step("gitea", r#"{"n": 3, "flag": true}"#);
    let (ot, oid, _) = resolve_instance_origin(&t, &step, "9").unwrap();
    assert_eq!(ot, "gitea");
    assert_eq!(oid, "gitea/3/9");
    // args_json 非法 → arg 表为空（origin 声明不含 arg token 时仍可解析）。
    let mut t2 = tpl("tpl", &[KIND_FETCH, KIND_LAND]);
    t2.origin_id = "{{connector}}/{{key}}".to_string();
    let step_bad = fetch_step("gitea", "{ not json");
    let (_, oid2, _) = resolve_instance_origin(&t2, &step_bad, "9").unwrap();
    assert_eq!(oid2, "gitea/9");
}

#[test]
fn parse_publish_step_protocol_plugin_ref_shapes() {
    let noop = |tpl: &str| tpl.to_string();
    let mut base: BTreeMap<String, String> = BTreeMap::from([
        ("create_op".to_string(), "plugin:Bad_Slug:op".to_string()),
        ("create_args".to_string(), r#"{"head":"h"}"#.to_string()),
        ("result_ref".to_string(), "$.number".to_string()),
        ("reconcile_op".to_string(), "pr_list".to_string()),
        (
            "reconcile_args".to_string(),
            r#"{"state":"all"}"#.to_string(),
        ),
        ("reconcile_match".to_string(), "$.head.ref == h".to_string()),
    ]);
    // plugin 引用形态坏 slug = 声明错误诚实报错。
    let err = {
        let get = |key: &str| base.get(key).cloned();
        parse_publish_step_protocol(&get, &noop).unwrap_err()
    };
    assert!(err.contains("plugin 引用形态但解析失败"), "{err}");
    // 合法引用过闸。
    base.insert(
        "create_op".to_string(),
        "plugin:bugfix-flow:create_pull".to_string(),
    );
    let get = |key: &str| base.get(key).cloned();
    let p = parse_publish_step_protocol(&get, &noop).unwrap();
    assert_eq!(p.create_op, "plugin:bugfix-flow:create_pull");
}

#[test]
fn config_token_names_and_missing_stamp_dirs() {
    // 未闭合 {{ 提前停（不 panic、不产键）。
    assert!(config_token_names("x {{config.a").is_empty());
    assert_eq!(
        config_token_names("{{config.a}} {{config.b}}"),
        vec!["a", "b"]
    );
    assert!(config_token_names("{{not_config.x}}").is_empty());
    // 目录缺席 → (0, 0)。
    let ws = temp_ws("stamp2");
    assert_eq!(template_stamp(&ws.join("absent"), &ws), (0, 0));
}

#[test]
fn validate_template_more_honest_rules() {
    // 模板 id 为空。
    assert!(validate_template(&tpl("", &[KIND_FETCH, KIND_LAND])).is_err());
    // publish 步缺必填协议键。
    assert!(validate_template(&tpl("t", &[KIND_FETCH, KIND_PUBLISH])).is_err());
    // repo_sync 缺 project/url。
    assert!(validate_template(&tpl("t", &[KIND_FETCH, KIND_REPO_SYNC])).is_err());
    // origin_id 未声明。
    let mut t = tpl("t", &[KIND_FETCH, KIND_LAND]);
    t.origin_id.clear();
    assert!(validate_template(&t).is_err());
}

#[test]
fn materialize_instances_filter_labels_and_origin_faces() {
    let ws = temp_ws("mat2");
    let flows = FlowStore::new(&ws);
    // filter 声明畸形 → 取数期整批 Err。
    let mut bad = FlowTemplateDef {
        slug: "s".to_string(),
        template: tpl("tpl", &[KIND_FETCH, KIND_LAND]),
    };
    bad.template.steps[0]
        .args
        .insert("filter".to_string(), "no-op".to_string());
    let err = materialize_instances(&flows, &bad, &[json!({"key": "A"})]).unwrap_err();
    assert!(err.contains("filter 声明畸形"), "{err}");

    // filter 不命中 / labels 不命中逐条跳过；命中条目正常物化。
    let mut def = FlowTemplateDef {
        slug: "s".to_string(),
        template: tpl("tpl", &[KIND_FETCH, KIND_LAND]),
    };
    def.template.steps[0].args = BTreeMap::from([
        ("filter".to_string(), "$.state == open".to_string()),
        ("labels".to_string(), "bug, ui".to_string()),
    ]);
    let entries = vec![
        json!({"key": "A", "state": "closed"}), // filter false
        json!({"key": "B", "state": "open"}),   // labels 不命中
        json!({"key": "C", "state": "open", "labels": [{"name": "ui"}]}),
    ];
    assert_eq!(materialize_instances(&flows, &def, &entries).unwrap(), 1);
    assert!(flows.exists("tpl", "C"));
    assert!(!flows.exists("tpl", "A"));
    assert!(!flows.exists("tpl", "B"));

    // 身份解析失败 → 该条目诚实跳过（整批不炸）。
    let mut broken = FlowTemplateDef {
        slug: "s".to_string(),
        template: tpl("tpl", &[KIND_FETCH, KIND_LAND]),
    };
    broken.template.origin_id = "{{connector}}/{{arg.owner}}#{{key}}".to_string();
    let created =
        materialize_instances(&flows, &broken, &[json!({"key": "D"}), json!({"key": "E"})])
            .unwrap();
    assert_eq!(created, 0);
    assert!(!flows.exists("tpl", "D"));
}

#[test]
fn scan_templates_skips_broken_payloads_and_templates() {
    let ws = temp_ws("scan2");
    let plugins = ws.join("plugins_wasm");
    let cfg = ws.join("config");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();
    // 开关开五家：载荷缺席 / 非 panel kind / 模板非法 / config 键未声明 /
    // 合法（含非字符串 config 值）。
    enable_switches(
        &cfg,
        r#"{
        "plugins": {
            "ghost": { "enabled": true },
            "tooly": { "enabled": true },
            "broken": { "enabled": true },
            "cfgbad": { "enabled": true },
            "good": { "enabled": true, "config": { "owner": 123 } }
        }
    }"#,
    );
    // tooly：tool kind 带 guest 载荷声明（可装载，被 kind 过滤不进清单）。
    write_plugin_toml(&plugins, "tooly", "tool", true, "");
    // broken：panel + 合法 kind 但 fetch 不在首步 → 引擎校验失败跳过。
    write_plugin_toml(
        &plugins,
        "broken",
        "panel",
        false,
        "[[flow.template]]\nid = \"tpl\"\ninterval-secs = 0\norigin-id = \"demo/{{key}}\"\n\n[[flow.template.steps]]\nid = \"land\"\nkind = \"board_land\"\n\n[[flow.template.steps]]\nid = \"fetch\"\nkind = \"connector_fetch\"\n",
    );
    // cfgbad：引用未声明的 config 键 → 装载期渲染失败跳过。
    write_plugin_toml(
        &plugins,
        "cfgbad",
        "panel",
        false,
        "[[flow.template]]\nid = \"tpl\"\ninterval-secs = 0\norigin-id = \"demo/{{key}}\"\n\n[[flow.template.steps]]\nid = \"fetch\"\nkind = \"connector_fetch\"\nargs = { owner = \"{{config.nope}}\" }\n",
    );
    // good：合法模板 + config-schema（运行值 123 标量化）。
    write_plugin_toml(
        &plugins,
        "good",
        "panel",
        false,
        "[config-schema]\nowner = { desc = \"属主\", default = \"\" }\n\n[[flow.template]]\nid = \"tpl\"\ninterval-secs = 0\norigin-type = \"gitea\"\norigin-id = \"gitea/{{key}}\"\n\n[[flow.template.steps]]\nid = \"fetch\"\nkind = \"connector_fetch\"\nargs = { project = \"gitea/{{config.owner}}\" }\n",
    );
    let out = scan_templates(&plugins, &cfg);
    assert_eq!(out.len(), 1, "只有 good 进清单: {out:?}");
    assert_eq!(out[0].slug, "good");
    // 非字符串 config 运行值标量化渲染进步 args。
    assert_eq!(out[0].template.steps[0].args["project"], "gitea/123");
}

// ---------------------------------------------------------------------------
// 装配与杂项缺口补面
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_engine_new_assembles_with_global_plugin_dir() {
    // 装配走真实构造器（plugins_dir 用全局解析面，测试环境落到 exe 旁）。
    let base = temp_ws("new");
    let ws = base.join("ws");
    let home = base.join("home");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let store = Arc::new(BoardStore::open(&ws.join("board").join("board.db"), "NB").unwrap());
    let db = nemesis_taskstore::TaskDb::open(&home.join("tasks.db")).unwrap();
    #[cfg(feature = "cluster")]
    let cluster: ClusterOwned = None;
    #[cfg(not(feature = "cluster"))]
    let cluster: ClusterOwned = ();
    let engine = Arc::new(FlowEngine::new(
        Arc::new(parking_lot::RwLock::new(None)),
        Arc::clone(&store),
        db,
        ws.clone(),
        home,
        Arc::new(EstopState::new()),
        None,
        cluster,
    ));
    // 模板清单为空 → 代答诚实拒绝（模板不存在）。
    let err = engine
        .answer("ghost", "list", &json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "{err}");
}

#[test]
fn parse_entries_scalar_shapes_error() {
    // 标量 JSON 形态诚实报错（既非数组也非可解包裹对象）。
    assert!(parse_entries("42").is_err());
    assert!(parse_entries("null").is_err());
    assert!(parse_entries("\"text\"").is_err());
}

#[test]
fn compare_str_string_less_than_and_extra_scalars() {
    // 字符串字典序 `<` 臂。
    assert!(eval_when("abc < done").unwrap());
    assert!(!eval_when("done < abc").unwrap());
    // extra 表非字符串值标量化（保留键与自由键两路）。
    let ws = temp_ws("extra-scalar");
    let flows = FlowStore::new(&ws);
    let mut inst = inst_with_steps("tpl", &[("land", KIND_LAND, "", "")]);
    complete_step(&flows, &mut inst, &json!({}));
    assert_eq!(substitute_tokens("[{{n}}]", &inst, &json!({"n": 7})), "[7]");
    assert_eq!(
        substitute_tokens("[{{board_issue}}]", &inst, &json!({"board_issue": 9})),
        "[9]"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn resolve_instance_origin_non_object_args_json() {
    // args_json 合法 JSON 但非对象 → arg 表为空（无 arg token 声明仍可解析）。
    let mut t = tpl("tpl", &[KIND_FETCH, KIND_LAND]);
    t.origin_id = "{{connector}}/{{key}}".to_string();
    let step = fetch_step("gitea", "\"x\"");
    let (_, oid, _) = resolve_instance_origin(&t, &step, "9").unwrap();
    assert_eq!(oid, "gitea/9");
}

#[test]
fn accounting_helpers_past_cursor_are_noop() {
    // 游标越界时记账函数不做任何步骤改动（防御面）。
    let flows = FlowStore::new(&temp_ws("oob"));
    let mut inst = inst_with_steps("tpl", &[("a", KIND_LAND, "", "")]);
    inst.current_step = 1;
    mark_running(&flows, &mut inst);
    complete_step(&flows, &mut inst, &json!({}));
    assert_eq!(inst.steps[0].status, StepStatus::Pending);
    assert!(inst.steps[0].artifacts.is_object());
}
