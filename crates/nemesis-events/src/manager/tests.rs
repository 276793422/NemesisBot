use super::*;
use crate::rules::{DeliverSpec, EventRule, WatchSpec};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn rule(id: &str, root: &str) -> EventRule {
    EventRule {
        id: id.to_string(),
        watch: WatchSpec {
            kind: "fs".to_string(),
            path: PathBuf::from(root),
            ..Default::default()
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

/// webhook/probe/sys 规则的 watch.path 恒空（非 fs 源无目录可盯）——
/// watch_roots 不得把空根喂给 notify rearm（空串 watch 必然 Err，
/// 假故障 WARN；修复回归锁 2026-11-16）。
#[test]
fn watch_roots_filters_empty_paths_from_non_fs_rules() {
    let mut wh = rule("wh1", "");
    wh.watch.kind = "webhook".to_string();
    wh.watch.token = Some("tok".to_string());
    let mut pr = rule("pr1", "");
    pr.watch.kind = "probe".to_string();
    let fs = rule("fs1", "C:/logs");
    let st = ManagerState {
        rules: vec![wh, pr, fs],
        ..ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP)
    };
    let roots = st.watch_roots();
    assert_eq!(roots.len(), 1, "空根必须剔除");
    assert_eq!(roots.iter().next().unwrap(), &PathBuf::from("C:/logs"));
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

/// BUG-20261117-063 回归锁：坏文件重载失败后必须钉住 mtime 游标——
/// 此前 Err 臂不钉，同一坏文件持续在盘期间每个泵 tick（250ms）都
/// stat+read+parse+warn 一轮（4Hz 无限重试 + warn 洪水）。
/// 修复语义：第一次 check_reload 尝试并失败（返回 true），此后同 mtime
/// 不再重试（返回 false）；文件修复（新 mtime）后恢复重载。
#[test]
fn check_reload_failure_pins_mtime_no_infinite_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    // 启动即坏文件：startup load_rules 失败（游标仍是 None）
    std::fs::write(&path, "[{broken").unwrap();
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.load_rules(&path);
    assert!(st.rules.is_empty());
    // 第一个泵 tick：尝试重载（仍失败 → true，且钉住游标）
    assert!(st.check_reload(&path));
    assert!(st.rules_mtime.is_some(), "失败臂之后游标必须被钉住");
    // 同一坏文件持续在盘（修复前这里每个 tick 都返回 true）：
    // 不再重试——连续两个 tick 均为 false
    assert!(!st.check_reload(&path), "同 mtime 失败后不得再次重试");
    assert!(!st.check_reload(&path));
    assert!(st.rules.is_empty());
    // 文件修复（新 mtime）→ 恢复重载成功
    std::thread::sleep(std::time::Duration::from_millis(50));
    crate::rules::save_rules(&path, &[rule("r1", "C:/logs")]).unwrap();
    assert!(st.check_reload(&path));
    assert_eq!(st.rules.len(), 1, "修复后必须恢复正常重载");
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

// ── 事件源扩展：webhook / proc / sys 泵逻辑 ─────────────────────────

fn wh_rule(id: &str, token: &str) -> EventRule {
    EventRule {
        id: id.to_string(),
        watch: WatchSpec {
            kind: "webhook".to_string(),
            token: Some(token.to_string()),
            ..Default::default()
        },
        deliver: DeliverSpec {
            session_key: "agent:main:session:t".to_string(),
            prompt: "外部推送：{payload}".to_string(),
            cooldown_secs: None,
            merge_window_secs: None,
            max_per_hour: None,
            enabled: None,
        },
    }
}

fn proc_rule(id: &str, job_id: u64, pattern: &str, notify_start: bool) -> EventRule {
    EventRule {
        id: id.to_string(),
        watch: WatchSpec {
            kind: "proc".to_string(),
            job_id: Some(job_id),
            pattern: Some(pattern.to_string()),
            notify_start: Some(notify_start),
            ..Default::default()
        },
        deliver: DeliverSpec {
            session_key: "agent:main:session:t".to_string(),
            prompt: "任务 #{job_id} 命中：{line}（{count}）".to_string(),
            cooldown_secs: None,
            merge_window_secs: None,
            max_per_hour: None,
            enabled: None,
        },
    }
}

fn sys_rule(id: &str, metric: &str, above: f64, below: f64) -> EventRule {
    EventRule {
        id: id.to_string(),
        watch: WatchSpec {
            kind: "sys".to_string(),
            metric: Some(metric.to_string()),
            above: Some(above),
            below: Some(below),
            ..Default::default()
        },
        deliver: DeliverSpec {
            session_key: "agent:main:session:t".to_string(),
            prompt: "系统告警：{payload}".to_string(),
            cooldown_secs: None,
            merge_window_secs: None,
            max_per_hour: None,
            enabled: None,
        },
    }
}

#[tokio::test]
async fn webhook_matches_token_and_renders_payload() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![wh_rule("wh1", "s3cret"), wh_rule("wh2", "other")];
    let matched = st.ingest_webhook("s3cret", "deploy done", 0);
    assert_eq!(matched, vec!["wh1".to_string()]);
    assert_eq!(st.stats.webhook_received, 1);
    assert_eq!(st.stats.webhook_unmatched, 0);
    assert_eq!(st.pending["wh1"].count(), 1);
    assert!(!st.pending.contains_key("wh2"));
    // 相同载荷是不同事件（无去重折叠）
    st.ingest_webhook("s3cret", "deploy done", 100);
    assert_eq!(st.pending["wh1"].count(), 2);
    // 未匹配 token → 诚实计数 + 空列表（HTTP 面 404）
    let none = st.ingest_webhook("nope", "x", 200);
    assert!(none.is_empty());
    assert_eq!(st.stats.webhook_unmatched, 1);
    // 渲染 {payload}
    let (hits, contents, inj) = ok_injector();
    st.rules[0].deliver.merge_window_secs = Some(0);
    st.run_cycle(200, false, &inj);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let msg = contents.lock().unwrap()[0].clone();
    assert!(msg.starts_with("[事件:wh1] "));
    assert!(msg.contains("deploy done"));
}

#[tokio::test]
async fn webhook_payload_oversize_truncated() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![wh_rule("wh1", "tk")];
    let big = "x".repeat(WEBHOOK_PAYLOAD_MAX + 500);
    st.ingest_webhook("tk", &big, 0);
    let p = st.pending["wh1"].paths()[0].clone();
    assert!(p.len() < big.len(), "载荷必须截断");
    assert!(p.contains("已截断"));
    assert!(p.contains("…["));
}

// --- BUG-20261124-010：webhook token 匹配换常量时间形态的行为等价锁 --------
//
// 规则匹配从裸 `==` 换 verify_token（公开端点 token 即凭据，时序 oracle
// 纵深防御补齐；webhook_inbound api_key / share resolve_share 同域先例）。
// 本测试锁行为等价：命中/不命中判定与 `==` 逐字节一致（含同长度不同内容
// 形态），且空串 token 规则在 verify_token「空 expected = 不设闸」语义下
// 不退化成万能通行（`!t.is_empty()` 前置闸）。

#[tokio::test]
async fn webhook_token_match_is_byte_exact_under_constant_time() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![wh_rule("wh1", "s3cret"), wh_rule("wh2", "t4cret")];
    // 精确命中
    let hit = st.ingest_webhook("s3cret", "p", 0);
    assert_eq!(hit, vec!["wh1".to_string()]);
    // 同长度不同内容（== 与 verify_token 必须同样判不命中——常量时间形态
    // 不因长度相等放宽；"a3cret" 与 wh1 同长、与 wh2 不同内容）
    let miss = st.ingest_webhook("a3cret", "p", 10);
    assert!(miss.is_empty());
    assert_eq!(st.stats.webhook_unmatched, 1);
    // 不同长度 + 前缀形态
    let miss2 = st.ingest_webhook("s3cre", "p", 20);
    assert!(miss2.is_empty());
    // 空串规则不认领任意 token（verify_token 空 expected = true 语义被
    // 前置闸拦住）
    let mut st2 = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st2.rules = vec![wh_rule("wh_empty", "")];
    let none = st2.ingest_webhook("anything", "p", 0);
    assert!(none.is_empty(), "empty-token rule must not claim any token");
    assert_eq!(st2.stats.webhook_unmatched, 1);
}

#[test]
fn truncate_annotated_reports_actual_retained_bytes() {
    // BUG-20261121-034：注记报实际保留字节数 cut，不报上限 max——多字节
    // 字符跨界回退后两者可差 1-3 字节，报 max 是谎报（BUG-20261118-165
    // 「注记数字必须为真」同纪律）。
    // 4 字节字符跨 max=1024 边界：1021 个 ASCII + 2 个 4 字节字符（1029B）
    // ——1024 落在首字符（1021..1025）内部，cut 必须回退到 1021。
    let s = format!("{}{}", "x".repeat(1021), "𩸽𩸽");
    assert_eq!(s.len(), 1029);
    let out = truncate_annotated(&s, 1024);
    let note = out
        .split("…[已截断 ")
        .nth(1)
        .expect("截断注记在场")
        .to_string();
    let arrow = note.split("→").nth(1).expect("→N B 形态").to_string();
    let claimed: usize = arrow
        .trim_end_matches("B]")
        .parse()
        .expect("注记数字可解析");
    let prefix_len = out.find('…').expect("省略号在场");
    assert_eq!(
        claimed, prefix_len,
        "注记保留字节数必须等于实际前缀字节数（修复前报 max=1024，实际保留 <1024）"
    );
    // 纯 ASCII 截断：cut == max，注记数字 == 上限（行为不变面）
    let ascii_out = truncate_annotated(&"x".repeat(2000), 1024);
    assert!(ascii_out.contains("→1024B]"));
    assert!(ascii_out.starts_with(&"x".repeat(1024)));
}

#[tokio::test]
async fn proc_matches_lines_and_renders_line_job_id() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![proc_rule("pr1", 7, "(?i)error", false)];
    let reader: ProcReaderFn = Arc::new(|_job, offset| {
        Box::pin(async move {
            Ok(serde_json::json!({
                "chunk": "line1\nERROR boom\nline3\n",
                "next_offset": offset + 21u64,
                "running": true
            }))
        })
    });
    st.sample_procs(Some(&reader), 1000).await;
    // running 任务：末尾空段留 tail；命中 1 行
    assert_eq!(st.pending["pr1"].count(), 1);
    assert_eq!(st.stats.proc_lines_matched, 1);
    let (hits, contents, inj) = ok_injector();
    st.rules[0].deliver.merge_window_secs = Some(0);
    st.run_cycle(1000, false, &inj);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let msg = contents.lock().unwrap()[0].clone();
    assert!(msg.contains("任务 #7 命中：ERROR boom（1）"));
}

#[tokio::test]
async fn proc_tail_carries_partial_line_across_reads() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![proc_rule("pr1", 7, "ERROR", false)];
    let reads = Arc::new(Mutex::new(Vec::new()));
    let reads2 = reads.clone();
    let reader: ProcReaderFn = Arc::new(move |_job, offset| {
        reads2.lock().unwrap().push(offset);
        let chunk = if offset == 0 { "hel" } else { "lo ERROR\n" };
        Box::pin(async move {
            Ok(serde_json::json!({
                "chunk": chunk,
                "next_offset": offset + chunk.len() as u64,
                "running": true
            }))
        })
    });
    st.sample_procs(Some(&reader), 0).await;
    assert_eq!(
        st.pending.get("pr1").map(|b| b.count()).unwrap_or(0),
        0,
        "半行不应命中（pending 可能尚未建 buffer）"
    );
    st.sample_procs(Some(&reader), 250).await;
    assert_eq!(st.pending["pr1"].count(), 1, "拼接后命中");
    assert_eq!(st.pending["pr1"].paths()[0], "hello ERROR");
    assert_eq!(
        reads.lock().unwrap().clone(),
        vec![0u64, 3],
        "offset 坐标单调推进（0 → 3）"
    );
}

#[tokio::test]
async fn proc_notify_start_fires_once_on_first_running_observation() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![proc_rule("pr1", 9, "(?i)error", true)];
    let reader: ProcReaderFn = Arc::new(|_job, _offset| {
        Box::pin(async move {
            Ok(serde_json::json!({
                "chunk": "",
                "next_offset": 0u64,
                "running": true
            }))
        })
    });
    st.sample_procs(Some(&reader), 0).await;
    let payloads = st.pending["pr1"].paths();
    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0], "[后台任务 #9 已启动]");
    // 第二次采样：seen_running 已置位 → 不重复启动通知
    st.sample_procs(Some(&reader), 1000).await;
    assert_eq!(st.pending["pr1"].count(), 1);
}

#[tokio::test]
async fn proc_read_error_marks_rule_idle_with_one_warn() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![proc_rule("pr1", 99, "x", false)];
    let reader: ProcReaderFn =
        Arc::new(|_job, _offset| Box::pin(async { Err("job not found".to_string()) }));
    st.sample_procs(Some(&reader), 0).await;
    st.sample_procs(Some(&reader), 1000).await;
    assert_eq!(st.stats.proc_read_errors, 2);
    assert!(st.pending.is_empty());
    assert!(st.proc_warned.contains("pr1"), "同规则只 warn 一次");
}

#[tokio::test]
async fn proc_without_reader_honest_noop() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![proc_rule("pr1", 7, "x", false)];
    st.sample_procs(None, 0).await;
    assert!(st.pending.is_empty());
    assert_eq!(st.stats.proc_read_errors, 0);
}

#[test]
fn sys_hysteresis_trigger_quiet_fall_and_retrigger() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![sys_rule("sy1", "cpu", 90.0, 80.0)];
    let sample = |cpu: f32| SysSample {
        cpu,
        mem_percent: 0.0,
        disks: vec![],
    };
    // 首轮基态落账（80 未过阈 → normal，无 pending）
    st.sample_sys(&sample(80.0), 0);
    assert!(st.pending.is_empty());
    assert_eq!(st.stats.sys_triggered, 0);
    // 过阈 → 触发一次
    st.sample_sys(&sample(95.0), 1000);
    assert_eq!(st.pending["sy1"].count(), 1);
    assert_eq!(st.stats.sys_triggered, 1);
    // firing 期 85（below ≤ v < above）→ 静默不重复
    st.sample_sys(&sample(85.0), 2000);
    assert_eq!(st.pending["sy1"].count(), 1);
    assert_eq!(st.stats.sys_triggered, 1);
    // 回落 < below → normal（不推）
    st.sample_sys(&sample(79.0), 3000);
    assert_eq!(st.pending["sy1"].count(), 1);
    // 再过阈 → 第二次触发
    st.sample_sys(&sample(91.0), 4000);
    assert_eq!(st.stats.sys_triggered, 2);
}

#[test]
fn sys_low_threshold_default_below_recovers_and_refires() {
    // BUG-20261121-033 行为面：above=8（≤10）旧缺省 below = (8-10).max(0)=0
    // →「value < 0」对使用率口径不可达，首次触发后永久卡 firing、永不
    // 再触发。缺省解析改 above 一半（=4）后滞回可回落、可再触发。
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    let mut r = sys_rule("sylow", "cpu", 8.0, 4.0);
    r.watch.below = None; // 走缺省臂：sys_below_or_default() = above/2 = 4.0
    st.rules = vec![r];
    let sample = |cpu: f32| SysSample {
        cpu,
        mem_percent: 0.0,
        disks: vec![],
    };
    // ≥8 → 触发
    st.sample_sys(&sample(30.0), 0);
    assert_eq!(st.stats.sys_triggered, 1);
    // firing 期 10（≥ 缺省 below 4）→ 维持 firing 不重复推
    st.sample_sys(&sample(10.0), 1000);
    assert_eq!(st.stats.sys_triggered, 1);
    // 3 < 4 → 回 normal（静默）
    st.sample_sys(&sample(3.0), 2000);
    // 再次 ≥8 → 第二次触发（修复前：below=0 时此臂永不发生，规则事实死亡）
    st.sample_sys(&sample(30.0), 3000);
    assert_eq!(st.stats.sys_triggered, 2);
}

#[test]
fn sys_mem_and_disk_metrics_resolve() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    let mut disk = sys_rule("syd", "disk", 50.0, 40.0);
    disk.watch.path = PathBuf::from("/data");
    st.rules = vec![sys_rule("sym", "mem", 90.0, 80.0), disk];
    // /data 40% used（< above 50 不触发）；/home 90% used。
    let sample = SysSample {
        cpu: 0.0,
        mem_percent: 95.0,
        disks: vec![
            (PathBuf::from("/data"), 600, 1000),
            (PathBuf::from("/home"), 100, 1000),
        ],
    };
    st.sample_sys(&sample, 0);
    // mem 过阈触发；disk 挂载点 /data 使用率 40 未过阈 → 不触发
    assert_eq!(st.pending["sym"].count(), 1);
    assert!(!st.pending.contains_key("syd"));
    // disk 无 path 时 = 任一盘最高使用率 → /home 90 过阈触发
    let mut st2 = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st2.rules = vec![sys_rule("syd", "disk", 80.0, 70.0)];
    st2.sample_sys(&sample, 0);
    assert_eq!(st2.pending["syd"].count(), 1);
    // 滞回：50% 触发 → 39%（< below 40）回 normal → 50% 再触发
    let mut st3 = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    let mut d3 = sys_rule("syd", "disk", 50.0, 40.0);
    d3.watch.path = PathBuf::from("/data");
    st3.rules = vec![d3];
    let disk_sample = |avail: u64| SysSample {
        cpu: 0.0,
        mem_percent: 0.0,
        disks: vec![(PathBuf::from("/data"), avail, 1000)],
    };
    st3.sample_sys(&disk_sample(500), 0); // 50% ≥ above → 触发
    assert_eq!(st3.pending["syd"].count(), 1);
    st3.sample_sys(&disk_sample(610), 1000); // 39% < below → 回 normal（不推）
    assert_eq!(st3.pending["syd"].count(), 1);
    st3.sample_sys(&disk_sample(500), 2000); // 50% 再过阈 → 第二次触发
    assert_eq!(st3.stats.sys_triggered, 2);
}

#[test]
fn disk_percent_longest_prefix_mount_resolution() {
    let sample = SysSample {
        cpu: 0.0,
        mem_percent: 0.0,
        disks: vec![
            (PathBuf::from("/data"), 250, 1000),     // 75%
            (PathBuf::from("/data/sub"), 500, 1000), // 50%
            (PathBuf::from("/home"), 100, 1000),     // 90%
        ],
    };
    // None = 任一盘最高
    assert_eq!(resolve_disk_percent(&sample, None), Some(90.0));
    // 最长前缀挂载点
    assert_eq!(
        resolve_disk_percent(&sample, Some(Path::new("/data/sub/x.txt"))),
        Some(50.0)
    );
    assert_eq!(
        resolve_disk_percent(&sample, Some(Path::new("/data/other.txt"))),
        Some(75.0)
    );
    // 无匹配挂载点 → None（本轮跳过）
    assert_eq!(
        resolve_disk_percent(&sample, Some(Path::new("/var/log/x"))),
        None
    );
}

#[tokio::test]
async fn estop_drops_multi_source_pending_and_release_recovers() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![
        rule_with("r1", "C:/logs", 0, 0, 0),
        wh_rule("wh1", "tk"),
        sys_rule("sy1", "cpu", 1.0, 0.5),
    ];
    let reader: ProcReaderFn = Arc::new(|_j, _o| {
        Box::pin(async {
            Ok(serde_json::json!({"chunk": "hit\n", "next_offset": 4u64, "running": false}))
        })
    });
    st.rules.push(proc_rule("pr1", 1, "hit", false));
    st.ingest(ev("C:/logs/a.txt", 0));
    st.ingest_webhook("tk", "payload", 0);
    st.sample_sys(
        &SysSample {
            cpu: 50.0,
            mem_percent: 0.0,
            disks: vec![],
        },
        0,
    );
    st.sample_procs(Some(&reader), 0).await;
    assert_eq!(st.pending.len(), 4);
    st.run_cycle(0, true, &|_, _| true); // 冻结 → 全源 pending 诚实丢弃
    assert!(st.pending.is_empty());
    assert_eq!(st.stats.dropped_estop, 4);
}

#[test]
fn snapshot_rule_view_detail_hides_webhook_token() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![
        wh_rule("wh1", "s3cret-token"),
        proc_rule("pr1", 7, "(?i)err", true),
        sys_rule("sy1", "mem", 90.0, 80.0),
    ];
    let snap = st.snapshot();
    let wh = snap.rules.iter().find(|r| r.id == "wh1").unwrap();
    assert_eq!(wh.kind, "webhook");
    assert!(!wh.detail.contains("s3cret-token"), "观测面不回显 token");
    let pr = snap.rules.iter().find(|r| r.id == "pr1").unwrap();
    assert!(pr.detail.contains("job #7"));
    assert!(pr.detail.contains("启动通知"));
    let sy = snap.rules.iter().find(|r| r.id == "sy1").unwrap();
    assert!(sy.detail.contains("mem"));
    assert!(sy.detail.contains("90%"));
}

#[tokio::test]
async fn reload_reconciles_proc_offset_and_sys_firing_state() {
    use crate::rules::save_rules;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    save_rules(&path, &[proc_rule("pr1", 7, "err", false)]).unwrap();
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.load_rules(&path);
    // 采样推进 offset
    let reader: ProcReaderFn = Arc::new(|_j, o| {
        Box::pin(async move {
            Ok(serde_json::json!({
                "chunk": "x\n", "next_offset": o + 2u64, "running": true
            }))
        })
    });
    st.sample_procs(Some(&reader), 0).await;
    assert_eq!(st.proc_state["pr1"].offset, 2);
    // 同 watch 段热重载 → offset 保留（不重扫历史）
    std::thread::sleep(std::time::Duration::from_millis(50));
    save_rules(&path, &[proc_rule("pr1", 7, "err", false)]).unwrap();
    assert!(st.check_reload(&path));
    assert_eq!(st.proc_state["pr1"].offset, 2, "watch 段未变 offset 不回退");
    // pattern 变更 → 状态丢弃（新 offset 从 0 重扫）
    std::thread::sleep(std::time::Duration::from_millis(50));
    save_rules(&path, &[proc_rule("pr1", 7, "err|warn", false)]).unwrap();
    assert!(st.check_reload(&path));
    assert!(!st.proc_state.contains_key("pr1"));
}

// ── 事件源扩展 P2：probe 探测源 ──────────────────────────────────────

fn probe_rule(id: &str, probe_type: &str, target: &str, interval: u64, timeout: u64) -> EventRule {
    EventRule {
        id: id.to_string(),
        watch: WatchSpec {
            kind: "probe".to_string(),
            probe_type: Some(probe_type.to_string()),
            target: Some(target.to_string()),
            interval_secs: Some(interval),
            timeout_secs: Some(timeout),
            ..Default::default()
        },
        deliver: DeliverSpec {
            session_key: "agent:main:session:t".to_string(),
            prompt: "{payload}".to_string(),
            cooldown_secs: None,
            merge_window_secs: None,
            max_per_hour: None,
            enabled: None,
        },
    }
}

fn outcome(rule_id: &str, generation: u64, up: bool, detail: &str) -> ProbeOutcome {
    ProbeOutcome {
        rule_id: rule_id.to_string(),
        generation,
        up,
        detail: detail.to_string(),
    }
}

#[test]
fn probe_baseline_silent_then_flip_triggers_and_recovers() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![probe_rule("pb1", "tcp", "127.0.0.1:1", 60, 5)];
    // 惰性建状态（代次 1；probe_checks 只建状态不排程——0 距 60s 间隔远未到期）
    let reqs = st.probe_checks(0, false);
    assert!(reqs.is_empty(), "0 时刻未到期，只落状态");
    assert_eq!(st.probe_state["pb1"].generation, 1);
    // 首探只定基态（DOWN），不触发
    st.apply_probe(outcome("pb1", 1, false, "连接失败"), 1000);
    assert!(st.pending.is_empty());
    assert_eq!(st.stats.probe_flips, 0);
    // 同态心跳静默
    st.apply_probe(outcome("pb1", 1, false, "连接失败"), 2000);
    assert_eq!(st.stats.probe_flips, 0);
    // 翻转 DOWN → UP：触发 + payload 带方向与判定依据
    st.apply_probe(outcome("pb1", 1, true, "连接成功"), 3000);
    assert_eq!(st.stats.probe_flips, 1);
    let payloads = st.pending["pb1"].paths();
    assert_eq!(payloads.len(), 1);
    assert!(payloads[0].contains("DOWN → UP"), "got: {}", payloads[0]);
    assert!(payloads[0].contains("连接成功"));
    // 同态静默
    st.apply_probe(outcome("pb1", 1, true, "连接成功"), 4000);
    assert_eq!(st.stats.probe_flips, 1);
    // 回落 UP → DOWN 再触发
    st.apply_probe(outcome("pb1", 1, false, "超时（5s）"), 5000);
    assert_eq!(st.stats.probe_flips, 2);
    assert!(st.pending["pb1"].paths()[1].contains("UP → DOWN"));
}

#[test]
fn probe_checks_respect_interval_freeze_and_enabled() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    let mut off = probe_rule("pboff", "tcp", "127.0.0.1:1", 60, 5);
    off.deliver.enabled = Some(false);
    st.rules = vec![
        probe_rule("pb1", "http", "https://a.example.com/h", 60, 10),
        off,
    ];
    // 冻结：排程暂停（状态不建，release 后重新惰性初始化）
    assert!(st.probe_checks(10_000_000, true).is_empty());
    assert!(st.probe_state.is_empty());
    // release 后首排程：last_ms=0 → 立即出发；disabled 规则不排
    let reqs = st.probe_checks(10_000_000, false);
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].rule_id, "pb1");
    assert_eq!(reqs[0].generation, 1, "首个状态代次从 1 起");
    assert_eq!(reqs[0].probe_type, "http");
    assert_eq!(reqs[0].timeout_secs, 10);
    // 周期内不重排
    assert!(st.probe_checks(10_000_000 + 30_000, false).is_empty());
    // interval 到期 → 再排（同代次——target 未变）
    let reqs = st.probe_checks(10_000_000 + 60_000, false);
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].generation, 1);
}

#[test]
fn probe_stale_generation_and_missing_rule_dropped() {
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![probe_rule("pb1", "tcp", "127.0.0.1:1", 60, 5)];
    let reqs = st.probe_checks(10_000_000, false);
    assert_eq!(reqs[0].generation, 1);
    // 基态 UP 落账
    st.apply_probe(outcome("pb1", 1, true, "连接成功"), 10_000_000);
    // 旧代次（0）在途结果 → 丢弃，基态不被打翻（防旧目标迟到结果串台）
    st.apply_probe(outcome("pb1", 0, false, "迟到的旧目标结果"), 11_000_000);
    assert_eq!(st.probe_state["pb1"].up, Some(true));
    assert_eq!(st.stats.probe_flips, 0);
    // 规则消失（热重载删除）→ 结果丢弃
    st.apply_probe(outcome("gone", 1, true, "x"), 11_000_000);
    assert!(!st.pending.contains_key("gone"));
}

#[tokio::test]
async fn tcp_probe_hits_real_listener_and_refused_port_reports_down() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (up, detail) = tcp_probe(&addr.to_string(), 5).await;
    assert!(up, "真监听端口必须 up，detail: {detail}");
    assert_eq!(detail, "连接成功");
    // 关掉监听（拒连）→ down 带根因
    drop(listener);
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let (up, detail) = tcp_probe(&addr.to_string(), 5).await;
    assert!(!up);
    assert!(detail.contains("连接失败"), "detail: {detail}");
}

#[tokio::test]
async fn http_probe_parses_status_and_reports_root_cause_on_failure() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let client = reqwest::Client::new();
    // 200 → up + "HTTP 200 OK"
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        let _ = sock
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await;
    });
    let (up, detail) = http_probe(&client, &format!("http://{addr}/health"), 5).await;
    assert!(up, "detail: {detail}");
    assert!(detail.contains("200"));
    server.await.unwrap();
    // 500 → down（非 2xx）
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        let _ = sock
            .write_all(
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\
                  Connection: close\r\n\r\n",
            )
            .await;
    });
    let (up, detail) = http_probe(&client, &format!("http://{addr}/"), 5).await;
    assert!(!up, "非 2xx 必须 down");
    assert!(detail.contains("500"), "detail: {detail}");
    server.await.unwrap();
    // 拒连 → down 带根因
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = listener.local_addr().unwrap();
    drop(listener);
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let (up, detail) = http_probe(&client, &format!("http://{dead}/"), 5).await;
    assert!(!up);
    assert!(detail.contains("请求失败"), "detail: {detail}");
}

#[tokio::test]
async fn http_probe_timeout_reports_honest_detail() {
    let client = reqwest::Client::new();
    // accept 后永不回包 → per-request timeout 兜底
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_sock, _) = listener.accept().await.unwrap();
        // 故意不读不写，挂住连接
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    });
    let (up, detail) = http_probe(&client, &format!("http://{addr}/"), 1).await;
    assert!(!up);
    assert!(detail.contains("超时（1s）"), "detail: {detail}");
    server.abort();
}

#[cfg(windows)]
#[tokio::test]
async fn command_probe_exit_code_semantics() {
    let (up, detail) = command_probe("cmd /c exit 0", 10, None).await;
    assert!(up, "退出码 0 必须 up，detail: {detail}");
    let (up, detail) = command_probe("cmd /c exit 3", 10, None).await;
    assert!(!up);
    assert!(detail.contains("退出码 3"), "detail: {detail}");
    // 启动失败（程序不存在）= down
    let (up, detail) = command_probe("definitely-not-a-real-program-xyz", 10, None).await;
    assert!(!up);
    assert!(detail.contains("启动失败"), "detail: {detail}");
}

#[cfg(not(windows))]
#[tokio::test]
async fn command_probe_exit_code_semantics() {
    let (up, detail) = command_probe("true", 10, None).await;
    assert!(up, "detail: {detail}");
    let (up, _detail) = command_probe("false", 10, None).await;
    assert!(!up, "false(1) 必须 down");
    let (up, detail) = command_probe("definitely-not-a-real-program-xyz", 10, None).await;
    assert!(!up);
    assert!(detail.contains("启动失败"), "detail: {detail}");
}

#[tokio::test]
async fn run_probe_unknown_probe_type_sends_nothing() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    run_probe(
        ProbeRequest {
            rule_id: "pb".to_string(),
            generation: 1,
            probe_type: "icmp".to_string(),
            target: "a".to_string(),
            timeout_secs: 1,
        },
        reqwest::Client::new(),
        tx,
        None,
    )
    .await;
    // 未知类型：防御分支静默返回——channel 关闭且无缓冲消息 = Ok(None)
    let got = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
    assert!(
        matches!(got, Ok(None)),
        "未知 probe_type 不得产生结果，got: {got:?}"
    );
}

#[tokio::test]
async fn pump_probe_flip_delivers_payload_end_to_end() {
    // 真监听端点：基线 UP → 关监听翻转 DOWN → 全链投递。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let rules_path = dir.path().join("events.json");
    let mut r = probe_rule("pb1", "tcp", &addr.to_string(), 5, 5);
    // merge=0（翻转即投）+ cooldown=0——泵 tick 与测试线程共享 current_thread
    // runtime，探测检查粒度（4 tick）在争用下可被拉长到 ~1.4s；默认 5s 合并窗
    // 会把投递顶出测试预算（与 webhook 泵测试同解法）。
    r.deliver.merge_window_secs = Some(0);
    r.deliver.cooldown_secs = Some(0);
    crate::rules::save_rules(&rules_path, &[r]).unwrap();
    let (hits, contents, inj) = ok_injector();
    let injector: InjectorFn = Arc::new(inj);
    let mgr = EventWatchManager::new(rules_path, injector, Arc::new(|| false));
    let snap = mgr.snapshot_handle();
    let handle = mgr.start();
    // 首探（~1s 检查粒度内）定基态 UP，不投递；2s 后关监听翻转 DOWN。
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
    drop(listener);
    // 翻转探测在 interval（5s）+ 检查粒度（1s）内出发 → 投递含方向与根因。
    let mut delivered = false;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if contents
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.contains("UP → DOWN") && c.contains("[事件:pb1]"))
        {
            delivered = true;
            break;
        }
    }
    let s = snap.read();
    assert!(
        delivered,
        "翻转必须全链投递；contents: {:?}; stats: {:?}; rules: {:?}; pending: {:?}",
        contents.lock().unwrap(),
        s.stats,
        s.rules,
        s.pending
    );
    assert!(hits.load(Ordering::SeqCst) >= 1);
    handle.abort();
}

// ── 事件源扩展 P3：fs content_pattern ────────────────────────────────

fn content_rule(id: &str, root: &std::path::Path, pattern: &str) -> EventRule {
    EventRule {
        id: id.to_string(),
        watch: WatchSpec {
            kind: "fs".to_string(),
            path: root.to_path_buf(),
            content_pattern: Some(pattern.to_string()),
            ..Default::default()
        },
        deliver: DeliverSpec {
            session_key: "agent:main:session:t".to_string(),
            prompt: "日志命中：{paths}（{count}）".to_string(),
            cooldown_secs: None,
            merge_window_secs: None,
            max_per_hour: None,
            enabled: None,
        },
    }
}

fn append_line(file: &std::path::Path, line: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(file).unwrap();
    writeln!(f, "{line}").unwrap();
}

#[test]
fn content_matching_line_triggers_and_non_matching_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.log");
    std::fs::write(&file, "startup ok\nall systems normal\n").unwrap();
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![content_rule("c1", dir.path(), "(?i)fatal|panic")];
    let ev_path = file.to_string_lossy().to_string();
    // 首见：基线窗（尾部 64KB）无命中 → 不触发
    st.ingest(ev(&ev_path, 1000));
    assert!(st.pending.is_empty());
    // 追加命中行 → 触发
    append_line(&file, "FATAL: core dumped");
    st.ingest(ev(&ev_path, 2000));
    assert_eq!(st.pending["c1"].count(), 1);
    // 追加不命中行 → 静默
    append_line(&file, "recovered");
    st.ingest(ev(&ev_path, 3000));
    assert_eq!(st.pending["c1"].count(), 1);
    assert!(
        st.content_state["c1"].offsets.contains_key(&file),
        "坐标已落账"
    );
}

#[test]
fn content_partial_line_waits_for_newline_then_matches_joined() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.log");
    std::fs::write(&file, "ERR").unwrap(); // 半行（无换行）
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![content_rule("c1", dir.path(), "ERROR")];
    let ev_path = file.to_string_lossy().to_string();
    // 基线窗无行边界 → 坐标停窗起点，不消费不触发（残段留给下次拼接）
    st.ingest(ev(&ev_path, 1000));
    assert!(st.pending.is_empty());
    assert_eq!(st.content_state["c1"].offsets.get(&file), Some(&0));
    // 续写补全换行 → 拼接命中
    append_line(&file, "OR boot failed"); // "ERR" + "OR boot failed\n"
    st.ingest(ev(&ev_path, 2000));
    assert_eq!(st.pending["c1"].count(), 1, "跨事件拼接的行必须命中");
}

#[test]
fn content_truncation_rebaselines_without_replaying_old_history() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.log");
    std::fs::write(&file, "old run ERROR\n").unwrap();
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![content_rule("c1", dir.path(), "ERROR")];
    let ev_path = file.to_string_lossy().to_string();
    st.ingest(ev(&ev_path, 1000));
    assert_eq!(st.pending["c1"].count(), 1, "首见基线窗命中照常触发");
    assert!(st.content_state["c1"].offsets[&file] > 0);
    // 截断（日志轮转）→ 重新基线，不触发（新内容无命中）
    std::fs::write(&file, "new run\n").unwrap();
    st.ingest(ev(&ev_path, 2000));
    assert_eq!(st.pending["c1"].count(), 1);
    // 截断后追加命中行 → 正常触发
    append_line(&file, "ERROR again");
    st.ingest(ev(&ev_path, 3000));
    assert_eq!(st.pending["c1"].count(), 2);
}

#[test]
fn content_binary_nul_advances_offset_without_trigger() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dump.bin");
    std::fs::write(&file, b"\x00\x01\x02panic\x00\xff").unwrap(); // 含 NUL + 命中形态
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![content_rule("c1", dir.path(), "panic")];
    let ev_path = file.to_string_lossy().to_string();
    // 二进制（NUL）跳过：不触发但坐标推进（不反复读同一段）
    st.ingest(ev(&ev_path, 1000));
    assert!(st.pending.is_empty());
    assert_eq!(
        st.content_state["c1"].offsets.get(&file),
        Some(&10),
        "坐标推进到段末（\\x00\\x01\\x02 panic \\x00\\xff = 10 字节）"
    );
    // 追加文本命中行 → 正常触发（skip 不影响后续增量）
    append_line(&file, "panic real");
    st.ingest(ev(&ev_path, 2000));
    assert_eq!(st.pending["c1"].count(), 1);
}

#[test]
fn content_read_failure_is_one_time_warn_without_trigger() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.log");
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![content_rule("c1", dir.path(), "ERROR")];
    let ev_path = missing.to_string_lossy().to_string();
    st.ingest(ev(&ev_path, 1000));
    st.ingest(ev(&ev_path, 2000));
    assert!(st.pending.is_empty());
    assert!(st.content_warned.contains("c1"), "一次性 warn 落账");
    assert_eq!(st.stats.delivered_events, 0);
}

#[test]
fn content_gate_scopes_to_own_rule_only() {
    // 同一路径两条 fs 规则：一条带 content_pattern（不命中），一条不带
    // （纯路径变化触发）——互不牵连。
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.log");
    std::fs::write(&file, "calm line\n").unwrap();
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![
        content_rule("c1", dir.path(), "FATAL"),
        rule("plain", dir.path().to_str().unwrap()),
    ];
    let ev_path = file.to_string_lossy().to_string();
    st.ingest(ev(&ev_path, 1000));
    assert!(!st.pending.contains_key("c1"), "content 不命中 → 不触发");
    assert!(st.pending.contains_key("plain"), "无过滤规则照常触发");
}

#[tokio::test]
async fn reload_carries_content_offset_and_probe_baseline_drops_on_change() {
    use crate::rules::save_rules;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    let file = dir.path().join("app.log");
    std::fs::write(&file, "hello ERROR\n").unwrap();
    let mut cr = content_rule("c1", dir.path(), "ERROR");
    let mut pr = probe_rule("pb1", "tcp", "127.0.0.1:1", 60, 5);
    save_rules(&path, &[cr.clone(), pr.clone()]).unwrap();
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.load_rules(&path);
    // drive：content offset 落账 + probe 基态落账
    let ev_path = file.to_string_lossy().to_string();
    st.ingest(ev(&ev_path, 1000));
    assert_eq!(
        st.content_state["c1"].offsets.get(&file),
        Some(&12),
        "\"hello ERROR\\n\" = 12 字节全消费"
    );
    let reqs = st.probe_checks(10_000_000, false);
    assert_eq!(reqs[0].generation, 1);
    st.apply_probe(outcome("pb1", 1, true, "连接成功"), 10_000_000);
    assert_eq!(st.probe_state["pb1"].up, Some(true));
    // 同 watch 段热重载（改 deliver prompt）→ 两态全保留
    std::thread::sleep(std::time::Duration::from_millis(50));
    cr.deliver.prompt = "改了模板：{paths}".to_string();
    pr.deliver.prompt = "改了模板：{payload}".to_string();
    save_rules(&path, &[cr.clone(), pr.clone()]).unwrap();
    assert!(st.check_reload(&path));
    assert_eq!(st.content_state["c1"].offsets.get(&file), Some(&12));
    assert_eq!(st.probe_state["pb1"].up, Some(true));
    assert_eq!(st.probe_state["pb1"].generation, 1, "target 未变代次不推进");
    // content_pattern 变更 → offset 保留（坐标与 pattern 无关）、正则换新
    std::thread::sleep(std::time::Duration::from_millis(50));
    cr.watch.content_pattern = Some("panic".to_string());
    save_rules(&path, &[cr.clone(), pr.clone()]).unwrap();
    assert!(st.check_reload(&path));
    assert_eq!(st.content_state["c1"].offsets.get(&file), Some(&12));
    assert_eq!(st.content_state["c1"].regex.as_str(), "panic", "滤网换新");
    // probe target 变更 → 基态丢弃 + 代次推进（旧目标在途回结果按代次丢）
    std::thread::sleep(std::time::Duration::from_millis(50));
    pr.watch.target = Some("127.0.0.1:2".to_string());
    save_rules(&path, &[cr, pr]).unwrap();
    assert!(st.check_reload(&path));
    assert!(!st.probe_state.contains_key("pb1"), "目标变更重新定基态");
    let reqs = st.probe_checks(20_000_000, false);
    assert_eq!(
        reqs[0].generation, 2,
        "新目标新代次——旧目标迟到结果按代次丢弃"
    );
}

// ── 补钉：content 基线窗历史命中「一次性」语义 ────────────────────────

#[test]
fn content_baseline_history_match_triggers_once_and_does_not_replay() {
    // 语义锚点（基线窗匹配的代价面，刻意钉死防将来误改）：规则安装前已在
    // 盘且历史内容命中的文件——首个变更事件的基线窗命中触发**一次**（这是
    // 「新文件首行命中立即可用」的对价）；此后坐标已越过基线段，新增不命中
    // 行不得重放历史命中（防基线语义退化成「历史一直命中=一直触发」）。
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app.log");
    std::fs::write(&file, "old panic last week\nmore old panic\n").unwrap();
    let mut st = ManagerState::new(DEFAULT_GLOBAL_HOURLY_CAP);
    st.rules = vec![content_rule("c1", dir.path(), "panic")];
    let ev_path = file.to_string_lossy().to_string();
    // 首见：基线窗（尾部 64KB）含历史命中行 → 触发一次（合并计数 = 1 事件）
    st.ingest(ev(&ev_path, 1000));
    assert_eq!(st.pending["c1"].count(), 1, "历史命中随基线窗触发一次");
    // 新增不命中行：坐标已越过基线段 → 不得重放历史
    append_line(&file, "recovered ok");
    st.ingest(ev(&ev_path, 2000));
    assert_eq!(st.pending["c1"].count(), 1, "历史命中不得重放");
    // 新增命中行：正常触发
    append_line(&file, "panic again");
    st.ingest(ev(&ev_path, 3000));
    assert_eq!(st.pending["c1"].count(), 2);
}

// ── sys 当前值展示面（events.status sys_current）─────────────────────

#[tokio::test]
async fn pump_sys_sampling_exposes_sys_current_and_snapshot_carries_it() {
    // 泵级真采样：sys 规则在场 → 懒闸开（2s 采样粒度）→ last_sys 覆写 →
    // snapshot.sys_current 透传。触发面用 mem above=1（任何已启动系统的
    // 内存使用率恒 >1%，首采样必过阈）保持确定性——旧用例的 above=0/
    // below=-1 退化形态（恒触发 + 永不回落）恰是 BUG-20261121-033 收口的
    // 不变量违例，加载期已被剔除，不可再作锚。
    let dir = tempfile::tempdir().unwrap();
    let rules_path = dir.path().join("events.json");
    let mut r = sys_rule("sy1", "mem", 1.0, 0.5);
    r.deliver.merge_window_secs = Some(0);
    r.deliver.cooldown_secs = Some(0);
    crate::rules::save_rules(&rules_path, &[r]).unwrap();
    let (hits, contents, inj) = ok_injector();
    let injector: InjectorFn = Arc::new(inj);
    let mgr = EventWatchManager::new(rules_path, injector, Arc::new(|| false));
    let snap = mgr.snapshot_handle();
    assert!(
        snap.read().sys_current.is_none(),
        "首采样前快照 sys_current = None（懒闸语义）"
    );
    let handle = mgr.start();
    // 采样周期 2s + 规则装载 ≤250ms；8s 预算足够最坏时序。
    let mut sampled = false;
    for _ in 0..32 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let s = snap.read();
        if let Some(cur) = &s.sys_current {
            sampled = true;
            assert!(
                cur.cpu >= 0.0 && cur.mem_percent >= 0.0,
                "采样值非法: cpu={} mem={}",
                cur.cpu,
                cur.mem_percent
            );
            assert!(
                !cur.disks.is_empty(),
                "盘列表不得为空（sysinfo 至少一块盘）"
            );
            for d in &cur.disks {
                assert!(
                    (0.0..=100.0).contains(&d.used_percent),
                    "盘使用率越界: {} {}",
                    d.mount,
                    d.used_percent
                );
            }
            break;
        }
    }
    assert!(
        sampled,
        "8s 内必须出现 sys_current 快照；stats: {:?}",
        snap.read().stats
    );
    assert!(
        hits.load(Ordering::SeqCst) >= 1,
        "mem above=1 规则必须触发投递（任何真实系统首采样必过阈）；contents: {:?}",
        contents.lock().unwrap()
    );
    handle.abort();
}

// ── 进程终态令牌（BUG-20261115-023 家族纪律；DI 形态同 nemesis-web
//    uploads/snooze sweeper，BUG-20261119-208）：泵是常驻 timer-park 循环
//    （select interval tick），cancel 后必须确定性退出、排空窗内不采样不
//    注入。局部令牌（shutdown_tests 纪律：CancellationToken 无复位，全局
//    令牌测试绝不可 cancel）。timeout(5s) 是活锁兜底——旧行为（无令牌
//    消费面）下两则都会 timeout FAIL（红证形态）。

#[tokio::test]
async fn pump_pre_cancelled_exits_immediately() {
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel(); // 预取消：spawn 前已终态 → 前置 is_cancelled 零工作退出
    let mgr = EventWatchManager::new(
        PathBuf::from("unused.json"),
        Arc::new(|_, _| false),
        Arc::new(|| false),
    )
    .with_terminal_token(token);
    let handle = mgr.start();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("预取消后泵必须立即退出（不重新 park timer）")
        .unwrap();
}

#[tokio::test]
async fn pump_cancel_during_park_exits_promptly() {
    let token = tokio_util::sync::CancellationToken::new();
    let mgr = EventWatchManager::new(
        PathBuf::from("unused.json"),
        Arc::new(|_, _| false),
        Arc::new(|| false),
    )
    .with_terminal_token(token.clone());
    let handle = mgr.start();
    // 泵已 park 在 select（tick/rx 臂）上 → cancel 后令牌臂胜出即退。
    token.cancel();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("cancel 后泵必须及时退出（令牌臂优先于 tick）")
        .unwrap();
}

// ---------------------------------------------------------------------------
// BUG-20261123-037：command 探测 cwd 锚点。Some(dir) → 子进程 cwd = dir
// （marker 落 dir）；None → 继承父进程 cwd（旧行为）。探针用 PATH 解析
// 的 cmd/sh 写 marker——绕开裸文件名可执行搜索的平台歧义，证明语义是
// 「子进程 cwd 正确」。
// -------------------------------------------------------------------------

#[cfg(windows)]
#[tokio::test]
async fn command_probe_cwd_anchor_child_cwd_is_anchored_dir() {
    let dir = tempfile::tempdir().unwrap();
    let marker = "probe_cwd_anchor_marker.txt";
    // cmd 的 `cd` 恒退出码 0（up）——用 marker 文件落锚定目录证明 cwd。
    let (up, detail) = command_probe(&format!("cmd /C cd>{marker}"), 10, Some(dir.path())).await;
    assert!(up, "cd 恒 0 必须 up，detail: {detail}");
    let printed = tokio::fs::read_to_string(dir.path().join(marker))
        .await
        .expect("marker 必须落在锚定目录（cwd 锚定生效）");
    let tail = dir.path().file_name().unwrap().to_string_lossy();
    assert!(
        printed.trim().ends_with(tail.as_ref()),
        "子进程 cd 回显尾部必须 = 锚定目录尾部组件（{tail}），got: {:?}",
        printed.trim()
    );
}

#[cfg(not(windows))]
#[tokio::test]
async fn command_probe_cwd_anchor_child_cwd_is_anchored_dir() {
    let dir = tempfile::tempdir().unwrap();
    let marker = "probe_cwd_anchor_marker.txt";
    // `test -f <marker>` 在子进程 cwd（= 锚定目录）内检查文件存在性——
    // 先用同 cwd 探测写入，再用同 cwd 探测断言（两步都锚定才 up）。
    let (up, detail) =
        command_probe(&format!("sh -c 'pwd > {marker}'"), 10, Some(dir.path())).await;
    assert!(up, "pwd 恒 0 必须 up，detail: {detail}");
    let (up, detail) = command_probe(&format!("test -f {marker}"), 10, Some(dir.path())).await;
    assert!(
        up,
        "marker 已写进锚定目录，cwd 锚定的 test -f 必须 up，detail: {detail}"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn command_probe_cwd_none_inherits_parent_process_cwd() {
    // None = 旧行为：marker 落父进程 cwd（测试二进制目录），断言在位后即删。
    let marker = format!("probe_cwd_none_marker_{}.txt", std::process::id());
    let (up, detail) = command_probe(&format!("cmd /C cd>{marker}"), 10, None).await;
    assert!(up, "cd 恒 0 必须 up，detail: {detail}");
    let parent_cwd = std::env::current_dir().unwrap();
    let printed = tokio::fs::read_to_string(parent_cwd.join(&marker))
        .await
        .expect("None = 旧行为：marker 必须落在父进程 cwd");
    let tail = parent_cwd.file_name().unwrap().to_string_lossy();
    assert!(
        printed.trim().ends_with(tail.as_ref()),
        "cd 回显尾部必须 = 父进程 cwd 尾部组件（{tail}），got: {:?}",
        printed.trim()
    );
    let _ = std::fs::remove_file(parent_cwd.join(&marker));
}

#[cfg(not(windows))]
#[tokio::test]
async fn command_probe_cwd_none_inherits_parent_process_cwd() {
    let marker = format!("probe_cwd_none_marker_{}.txt", std::process::id());
    let (up, detail) = command_probe(&format!("sh -c 'pwd > {marker}'"), 10, None).await;
    assert!(up, "detail: {detail}");
    let parent_cwd = std::env::current_dir().unwrap();
    let printed = tokio::fs::read_to_string(parent_cwd.join(&marker))
        .await
        .expect("None = 旧行为：marker 必须落在父进程 cwd");
    assert_eq!(printed.trim(), parent_cwd.to_str().unwrap());
    let _ = std::fs::remove_file(parent_cwd.join(&marker));
}

// ---- BUG-20261125-061：probe client 重定向策略（行为级回归锁） ----
//
// probe 目标是运维配置的监控 URL，凭据内嵌 path/query 形态常见（
// healthchecks.io /ping/<uuid>、uptime-kuma /api/push/<token>）。防线的
// 裁决语义由 nemesis-utils::http_redirect 决策纯函数测试钉死；这里在
// 行为层（真实 HTTP 往返，loopback 双监听）钉住消费面挂线：
// `probe_http_client()` 构造的 client 必须跨源 stop（原 URL 连同内嵌
// 凭据不出站）、同源 follow（http→https 同主机不断链）。若回归摘掉
// `.redirect(no_cross_host_redirect_policy())`（reqwest 默认 = 跨源也
// 跟随、URL 原样带去目标主机），跨源测试当轮红。

/// 读一次 HTTP 请求（请求头 < 2KB，一段 read 足够）并写出响应，
/// 返回收到的请求原文。
async fn serve_one(listener: &tokio::net::TcpListener, response: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut sock, _) = listener.accept().await.expect("accept");
    let mut buf = [0u8; 2048];
    let n = AsyncReadExt::read(&mut sock, &mut buf).await.unwrap_or(0);
    AsyncWriteExt::write_all(&mut sock, response.as_bytes())
        .await
        .expect("write response");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

/// 跨源 302 必须终停：3xx 透传 → 探测诚实报 down（detail 带 302），
/// 第二跳主机不得收到任何请求（URL 内嵌凭据 `/ping/SECRET` 不出站——
/// reqwest 默认策略会带着原 URL 跟随）。
#[tokio::test]
async fn probe_client_stops_cross_host_redirect() {
    let l_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let l_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port_b = l_b.local_addr().unwrap().port();
    let url_a = format!(
        "http://127.0.0.1:{}/ping/SECRET-uuid-token",
        l_a.local_addr().unwrap().port()
    );

    // 第一跳哨兵：断言首跳请求确实带着内嵌凭据路径（证明泄漏类真实存在）。
    let hop_a = tokio::spawn(async move {
        let target = format!("http://127.0.0.1:{port_b}/evil");
        let resp = format!(
            "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        serve_one(&l_a, &resp).await
    });
    // 第二跳哨兵：策略生效时永远等不到连接（500ms 超时 = 诚实通过）；
    // 策略漏挂时 reqwest 立即跟随，哨兵捕获带凭据 URL 的请求 → 断言红。
    let sentry_b = tokio::spawn(async move {
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        serve_one(&l_b, resp).await
    });

    let client = probe_http_client();
    let (up, detail) = http_probe(&client, &url_a, 5).await;
    assert!(
        !up,
        "跨源 302 透传为最终响应 → is_success()=false → 诚实报 down"
    );
    assert!(
        detail.contains("302"),
        "detail 必须带 HTTP 302 根因（翻转载荷可见），got: {detail}"
    );
    let first = hop_a.await.expect("first hop");
    assert!(
        first.contains("/ping/SECRET-uuid-token"),
        "首跳请求必须携带内嵌凭据路径（证明本防线的泄漏类真实），got: {first}"
    );
    let leaked = tokio::time::timeout(std::time::Duration::from_millis(500), sentry_b).await;
    if let Ok(Ok(second_hop_req)) = leaked {
        panic!(
            "BUG-20261125-061：跨源重定向把探测请求（含内嵌凭据 URL）带去了第三方主机，第二跳收到: {second_hop_req}"
        );
        // 超时/哨兵被丢 = 第二跳无连接 = 凭据未出站 ✓
    }
}

/// 同源 302 必须跟随：单监听两次 accept，第二跳命中同源新路径
/// （http→https 同主机等合规场景不断链——策略不是一刀切 Policy::none()）。
#[tokio::test]
async fn probe_client_follows_same_origin_redirect() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/ping/SECRET-uuid-token");

    let server = tokio::spawn(async move {
        let resp302 = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/real\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let first = serve_one(&l, &resp302).await;
        let resp200 = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        let second = serve_one(&l, resp200).await;
        (first, second)
    });

    let client = probe_http_client();
    let (up, detail) = http_probe(&client, &url, 5).await;
    assert!(up, "同源重定向必须跟随到 200 → up，detail: {detail}");
    let (first, second) = server.await.expect("server");
    assert!(
        first.contains("/ping/SECRET-uuid-token"),
        "首跳携带原始路径，got: {first}"
    );
    assert!(
        second.contains("/real"),
        "第二跳命中同源新路径，got: {second}"
    );
}
