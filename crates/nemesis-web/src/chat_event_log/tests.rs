//! L2（devtool-upgrade 阶段 6）— chat_event_log 单元测试。
//!
//! ⚠️ 进程级全局状态（SESSION_LOGS OnceLock）——并行测试共用一张表。
//! 隔离手段：每测试用唯一 session id（pid + 单调计数），互不可见；
//! 全局表有 128 会话上界，测试条目自然被逐出，无泄漏。

use super::*;

/// 每个触碰全局表的测试整段互斥（session_cap sweep 会逐别人的会话）。
/// 纯同步 #[test] 无运行时 → blocking_lock 合法。
macro_rules! table_guard {
    () => {
        super::test_support::GLOBAL_TABLE_LOCK.blocking_lock()
    };
}

fn unique_session(tag: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "l2-{}-{}-{tag}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

#[test]
fn record_seqs_are_monotonic_per_session() {
    let _guard = table_guard!();
    let sid = unique_session("mono");
    let s1 = record(&sid, "assistant", "first", None);
    let s2 = record(&sid, "assistant", "second", Some("m1"));
    let s3 = record(&sid, "user", "third", None);
    assert_eq!((s1, s2, s3), (1, 2, 3));
}

#[test]
fn independent_sessions_have_independent_counters() {
    let a = unique_session("a");
    let b = unique_session("b");
    assert_eq!(record(&a, "assistant", "x", None), 1);
    assert_eq!(record(&b, "assistant", "y", None), 1);
    assert_eq!(record(&a, "assistant", "z", None), 2);
}

#[test]
fn replay_after_returns_window_in_order() {
    let _guard = table_guard!();
    let sid = unique_session("replay");
    for i in 1..=5 {
        record(&sid, "assistant", &format!("m{i}"), None);
    }
    let (events, gap) = replay_after(&sid, 2);
    assert!(!gap);
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].seq, 3);
    assert_eq!(events[2].content, "m5");
    // 追平 → 空、无 gap
    let (events, gap) = replay_after(&sid, 5);
    assert!(events.is_empty() && !gap);
    // after=0 → 窗口内全量
    let (events, gap) = replay_after(&sid, 0);
    assert_eq!(events.len(), 5);
    assert!(!gap);
}

#[test]
fn replay_unknown_session_honest_gap_for_stale_clients() {
    let _guard = table_guard!();
    let sid = unique_session("unknown");
    // from-scratch 客户端（after=0）：无可补，非缺口
    let (events, gap) = replay_after(&sid, 0);
    assert!(events.is_empty() && !gap);
    // 持旧 seq（网关重启后会话环形为空）：gap=true → 前端全量刷新兜底
    let (events, gap) = replay_after(&sid, 42);
    assert!(events.is_empty() && gap);
}

#[test]
fn ring_cap_eviction_produces_gap() {
    let _guard = table_guard!();
    let sid = unique_session("cap");
    // 填满 + 溢出：推 SESSION_REPLAY_CAP + 50 条
    let total = SESSION_REPLAY_CAP + 50;
    for i in 1..=total {
        record(&sid, "assistant", &format!("m{i}"), None);
    }
    // after=0：最老缓冲 seq = 51 > 1 → 缺口滑出窗口
    let (events, gap) = replay_after(&sid, 0);
    assert!(gap);
    assert_eq!(events.len(), SESSION_REPLAY_CAP);
    assert_eq!(events[0].seq, 51);
    // 窗口内起点 → 无 gap
    let (events, gap) = replay_after(&sid, 50);
    assert!(!gap);
    assert_eq!(events.len(), total - 50);
    assert_eq!(events[0].content, "m51");
}

#[test]
fn session_cap_eviction_does_not_evict_current_session() {
    // 超 MAX_SESSIONS 后：新会话仍能记录且自身不被逐出（逐出别人）。
    // 此测试会制造 129+ 会话（逐出旧行）——上界保证无泄漏。
    let _guard = table_guard!();
    for i in 0..(MAX_SESSIONS + 5) {
        let sid = format!("l2-sweep-{}-{i}", std::process::id());
        assert_eq!(record(&sid, "assistant", "x", None), 1);
        let (events, _) = replay_after(&sid, 0);
        assert_eq!(events.len(), 1, "session {sid} must survive its own record");
    }
}
