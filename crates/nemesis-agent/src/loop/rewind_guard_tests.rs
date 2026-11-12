// R840 回归锁：rewind/redo 守卫面七处修复的编排测试（git 影子形态为主）。
//
// - H-1（外部新文件保护）：空闲期新文件默认拒绝（force 显式强过）；
//   turn 窗口内的产物不拦——同一回退里按诞生时刻区分两类未声明文件。
// - M-1（redo 前向恢复整体失败）：影子库不可用时零推进 + 条目还栈可重试，
//   removed_rows 唯一副本不随失败销毁。
// - M-2（双形态声明）：同文件以相对/绝对两形态先后声明不再误报冲突。
// - M-3（守卫 2 会话收窄）：其他会话 undo 后零盘面 begin 不再击穿 redo；
//   （真盘面波及改由冲突预检拦截的契约更新见 e3_tests r71stale）。
// - L-2（session_file_diff 越界闸）：`..` 段路径拒绝读取。
// - L-3（dedup_file_changes_with_key）：去重键注入纯函数。

use super::*;
use crate::chat_log::{delete_chat_log, read_chat_log};

// —— 空 provider：编排测试不走 dispatch/LLM ——

struct R840NoopProvider;

#[async_trait]
impl LlmProvider for R840NoopProvider {
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

fn r840_config() -> AgentConfig {
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

fn r840_uniq_key(tag: &str) -> String {
    format!(
        "test:r840guard:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn r840_modify(rel: &str) -> FileChange {
    FileChange {
        path: rel.to_string(),
        kind: FileChangeKind::Modify,
    }
}

/// 按 真实写入形态摆行：user 行带 checkpoint_turn 标记。
fn r840_append(key: &str, role: &str, content: &str, turn: Option<usize>) {
    crate::chat_log::append_chat_log_meta(
        key,
        role,
        content,
        &crate::chat_log::ChatLogMeta {
            checkpoint_turn: turn,
            ..Default::default()
        },
    )
}

/// git 影子形态 + 封印链两轮摆法（同 p20 stage_sealed_two_turns 姿态）：
/// 每 turn begin → 工具写盘 → snapshot 声明 → seal_turn 收尾封印 → 行落盘。
/// turn1 完成态 code.txt=v2、turn2 完成态 v3（均已封印）。
async fn stage_sealed_git(tag: &str) -> (AgentLoop, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    git2::Repository::init(&root).unwrap();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let al = AgentLoop::new(Box::new(R840NoopProvider), r840_config());
    let store = Arc::new(crate::checkpoint::CheckpointStore::new(None, root.clone()));
    assert_eq!(
        store.backend(),
        crate::checkpoint::CheckpointBackend::Git,
        "root/.git 目录 → git 影子 backend"
    );
    al.set_checkpoint_store(store);
    let key = r840_uniq_key(tag);
    delete_chat_log(&key);

    let cp = al.attached_checkpoint().unwrap();

    // turn 1：begin → 写 v2 → 声明 → 封印 → 行落盘。
    cp.begin(1, "turn 1");
    std::fs::write(root.join("code.txt"), "v2").unwrap();
    cp.snapshot(&r840_modify("code.txt")).await;
    cp.seal_turn(1);
    r840_append(&key, "user", "turn 1 q", Some(1));
    r840_append(&key, "assistant", "turn 1 a", None);

    // turn 2：begin → 写 v3 → 声明 → 封印 → 行落盘。
    cp.begin(2, "turn 2");
    std::fs::write(root.join("code.txt"), "v3").unwrap();
    cp.snapshot(&r840_modify("code.txt")).await;
    cp.seal_turn(2);
    r840_append(&key, "user", "turn 2 q", Some(2));
    r840_append(&key, "assistant", "turn 2 a", None);

    (al, key, dir)
}

/// H-1 回归锁 A：会话结束后新出现的外部文件（未声明、诞生晚于最后被回退
/// 行）默认拒绝——删除不可逆，用户手工放置的文件不得被回退静默擦掉。零副
/// 作用：文件在盘、会话不截、undo 栈不入。
#[tokio::test]
async fn r840_rewind_blocks_idle_period_external_new_file() {
    let (al, key, dir) = stage_sealed_git("idle-block").await;
    let ws = dir.path().join("ws");

    // 会话结束（最后行落盘）之后外部放置的文件。间隔纪律（BUG-20261112-106
    // ）：staging 与落盘之间必须拉开 > fs inode 时间戳粒度（ext4 等本地
    // fs 走 coarse 时钟记账，实测可滞后 CLOCK_REALTIME 精读 ~10ms）——
    // 零间隔会让 recorded-born 逆行进 turn 窗口，守卫测不出「之后」。生
    // 产语义不受影响：真实空闲期 = 秒级之后，精度绰绰有余。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(ws.join("external.txt"), "user-note").unwrap();

    let out = al.rewind_to_message(&key, 0, false, None).await.unwrap();
    assert_eq!(out["blocked"], true, "{out}");
    assert_eq!(out["reason"], "external_new_files");
    let listed = out["external_new_paths"].as_array().unwrap();
    assert!(
        listed.iter().any(|p| p == "external.txt"),
        "空闲期新文件必须在保护清单里: {out}"
    );

    // 零副作用三连：文件在盘、会话不截、undo 栈不入。
    assert_eq!(
        std::fs::read_to_string(ws.join("external.txt")).unwrap(),
        "user-note",
        "被拒回退不得删除外部文件"
    );
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4, "被拒回退不得截断会话");
    let err = al.redo_rewind(&key).await.unwrap_err();
    assert!(err.contains("没有可重做"), "{err}");

    delete_chat_log(&key);
}

/// H-1 回归锁 B：空闲期新文件 force 显式强过——删除生效、forced 审计留痕、
/// 回退正常推进。blocked 是可重试阻塞点（零副作用），不是终态。
/// （判别力的另一侧——turn 窗口内诞生的未声明文件**不拦**、随回退删除——
/// 由既有 `e3_git_rewind_redo_covers_undeclared_shell_side_effects` 锁定：
/// 该用例在 H-1 闸在场后仍须保持直通，即 mtime 判别不误伤 turn 产物。）
#[tokio::test]
async fn r840_rewind_force_overrides_idle_period_external_file() {
    let (al, key, dir) = stage_sealed_git("idle-force").await;
    let ws = dir.path().join("ws");

    // 间隔纪律同上（> fs inode 时间戳粒度，否则 recorded-born 逆行）。
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(ws.join("external.txt"), "user-note").unwrap();

    // 默认拒绝（同 H-1 A）。
    let out = al.rewind_to_message(&key, 0, false, None).await.unwrap();
    assert_eq!(out["blocked"], true, "{out}");
    assert_eq!(out["reason"], "external_new_files");

    // force 显式强过：删除生效 + forced 留痕 + 回退正常推进。
    let out = al.rewind_to_message(&key, 0, true, None).await.unwrap();
    assert_eq!(
        out["blocked"],
        serde_json::Value::Null,
        "force 强过不拒绝: {out}"
    );
    assert_eq!(
        out["forced"], true,
        "空闲期新文件 + force = 审计留痕: {out}"
    );
    assert!(!ws.join("external.txt").exists(), "force 强过后删除生效");
    assert_eq!(std::fs::read_to_string(ws.join("code.txt")).unwrap(), "v2");
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 2, "截断正常推进");

    delete_chat_log(&key);
}

/// M-1 回归锁：redo 前向恢复**整体失败**（影子库 checkpoints.git 被清理/
/// 损坏）= 零推进 + 条目还栈可重试——旧形态只记 note 照常回填并消耗条目，
/// removed_rows 唯一副本随条目丢失，redo 安全网永久失效。
#[tokio::test]
async fn r840_redo_forward_restore_total_failure_blocks_and_retains_entry() {
    let (al, key, dir) = stage_sealed_git("redo-no-shadow").await;
    let ws = dir.path().join("ws");

    let out = al.rewind_to_message(&key, 0, false, None).await.unwrap();
    assert_eq!(out["file_restore"], "applied", "{out}");
    assert_eq!(std::fs::read_to_string(ws.join("code.txt")).unwrap(), "v2");

    // 影子库整体不可用（外部清理/损坏的模拟）。
    let shadow = ws.join("logs").join("checkpoints.git");
    assert!(shadow.exists(), "影子库应已创建");
    std::fs::remove_dir_all(&shadow).unwrap();

    // redo：前向恢复整体失败 → blocked 结构化回执，行回填未做、条目还栈。
    let out = al.redo_rewind(&key).await.unwrap();
    assert_eq!(out["blocked"], true, "{out}");
    assert_eq!(out["reason"], "restore_failed");
    assert_eq!(out["file_restore"], "failed", "{out}");
    let (_, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 2, "行回填未做（零推进）");
    assert_eq!(
        std::fs::read_to_string(ws.join("code.txt")).unwrap(),
        "v2",
        "文件保持回退态"
    );

    // 条目还栈可重试：第二次 redo 撞同一个 blocked（影子库仍不在），
    // **不是**「没有可重做的回退」——条目未被首次失败消耗。
    let out2 = al.redo_rewind(&key).await.unwrap();
    assert_eq!(out2["blocked"], true, "{out2}");
    assert_eq!(out2["reason"], "restore_failed", "{out2}");

    delete_chat_log(&key);
}

/// M-2 回归锁：同一文件以**双形态**（turn 1 相对、turn 2 绝对）先后声明
/// 并封印——冲突预检按 comparable 归一只认最后声明的封印指纹，不再把旧
/// turn 的陈旧指纹当「外部修改」逼用户 force 自己的正常回退。
#[tokio::test]
async fn r840_rewind_dual_form_declaration_no_false_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    git2::Repository::init(&root).unwrap();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let al = AgentLoop::new(Box::new(R840NoopProvider), r840_config());
    let store = Arc::new(crate::checkpoint::CheckpointStore::new(None, root.clone()));
    al.set_checkpoint_store(store);
    let key = r840_uniq_key("dual-form");
    delete_chat_log(&key);
    let cp = al.attached_checkpoint().unwrap();

    // turn 1：相对形态声明。
    cp.begin(1, "turn 1");
    std::fs::write(root.join("code.txt"), "v2").unwrap();
    cp.snapshot(&r840_modify("code.txt")).await;
    cp.seal_turn(1);
    r840_append(&key, "user", "turn 1 q", Some(1));
    r840_append(&key, "assistant", "turn 1 a", None);

    // turn 2：同一文件的**绝对路径**形态声明（工具入参逐字串的真实形态）。
    cp.begin(2, "turn 2");
    let abs = root.join("code.txt");
    std::fs::write(&abs, "v3").unwrap();
    cp.snapshot(&FileChange {
        path: abs.to_string_lossy().to_string(),
        kind: FileChangeKind::Modify,
    })
    .await;
    cp.seal_turn(2);
    r840_append(&key, "user", "turn 2 q", Some(2));
    r840_append(&key, "assistant", "turn 2 a", None);

    // 盘面停在 v3 = 最后声明（绝对形态）的封印指纹一致 → 无冲突直通。
    // 修复前：字面键下两槽并存，相对槽的陈旧指纹（v2）对不上现盘 v3 →
    // 假冲突 blocked。
    let out = al.rewind_to_message(&key, 0, false, None).await.unwrap();
    assert_eq!(
        out["blocked"],
        serde_json::Value::Null,
        "双形态声明不得误报冲突: {out}"
    );
    assert_eq!(out["file_restore"], "applied", "{out}");
    assert_eq!(out["kept_count"], 2);
    assert_eq!(out["removed_count"], 2);
    assert_eq!(std::fs::read_to_string(&abs).unwrap(), "v2");

    // redo 往返仍工作（前向恢复到 rewind 时刻实时树 v3）。
    let out = al.redo_rewind(&key).await.unwrap();
    assert_eq!(out["file_restore"], "applied", "{out}");
    assert_eq!(std::fs::read_to_string(&abs).unwrap(), "v3");

    delete_chat_log(&key);
}

/// M-3 回归锁：undo 之后**其他会话** begin 新 turn（零盘面变更）不再击穿
/// redo——守卫 2 基线两侧同读本会话收窄口径，其他会话的索引活动不构成
/// 「本会话基线漂移」。修复前：全局 latest tree 把 B 的新 begin 树当漂移，
/// Err 直接销毁条目（removed_rows 唯一副本丢失）。
#[tokio::test]
async fn r840_redo_survives_other_session_zero_disk_change_begin() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    git2::Repository::init(&root).unwrap();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let al = AgentLoop::new(Box::new(R840NoopProvider), r840_config());
    let store = Arc::new(crate::checkpoint::CheckpointStore::new(None, root.clone()));
    al.set_checkpoint_store(store);
    let key_a = r840_uniq_key("m3-a");
    let key_b = r840_uniq_key("m3-b");
    delete_chat_log(&key_a);
    delete_chat_log(&key_b);
    let cp = al.attached_checkpoint().unwrap();

    // A 两 turn（v1→v2→v3）。
    cp.begin_for_session(1, "a1", Some(&key_a));
    std::fs::write(root.join("code.txt"), "v2").unwrap();
    r840_append(&key_a, "user", "a turn 1 q", Some(1));
    r840_append(&key_a, "assistant", "a turn 1 a", None);
    cp.begin_for_session(2, "a2", Some(&key_a));
    std::fs::write(root.join("code.txt"), "v3").unwrap();
    r840_append(&key_a, "user", "a turn 2 q", Some(2));
    r840_append(&key_a, "assistant", "a turn 2 a", None);

    // A 回退：文件回 v2，undo 入栈。
    let out = al.rewind_to_message(&key_a, 0, false, None).await.unwrap();
    assert_eq!(out["redoable"], true, "{out}");
    assert_eq!(
        std::fs::read_to_string(root.join("code.txt")).unwrap(),
        "v2"
    );

    // undo 之后 B begin 新 turn（零盘面变更——只 begin 不写文件）。
    cp.begin_for_session(3, "b1", Some(&key_b));

    // redo A：守卫 2 会话收窄 → 通行；冲突预检无漂移 → 前向恢复 + 回填。
    let out = al.redo_rewind(&key_a).await.unwrap();
    assert_eq!(out["restored_count"], 2, "{out}");
    assert_eq!(out["file_restore"], "applied", "{out}");
    let (_, total, _, _) = read_chat_log(&key_a, 100, None);
    assert_eq!(total, 4, "行回填");
    assert_eq!(
        std::fs::read_to_string(root.join("code.txt")).unwrap(),
        "v3",
        "前向恢复到 undo 时刻实时树"
    );

    delete_chat_log(&key_a);
    delete_chat_log(&key_b);
}

/// L-2 回归锁：session_file_diff 对含 `..` 段的声明路径拒绝读取——声明侧
/// 在边界校验之前登记，被 WorkspaceBoundary 拒绝的越界写尝试也会留声明，
/// 不挡的话 diff head 会把工作区外文件的现盘内容整体带出。
#[tokio::test]
async fn r840_session_file_diff_rejects_dotdot_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    git2::Repository::init(&root).unwrap();

    let al = AgentLoop::new(Box::new(R840NoopProvider), r840_config());
    let store = Arc::new(crate::checkpoint::CheckpointStore::new(None, root.clone()));
    al.set_checkpoint_store(store);
    let key = r840_uniq_key("dotdot");
    delete_chat_log(&key);
    let cp = al.attached_checkpoint().unwrap();

    // 越界写尝试的声明（tool_dispatch preview 阶段在边界校验前登记）。
    cp.begin(1, "turn 1");
    cp.snapshot(&FileChange {
        path: "..\\..\\evil.txt".to_string(),
        kind: FileChangeKind::Modify,
    })
    .await;

    let err = al
        .session_file_diff(&key, "..\\..\\evil.txt")
        .await
        .unwrap_err();
    assert!(err.contains(".."), "越界防护报错须点名 `..`: {err}");

    delete_chat_log(&key);
}

/// L-3 回归锁：dedup_file_changes_with_key 的去重键注入——同文件双形态
/// （键归一后相等）合并为一条（保首次出现位置、kind 取最后一次），不同
/// 文件各自保留；字面版（dedup_file_changes）对同串去重、异形态不合并。
#[test]
fn r840_dedup_file_changes_with_key_merges_dual_forms() {
    let fc = |p: &str, k: FileChangeKind| FileChange {
        path: p.to_string(),
        kind: k,
    };
    let changes = vec![
        fc("docs\\a.md", FileChangeKind::Create),
        fc("docs/a.md", FileChangeKind::Modify),
        fc("b.md", FileChangeKind::Modify),
    ];

    // 归一键：分隔符差异合并（保首次形态、kind 取最后）。
    let out =
        crate::chat_log::dedup_file_changes_with_key(changes.clone(), |p| p.replace('\\', "/"));
    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(out[0].path, "docs\\a.md", "保留首次出现形态");
    assert_eq!(out[0].kind, FileChangeKind::Modify, "kind 取最后一次登记");
    assert_eq!(out[1].path, "b.md");

    // 字面版：同串去重、异形态不合并（字面语义保持）。
    let out = crate::chat_log::dedup_file_changes(changes);
    assert_eq!(out.len(), 3, "字面键下异形态不得合并: {out:?}");
}

/// BUG-20261112-110 回归锁：rewind→redo 后对该区间的二次 rewind 必须诚实。
/// redo 不重建 checkpoint 索引（诚实边界，见 redo_rewind 文档）——redo 过
/// 的区间行标记还在、封印条目已退役。修复前：第二次 rewind 的 restore 是
/// 静默 no-op，回执却盖「applied」戳且 note=null（用户以为文件已恢复，
/// 实盘留着外部修改），冲突预检也因无条目失明。修复后：file_restore=
/// "skipped" + note 说明退役根因（与无锚臂「只截断对话不回滚文件」同
/// 待遇），文件保持外部修改态不被谎称恢复。
#[tokio::test]
async fn r840_rewind_after_redo_honest_skipped_receipt() {
    let (al, key, dir) = stage_sealed_git("redo-rewind-honest").await;
    let ws = dir.path().join("ws");

    // 第一次 rewind：截掉 turn 2 区间（rows 2-3），文件恢复 turn 2 begin
    // 态 v2；随后 redo 行回填 + 文件前向恢复 v3（封印条目仍退役）。
    let r1 = al.rewind_to_message(&key, 1, false, None).await.unwrap();
    assert_eq!(r1["file_restore"], "applied", "首回退正常恢复: {r1}");
    assert_eq!(
        std::fs::read_to_string(ws.join("code.txt")).unwrap(),
        "v2",
        "首回退文件恢复 v2"
    );
    let r2 = al.redo_rewind(&key).await.unwrap();
    assert_eq!(r2["file_restore"], "applied", "redo 前向恢复: {r2}");
    assert_eq!(
        std::fs::read_to_string(ws.join("code.txt")).unwrap(),
        "v3",
        "redo 前向恢复 v3"
    );

    // redo 后窗口内的外部修改（无新封印，预检本就看不见）。
    std::fs::write(ws.join("code.txt"), "v3-external").unwrap();

    // 第二次 rewind：restore_code 撞零条目区间 → skipped + note，不得谎
    // 称 applied；文件保持外部修改态。
    let r3 = al.rewind_to_message(&key, 1, false, None).await.unwrap();
    assert!(
        r3["blocked"].is_null(),
        "零条目区间不再伪装冲突面，回退本身照常: {r3}"
    );
    assert_eq!(r3["file_restore"], "skipped", "不得盖 applied 戳: {r3}");
    let note = r3["file_restore_note"].as_str().unwrap_or("");
    assert!(note.contains("退役"), "note 必须说明退役根因: {note}");
    assert_eq!(
        std::fs::read_to_string(ws.join("code.txt")).unwrap(),
        "v3-external",
        "文件保持外部修改态（诚实不恢复）"
    );
    // 对话截断照常生效（2b 截断与文件恢复解耦）。
    let rows = crate::chat_log::read_chat_log(&key, usize::MAX, None).1;
    assert_eq!(rows, 2, "第二次 rewind 行截断照常");
}
