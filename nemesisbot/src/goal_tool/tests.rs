//! goal_tool 单测（独立文件纪律——生产文件零内联测试）：
//! - find_bound_goal：未绑定 None / done 跳过 / 多条最新者胜；
//! - add：正常建（created_by=agent + goal.md 落盘）/ 撞既有未完成拒 /
//!   content 缺失拒 / 未知 action 拒 / 空会话键拒；
//! - delete：自建可删三清 / 人建拒；
//! - disable：自建直接停 / 人建受 allow_agent_disable 开关（true 放行 /
//!   false 拒）/ paused 再 disable 拒；
//! - add 后同会话再 add 拒；done 后再 add 放行新 id。

use super::*;
use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::Tool;
use nemesis_goals::{GoalCreatedBy, GoalEntry, GoalStatus};

const SESSION_KEY: &str = "agent:main:session:bench-goal-a";

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat-1", "user", SESSION_KEY)
}

fn unbound_ctx() -> RequestContext {
    RequestContext::new("web", "chat-2", "user", "agent:main:session:unbound")
}

fn no_session_ctx() -> RequestContext {
    RequestContext::new("web", "chat-3", "user", "")
}

fn tool(ws: &std::path::Path) -> GoalTool {
    GoalTool::new(ws.to_path_buf(), None)
}

fn seed_goal(
    ws: &std::path::Path,
    id: &str,
    created_by: GoalCreatedBy,
    status: GoalStatus,
) -> GoalEntry {
    let mut e = GoalEntry::new(
        id.to_string(),
        SESSION_KEY.to_string(),
        created_by,
        None,
        None,
        now_ms(),
    );
    e.status = status;
    let entries = vec![e.clone()];
    nemesis_goals::save_registry(ws, &entries).unwrap();
    nemesis_goals::write_goal_md(ws, id, "seed").unwrap();
    e
}

#[test]
fn find_bound_goal_none_when_unbound() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_goal(ws, "G1", GoalCreatedBy::Agent, GoalStatus::Active);
    assert!(
        find_bound_goal(ws, "agent:main:session:other")
            .unwrap()
            .is_none()
    );
}

#[test]
fn find_bound_goal_skips_done_and_prefers_newest() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // done 条目不占会话；两条活跃取 updated_at 最新。
    seed_goal(ws, "G1", GoalCreatedBy::Agent, GoalStatus::Done);
    let mut g2 = seed_goal(ws, "G2", GoalCreatedBy::Agent, GoalStatus::Active);
    g2.updated_at_ms = 1000;
    nemesis_goals::save_registry(ws, &[g2.clone()]).unwrap();
    let mut g3 = GoalEntry::new(
        "G3".to_string(),
        SESSION_KEY.to_string(),
        GoalCreatedBy::Agent,
        None,
        None,
        2000,
    );
    g3.updated_at_ms = 2000;
    nemesis_goals::update_registry(ws, |entries| {
        entries.push(g3);
        Ok(())
    })
    .unwrap();
    let hit = find_bound_goal(ws, SESSION_KEY).unwrap().unwrap();
    assert_eq!(hit.id, "G3");
}

#[tokio::test]
async fn add_creates_agent_goal_with_content() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let out = tool(ws)
        .execute(
            r#"{"action":"add","content":"修复登录 bug\n验收：测试全绿","max_rounds":10,"eval_model":"small"}"#,
            &ctx(),
        )
        .await
        .unwrap();
    assert!(out.contains("G1"), "unexpected: {out}");
    let entries = nemesis_goals::load_registry(ws).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].created_by, GoalCreatedBy::Agent);
    assert_eq!(entries[0].session_key, SESSION_KEY);
    assert_eq!(entries[0].max_rounds, 10);
    assert_eq!(entries[0].eval_model.as_deref(), Some("small"));
    assert_eq!(
        nemesis_goals::read_goal_md(ws, "G1").unwrap().unwrap(),
        "修复登录 bug\n验收：测试全绿"
    );
}

#[tokio::test]
async fn add_rejects_existing_incomplete_goal() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_goal(ws, "G1", GoalCreatedBy::Human, GoalStatus::Active);
    let err = tool(ws)
        .execute(r#"{"action":"add","content":"新目标"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("未完成目标"), "unexpected: {err}");
    // paused 同样算未完成。
    nemesis_goals::update_registry(ws, |entries| {
        entries[0].status = GoalStatus::Paused;
        Ok(())
    })
    .unwrap();
    let err = tool(ws)
        .execute(r#"{"action":"add","content":"新目标"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("未完成目标"), "unexpected: {err}");
}

#[tokio::test]
async fn add_allows_new_goal_after_done() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_goal(ws, "G1", GoalCreatedBy::Agent, GoalStatus::Done);
    let out = tool(ws)
        .execute(r#"{"action":"add","content":"新目标"}"#, &ctx())
        .await
        .unwrap();
    assert!(out.contains("G2"), "unexpected: {out}");
}

#[tokio::test]
async fn add_rejects_missing_content_or_bad_action() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let t = tool(ws);
    let err = t.execute(r#"{"action":"add"}"#, &ctx()).await.unwrap_err();
    assert!(err.contains("content"), "unexpected: {err}");
    let err = t
        .execute(r#"{"action":"enable"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("未知 action"), "unexpected: {err}");
    let err = t
        .execute(r#"{"action":"add","content":"x"}"#, &no_session_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("会话键"), "unexpected: {err}");
    let err = t.execute("not json", &ctx()).await.unwrap_err();
    assert!(err.contains("JSON"), "unexpected: {err}");
}

#[tokio::test]
async fn delete_removes_own_goal_only() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // 自建：可删三清。
    seed_goal(ws, "G1", GoalCreatedBy::Agent, GoalStatus::Active);
    nemesis_goals::append_eval(
        ws,
        "G1",
        &nemesis_goals::EvalRecord {
            at_ms: 1,
            round: 1,
            verdict: "continue".to_string(),
            gap: None,
            model: "m".to_string(),
            duration_ms: 1,
            raw_excerpt: String::new(),
        },
    )
    .unwrap();
    let out = tool(ws)
        .execute(r#"{"action":"delete"}"#, &ctx())
        .await
        .unwrap();
    assert!(out.contains("G1"), "unexpected: {out}");
    assert!(nemesis_goals::load_registry(ws).unwrap().is_empty());
    assert!(nemesis_goals::read_goal_md(ws, "G1").unwrap().is_none());
    assert!(nemesis_goals::read_evals_tail(ws, "G1", 10).is_empty());

    // 人建：拒。
    seed_goal(ws, "G2", GoalCreatedBy::Human, GoalStatus::Active);
    let err = tool(ws)
        .execute(r#"{"action":"delete"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("人工创建"), "unexpected: {err}");
}

#[tokio::test]
async fn delete_and_disable_unbound_session_honest_error() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let t = tool(ws);
    let err = t
        .execute(r#"{"action":"delete"}"#, &unbound_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("未绑定"), "unexpected: {err}");
    let err = t
        .execute(r#"{"action":"disable"}"#, &unbound_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("未绑定"), "unexpected: {err}");
}

#[tokio::test]
async fn disable_own_goal_then_guard_again() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_goal(ws, "G1", GoalCreatedBy::Agent, GoalStatus::Active);
    let out = tool(ws)
        .execute(r#"{"action":"disable"}"#, &ctx())
        .await
        .unwrap();
    assert!(
        out.contains("G1") && out.contains("paused"),
        "unexpected: {out}"
    );
    let entries = nemesis_goals::load_registry(ws).unwrap();
    assert_eq!(entries[0].status, GoalStatus::Paused);
    // paused 再 disable = 非法转移。
    let err = tool(ws)
        .execute(r#"{"action":"disable"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("不允许转移"), "unexpected: {err}");
}

#[tokio::test]
async fn disable_human_goal_blocked_by_config_switch() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_goal(ws, "G1", GoalCreatedBy::Human, GoalStatus::Active);

    // 开关缺省（无 config）= true 允许。
    let out = GoalTool::new(ws.to_path_buf(), None)
        .execute(r#"{"action":"disable"}"#, &ctx())
        .await
        .unwrap();
    assert!(out.contains("paused"), "unexpected: {out}");

    // 复位 + config 显式 false = 拒。
    nemesis_goals::update_registry(ws, |entries| {
        entries[0].status = GoalStatus::Active;
        Ok(())
    })
    .unwrap();
    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"model_list":[],"goals":{"allow_agent_disable":false}}"#,
    )
    .unwrap();
    let err = GoalTool::new(ws.to_path_buf(), Some(cfg_path))
        .execute(r#"{"action":"disable"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("allow_agent_disable"), "unexpected: {err}");
}

#[test]
fn allow_agent_disable_defaults_and_reads() {
    // None = 缺省 true（fail-open）。
    assert!(allow_agent_disable(None));
    // 显式 true / false。
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("c.json");
    std::fs::write(
        &p,
        r#"{"model_list":[],"goals":{"allow_agent_disable":false}}"#,
    )
    .unwrap();
    assert!(!allow_agent_disable(Some(&p)));
    std::fs::write(
        &p,
        r#"{"model_list":[],"goals":{"allow_agent_disable":true}}"#,
    )
    .unwrap();
    assert!(allow_agent_disable(Some(&p)));
    // goals 段缺席 = true；文件损坏 = true（fail-open）。
    std::fs::write(&p, r#"{"model_list":[]}"#).unwrap();
    assert!(allow_agent_disable(Some(&p)));
    std::fs::write(&p, "{broken").unwrap();
    assert!(allow_agent_disable(Some(&p)));
}
