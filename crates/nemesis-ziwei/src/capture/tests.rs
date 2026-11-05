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

/// BUG-20261105-018：大文件下 read_recent 只留尾部 limit 条（滑动窗口），
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
