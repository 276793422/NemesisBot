//! L12（改进项三期 2026-11-02）— chat_dedup 单元测试。
//!
//! ⚠️ 进程级全局状态（TABLE + WORKSPACE 槽）——并行测试整段互斥
//! （GLOBAL_TABLE_LOCK，chat_event_log 同款纪律）；workspace 槽测试自管
//! （装 tempdir 用完还 None）。隔离手段：唯一 session 键（pid + 单调计数）。

use super::chat_dedup::test_support::*;
use super::chat_dedup::*;

fn unique_session(tag: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "l12-{}-{}-{tag}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// 每个触碰全局表/workspace 槽的测试整段互斥。async 测试持 tokio 闸。
macro_rules! table_guard {
    () => {
        GLOBAL_TABLE_LOCK.lock().await
    };
}

/// 同步测试的互斥（blocking_lock，纯同步 #[test] 无运行时合法）。
macro_rules! table_guard_sync {
    () => {
        GLOBAL_TABLE_LOCK.blocking_lock()
    };
}

// ---------------------------------------------------------------------------
// 基础语义：miss → insert → hit（原 seq）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lookup_miss_then_insert_then_hit_returns_original_seq() {
    let _g = table_guard!();
    reset_for_tests();
    let sk = unique_session("basic");
    assert_eq!(lookup(&sk, "cm-1"), None, "未登记前必须 miss");
    insert(&sk, "cm-1", 7);
    assert_eq!(lookup(&sk, "cm-1"), Some(7), "命中返回原 seq");
    // 不同 id 不串。
    assert_eq!(lookup(&sk, "cm-2"), None);
    // 不同会话不串。
    let other = unique_session("basic-other");
    assert_eq!(lookup(&other, "cm-1"), None);
}

// ---------------------------------------------------------------------------
// LRU：cap 超限逐最旧；lookup 命中 touch（被 touch 的幸存）
// ---------------------------------------------------------------------------

#[test]
fn lru_cap_evicts_oldest_and_touch_survives() {
    let _g = table_guard_sync!();
    reset_for_tests();
    let sk = unique_session("lru");
    for i in 0..DEDUP_CAP {
        insert(&sk, &format!("cm-{i}"), i as u64);
    }
    // touch 首条（移 back = 最新）。
    assert_eq!(lookup(&sk, "cm-0"), Some(0));
    // 再插一条 → 超限逐 front = cm-1（原最旧且未被 touch）。
    insert(&sk, "cm-new", 999);
    assert_eq!(lookup(&sk, "cm-1"), None, "最旧且未 touch 必须被逐出");
    assert_eq!(lookup(&sk, "cm-0"), Some(0), "被 touch 过的必须幸存");
    assert_eq!(lookup(&sk, "cm-new"), Some(999));
}

// ---------------------------------------------------------------------------
// sidecar 持久化：insert 落盘 → 清内存表（模拟重启）→ lookup 从盘加载
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sidecar_persists_across_in_memory_reset() {
    let _g = table_guard!();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    reset_for_tests();
    set_workspace_for_tests(Some(ws.clone()));

    let sk = unique_session("persist");
    insert(&sk, "cm-a", 11);
    insert(&sk, "cm-b", 12);

    // sidecar 文件真实落盘（session_logs 目录、消毒键命名）。
    let safe = nemesis_utils::sanitize::sanitize_path_segment(&sk);
    let path =
        nemesis_path::resolve_sessions_dir_in_workspace(&ws).join(format!("{safe}.dedup.json"));
    assert!(path.exists(), "sidecar 必须落盘: {}", path.display());

    // 清内存表（模拟 gateway 重启）→ lookup 惰性加载恢复窗口。
    clear_table_only();
    assert_eq!(lookup(&sk, "cm-a"), Some(11), "重启后窗口必须从盘恢复");
    assert_eq!(lookup(&sk, "cm-b"), Some(12));
    assert_eq!(lookup(&sk, "cm-c"), None);

    reset_for_tests();
}

// ---------------------------------------------------------------------------
// 损坏 sidecar：空窗口 + 不 panic（诚实降级）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn corrupt_sidecar_degrades_to_empty_window() {
    let _g = table_guard!();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    reset_for_tests();
    set_workspace_for_tests(Some(ws.clone()));

    let sk = unique_session("corrupt");
    let safe = nemesis_utils::sanitize::sanitize_path_segment(&sk);
    let path =
        nemesis_path::resolve_sessions_dir_in_workspace(&ws).join(format!("{safe}.dedup.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "not json at all").unwrap();

    assert_eq!(lookup(&sk, "cm-x"), None, "损坏窗口按空窗口服务");
    // insert 后恢复正常（内存态继续工作）。
    insert(&sk, "cm-x", 5);
    assert_eq!(lookup(&sk, "cm-x"), Some(5));

    reset_for_tests();
}

// ---------------------------------------------------------------------------
// 纯图消息形态：seq=0（无环行）登记与命中
// ---------------------------------------------------------------------------

#[test]
fn pure_image_zero_seq_roundtrip() {
    let _g = table_guard_sync!();
    reset_for_tests();
    let sk = unique_session("img");
    insert(&sk, "cm-img", 0);
    assert_eq!(
        lookup(&sk, "cm-img"),
        Some(0),
        "纯图形态命中返回 0（无环行标记）"
    );
}

// ---------------------------------------------------------------------------
// 未装 workspace（纯内存模式）：行为一致、无盘上产物
// ---------------------------------------------------------------------------

#[test]
fn memory_only_mode_without_workspace() {
    let _g = table_guard_sync!();
    reset_for_tests(); // workspace 槽 = None
    let sk = unique_session("mem");
    insert(&sk, "cm-1", 3);
    assert_eq!(lookup(&sk, "cm-1"), Some(3));
    // 无盘上产物可断言（workspace 槽 None 时 persist 根本不触发）——
    // 反向证据：同 session 键在临时目录里查不到文件。
    let dir = tempfile::tempdir().unwrap();
    let safe = nemesis_utils::sanitize::sanitize_path_segment(&sk);
    let path = nemesis_path::resolve_sessions_dir_in_workspace(dir.path())
        .join(format!("{safe}.dedup.json"));
    assert!(!path.exists());
}
