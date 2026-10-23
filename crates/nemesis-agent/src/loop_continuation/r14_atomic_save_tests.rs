//! BUG-HUNT R14：ContinuationStore::save 原子写回归锁。
//!
//! 修复前 `save` 用裸 `std::fs::write` 直写目标——崩溃/断电落在写中 → 半截
//! JSON 快照，重启后 load 解析失败被 warn 跳过，续行与 G5 pending 重建静默
//! 丢失。修复后走 `nemesis_utils::write_file_atomic`（同目录 cluster 侧
//! twin store 的 REL-002 形态：唯一临时名 + sync_all 先于 rename + 失败清理）。
//!
//! 进程内无法真实注入「写中崩溃」，本批锁三件可观察行为：
//! ① save→load 往返保真 + 覆盖写取最新；② save 成功后目录零 `.tmp-` 残留
//! （临时文件必被 rename 走或清理）；③ 崩溃残留的 `.tmp-*` 文件不被
//! list_pending 当 pending 任务（原子写引入的新不变量——本 store 开始产生
//! 临时文件后，恢复扫描必须天然无视它们）。

use super::*;

fn r14_snapshot(task: &str, marker: &str) -> ContinuationSnapshot {
    ContinuationSnapshot {
        task_id: task.to_string(),
        messages: format!(r#"[]/* {marker} */"#),
        tool_call_id: format!("tc_{marker}"),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: String::new(),
        created_at: chrono::Local::now().to_rfc3339(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(),
        final_persisted: false,
    }
}

fn assert_no_tmp_residue(cache_dir: &std::path::Path, where_: &str) {
    let residue: Vec<_> = std::fs::read_dir(cache_dir)
        .expect("read cache dir")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(
        residue.is_empty(),
        "{where_}: 目录不得残留 .tmp- 临时文件，实际 {residue:?}"
    );
}

/// save→load 往返保真；覆盖写后 load 取最新；全程零 `.tmp-` 残留。
#[test]
fn atomic_save_roundtrip_overwrite_and_no_residue() {
    let dir = tempfile::tempdir().expect("tempdir").keep();
    let store = ContinuationStore::new(&dir);
    let task = format!("r14atomic_{}", std::process::id());

    store.save(&r14_snapshot(&task, "v1")).expect("save v1");
    let loaded = store.load(&task).expect("load v1");
    assert_eq!(loaded.tool_call_id, "tc_v1");
    assert_eq!(loaded.messages, r#"[]/* v1 */"#);

    // 覆盖写：同 task_id 第二次 save 必须原子替换（不追加、不并存）。
    store.save(&r14_snapshot(&task, "v2")).expect("save v2");
    let loaded = store.load(&task).expect("load v2");
    assert_eq!(loaded.tool_call_id, "tc_v2");

    assert_no_tmp_residue(&dir.join("cluster").join("rpc_cache"), "after saves");
}

/// 崩溃残留的 `.tmp-*` 文件（无 .json 扩展名）不得被 list_pending 当作
/// pending 任务——原子写引入临时文件后，恢复扫描必须天然无视它们。
#[test]
fn crash_tmp_residue_not_listed_as_pending() {
    let dir = tempfile::tempdir().expect("tempdir").keep();
    let store = ContinuationStore::new(&dir);
    let cache = dir.join("cluster").join("rpc_cache");
    std::fs::create_dir_all(&cache).expect("mkdir cache");

    // 模拟崩溃残缺：半截 JSON 落在 write_file_atomic 的临时名形态里。
    std::fs::write(
        cache.join(format!(".tmp-{}-0-1", std::process::id())),
        r#"{"task_id": "half-writ"#,
    )
    .expect("plant tmp residue");

    assert!(
        store.list_pending().is_empty(),
        "崩溃残留 .tmp-* 不得进 pending 列表，实际 {:?}",
        store.list_pending()
    );
}
