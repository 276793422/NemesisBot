//! `dreaming_job` 报告轮转（prune_old_reports）单测。
//!
//! 文件名时间戳定宽格式（`sweep_%Y%m%d_%H%M%S`）字典序=时间序——轮转按
//! 字典序排序删最旧，测试用同名形态构造（Canvas #7 修复的回归钉）。

use super::prune_old_reports;
use super::{SWEEP_INFLIGHT, fire_dreaming_sweep, try_acquire_sweep_slot};
use std::sync::atomic::{AtomicBool, Ordering};

/// BUG-20261108-011 回归锁：sweep 执行槽互斥语义。计划触发与手动「立即
/// 运行」可能双触发同一 sweep——并发 sweep 会同批候选交叉决策（run_sweep
/// 的 taken 集合只是本轮内幂等）、LLM 成本双花、报告同秒覆盖。已在跑时
/// 第二次触发必须被拒；guard Drop（含 panic/早退）必须释放槽位。
#[test]
fn sweep_slot_mutex_second_acquire_rejected_and_drop_releases() {
    let flag = AtomicBool::new(false);
    let g1 = try_acquire_sweep_slot(&flag).expect("空闲槽首拍必须占到");
    assert!(flag.load(Ordering::Acquire), "占用后标志必须为 true");
    assert!(
        try_acquire_sweep_slot(&flag).is_none(),
        "已在跑时第二触发必须被拒（并发 sweep 会同批候选交叉决策）"
    );
    drop(g1);
    assert!(
        !flag.load(Ordering::Acquire),
        "Drop 必须释放槽位（panic/早退全覆盖）"
    );
    assert!(try_acquire_sweep_slot(&flag).is_some(), "释放后必须可再占");
}

/// BUG-20261108-011 回归锁：无 tokio 运行时上下文的 Err 路径不得泄露执行
/// 槽——一次环境失败若把 static 槽位占死，后续所有触发（含计划触发）将
/// 永久被拒。
#[test]
fn fire_without_runtime_returns_err_and_does_not_leak_slot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mgr = std::sync::Arc::new(nemesis_memory::manager::MemoryManager::new(
        &nemesis_memory::manager::Config::new(tmp.path().join("memory")),
    ));
    // 无 #[tokio::test] = 无 tokio 运行时上下文 → fire 走 try_current Err 分支。
    let err = fire_dreaming_sweep(mgr, tmp.path().to_path_buf()).unwrap_err();
    assert!(err.contains("tokio"), "错误必须是无运行时上下文: {err}");
    // Err 路径后 static 槽位必须已复位（再占应成功），否则一次失败锁死全部后续触发。
    let probe =
        try_acquire_sweep_slot(&SWEEP_INFLIGHT).expect("Err 路径后槽位必须已释放（防泄露锁死）");
    drop(probe); // 测试内占用的 static 槽位由 guard drop 复位，不留给并行测试
}

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
