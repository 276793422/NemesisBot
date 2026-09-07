//! E3（devtool-upgrade 阶段 5）：消息级回退/重做的 AgentLoop 编排测试。
//!
//! JSON 形态 checkpoint store + 手工摆两轮对话（行带 `checkpoint_turn`
//! 标记 + store begin/snapshot 同步推进），验证：
//! - `rewind_to_message`：turn 对齐截断（index 所在 turn 整 turn 保留）+
//!   文件恢复到下一 turn 开始态 + undo 栈压入；
//! - `redo_rewind`：行 VERBATIM 回填 + JSON 形态文件步诚实跳过；栈空诚实
//!   报错；
//! - 陈旧性守卫：undo 后新消息 → redo 诚实拒绝；
//! - 边界：空会话 / 越界 index / 末尾消息（no-op 回执，redoable=false）。

use super::*;
use crate::chat_log::{append_chat_log, delete_chat_log, read_chat_log};

// —— 空 provider：编排测试不走 dispatch/LLM ——

struct E3NoopProvider;

#[async_trait]
impl LlmProvider for E3NoopProvider {
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

fn e3_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

fn e3_uniq_key(tag: &str) -> String {
    format!(
        "test:e3loop:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn e3_modify(rel: &str) -> FileChange {
    FileChange {
        path: rel.to_string(),
        kind: FileChangeKind::Modify,
    }
}

/// 摆两轮对话（jsonl 行 + checkpoint store 推进同真实施时序：begin →
/// snapshot（捕 pre-edit 内容）→ 落盘行）。返回 (loop, key, workspace)。
async fn stage_two_turns(tag: &str) -> (AgentLoop, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let al = AgentLoop::new(Box::new(E3NoopProvider), e3_config());
    al.set_checkpoint_store(Arc::new(crate::checkpoint::CheckpointStore::new(
        None, root,
    )));
    let key = e3_uniq_key(tag);
    delete_chat_log(&key);

    let cp = al.attached_checkpoint().unwrap();

    // turn 1：begin → snapshot 捕 v1 → 工具改成 v2 → 行落盘。
    cp.begin(1, "turn 1");
    cp.snapshot(&e3_modify("code.txt")).await;
    std::fs::write(dir.path().join("ws").join("code.txt"), "v2").unwrap();
    e3_append(&key, "user", "turn 1 q", Some(1));
    e3_append(&key, "assistant", "turn 1 a", None);

    // turn 2：begin → snapshot 捕 v2 → 工具改成 v3 → 行落盘。
    cp.begin(2, "turn 2");
    cp.snapshot(&e3_modify("code.txt")).await;
    std::fs::write(dir.path().join("ws").join("code.txt"), "v3").unwrap();
    e3_append(&key, "user", "turn 2 q", Some(2));
    e3_append(&key, "assistant", "turn 2 a", None);

    (al, key, dir)
}

/// 按 E3 真实写入形态摆行：user 行带 checkpoint_turn 标记（run_agent_loop_
/// internal 的 append_chat_log_meta 同构）。
fn e3_append(key: &str, role: &str, content: &str, turn: Option<usize>) {
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

/// rewind 到第 0 行（turn 1 的 user 行）：turn 1 整 turn 保留，turn 2 截
/// 掉，文件恢复到 turn 2 begin 前（v2）；undo 入栈。
#[tokio::test]
async fn e3_rewind_truncates_turn_aligned_and_restores_files() {
    let (al, key, _dir) = stage_two_turns("rew").await;

    let out = al.rewind_to_message(&key, 0).await.unwrap();
    assert_eq!(out["kept_count"], 2, "turn 1 整 turn 保留: {out}");
    assert_eq!(out["removed_count"], 2);
    assert_eq!(out["restore_turn"], 2);
    assert_eq!(out["file_restore"], "applied");
    assert_eq!(out["redoable"], true);
    assert_eq!(
        out["restored_files"]["written"][0], "code.txt",
        "恢复清单含 code.txt: {out}"
    );

    let (rows, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 2);
    assert_eq!(rows[0]["content"], "turn 1 q");
    assert_eq!(rows[1]["content"], "turn 1 a");

    // 文件回 v2（restore_turn=2 的 pre-edit 态），不是 v1。
    assert_eq!(
        std::fs::read_to_string(_dir.path().join("ws").join("code.txt")).unwrap(),
        "v2"
    );

    delete_chat_log(&key);
}

/// redo：行 VERBATIM 回填（时间戳/标记原样）；JSON 形态 forward_tree=None
/// → 文件步诚实 skipped 不装成功；栈空后诚实报错。
#[tokio::test]
async fn e3_redo_restores_rows_and_honestly_skips_files() {
    let (al, key, _dir) = stage_two_turns("redo").await;
    let (before, _, _, _) = read_chat_log(&key, 100, None);

    al.rewind_to_message(&key, 0).await.unwrap();
    let out = al.redo_rewind(&key).await.unwrap();
    assert_eq!(out["restored_count"], 2, "{out}");
    assert_eq!(out["file_restore"], "skipped", "JSON 形态无影子 tree");
    assert!(out["file_restore_note"].is_string());

    let (rows, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4, "行回填");
    // VERBATIM：时间戳与 checkpoint_turn 标记原样回来。
    assert_eq!(rows[0]["timestamp"], before[0]["timestamp"]);
    assert_eq!(rows[2]["checkpoint_turn"], serde_json::Value::from(2));
    assert_eq!(rows[3]["content"], "turn 2 a");

    // 栈空：再 redo 诚实报错。
    let err = al.redo_rewind(&key).await.unwrap_err();
    assert!(err.contains("没有可重做"), "{err}");

    delete_chat_log(&key);
}

/// 陈旧性守卫：undo 之后会话来了新行 → redo 诚实拒绝（栈条目作废）。
#[tokio::test]
async fn e3_redo_rejects_after_new_activity() {
    let (al, key, _dir) = stage_two_turns("guard").await;

    al.rewind_to_message(&key, 0).await.unwrap();
    append_chat_log(&key, "user", "new message after rewind");

    let err = al.redo_rewind(&key).await.unwrap_err();
    assert!(err.contains("失效"), "{err}");
    // 行数没被 redo 动过（截断态保持）。
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 3);

    delete_chat_log(&key);
}

/// 边界：空会话 Err；越界 index Err；末尾消息 no-op（removed 0，
/// redoable=false，文件/日志都不动）。
#[tokio::test]
async fn e3_edge_cases_empty_out_of_range_and_noop() {
    let (al, key, dir) = stage_two_turns("edge").await;
    let file = dir.path().join("ws").join("code.txt");

    // 空会话。
    let empty_key = e3_uniq_key("edge-empty");
    delete_chat_log(&empty_key);
    let err = al.rewind_to_message(&empty_key, 0).await.unwrap_err();
    assert!(err.contains("没有可回退"), "{err}");

    // 越界。
    let err = al.rewind_to_message(&key, 99).await.unwrap_err();
    assert!(err.contains("超出范围"), "{err}");

    // 末尾消息（最后一 turn 的 assistant 行）：nothing after → no-op。
    let out = al.rewind_to_message(&key, 3).await.unwrap();
    assert_eq!(out["removed_count"], 0, "{out}");
    assert_eq!(out["redoable"], false);
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4, "日志不动");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v3", "文件不动");
    // no-op 不压栈 → redo 报错。
    assert!(al.redo_rewind(&key).await.is_err());

    delete_chat_log(&key);
    delete_chat_log(&empty_key);
}

// —— git 影子形态：redo 前向恢复目标 = rewind 时刻实时树 ——

/// git 形态的两轮摆法（root 带 .git 目录 → store 构造期选 git backend）。
/// turn 2 里写出的 v3 不在任何 begin 树里（begin 树 = 各 turn 开始态）。
async fn stage_two_turns_git(tag: &str) -> (AgentLoop, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let al = AgentLoop::new(Box::new(E3NoopProvider), e3_config());
    let store = Arc::new(crate::checkpoint::CheckpointStore::new(None, root.clone()));
    assert_eq!(
        store.backend(),
        crate::checkpoint::CheckpointBackend::Git,
        "root/.git 目录 → git 影子 backend"
    );
    al.set_checkpoint_store(store);
    let key = e3_uniq_key(tag);
    delete_chat_log(&key);

    let cp = al.attached_checkpoint().unwrap();

    // turn 1：begin（树={v1}）→ 工具改成 v2 → 行落盘。
    cp.begin(1, "turn 1");
    std::fs::write(root.join("code.txt"), "v2").unwrap();
    e3_append(&key, "user", "turn 1 q", Some(1));
    e3_append(&key, "assistant", "turn 1 a", None);

    // turn 2：begin（树={v2}）→ 工具改成 v3 → 行落盘。v3 不在任何树里。
    cp.begin(2, "turn 2");
    std::fs::write(root.join("code.txt"), "v3").unwrap();
    e3_append(&key, "user", "turn 2 q", Some(2));
    e3_append(&key, "assistant", "turn 2 a", None);

    (al, key, dir)
}

/// 回归（2026-09-06 门 5 实测发现）：redo 前向恢复目标必须是 rewind 时刻
/// 的**实时**树（含最后一个 turn 的变更），不是最后一个 begin 树（只到上
/// 一 turn 开始态）。修复前 redo 后文件停 v2，与已回填的「turn 2 改成 v3」
/// 对话行不一致。
#[tokio::test]
async fn e3_git_redo_restores_to_live_tree_including_last_turn() {
    let (al, key, dir) = stage_two_turns_git("gredo").await;
    let file = dir.path().join("ws").join("code.txt");

    // rewind 到 turn 1：文件回 v2（turn 2 begin 树 = turn 1 完成态）。
    let out = al.rewind_to_message(&key, 0).await.unwrap();
    assert_eq!(out["file_restore"], "applied", "{out}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");

    // redo：行回填 + 文件恢复到 rewind 时刻实时树 = v3。
    let out = al.redo_rewind(&key).await.unwrap();
    assert_eq!(out["file_restore"], "applied", "{out}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "v3",
        "redo 恢复到实时树（含最后 turn 变更）， redo 后文件与对话一致: {out}"
    );
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4, "行回填");

    delete_chat_log(&key);
}

/// git 形态：rewind 恢复覆盖**未声明**的 shell 副作用文件（tree diff 语
/// 义，对照 JSON 形态的声明路径盲区）；redo 一并恢复。
#[tokio::test]
async fn e3_git_rewind_redo_covers_undeclared_shell_side_effects() {
    let (al, key, dir) = stage_two_turns_git("gshell").await;
    let ws = dir.path().join("ws");

    // turn 2 期间 shell 副作用产物（无 preview 声明）。
    std::fs::write(ws.join("side_effect.txt"), "boom").unwrap();

    let out = al.rewind_to_message(&key, 0).await.unwrap();
    assert_eq!(out["file_restore"], "applied", "{out}");
    assert!(
        !ws.join("side_effect.txt").exists(),
        "未声明副作用文件随 tree diff 一并回滚: {out}"
    );
    assert_eq!(std::fs::read_to_string(ws.join("code.txt")).unwrap(), "v2");

    let out = al.redo_rewind(&key).await.unwrap();
    assert_eq!(out["file_restore"], "applied", "{out}");
    assert_eq!(
        std::fs::read_to_string(ws.join("side_effect.txt")).unwrap(),
        "boom",
        "redo 从实时树恢复副作用文件"
    );

    delete_chat_log(&key);
}
