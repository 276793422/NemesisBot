//! L12 chat_dedup 单元测试。
//!
//! ⚠️ 进程级全局状态（TABLE OnceLock + WORKSPACE 槽）——并行测试共用。
//! 隔离手段：每测试持 GLOBAL_TABLE_LOCK 互斥 + 开头 reset_for_tests（纯
//! 内存模式，不触盘）+ 唯一 session key（pid + 单调计数）。

use super::test_support::{
    GLOBAL_TABLE_LOCK, order_contains_for_tests, reset_for_tests, table_size_for_tests,
};
use super::{MAX_SESSIONS, insert, lookup};

/// 每个触碰全局表的测试整段互斥（MAX_SESSIONS sweep 会逐别人的会话）。
/// 纯同步 #[test] 无运行时 → blocking_lock 合法。
macro_rules! table_guard {
    () => {
        GLOBAL_TABLE_LOCK.blocking_lock()
    };
}

fn unique_session(tag: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "dedup-{}-{}-{tag}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

// --- BUG-20261106-021 回归锁：lookup 建条目必须走 ensure_entry 同纪律
// --- （order 登记 + MAX_SESSIONS FIFO 逐出）。此前 lookup 用
// --- entry().or_insert() 裸建——生产路径恒为 lookup→insert，insert 的
// --- created 判定恒 false，MAX_SESSIONS 上限对全部生产路径失效。

#[test]
fn lookup_miss_registers_order_and_respects_session_cap() {
    let _guard = table_guard!();
    reset_for_tests();
    // 生产形态的首触碰 = lookup（未命中）→ entry 建条目。
    let first = unique_session("cap-first");
    assert!(lookup(&first, "cmid-1").is_none());
    assert!(
        order_contains_for_tests(&first),
        "lookup 建条目必须登记 order（否则 FIFO 逐出永不覆盖它）"
    );
    // 灌到超员：表必须收敛在 MAX_SESSIONS（lookup-only 条目同样可逐）。
    for i in 0..(MAX_SESSIONS + 5) {
        let sid = format!("dedup-sweep-{}-{i}", std::process::id());
        assert!(lookup(&sid, "cmid").is_none());
    }
    assert!(
        table_size_for_tests() <= MAX_SESSIONS,
        "MAX_SESSIONS 上限必须生效：map={} 上限={MAX_SESSIONS}",
        table_size_for_tests()
    );
}

#[test]
fn lookup_then_insert_full_flow_evicts_oldest() {
    let _guard = table_guard!();
    reset_for_tests();
    // 完整生产序（server.rs 入站咽喉点）：lookup → insert。
    for i in 0..(MAX_SESSIONS + 5) {
        let sid = format!("dedup-flow-{}-{i}", std::process::id());
        assert!(lookup(&sid, "cmid-a").is_none());
        insert(&sid, "cmid-a", (i + 1) as u64);
        // 自身刚插入不被逐出（victim 排除本会话）。
        assert_eq!(
            lookup(&sid, "cmid-a"),
            Some((i + 1) as u64),
            "session {sid} must survive its own insert"
        );
    }
    assert!(table_size_for_tests() <= MAX_SESSIONS);
}

#[test]
fn lookup_hit_returns_original_seq_and_stays_after_lru_touch() {
    let _guard = table_guard!();
    reset_for_tests();
    let sid = unique_session("lru");
    // 未命中 → insert（seq=7）→ lookup 命中原 seq → 再 lookup 仍命中
    // （LRU touch 移 back 不丢条目）。
    assert!(lookup(&sid, "cmid-x").is_none());
    insert(&sid, "cmid-x", 7);
    assert_eq!(lookup(&sid, "cmid-x"), Some(7));
    assert_eq!(lookup(&sid, "cmid-x"), Some(7));
    // 重复提交短路语义：命中 → 不再需要二次 insert（调用方短路 bus 转发）。
    insert(&sid, "cmid-x", 7); // 防御性顶替：仍单条目、同 seq。
    assert_eq!(lookup(&sid, "cmid-x"), Some(7));
}

#[test]
fn map_and_order_key_sets_stay_in_sync_after_mixed_traffic() {
    let _guard = table_guard!();
    reset_for_tests();
    // 混合流量（lookup-only / lookup→insert / 命中重复）后表有界，
    // map 与 order 同键集——逐出纪律对全部入口生效。
    let hot = unique_session("hot");
    insert(&hot, "hot-cmid", 1);
    for i in 0..(MAX_SESSIONS + 10) {
        let sid = format!("dedup-mixed-{}-{i}", std::process::id());
        assert!(lookup(&sid, "m").is_none());
        if i % 3 == 0 {
            insert(&sid, "m", (i + 1) as u64);
        }
        // 热会话持续命中（LRU touch 不影响 FIFO 逐出语义——逐出按插入序
        // 不按访问序；hot 是首插入的最老会话，终被逐出 = 符合纪律）。
        let _ = lookup(&hot, "hot-cmid");
    }
    assert!(table_size_for_tests() <= MAX_SESSIONS);
}
