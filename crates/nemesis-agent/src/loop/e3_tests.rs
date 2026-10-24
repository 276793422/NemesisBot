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

fn e3_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
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

    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
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

/// R3 回归锁（2026-10-24 外评探针反转）：文件恢复失败时 rewind **零推进**
/// ——jsonl 不截断、checkpoint 保留、返回 blocked 结构化失败清单；解锁后
/// 重试恢复正常推进。旧形态：恢复失败被吞、照常盖「applied」戳 + 无条件
/// 截断对话 + 摧毁 checkpoint（文件没恢复、对话被截、重试不可能，三输）。
/// share_mode(1)（仅 FILE_SHARE_READ）锁目标文件逼 hybrid_restore 写失败。
#[cfg(windows)]
#[tokio::test]
async fn rewind_restore_failure_blocks_without_discarding_history() {
    use std::os::windows::fs::OpenOptionsExt;
    let (al, key, dir) = stage_two_turns("r3-locked").await;
    let path = dir.path().join("ws/code.txt");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .expect("打开锁句柄");

    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
    assert_eq!(out["blocked"], true, "恢复失败必须 blocked: {out}");
    assert_eq!(out["reason"], "restore_failed");
    assert_eq!(
        out["file_restore"], "failed",
        "不完整恢复不得盖 applied 戳: {out}"
    );
    assert!(
        out["restore_failures"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "失败清单非空: {out}"
    );

    // 零推进三断言：文件未动 + 会话未截 + checkpoint 保留（重试依据）。
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "v3",
        "恢复失败文件不得半途而废地保持错误内容"
    );
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4, "恢复失败 = 会话不截断");
    assert!(
        al.checkpoint_list().iter().any(|c| c.turn == 2),
        "checkpoint turn 2 保留（重试依据不被摧毁）"
    );

    // 解锁重试：恢复正常推进（恢复幂等——重写同样内容）。
    drop(lock);
    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
    assert_eq!(out["file_restore"], "applied", "{out}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 2, "重试后截断生效");

    delete_chat_log(&key);
}

/// redo：行 VERBATIM 回填（时间戳/标记原样）；JSON 形态 forward_tree=None
/// → 文件步诚实 skipped 不装成功；栈空后诚实报错。
#[tokio::test]
async fn e3_redo_restores_rows_and_honestly_skips_files() {
    let (al, key, _dir) = stage_two_turns("redo").await;
    let (before, _, _, _) = read_chat_log(&key, 100, None);

    al.rewind_to_message(&key, 0, false).await.unwrap();
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

    al.rewind_to_message(&key, 0, false).await.unwrap();
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
    let err = al
        .rewind_to_message(&empty_key, 0, false)
        .await
        .unwrap_err();
    assert!(err.contains("没有可回退"), "{err}");

    // 越界。
    let err = al.rewind_to_message(&key, 99, false).await.unwrap_err();
    assert!(err.contains("超出范围"), "{err}");

    // 末尾消息（最后一 turn 的 assistant 行）：nothing after → no-op。
    let out = al.rewind_to_message(&key, 3, false).await.unwrap();
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

// —— 行动代理 P0（2026-10-14）撤销一等动词：rewind_last_turn ——

/// 撤销最后一轮：两轮会话 undo → turn 2 整轮移除（user 行一并，与
/// `rewind_to_message` 的「保留 index 所在轮」语义不同）、文件回 turn 2
/// begin 前的 v2、redoable=true。
#[tokio::test]
async fn e3_undo_last_turn_removes_last_turn_and_restores_files() {
    let (al, key, dir) = stage_two_turns("undo").await;
    let file = dir.path().join("ws").join("code.txt");

    let out = al.rewind_last_turn(&key).await.unwrap();
    assert_eq!(out["kept_count"], 2, "turn 1 保留: {out}");
    assert_eq!(out["removed_count"], 2, "turn 2 user 行一并移除: {out}");
    assert_eq!(out["restore_turn"], 2);
    assert_eq!(out["file_restore"], "applied");
    assert_eq!(out["redoable"], true);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");

    let (rows, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 2);
    assert_eq!(rows[0]["content"], "turn 1 q");
    assert_eq!(rows[1]["content"], "turn 1 a");

    delete_chat_log(&key);
}

/// 单轮会话撤销：cut=0 合法（截到空）——「下单→撤销」剧本的主路径，也是
/// `rewind_to_message` 契约表达不了的形态。连续 undo 两轮 → 空会话；redo
/// 按 LIFO 弹最后一笔（turn 1）回填。无 user 行的会话诚实拒绝。
#[tokio::test]
async fn e3_undo_last_turn_single_turn_cut_to_zero_and_redo() {
    let (al, key, _dir) = stage_two_turns("undo0").await;

    // 第一笔：撤 turn 2。
    let out = al.rewind_last_turn(&key).await.unwrap();
    assert_eq!(out["removed_count"], 2, "{out}");

    // 第二笔：剩 turn 1 → cut=0 截到空，redoable 恒 true（removed 永不空）。
    let out = al.rewind_last_turn(&key).await.unwrap();
    assert_eq!(out["kept_count"], 0, "{out}");
    assert_eq!(out["removed_count"], 2);
    assert_eq!(out["redoable"], true);
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 0, "会话截到空");

    // 空/无 user 行会话：诚实拒绝。
    let err = al.rewind_last_turn(&key).await.unwrap_err();
    assert!(err.contains("没有可撤销"), "{err}");

    // redo LIFO：先回 turn 1（最后一笔 undo 的行）。
    let out = al.redo_rewind(&key).await.unwrap();
    assert_eq!(out["restored_count"], 2, "{out}");
    let (rows, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 2);
    assert_eq!(rows[0]["content"], "turn 1 q");

    delete_chat_log(&key);
}

// —— E3（收官批 2026-10-15）：WSAPI 撤销入口 undo_last_turn 的 busy 闸 ——

/// 会话处理中（busy）诚实拒绝且零副作用（对话行数/文件都不动）；释放后
/// 放行直达 rewind_last_turn 语义。
#[tokio::test]
async fn e3_undo_public_entry_rejects_busy_session() {
    let (al, key, dir) = stage_two_turns("undo-busy").await;
    let file = dir.path().join("ws").join("code.txt");

    // 占住会话（模拟 turn 处理中）。
    assert!(al.try_acquire_session(&key));
    let err = al.undo_last_turn(&key).await.unwrap_err();
    assert!(err.contains("正在处理"), "{err}");
    // 零副作用：日志与文件都不动。
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v3");

    // 释放后放行（维护尾巴同款 gate 语义下，撤销动词正常出效果）。
    al.release_session(&key);
    let out = al.undo_last_turn(&key).await.unwrap();
    assert_eq!(out["removed_count"], 2, "{out}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");

    delete_chat_log(&key);
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
    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
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

    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
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

// —— BUG-HUNT R71（BUG-20261024-02）：R31 scoped 截断 × redo 守卫口径 ——

/// R71 git 多会话摆法：A 两 turn（v1→v2→v3，turn 边界 begin 捕树）+ B 一
/// turn（无文件变更，begin 树={v3}）。B 的条目经 `begin_for_session` 穿针
/// ——R31 起 scoped 截断按 session 字段放行其在 A 的回退中存活。
#[allow(clippy::type_complexity)]
async fn stage_git_sessions_ab(
    tag: &str,
) -> (
    AgentLoop,
    String,
    String,
    Arc<crate::checkpoint::CheckpointStore>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let al = AgentLoop::new(Box::new(E3NoopProvider), e3_config());
    let store = Arc::new(crate::checkpoint::CheckpointStore::new(None, root.clone()));
    assert_eq!(store.backend(), crate::checkpoint::CheckpointBackend::Git);
    al.set_checkpoint_store(store);
    let key_a = e3_uniq_key(&format!("{tag}-a"));
    let key_b = e3_uniq_key(&format!("{tag}-b"));
    delete_chat_log(&key_a);
    delete_chat_log(&key_b);

    let cp = al.attached_checkpoint().unwrap();

    // A turn 1：begin（树={v1}）→ v2 → 行落盘。
    cp.begin_for_session(1, "a1", Some(&key_a));
    std::fs::write(root.join("code.txt"), "v2").unwrap();
    e3_append(&key_a, "user", "a turn 1 q", Some(1));
    e3_append(&key_a, "assistant", "a turn 1 a", None);

    // A turn 2：begin（树={v2}）→ v3 → 行落盘（v3 只在实况）。
    cp.begin_for_session(2, "a2", Some(&key_a));
    std::fs::write(root.join("code.txt"), "v3").unwrap();
    e3_append(&key_a, "user", "a turn 2 q", Some(2));
    e3_append(&key_a, "assistant", "a turn 2 a", None);

    // B turn 3：begin（树={v3}）——A 回退时 scoped 截断保留这条带树条目。
    cp.begin_for_session(3, "b1", Some(&key_b));
    e3_append(&key_b, "user", "b turn 3 q", Some(3));
    e3_append(&key_b, "assistant", "b turn 3 a", None);

    (al, key_a, key_b, cp, dir)
}

/// 核心回归臂：B 会话在 A 锚点之后 begin 过 turn（带树），A 回退时 scoped
/// 截断保留 B 的条目。修复前 post 基线在截断前按 tree_hex_before(t) 采集
/// （={v1}），redo 守卫 2 拿截断后 latest_tree_hex()（=B turn-3 的 {v3}）
/// 对照，把 B 的存量条目误判成「undo 后工作区变了」——零变化也恒拒，且
/// 弹出条目作废、redo 永久死亡。修复后两侧同读截断后索引，redo 通行且回
/// 填/前向恢复正确。
#[tokio::test]
async fn r71_git_redo_not_blocked_by_other_session_turns() {
    let (al, key_a, key_b, _cp, dir) = stage_git_sessions_ab("r71redo").await;
    let root = dir.path().join("ws");

    // A 回退到 turn 1 的 user 行：A 的 turn 2 截掉（文件回 {v2}），B 存活。
    let out = al.rewind_to_message(&key_a, 0, false).await.unwrap();
    assert_eq!(out["removed_count"], 2, "{out}");
    assert_eq!(out["redoable"], true);
    assert_eq!(
        std::fs::read_to_string(root.join("code.txt")).unwrap(),
        "v2",
        "文件回 turn 2 begin 树"
    );

    // 零变化直接 redo：修复前守卫 2 恒假拒。
    let out = al.redo_rewind(&key_a).await.unwrap();
    assert_eq!(out["restored_count"], 2, "{out}");
    assert_eq!(out["file_restore"], "applied", "{out}");
    let (_, total, _, _) = read_chat_log(&key_a, 100, None);
    assert_eq!(total, 4, "A 的行回填");
    assert_eq!(
        std::fs::read_to_string(root.join("code.txt")).unwrap(),
        "v3",
        "前向恢复到 undo 时刻实时树（含 A turn 2 的 v3）"
    );

    delete_chat_log(&key_a);
    delete_chat_log(&key_b);
}

/// 真阳性保持：undo 之后其他会话又 begin 新 turn（工作区真的变了）→
/// redo 仍诚实拒绝，行保持截断态（截断后索引基线的陈旧性语义不丢）。
#[tokio::test]
async fn r71_git_redo_still_rejects_after_genuinely_new_turn() {
    let (al, key_a, key_b, cp, dir) = stage_git_sessions_ab("r71stale").await;
    let root = dir.path().join("ws");

    let out = al.rewind_to_message(&key_a, 0, false).await.unwrap();
    assert_eq!(out["redoable"], true, "{out}");

    // undo 之后 B 再开新 turn：先改文件再 begin（begin 捕 {v4} 新树）。
    std::fs::write(root.join("code.txt"), "v4").unwrap();
    cp.begin_for_session(4, "b2", Some(&key_b));

    let err = al.redo_rewind(&key_a).await.unwrap_err();
    assert!(err.contains("失效"), "{err}");
    let (_, total, _, _) = read_chat_log(&key_a, 100, None);
    assert_eq!(total, 2, "行保持截断态");

    delete_chat_log(&key_a);
    delete_chat_log(&key_b);
}

// —— 重新生成支撑（2026-10-20）：rewind_before_message ——

/// 三轮摆法（两轮 stage 的直延长：turn 3 begin → snapshot 捕 v3 → 工具改
/// v4 → 行落盘）。返回时 code.txt = v4。
async fn stage_three_turns(tag: &str) -> (AgentLoop, String, tempfile::TempDir) {
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

    for (turn, next) in [(1usize, "v2"), (2, "v3"), (3, "v4")] {
        cp.begin(turn, "turn");
        cp.snapshot(&e3_modify("code.txt")).await;
        std::fs::write(dir.path().join("ws").join("code.txt"), next).unwrap();
        e3_append(&key, "user", &format!("turn {turn} q"), Some(turn));
        e3_append(&key, "assistant", &format!("turn {turn} a"), None);
    }

    (al, key, dir)
}

/// 重新生成热路径（末轮）：两轮会话对 turn 2 提问行（index 2）before 回退
/// → 提问行与其回复一并移除、turn 1 完整保留、文件回 turn 2 begin 前（v2）
/// ——「保留提问、撤掉回复」形态，`rewind_to_message` 表达不了（其 cut 落
/// 在下一轮 user 行，末轮 = 空转）。
#[tokio::test]
async fn e3_rewind_before_last_turn_drops_reply_keeps_question() {
    let (al, key, dir) = stage_two_turns("before-last").await;
    let file = dir.path().join("ws").join("code.txt");

    let out = al.rewind_before_message(&key, 2, false).await.unwrap();
    assert_eq!(out["kept_count"], 2, "turn 1 保留: {out}");
    assert_eq!(out["removed_count"], 2, "turn 2 问+答一并移除: {out}");
    assert_eq!(out["restore_turn"], 2, "锚 = 提问行自带的 begin 标记");
    assert_eq!(out["file_restore"], "applied");
    assert_eq!(out["redoable"], true);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");

    let (rows, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 2);
    assert_eq!(rows[0]["content"], "turn 1 q");
    assert_eq!(rows[1]["content"], "turn 1 a");

    delete_chat_log(&key);
}

/// 中间轮 before 回退：三轮会话对 turn 2 提问行（index 2）→ turn 2 与其后
/// 所有轮（turn 3）一并移除（前缀截断语义），文件把 turn 3 的 v4 也滚回 v2
/// ——锚=提问行 begin 标记，restore_code(2) 覆盖 ≥turn 2 的全部变更。
#[tokio::test]
async fn e3_rewind_before_mid_conversation_drops_question_and_later_turns() {
    let (al, key, dir) = stage_three_turns("before-mid").await;
    let file = dir.path().join("ws").join("code.txt");

    let out = al.rewind_before_message(&key, 2, false).await.unwrap();
    assert_eq!(out["kept_count"], 2, "{out}");
    assert_eq!(out["removed_count"], 4, "turn 2 + turn 3 各两行: {out}");
    assert_eq!(out["restore_turn"], 2);
    assert_eq!(out["file_restore"], "applied");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "v2",
        "turn 3 的 v4 一并回滚"
    );

    let (rows, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 2);
    assert_eq!(rows[1]["content"], "turn 1 a");

    delete_chat_log(&key);
}

/// cut=0：首轮提问行 before 回退 → 会话截到空（`rewind_to_message` 契约表
/// 达不了的形态，`rewind_last_turn` 注释为证）；文件回 turn 1 begin 前
/// （v1）；redo 整段回填（LIFO 栈条目 kept_count=0 与空会话吻合）。
#[tokio::test]
async fn e3_rewind_before_cut_zero_clears_all_and_redo_roundtrips() {
    let (al, key, dir) = stage_two_turns("before-zero").await;
    let file = dir.path().join("ws").join("code.txt");

    let out = al.rewind_before_message(&key, 0, false).await.unwrap();
    assert_eq!(out["kept_count"], 0, "{out}");
    assert_eq!(out["removed_count"], 4);
    assert_eq!(out["restore_turn"], 1, "锚 = 首个 checkpoint turn");
    assert_eq!(out["file_restore"], "applied");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");

    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 0, "会话截到空");

    // redo：空会话行数守卫（0 == kept_count）通过，整段回填。
    let out = al.redo_rewind(&key).await.unwrap();
    assert_eq!(out["restored_count"], 4, "{out}");
    let (rows, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4);
    assert_eq!(rows[0]["content"], "turn 1 q");
    assert_eq!(rows[3]["content"], "turn 2 a");

    delete_chat_log(&key);
}

/// 契约守卫：assistant 行落点 loud 拒绝（会产生「删提问留回复」破碎轮）；
/// 越界 / 空会话诚实报错。
#[tokio::test]
async fn e3_rewind_before_rejects_non_user_row_out_of_range_and_empty() {
    let (al, key, _dir) = stage_two_turns("before-guard").await;

    // index 1 = assistant 行。
    let err = al.rewind_before_message(&key, 1, false).await.unwrap_err();
    assert!(err.contains("不是 user 行"), "{err}");

    // 越界（== len 与 > len 同罪）。
    let err = al.rewind_before_message(&key, 4, false).await.unwrap_err();
    assert!(err.contains("超出范围"), "{err}");

    // 空会话。
    let empty_key = e3_uniq_key("before-guard-empty");
    delete_chat_log(&empty_key);
    let err = al
        .rewind_before_message(&empty_key, 0, false)
        .await
        .unwrap_err();
    assert!(err.contains("没有可回退"), "{err}");
    delete_chat_log(&empty_key);

    delete_chat_log(&key);
}

/// busy 闸：会话处理中零副作用诚实拒绝（与 rewind_to_message / undo 入口
/// 同语义），释放后放行。
#[tokio::test]
async fn e3_rewind_before_rejects_busy_session() {
    let (al, key, dir) = stage_two_turns("before-busy").await;
    let file = dir.path().join("ws").join("code.txt");

    assert!(al.try_acquire_session(&key));
    let err = al.rewind_before_message(&key, 2, false).await.unwrap_err();
    assert!(err.contains("正在处理"), "{err}");
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4, "零副作用");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v3");

    al.release_session(&key);
    let out = al.rewind_before_message(&key, 2, false).await.unwrap();
    assert_eq!(out["removed_count"], 2, "{out}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");

    delete_chat_log(&key);
}

// —— BUG-HUNT R6（2026-10-23）：under_history_gate 会话闸补测 ——

/// busy 闸补齐 rewind_to_message 入口（undo / rewind_before 已有专测）：
/// 拒绝 + 零副作用 + 释放后放行。
#[tokio::test]
async fn e3_rewind_to_message_rejects_busy_session() {
    let (al, key, dir) = stage_two_turns("rew-busy").await;
    let file = dir.path().join("ws").join("code.txt");

    assert!(al.try_acquire_session(&key));
    let err = al.rewind_to_message(&key, 0, false).await.unwrap_err();
    assert!(err.contains("正在处理"), "{err}");
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4, "零副作用");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v3");

    al.release_session(&key);
    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
    assert_eq!(out["removed_count"], 2, "{out}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "v2");

    delete_chat_log(&key);
}

/// busy 闸补齐 redo_rewind 入口：闸拒绝先于「栈空」语义错误——本测连
/// undo 都没做过，若闸失效会落到「没有可重做」而非「正在处理」。
#[tokio::test]
async fn e3_redo_rejects_busy_session_before_stack_semantics() {
    let (al, key, _dir) = stage_two_turns("redo-busy").await;

    assert!(al.try_acquire_session(&key));
    let err = al.redo_rewind(&key).await.unwrap_err();
    assert!(err.contains("正在处理"), "闸拒绝必须先于栈空语义: {err}");

    al.release_session(&key);
    let err = al.redo_rewind(&key).await.unwrap_err();
    assert!(err.contains("没有可重做"), "{err}");

    delete_chat_log(&key);
}

/// 闸的错误/成功路径释放：内层 Err（越界 index）早退后 guard drop 必须
/// 把 busy 旗标还回去——不留「永久 busy、会话从此全拒」的死会话。
#[tokio::test]
async fn e3_gate_releases_session_after_inner_error() {
    let (al, key, _dir) = stage_two_turns("gate-err").await;

    let err = al.rewind_to_message(&key, 99, false).await.unwrap_err();
    assert!(!err.contains("正在处理"), "{err}");
    assert!(!al.is_session_busy(&key), "错误路径必须释放 busy 旗标");

    // 释放成立：合法回退紧接着可用（若旗标泄漏这里会假拒绝）。
    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
    assert_eq!(out["removed_count"], 2, "{out}");
    assert!(!al.is_session_busy(&key), "成功路径也要释放");

    delete_chat_log(&key);
}

/// steer 注入行追加（BUG-20261025-19 写入形态：role=user + `"steer": true`
/// 标记、无 checkpoint_turn——run_loop 注入路径同构）。
fn e3_append_steer(key: &str, content: &str) {
    crate::chat_log::append_chat_log_meta(
        key,
        "user",
        &format!("[steer] {content}"),
        &crate::chat_log::ChatLogMeta {
            steer: true,
            ..Default::default()
        },
    );
}

/// BUG-20261025-19 回归锁：「撤销最后一轮」在最后一轮含 steer 注入时，
/// 截断点必须落在真实轮首——旧行为切在 steer 行上，只删掉 steer 尾巴、
/// 轮的提问与首轮回复残留、文件也不回滚（restore 锚 None）。
#[tokio::test]
async fn e3_rewind_last_turn_skips_steer_rows() {
    let (al, key, dir) = stage_two_turns("undo-steer").await;
    let file = dir.path().join("ws").join("code.txt");
    // steer 注入 + 后续回复落在 turn 2 尾部（轮中间注入的落盘时序）。
    e3_append_steer(&key, "换 venv 重跑");
    e3_append(&key, "assistant", "turn 2 a after steer", None);

    let out = al.rewind_last_turn(&key).await.unwrap();
    // cut = turn 2 真实轮首（idx 2）→ turn 2 连 steer 整轮移除。
    assert_eq!(out["kept_count"], 2, "{out}");
    assert_eq!(out["removed_count"], 4);
    assert_eq!(out["restore_turn"], 2, "{out}");
    assert_eq!(out["file_restore"], "applied");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "v2",
        "整轮回滚到 turn 2 begin 前"
    );

    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 2, "turn 1 完整保留");
    delete_chat_log(&key);
}

/// BUG-20261025-19 回归锁：回退到含 steer 注入的轮首 = 整轮保留（steer
/// 与其后回复随轮走）——旧行为切在 steer 行上，轮被拦腰截断（后 steer
/// 回复丢失）且回滚锚错位。
#[tokio::test]
async fn e3_rewind_to_message_keeps_whole_steered_turn() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    let file = root.join("code.txt");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&file, "v1").unwrap();

    let al = AgentLoop::new(Box::new(E3NoopProvider), e3_config());
    al.set_checkpoint_store(Arc::new(crate::checkpoint::CheckpointStore::new(
        None, root,
    )));
    let key = e3_uniq_key("rew-steer");
    delete_chat_log(&key);
    let cp = al.attached_checkpoint().unwrap();

    // turn 1：begin → snapshot → 工具改 v2 → 行落盘（含轮中间 steer 注入
    // + 后续回复——都在 turn 1 begin 之后、turn 2 begin 之前）。
    cp.begin(1, "turn 1");
    cp.snapshot(&e3_modify("code.txt")).await;
    std::fs::write(dir.path().join("ws").join("code.txt"), "v2").unwrap();
    e3_append(&key, "user", "turn 1 q", Some(1));
    e3_append(&key, "assistant", "turn 1 a", None);
    e3_append_steer(&key, "补充说明");
    e3_append(&key, "assistant", "turn 1 a after steer", None);

    // turn 2：begin → snapshot → 工具改 v3 → 行落盘。
    cp.begin(2, "turn 2");
    cp.snapshot(&e3_modify("code.txt")).await;
    std::fs::write(dir.path().join("ws").join("code.txt"), "v3").unwrap();
    e3_append(&key, "user", "turn 2 q", Some(2));
    e3_append(&key, "assistant", "turn 2 a", None);

    // 回退到 turn 1 轮首（idx 0）：cut = turn 2 轮首（idx 4）→ turn 1 连
    // steer 整轮 4 行保留，turn 2 两行移除。
    let out = al.rewind_to_message(&key, 0, false).await.unwrap();
    assert_eq!(out["kept_count"], 4, "{out}");
    assert_eq!(out["removed_count"], 2);
    assert_eq!(out["restore_turn"], 2);
    assert_eq!(out["file_restore"], "applied");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "v2",
        "回滚到 turn 1 完成（含 steer 尾巴）时的文件态"
    );

    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4);
    delete_chat_log(&key);
}

/// BUG-20261025-19 回归锁：`rewind_before_message` 落在 steer 注入行上
/// loud 拒绝（steer 行不是轮首，与 assistant 行同罪——旧行为放行会产生
/// 「删 steer 尾巴留轮首」的破碎半轮）。
#[tokio::test]
async fn e3_rewind_before_rejects_steer_row() {
    let (al, key, _dir) = stage_two_turns("before-steer").await;
    e3_append_steer(&key, "补充");
    // steer 行 index = 4（旧行为在此放行并切出破碎半轮）。
    let err = al.rewind_before_message(&key, 4, false).await.unwrap_err();
    assert!(err.contains("steer"), "{err}");
    delete_chat_log(&key);
}
