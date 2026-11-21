//! state + store 单测（roundtrip / id 分配 / 坏文件容错 / 环形截断）。

use crate::state::{LoopState, LoopStatus, PROGRESS_LOG_CAP};
use crate::store::{alloc_id, list_loops, load_loop, loops_dir, remove_loop, save_loop};
use std::path::PathBuf;

fn st(id: &str, session: &str) -> LoopState {
    LoopState::new(
        id.to_string(),
        "测试目标".to_string(),
        session.to_string(),
        1000,
    )
}

#[test]
fn new_state_defaults_active_and_unscheduled() {
    let s = st("L1", "agent:main:session:a");
    assert_eq!(s.status, LoopStatus::Active);
    assert_eq!(s.next_wake_ms, None);
    assert_eq!(s.cron, None);
    assert_eq!(s.spent.turns, 0);
    assert!(s.finish_summary.is_none());
}

#[test]
fn push_progress_caps_at_limit_dropping_oldest() {
    let mut s = st("L1", "k");
    for i in 0..(PROGRESS_LOG_CAP + 20) {
        s.push_progress(i as i64, format!("p{i}"));
    }
    assert_eq!(s.progress_log.len(), PROGRESS_LOG_CAP);
    // 最旧 20 条被丢：首条是 p20。
    assert_eq!(s.progress_log[0].text, "p20");
    assert_eq!(
        s.progress_log.last().unwrap().text,
        format!("p{}", PROGRESS_LOG_CAP + 19)
    );
}

#[test]
fn progress_tail_returns_last_n_in_order() {
    let mut s = st("L1", "k");
    for i in 0..5 {
        s.push_progress(i, format!("p{i}"));
    }
    // 原序（旧→新）取尾部 2 条——与完整日志同向，时间线自然阅读。
    let tail: Vec<String> = s
        .progress_tail(2)
        .into_iter()
        .map(|e| e.text.clone())
        .collect();
    assert_eq!(tail, vec!["p3", "p4"]);
}

#[test]
fn save_load_roundtrip_preserves_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = st("L1", "agent:main:session:a");
    s.push_progress(100, "第一步完成");
    s.next_wake_ms = Some(9999);
    s.cron = Some("0 9 * * *".to_string());
    s.plan = "先做A再做B".to_string();
    save_loop(dir.path(), &s).unwrap();
    let back = load_loop(dir.path(), "L1").unwrap().unwrap();
    assert_eq!(back.goal, s.goal);
    assert_eq!(back.status, LoopStatus::Active);
    assert_eq!(back.progress_log.len(), 1);
    assert_eq!(back.next_wake_ms, Some(9999));
    assert_eq!(back.cron.as_deref(), Some("0 9 * * *"));
    assert_eq!(back.plan, "先做A再做B");
    // 缺文件 = None 非错误。
    assert!(load_loop(dir.path(), "L999").unwrap().is_none());
}

#[test]
fn list_loops_sorted_and_skips_corrupt_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = st("L1", "k1");
    a.goal = "AAA".to_string();
    let mut c = st("L3", "k3");
    c.goal = "CCC".to_string();
    save_loop(dir.path(), &a).unwrap();
    save_loop(dir.path(), &c).unwrap();
    // 坏文件混入（半写）。
    std::fs::write(dir.path().join("L2.json"), "{broken").unwrap();
    let all = list_loops(dir.path());
    assert_eq!(all.len(), 2, "坏文件必须跳过不炸整表");
    assert_eq!(all[0].id, "L1");
    assert_eq!(all[1].id, "L3");
}

#[test]
fn alloc_id_monotonic_and_ignores_non_loop_files() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(alloc_id(dir.path()), "L1");
    let mut s = st("L7", "k");
    s.goal = "g".to_string();
    save_loop(dir.path(), &s).unwrap();
    // 非 loop 形态文件（events.json 之类同名目录混入物）不参与 max。
    std::fs::write(dir.path().join("junk.json"), "{}").unwrap();
    assert_eq!(alloc_id(dir.path()), "L8");
}

#[test]
fn remove_loop_reports_existence() {
    let dir = tempfile::tempdir().unwrap();
    let s = st("L1", "k");
    save_loop(dir.path(), &s).unwrap();
    assert!(remove_loop(dir.path(), "L1").unwrap());
    assert!(!remove_loop(dir.path(), "L1").unwrap());
}

#[test]
fn loops_dir_layout() {
    assert_eq!(
        loops_dir(&PathBuf::from("/ws")),
        PathBuf::from("/ws").join("loops")
    );
}

#[test]
fn create_loop_concurrent_threads_allocates_distinct_ids() {
    // BUG-20261119-186 回归锁：裸 alloc_id+save_loop 两步在并发创建者
    // 下互拿到同一 max+1 → 同 id 互覆（先建 loop 静默丢失）。create_loop
    // 进程级锁串行化后，多线程并发创建必须各得唯一 id 且全落盘。
    let dir = tempfile::tempdir().unwrap();
    let dir_path = dir.path().to_path_buf();
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let p = dir_path.clone();
            std::thread::spawn(move || {
                crate::store::create_loop(&p, |id| {
                    let mut s = st(&id, &format!("agent:main:session:t{i}"));
                    s.goal = format!("goal-{i}");
                    Ok(s)
                })
                .unwrap()
            })
        })
        .collect();
    let mut ids = Vec::new();
    for h in handles {
        ids.push(h.join().unwrap().id);
    }
    ids.sort();
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        8,
        "并发创建必须各得唯一 id（同 id = 后写者覆盖前者）: {ids:?}"
    );
    // 全部落盘且 list 可见（8 条状态文件，无互相覆盖的丢失）。
    let all = list_loops(dir.path());
    assert_eq!(all.len(), 8, "并发创建必须全部落盘: {}", all.len());
}

#[test]
fn create_loop_build_error_aborts_without_write() {
    let dir = tempfile::tempdir().unwrap();
    let err = crate::store::create_loop(dir.path(), |_id| Err("校验失败".to_string())).unwrap_err();
    assert_eq!(err, "校验失败");
    // 放弃不落盘：目录里无 L*.json 残留（id 作废留空档是预期语义）。
    assert!(list_loops(dir.path()).is_empty());
    assert_eq!(alloc_id(dir.path()), "L1", "作废号不留死档，下分配从头起");
}

// BUG-20261121-032：id 路径安全——`../`、`\`、盘符形态经 loop_path join
// 可达 loops 目录外（remove 面 = 删任意 .json；load 面 = 目录外文件存在
// 性/可解析性探针）。入口校验关死后越界 id 必 Err 且目录外目标文件原封
// 不动；save 侧 st.id 同闸（伪造内容里的越界 id 不得借此落盘写文件）。
#[test]
fn loop_id_rejects_traversal_and_stays_in_dir() {
    let dir = tempfile::tempdir().unwrap();
    // 目录外诱饵文件：若无校验，remove_loop(dir, "../target") 会删掉它。
    let outside = dir.path().join("target.json");
    std::fs::write(&outside, "{}").unwrap();

    assert!(
        load_loop(dir.path(), "../target").is_err(),
        "load 越界 id 必拒"
    );
    assert!(
        remove_loop(dir.path(), "../target").is_err(),
        "remove 越界 id 必拒"
    );
    assert!(outside.exists(), "越界 id 不得删目录外文件");

    let mut s = st("L1", "k");
    s.id = "..\\evil".to_string();
    assert!(
        save_loop(dir.path(), &s).is_err(),
        "save 侧 st.id 同闸（越界 id 状态文件不得落盘）"
    );
    let s = st("c:evil", "k");
    assert!(save_loop(dir.path(), &s).is_err(), "盘符形态同拒");

    // 合法形态不受影响（roundtrip 照常）。
    let s = st("L1", "k");
    save_loop(dir.path(), &s).unwrap();
    assert!(load_loop(dir.path(), "L1").unwrap().is_some());
    assert!(remove_loop(dir.path(), "L1").unwrap());
}

#[test]
fn status_terminal_and_str_roundtrip() {
    assert!(LoopStatus::Done.is_terminal());
    assert!(LoopStatus::Failed.is_terminal());
    assert!(!LoopStatus::Active.is_terminal());
    assert!(!LoopStatus::Paused.is_terminal());
    assert!(!LoopStatus::Parked.is_terminal());
    for (s, name) in [
        (LoopStatus::Active, "active"),
        (LoopStatus::Paused, "paused"),
        (LoopStatus::Parked, "parked"),
        (LoopStatus::Done, "done"),
        (LoopStatus::Failed, "failed"),
    ] {
        assert_eq!(s.as_str(), name);
    }
}
