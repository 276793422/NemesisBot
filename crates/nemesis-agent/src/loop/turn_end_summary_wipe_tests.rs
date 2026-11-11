//! turn 末持久化 stale-cache-wipe 回归锁（2026-11-05 压缩成果被抹根修）。
//!
//! 设备实证时序（15:01 窗口）：turn A 装载实例（store 无摘要 ⇒ summary
//! cache = None）→ LLM 进行中（87s），另一 turn 收尾触发的压缩完成并把
//! 摘要写入 store → turn A 末持久化块按**本实例 stale cache** 写回。旧
//! 形态无条件写：None 臂 `set_summary("") + covers=None` 把并发压缩刚写
//! 入的成果抹掉（store 终态「全量历史 + 无摘要」= 用户观测「压了等于
//! 没压」；17:05 手动 /compact 被压缩窗口内到达的 turn 同机制抹掉），
//! Some 臂则把 covers 回退到装载时的旧值——同一竞态的两半。
//!
//! 修复语义：摘要持久化仅在「cache 相对装载时快照变化」时写（唯一变化
//! 路径 = 本 turn 内压缩成功，这正是必须写回的场景）；未变化 ⇒ 不触碰。
//! 合法清空唯一路径是 clear_session（它自己清 store，不经本块）；
//! set_summary_cache(None) 生产零调用，cache 由 Some 变 None 不可达。
//!
//! 注：get_or_create_instance 无实例池、总是新建并从 store 水合——装载
//! 快照即 store 当时的摘要态，所以「未变化 ⇒ 写回是冗余」成立。
//!
//! 测试手法：provider 在 chat() 内（= turn 进行中）把摘要写入 store，
//! 精确复刻「实例装载之后、turn 末持久化之前」的并发压缩窗口；turn 走
//! 完整 process_inbound_message 链（gate → run_agent_loop_internal →
//! 持久化块）。

use super::*;

// ---------- 本地 mock（与 loop/tests.rs 同构，模块私有不可共享） ----------

/// turn 进行中并发压缩模拟器：chat() 内先把摘要写入 store（复刻压缩
/// merge 完成落盘的时点——晚于实例装载、早于 turn 末持久化），再返回
/// 固定回复。摘要文本/covers 可参数化（第二个用例需要 covers 推进到 4
/// 才能证明「不回退」）。
struct ConcurrentCompactProvider {
    store: std::sync::Arc<crate::session::SessionStore>,
    key: String,
    summary: &'static str,
    covers: usize,
}

#[async_trait]
impl LlmProvider for ConcurrentCompactProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.store.get_or_create(&self.key);
        self.store.set_summary(&self.key, self.summary);
        self.store
            .set_summary_covers_up_to(&self.key, Some(self.covers));
        let _ = self.store.save(&self.key);
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "turn reply".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        system_prompt_split: None,
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 进程内唯一 key（时间戳纳秒 + 标签），chat_log 落盘测试互不干扰。
fn unique_key(tag: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("agent:main:session:wipe_{}_{}", tag, nanos)
}

// ---------- 核心回归锁 ----------

/// turn 末持久化（cache=None 臂）不得抹掉并发压缩写入 store 的摘要。
/// 旧形态本测试红（store 摘要被清回空）；修复后绿（摘要 + covers 存活）。
#[tokio::test]
async fn turn_end_persist_preserves_concurrent_compact_summary() {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let key = unique_key("concurrent");
    // 预置历史（无摘要）——turn 装载实例的起点（cache 必为 None）。
    store.get_or_create(&key);
    for i in 0..4 {
        store.add_message(&key, "user", &format!("old msg {i}"));
    }
    store.add_message(&key, "assistant", "old reply");
    store.save(&key).expect("save preloaded history");

    let (_bus_tx, _bus_rx) = tokio::sync::mpsc::channel(16);
    let mut al = AgentLoop::new_bus(
        Box::new(ConcurrentCompactProvider {
            store: store.clone(),
            key: key.clone(),
            summary: "CONCURRENT COMPACT SUMMARY",
            covers: 3,
        }),
        test_config(),
        _bus_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );
    al.set_session_store(store.clone());

    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "wipeuser".to_string(),
        chat_id: "wipechat".to_string(),
        content: "在么".to_string(),
        media: vec![],
        session_key: key.clone(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    let (_agent_id, response, err, _) = al.process_inbound_message(&msg).await;
    assert!(err.is_none(), "turn 必须正常完成: {err:?}");
    assert_eq!(response, "turn reply");

    // 核心断言：turn 末持久化后，并发压缩的摘要与 covers 必须存活。
    assert_eq!(
        store.get_summary(&key),
        "CONCURRENT COMPACT SUMMARY",
        "turn 末持久化把并发压缩写入的摘要抹掉了（stale-cache-wipe 回归）"
    );
    assert_eq!(
        store.get_summary_covers_up_to(&key),
        Some(3),
        "covers_up_to 被清回 None（stale-cache-wipe 回归）"
    );
}

/// 对照臂（Some 半边）：store 已有旧摘要（covers=3）→ turn 装载水合
/// cache=旧值 → turn 进行中并发压缩写入**更新**版（covers=4）→ turn 末
/// 持久化不得用装载时的 stale 旧值覆盖它（旧 Some 臂无条件写回 = covers
/// 回退、新摘要文本丢失）。cache 未变化 ⇒ 不写。
#[tokio::test]
async fn turn_end_persist_does_not_regress_concurrent_recompact() {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let key = unique_key("recompact");
    store.get_or_create(&key);
    for i in 0..5 {
        store.add_message(&key, "user", &format!("old msg {i}"));
    }
    store.add_message(&key, "assistant", "old reply");
    // 上一轮压缩成果已在 store：turn 装载时水合进实例 cache（covers=3）。
    store.set_summary(&key, "PRIOR COMPACT SUMMARY");
    store.set_summary_covers_up_to(&key, Some(3));
    store.save(&key).expect("save preloaded history");

    // provider 的 chat() 模拟并发压缩完成：写入更深覆盖的新摘要
    // （covers=4 > 装载时的 3）——晚于实例装载、早于 turn 末持久化。
    let (_bus_tx, _bus_rx) = tokio::sync::mpsc::channel(16);
    let mut al = AgentLoop::new_bus(
        Box::new(ConcurrentCompactProvider {
            store: store.clone(),
            key: key.clone(),
            summary: "RECOMPACTED DEEPER SUMMARY",
            covers: 4,
        }),
        test_config(),
        _bus_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );
    al.set_session_store(store.clone());

    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "wipeuser".to_string(),
        chat_id: "wipechat".to_string(),
        content: "hello".to_string(),
        media: vec![],
        session_key: key.clone(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    let (_agent_id, _response, err, _) = al.process_inbound_message(&msg).await;
    assert!(err.is_none(), "turn 必须正常完成: {err:?}");

    // 核心断言：并发压缩的更新版存活，未被装载时的 stale 旧值覆盖回退。
    assert_eq!(
        store.get_summary(&key),
        "RECOMPACTED DEEPER SUMMARY",
        "turn 末持久化用装载时 stale 摘要覆盖了并发压缩的新版（Some 臂回退回归）"
    );
    assert_eq!(
        store.get_summary_covers_up_to(&key),
        Some(4),
        "并发压缩推进的 covers 被回退（Some 臂回退回归）"
    );
}
