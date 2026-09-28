//! WS9（能力扩展 P17+P18）：rewind 侧谱系与分支摘要的 AgentLoop 编排测试。
//!
//! - P17：`rewind_to_message` 截断后写 `last_rewind` 谱系进本会话 sidecar
//!   meta（no-op 回退 = 无谱系事实，不写）；
//! - P18：遗弃后缀 ≥3 个完整 user 轮且 `agents.small_model` 已配置 →
//!   mock 小模型生成结构化摘要落盘（`branch_summary`）；
//! - P18 降级：小模型未配置 = 诚实跳过（谱系照写、摘要缺席、不阻塞回退）；
//! - P18 注入：`branch_summary` 在 `build_messages` 里成为 `# Branch
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

/// 假小模型：固定回复（E7 TitleProvider 同款形态）。
struct Ws9SmallProvider {
    response: String,
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
        Ok(LlmResponse {
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
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
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

/// 轮询谓词成立（100ms 步进——等 spawn_write 后台落盘）。
async fn ws9_wait_for(mut pred: impl FnMut() -> bool, max: Duration) -> bool {
    let deadline = std::time::Instant::now() + max;
    while std::time::Instant::now() < deadline {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    pred()
}

/// P17：rewind 截断把 `last_rewind`（at_index/dropped_rows/dropped_turns）
/// 写进本会话 meta；no-op 回退（末尾消息）无谱系事实，不写。
#[tokio::test]
async fn ws9_rewind_writes_last_rewind_meta() {
    let al = AgentLoop::new(Box::new(Ws9NoopProvider), ws9_config());
    let key = ws9_uniq_key("rewmeta");
    delete_chat_log(&key);
    ws9_seed_turns(&key, 4);

    // 回退到第 0 行（turn 1 user 行）：保留 turn 1，遗弃 3 轮 / 6 行。
    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
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
    let out = al.rewind_to_message(&key, total - 1, false).await.unwrap();
    assert_eq!(out["removed_count"], 0, "{out}");
    let meta2 = read_session_meta_full(&key).expect("meta 应保留");
    assert_eq!(
        meta2.last_rewind.as_ref().map(|r| r.dropped_rows),
        Some(6),
        "no-op 不覆盖已有谱系（removed 空 = 不写）"
    );

    delete_chat_log(&key);
}

/// P18：rewind 遗弃 ≥3 轮 + small_model 已配置 → mock 小模型生成结构化
/// 摘要，后台落盘到本会话 meta 的 branch_summary。
#[tokio::test]
async fn ws9_rewind_generates_branch_summary_via_small_model() {
    let al = AgentLoop::new(Box::new(Ws9NoopProvider), ws9_config());
    let provider = Arc::new(Ws9SmallProvider {
        response: "## Goal\n被遗弃部分的独有标记XYZ\n## Constraints\n（无）\n## Progress\n（无）\n## Decisions\n（无）\n## Files\n（无）\n## Next Steps\n（无）".to_string(),
    });
    al.set_small_model(Some((
        provider.clone() as Arc<dyn LlmProvider>,
        "test-small".to_string(),
    )));

    let key = ws9_uniq_key("rewsum");
    delete_chat_log(&key);
    ws9_seed_turns(&key, 4);

    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
    assert_eq!(out["removed_count"], 6, "{out}");

    // spawn_write 后台落盘——轮询等 meta 出现。
    let landed = ws9_wait_for(
        || {
            read_session_meta_full(&key)
                .and_then(|m| m.branch_summary)
                .is_some()
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(landed, "分支摘要应在超时前落盘");

    let meta = read_session_meta_full(&key).unwrap();
    let summary = meta.branch_summary.unwrap();
    assert!(
        summary.contains("独有标记XYZ"),
        "六节解析（或原样回退）后摘要应含 mock 内容: {summary}"
    );
    // 谱系与摘要共存（同一次 rewind 的两份产物）。
    assert!(meta.last_rewind.is_some(), "谱系照写");

    delete_chat_log(&key);
}

/// P18 降级：small_model 未配置 = 诚实跳过——回退本体成功、谱系照写、
/// branch_summary 缺席、绝不回退主模型（Noop 主 provider 若被调用会返回
/// "ok"——meta 里不该出现）。
#[tokio::test]
async fn ws9_rewind_without_small_model_honestly_skips() {
    let al = AgentLoop::new(Box::new(Ws9NoopProvider), ws9_config());
    let key = ws9_uniq_key("nosmall");
    delete_chat_log(&key);
    ws9_seed_turns(&key, 4);

    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
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

/// P18 注入：branch_summary 在 build_messages（显式传 session_key 的主循环
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
    let (msgs, _) =
        al.build_messages_with_memory_annotated_for(&instance, None, Some(&key), None);
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
