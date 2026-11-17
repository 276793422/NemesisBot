//! durable_loops 资源盘点 provider 测试——三态 / 会话过滤 / 摘要形状。

use crate::resources::DurableLoopsResourceProvider;
use crate::store::{loops_dir, save_loop};
use crate::{LoopState, LoopStatus};
use nemesis_types::resources::{GlobalResourceProvider, ResourceCtx};
use serde_json::json;
use std::path::Path;

fn ctx_for<'a>(session_key: &'a str, ws: &'a Path) -> ResourceCtx<'a> {
    ResourceCtx {
        session_key,
        bare_sid: "s1",
        workspace_root: ws,
    }
}

fn st(id: &str, session: &str) -> LoopState {
    LoopState::new(
        id.to_string(),
        format!("目标{id}"),
        session.to_string(),
        1000,
    )
}

#[test]
fn meta_contract() {
    let p = DurableLoopsResourceProvider::new();
    assert_eq!(p.meta().category_id, "durable_loops");
    assert_eq!(p.meta().label, "目标");
    assert!(!p.meta().transient);
}

#[test]
fn empty_when_loops_dir_absent() {
    let dir = tempfile::tempdir().unwrap();
    let p = DurableLoopsResourceProvider::new();
    let view = p.collect(&ctx_for("agent:main:session:s1", dir.path()));
    assert_eq!(view.status.as_str(), "empty");
    assert_eq!(view.total, 0);
}

#[test]
fn ok_lists_only_this_session_loops() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let d = loops_dir(ws);
    std::fs::create_dir_all(&d).unwrap();
    let mine = st("L1", "agent:main:session:s1");
    save_loop(&d, &mine).unwrap();
    let other = st("L2", "agent:main:session:other");
    save_loop(&d, &other).unwrap();

    let p = DurableLoopsResourceProvider::new();
    let view = p.collect(&ctx_for("agent:main:session:s1", ws));
    assert_eq!(view.status.as_str(), "ok");
    assert_eq!(view.total, 1);
    let loops = view.summary["loops"].as_array().unwrap();
    assert_eq!(loops.len(), 1);
    assert_eq!(loops[0]["id"], json!("L1"));
    assert_eq!(loops[0]["goal"], json!("目标L1"));
    assert_eq!(loops[0]["status"], json!("active"));
    assert_eq!(loops[0]["finished"], json!(false));
    assert_eq!(view.summary["active"], json!(1));
    assert_eq!(view.summary["omitted"], json!(0));
}

#[test]
fn empty_when_dir_exists_but_no_match() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let d = loops_dir(ws);
    std::fs::create_dir_all(&d).unwrap();
    save_loop(&d, &st("L1", "agent:main:session:other")).unwrap();

    let p = DurableLoopsResourceProvider::new();
    let view = p.collect(&ctx_for("agent:main:session:s1", ws));
    assert_eq!(view.status.as_str(), "empty");
}

#[test]
fn finished_loop_counts_active_zero() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let d = loops_dir(ws);
    std::fs::create_dir_all(&d).unwrap();
    let mut done = st("L1", "agent:main:session:s1");
    done.status = LoopStatus::Done;
    done.finish_summary = Some("收工".into());
    save_loop(&d, &done).unwrap();

    let p = DurableLoopsResourceProvider::new();
    let view = p.collect(&ctx_for("agent:main:session:s1", ws));
    assert_eq!(view.total, 1);
    assert_eq!(view.summary["active"], json!(0));
    assert_eq!(view.summary["loops"][0]["finished"], json!(true));
    assert_eq!(view.summary["loops"][0]["finish_summary"], json!("收工"));
}

// 回归锁（2026-11-17 自查修复）：active 必须**全量**统计，不得只数展示
// 截断窗（rows 保前 5）——旧实现 active=rows.filter(...)，第 6 个活跃目标
// 撞上窗内全 finished 时计数错报 0。
#[test]
fn active_counts_beyond_display_window() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let d = loops_dir(ws);
    std::fs::create_dir_all(&d).unwrap();
    // 窗内 5 条全部 finished + 窗外（第 6 条）active。
    for i in 1..=5 {
        let mut done = st(&format!("L{i}"), "agent:main:session:s1");
        done.status = LoopStatus::Done;
        save_loop(&d, &done).unwrap();
    }
    save_loop(&d, &st("L6", "agent:main:session:s1")).unwrap();

    let p = DurableLoopsResourceProvider::new();
    let view = p.collect(&ctx_for("agent:main:session:s1", ws));
    assert_eq!(view.total, 6);
    let loops = view.summary["loops"].as_array().unwrap();
    assert_eq!(loops.len(), 5); // 展示窗截断照旧
    assert_eq!(view.summary["omitted"], json!(1));
    // 关键断言：窗外活跃目标计入 active（旧实现此处错报 0）。
    assert_eq!(view.summary["active"], json!(1));
}
