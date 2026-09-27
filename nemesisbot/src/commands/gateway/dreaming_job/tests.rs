//! `dreaming_job` 报告轮转（prune_old_reports）单测。
//!
//! 文件名时间戳定宽格式（`sweep_%Y%m%d_%H%M%S`）字典序=时间序——轮转按
//! 字典序排序删最旧，测试用同名形态构造（Canvas #7 修复的回归钉）。

use super::prune_old_reports;

/// 建一份报告文件（名字按真实形态递增）。
fn seed(dir: &std::path::Path, name: &str) {
    std::fs::write(dir.join(name), b"{}").expect("seed report");
}

fn list(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .expect("read_dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn prune_keeps_newest_and_drops_oldest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    for (i, name) in [
        "sweep_20260927_010000.json",
        "sweep_20260927_020000.json",
        "sweep_20260927_030000.json",
        "sweep_20260927_040000.json",
    ]
    .iter()
    .enumerate()
    {
        let _ = i;
        seed(dir, name);
    }
    prune_old_reports(dir, 2);
    assert_eq!(
        list(dir),
        vec![
            "sweep_20260927_030000.json".to_string(),
            "sweep_20260927_040000.json".to_string(),
        ],
        "保留字典序（=时间序）最新的 keep 份"
    );
}

#[test]
fn prune_under_keep_is_noop() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    seed(dir, "sweep_20260927_010000.json");
    seed(dir, "sweep_20260927_020000.json");
    prune_old_reports(dir, 30);
    assert_eq!(list(dir).len(), 2, "不足 keep 份不动任何文件");
}

#[test]
fn prune_ignores_non_json_and_missing_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    seed(dir, "sweep_20260927_010000.json");
    seed(dir, "sweep_20260927_020000.json");
    seed(dir, "notes.txt");
    seed(dir, "sweep_20260927_030000.json");
    // 非 json（notes.txt）不算报告，不参与计数也绝不删除。
    prune_old_reports(dir, 2);
    let left = list(dir);
    assert!(
        left.contains(&"notes.txt".to_string()),
        "非 json 文件不清理: {left:?}"
    );
    assert_eq!(
        left.len(),
        3,
        "json 只留最新 2 份 + notes.txt = 3: {left:?}"
    );
    // 目录不存在 = 静默返回（幂等，清扫失败不致命的边界形态）。
    prune_old_reports(&dir.join("no_such_dir"), 2);
}
