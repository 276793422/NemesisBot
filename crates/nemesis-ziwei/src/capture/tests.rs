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
