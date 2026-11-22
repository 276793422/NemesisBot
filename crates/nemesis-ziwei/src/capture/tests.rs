//! capture 存储测试（外置：`#[cfg(test)] mod tests;` 约定）。

use super::*;
use std::time::Duration;

fn temp_ws(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "zw-cap-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn append_and_read_recent() {
    let ws = temp_ws("round");
    let store = CaptureStore::new(&ws);
    for i in 0..5 {
        store
            .append("p1", serde_json::json!({ "tool": "t", "i": i }))
            .unwrap();
    }
    let all = store.read_recent("p1", 100);
    assert_eq!(all.len(), 5);
    assert!(all[0]["ts"].is_string(), "ts 缺省补齐");
    assert_eq!(all[4]["i"], 4);
    // limit 生效：取尾部
    let tail2 = store.read_recent("p1", 2);
    assert_eq!(tail2.len(), 2);
    assert_eq!(tail2[1]["i"], 4);
    // 不存在的 profile = 空，不炸
    assert!(store.read_recent("ghost", 10).is_empty());
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

#[test]
fn profiles_isolated() {
    let ws = temp_ws("iso");
    let store = CaptureStore::new(&ws);
    store.append("a", serde_json::json!({"who":"a"})).unwrap();
    store.append("b", serde_json::json!({"who":"b"})).unwrap();
    assert_eq!(store.read_recent("a", 10)[0]["who"], "a");
    assert_eq!(store.read_recent("b", 10)[0]["who"], "b");
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

/// 大文件下 read_recent 只留尾部 limit 条（滑动窗口），
/// 返回 = 尾部且恰好 limit 条；limit=0 = 空（旧语义不变）。
#[test]
fn read_recent_sliding_window_keeps_tail_only() {
    let ws = temp_ws("slide");
    let store = CaptureStore::new(&ws);
    for i in 0..2000 {
        store.append("bulk", serde_json::json!({ "i": i })).unwrap();
    }
    let tail = store.read_recent("bulk", 50);
    assert_eq!(tail.len(), 50, "恰好 limit 条");
    assert_eq!(tail[0]["i"], 1950, "窗口首条 = 总数-limit");
    assert_eq!(tail[49]["i"], 1999, "窗口尾条 = 最新");
    // limit=0 = 空（旧行为钉死）
    assert!(store.read_recent("bulk", 0).is_empty());
    // limit > 行数 = 全量
    assert_eq!(store.read_recent("bulk", 5000).len(), 2000);
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

#[test]
fn ttl_sweep_expires_old_keeps_new() {
    let ws = temp_ws("ttl");
    let store = CaptureStore::new(&ws);
    store.append("old", serde_json::json!({"x":1})).unwrap();
    store.append("new", serde_json::json!({"x":2})).unwrap();
    // 把 old 的 mtime 拨到 8 天前（std FileTimes，无需额外依赖）
    let old_file = ws
        .join("ziwei")
        .join("runs")
        .join("old")
        .join("capture.jsonl");
    let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600);
    let f = std::fs::File::options()
        .write(true)
        .open(&old_file)
        .unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(old_time))
        .unwrap();
    let report = store.sweep(Duration::from_secs(7 * 24 * 3600), MAX_BYTES_PER_PROFILE);
    assert_eq!(report.expired_profiles, 1);
    assert_eq!(report.kept, 1);
    assert!(!old_file.exists(), "过期 profile 目录应删除");
    assert!(store.read_recent("new", 10).len() == 1);
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

#[test]
fn size_cap_truncates_keeping_newest() {
    let ws = temp_ws("cap");
    let store = CaptureStore::new(&ws);
    // 每条 ~100 字节 × 50 条 = ~5KB；上限 1KB → 应保尾部 ~10 条
    for i in 0..50 {
        store
            .append("big", serde_json::json!({"i": i, "pad": "x".repeat(80)}))
            .unwrap();
    }
    let report = store.sweep(Duration::from_secs(3600), 1024);
    assert_eq!(report.size_truncated, 1);
    let kept = store.read_recent("big", 1000);
    assert!(!kept.is_empty());
    assert_eq!(kept.last().unwrap()["i"], 49, "最新一条必须保住");
    // 数量显著缩水（< 50）且尾行时间戳最新
    assert!(kept.len() < 50, "应截断，实保 {} 条", kept.len());
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

#[test]
fn sweep_tolerates_missing_dir_and_strangers() {
    let ws = temp_ws("stray");
    let store = CaptureStore::new(&ws);
    // runs 目录不存在 / 有陌生日录（无 capture.jsonl）都不炸、不误删
    std::fs::create_dir_all(ws.join("ziwei").join("runs").join("stranger")).unwrap();
    let report = store.sweep(Duration::from_secs(1), 1024);
    assert_eq!(report.expired_profiles, 0);
    assert!(ws.join("ziwei").join("runs").join("stranger").is_dir());
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

/// BUG-20261121-058① 回归锁：唯一临时名权威 helper 收编——旧固定名
/// `capture.jsonl.tmp` 被残留垃圾（目录形态最顽固，bug59 同族）占用时
/// File::create 必败，容量截断每轮 sweep WARN 空转永久失效。新实现走
/// 唯一临时名照常截断，且不碰陌生的固定名残留（best-effort 不扩大面）。
#[test]
fn b058_truncation_survives_residue_at_fixed_tmp_path() {
    let ws = temp_ws("b058tmp");
    let store = CaptureStore::new(&ws);
    for i in 0..50 {
        store
            .append("res", serde_json::json!({ "i": i, "pad": "x".repeat(80) }))
            .unwrap();
    }
    // 在旧固定临时名上种目录（比文件更顽固：File::create 对目录恒败）。
    let fixed_tmp = ws
        .join("ziwei")
        .join("runs")
        .join("res")
        .join("capture.jsonl.tmp");
    std::fs::create_dir(&fixed_tmp).unwrap();

    let report = store.sweep(Duration::from_secs(3600), 1024);
    assert_eq!(report.size_truncated, 1, "残留垃圾占固定名不得阻断容量截断");
    assert!(fixed_tmp.is_dir(), "陌生的固定名残留不得被 helper 触碰");
    let kept = store.read_recent("res", 1000);
    assert!(!kept.is_empty() && kept.len() < 50, "照常截断保尾部");
    assert_eq!(kept.last().unwrap()["i"], 49, "最新一条必须保住");
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

/// BUG-20261121-058② 回归锁：读失败/非 UTF-8 行诚实上浮放弃截断——旧
/// `map_while(|l| l.ok())` 在错误点静默掐断，重写后错误点之后全部行
/// 永久丢失（截断永远不得静默丢行，与「单条超大记录不静默丢」同一
/// 不变量）。新实现 Err 上浮 → 文件原样保留，下轮 sweep 重试。
#[test]
fn b058_truncation_aborts_on_invalid_utf8_without_silent_tail_loss() {
    let ws = temp_ws("b058utf");
    let store = CaptureStore::new(&ws);
    store.append("bad", serde_json::json!({ "i": 0 })).unwrap();
    store.append("bad", serde_json::json!({ "i": 1 })).unwrap();
    // 中段注入非 UTF-8 字节（模拟磁盘损坏/外力截断），尾行再补一条合法记录。
    let file = ws
        .join("ziwei")
        .join("runs")
        .join("bad")
        .join("capture.jsonl");
    let mut raw = std::fs::read(&file).unwrap();
    let mid = raw.len() / 2;
    raw.splice(mid..mid, [0xFFu8, 0xFEu8]);
    raw.extend_from_slice(br#"{"i":2}"#);
    raw.push(b'\n');
    std::fs::write(&file, &raw).unwrap();

    let report = store.sweep(Duration::from_secs(3600), 1); // 上限 1B：必触发截断路径
    assert_eq!(report.size_truncated, 0, "读失败必须诚实放弃截断（不计数）");
    let after = std::fs::read(&file).unwrap();
    assert_eq!(after, raw, "文件必须原样保留（不静默丢错误点之后的行）");
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}
