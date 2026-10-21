//! HotReloader force_reload 语义测试（2026-10-22 CI Windows 审批 pattern
//! 假红的回归钉：写方落盘后必须能不依赖 mtime 检测地刷新内存态）。

use crate::hot_reload::HotReloader;
use std::path::Path;
use std::time::Duration;

fn load_vec(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

fn write_json(path: &Path, v: &[&str]) {
    std::fs::write(path, serde_json::to_string(&v).unwrap()).unwrap();
}

#[test]
fn force_reload_picks_up_rewrite_without_mtime_change_signal() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hot.json");
    write_json(&path, &["v1"]);
    let r = HotReloader::new(path.clone(), load_vec);
    assert_eq!(r.get(), vec!["v1".to_string()]);

    // 常规路径 sanity：mtime 信号确定变化 → check() 重载。mtime 用
    // set_times 钉到远离当前的固定值——不能赌连续两次快速写自然产生
    // 不同 mtime（CI Windows runner 实录：亚毫秒级连写撞同值 mtime，
    // check() 漏检假红 2026-10-22；这正是 force_reload 防的现象本身）。
    write_json(&path, &["v2"]);
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)),
    )
    .unwrap();
    assert!(r.check(), "mtime 变化应触发重载");
    assert_eq!(r.get(), vec!["v2".to_string()]);

    // force_reload：绕过 mtime 比较也能拿到最新值（写方自刷新契约）。
    write_json(&path, &["v3"]);
    assert!(r.force_reload());
    assert_eq!(r.get(), vec!["v3".to_string()]);

    // 文件消失：load 容错返回空、stat None 记账、返回 false。
    std::fs::remove_file(&path).unwrap();
    assert!(!r.force_reload());
    assert!(r.get().is_empty());
}
