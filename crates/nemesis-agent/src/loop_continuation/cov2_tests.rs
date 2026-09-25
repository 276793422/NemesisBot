//! loop_continuation 覆盖率收尾批次（Wave6B）：no-vision 投影的重复占位
//! 跳过臂 / 磁盘快照按每轮引用重水合（else 臂）/ list_bg_spawn_pending_sync
//! 三分支（内存 / 磁盘回退 / 无仓）/ 单飞闸认领失败的诚实早退。
//!
//! 自造 helper（兄弟测试模块不能互相 import）。

use super::*;
use crate::r#loop::LlmResponse;
use crate::test_support::capture_logs;
use async_trait::async_trait;
use std::collections::HashMap;

fn make_message(role: &str, content: &str) -> LlmMessage {
    LlmMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }
}

fn cont_data(ready: bool) -> Arc<ContinuationData> {
    Arc::new(ContinuationData {
        messages: Vec::new(),
        tool_call_id: "tc".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(),
        ready: Arc::new(tokio::sync::Notify::new()),
        ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(ready)),
    })
}

/// 最小合法 PNG（magic 8 字节 + 尾量），过 image_path_detector::verify。
fn png_bytes() -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend_from_slice(&[0u8; 16]);
    v
}

// ---------------------------------------------------------------------------
// project_messages_for_no_vision：重复占位跳过（257）
// ---------------------------------------------------------------------------

/// 内容已含同文占位 → 不重复堆叠（257 continue 臂）。
#[test]
fn project_no_vision_skips_duplicate_note() {
    let note = "[图片已省略: 当前模型仅支持文本]";
    let mut msgs = vec![LlmMessage {
        role: "user".to_string(),
        content: format!("look\n{note}"),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: vec![crate::image_attach::LlmImage {
            path: "x.png".to_string(),
            media_type: "image/png".to_string(),
            data: "AAAA".to_string(),
        }],
    }];
    project_messages_for_no_vision(&mut msgs);
    // 图片被清、占位只出现一次（second pass 场景同效）。
    assert!(msgs[0].images.is_empty());
    assert_eq!(msgs[0].content.matches(note).count(), 1);

    // 再跑一遍（images 已空 → 早退 continue），内容不再变。
    project_messages_for_no_vision(&mut msgs);
    assert_eq!(msgs[0].content.matches(note).count(), 1);
}

// ---------------------------------------------------------------------------
// try_load_from_disk：按每轮引用重水合（784-785 else 臂）
// ---------------------------------------------------------------------------

/// image_refs_by_user_turn 非空 → 走逐轮重水合（784-785），两条 user 轮
/// 的图片都水合回来（旧单层语义只救最后一条）。
#[tokio::test]
async fn try_load_from_disk_rehydrates_by_user_turn() {
    let tmp = tempfile::TempDir::new().unwrap();
    let p1 = tmp.path().join("t1.png");
    let p2 = tmp.path().join("t2.png");
    std::fs::write(&p1, png_bytes()).unwrap();
    std::fs::write(&p2, png_bytes()).unwrap();

    let messages = serde_json::to_string(&vec![
        make_message("user", "q1"),
        make_message("assistant", "a1"),
        make_message("user", "q2"),
    ])
    .unwrap();
    let snapshot = ContinuationSnapshot {
        task_id: "bg-covw6b-turns".to_string(),
        messages,
        tool_call_id: "tc_t".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
        session_key: "sk".to_string(),
        peer_id: "peer1".to_string(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: vec![
            vec![p1.to_string_lossy().to_string()],
            vec![p2.to_string_lossy().to_string()],
        ],
        created_at: "t".to_string(),
        final_persisted: false,
    };
    ContinuationStore::new(tmp.path())
        .save(&snapshot)
        .expect("save");

    let manager = ContinuationManager::with_disk_store(tmp.path());
    let loaded = manager
        .try_load_from_disk("bg-covw6b-turns")
        .await
        .expect("snapshot loads");
    // 逐轮重水合：q1 和 q2 都有图（单层语义只会救 q2）。
    assert_eq!(loaded.messages.len(), 3);
    assert!(!loaded.messages[0].images.is_empty(), "q1 应重水合");
    assert!(!loaded.messages[2].images.is_empty(), "q2 应重水合");
    assert_eq!(loaded.session_key, "sk");
    assert_eq!(loaded.peer_id, "peer1");
}

// ---------------------------------------------------------------------------
// list_bg_spawn_pending_sync：三分支（922-930）
// ---------------------------------------------------------------------------

/// 内存分支：try_lock 成功 → 只返回 bg_ 前缀 id（只读不删）。
#[tokio::test]
async fn list_bg_spawn_pending_sync_reads_memory_map() {
    let manager = ContinuationManager::new();
    manager.insert_continuation_sync("bg_covw6b_m1".to_string(), cont_data(false));
    manager.insert_continuation_sync("plain-task".to_string(), cont_data(false));

    let ids = manager.list_bg_spawn_pending_sync();
    assert_eq!(ids, vec!["bg_covw6b_m1".to_string()]);
    // 只读：条目还在。
    assert!(manager.has_continuation_sync("bg_covw6b_m1"));
}

/// 磁盘回退分支：锁被占（try_lock 失败）→ 落盘清单 + bg_ 前缀过滤。
#[tokio::test]
async fn list_bg_spawn_pending_sync_falls_back_to_disk_when_locked() {
    let tmp = tempfile::TempDir::new().unwrap();
    let manager = ContinuationManager::with_disk_store(tmp.path());
    let store = ContinuationStore::new(tmp.path());
    for id in ["bg_covw6b_d1", "plain-d2"] {
        let snapshot = ContinuationSnapshot {
            task_id: id.to_string(),
            messages: "[]".to_string(),
            tool_call_id: "tc".to_string(),
            channel: "web".to_string(),
            chat_id: "c".to_string(),
            session_key: String::new(),
            peer_id: String::new(),
            image_refs: Vec::new(),
            image_refs_by_user_turn: Vec::new(),
            created_at: "t".to_string(),
            final_persisted: false,
        };
        store.save(&snapshot).unwrap();
    }

    // 占住内存 map 锁 → 强制走磁盘分支（928-931）。
    let guard = manager.continuations.try_lock().expect("acquire lock");
    let ids = manager.list_bg_spawn_pending_sync();
    drop(guard);
    assert_eq!(ids, vec!["bg_covw6b_d1".to_string()]);
}

/// 无仓 + 锁被占 → 空清单（933 兜底臂）。
#[tokio::test]
async fn list_bg_spawn_pending_sync_empty_without_store() {
    let manager = ContinuationManager::new();
    let guard = manager.continuations.try_lock().expect("acquire lock");
    assert!(manager.list_bg_spawn_pending_sync().is_empty());
    drop(guard);
}

// ---------------------------------------------------------------------------
// handle_cluster_continuation：单飞闸认领失败早退（1075-1076）
// ---------------------------------------------------------------------------

struct NeverProvider;
#[async_trait]
impl LlmProvider for NeverProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("provider must not be reached on claim failure".to_string())
    }
}

/// 已被认领（重复回调）→ debug + 诚实早退：provider 不被调用、零出站。
#[tokio::test]
async fn claim_failure_early_returns_without_processing() {
    let _logs = capture_logs();
    let manager = ContinuationManager::new();
    assert!(manager.claim_handling("task-covw6b-claim").await);

    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    handle_cluster_continuation(
        &manager,
        "task-covw6b-claim",
        "late response",
        false,
        None,
        &NeverProvider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true,
        None,
    )
    .await;

    assert!(outbound_rx.try_recv().is_err(), "认领失败必须零出站");
    manager.finish_handling("task-covw6b-claim").await;
}
