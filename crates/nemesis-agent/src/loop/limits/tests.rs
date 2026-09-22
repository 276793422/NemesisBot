//! P0 vault（D2）：limits 机制测试（滑动窗口、缺省全关、升级计数语义）。

use super::*;
use parking_lot::Mutex as PlMutex;

/// 全局规则/计数器是进程单例——测试互斥 + 全量清理。
static LIMITS_LOCK: PlMutex<()> = PlMutex::new(());

fn rule(max: u32, window_secs: u64) -> std::collections::BTreeMap<String, LimitRule> {
    let mut m = std::collections::BTreeMap::new();
    m.insert("test_cat".to_string(), LimitRule { max, window_secs });
    m
}

/// 未配置类别 = 不限（缺省全关）。
#[test]
fn unconfigured_category_unlimited() {
    let _g = LIMITS_LOCK.lock();
    clear_all();
    let cats = vec!["never_configured".to_string()];
    for _ in 0..100 {
        assert!(check_and_record(&cats).is_none());
    }
}

/// 无类别声明（大多数工具）零开销直通。
#[test]
fn empty_categories_free() {
    let _g = LIMITS_LOCK.lock();
    set_rules(rule(1, 3600));
    assert!(check_and_record(&[]).is_none());
}

/// 窗口内第 max+1 次超限（超限那次不计数——放行与否归升级结果管）。
#[test]
fn window_enforces_max() {
    let _g = LIMITS_LOCK.lock();
    clear_all();
    set_rules(rule(3, 3600));
    let cats = vec!["test_cat".to_string()];
    assert!(check_and_record(&cats).is_none());
    assert!(check_and_record(&cats).is_none());
    assert!(check_and_record(&cats).is_none());
    let over = check_and_record(&cats).expect("第 4 次应超限");
    assert_eq!(over.category, "test_cat");
    assert_eq!(over.count, 3);
    assert_eq!(over.max, 3);
    // 超限那次不计入——再查仍 3 次。
    assert_eq!(check_limit(&cats).unwrap().count, 3);
    let msg = over.denial_message();
    assert!(msg.contains("RATE LIMIT EXCEEDED"), "{msg}");
    assert!(msg.contains("security.limits"), "{msg}");
}

/// 窗口滑动：出窗项淘汰后恢复配额（用极短窗口真实等待 20ms）。
#[test]
fn sliding_window_expires() {
    let _g = LIMITS_LOCK.lock();
    clear_all();
    set_rules(rule(1, 0)); // window_secs=0 → 立即出窗（>0ms 即过期）
    let cats = vec!["test_cat".to_string()];
    assert!(check_and_record(&cats).is_none());
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(check_and_record(&cats).is_none(), "出窗后应恢复配额");
}

/// 升级获批路径：超限后显式 record 再查——计数含批准的那次（后续仍受限）。
#[test]
fn explicit_record_after_escalation_counts() {
    let _g = LIMITS_LOCK.lock();
    clear_all();
    set_rules(rule(2, 3600));
    let cats = vec!["test_cat".to_string()];
    assert!(check_and_record(&cats).is_none());
    assert!(check_and_record(&cats).is_none());
    assert!(check_and_record(&cats).is_some(), "第 3 次超限");
    // 模拟人工批准：显式计数。
    record(&cats);
    assert!(
        check_limit(&cats).is_some(),
        "批准后的调用计入配额，下一次仍超限"
    );
}

/// 多类别声明：任一类别超限即拦（fail-closed 取最先命中的类别）。
#[test]
fn any_category_over_limit_blocks() {
    let _g = LIMITS_LOCK.lock();
    clear_all();
    let mut m = std::collections::BTreeMap::new();
    m.insert(
        "a".to_string(),
        LimitRule {
            max: 5,
            window_secs: 3600,
        },
    );
    m.insert(
        "b".to_string(),
        LimitRule {
            max: 1,
            window_secs: 3600,
        },
    );
    set_rules(m);
    let cats = vec!["a".to_string(), "b".to_string()];
    assert!(check_and_record(&cats).is_none());
    let over = check_and_record(&cats).expect("b 类别应超限");
    assert_eq!(over.category, "b");
}

/// TOCTOU 回归锁：check 与 record 必须同一临界区——并发调用放行总数不得
/// 超过 max（旧实现分离两次加锁，并发下窗口计数可超出 max）。
#[test]
fn concurrent_check_and_record_never_exceeds_max() {
    let _g = LIMITS_LOCK.lock();
    clear_all();
    const MAX: u32 = 8;
    const THREADS: usize = 32;
    set_rules(rule(MAX, 3600));
    let cats = std::sync::Arc::new(vec!["test_cat".to_string()]);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let allowed = std::sync::Arc::new(PlMutex::new(0u32));
    let mut handles = Vec::new();
    for _ in 0..THREADS {
        let cats = cats.clone();
        let barrier = barrier.clone();
        let allowed = allowed.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            if check_and_record(&cats).is_none() {
                *allowed.lock() += 1;
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(*allowed.lock(), MAX, "放行总数必须恰好 = max");
    assert_eq!(check_limit(&cats).unwrap().count, MAX);
}

/// 无规则类别不记数：不限额类别不得在计数器里留下无界增长的死条目
/// （检查侧从不淘汰无规则类别的队列，记了就是常驻进程的内存泄漏）。
#[test]
fn ruleless_categories_are_not_recorded() {
    let _g = LIMITS_LOCK.lock();
    clear_all();
    set_rules(rule(5, 3600));
    let cats = vec!["test_cat".to_string(), "no_rule_category".to_string()];
    assert!(check_and_record(&cats).is_none());
    record(&["no_rule_category".to_string()]);
    assert!(
        !COUNTERS.lock().contains_key("no_rule_category"),
        "无规则类别不得入账"
    );
    assert_eq!(COUNTERS.lock().get("test_cat").map(|q| q.len()), Some(1));
}
