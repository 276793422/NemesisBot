//! todo 收尾提醒 hook 测试（2026-09-24）。
//!
//! 覆盖：① 未完成清单 → Continue + 提醒点名未完成项；② 全 completed →
//! Stop；③ 清单文件缺失 → Stop；④ 清单损坏 → Stop（fail-open）；⑤
//! `stop_hook_active` 自限跳过；⑥ 写读同源回归——真实 `TodoWriteTool`
//! 落盘的文件 hook 必须读到（sanitize/路径同构，F-U4-4）。

use std::time::{SystemTime, UNIX_EPOCH};

use super::TodoCloseoutHook;
use crate::context::RequestContext;
use crate::hooks::{HookTurnEnd, LifecycleHook, TurnEndDecision};
use crate::r#loop::Tool;
use crate::loop_tools::TodoWriteTool;

/// 每测试唯一的临时 workspace（nanos 后缀避免并行碰撞）。
fn unique_workspace(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nb_todo_closeout_{tag}_{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const SESSION_KEY: &str = "agent:web:session:s1";

fn hook_with(ws: &std::path::Path) -> TodoCloseoutHook {
    TodoCloseoutHook::new(ws.to_path_buf())
}

fn turn_end(stop_hook_active: bool) -> HookTurnEnd {
    HookTurnEnd {
        session_key: SESSION_KEY.to_string(),
        channel: "web".to_string(),
        chat_id: "web:t1".to_string(),
        final_content: "汇总报告交完了".to_string(),
        stop_hook_active,
    }
}

/// 用真实 `TodoWriteTool` 落盘一份清单（与生产写路径完全同构）。
async fn write_via_tool(ws: &std::path::Path, todos_json: &str) {
    let tool = TodoWriteTool::new(ws.to_path_buf(), None);
    let ctx = RequestContext::new("web", "web:t1", "user", SESSION_KEY);
    tool.execute(todos_json, &ctx)
        .await
        .expect("todowrite execute must succeed");
}

#[tokio::test]
async fn unfinished_list_blocks_stop_with_reminder() {
    let ws = unique_workspace("unfinished");
    write_via_tool(
        &ws,
        &serde_json::json!({
            "todos": [
                {"content": "读取四份专利交底书", "status": "completed"},
                {"content": "汇总报告", "status": "in_progress"},
                {"content": "补充附图说明", "status": "pending"}
            ]
        })
        .to_string(),
    )
    .await;

    match hook_with(&ws).on_turn_end(&turn_end(false)).await {
        TurnEndDecision::Continue { feedback } => {
            assert!(feedback.contains("todowrite"), "reminder names the tool");
            assert!(
                feedback.contains("汇总报告"),
                "reminder names the in_progress item: {feedback}"
            );
            assert!(
                feedback.contains("补充附图说明"),
                "reminder names the pending item: {feedback}"
            );
            assert!(
                feedback.contains("不要虚报完成"),
                "reminder carries the honesty rule: {feedback}"
            );
        }
        TurnEndDecision::Stop => panic!("unfinished list must block stopping"),
    }
}

#[tokio::test]
async fn all_completed_stops() {
    let ws = unique_workspace("all_done");
    write_via_tool(
        &ws,
        &serde_json::json!({
            "todos": [
                {"content": "步骤一", "status": "completed"},
                {"content": "步骤二", "status": "completed"}
            ]
        })
        .to_string(),
    )
    .await;

    assert!(matches!(
        hook_with(&ws).on_turn_end(&turn_end(false)).await,
        TurnEndDecision::Stop
    ));
}

#[tokio::test]
async fn missing_file_stops() {
    let ws = unique_workspace("missing");
    assert!(matches!(
        hook_with(&ws).on_turn_end(&turn_end(false)).await,
        TurnEndDecision::Stop
    ));
}

#[tokio::test]
async fn malformed_json_stops_fail_open() {
    let ws = unique_workspace("malformed");
    // 直接落一份损坏文件（工具只认 {todos:[...]} args，坏内容只能来自
    // 外部改写——真实损坏形态）。
    let safe = nemesis_utils::sanitize::sanitize_path_segment(SESSION_KEY);
    let path =
        nemesis_path::resolve_sessions_dir_in_workspace(&ws).join(format!("todo_{safe}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{{{broken").unwrap();

    assert!(matches!(
        hook_with(&ws).on_turn_end(&turn_end(false)).await,
        TurnEndDecision::Stop
    ));
}

#[tokio::test]
async fn stop_hook_active_skips() {
    let ws = unique_workspace("self_limit");
    write_via_tool(
        &ws,
        &serde_json::json!({
            "todos": [{"content": "没做完的活", "status": "in_progress"}]
        })
        .to_string(),
    )
    .await;

    // 本轮已被任何 hook Continue 过一次（stop_hook_active=true）→ 自限跳过。
    assert!(matches!(
        hook_with(&ws).on_turn_end(&turn_end(true)).await,
        TurnEndDecision::Stop
    ));
}

#[tokio::test]
async fn writer_reader_parity_different_session_keys() {
    // 写读同源：多种 session_key 形态（含 F-U4-4 的 '/' 复合键）经真实
    // 工具落盘后，hook 用同一 sanitize 真相源都能找到同一份文件。
    let ws = unique_workspace("parity");
    let tool = TodoWriteTool::new(ws.to_path_buf(), None);
    let body = serde_json::json!({
        "todos": [{"content": "跨节点任务", "status": "in_progress"}]
    })
    .to_string();

    for key in ["agent:web:session:s2", "cluster_rpc:node-a/chat-1"] {
        let ctx = RequestContext::new("web", "web:t1", "user", key);
        tool.execute(&body, &ctx)
            .await
            .expect("todowrite execute must succeed");

        let mut end = turn_end(false);
        end.session_key = key.to_string();
        match hook_with(&ws).on_turn_end(&end).await {
            TurnEndDecision::Continue { feedback } => {
                assert!(feedback.contains("跨节点任务"), "key {key} round-trips");
            }
            TurnEndDecision::Stop => panic!("key {key} must round-trip to the hook"),
        }
    }
}

// wave5：LifecycleHook::name 恒等（53-55）。
#[test]
fn hook_name_is_todo_closeout_reminder() {
    let hook = TodoCloseoutHook::new(std::env::temp_dir());
    assert_eq!(LifecycleHook::name(&hook), "todo-closeout-reminder");
}
