//! loop_state_tool 单测（独立文件纪律——生产文件零内联测试）。

use super::*;
use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::Tool;
use nemesis_loop::{load_loop, save_loop};

const SESSION_KEY: &str = "agent:main:session:bench-loop-a";

fn seed_loop(dir: &std::path::Path, id: &str, status: LoopStatus) -> LoopState {
    let mut st = LoopState::new(
        id.to_string(),
        "把仓库 issue 清零".to_string(),
        SESSION_KEY.to_string(),
        now_ms(),
    );
    st.status = status;
    save_loop(dir, &st).unwrap();
    st
}

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat-1", "user", SESSION_KEY)
}

fn other_ctx() -> RequestContext {
    RequestContext::new("web", "chat-2", "user", "agent:main:session:unbound")
}

fn wakeup(dir: &std::path::Path) -> LoopWakeupTool {
    LoopWakeupTool::new(
        dir.to_path_buf(),
        Arc::new(nemesis_agent::estop::EstopState::new()),
    )
}

#[test]
fn find_bound_loop_none_when_unbound() {
    let dir = tempfile::tempdir().unwrap();
    let loops = dir.path().join("loops");
    std::fs::create_dir_all(&loops).unwrap();
    seed_loop(&loops, "L1", LoopStatus::Active);
    let hits = find_bound_loop(&loops, "agent:main:session:other").unwrap();
    assert!(hits.is_none());
}

#[test]
fn find_bound_loop_newest_wins_on_multi() {
    let dir = tempfile::tempdir().unwrap();
    let loops = dir.path().join("loops");
    std::fs::create_dir_all(&loops).unwrap();
    let mut old = seed_loop(&loops, "L1", LoopStatus::Active);
    old.updated_at_ms = 1000;
    save_loop(&loops, &old).unwrap();
    let mut new = seed_loop(&loops, "L2", LoopStatus::Active);
    new.updated_at_ms = 2000;
    save_loop(&loops, &new).unwrap();
    let hit = find_bound_loop(&loops, SESSION_KEY).unwrap().unwrap();
    assert_eq!(hit.id, "L2");
}

#[tokio::test]
async fn wakeup_unbound_session_honest_error() {
    let dir = tempfile::tempdir().unwrap();
    let loops = dir.path().join("loops");
    std::fs::create_dir_all(&loops).unwrap();
    let err = wakeup(&loops)
        .execute(r#"{"delay_secs": 60}"#, &other_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("未绑定"), "unexpected: {err}");
}

#[tokio::test]
async fn wakeup_estop_freeze_rejects_scheduling() {
    let dir = tempfile::tempdir().unwrap();
    let loops = dir.path().join("loops");
    std::fs::create_dir_all(&loops).unwrap();
    seed_loop(&loops, "L1", LoopStatus::Active);
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    let tool = LoopWakeupTool::new(loops.clone(), estop);
    let err = tool
        .execute(r#"{"delay_secs": 60}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("急停"), "unexpected: {err}");
}

#[tokio::test]
async fn wakeup_paused_parked_done_rejected_with_human_hint() {
    let dir = tempfile::tempdir().unwrap();
    let loops = dir.path().join("loops");
    std::fs::create_dir_all(&loops).unwrap();
    for (id, status) in [
        ("L1", LoopStatus::Paused),
        ("L2", LoopStatus::Parked),
        ("L3", LoopStatus::Done),
    ] {
        seed_loop(&loops, id, status);
    }
    let tool = wakeup(&loops);
    for id in ["L1", "L2", "L3"] {
        let mut st = load_loop(&loops, id).unwrap().unwrap();
        st.session_key = SESSION_KEY.to_string();
        save_loop(&loops, &st).unwrap();
    }
    let err = tool
        .execute(r#"{"delay_secs": 60}"#, &ctx())
        .await
        .unwrap_err();
    // paused/parked 最新（updated_at 同刻取后者——防御性收敛取其一即可）。
    assert!(
        err.contains("仅人可暂停/恢复") || err.contains("终态"),
        "unexpected: {err}"
    );
}

#[tokio::test]
async fn wakeup_schedules_one_shot_and_updates_plan_note() {
    let dir = tempfile::tempdir().unwrap();
    let loops = dir.path().join("loops");
    std::fs::create_dir_all(&loops).unwrap();
    seed_loop(&loops, "L1", LoopStatus::Active);
    let out = wakeup(&loops)
        .execute(
            r#"{"delay_secs": 120, "note": "完成扫描器", "plan": "下一步：修 board"}"#,
            &ctx(),
        )
        .await
        .unwrap();
    assert!(out.contains("已排程"), "unexpected: {out}");
    let st = load_loop(&loops, "L1").unwrap().unwrap();
    assert!(st.next_wake_ms.is_some());
    assert!(st.cron.is_none());
    assert_eq!(st.plan, "下一步：修 board");
    assert_eq!(st.progress_log.len(), 1);
    assert!(st.progress_log[0].text.contains("完成扫描器"));
}

#[tokio::test]
async fn wakeup_cron_and_mutual_exclusion() {
    let dir = tempfile::tempdir().unwrap();
    let loops = dir.path().join("loops");
    std::fs::create_dir_all(&loops).unwrap();
    seed_loop(&loops, "L1", LoopStatus::Active);
    let tool = wakeup(&loops);
    let out = tool
        .execute(r#"{"cron": "0 9 * * *"}"#, &ctx())
        .await
        .unwrap();
    assert!(out.contains("周期 cron"), "unexpected: {out}");
    let st = load_loop(&loops, "L1").unwrap().unwrap();
    assert_eq!(st.cron.as_deref(), Some("0 9 * * *"));
    // 互斥：同时给 delay + cron → 拒。
    let err = tool
        .execute(r#"{"delay_secs": 60, "cron": "0 9 * * *"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("互斥"), "unexpected: {err}");
    // 都不给 → 拒。
    let err = tool.execute("{}", &ctx()).await.unwrap_err();
    assert!(err.contains("必给其一"), "unexpected: {err}");
}

#[tokio::test]
async fn status_view_read_only_with_progress_tail() {
    let dir = tempfile::tempdir().unwrap();
    let loops = dir.path().join("loops");
    std::fs::create_dir_all(&loops).unwrap();
    let mut st = seed_loop(&loops, "L1", LoopStatus::Active);
    for i in 0..7 {
        st.push_progress(now_ms() + i, format!("进度 {i}"));
    }
    save_loop(&loops, &st).unwrap();
    let tool = LoopStatusTool::new(loops.clone());
    assert!(tool.is_read_only());
    let out = tool.execute("{}", &ctx()).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["id"].as_str(), Some("L1"));
    assert_eq!(v["status"].as_str(), Some("active"));
    // progress_tail(5)：7 条取尾部 5。
    assert_eq!(v["progress_tail"].as_array().unwrap().len(), 5);
    assert_eq!(v["progress_tail"][4].as_str(), Some("进度 6"));
}

#[tokio::test]
async fn finish_requires_summary_then_terminal_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let loops = dir.path().join("loops");
    std::fs::create_dir_all(&loops).unwrap();
    let mut st = seed_loop(&loops, "L1", LoopStatus::Active);
    st.next_wake_ms = Some(now_ms() + 60_000);
    st.cron = Some("0 9 * * *".to_string());
    save_loop(&loops, &st).unwrap();
    let tool = LoopFinishTool::new(loops.clone());

    // summary 必填。
    let err = tool
        .execute(r#"{"summary": "  "}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("summary"), "unexpected: {err}");

    let out = tool
        .execute(r#"{"summary": "三个 issue 全部关闭"}"#, &ctx())
        .await
        .unwrap();
    assert!(out.contains("已收尾"), "unexpected: {out}");
    let st = load_loop(&loops, "L1").unwrap().unwrap();
    assert_eq!(st.status, LoopStatus::Done);
    assert!(st.next_wake_ms.is_none());
    assert!(st.cron.is_none());
    assert_eq!(st.finish_summary.as_deref(), Some("三个 issue 全部关闭"));

    // 终态幂等：重复 finish = Ok 不报错。
    let out2 = tool
        .execute(r#"{"summary": "再来一次"}"#, &ctx())
        .await
        .unwrap();
    assert!(out2.contains("终态"), "unexpected: {out2}");
}
