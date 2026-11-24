//! （BUG-20261124-001 根修）：checkpoint attach 的 turn 计数器自举回归锁。
//!
//! turn 序号是 loop 内存计数（`turn_counter.fetch_add`，重启归零），而
//! checkpoint 索引持久化跨世代存活。`set_checkpoint_store` 必须把计数器
//! 顶到存量之上（`fetch_max(max_turn + 1)`）——否则重启后新世代 mint 的
//! 低号 turn 与盘上旧条目撞号，rewind 波及面/恢复靶树全被旧世代条目
//! 污染。git 形态（begin 落盘需要 tree）。

use super::*;

struct Bug24NoopProvider;

#[async_trait]
impl LlmProvider for Bug24NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn bug24_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn bug24_create(rel: &str) -> FileChange {
    FileChange {
        path: rel.to_string(),
        kind: FileChangeKind::Create,
    }
}

/// 盘上存量最高 turn = 5（上一世代封印落盘）→ attach 后计数器自举到 6：
/// 本世代首个新 mint 的 turn（fetch_add 返回 6）不再与旧条目撞号。
#[tokio::test]
async fn checkpoint_attach_bootstraps_turn_counter_above_disk_max() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    let cp_dir = root.join("logs").join("checkpoints");

    // 上一世代：turn 5 封印落盘。
    {
        let store = crate::checkpoint::CheckpointStore::new(Some(cp_dir.clone()), root.clone());
        std::fs::write(root.join("f.txt"), "v0").unwrap();
        store.begin_for_session(5, "old-gen", Some("old-session"));
        std::fs::write(root.join("f5.txt"), "v5").unwrap();
        store
            .snapshot_for_session(&bug24_create("f5.txt"), Some("old-session"))
            .await;
        store.seal_turn(5);
        assert!(
            cp_dir.join("turn-5.json").exists(),
            "封印条目必须落盘（重载前提）"
        );
    } // drop = 上一世代进程退出

    // 本世代：计数器从 0 起，attach 存量索引 → fetch_max(5+1)。
    let al = AgentLoop::new(Box::new(Bug24NoopProvider), bug24_config());
    assert_eq!(
        al.turn_counter.load(Ordering::Relaxed),
        0,
        "新 loop 计数器从 0 起（撞号根床前提）"
    );
    al.set_checkpoint_store(Arc::new(crate::checkpoint::CheckpointStore::new(
        Some(cp_dir),
        root,
    )));
    assert_eq!(
        al.turn_counter.load(Ordering::Relaxed),
        6,
        "attach 后计数器必须顶到存量 max+1（turn = fetch_add 前值，0 起始）"
    );

    // 空索引（None）不得动计数器。
    let al2 = AgentLoop::new(Box::new(Bug24NoopProvider), bug24_config());
    al2.turn_counter.store(3, Ordering::Relaxed);
    al2.set_checkpoint_store(Arc::new(crate::checkpoint::CheckpointStore::new(
        None,
        dir.path().to_path_buf(),
    )));
    assert_eq!(
        al2.turn_counter.load(Ordering::Relaxed),
        3,
        "空索引自举是 no-op，不得回拨计数器"
    );
}
