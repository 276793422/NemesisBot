//! todo 收尾提醒 hook 测试（2026-09-24；2026-11-22 语义反转随批更新）。
//!
//! 覆盖：① 未完成清单 → Continue + 提醒点名未完成项且方向=继续执行；
//! ② 全 completed → Stop；③ 清单文件缺失 → Stop；④ 清单损坏 → Stop
//! （fail-open）；⑤ 首拦模板=「继续用工具执行」、⑥ `stop_hook_active`
//! 升级模板（不再是自限跳过）；⑦ 写读同源回归——真实 `TodoWriteTool`
//! 落盘的文件 hook 必须读到（sanitize/路径同构，F）。

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
                feedback.contains("继续用工具执行"),
                "first block commands resuming work, not reporting: {feedback}"
            );
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
        TurnEndDecision::Inject { .. } => {
            panic!("todo closeout reminder must Continue, never Inject")
        }
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
async fn stop_hook_active_escalates_instead_of_skipping() {
    let ws = unique_workspace("escalation");
    write_via_tool(
        &ws,
        &serde_json::json!({
            "todos": [{"content": "没做完的活", "status": "in_progress"}]
        })
        .to_string(),
    )
    .await;

    // v2 语义反转：本轮已被 Continue 过一次（stop_hook_active=true）不再是
    // 自限跳过，而是升级拦停——模板从「继续执行」换成「列证据或继续干活」，
    // 第 3 次停车由 loop 侧 MAX_TURN_END_CONTINUES=2 硬封顶放行。
    match hook_with(&ws).on_turn_end(&turn_end(true)).await {
        TurnEndDecision::Continue { feedback } => {
            assert!(
                feedback.contains("再次"),
                "escalation names the repeated stop: {feedback}"
            );
            assert!(
                feedback.contains("证据清单"),
                "escalation demands evidence for user-domain blocks: {feedback}"
            );
            assert!(
                !feedback.contains("继续用工具执行，不要以汇报收摊"),
                "escalation must not repeat the first-block template verbatim: {feedback}"
            );
        }
        TurnEndDecision::Stop => {
            panic!("escalation must still block (loop cap fail-opens the 3rd)")
        }
        TurnEndDecision::Inject { .. } => {
            panic!("todo closeout reminder must Continue, never Inject")
        }
    }
}

#[tokio::test]
async fn first_block_and_escalation_templates_differ() {
    // 同一清单两种停车状态产出不同模板（首拦=继续执行，升级=列证据/继续）。
    let ws = unique_workspace("templates_differ");
    write_via_tool(
        &ws,
        &serde_json::json!({
            "todos": [{"content": "跨模板项", "status": "pending"}]
        })
        .to_string(),
    )
    .await;

    let first = match hook_with(&ws).on_turn_end(&turn_end(false)).await {
        TurnEndDecision::Continue { feedback } => feedback,
        _ => panic!("first stop must Continue"),
    };
    let second = match hook_with(&ws).on_turn_end(&turn_end(true)).await {
        TurnEndDecision::Continue { feedback } => feedback,
        _ => panic!("second stop must escalate with Continue"),
    };
    assert_ne!(first, second, "two stages must use distinct templates");
}

#[tokio::test]
async fn writer_reader_parity_different_session_keys() {
    // 写读同源：多种 session_key 形态（含 F- 的 '/' 复合键）经真实
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
            TurnEndDecision::Inject { .. } => {
                panic!("todo closeout reminder must Continue, never Inject")
            }
        }
    }
}

// wave5：LifecycleHook::name 恒等（53-55）。
#[test]
fn hook_name_is_todo_closeout_reminder() {
    let hook = TodoCloseoutHook::new(std::env::temp_dir());
    assert_eq!(LifecycleHook::name(&hook), "todo-closeout-reminder");
}
