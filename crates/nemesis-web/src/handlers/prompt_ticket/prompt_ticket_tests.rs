//! prompt_ticket 单元测试（2026-11-13）：发/消费/单次/过期/错配五臂 +
//! 容量钳制。纯内存 + 注入 Instant，零 IO 零等待（过期臂用 now+TTL 移针）。
//!
//! ⚠ store 是进程级 OnceLock——full_table 臂会填满共享表，并发跑会挤兑
//! 其他用例的 issue（cargo test 默认多线程）。模块级 TEST_LOCK 全串行
//! （background_registry 同款纪律）。

use super::*;
use parking_lot::Mutex as PlMutex;
use std::time::{Duration, Instant};

static TEST_LOCK: PlMutex<()> = PlMutex::new(());

#[test]
fn issue_then_consume_ok() {
    let _serial = TEST_LOCK.lock();
    let now = Instant::now();
    let t = issue("t1", "agent:main:session:s1", "tools", now);
    assert_eq!(t.ttl_secs, PROMPT_TICKET_TTL_SECS);
    assert!(consume(&t.ticket, "agent:main:session:s1", "tools", now).is_ok());
}

#[test]
fn consume_is_single_use() {
    let _serial = TEST_LOCK.lock();
    let now = Instant::now();
    let t = issue("t2", "agent:main:session:s1", "tools", now);
    assert!(consume(&t.ticket, "agent:main:session:s1", "tools", now).is_ok());
    // 第二次（刷新重放）= 已消费 → 诚实报错
    assert!(consume(&t.ticket, "agent:main:session:s1", "tools", now).is_err());
}

#[test]
fn consume_unknown_ticket_err() {
    let _serial = TEST_LOCK.lock();
    let now = Instant::now();
    assert!(consume("never-issued", "agent:main:session:s1", "tools", now).is_err());
}

#[test]
fn expired_ticket_rejected() {
    let _serial = TEST_LOCK.lock();
    let now = Instant::now();
    let t = issue("t3", "agent:main:session:s1", "history", now);
    // 移针到 TTL 之后（零等待）
    let later = now + Duration::from_secs(PROMPT_TICKET_TTL_SECS);
    assert!(consume(&t.ticket, "agent:main:session:s1", "history", later).is_err());
}

#[test]
fn mismatched_binding_rejected() {
    let _serial = TEST_LOCK.lock();
    let now = Instant::now();
    let t = issue("t4", "agent:main:session:s1", "tools", now);
    // 同票换段
    assert!(consume(&t.ticket, "agent:main:session:s1", "system", now).is_err());
    // 同票换会话
    assert!(consume(&t.ticket, "agent:main:session:s2", "tools", now).is_err());
}

#[test]
fn expired_entries_swept_on_issue() {
    let _serial = TEST_LOCK.lock();
    let now = Instant::now();
    issue("t5", "agent:main:session:s1", "tools", now);
    let later = now + Duration::from_secs(PROMPT_TICKET_TTL_SECS + 1);
    // 在 later 发新票：过期 t5 被清扫（表里只剩新票）
    let _ = issue("t6", "agent:main:session:s1", "tools", later);
    assert!(consume("t5", "agent:main:session:s1", "tools", later).is_err());
}

#[test]
fn full_table_rejects_issue_but_keeps_old_valid() {
    let _serial = TEST_LOCK.lock();
    store().lock().clear();
    let now = Instant::now();
    // 预填一张有效票
    let kept = issue("kept", "agent:main:session:s1", "tools", now);
    for i in 0..MAX_TICKETS {
        let _ = issue(&format!("fill-{i}"), "agent:main:session:sX", "tools", now);
    }
    let overflow = issue("overflow", "agent:main:session:s1", "tools", now);
    // 满表：新票拒收（consume 报「已失效」）
    assert!(consume(&overflow.ticket, "agent:main:session:s1", "tools", now).is_err());
    // 先入的有效票不受挤兑
    assert!(consume(&kept.ticket, "agent:main:session:s1", "tools", now).is_ok());
    // 收尾清空，防污染同进程后续测试
    store().lock().clear();
}
