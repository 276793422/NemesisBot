//! P33（2026-09-25 能力扩展计划 WS7）：中断安全重放单测。
//!
//! 覆盖：挂起 tool_use 检测 / interrupted 折算注入（含调用序、混合应答、
//! 直通）/ 折算可被真实结果替换（merge_real_tool_result 占位契约）/
//! 相位机合法链与非法转移 loud 拒绝 / recover_to_manager 端到端（折算 +
//! 写回 + 相位标记 + 无挂起直通）。

use super::*;
use crate::r#loop::LlmMessage;
use crate::loop_continuation::{ContinuationManager, ContinuationStore};
use crate::types::ToolCallInfo;

// —— 摆件 ——

fn user(content: &str) -> LlmMessage {
    LlmMessage {
        role: "user".to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }
}

fn assistant_with_calls(calls: &[(&str, &str)]) -> LlmMessage {
    LlmMessage {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: Some(
            calls
                .iter()
                .map(|(id, name)| ToolCallInfo {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments: "{}".to_string(),
                })
                .collect(),
        ),
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }
}

fn tool_result(id: &str, content: &str) -> LlmMessage {
    LlmMessage {
        role: "tool".to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: Some(id.to_string()),
        reasoning_content: None,
        images: Vec::new(),
    }
}

// —— 检测 ——

#[test]
fn p33_detects_pending_and_answered_calls() {
    let msgs = vec![
        user("q"),
        assistant_with_calls(&[("tc_ok", "read_file"), ("tc_hang", "exec")]),
        tool_result("tc_ok", "done"),
    ];
    let pending = find_pending_tool_calls(&msgs);
    assert_eq!(pending, vec!["tc_hang".to_string()], "只挂起的算 pending");
}

#[test]
fn p33_detects_none_when_all_answered() {
    let msgs = vec![
        user("q"),
        assistant_with_calls(&[("tc_1", "exec")]),
        tool_result("tc_1", "done"),
    ];
    assert!(find_pending_tool_calls(&msgs).is_empty());
}

// —— 折算注入 ——

#[test]
fn p33_synthesis_injects_interrupted_result_after_assistant() {
    let msgs = vec![user("q"), assistant_with_calls(&[("tc_9", "exec")])];
    let (out, ids) = synthesize_interrupted_results(msgs);
    assert_eq!(ids, vec!["tc_9".to_string()]);
    assert_eq!(out.len(), 3, "折算行插在 assistant 之后");
    let synth = &out[2];
    assert_eq!(synth.role, "tool");
    assert_eq!(synth.tool_call_id.as_deref(), Some("tc_9"));
    // 契约内容：status=interrupted + 钦定注记 + merge 可识别的占位前缀。
    assert!(
        synth
            .content
            .contains(&format!("\"status\":\"{INTERRUPTED_STATUS}\"")),
        "{}",
        synth.content
    );
    assert!(
        synth.content.contains(INTERRUPTED_NOTE),
        "{}",
        synth.content
    );
    assert!(
        synth
            .content
            .starts_with(&format!("[{TOOL_OUTCOME_UNKNOWN}]")),
        "merge_real_tool_result 占位识别依赖此前缀: {}",
        synth.content
    );
}

#[test]
fn p33_synthesis_multiple_pending_keep_call_order() {
    let msgs = vec![
        user("q"),
        assistant_with_calls(&[("tc_a", "exec"), ("tc_b", "write_file")]),
    ];
    let (out, ids) = synthesize_interrupted_results(msgs);
    assert_eq!(ids, vec!["tc_a".to_string(), "tc_b".to_string()]);
    assert_eq!(out[2].tool_call_id.as_deref(), Some("tc_a"), "同批按调用序");
    assert_eq!(out[3].tool_call_id.as_deref(), Some("tc_b"));
}

#[test]
fn p33_synthesis_mixed_batch_only_pends() {
    let msgs = vec![
        user("q"),
        assistant_with_calls(&[("tc_ok", "read_file"), ("tc_hang", "exec")]),
        tool_result("tc_ok", "content"),
        user("next"),
    ];
    let (out, ids) = synthesize_interrupted_results(msgs);
    assert_eq!(ids, vec!["tc_hang".to_string()]);
    // tc_ok 的应答原样保留；折算行插在 assistant 之后（tc_ok 应答之前）。
    assert_eq!(out[2].tool_call_id.as_deref(), Some("tc_hang"));
    assert_eq!(out[3].tool_call_id.as_deref(), Some("tc_ok"));
    assert_eq!(out[3].content, "content");
}

#[test]
fn p33_synthesis_passthrough_when_no_pending() {
    let msgs = vec![
        user("q"),
        assistant_with_calls(&[("tc_1", "exec")]),
        tool_result("tc_1", "done"),
        user("q2"),
    ];
    let before = serde_json::to_string(&msgs).unwrap();
    let (out, ids) = synthesize_interrupted_results(msgs);
    assert!(ids.is_empty(), "无挂起 = 直通不折算");
    assert_eq!(serde_json::to_string(&out).unwrap(), before, "消息面零变化");
}

// —— merge 占位契约：真实结果到达时替换折算（不产生双 tool 消息）——

#[test]
fn p33_real_result_replaces_interrupted_placeholder() {
    let msgs = vec![user("q"), assistant_with_calls(&[("tc_1", "cluster_rpc")])];
    let (out, ids) = synthesize_interrupted_results(msgs);
    assert_eq!(ids, vec!["tc_1".to_string()]);
    let merged = crate::loop_continuation::merge_real_tool_result(out, "tc_1", "REAL".to_string());
    assert_eq!(merged.len(), 3, "替换非追加——同 id 仍只有一条 tool 消息");
    assert_eq!(merged[2].content, "REAL");
}

// —— 相位机 ——

#[test]
fn p33_phase_machine_legal_chain() {
    // 恢复链：checkpoint → restored → confirmed。
    validate_restore_transition(RestorePhase::Checkpoint, RestorePhase::Restored).unwrap();
    validate_restore_transition(RestorePhase::Restored, RestorePhase::Confirmed).unwrap();
    // 存活快照直达：checkpoint → confirmed。
    validate_restore_transition(RestorePhase::Checkpoint, RestorePhase::Confirmed).unwrap();
}

#[test]
fn p33_phase_machine_rejects_illegal_transitions() {
    let illegal = [
        (RestorePhase::Checkpoint, RestorePhase::Checkpoint),
        (RestorePhase::Restored, RestorePhase::Restored),
        (RestorePhase::Restored, RestorePhase::Checkpoint),
        (RestorePhase::Confirmed, RestorePhase::Checkpoint),
        (RestorePhase::Confirmed, RestorePhase::Restored),
        (RestorePhase::Confirmed, RestorePhase::Confirmed),
    ];
    for (from, to) in illegal {
        let err = validate_restore_transition(from, to).unwrap_err();
        assert!(
            err.contains("非法快照恢复相位转移"),
            "{from:?}→{to:?}: {err}"
        );
    }
}

// —— recover_to_manager 端到端 ——

fn p33_tmp_ws(tag: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join(tag);
    std::fs::create_dir_all(&ws).unwrap();
    (dir, ws)
}

fn p33_uniq_task(tag: &str) -> String {
    format!(
        "task_{tag}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// 带未完成工具的快照恢复 → interrupted 折算注入（内存 + 盘上写回）+
/// 相位标记 Restored（重复恢复转移 loud 拒绝）。
#[tokio::test]
async fn p33_recovery_synthesizes_interrupted_and_writes_back() {
    let (_dir, ws) = p33_tmp_ws("p33ws");
    let store = ContinuationStore::new(&ws);
    let task = p33_uniq_task("hang");
    let snapshot = crate::loop_continuation::ContinuationSnapshot {
        task_id: task.clone(),
        messages: serde_json::to_string(&vec![
            user("q"),
            assistant_with_calls(&[("tc_hang", "cluster_rpc")]),
        ])
        .unwrap(),
        tool_call_id: "tc_hang".to_string(),
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        session_key: "agent:main:session:p33".to_string(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(),
        created_at: chrono::Local::now().to_rfc3339(),
        final_persisted: false,
    };
    store.save(&snapshot).unwrap();

    let manager = ContinuationManager::new();
    let recovered = store.recover_to_manager(&manager);
    assert_eq!(recovered, 1);

    // 内存侧：加载到的消息带 interrupted 折算行。
    let data = manager
        .load_continuation(&task)
        .await
        .expect("恢复的快照可加载");
    assert_eq!(data.messages.len(), 3);
    assert_eq!(data.messages[2].role, "tool");
    assert_eq!(data.messages[2].tool_call_id.as_deref(), Some("tc_hang"));
    assert!(data.messages[2].content.contains(INTERRUPTED_NOTE));

    // 盘上写回：重读磁盘快照同样带折算（重启/再恢复读到折算后真相）。
    let reloaded = store.load(&task).unwrap();
    let disk_msgs: Vec<LlmMessage> = serde_json::from_str(&reloaded.messages).unwrap();
    assert_eq!(disk_msgs.len(), 3);
    assert!(disk_msgs[2].content.contains(INTERRUPTED_NOTE));

    // 相位机：恢复后相位 = Restored（重复 Restored 转移非法 → loud 拒绝）。
    let err = manager.mark_restored(&task).unwrap_err();
    assert!(err.contains("非法快照恢复相位转移"), "{err}");
    // 恢复态确认续行合法。
    manager.confirm_continuation(&task).unwrap();
    // 重复确认非法。
    assert!(manager.confirm_continuation(&task).is_err());
}

/// 无未完成工具的快照照旧直通（消息面零变化、无写回）。
#[tokio::test]
async fn p33_recovery_passthrough_without_pending_tools() {
    let (_dir, ws) = p33_tmp_ws("p33ws2");
    let store = ContinuationStore::new(&ws);
    let task = p33_uniq_task("clean");
    let messages = vec![
        user("q"),
        assistant_with_calls(&[("tc_done", "read_file")]),
        tool_result("tc_done", "content"),
    ];
    let messages_json = serde_json::to_string(&messages).unwrap();
    let snapshot = crate::loop_continuation::ContinuationSnapshot {
        task_id: task.clone(),
        messages: messages_json.clone(),
        tool_call_id: "tc_done".to_string(),
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        session_key: "agent:main:session:p33b".to_string(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(),
        created_at: chrono::Local::now().to_rfc3339(),
        final_persisted: false,
    };
    store.save(&snapshot).unwrap();

    let manager = ContinuationManager::new();
    assert_eq!(store.recover_to_manager(&manager), 1);
    let data = manager.load_continuation(&task).await.unwrap();
    assert_eq!(
        serde_json::to_string(&data.messages).unwrap(),
        messages_json,
        "无挂起 = 直通，消息面零变化"
    );
    // 相位仍标记 Restored（恢复过盘上快照）——确认续行合法。
    manager.confirm_continuation(&task).unwrap();
}
