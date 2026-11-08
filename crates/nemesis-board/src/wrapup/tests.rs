//! 汇报包渲染测试：六节齐备 / 空项目诚实缺席 / 确认过的规格不进替决
//! 清单 / 复核清单聚合。

use super::*;
use crate::assignment::Actor;
use crate::models::{IssueStatus, NewIssue, NewTouchpoint};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// 唯一临时目录（crate 无 tempfile 依赖，与 store/tests.rs 同款）。
fn temp_root(name: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-wrtest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn temp_store(name: &str) -> BoardStore {
    let dir = temp_root(name);
    BoardStore::open(&dir.join("board.db"), "NB").expect("open store")
}

fn admin() -> Actor {
    Actor::admin("tester")
}

fn make_project(store: &BoardStore, name: &str) -> crate::models::Project {
    store
        .create_project(name, "", None, "", "", None, None)
        .unwrap()
}

fn make_issue(store: &BoardStore, title: &str, project_id: i64) -> Issue {
    store
        .create_issue(NewIssue {
            title: title.to_string(),
            description: "说明".to_string(),
            project_id: Some(project_id),
            creator: admin(),
            ..Default::default()
        })
        .unwrap()
}

fn mark_done(store: &BoardStore, id: i64) {
    let actor = Actor::system("test");
    store
        .transition_issue(id, IssueStatus::Todo, &actor)
        .unwrap();
    store
        .transition_issue(id, IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(id, IssueStatus::InReview, &actor)
        .unwrap();
    store
        .transition_issue(id, IssueStatus::Done, &actor)
        .unwrap();
}

fn system_comment(store: &BoardStore, issue_id: i64, content: &str) {
    store
        .add_comment(crate::models::NewComment {
            issue_id,
            author: Actor::system("test"),
            content: content.to_string(),
            parent_id: None,
            ctype: crate::models::CommentType::System,
        })
        .unwrap();
}

fn hang_tp(
    store: &BoardStore,
    issue_id: i64,
    project_id: i64,
    kind: crate::models::TouchpointKind,
    payload: &str,
) -> crate::models::Touchpoint {
    store
        .create_touchpoint(&NewTouchpoint {
            issue_id,
            project_id: Some(project_id),
            kind,
            deadline_ms: 0,
            payload: payload.to_string(),
        })
        .unwrap()
}

#[test]
fn empty_project_renders_honest_absences() {
    let store = temp_store("empty");
    let project = make_project(&store, "空项目");
    let report = render_project_report(&store, project.id, &[]).unwrap();
    assert!(report.contains("# 项目汇报包：空项目"));
    assert!(report.contains("## 一、做了什么"));
    assert!(report.contains("项目下无单据"));
    assert!(report.contains("## 二、未确认规格假设清单"));
    assert!(report.contains("无——本项目的规格都经人工确认"));
    assert!(report.contains("## 三、红队迷路报告"));
    assert!(report.contains("无——本项目无原型触点"));
    assert!(report.contains("## 四、行为闸结果"));
    assert!(report.contains("## 五、预算消耗"));
    assert!(report.contains("无已记账任务"));
    assert!(report.contains("## 六、人工复核清单"));
    assert!(report.contains("永不阻塞"));
}

#[test]
fn full_project_renders_all_six_sections() {
    let store = temp_store("full");
    let project = make_project(&store, "全量项目");
    let a = make_issue(&store, "任务甲", project.id);
    let b = make_issue(&store, "任务乙", project.id);
    mark_done(&store, a.id);
    mark_done(&store, b.id);
    system_comment(&store, a.id, "✅ 验收通过（无人值守自动收货）");

    // 派发完成记录（预算/归因面）。
    store
        .insert_dispatch("task-a", a.id, "node-a", &Actor::system("test"))
        .unwrap();
    store
        .finish_dispatch("task-a", crate::models::dispatch_state::DONE)
        .unwrap();

    // 甲：超时默认的规格触点（替决假设清单素材）+ 通过的原型触点（迷路/闸素材）。
    let spec = hang_tp(
        &store,
        a.id,
        project.id,
        crate::models::TouchpointKind::Spec,
        r#"{"version":"v1","spec":"草案","assumptions":["假设一：单机即可","假设二：无登录"],"distilled":true}"#,
    );
    store
        .transition_touchpoint(
            spec.id,
            crate::models::TouchpointStatus::TimeoutDefaulted,
            r#"{"default_action":"proceed_with_draft","origin":"unattended"}"#,
        )
        .unwrap();
    hang_tp(
        &store,
        a.id,
        project.id,
        crate::models::TouchpointKind::Prototype,
        r#"{"version":1,"machine_gate":"pass","summary":"全部试玩场景通过","report":"{}"}"#,
    );

    // 乙：否决过的原型触点（迷路报告否决计数素材）。
    let proto = hang_tp(
        &store,
        b.id,
        project.id,
        crate::models::TouchpointKind::Prototype,
        r#"{"version":1,"machine_gate":"fail","summary":"碰撞体缺失"}"#,
    );
    store
        .transition_touchpoint(
            proto.id,
            crate::models::TouchpointStatus::VetoedLocal,
            "本地试玩否决",
        )
        .unwrap();

    let usage = vec![("task-a".to_string(), "node-a".to_string(), 100, 50)];
    let report = render_project_report(&store, project.id, &usage).unwrap();

    // ① 表格 + 系统备注。
    assert!(report.contains("## 一、做了什么"));
    assert!(report.contains("任务甲") && report.contains("任务乙"));
    assert!(report.contains("无人值守自动收货"));
    // ② 替决假设清单。
    assert!(report.contains("假设一：单机即可"));
    assert!(report.contains("假设二：无登录"));
    // ③ 迷路报告。
    assert!(report.contains("## 三、红队迷路报告"));
    assert!(report.contains("全部试玩场景通过"));
    assert!(report.contains("否决 1 次"));
    assert!(report.contains("碰撞体缺失"));
    // ④ 行为闸。
    assert!(report.contains("规格蒸馏 ✅"));
    assert!(report.contains("原型机器闸 pass"));
    // ⑤ 预算。
    assert!(report.contains("input 100 + output 50 = 150 tokens"));
    // ⑥ 复核清单：超时直通 + 否决待续 + 关门不锁门线。
    assert!(report.contains("超时按默认流转"));
    assert!(report.contains("origin=unattended"));
    assert!(report.contains("否决待续"));
    assert!(report.contains("以上是标记面，不是闸门"));
}

#[test]
fn confirmed_spec_never_enters_assumptions() {
    let store = temp_store("confirmed");
    let project = make_project(&store, "确认项目");
    let issue = make_issue(&store, "人工确认单", project.id);
    mark_done(&store, issue.id);
    let spec = hang_tp(
        &store,
        issue.id,
        project.id,
        crate::models::TouchpointKind::Spec,
        r#"{"version":"v1","assumptions":["不该出现的假设"]}"#,
    );
    store
        .transition_touchpoint(
            spec.id,
            crate::models::TouchpointStatus::Confirmed,
            "用户确认",
        )
        .unwrap();
    let report = render_project_report(&store, project.id, &[]).unwrap();
    assert!(report.contains("无——本项目的规格都经人工确认"));
    assert!(
        !report.contains("不该出现的假设"),
        "确认过的假设不得进替决清单"
    );
}

#[test]
fn review_section_lists_parked_and_escalations() {
    let store = temp_store("review");
    let project = make_project(&store, "复核项目");
    let issue = make_issue(&store, "停车单", project.id);
    mark_done(&store, issue.id);
    let spec = hang_tp(
        &store,
        issue.id,
        project.id,
        crate::models::TouchpointKind::Spec,
        r#"{"version":"v1"}"#,
    );
    store
        .transition_touchpoint(
            spec.id,
            crate::models::TouchpointStatus::ParkedSync,
            "否决熔断停车",
        )
        .unwrap();
    // escalate_human 决策留痕（activity_log）。
    store
        .add_activity(
            issue.id,
            &Actor::system("review"),
            "auto_decide",
            Some(r#"{"decision":"escalate_human"}"#),
        )
        .unwrap();

    let report = render_project_report(&store, project.id, &[]).unwrap();
    assert!(report.contains("已停车标「需同步沟通」"));
    assert!(report.contains("escalate_human"));
}
