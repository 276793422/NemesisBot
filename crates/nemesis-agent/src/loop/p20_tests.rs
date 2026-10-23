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

fn p20_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
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

// ---------------------------------------------------------------------------
// F8（2026-09-27）：force 强过的强恢复动作落审计链台账（Merkle
// append-only；此前只有 tracing warn——轮转即失，不算诚实留痕）。
// ---------------------------------------------------------------------------

/// force rewind 覆盖外部修改冲突 → 审计链落一条 session.rewind/forced 事件
/// （operation/tool/decision/target 可回读，链校验通过）；无冲突直通的
/// rewind 不产生假账（链保持空）。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p20_rewind_force_records_audit_chain_event() {
    let (al, key, dir) = stage_sealed_two_turns("audit").await;
    let file = dir.path().join("ws").join("code.txt");

    // 挂真实 SecurityPlugin（审计链启用，指向 tempdir）。
    let chain_path = dir.path().join("audit_chain.jsonl");
    let plugin = std::sync::Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig {
            enabled: true,
            audit_chain_enabled: true,
            audit_chain_path: Some(chain_path.to_string_lossy().to_string()),
            default_action: "allow".to_string(),
            ..Default::default()
        },
    ));
    let mut al_mut = al;
    al_mut.set_security_plugin(plugin.clone());
    let al = al_mut;
    let chain = plugin.audit_chain().unwrap();

    // 冲突 + force → 台账一条 forced 事件。
    std::fs::write(&file, "v4-external").unwrap();
    let out = al.rewind_to_message(&key, 0, true).await.unwrap();
    assert_eq!(out["forced"], true, "{out}");

    let ev = chain
        .get_event(0)
        .expect("force rewind 必须在审计链落一条事件");
    assert_eq!(ev.operation, "session.rewind");
    assert_eq!(ev.tool_name, "rewind_to_message");
    assert_eq!(ev.decision, "forced");
    assert_eq!(ev.user, "user");
    assert!(
        ev.target.contains(&key) && ev.target.contains("idx=0"),
        "target 应含会话键与消息下标: {}",
        ev.target
    );
    assert!(
        ev.reason.contains("1 个外部修改冲突"),
        "reason 应含冲突计数: {}",
        ev.reason
    );
    assert!(chain.verify_range(0, 0).unwrap(), "链校验");

    // 无冲突直通的 rewind 不产生假账（链仍只有 1 条）。
    let out = al.rewind_to_message(&key, 0, true).await.unwrap();
    assert_eq!(out["forced"], false, "{out}");
    assert_eq!(chain.total_event_count(), 1, "无冲突不得记账");

    delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// R31（2026-10-23 会话回退波及面收窄）：rewind 是会话级 API，但 turn 序号
// 与 checkpoint 索引是 loop 全局的——修复前回退会话 A 会把并发会话 B 在
// 其回退点之后的文件变更静默碾掉（B 的封印=现盘，冲突预检判「干净」不
// 拦），并把 B 的 checkpoint 条目整段清掉（永久摧毁 B 的 rewind/redo）。
// ---------------------------------------------------------------------------

/// 会话级回退的全链路编排测试：A(t1,t2) + B(t3) 交错在同一 store（主 loop
/// 上多会话共享一个 checkpoint store 是默认形态）。A 回退自己的 t2：
/// - 默认（force=false）：b.txt 是其他会话波及面 → **零副作用拒绝**，
///   `conflicts` 空 + `other_session_paths=["b.txt"]`（语义区分「外部修改」
///   与「波及并发会话」）；
/// - force=true：强过（a.txt 恢复到 t2 begin 树；全树恢复按用户意志碾掉
///   b.txt），但 **B 的会话行与 checkpoint 条目（turn 3）原样保留**——
///   修复前这里被 scoped 掉的 truncate_from 整段清空。
#[tokio::test]
async fn r31_rewind_session_scoped_other_session_paths_block_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    std::fs::write(root.join("a.txt"), "a1").unwrap();

    let al = AgentLoop::new(Box::new(P20NoopProvider), p20_config());
    let store = Arc::new(crate::checkpoint::CheckpointStore::new(None, root.clone()));
    al.set_checkpoint_store(store);
    let key_a = p20_uniq_key("r31a");
    let key_b = p20_uniq_key("r31b");
    delete_chat_log(&key_a);
    delete_chat_log(&key_b);

    let cp = al.attached_checkpoint().unwrap();

    // A turn1：a.txt → v2。turn 序号模拟 loop 全局计数器（turn_preamble
    // 的 fetch_add），会话归属经 begin_for_session 穿针。
    cp.begin_for_session(1, "a q1", Some(&key_a));
    std::fs::write(root.join("a.txt"), "a2").unwrap();
    cp.snapshot(&p20_modify("a.txt")).await;
    cp.seal_turn(1);
    p20_append(&key_a, "user", "a q1", Some(1));
    p20_append(&key_a, "assistant", "a ans1", None);

    // A turn2：a.txt → v3（本测试的回退目标）。
    cp.begin_for_session(2, "a q2", Some(&key_a));
    std::fs::write(root.join("a.txt"), "a3").unwrap();
    cp.snapshot(&p20_modify("a.txt")).await;
    cp.seal_turn(2);
    p20_append(&key_a, "user", "a q2", Some(2));
    p20_append(&key_a, "assistant", "a ans2", None);

    // B turn3（在 A 的 t2 之后被接纳）：b.txt 独立会话的工作。
    cp.begin_for_session(3, "b q1", Some(&key_b));
    std::fs::write(root.join("b.txt"), "b2").unwrap();
    cp.snapshot(&p20_modify("b.txt")).await;
    cp.seal_turn(3);
    p20_append(&key_b, "user", "b q1", Some(3));
    p20_append(&key_b, "assistant", "b ans1", None);

    // A 回退保自己的 t1（index 0 所在 turn 之后全截）→ restore_turn=2，
    // 回退区间 [2,∞) 覆盖 B 的 turn3。
    let out = al.rewind_to_message(&key_a, 0, false).await.unwrap();
    assert_eq!(out["blocked"], true, "{out}");
    assert_eq!(out["reason"], "conflict");
    assert_eq!(out["restore_turn"], 2);
    assert!(
        out["conflicts"].as_array().unwrap().is_empty(),
        "b.txt 是波及面不是外部修改: {out}"
    );
    assert_eq!(
        out["other_session_paths"],
        serde_json::json!(["b.txt"]),
        "{out}"
    );

    // 零副作用四连：两个会话的 jsonl 不动、两个文件不动、undo 栈不入。
    assert_eq!(read_chat_log(&key_a, 100, None).1, 4);
    assert_eq!(read_chat_log(&key_b, 100, None).1, 2);
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "a3");
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "b2");
    let err = al.redo_rewind(&key_a).await.unwrap_err();
    assert!(err.contains("没有可重做"), "{err}");

    // force 强过：a.txt 恢复到 t2 begin 树（=t1 完成态 v2）；全树恢复把
    // b.txt 碾掉（force = 用户对波及面的显式意志，与 P20 外部冲突同款）。
    let out = al.rewind_to_message(&key_a, 0, true).await.unwrap();
    assert_eq!(out["blocked"], serde_json::Value::Null, "{out}");
    assert_eq!(out["forced"], true, "波及面 + force = 审计留痕: {out}");
    assert_eq!(out["file_restore"], "applied");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "a2");
    assert!(!root.join("b.txt").exists());

    // B 的会话行与 checkpoint 条目（turn 3）原样保留——修复前 turn 3 的
    // 条目被全局 truncate_from 清空，B 的 rewind/redo 能力被永久摧毁。
    assert_eq!(read_chat_log(&key_b, 100, None).1, 2, "B 的会话行不动");
    let turns: Vec<usize> = al.checkpoint_list().iter().map(|m| m.turn).collect();
    assert_eq!(turns, vec![1, 3], "B 的 checkpoint 条目必须幸存");

    delete_chat_log(&key_a);
    delete_chat_log(&key_b);
}

/// store 级 scope 语义：冲突报告与会话收窄截断按归属处置；`begin`（无会
/// 话，legacy）条目按「同会话」清（不可归属不惩罚 = 存量行为逐字节不变）。
#[tokio::test]
async fn r31_scoped_truncate_and_conflict_report_are_session_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let ckpt_dir = dir.path().join(".ck");
    let cp = Arc::new(crate::checkpoint::CheckpointStore::new(
        Some(ckpt_dir.clone()),
        root.clone(),
    ));

    // sA turn1 / sB turn2 / legacy turn3（begin 无会话）。
    cp.begin_for_session(1, "a1", Some("sA"));
    std::fs::write(root.join("a.txt"), "a2").unwrap();
    cp.snapshot(&p20_modify("a.txt")).await;
    cp.seal_turn(1);
    cp.begin_for_session(2, "b1", Some("sB"));
    std::fs::write(root.join("b.txt"), "b2").unwrap();
    cp.snapshot(&p20_modify("b.txt")).await;
    cp.seal_turn(2);
    cp.begin(3, "legacy");
    std::fs::write(root.join("c.txt"), "c2").unwrap();
    cp.snapshot(&p20_modify("c.txt")).await;
    cp.seal_turn(3);

    // sA 视角（turn>=2）：b.txt 是其他会话波及面；legacy c.txt 走既有指纹
    // 语义（同会话）。无外部修改 → conflicts 空。
    let rpt = cp.conflict_scan_scoped(2, Some("sA"));
    assert!(rpt.conflicts.is_empty(), "{rpt:?}");
    assert_eq!(rpt.other_session_paths, vec!["b.txt".to_string()]);
    // 不收窄（conflict_scan / rewind_session=None）= 旧全局语义，不产
    // other_session_paths。
    let rpt = cp.conflict_scan(2);
    assert!(rpt.conflicts.is_empty(), "{rpt:?}");
    assert!(rpt.other_session_paths.is_empty(), "{rpt:?}");

    // sA 视角截断：turn2（sB，其他会话）保留；turn3（None，同会话）清；
    // 磁盘镜像同判定（其他会话的 turn-N.json 幸存）。
    cp.truncate_from_scoped(2, Some("sA"));
    let turns: Vec<usize> = cp.list_meta().iter().map(|m| m.turn).collect();
    assert_eq!(turns, vec![1, 2]);
    assert!(
        ckpt_dir.join("turn-2.json").exists(),
        "其他会话的落盘条目必须保留"
    );
    assert!(!ckpt_dir.join("turn-3.json").exists());

    // sB 视角截断：turn2 同会话 → 清，磁盘同步。
    cp.truncate_from_scoped(2, Some("sB"));
    let turns: Vec<usize> = cp.list_meta().iter().map(|m| m.turn).collect();
    assert_eq!(turns, vec![1]);
    assert!(!ckpt_dir.join("turn-2.json").exists());
}

/// 老载荷兼容：历史 turn-N.json 无 `session` 字段（serde default → None）
/// 加载不受影响，scope 判定按「同会话」（不可归属不惩罚，全局截断照清）。
#[tokio::test]
async fn r31_old_payload_without_session_field_loads_as_same_session() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let ckpt_dir = dir.path().join(".ck");
    std::fs::create_dir_all(&ckpt_dir).unwrap();
    // 手工摆一份无 session 字段的历史载荷（修复前形态）。
    std::fs::write(
        ckpt_dir.join("turn-5.json"),
        br#"{"turn":5,"time":"t","prompt":"legacy","files":[],"paths":["old.txt"],"tree":null,"after":[],"sealed":true}"#,
    )
    .unwrap();

    let cp = crate::checkpoint::CheckpointStore::new(Some(ckpt_dir.clone()), root);
    let metas = cp.list_meta();
    assert_eq!(metas.len(), 1, "历史载荷照常加载");
    assert_eq!(metas[0].turn, 5);

    // scoped 截断把不可归属条目按同会话清——存量行为逐字节不变。
    cp.truncate_from_scoped(5, Some("any-session"));
    assert!(
        !ckpt_dir.join("turn-5.json").exists(),
        "不可归属 = 同会话 = 可清"
    );
    assert!(cp.list_meta().is_empty());
}
