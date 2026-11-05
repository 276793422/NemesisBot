//! HotReloader force_reload 语义测试（2026-10-22 CI Windows 审批 pattern
//! 假红的回归钉：写方落盘后必须能不依赖变更检测地刷新内存态）+ 同刻改写
//! 回归钉（内容比对不赌钟——mtime 钉回旧值也必须重载）。

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

/// 把 mtime 钉回指定值（内容不动）——确定性复现 Windows 毫秒级粒度下
/// 「同刻改写」形态（CI Windows 亚毫秒级连写撞同值 mtime）。
fn pin_mtime(path: &Path, t: std::time::SystemTime) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(t))
        .unwrap();
}

#[test]
fn force_reload_picks_up_rewrite_without_change_signal() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hot.json");
    write_json(&path, &["v1"]);
    let r = HotReloader::new(path.clone(), load_vec);
    assert_eq!(r.get(), vec!["v1".to_string()]);

    // 常规路径 sanity：内容变化 → check 重载。mtime 钉到远离当前的
    // 固定值——不赌连续两次快速写自然产生不同 mtime（CI Windows runner
    // 亚毫秒级连写撞同值 mtime，；这正是内容闸收编前
    // force_reload 防的现象本身）。
    write_json(&path, &["v2"]);
    pin_mtime(
        &path,
        std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000),
    );
    assert!(r.check(), "内容变化应触发重载");
    assert_eq!(r.get(), vec!["v2".to_string()]);

    // force_reload：绕过变更检测也能拿到最新值（写方自刷新契约）。
    write_json(&path, &["v3"]);
    assert!(r.force_reload());
    assert_eq!(r.get(), vec!["v3".to_string()]);

    // 文件消失：load 容错返回空、读 None 记账、返回 false。
    std::fs::remove_file(&path).unwrap();
    assert!(!r.force_reload());
    assert!(r.get().is_empty());
}

/// 回归钉：mtime 不变（同刻改写）也必须重载——内容
/// 比对不赌钟。旧 mtime 等值闸在此直接跳过 = 配置静默永久陈旧。
#[test]
fn check_reloads_when_content_changes_with_pinned_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("hot.json");
    write_json(&path, &["v1"]);
    let r = HotReloader::new(path.clone(), load_vec);
    assert!(!r.check(), "无变化 no-op");
    let old_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

    // 同刻改写：内容换 v2，mtime 钉回旧值 → check 必须重载。
    write_json(&path, &["v2"]);
    pin_mtime(&path, old_mtime);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        old_mtime,
        "mtime 已钉回旧值（前置成立）"
    );
    assert!(r.check(), "mtime 不变的同刻改写也必须重载（内容比对）");
    assert_eq!(r.get(), vec!["v2".to_string()]);

    // 内容一致的覆写（mtime 前进）→ no-op：内容未变不重载。
    write_json(&path, &["v2"]);
    assert!(!r.check(), "同内容重写不触发冗余重载");
    assert_eq!(r.get(), vec!["v2".to_string()]);

    // 文件消失 → 重载（load 兜底空表）+ 内容恢复后重载回来。
    std::fs::remove_file(&path).unwrap();
    assert!(r.check(), "文件消失必须重载（换缺省语义）");
    assert!(r.get().is_empty());
    write_json(&path, &["v4"]);
    assert!(r.check());
    assert_eq!(r.get(), vec!["v4".to_string()]);
}
