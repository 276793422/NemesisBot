//! （+）：rewind 侧谱系与分支摘要的 AgentLoop 编排测试。
//!
//! -`rewind_to_message` 截断后写 `last_rewind` 谱系进本会话 sidecar
//!   meta（no-op 回退 = 无谱系事实，不写）；
//! - 分支摘要撤除（2026-11-24 快照复活封堵批）：rewind 是删除语义——
//!   **不再生成** branch_summary（小模型零调用），且把已有摘要一并
//!   purge（Branch Context 注入点每轮重读 meta，摘要回注 = 已删内容
//!   以摘要形态回流模型上下文）；fork 保持生成（遗弃内容仍活在父
//!   会话，摘要是新分支的合法背景供给）；
//! - 注入：`branch_summary` 在 `build_messages` 里成为 `# Branch
//!   Context` 临时 system 节（仅显式传 session_key 的路径——主循环
//!   `build_round_messages` 的接线形态）。
//!
//! 形态同 `e3_tests.rs`：裸 AgentLoop（无 checkpoint store——只截对话不
//! 回滚文件的诚实路径）+ 唯一键 + 结尾清理。

use super::*;
use crate::chat_log::{append_chat_log, delete_chat_log, read_session_meta_full};
use crate::types::ConversationTurn;
use std::sync::Arc;
use std::time::Duration;

// —— 空 provider：主模型不参与（rewind 编排不走 LLM）——

struct Ws9NoopProvider;

#[async_trait]
impl LlmProvider for Ws9NoopProvider {
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

/// 假小模型：固定回复（TitleProvider 同款形态）+ 调用计数（撤除断言用）。
struct Ws9SmallProvider {
    response: String,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl LlmProvider for Ws9SmallProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: self.response.clone(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn ws9_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        // system 节在位时 build_messages 才渲染 sections（replay 测试同款
        // 前置条件）——Branch Context 注入断言依赖它。
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 唯一键（pid + nanos + 原子序号，防 Windows 时钟 tick 粗粒度并行撞键）。
fn ws9_uniq_key(tag: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(
        "test:ws9loop:{tag}:{}:{}:{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        seq
    )
}

/// 摆 n 个完整 (user, assistant) 轮。
fn ws9_seed_turns(key: &str, turns: usize) {
    for i in 1..=turns {
        append_chat_log(key, "user", &format!("turn {i} question"));
        append_chat_log(key, "assistant", &format!("turn {i} answer"));
    }
}

/// rewind 截断把 `last_rewind`（at_index/dropped_rows/dropped_turns）
/// 写进本会话 meta；no-op 回退（末尾消息）无谱系事实，不写。
#[tokio::test]
async fn ws9_rewind_writes_last_rewind_meta() {
    let al = AgentLoop::new(Box::new(Ws9NoopProvider), ws9_config());
    let key = ws9_uniq_key("rewmeta");
    delete_chat_log(&key);
    ws9_seed_turns(&key, 4);

    // 回退到第 0 行（turn 1 user 行）：保留 turn 1，遗弃 3 轮 / 6 行。
    let out = al.rewind_to_message(&key, 0, false, None).await.unwrap();
    assert_eq!(out["removed_count"], 6, "{out}");

    let meta = read_session_meta_full(&key).expect("谱系写入应产生 meta");
    let rw = meta.last_rewind.expect("last_rewind 应落盘");
    assert_eq!(rw.at_index, 0);
    assert_eq!(rw.dropped_rows, 6);
    assert_eq!(rw.dropped_turns, 3);
    assert!(!rw.ts.is_empty(), "时间戳在位");

    // no-op 回退（末尾 assistant 行）：removed 0 = 无谱系事实。
    ws9_seed_turns(&key, 1);
    let (rows, total, _, _) = crate::chat_log::read_chat_log(&key, 100, None);
    assert_eq!(total, 4);
    let _ = rows;
    let out = al
        .rewind_to_message(&key, total - 1, false, None)
        .await
        .unwrap();
    assert_eq!(out["removed_count"], 0, "{out}");
    let meta2 = read_session_meta_full(&key).expect("meta 应保留");
    assert_eq!(
        meta2.last_rewind.as_ref().map(|r| r.dropped_rows),
        Some(6),
        "no-op 不覆盖已有谱系（removed 空 = 不写）"
    );

    delete_chat_log(&key);
}

/// 分支摘要撤除（2026-11-24）：rewind 是删除语义——即便 small_model
/// 已配置也不生成摘要（零调用），且把已有摘要 purge（Branch Context
/// 注入点每轮重读 meta，摘要回注 = 已删内容以摘要形态回流模型上下文）。
#[tokio::test]
async fn ws9_rewind_purges_branch_summary_and_never_calls_small_model() {
    let al = AgentLoop::new(Box::new(Ws9NoopProvider), ws9_config());
    let provider = Arc::new(Ws9SmallProvider {
        response: "## Goal\n被遗弃部分的独有标记XYZ\n## Constraints\n（无）\n## Progress\n（无）\n## Decisions\n（无）\n## Files\n（无）\n## Next Steps\n（无）".to_string(),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    al.set_small_model(Some((
        provider.clone() as Arc<dyn LlmProvider>,
        "test-small".to_string(),
    )));

    let key = ws9_uniq_key("rewsum");
    delete_chat_log(&key);
    ws9_seed_turns(&key, 4);
    // 预置已有摘要（模拟此前 fork 生成）——rewind 的 purge 必须清掉它。
    crate::chat_log::write_session_branch_summary(&key, "旧摘要标记OLD");

    let out = al.rewind_to_message(&key, 0, false, None).await.unwrap();
    assert_eq!(out["removed_count"], 6, "{out}");

    // 分支摘要撤除（2026-11-24）：rewind 是删除语义——小模型零调用
    // （不生成），已有摘要被 purge（Branch Context 注入点每轮重读
    // meta，摘要回注 = 已删内容回流模型上下文）。
    assert_eq!(
        provider.calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "rewind 不得调用小模型生成摘要"
    );
    // 给假想的后台 spawn 留窗口，确认无迟到写入。
    tokio::time::sleep(Duration::from_millis(300)).await;
    let meta = read_session_meta_full(&key).expect("谱系照写");
    assert!(meta.last_rewind.is_some(), "谱系照写");
    assert!(
        meta.branch_summary.is_none(),
        "已有摘要必须被 purge 且无新摘要落盘: {:?}",
        meta.branch_summary
    );

    delete_chat_log(&key);
}

/// 降级：small_model 未配置 = 诚实跳过——回退本体成功、谱系照写、
/// branch_summary 缺席、绝不回退主模型（Noop 主 provider 若被调用会返回
/// "ok"——meta 里不该出现）。
#[tokio::test]
async fn ws9_rewind_without_small_model_honestly_skips() {
    let al = AgentLoop::new(Box::new(Ws9NoopProvider), ws9_config());
    let key = ws9_uniq_key("nosmall");
    delete_chat_log(&key);
    ws9_seed_turns(&key, 4);

    let out = al.rewind_to_message(&key, 0, false, None).await.unwrap();
    assert_eq!(out["removed_count"], 6, "回退本体不受影响: {out}");

    let meta = read_session_meta_full(&key).expect("谱系照写");
    assert!(meta.last_rewind.is_some(), "last_rewind 落盘");
    assert!(meta.branch_summary.is_none(), "无小模型 = 无摘要");

    // 给后台路径留窗口，确认确实不会迟到写入。
    tokio::time::sleep(Duration::from_millis(300)).await;
    let meta2 = read_session_meta_full(&key).unwrap();
    assert!(meta2.branch_summary.is_none(), "不应迟到落盘");
    if let Some(s) = meta2.branch_summary {
        let _ = s; // keep the compiler honest if the assert above is edited
    }

    delete_chat_log(&key);
}

/// 注入：branch_summary 在 build_messages（显式传 session_key 的主循环
/// 形态）里成为 `# Branch Context` 临时节；不传 session_key（旧路径）不注入。
#[tokio::test]
async fn ws9_branch_summary_injected_into_build_messages() {
    let al = AgentLoop::new(Box::new(Ws9NoopProvider), ws9_config());
    let key = ws9_uniq_key("inject");
    delete_chat_log(&key);
    crate::chat_log::write_session_branch_summary(&key, "被遗弃部分结论标记ABC");

    let instance = AgentInstance::new(ws9_config());
    instance.set_history(vec![ConversationTurn {
        role: "user".to_string(),
        content: "当前轮问题".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: chrono::Local::now().to_rfc3339(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }]);

    // 显式 session_key（主循环 build_round_messages 的接线形态）：注入。
    let (msgs, _) = al.build_messages_with_memory_annotated_for(&instance, None, Some(&key), None);
    let joined = msgs.iter().map(|m| m.content.as_str()).collect::<String>();
    assert!(
        joined.contains("# Branch Context"),
        "应注入分支前情提要节: {joined}"
    );
    assert!(
        joined.contains("被遗弃部分结论标记ABC"),
        "摘要正文在位: {joined}"
    );
    assert!(joined.contains("当前轮问题"), "原对话不受影响: {joined}");

    // 旧路径（不传 session_key）：不注入（字节兼容）。
    let (msgs_old, _) = al.build_messages_with_memory_annotated(&instance, None, None);
    let joined_old = msgs_old
        .iter()
        .map(|m| m.content.as_str())
        .collect::<String>();
    assert!(
        !joined_old.contains("# Branch Context"),
        "旧路径不注入: {joined_old}"
    );

    // 无摘要的会话：不产生空节。
    let key2 = ws9_uniq_key("inject-none");
    delete_chat_log(&key2);
    let (msgs_none, _) =
        al.build_messages_with_memory_annotated_for(&instance, None, Some(&key2), None);
    let joined_none = msgs_none
        .iter()
        .map(|m| m.content.as_str())
        .collect::<String>();
    assert!(!joined_none.contains("# Branch Context"), "无摘要不注入");

    delete_chat_log(&key);
    delete_chat_log(&key2);
}
