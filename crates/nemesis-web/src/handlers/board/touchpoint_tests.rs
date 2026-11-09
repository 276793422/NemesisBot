//! 人工触点 responder 漏斗 + sweep 测试（看板人工触点体系 P1）。
//!
//! 分层纪律：状态机/审计纯函数已在 `nemesis-board`（touchpoint/tests.rs）
//! 钉死，这里只测 **漏斗层**（web 侧组合语义）：respond 五动作、sweep
//! 三拍（提醒/默认收口/checkpoint 死等/停车通报）、取数面、workbench
//! handler 路由壳。
//!
//! estop 短路不在此测：BOARD_ESTOP OnceLock 由 gateway 装配、单测不装配
//! （既定先例「单测不装配零波及」），sweep 短路分支恒 false——真装配面
//! 归 e2e/集成层。

use std::sync::{Arc, Mutex};

use nemesis_board::BoardStore;
use nemesis_board::assignment::Actor;
use nemesis_board::models::{IssueStatus, NewIssue, TouchpointKind, TouchpointStatus};
use nemesis_config::BoardAttendanceConfig;

use super::tests::{make_ctx_with_board, unique_dir};
use super::touchpoints::{
    TouchpointResume, blocking_cap_ms, create_and_default_touchpoint, create_touchpoint_card,
    deadline_from_cfg, install_touchpoint_resume_hook, latest_rework_recheck, list_touchpoints,
    now_ms, resolve_unattended, respond_touchpoint, sweep_touchpoint_timeouts_with_config,
    touchpoint_workbench_payload,
};
use crate::handlers::workbench::WorkbenchHandler;
use crate::ws_router::{ModuleHandler, RequestContext};

// 与 touchpoints.rs 私有 REMIND_AFTER_MS 对齐（私有常量——测试侧硬编码 +
// 注释锚点；改漏斗侧时此处同步）。
const REMIND_AFTER_MS: i64 = 600_000;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn open_store(name: &str) -> (std::path::PathBuf, BoardStore) {
    let dir = unique_dir(name);
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    (dir, store)
}

/// 建单并推到 in_review（挂卡的前置形态：评审中列内置顶徽标的载体）。
fn in_review_issue(
    store: &BoardStore,
    title: &str,
    project_id: Option<i64>,
) -> nemesis_board::models::Issue {
    let actor = Actor::admin("test");
    let issue = store
        .create_issue(NewIssue {
            title: title.into(),
            project_id,
            creator: actor.clone(),
            ..Default::default()
        })
        .expect("create issue");
    store
        .transition_issue(issue.id, IssueStatus::InProgress, &actor)
        .expect("in_progress");
    store
        .transition_issue(issue.id, IssueStatus::InReview, &actor)
        .expect("in_review");
    store.get_issue(issue.id).expect("re-read issue")
}

/// 挂卡统一入口封装（deadline 显式传——sweep 测试要造过去/未来的钟）。
fn hang(
    store: &BoardStore,
    issue_id: i64,
    project_id: Option<i64>,
    kind: TouchpointKind,
    deadline_ms: i64,
) -> nemesis_board::models::Touchpoint {
    create_touchpoint_card(
        store,
        &Actor::admin("test"),
        issue_id,
        project_id,
        kind,
        r#"{"version":"v1","spec":"初版规格"}"#,
        deadline_ms,
    )
    .expect("hang card")
}

fn att_cfg() -> BoardAttendanceConfig {
    BoardAttendanceConfig::default()
}

fn att_cfg_checkpoint(cap_secs: u64) -> BoardAttendanceConfig {
    BoardAttendanceConfig {
        mode: "checkpoint".into(),
        blocking_cap_secs: cap_secs,
        ..Default::default()
    }
}

fn att_cfg_veto(veto_max_rounds: u32) -> BoardAttendanceConfig {
    BoardAttendanceConfig {
        veto_max_rounds,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// deadline 换算（纯函数）
// ---------------------------------------------------------------------------

#[test]
fn test_deadline_from_cfg_per_kind() {
    let cfg = att_cfg();
    let now = 1_700_000_000_000i64;
    // spec = 43200s 窗；prototype = 14400s 窗（毫秒换算）。
    assert_eq!(
        deadline_from_cfg(now, &cfg, TouchpointKind::Spec),
        now + 43_200 * 1000
    );
    assert_eq!(
        deadline_from_cfg(now, &cfg, TouchpointKind::Prototype),
        now + 14_400 * 1000
    );
    // 显式收窄窗（config 覆盖语义）。
    let narrow = BoardAttendanceConfig {
        spec_timeout_secs: 10,
        ..Default::default()
    };
    assert_eq!(
        deadline_from_cfg(now, &narrow, TouchpointKind::Spec),
        now + 10_000
    );
}

// 0 值语义（「0 = 不超时」）：返回 0（消费侧 deadline_ms <= 0 = 不设限
// 惯例闸）——非裸加法吞成 now_ms（触点创建即超时，方向反转）。
#[test]
fn test_deadline_from_cfg_zero_means_no_timeout() {
    let zero = BoardAttendanceConfig {
        spec_timeout_secs: 0,
        prototype_timeout_secs: 0,
        ..Default::default()
    };
    assert_eq!(
        deadline_from_cfg(1_700_000_000_000, &zero, TouchpointKind::Spec),
        0
    );
    assert_eq!(
        deadline_from_cfg(1_700_000_000_000, &zero, TouchpointKind::Prototype),
        0
    );
}

// 溢出安全：巨值秒数钳制——裸 `as i64 * 1000` 环绕成负（deadline 反向
// 「已过期」）被钳为 saturating 不环绕。
#[test]
fn test_deadline_from_cfg_huge_secs_does_not_wrap_negative() {
    let huge = BoardAttendanceConfig {
        spec_timeout_secs: u64::MAX,
        ..Default::default()
    };
    let now = 1_700_000_000_000i64;
    let deadline = deadline_from_cfg(now, &huge, TouchpointKind::Spec);
    assert!(
        deadline > now,
        "huge window must stay in the future, got {deadline}"
    );
}

// blocking_cap 巨值钳制（checkpoint 硬顶 / parked 通报共用 helper）。
#[test]
fn test_blocking_cap_ms_huge_does_not_wrap_negative() {
    let huge = BoardAttendanceConfig {
        blocking_cap_secs: u64::MAX,
        ..Default::default()
    };
    let ms = blocking_cap_ms(&huge);
    assert!(ms > 0, "huge cap must stay positive, got {ms}");
    let zero = BoardAttendanceConfig::default();
    // 默认 3 天窗。
    assert_eq!(blocking_cap_ms(&zero), 259_200 * 1000);
}

// ---------------------------------------------------------------------------
// confirm（人工确认 → 承载单按 kind 落位 + 审计）
// ---------------------------------------------------------------------------

#[test]
fn test_confirm_closes_issue_and_touchpoint() {
    let (_dir, store) = open_store("tp-confirm");
    let issue = in_review_issue(&store, "确认单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );

    let out = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("human"),
        tp.id,
        "confirm",
        &serde_json::json!({}),
    )
    .expect("confirm");
    assert_eq!(out["touchpoint"]["status"], "confirmed");
    // 规格点火单：方向已定、拆解链还要走 → 回 in_progress（不是 done）。
    assert_eq!(out["issue_status"], "in_progress");

    // 盘上真相回读（不只信返回值）。
    let fresh = store.get_touchpoint(tp.id).unwrap().expect("tp exists");
    assert_eq!(fresh.status, TouchpointStatus::Confirmed);
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress
    );
    // 审计链：hang + confirm 都在场。
    let audit = fresh.audit;
    assert!(audit.contains("\"hang\""), "audit 缺 hang: {audit}");
    assert!(audit.contains("\"confirm\""), "audit 缺 confirm: {audit}");
    // 系统评论在场（✅ 确认）。
    let comments = store.list_comments(issue.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("确认")),
        "缺确认评论"
    );
}

#[test]
fn test_confirm_settle_diverges_by_kind() {
    // kind 决定承载单落点（kind-aware settle）：原型里程碑单 confirm =
    // 机器闸已过、人工方向已定 → 单真的做完 → done；规格点火单 confirm =
    // 方向已定、拆解链还要走 → 回 in_progress。提前 done 会把自动验收/
    // 自动收父单链骗成「已完工」。
    let (_dir, store) = open_store("tp-confirm-divergence");
    let proto_issue = in_review_issue(&store, "原型里程碑单", None);
    let proto_tp = hang(
        &store,
        proto_issue.id,
        None,
        TouchpointKind::Prototype,
        now_ms() + 3_600_000,
    );

    let out = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("human"),
        proto_tp.id,
        "confirm",
        &serde_json::json!({}),
    )
    .expect("prototype confirm");
    assert_eq!(out["touchpoint"]["status"], "confirmed");
    assert_eq!(out["issue_status"], "done", "原型单 confirm 应落 done");
    assert_eq!(
        store.get_issue(proto_issue.id).unwrap().status,
        IssueStatus::Done,
        "原型单与规格单落点必须分歧（done vs in_progress）"
    );
}

// ---------------------------------------------------------------------------
// revise_spec（否决 → v2 重挂 → 熔断）
// ---------------------------------------------------------------------------

#[test]
fn test_revise_spec_rehangs_v2_with_reset_deadline() {
    let (_dir, store) = open_store("tp-revise");
    let issue = in_review_issue(&store, "改规格单", None);
    let hang_deadline = now_ms() + 5_000; // 故意给个近窗，验证重置
    let tp = hang(&store, issue.id, None, TouchpointKind::Spec, hang_deadline);

    let out = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("human"),
        tp.id,
        "revise_spec",
        &serde_json::json!({ "spec": "修订后的规格正文" }),
    )
    .expect("revise_spec");
    // v2 重挂：回 pending、版本自增、spec 替换、倒计时重置、否决计数 +1。
    assert_eq!(out["touchpoint"]["status"], "pending");
    assert_eq!(out["issue_status"], "in_review");
    let payload: serde_json::Value =
        serde_json::from_str(out["touchpoint"]["payload"].as_str().unwrap()).unwrap();
    assert_eq!(payload["version"], "v2");
    assert_eq!(payload["spec"], "修订后的规格正文");
    assert_eq!(out["touchpoint"]["veto_rounds"], 1);
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert!(
        fresh.deadline_ms > hang_deadline,
        "deadline 未重置：{} ≤ {}",
        fresh.deadline_ms,
        hang_deadline
    );
    let audit = fresh.audit;
    assert!(
        audit.contains("\"veto_spec\""),
        "audit 缺 veto_spec: {audit}"
    );
    assert!(audit.contains("\"rehang\""), "audit 缺 rehang: {audit}");
}

#[test]
fn test_veto_meltdown_at_max_rounds_parks() {
    let (_dir, store) = open_store("tp-meltdown");
    let issue = in_review_issue(&store, "熔断单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    let cfg = att_cfg_veto(2); // 默认即 2，显式写明

    // 第一轮否决：veto_rounds 0→1，未达上限 → 正常重挂。
    respond_touchpoint(
        &store,
        &cfg,
        &Actor::admin("human"),
        tp.id,
        "revise_spec",
        &serde_json::json!({ "spec": "v2 规格" }),
    )
    .expect("first revise");
    // 第二轮否决：veto_rounds 1→2 达上限 → 熔断停车。
    let out = respond_touchpoint(
        &store,
        &cfg,
        &Actor::admin("human"),
        tp.id,
        "revise_spec",
        &serde_json::json!({ "spec": "v3 规格" }),
    )
    .expect("second revise (meltdown)");
    assert_eq!(out["touchpoint"]["status"], "parked_sync");
    assert_eq!(out["issue_status"], "blocked");
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert_eq!(fresh.status, TouchpointStatus::ParkedSync);
    assert!(
        fresh.audit.contains("\"parked_sync\""),
        "audit 缺 parked_sync"
    );
    // 系统评论带 🛑 熔断通报。
    let comments = store.list_comments(issue.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("🛑")),
        "缺熔断评论"
    );
}

#[test]
fn test_veto_meltdown_disabled_when_zero() {
    let (_dir, store) = open_store("tp-veto-off");
    let issue = in_review_issue(&store, "熔断关闭单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    let cfg = att_cfg_veto(0); // 0 = 熔断关闭

    for round in 2..6 {
        let out = respond_touchpoint(
            &store,
            &cfg,
            &Actor::admin("human"),
            tp.id,
            "revise_spec",
            &serde_json::json!({ "spec": format!("第 {round} 版规格") }),
        )
        .expect("revise with cap off");
        assert_eq!(
            out["touchpoint"]["status"], "pending",
            "round {round} 应正常重挂"
        );
    }
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert_eq!(fresh.veto_rounds, 4, "否决计数应累计（2..6 = 4 轮）");
    assert_eq!(fresh.status, TouchpointStatus::Pending);
}

// ---------------------------------------------------------------------------
// rework_local / cancel / mute
// ---------------------------------------------------------------------------

#[test]
fn test_rework_local_requires_reason() {
    let (_dir, store) = open_store("tp-rework");
    let issue = in_review_issue(&store, "局部返工单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );

    // 缺 reason = loud 拒绝（不猜意图）。
    let err = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("human"),
        tp.id,
        "rework_local",
        &serde_json::json!({}),
    )
    .expect_err("缺 reason 应拒绝");
    assert!(err.contains("reason"), "报错应点名 reason：{err}");
}

#[test]
fn test_rework_local_spec_rehangs_in_row() {
    let (_dir, store) = open_store("tp-rework-spec");
    let issue = in_review_issue(&store, "规格局部返工单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );

    // spec 臂：原载荷重挂（版本不变），否决计数行内 +1。
    let out = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("human"),
        tp.id,
        "rework_local",
        &serde_json::json!({ "reason": "验收标准写漏了" }),
    )
    .expect("rework_local");
    assert_eq!(out["touchpoint"]["status"], "pending");
    assert_eq!(out["touchpoint"]["veto_rounds"], 1);
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert!(fresh.audit.contains("验收标准写漏了"), "audit 缺否决原因");
    let payload: serde_json::Value = serde_json::from_str(fresh.payload.as_str()).unwrap();
    assert_eq!(payload["version"], "v1", "局部返工不改版本");
}

#[test]
fn test_rework_local_prototype_vetoes_without_rehang() {
    let (_dir, store) = open_store("tp-rework-proto");
    let issue = in_review_issue(&store, "原型局部返工单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Prototype,
        now_ms() + 3_600_000,
    );

    // 原型臂：落账否决（不重挂），重派交恢复钩子。TOUCHPOINT_RESUME_HOOK
    // 是进程级单例、装配与否取决于同进程并行测试的装配顺序（hook sink
    // 测试会装），评论尾注两种形态都合法（armed=机器闸复验 / unarmed=需
    // 人工派发），不硬编码断言其中一种。
    let out = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("human"),
        tp.id,
        "rework_local",
        &serde_json::json!({ "reason": "按钮布局不对" }),
    )
    .expect("rework_local");
    assert_eq!(out["touchpoint"]["status"], "vetoed_local");
    assert_eq!(out["touchpoint"]["veto_rounds"], 1);
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert!(fresh.audit.contains("按钮布局不对"), "audit 缺否决原因");
    // 承载单不被局部返工推动（留在 in_review 等重派/人工）。
    assert_eq!(out["issue_status"], "in_review");
    let comments = store.list_comments(issue.id).unwrap();
    let note = comments
        .iter()
        .find(|c| c.content.contains("局部返工已记录"))
        .expect("缺局部返工评论");
    assert!(note.content.contains("按钮布局不对"), "评论缺否决原因");
    assert!(
        note.content.contains("机器闸复验") || note.content.contains("恢复钩子未装配"),
        "评论尾注应注明续跑去向（armed/unarmed 二选一）：{}",
        note.content
    );
}

#[test]
fn test_rework_local_prototype_meltdown_sums_across_cards() {
    let (_dir, store) = open_store("tp-rework-meltdown");
    let issue = in_review_issue(&store, "原型熔断单", None);
    let cfg = att_cfg_veto(2);

    // 第一张卡：否决 1 次（跨卡计数 1 < 2，正常落账）。
    let tp_a = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Prototype,
        now_ms() + 3_600_000,
    );
    let out = respond_touchpoint(
        &store,
        &cfg,
        &Actor::admin("human"),
        tp_a.id,
        "rework_local",
        &serde_json::json!({ "reason": "第一轮否决" }),
    )
    .expect("rework a");
    assert_eq!(out["touchpoint"]["status"], "vetoed_local");

    // 机器闸复验重派后新挂第二张卡（卡内计数从 0 起）再否决：跨卡求和
    // 1+0+1 = 2 ≥ veto_max_rounds → 熔断停车。
    let tp_b = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Prototype,
        now_ms() + 3_600_000,
    );
    let out = respond_touchpoint(
        &store,
        &cfg,
        &Actor::admin("human"),
        tp_b.id,
        "rework_local",
        &serde_json::json!({ "reason": "第二轮否决" }),
    )
    .expect("rework b");
    assert_eq!(out["touchpoint"]["status"], "parked_sync");
    assert_eq!(out["issue_status"], "blocked");
    let comments = store.list_comments(issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("veto_max_rounds=2") && c.content.contains("停车")),
        "缺熔断停车通报"
    );
}

#[test]
fn test_cancel_closes_issue_with_and_without_reason() {
    let (_dir, store) = open_store("tp-cancel");
    let actor = Actor::admin("human");
    // 无 reason。
    let issue_a = in_review_issue(&store, "取消单A", None);
    let tp_a = hang(
        &store,
        issue_a.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    let out = respond_touchpoint(
        &store,
        &att_cfg(),
        &actor,
        tp_a.id,
        "cancel",
        &serde_json::json!({}),
    )
    .expect("cancel a");
    assert_eq!(out["touchpoint"]["status"], "cancelled");
    assert_eq!(out["issue_status"], "cancelled");
    // 带 reason。
    let issue_b = in_review_issue(&store, "取消单B", None);
    let tp_b = hang(
        &store,
        issue_b.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    let out = respond_touchpoint(
        &store,
        &att_cfg(),
        &actor,
        tp_b.id,
        "cancel",
        &serde_json::json!({ "reason": "需求变了" }),
    )
    .expect("cancel b");
    assert_eq!(out["issue_status"], "cancelled");
    let comments = store.list_comments(issue_b.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("需求变了")),
        "取消评论缺原因"
    );
}

#[test]
fn test_mute_flips_project_and_defaults() {
    let (_dir, store) = open_store("tp-mute");
    let project = store
        .create_project(
            "mute-project",
            "静音项目",
            None,
            "📦",
            "",
            None,
            None, // unattended 未表态
        )
        .unwrap();
    let issue = in_review_issue(&store, "静音单", Some(project.id));
    let tp = hang(
        &store,
        issue.id,
        Some(project.id),
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );

    let out = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("human"),
        tp.id,
        "mute",
        &serde_json::json!({}),
    )
    .expect("mute");
    // 项目就地翻转无人值守。
    assert_eq!(
        store.get_project(project.id).unwrap().unattended,
        Some(true),
        "项目未被翻转为无人值守"
    );
    // 触点按默认动作立即收口（关门不锁门）；规格单回 in_progress 续链。
    assert_eq!(out["touchpoint"]["status"], "timeout_defaulted");
    assert_eq!(out["issue_status"], "in_progress");
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    let human: serde_json::Value = serde_json::from_str(fresh.human_input.as_str()).unwrap();
    assert_eq!(human["default_action"], "proceed_with_draft");
    assert_eq!(human["origin"], "mute");
    assert!(fresh.audit.contains("\"mute\""), "audit 缺 mute");
}

#[test]
fn test_mute_without_project_is_honest_error() {
    let (_dir, store) = open_store("tp-mute-noproj");
    let issue = in_review_issue(&store, "散单静音", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    let err = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("human"),
        tp.id,
        "mute",
        &serde_json::json!({}),
    )
    .expect_err("散单 mute 应诚实报错");
    assert!(err.contains("项目"), "报错应说明缺项目落点：{err}");
    // 触点未被动过（pending 原样）。
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert_eq!(fresh.status, TouchpointStatus::Pending);
}

// ---------------------------------------------------------------------------
// 终态闸门 + 未知动作（关门不锁门 + 不静默纠偏）
// ---------------------------------------------------------------------------

#[test]
fn test_terminal_touchpoint_rejects_respond() {
    let (_dir, store) = open_store("tp-terminal");
    let issue = in_review_issue(&store, "终态闸门单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("h"),
        tp.id,
        "confirm",
        &serde_json::json!({}),
    )
    .expect("first confirm");
    let err = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("h"),
        tp.id,
        "confirm",
        &serde_json::json!({}),
    )
    .expect_err("终态触点应拒绝动作");
    assert!(err.contains("不接受人工动作"), "报错应点名终态闸门：{err}");
    assert!(
        err.contains("评论"),
        "报错应指引迟到输入走评论（关门不锁门）：{err}"
    );
}

#[test]
fn test_unknown_action_is_loud() {
    let (_dir, store) = open_store("tp-unknown");
    let issue = in_review_issue(&store, "未知动作单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    let err = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("h"),
        tp.id,
        "explode",
        &serde_json::json!({}),
    )
    .expect_err("未知动作应 loud 拒绝");
    assert!(err.contains("未知触点动作"), "报错应点名动作词表：{err}");
    assert!(err.contains("confirm"), "报错应列出动作词表：{err}");
    // 不存在的触点 id 同样诚实报错。
    let err = respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("h"),
        999_999,
        "confirm",
        &serde_json::json!({}),
    )
    .expect_err("不存在触点应报错");
    assert!(err.contains("不存在"), "报错应点名触点不存在：{err}");
}

// ---------------------------------------------------------------------------
// sweep：unattended 默认收口 + 零 deadline 不设限
// ---------------------------------------------------------------------------

#[test]
fn test_sweep_unattended_defaults_past_deadline() {
    let (_dir, store) = open_store("tp-sweep-default");
    let issue = in_review_issue(&store, "超时收口单", None);
    let past = now_ms() - 1_000;
    let tp = hang(&store, issue.id, None, TouchpointKind::Spec, past);

    let stats = sweep_touchpoint_timeouts_with_config(&att_cfg(), &store, now_ms());
    assert_eq!(stats.checked, 1);
    assert_eq!(stats.defaulted, 1);
    assert_eq!(
        stats,
        super::touchpoints::TouchpointSweepStats {
            checked: 1,
            defaulted: 1,
            ..Default::default()
        }
    );

    // 收口语义全落账：触点终态 + 规格单回 in_progress + 审计 + 系统评论。
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert_eq!(fresh.status, TouchpointStatus::TimeoutDefaulted);
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress
    );
    assert!(
        fresh.audit.contains("\"timeout_default\""),
        "audit 缺 timeout_default"
    );
    assert!(
        fresh.audit.contains("\"timeout\""),
        "audit origin 应为 timeout"
    );
    let human: serde_json::Value = serde_json::from_str(fresh.human_input.as_str()).unwrap();
    assert_eq!(human["default_action"], "proceed_with_draft");
    let comments = store.list_comments(issue.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("已按默认继续")),
        "缺默认收口系统评论"
    );

    // 终态后 sweep 再跑：不重复收口（pendings 列表已不含它）。
    let stats = sweep_touchpoint_timeouts_with_config(&att_cfg(), &store, now_ms());
    assert_eq!(stats.checked, 0);
    assert_eq!(stats.defaulted, 0);
}

#[test]
fn test_sweep_zero_deadline_is_untouched() {
    let (_dir, store) = open_store("tp-sweep-zero");
    let issue = in_review_issue(&store, "不设限单", None);
    let tp = hang(&store, issue.id, None, TouchpointKind::Spec, 0); // 0 = 不设限

    let stats = sweep_touchpoint_timeouts_with_config(&att_cfg(), &store, now_ms());
    assert_eq!(stats.checked, 1, "0 deadline 仍计数 checked");
    assert_eq!(stats.defaulted, 0, "0 deadline 不收口");
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert_eq!(fresh.status, TouchpointStatus::Pending);
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );
}

// ---------------------------------------------------------------------------
// sweep：提醒拍（T+10min，幂等）
// ---------------------------------------------------------------------------

#[test]
fn test_sweep_remind_beat_once_and_idempotent() {
    let (_dir, store) = open_store("tp-sweep-remind");
    let issue = in_review_issue(&store, "提醒拍单", None);
    // 未来 deadline（远超 sweep 模拟时刻）→ 只走提醒拍，不走收口。
    let deadline = now_ms() + 100_000_000;
    let tp = hang(&store, issue.id, None, TouchpointKind::Spec, deadline);
    let t0 = now_ms();

    // T+100s：未到提醒点，不提醒。
    let stats = sweep_touchpoint_timeouts_with_config(&att_cfg(), &store, t0 + 100_000);
    assert_eq!(stats.reminded, 0);

    // T+10min：提醒一次。
    let stats = sweep_touchpoint_timeouts_with_config(&att_cfg(), &store, t0 + REMIND_AFTER_MS);
    assert_eq!(stats.reminded, 1);
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert!(fresh.audit.contains("\"remind\""), "audit 缺 remind");
    assert_eq!(fresh.status, TouchpointStatus::Pending, "提醒不收口");
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );

    // 再 sweep：幂等（audit 去重），不再提醒。
    let stats = sweep_touchpoint_timeouts_with_config(&att_cfg(), &store, t0 + REMIND_AFTER_MS + 1);
    assert_eq!(stats.reminded, 0);
}

// ---------------------------------------------------------------------------
// sweep：checkpoint 死等语义（窗口到 → 等待通报；硬顶到 → 默认收口）
// ---------------------------------------------------------------------------

#[test]
fn test_sweep_checkpoint_wait_then_cap_default() {
    let (_dir, store) = open_store("tp-sweep-checkpoint");
    let issue = in_review_issue(&store, "checkpoint 单", None);
    let t0 = now_ms();
    let tp = hang(&store, issue.id, None, TouchpointKind::Spec, t0 - 1_000); // 已过窗
    let cfg = att_cfg_checkpoint(60); // 硬顶 60s

    // 窗口到但硬顶未到：checkpoint_wait 通报一次，触点仍 pending。
    let stats = sweep_touchpoint_timeouts_with_config(&cfg, &store, t0);
    assert_eq!(stats.checked, 1);
    assert_eq!(stats.parked, 1, "应转同步沟通等待");
    assert_eq!(stats.defaulted, 0, "checkpoint 模式窗口到不默认收口");
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert_eq!(fresh.status, TouchpointStatus::Pending);
    assert!(
        fresh.audit.contains("\"checkpoint_wait\""),
        "audit 缺 checkpoint_wait"
    );
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );

    // 再 sweep：幂等，不重复通报。
    let stats = sweep_touchpoint_timeouts_with_config(&cfg, &store, t0 + 1_000);
    assert_eq!(stats.parked, 0);

    // 硬顶到顶：默认收口（死等有界，机器不停摆）。
    let stats = sweep_touchpoint_timeouts_with_config(&cfg, &store, t0 + 61_000);
    assert_eq!(stats.defaulted, 1);
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert_eq!(fresh.status, TouchpointStatus::TimeoutDefaulted);
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress
    );
    assert!(
        fresh.audit.contains("\"blocking_cap\""),
        "audit origin 应为 blocking_cap: {}",
        fresh.audit
    );
}

#[test]
fn test_sweep_checkpoint_zero_cap_defaults_immediately() {
    let (_dir, store) = open_store("tp-sweep-cap0");
    let issue = in_review_issue(&store, "零硬顶单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() - 1_000,
    );
    let cfg = att_cfg_checkpoint(0); // 0 = 不兜底：窗口到即默认收口

    let stats = sweep_touchpoint_timeouts_with_config(&cfg, &store, now_ms());
    assert_eq!(stats.defaulted, 1);
    assert_eq!(stats.parked, 0, "零硬顶不进等待");
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert_eq!(fresh.status, TouchpointStatus::TimeoutDefaulted);
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress
    );
}

// ---------------------------------------------------------------------------
// sweep：parked_sync 滞留通报（人工升级留痕，机器不代答）
// ---------------------------------------------------------------------------

#[test]
fn test_sweep_park_cap_notice_once() {
    let (_dir, store) = open_store("tp-sweep-parkcap");
    let issue = in_review_issue(&store, "停车滞留单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    // 直落 parked_sync（pending → parked_sync 合法边；熔断已在漏斗测试钉）。
    store
        .transition_touchpoint(tp.id, TouchpointStatus::ParkedSync, "{}")
        .unwrap();
    let cfg = att_cfg_checkpoint(60); // blocking_cap_secs=60 复用为滞留窗

    let t0 = now_ms();
    let stats = sweep_touchpoint_timeouts_with_config(&cfg, &store, t0 + 60_000);
    assert_eq!(stats.cap_noticed, 1, "滞留超硬顶应通报一次");
    let fresh = store.get_touchpoint(tp.id).unwrap().unwrap();
    assert!(
        fresh.audit.contains("\"park_cap_notice\""),
        "audit 缺 park_cap_notice"
    );
    assert_eq!(fresh.status, TouchpointStatus::ParkedSync, "通报不解除停车");

    // 幂等：再 sweep 不重复通报。
    let stats = sweep_touchpoint_timeouts_with_config(&cfg, &store, t0 + 60_001);
    assert_eq!(stats.cap_noticed, 0);
}

// ---------------------------------------------------------------------------
// resolve_unattended 三态优先级
// ---------------------------------------------------------------------------

#[test]
fn test_resolve_unattended_precedence() {
    let (_dir, store) = open_store("tp-resolve");
    let global_unattended = att_cfg(); // 默认全局无人值守
    let global_checkpoint = att_cfg_checkpoint(60);

    // 项目显式表态（有人值守）覆盖全局无人值守。
    let project = store
        .create_project("att-proj", "考勤项目", None, "📦", "", None, Some(false))
        .unwrap();
    let in_project = in_review_issue(&store, "项目单", Some(project.id));
    assert!(
        !resolve_unattended(&store, &in_project, &global_unattended).unwrap(),
        "项目显式 false 应压过全局 unattended"
    );

    // 任务级 Some(true) 压过项目级 Some(false)。
    let actor = Actor::admin("test");
    let pinned = store
        .create_issue(NewIssue {
            title: "任务级无人值守".into(),
            project_id: Some(project.id),
            creator: actor.clone(),
            unattended: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert!(
        resolve_unattended(&store, &pinned, &global_unattended).unwrap(),
        "任务级 Some(true) 应压过项目级 false"
    );

    // 全 None：继承全局模式（unattended → true / checkpoint → false）。
    let loose = in_review_issue(&store, "全继承单", None);
    assert!(resolve_unattended(&store, &loose, &global_unattended).unwrap());
    assert!(
        !resolve_unattended(&store, &loose, &global_checkpoint).unwrap(),
        "checkpoint 全局模式应解析为有人值守"
    );

    // 项目行缺失 = Err（诚实失败，不静默当无人值守）。
    let mut orphan = loose.clone();
    orphan.project_id = Some(999_999);
    assert!(resolve_unattended(&store, &orphan, &global_unattended).is_err());
}

// ---------------------------------------------------------------------------
// list_touchpoints（看板触点列表取数面）
// ---------------------------------------------------------------------------

#[test]
fn test_list_touchpoints_filters_and_counts() {
    let (_dir, store) = open_store("tp-list");
    let project = store
        .create_project("list-proj", "列表项目", None, "📦", "", None, None)
        .unwrap();
    // A：项目内 pending（未来窗，不 overdue）。
    let issue_a = in_review_issue(&store, "列表单A", Some(project.id));
    hang(
        &store,
        issue_a.id,
        Some(project.id),
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    // B：项目内 pending（已过窗 → overdue）。
    let issue_b = in_review_issue(&store, "列表单B", Some(project.id));
    let tp_b = hang(
        &store,
        issue_b.id,
        Some(project.id),
        TouchpointKind::Prototype,
        now_ms() - 1_000,
    );
    // C：散单 confirm（终态，非 pending）。
    let issue_c = in_review_issue(&store, "列表单C", None);
    let tp_c = hang(
        &store,
        issue_c.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("h"),
        tp_c.id,
        "confirm",
        &serde_json::json!({}),
    )
    .unwrap();
    let _ = (tp_b, tp_c);

    // 缺省 = pending 活卡：A + B，pending_count=2，B overdue。
    let out = list_touchpoints(&store, &serde_json::json!({})).unwrap();
    let rows = out["touchpoints"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "缺省只出 pending：{out}");
    assert_eq!(out["pending_count"], 2);
    let numbers: Vec<&str> = rows
        .iter()
        .map(|r| r["issue_number"].as_str().unwrap())
        .collect();
    assert!(numbers.contains(&issue_a.number.as_str()));
    assert!(numbers.contains(&issue_b.number.as_str()));
    let row_b = rows
        .iter()
        .find(|r| r["issue_number"] == issue_b.number.as_str())
        .unwrap();
    assert_eq!(row_b["overdue"], true, "过窗 pending 应标 overdue");
    let row_a = rows
        .iter()
        .find(|r| r["issue_number"] == issue_a.number.as_str())
        .unwrap();
    assert_eq!(row_a["overdue"], false);

    // all=true：终态也进（3 行），pending_count 仍只数 pending（2）。
    let out = list_touchpoints(&store, &serde_json::json!({ "all": true })).unwrap();
    assert_eq!(out["touchpoints"].as_array().unwrap().len(), 3);
    assert_eq!(out["pending_count"], 2);

    // 项目过滤。
    let out = list_touchpoints(
        &store,
        &serde_json::json!({ "all": true, "project_id": project.id }),
    )
    .unwrap();
    let rows = out["touchpoints"].as_array().unwrap();
    assert_eq!(rows.len(), 2);

    // status 过滤。
    let out = list_touchpoints(
        &store,
        &serde_json::json!({ "all": true, "status": "confirmed" }),
    )
    .unwrap();
    assert_eq!(out["touchpoints"].as_array().unwrap().len(), 1);

    // 未知 status = loud 拒绝。
    assert!(
        list_touchpoints(
            &store,
            &serde_json::json!({ "all": true, "status": "bogus" })
        )
        .is_err()
    );
}

// ---------------------------------------------------------------------------
// workbench 取数面
// ---------------------------------------------------------------------------

#[test]
fn test_workbench_payload_semantics() {
    let (_dir, store) = open_store("tp-workbench");
    let issue = in_review_issue(&store, "工作台单", None);
    let spec_tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );

    let out = touchpoint_workbench_payload(&store, &att_cfg(), spec_tp.id).unwrap();
    for key in [
        "touchpoint",
        "issue",
        "overdue",
        "default_action",
        "unattended",
        "timeout_secs",
    ] {
        assert!(out.get(key).is_some(), "载荷缺键 {key}: {out}");
    }
    assert_eq!(out["touchpoint"]["kind"], "spec");
    assert_eq!(out["default_action"], "proceed_with_draft");
    assert_eq!(out["unattended"], true, "全局默认无人值守");
    assert_eq!(out["timeout_secs"], 43_200);
    assert_eq!(out["overdue"], false);
    assert_eq!(out["issue"]["id"], issue.id);

    // 原型触点语义：默认动作 = 按当前版本继续，窗 4h。
    let proto_issue = in_review_issue(&store, "原型工作台单", None);
    let proto_tp = hang(
        &store,
        proto_issue.id,
        None,
        TouchpointKind::Prototype,
        now_ms() + 3_600_000,
    );
    let out = touchpoint_workbench_payload(&store, &att_cfg(), proto_tp.id).unwrap();
    assert_eq!(out["default_action"], "proceed_current_version");
    assert_eq!(out["timeout_secs"], 14_400);

    // 不存在的触点 = 诚实报错。
    let err =
        touchpoint_workbench_payload(&store, &att_cfg(), 999_999).expect_err("不存在触点应报错");
    assert!(err.contains("不存在"));
}

// ---------------------------------------------------------------------------
// workbench handler 路由壳（kind 映射 + 身份固定 + 一致性检查）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_workbench_handler_routing() {
    let dir = unique_dir("tp-wb-handler");
    let ctx: RequestContext = make_ctx_with_board(&dir);
    let store = BoardStore::open(&dir.join("board.db"), "NB").unwrap();
    let issue = in_review_issue(&store, "路由单", None);
    let spec_tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );

    // 正常取数：URL 段 touchpoint-spec 命中 spec 触点。
    let out = WorkbenchHandler
        .handle_cmd(
            "workbench.get",
            Some(serde_json::json!({ "kind": "touchpoint-spec", "id": spec_tp.id })),
            &ctx,
        )
        .await
        .expect("workbench.get spec");
    assert_eq!(out.unwrap()["touchpoint"]["kind"], "spec");

    // kind 与触点实体不一致（stale/手打交叉）= 诚实报错。
    let err = WorkbenchHandler
        .handle_cmd(
            "workbench.get",
            Some(serde_json::json!({ "kind": "touchpoint-prototype", "id": spec_tp.id })),
            &ctx,
        )
        .await
        .expect_err("kind 不匹配应报错");
    assert!(err.contains("kind 不匹配"), "报错应点名 kind 不匹配：{err}");

    // 未知 kind = loud 报错（列权威形态）。
    let err = WorkbenchHandler
        .handle_cmd(
            "workbench.get",
            Some(serde_json::json!({ "kind": "nope", "id": spec_tp.id })),
            &ctx,
        )
        .await
        .expect_err("未知 kind 应报错");
    assert!(
        err.contains("未知工作台 kind"),
        "报错应点名 kind 值域：{err}"
    );

    // 缺 id = 报错。
    let err = WorkbenchHandler
        .handle_cmd(
            "workbench.get",
            Some(serde_json::json!({ "kind": "touchpoint-spec" })),
            &ctx,
        )
        .await
        .expect_err("缺 id 应报错");
    assert!(err.contains("id"), "报错应点名 id：{err}");

    // respond 透传漏斗：未知动作错误从漏斗冒出（kind 只做值域校验，id 才是权威键）。
    let err = WorkbenchHandler
        .handle_cmd(
            "workbench.respond",
            Some(serde_json::json!({
                "kind": "touchpoint-spec",
                "id": spec_tp.id,
                "action": "explode"
            })),
            &ctx,
        )
        .await
        .expect_err("未知动作应报错");
    assert!(err.contains("未知触点动作"), "漏斗错误应透传：{err}");

    // respond 成功路径（身份固定 dashboard admin）。
    let out = WorkbenchHandler
        .handle_cmd(
            "workbench.respond",
            Some(serde_json::json!({
                "kind": "touchpoint-spec",
                "id": spec_tp.id,
                "action": "confirm"
            })),
            &ctx,
        )
        .await
        .expect("workbench.respond confirm");
    assert_eq!(out.unwrap()["issue_status"], "in_progress");
}

// ---------------------------------------------------------------------------
// 恢复钩子（install-once；并行测试容忍占位）
// ---------------------------------------------------------------------------

#[test]
fn test_resume_hook_fires_on_confirm() {
    let (_dir, store) = open_store("tp-hook");
    let issue = in_review_issue(&store, "钩子确认单", None);
    let tp = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );

    let fired: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&fired);
    let installed = install_touchpoint_resume_hook(Arc::new(move |r: &TouchpointResume| {
        sink.lock().expect("hook sink lock").push(r.touchpoint_id);
        Ok(())
    }));

    respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("h"),
        tp.id,
        "confirm",
        &serde_json::json!({}),
    )
    .expect("confirm");

    // install-once 进程级槽：并行测试里可能已被别的测试占位（installed=false）
    // ——只有装上了才断言收到 fire（containment 而非精确计数：其他测试的
    // confirm/sweep 也会打进同一槽）。
    if installed {
        assert!(
            fired.lock().expect("hook sink lock").contains(&tp.id),
            "confirm 后恢复钩子应收到 fire"
        );
    }
}

// ---------------------------------------------------------------------------
// latest_rework_recheck（复验诉求 latest-veto-wins 纯查询；消费点 = 机器
// 行为闸 PASS 臂无人值守分支）
// ---------------------------------------------------------------------------

#[test]
fn test_latest_rework_recheck_latest_veto_wins() {
    let (_dir, store) = open_store("tp-recheck");
    let issue = in_review_issue(&store, "复验诉求单", None);
    let cfg = att_cfg_veto(0); // 0 = 熔断关闭（连续否决不停车）

    // 无否决记录 = false。
    assert!(!latest_rework_recheck(&store, issue.id));

    // 否决带 recheck=true → true。
    let tp_a = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Prototype,
        now_ms() + 3_600_000,
    );
    respond_touchpoint(
        &store,
        &cfg,
        &Actor::admin("human"),
        tp_a.id,
        "rework_local",
        &serde_json::json!({ "reason": "改完再看", "recheck": true }),
    )
    .expect("rework a");
    assert!(latest_rework_recheck(&store, issue.id));

    // 之后一次不带 recheck 的否决（新卡）→ latest-veto-wins 落 false。
    let tp_b = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Prototype,
        now_ms() + 3_600_000,
    );
    respond_touchpoint(
        &store,
        &cfg,
        &Actor::admin("human"),
        tp_b.id,
        "rework_local",
        &serde_json::json!({ "reason": "又不对了" }),
    )
    .expect("rework b");
    assert!(!latest_rework_recheck(&store, issue.id));

    // spec 卡的否决不参与（kind 过滤）：spec 否决带 recheck=true 不改变
    // 原型结论——最新 PROTOTYPE 否决仍是 tp_b（recheck=false）。
    let tp_c = hang(
        &store,
        issue.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    respond_touchpoint(
        &store,
        &cfg,
        &Actor::admin("human"),
        tp_c.id,
        "rework_local",
        &serde_json::json!({ "reason": "规格也不对", "recheck": true }),
    )
    .expect("rework c");
    assert!(
        !latest_rework_recheck(&store, issue.id),
        "spec 否决不应计入原型复验诉求"
    );
}

// ---------------------------------------------------------------------------
// P2 规格蒸馏闸（纯函数 + store 面 + 链路早退）
// ---------------------------------------------------------------------------

use super::touchpoints::{
    SpecGateAction, apply_spec_checks_to_ac, apply_spec_to_description, apply_spec_to_issue,
    fallback_spec, parse_distill_output, spec_from_payload, spec_gate_decision, spec_payload,
};

/// 体验型单据夹具（quality=experience + 显式 unattended 三态控制——
/// issue 级 Some 覆盖全局 mode，默认态 unattended 不会劫持断言）。
fn experience_issue(
    store: &BoardStore,
    title: &str,
    unattended: Option<bool>,
) -> nemesis_board::models::Issue {
    let actor = Actor::admin("test");
    let issue = store
        .create_issue(NewIssue {
            title: title.into(),
            quality: "experience".into(),
            unattended,
            creator: actor.clone(),
            ..Default::default()
        })
        .expect("create experience issue");
    store
        .transition_issue(issue.id, IssueStatus::InProgress, &actor)
        .expect("in_progress");
    store
        .transition_issue(issue.id, IssueStatus::InReview, &actor)
        .expect("in_review");
    store.get_issue(issue.id).expect("re-read issue")
}

#[test]
fn test_apply_spec_to_description_append_and_replace() {
    // 无标记 = 追加小节（原文保留）。
    let out = apply_spec_to_description("做一个点格子小游戏", "v1", "核心循环：点格子");
    assert!(out.starts_with("做一个点格子小游戏"));
    assert!(out.contains("## 📐 规格（触点 v1）"));
    assert!(out.contains("核心循环：点格子"));

    // 有标记 = 整节替换（版本升级幂等重烧；标记前正文原样保留）。
    let with_old = "做一个点格子小游戏\n\n## 📐 规格（触点 v1）\n\n老规格正文".to_string();
    let out2 = apply_spec_to_description(&with_old, "v2", "新规格正文");
    assert!(out2.starts_with("做一个点格子小游戏"));
    assert!(out2.contains("## 📐 规格（触点 v2）"));
    assert!(out2.contains("新规格正文"));
    assert!(!out2.contains("老规格正文"), "旧版规格整节被替换：{out2}");
    assert!(!out2.contains("v1）"), "旧版本行不残留：{out2}");

    // 幂等重烧（同版本同正文）= 逐字节稳定。
    let out3 = apply_spec_to_description(&out2, "v2", "新规格正文");
    assert_eq!(out2, out3);
}

#[test]
fn test_apply_spec_checks_to_ac_dedupe() {
    let checks = vec!["棋盘可见".to_string(), "点击有反馈".to_string()];

    // 全新 AC：每条 [CHECK] 前缀。
    let fresh = apply_spec_checks_to_ac(None, &checks).expect("fresh");
    assert_eq!(fresh, "[CHECK] 棋盘可见\n[CHECK] 点击有反馈");

    // 已有条目原样保留 + 新增追加 + 逐条去重。
    let mixed = apply_spec_checks_to_ac(Some("界面无报错"), &checks).expect("mixed");
    assert!(mixed.starts_with("界面无报错\n"));
    assert_eq!(mixed.lines().count(), 3);

    let dedup = apply_spec_checks_to_ac(Some(&fresh), &checks);
    assert_eq!(dedup, None, "全部已存在 = None（无需 patch）");

    // 空清单 = None。
    assert_eq!(apply_spec_checks_to_ac(None, &[]), None);
}

#[test]
fn test_parse_distill_output_arms() {
    let good = r###"{"version":"v1","spec":"## 核心循环","assumptions":["鼠标操作"],"acceptance_checks":["棋盘可见"]}"###;
    let d = parse_distill_output(good).expect("good");
    assert_eq!(d.version, "v1");
    assert_eq!(d.spec, "## 核心循环");
    assert_eq!(d.assumptions, vec!["鼠标操作"]);
    assert!(d.distilled);

    // 围栏 + 前后杂讯：宽容剥出 JSON。
    let fenced = format!("好的，这是草案：\n```json\n{good}\n```\n请确认。");
    let d2 = parse_distill_output(&fenced).expect("fenced");
    assert_eq!(d2.spec, "## 核心循环");

    // 缺 version = 默认 v1。
    let nover = r#"{"spec":"正文"}"#;
    assert_eq!(
        parse_distill_output(nover).expect("v1 default").version,
        "v1"
    );

    // 垃圾输出 / 缺 spec / 空 spec = None（调用方回退草案）。
    assert_eq!(parse_distill_output("我觉得应该先做地图"), None);
    assert_eq!(parse_distill_output(r#"{"assumptions":[]}"#), None);
    assert_eq!(parse_distill_output(r#"{"spec":"   "}"#), None);
}

#[test]
fn test_spec_payload_round_trip_and_fallback() {
    let spec = super::touchpoints::DistilledSpec {
        version: "v2".into(),
        spec: "核心循环正文".into(),
        assumptions: vec!["假设甲".into()],
        acceptance_checks: vec!["条款一".into()],
        distilled: true,
    };
    let payload = spec_payload(&spec);
    let back = spec_from_payload(&payload).expect("round trip");
    assert_eq!(back.version, "v2");
    assert_eq!(back.spec, "核心循环正文");
    assert_eq!(back.assumptions, vec!["假设甲"]);
    assert_eq!(back.acceptance_checks, vec!["条款一"]);
    assert!(back.distilled);

    // payload 损坏 / 缺 spec = None（烧录跳过 + 注记，不阻塞链）。
    assert_eq!(spec_from_payload("not json"), None);
    assert_eq!(spec_from_payload(r#"{"version":"v1"}"#), None);

    // 回退草案：原文即规格 + distilled=false 审计可辨。
    let (_dir, store) = open_store("tp-fallback");
    let issue = experience_issue(&store, "回退单", None);
    let fb = fallback_spec(&issue);
    assert!(!fb.distilled);
    assert!(fb.spec.contains("原始愿望原文即草案"));
    assert!(fb.spec.contains(issue.description.trim()));
    assert!(fb.assumptions.is_empty() && fb.acceptance_checks.is_empty());
}

#[test]
fn test_spec_gate_decision_five_arms() {
    let (_dir, store) = open_store("tp-gate");

    // ① 非体验型（quality 空 = 标准档）→ Skip 零开销直通。
    let std_issue = in_review_issue(&store, "标准单", None);
    assert!(matches!(
        spec_gate_decision(&store, &std_issue).expect("skip"),
        SpecGateAction::Skip
    ));

    // ② 体验型 + 无人值守（issue 级 Some(true)）→ DistillProceed。
    let unatt = experience_issue(&store, "无人值守体验单", Some(true));
    assert!(matches!(
        spec_gate_decision(&store, &unatt).expect("proceed"),
        SpecGateAction::DistillProceed
    ));

    // ③ 体验型 + 有人值守（issue 级 Some(false) 覆盖全局 unattended 默认）
    //    + 无触点 → DistillWait。
    let att = experience_issue(&store, "有人值守体验单", Some(false));
    assert!(matches!(
        spec_gate_decision(&store, &att).expect("wait"),
        SpecGateAction::DistillWait
    ));

    // ④ 有人值守 + pending 规格卡在场 → WaitPending（幂等早退）。
    let pending_tp = hang(
        &store,
        att.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    match spec_gate_decision(&store, &att).expect("wait pending") {
        SpecGateAction::WaitPending(tp) => assert_eq!(tp.id, pending_tp.id),
        other => panic!("应 WaitPending，得 {other:?}"),
    }

    // ⑤ 终态规格在场（confirm 收口）→ ProceedWithSpec（恢复钩子落点臂；
    //    无人值守同命中——终态优先于三态解析）。
    respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("human"),
        pending_tp.id,
        "confirm",
        &serde_json::json!({}),
    )
    .expect("confirm");
    match spec_gate_decision(&store, &att).expect("proceed with spec") {
        SpecGateAction::ProceedWithSpec(tp) => assert_eq!(tp.id, pending_tp.id),
        other => panic!("应 ProceedWithSpec，得 {other:?}"),
    }
    // 无人值守 + 终态在场：同样 ProceedWithSpec（不重复蒸馏）。
    let unatt_tp = hang(
        &store,
        unatt.id,
        None,
        TouchpointKind::Spec,
        now_ms() + 3_600_000,
    );
    respond_touchpoint(
        &store,
        &att_cfg(),
        &Actor::admin("human"),
        unatt_tp.id,
        "confirm",
        &serde_json::json!({}),
    )
    .expect("confirm unatt");
    match spec_gate_decision(&store, &unatt).expect("unatt settled") {
        SpecGateAction::ProceedWithSpec(tp) => assert_eq!(tp.id, unatt_tp.id),
        other => panic!("终态规格在场时无人值守也应 ProceedWithSpec，得 {other:?}"),
    }
}

#[test]
fn test_apply_spec_to_issue_round_trip() {
    let (_dir, store) = open_store("tp-apply");
    let issue = experience_issue(&store, "烧录单", None);
    let spec = super::touchpoints::DistilledSpec {
        version: "v1".into(),
        spec: "核心循环：点格子得分".into(),
        assumptions: vec![],
        acceptance_checks: vec!["棋盘可见".into(), "点击有分数反馈".into()],
        distilled: true,
    };

    apply_spec_to_issue(&store, &issue, "v1", &spec).expect("apply");
    let after = store.get_issue(issue.id).expect("re-read");
    assert!(after.description.contains("## 📐 规格（触点 v1）"));
    assert!(after.description.contains("核心循环：点格子得分"));
    let ac = after.acceptance_criteria.as_deref().expect("ac written");
    assert!(ac.contains("[CHECK] 棋盘可见"));
    assert!(ac.contains("[CHECK] 点击有分数反馈"));

    // 幂等重烧：同版本同正文 + 条款已全部在场 = 零 patch（update 不发生）。
    apply_spec_to_issue(&store, &after, "v1", &spec).expect("re-apply no-op");
    let again = store.get_issue(issue.id).expect("re-read 2");
    assert_eq!(again.description, after.description);
    assert_eq!(again.acceptance_criteria, after.acceptance_criteria);
}

// ---------------------------------------------------------------------------
// P2 链路（cluster 门控）：DistillWait 提前返回不进 planner / DistillProceed
// 蒸馏后直通拆解。双臂 provider 按 prompt 特征路由（蒸馏 prompt 含
// 「原始愿望（看板单据」标记，planner prompt 没有）。
// ---------------------------------------------------------------------------

#[cfg(feature = "cluster")]
const TP_DISTILL_JSON: &str = r###"{"version":"v1","spec":"## 核心循环\n\n点格子得分。\n\n## 前 60 秒体验\n\n点第一格即得分。","assumptions":["玩家用鼠标操作"],"acceptance_checks":["棋盘可见","点击格子有分数反馈"]}"###;

#[cfg(feature = "cluster")]
const TP_PLAN_JSON: &str = r#"[{"title":"子任务甲","acceptance_criteria":"棋盘可见"},{"title":"子任务乙","depends_on":[0]}]"#;

#[cfg(feature = "cluster")]
struct TpGateProvider {
    seen_distill: Arc<Mutex<bool>>,
    seen_plan: Arc<Mutex<bool>>,
}

#[cfg(feature = "cluster")]
#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for TpGateProvider {
    async fn chat(
        &self,
        _: &str,
        messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        let is_distill = messages
            .iter()
            .any(|m| m.content.contains("原始愿望（看板单据"));
        {
            let slot = if is_distill {
                &self.seen_distill
            } else {
                &self.seen_plan
            };
            *slot.lock().expect("provider slot lock") = true;
        }
        let content = if is_distill {
            TP_DISTILL_JSON.to_string()
        } else {
            TP_PLAN_JSON.to_string()
        };
        Ok(nemesis_agent::r#loop::LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content,
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_spec_gate_distill_wait_hangs_card_and_early_returns() {
    // RUNNING_PLANS/PLAN_CACHE 进程级全局——与 plan 链测试同锁串行。
    let _serial = super::tests::PLAN_TEST_LOCK.lock().await;
    super::tests::clear_plan_globals();
    let dir = unique_dir("tp-gate-wait");
    std::fs::create_dir_all(&dir).unwrap();
    let ctx: RequestContext = make_ctx_with_board(&dir);
    let store = super::tests::store_of(&ctx);
    let issue = experience_issue(&store, "等人体验单", Some(false));
    let actor = Actor::admin("agt");
    let hub = crate::events::EventHub::new();
    let seen_distill = Arc::new(Mutex::new(false));
    let seen_plan = Arc::new(Mutex::new(false));
    let al = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(TpGateProvider {
            seen_distill: Arc::clone(&seen_distill),
            seen_plan: Arc::clone(&seen_plan),
        }),
        nemesis_agent::types::AgentConfig::default(),
    ));

    let out = super::plan::execute_plan_chain(
        &store,
        None,
        al,
        issue.clone(),
        actor,
        "tp-gate-wait",
        None,
        Some(&hub),
        1,
    )
    .await
    .expect("DistillWait 提前返回 Ok");

    assert_eq!(out["status"], serde_json::json!("waiting_human"));
    assert_eq!(out["phase"], serde_json::json!("spec-distill"));
    assert_eq!(out["distilled"], serde_json::json!(true));
    let tp_id = out["touchpoint_id"].as_i64().expect("touchpoint_id");

    // 卡已挂（pending spec）+ 蒸馏 payload 结构化落账。
    let tp = store
        .get_touchpoint(tp_id)
        .expect("tp query")
        .expect("tp exists");
    assert_eq!(tp.kind, TouchpointKind::Spec);
    assert_eq!(tp.status, TouchpointStatus::Pending);
    let payload_spec = spec_from_payload(&tp.payload).expect("payload parseable");
    assert!(payload_spec.spec.contains("核心循环"));
    assert!(
        payload_spec.spec.contains("前 60 秒体验"),
        "蒸馏 spec 应带必须章节：{}",
        payload_spec.spec
    );

    // 承载单停在 in_review（等人）；描述未烧录（烧录只发生在确认臂）。
    let after = store.get_issue(issue.id).expect("issue re-read");
    assert_eq!(after.status, IssueStatus::InReview);
    assert!(
        !after.description.contains("## 📐 规格"),
        "等人臂不烧录描述"
    );

    // 诚实链：📐 蒸馏评论在场；🧩 拆解评论绝不出现（提前返回没拆解）。
    let comments = store.list_comments(issue.id).expect("comments");
    assert!(
        comments.iter().any(|c| c.content.contains("📐")),
        "应有 📐 蒸馏评论"
    );
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("已开始自动拆解")),
        "提前返回不得谎称已开始拆解"
    );

    // planner 未消费（等待中不拆解）。
    assert!(*seen_distill.lock().expect("lock"), "蒸馏 LLM 应被调用");
    assert!(!*seen_plan.lock().expect("lock"), "等人臂不得进 planner");
    assert!(store.list_children(issue.id).expect("children").is_empty());
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn test_spec_gate_distill_proceed_distills_then_plans() {
    let _serial = super::tests::PLAN_TEST_LOCK.lock().await;
    super::tests::clear_plan_globals();
    let dir = unique_dir("tp-gate-proceed");
    std::fs::create_dir_all(&dir).unwrap();
    let ctx: RequestContext = make_ctx_with_board(&dir);
    let store = super::tests::store_of(&ctx);
    let issue = experience_issue(&store, "无人值守体验单", Some(true));
    let actor = Actor::admin("agt");
    let hub = crate::events::EventHub::new();
    let seen_distill = Arc::new(Mutex::new(false));
    let seen_plan = Arc::new(Mutex::new(false));
    let al = Arc::new(nemesis_agent::r#loop::AgentLoop::new(
        Box::new(TpGateProvider {
            seen_distill: Arc::clone(&seen_distill),
            seen_plan: Arc::clone(&seen_plan),
        }),
        nemesis_agent::types::AgentConfig::default(),
    ));

    let out = super::plan::execute_plan_chain(
        &store,
        None,
        al,
        issue.clone(),
        actor,
        "tp-gate-proceed",
        None,
        Some(&hub),
        1,
    )
    .await
    .expect("DistillProceed 直通拆解");

    // 直通臂走完 planner（home=None → auto_confirm fail-closed → planned）。
    assert_eq!(out["status"], serde_json::json!("planned"));

    // 规格已烧进单据（描述小节 + [CHECK] 锚点源——锚点管线数据源）。
    let after = store.get_issue(issue.id).expect("issue re-read");
    assert!(after.description.contains("## 📐 规格（触点 v1）"));
    assert!(after.description.contains("核心循环"));
    let ac = after.acceptance_criteria.as_deref().expect("ac");
    assert!(ac.contains("[CHECK] 棋盘可见"));

    // 假设清单落账（📐 无人值守评论）。
    let comments = store.list_comments(issue.id).expect("comments");
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("无人值守") && c.content.contains("假设清单")),
        "无人值守臂应有假设清单评论"
    );
    assert!(*seen_distill.lock().expect("lock"), "蒸馏 LLM 应被调用");
    assert!(*seen_plan.lock().expect("lock"), "直通臂应进 planner");
}

// ---------------------------------------------------------------------------
// 无人值守直通挂卡（P4 ⑥：helper 全程静默 + 默认收口留痕）
// ---------------------------------------------------------------------------

#[test]
fn test_unattended_direct_settle_is_silent_and_audited() {
    // 直通挂卡契约（helper doc）：卡照建、照按默认收口、全程留痕，但
    // **不评论/不通知/不转移承载单/不触发续跑钩子**——无人值守链的评论
    // 与收货/续跑语义由调用方（DistillProceed / prototype_gate）自己走，
    // helper 重复触发会造成双评论/双续跑。本测试把「静默」当主断言。
    let (_dir, store) = open_store("tp-unattended-direct");
    let issue = in_review_issue(&store, "无人值守直通单", None);

    let tp = create_and_default_touchpoint(
        &store,
        &Actor::agent("node-x"),
        issue.id,
        None,
        TouchpointKind::Spec,
        r#"{"version":"v1","assumptions":["假设甲"]}"#,
    )
    .expect("unattended direct hang");

    // 盘上真相回读：不等 sweep，建卡即按默认动作收口（无人值守默认态）。
    let fresh = store.get_touchpoint(tp.id).unwrap().expect("tp exists");
    assert_eq!(fresh.status, TouchpointStatus::TimeoutDefaulted);
    // 机器 actor 用裸 kind:id 形态（human_audit_actor 的 "human:" 前缀
    // 只属于人；timeout-sweep 审计同款约定）。
    assert!(
        fresh.audit.contains("\"hang\"")
            && fresh.audit.contains("agent:node-x")
            && fresh.audit.contains("unattended-direct"),
        "audit 缺直通 hang（agent:node-x/unattended-direct）: {}",
        fresh.audit
    );
    assert!(
        fresh.audit.contains("\"timeout_default\"")
            && fresh.audit.contains("timeout-sweep")
            && fresh.audit.contains("\"unattended\""),
        "audit 缺默认收口（timeout-sweep/timeout_default/unattended）: {}",
        fresh.audit
    );
    // 人工输入槽记录默认动作 + 来源（汇报包替决假设清单的数据源）。
    assert!(
        fresh.human_input.contains("\"origin\":\"unattended\"")
            && fresh.human_input.contains("default_action"),
        "human_input 缺 origin/default_action: {}",
        fresh.human_input
    );

    // 静默面：无挂卡评论、无 inbox 通知、承载单未被 helper 转移。
    let comments = store.list_comments(issue.id).unwrap();
    assert!(
        !comments.iter().any(|c| c.content.contains("触点已挂卡")),
        "直通 helper 不应发挂卡评论（评论归调用方）"
    );
    let notifs = store
        .list_notifications("admin", Some("dashboard"), false, 50)
        .expect("list notifications");
    assert!(
        !notifs.iter().any(|n| n.kind == "touchpoint_pending"),
        "直通 helper 不应发挂卡通知（通知归调用方）"
    );
    assert_eq!(
        store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "helper 不转移承载单——收货语义归调用方"
    );
}
