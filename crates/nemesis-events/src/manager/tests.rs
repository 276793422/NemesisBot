use super::*;
use crate::rules::{DeliverSpec, EventRule, WatchSpec};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn rule(id: &str, root: &str) -> EventRule {
    EventRule {
        id: id.to_string(),
        watch: WatchSpec {
            kind: "fs".to_string(),
            path: PathBuf::from(root),
            glob: None,
            debounce_ms: None,
        },
        deliver: DeliverSpec {
            session_key: "agent:main:session:t".to_string(),
            prompt: "变化 {paths} 共 {count}".to_string(),
            cooldown_secs: None,
            merge_window_secs: None,
            max_per_hour: None,
            enabled: None,
        },
    }
}

fn rule_with(id: &str, root: &str, cooldown: u64, merge: u64, per_hour: u32) -> EventRule {
    let mut r = rule(id, root);
    r.deliver.cooldown_secs = Some(cooldown);
    r.deliver.merge_window_secs = Some(merge);
    r.deliver.max_per_hour = Some(per_hour);
    r
}

fn ev(path: &str, at: u64) -> RawFsEvent {
    RawFsEvent {
        path: path.to_string(),
        at_ms: at,
    }
}

fn ok_injector() -> (
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<String>>>,
    impl Fn(&str, &str) -> bool,
) {
    let hits = Arc::new(AtomicUsize::new(0));
    let contents = Arc::new(Mutex::new(Vec::new()));
    let (h, c) = (hits.clone(), contents.clone());
    let f = move |_session: &str, content: &str| -> bool {
        h.fetch_add(1, Ordering::SeqCst);
        c.lock().unwrap().push(content.to_string());
        true
    };
    (hits, contents, f)
}

#[test]
fn ingest_matches_rules_and_run_cycle_delivers_merged_burst() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![rule_with("r1", "C:/logs", 0, 0, 0)]; // merge=0 → 立即到期
    st.ingest(ev("C:/logs/a.txt", 1000));
    st.ingest(ev("C:/logs/b.txt", 1100));
    st.ingest(ev("C:/other/c.txt", 1200)); // 不匹配
    assert!(st.pending.contains_key("r1"));
    assert_eq!(st.pending["r1"].count(), 2);
    let (hits, contents, inj) = ok_injector();
    st.run_cycle(1200, false, &inj);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let msg = contents.lock().unwrap()[0].clone();
    assert!(msg.starts_with("[事件:r1] "));
    assert!(msg.contains("共 2"));
    assert!(msg.contains("a.txt"));
    assert_eq!(st.stats.delivered_messages, 1);
    assert_eq!(st.stats.delivered_events, 2);
}

#[test]
fn merge_window_holds_until_quiet() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![rule_with("r1", "C:/logs", 0, 5, 0)];
    st.ingest(ev("C:/logs/a.txt", 1000));
    let (hits, _c, inj) = ok_injector();
    st.run_cycle(1000 + 5_000 - 1, false, &inj);
    assert_eq!(hits.load(Ordering::SeqCst), 0); // 静默期未满
    st.run_cycle(1000 + 5_000, false, &inj);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn cooldown_drops_bursts_within_window_then_recovers() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![rule_with("r1", "C:/logs", 60, 0, 0)];
    let (hits, _c, inj) = ok_injector();
    st.ingest(ev("C:/logs/a.txt", 0));
    st.run_cycle(0, false, &inj); // 首投
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    st.ingest(ev("C:/logs/b.txt", 59_000));
    st.run_cycle(59_000, false, &inj); // 冷却内 → 丢
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(st.stats.dropped_cooldown, 1);
    st.ingest(ev("C:/logs/c.txt", 60_000));
    st.run_cycle(60_000, false, &inj); // 恰满冷却 → 投
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[test]
fn per_rule_hourly_cap_blocks_overflow() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![rule_with("r1", "C:/logs", 0, 0, 1)];
    let (hits, _c, inj) = ok_injector();
    st.ingest(ev("C:/logs/a.txt", 0));
    st.run_cycle(0, false, &inj);
    st.ingest(ev("C:/logs/b.txt", 1000));
    st.run_cycle(1000, false, &inj); // 本小时第 2 条 → 丢
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(st.stats.dropped_hourly, 1);
    // 1h 后滑出 → 恢复
    st.ingest(ev("C:/logs/c.txt", 3_600_000));
    st.run_cycle(3_600_000, false, &inj);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[test]
fn global_hourly_cap_blocks_across_rules() {
    let mut st = ManagerState::new(1); // 全局上限 1/h
    st.rules = vec![
        rule_with("r1", "C:/a", 0, 0, 0),
        rule_with("r2", "C:/b", 0, 0, 0),
    ];
    let (hits, _c, inj) = ok_injector();
    st.ingest(ev("C:/a/x", 0));
    st.run_cycle(0, false, &inj);
    st.ingest(ev("C:/b/y", 1000));
    st.run_cycle(1000, false, &inj); // 全局满 → 丢
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(st.stats.dropped_hourly, 1);
}

#[test]
fn estop_freeze_drops_pending_and_release_recovers() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![rule_with("r1", "C:/logs", 0, 0, 0)]; // merge=0 → 到期即可投
    let (hits, _c, inj) = ok_injector();
    st.ingest(ev("C:/logs/a.txt", 0));
    st.ingest(ev("C:/logs/b.txt", 100));
    st.run_cycle(200, true, &inj); // 冻结 → 全丢
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(st.stats.dropped_estop, 2);
    assert!(st.pending.is_empty());
    st.ingest(ev("C:/logs/c.txt", 300));
    st.run_cycle(300, false, &inj); // release → 恢复积累投递
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(st.stats.delivered_events, 1);
}

#[test]
fn delivery_failure_counts_and_does_not_consume_quota() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![rule_with("r1", "C:/logs", 60, 0, 0)];
    let misses = Arc::new(AtomicUsize::new(0));
    let miss_inj = {
        let m = misses.clone();
        move |_s: &str, _c: &str| -> bool {
            m.fetch_add(1, Ordering::SeqCst);
            false
        }
    };
    st.ingest(ev("C:/logs/a.txt", 0));
    st.run_cycle(0, false, &miss_inj);
    assert_eq!(misses.load(Ordering::SeqCst), 1);
    assert_eq!(st.stats.dropped_delivery, 1);
    // 失败不启动冷却：马上再来一次仍尝试（不静默吞）
    st.ingest(ev("C:/logs/b.txt", 1000));
    st.run_cycle(1000, false, &miss_inj);
    assert_eq!(misses.load(Ordering::SeqCst), 2);
}

#[test]
fn debounce_folds_editor_save_duplicates() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    let mut r = rule("r1", "C:/logs");
    r.watch.debounce_ms = Some(500);
    st.rules = vec![r];
    st.ingest(ev("C:/logs/a.txt", 0));
    st.ingest(ev("C:/logs/a.txt", 100)); // 一次保存的重复事件
    st.ingest(ev("C:/logs/a.txt", 200));
    assert_eq!(st.pending["r1"].count(), 1);
    assert_eq!(st.stats.deduped, 2);
}

#[test]
fn check_reload_applies_changes_and_rearm_roots_follow() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    crate::rules::save_rules(&path, &[rule("r1", "C:/logs")]).unwrap();
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.load_rules(&path);
    assert_eq!(st.rules.len(), 1);
    assert_eq!(st.watch_roots().len(), 1);
    // 同 mtime → 不重载
    assert!(!st.check_reload(&path));
    // 规则换根 → 重载 + 新根
    std::thread::sleep(std::time::Duration::from_millis(50));
    crate::rules::save_rules(&path, &[rule("r1", "C:/other")]).unwrap();
    assert!(st.check_reload(&path));
    assert_eq!(
        st.watch_roots().iter().next().unwrap(),
        &PathBuf::from("C:/other")
    );
}

#[test]
fn check_reload_keeps_old_rules_on_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    crate::rules::save_rules(&path, &[rule("r1", "C:/logs")]).unwrap();
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.load_rules(&path);
    assert_eq!(st.rules.len(), 1);
    // 半写文件（解析失败）→ 保留旧规则
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&path, "[{broken").unwrap();
    assert!(st.check_reload(&path));
    assert_eq!(st.rules.len(), 1, "解析失败必须保留旧规则，不清空在监听集");
    // 规则删除（文件不存在）→ 空规则集
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::remove_file(&path).unwrap();
    assert!(st.check_reload(&path));
    assert!(st.rules.is_empty());
}

#[test]
fn reload_drops_pending_of_removed_rules() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    crate::rules::save_rules(&path, &[rule("r1", "C:/logs"), rule("r2", "C:/other")]).unwrap();
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.load_rules(&path);
    st.ingest(ev("C:/logs/a.txt", 0));
    st.ingest(ev("C:/other/b.txt", 0));
    std::thread::sleep(std::time::Duration::from_millis(50));
    crate::rules::save_rules(&path, &[rule("r1", "C:/logs")]).unwrap(); // r2 消失
    st.check_reload(&path);
    assert!(st.pending.contains_key("r1"));
    assert!(!st.pending.contains_key("r2"));
    assert_eq!(st.stats.dropped_reload, 1);
}

#[test]
fn disabled_rule_ingests_but_never_delivers() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    let mut r = rule("r1", "C:/logs");
    r.deliver.enabled = Some(false);
    st.rules = vec![r];
    let (hits, _c, inj) = ok_injector();
    st.ingest(ev("C:/logs/a.txt", 0));
    st.run_cycle(0, false, &inj); // enabled=false → run_cycle 不挑它
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    // buffer 还在 pending（未被丢弃）——重新 enabled 后可投
    assert!(st.pending.contains_key("r1"));
}

// ── P2 观测快照（EventsView 消费面）─────────────────────────────────

#[test]
fn snapshot_expands_defaults_and_counts_pending() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    // 规则带显式值；另一条全缺省（RuleView 必须展开缺省而非 None）。
    let mut custom = rule_with("custom", "C:/logs", 30, 5, 10);
    custom.deliver.enabled = Some(false);
    st.rules = vec![custom, rule("bare", "C:/other")];
    st.ingest(ev("C:/logs/a.txt", 0));
    st.ingest(ev("C:/logs/b.txt", 1));
    let snap = st.snapshot();
    assert_eq!(snap.rules.len(), 2);
    let custom = snap.rules.iter().find(|r| r.id == "custom").unwrap();
    assert_eq!(custom.cooldown_secs, 30);
    assert_eq!(custom.merge_window_secs, 5);
    assert_eq!(custom.max_per_hour, 10);
    assert!(!custom.enabled);
    let bare = snap.rules.iter().find(|r| r.id == "bare").unwrap();
    assert_eq!(bare.cooldown_secs, 60); // DEFAULT_COOLDOWN_SECS
    assert_eq!(bare.merge_window_secs, 5); // DEFAULT_MERGE_WINDOW_SECS
    assert_eq!(bare.max_per_hour, 10); // DEFAULT_MAX_PER_HOUR
    assert!(bare.enabled);
    // pending：单规则双路径去重窗折叠前 = 2 个事件在攒。
    let p = snap.pending.iter().find(|p| p.rule_id == "custom").unwrap();
    assert_eq!(p.count, 2);
    // stats 直通。
    assert_eq!(snap.stats.delivered_events, 0);
}

#[test]
fn snapshot_handle_shared_arc_receives_pump_writes() {
    // EventWatchManager::new 即建句柄——gateway 可在 start 前注入 web 面；
    // start 后泵每周期覆写（这里不真拉泵，验证句柄共享同一 Arc 即可——
    // 泵写路径在 e2e event_wakeup 全链覆盖）。
    let mgr = EventWatchManager::new(
        PathBuf::from("unused.json"),
        Arc::new(|_, _| false),
        Arc::new(|| false),
    );
    let h1 = mgr.snapshot_handle();
    let h2 = mgr.snapshot_handle();
    assert!(Arc::ptr_eq(&h1, &h2), "两次取句柄必须同一 Arc");
    *h1.write() = EventSnapshot::default();
    assert_eq!(h2.read().stats.delivered_events, 0);
}
