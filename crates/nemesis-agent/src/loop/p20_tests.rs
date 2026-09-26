//! P20（2026-09-25 能力扩展 WS7）：rewind 冲突预检编排测试（git 影子形态）。
//!
//! 验证 `rewind_to_message` 的 force 语义全链路：
//! - turn 收尾封印（seal_turn）后文件被外部修改 → rewind 默认**零副作用**
//!   拒绝（`blocked:true` + 结构化冲突清单 path/期望指纹/当前指纹；jsonl
//!   未截断、文件未恢复、undo 栈未入）；
//! - `force=true` 强过（`forced:true` + 文件恢复 + 截断生效 + 审计日志），
//!   且 redo 往返仍工作（行 VERBATIM 回填 + forward_tree 前向恢复）。

use super::*;
use crate::chat_log::{delete_chat_log, read_chat_log};

// —— 空 provider：编排测试不走 dispatch/LLM ——

struct P20NoopProvider;

#[async_trait]
impl LlmProvider for P20NoopProvider {
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

fn p20_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

fn p20_uniq_key(tag: &str) -> String {
    format!(
        "test:p20rewind:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn p20_modify(rel: &str) -> FileChange {
    FileChange {
        path: rel.to_string(),
        kind: FileChangeKind::Modify,
    }
}

/// 按 E3 真实写入形态摆行：user 行带 checkpoint_turn 标记。
fn p20_append(key: &str, role: &str, content: &str, turn: Option<usize>) {
    crate::chat_log::append_chat_log_meta(
        key,
        role,
        content,
        &crate::chat_log::ChatLogMeta {
            checkpoint_turn: turn,
            ..Default::default()
        },
    );
}

/// sha256 hex（与 checkpoint::hash_on_disk 同口径，断言期望指纹用）。
fn p20_hash(content: &str) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(content.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// git 影子形态两轮对话 staging：每 turn begin → 工具写盘 → snapshot 声明
/// → seal_turn 收尾封印（模拟 process_admitted 尾的封印点）→ 行落盘。
/// turn1 完成态 code.txt=v2、turn2 完成态 v3（均已封印）。
async fn stage_sealed_two_turns(tag: &str) -> (AgentLoop, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let al = AgentLoop::new(Box::new(P20NoopProvider), p20_config());
    let store = Arc::new(crate::checkpoint::CheckpointStore::new(None, root.clone()));
    assert_eq!(
        store.backend(),
        crate::checkpoint::CheckpointBackend::Git,
        "root/.git 目录 → git 影子 backend"
    );
    al.set_checkpoint_store(store);
    let key = p20_uniq_key(tag);
    delete_chat_log(&key);

    let cp = al.attached_checkpoint().unwrap();

    // turn 1：begin → 写 v2 → 声明 → 收尾封印 → 行落盘。
    cp.begin(1, "turn 1");
    std::fs::write(root.join("code.txt"), "v2").unwrap();
    cp.snapshot(&p20_modify("code.txt")).await;
    cp.seal_turn(1);
    p20_append(&key, "user", "turn 1 q", Some(1));
    p20_append(&key, "assistant", "turn 1 a", None);

    // turn 2：begin → 写 v3 → 声明 → 收尾封印 → 行落盘。
    cp.begin(2, "turn 2");
    std::fs::write(root.join("code.txt"), "v3").unwrap();
    cp.snapshot(&p20_modify("code.txt")).await;
    cp.seal_turn(2);
    p20_append(&key, "user", "turn 2 q", Some(2));
    p20_append(&key, "assistant", "turn 2 a", None);

    (al, key, dir)
}

/// 外部修改后 rewind 默认拒绝：结构化冲突清单 + 零副作用（jsonl/文件/undo
/// 栈三不动）。
#[tokio::test]
async fn p20_rewind_blocks_on_external_modification_without_force() {
    let (al, key, dir) = stage_sealed_two_turns("block").await;
    let file = dir.path().join("ws").join("code.txt");

    // 外部修改（绕过 agent 工具——rewind 要防的正是这个）。
    std::fs::write(&file, "v4-external").unwrap();

    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
    assert_eq!(out["blocked"], true, "{out}");
    assert_eq!(out["reason"], "conflict");
    assert_eq!(out["restore_turn"], 2);
    let conflicts = out["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "{out}");
    assert_eq!(conflicts[0]["path"], "code.txt");
    assert_eq!(conflicts[0]["expected_hash"], p20_hash("v3"));
    assert_eq!(conflicts[0]["current_hash"], p20_hash("v4-external"));

    // 零副作用三连：jsonl 未截断、文件未恢复、undo 栈未入。
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4, "被拒回退不得截断会话");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v4-external");
    let err = al.redo_rewind(&key).await.unwrap_err();
    assert!(err.contains("没有可重做"), "{err}");

    delete_chat_log(&key);
}

/// force=true 强过：文件恢复 + 截断生效 + 回执留痕（forced:true）；redo
/// 往返仍工作（行回填 + forward_tree 前向恢复到 rewind 时刻实况）。
/// 用全新 staging——rewind 的 truncate_from 会把 turn 2 清出索引，与
/// 「无冲突 force」场景不能共用同一 store 实例。
#[tokio::test]
async fn p20_rewind_force_overrides_and_redo_roundtrip_works() {
    let (al, key, dir) = stage_sealed_two_turns("force").await;
    let file = dir.path().join("ws").join("code.txt");

    // 冲突场景 + force：强过。
    std::fs::write(&file, "v4-external").unwrap();
    let out = al.rewind_to_message(&key, 0, true).await.unwrap();
    assert_eq!(
        out["blocked"],
        serde_json::Value::Null,
        "force 强过不拒绝: {out}"
    );
    assert_eq!(out["forced"], true, "冲突 + force = 审计留痕: {out}");
    assert_eq!(out["kept_count"], 2);
    assert_eq!(out["file_restore"], "applied");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "v2",
        "文件恢复到 turn 2 begin 树"
    );

    // redo 往返：行回填 + 前向恢复到 rewind 时刻实时树（含外部修改 v4——
    // 诚实语义：redo 恢复的是 undo 时刻实况，不是理想态）。
    let out = al.redo_rewind(&key).await.unwrap();
    assert_eq!(out["restored_count"], 2, "{out}");
    assert_eq!(read_chat_log(&key, 100, None).1, 4, "行回填");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "v4-external",
        "前向恢复到 undo 时刻实时树"
    );

    delete_chat_log(&key);
}

/// 无外部修改时 force 也不留痕（forced=false）且直通——冲突预检不误伤
/// 正常回退（git 形态 + 封印链下的干净路径）。
#[tokio::test]
async fn p20_rewind_passes_through_when_seals_match_disk() {
    let (al, key, dir) = stage_sealed_two_turns("clean").await;
    let file = dir.path().join("ws").join("code.txt");

    // 盘面停在 v3（turn 2 收尾态）= 最后封印指纹一致 → 无冲突直通。
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v3");
    let out = al.rewind_to_message(&key, 0, true).await.unwrap();
    assert_eq!(out["blocked"], serde_json::Value::Null, "{out}");
    assert_eq!(out["forced"], false, "无冲突时 force 不留痕: {out}");
    assert_eq!(out["file_restore"], "applied");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");

    delete_chat_log(&key);
}
