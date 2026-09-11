//! board_issue_tool 单测（全自动流转 P3）：args parse 纯函数穷举 + tier 闸
//! 拒 Mini + moderator 槽空诚实报错。

use super::*;
use nemesis_agent::r#loop::Tool as _;

// ---------------- parse_board_issue_args 穷举 ----------------

#[test]
fn parse_create_happy_full() {
    let cmd = parse_board_issue_args(
        r#"{"subcommand":"create","title":"修复登录","description":"背景…","acceptance_criteria":"1. 能登录","priority":2,"project_id":7}"#,
    )
    .expect("full create args must parse");
    match cmd {
        BoardIssueCommand::Create {
            title,
            description,
            acceptance_criteria,
            priority,
            project_id,
        } => {
            assert_eq!(title, "修复登录");
            assert_eq!(description, "背景…");
            assert_eq!(acceptance_criteria.as_deref(), Some("1. 能登录"));
            assert_eq!(priority, Some(2));
            assert_eq!(project_id, Some(7));
        }
        other => panic!("expected Create, got {other:?}"),
    }
}

#[test]
fn parse_create_minimal_defaults() {
    let cmd = parse_board_issue_args(r#"{"subcommand":"create","title":"  建个单  "}"#)
        .expect("minimal create args must parse");
    match cmd {
        BoardIssueCommand::Create {
            title,
            description,
            acceptance_criteria,
            priority,
            project_id,
        } => {
            assert_eq!(title, "建个单"); // trim 生效
            assert_eq!(description, "");
            assert!(acceptance_criteria.is_none());
            assert!(priority.is_none());
            assert!(project_id.is_none());
        }
        other => panic!("expected Create, got {other:?}"),
    }
}

#[test]
fn parse_create_rejects_empty_title() {
    assert!(parse_board_issue_args(r#"{"subcommand":"create"}"#).is_err());
    assert!(parse_board_issue_args(r#"{"subcommand":"create","title":"   "}"#).is_err());
}

#[test]
fn parse_create_rejects_bad_priority() {
    assert!(parse_board_issue_args(r#"{"subcommand":"create","title":"t","priority":4}"#).is_err());
    assert!(
        parse_board_issue_args(r#"{"subcommand":"create","title":"t","priority":-1}"#).is_err()
    );
    assert!(
        parse_board_issue_args(r#"{"subcommand":"create","title":"t","priority":"high"}"#).is_err()
    );
}

#[test]
fn parse_plan_happy_and_alias() {
    let by_issue = parse_board_issue_args(r#"{"subcommand":"plan","issue":"NB-7"}"#)
        .expect("plan by issue must parse");
    let by_alias = parse_board_issue_args(r#"{"subcommand":"plan","issue_id":"7"}"#)
        .expect("plan by issue_id alias must parse");
    for cmd in [by_issue, by_alias] {
        match cmd {
            BoardIssueCommand::Plan { issue_ref } => match issue_ref.as_str() {
                "NB-7" | "7" => {}
                other => panic!("unexpected issue_ref {other:?}"),
            },
            other => panic!("expected Plan, got {other:?}"),
        }
    }
}

#[test]
fn parse_plan_rejects_missing_issue() {
    assert!(parse_board_issue_args(r#"{"subcommand":"plan"}"#).is_err());
    assert!(parse_board_issue_args(r#"{"subcommand":"plan","issue":"  "#).is_err());
}

#[test]
fn parse_rejects_unknown_subcommand_and_bad_json() {
    assert!(parse_board_issue_args(r#"{"subcommand":"delete","title":"t"}"#).is_err());
    assert!(parse_board_issue_args(r#"{"title":"no subcommand"}"#).is_err());
    assert!(parse_board_issue_args("not json").is_err());
}

// ---------------- tier 闸 ----------------

#[test]
fn tier_gate_rejects_mini_with_actionable_message() {
    let err = BoardIssueTool::check_tier(nemesis_types::capability::ModelTier::Mini)
        .expect_err("mini must be rejected");
    assert!(err.contains("mini"), "message should name the tier: {err}");
    assert!(
        err.contains("normal/big"),
        "message should say target: {err}"
    );
}

#[test]
fn tier_gate_allows_normal_big_and_auto() {
    use nemesis_types::capability::ModelTier;
    for tier in [ModelTier::Auto, ModelTier::Normal, ModelTier::Big] {
        BoardIssueTool::check_tier(tier)
            .unwrap_or_else(|e| panic!("tier {tier:?} must pass, got {e}"));
    }
}

// ---------------- moderator 槽空 / store 未就绪诚实报错 ----------------

#[test]
fn execute_with_empty_slot_fails_honestly() {
    let dir = std::env::temp_dir().join(format!("nb_board_issue_tool_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(
        nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("store opens"),
    );
    let tool = BoardIssueTool::new(
        store,
        None,
        Arc::new(std::sync::OnceLock::new()), // 永不填充
        dir.clone(),
        None,
    );
    let ctx = nemesis_agent::context::RequestContext::new("test", "chat", "tester", "sess");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for args in [
        r#"{"subcommand":"create","title":"t"}"#,
        r#"{"subcommand":"plan","issue":"NB-1"}"#,
    ] {
        let err = rt
            .block_on(tool.execute(args, &ctx))
            .expect_err("empty moderator slot must fail honestly");
        assert!(
            err.contains("未就绪") || err.contains("未运行"),
            "honest readiness error expected, got: {err}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
