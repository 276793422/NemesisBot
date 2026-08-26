//! S9 覆盖率批次：loop_continuation.rs 剩余未覆盖行。
//! - 164：ContinuationStore::delete 的 remove 失败 warn（快照路径预置为
//!   目录）。
//! - 187：list_pending 的扩展名分支收尾（.json + 非 .json 混合目录）。
//! - 479-483：wait_for_continuation 超时耗尽 → warn + None（直接构造
//!   未就绪条目 + barrier_timeout=0，确定性）。
//! - 931/934：handle_cluster_continuation 最终响应 info! 参数行 + 块收尾
//!   （复刻 simple_response 场景 + subscriber）。
//! - 855-868 / 872-884：结构性死区——execute_tool_for_continuation 是
//!   ContinuationToolResult 的唯一生产者（loop_continuation.rs:954-977 三
//!   个构造点全部 `..Default::default()`，for_user 恒空、is_async 恒
//!   false、silent 恒 true）→ for_user 发送臂与嵌套异步保存臂永假。
//! - 387-392：messages 序列化失败分支（Vec<LlmMessage> 恒可序列化）结构性。
//! - 464-473：双检竞态窗口（drop 锁后到 load 之间就绪）——非确定性窗口，
//!   守卫性质，测试不可达。
//! - 281：metadata().modified() 失败——机器依赖。

use super::*;
use crate::r#loop::LlmResponse;
use crate::test_support::capture_logs;
use async_trait::async_trait;
use std::time::Duration;

fn make_message(role: &str, content: &str) -> LlmMessage {
    LlmMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
    }
}

fn snapshot_for(task: &str) -> ContinuationSnapshot {
    ContinuationSnapshot {
        task_id: task.to_string(),
        messages: "[]".to_string(),
        tool_call_id: "tc_1".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: String::new(),
        created_at: chrono::Local::now().to_rfc3339(),
    }
}

/// 快照路径预置为目录 → delete 的 remove_file 失败 → warn（164-167）。
#[tokio::test]
async fn store_delete_blocked_by_directory_warns() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().expect("tempdir").keep();
    let store = ContinuationStore::new(&dir);
    let task = format!("s9deltask_{}", std::process::id());

    store.save(&snapshot_for(&task)).expect("save");
    let path = dir.join("cluster").join("rpc_cache").join(format!("{}.json", task));
    assert!(path.exists(), "snapshot at {path:?}");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir_all(&path).unwrap(); // 目录挡住 remove_file

    store.delete(&task); // 必须不 panic；warn 分支执行

    assert!(path.is_dir(), "blocker intact");
    let _ = std::fs::remove_dir_all(&dir);
}

/// list_pending：.json 收进列表、非 .json 忽略（181-188 全分支收尾）。
#[test]
fn store_list_pending_mixed_extensions() {
    let dir = tempfile::tempdir().expect("tempdir").keep();
    let store = ContinuationStore::new(&dir);
    let mine = format!("tj_{}", std::process::id());
    store.save(&snapshot_for(&mine)).expect("save");
    let cache = dir.join("cluster").join("rpc_cache");
    std::fs::write(cache.join("not_a_snapshot.txt"), "x").unwrap();
    std::fs::write(cache.join("weird"), "x").unwrap();

    let pending = store.list_pending();
    assert!(pending.contains(&mine), "json snapshot listed: {pending:?}");
    assert!(!pending.iter().any(|p| p.contains("not_a_snapshot")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// wait_for_continuation：条目存在但未就绪 + barrier_timeout=0 →
/// remaining 归零 → warn + None（477-483，确定性）。
#[tokio::test]
async fn wait_for_continuation_expired_deadline_falls_back() {
    let _logs = capture_logs();
    let mut manager = ContinuationManager::new();
    manager.set_barrier_timeout(Duration::ZERO);
    let task = format!("s9wait_{}", std::process::id());

    // 直接构造「已插入、未就绪」的条目（复刻 save_continuation 在
    // 381 insert 之后、411 置 ready 之前的窗口态）。
    {
        let mut conts = manager.continuations.lock().await;
        conts.insert(
            task.clone(),
            Arc::new(ContinuationData {
                messages: vec![make_message("user", "hi")],
                tool_call_id: "tc_1".to_string(),
                channel: "web".to_string(),
                chat_id: "chat1".to_string(),
                session_key: "s9sess".to_string(),
                ready: Arc::new(tokio::sync::Notify::new()),
                ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
        );
    }

    let out = manager.wait_for_continuation(&task).await;
    assert!(out.is_none(), "expired deadline must fall back to None");
}

// ---------- handle_cluster_continuation 最终响应 info 行 ----------

struct OneShotProvider {
    response: LlmResponse,
}

#[async_trait]
impl crate::r#loop::LlmProvider for OneShotProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: self.response.content.clone(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 完整续行：快照 → 工具结果 → LLM 终答 → 出站 + info 字段行求值
/// （928-933 / 934）。
#[tokio::test]
async fn continuation_final_response_logs_info_fields() {
    let _logs = capture_logs();
    let manager = ContinuationManager::new();
    let task = format!("s9final_{}", std::process::id());
    manager
        .save_continuation(
            &task,
            vec![make_message("user", "run it")],
            "tc_1",
            "web",
            "chat9",
            "s9sess",
        )
        .await;

    let provider = OneShotProvider {
        response: LlmResponse {
            content: "final s9 answer".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    };
    let (outbound_tx, mut outbound_rx) =
        tokio::sync::mpsc::channel::<nemesis_types::channel::OutboundMessage>(16);

    handle_cluster_continuation(
        &manager,
        &task,
        "task response",
        false,
        None,
        &provider,
        "test-model",
        &std::collections::HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
    )
    .await;

    let out = outbound_rx
        .try_recv()
        .expect("final response sent to outbound");
    assert_eq!(out.chat_id, "chat9");
    assert!(out.content.contains("final s9 answer"));
}
