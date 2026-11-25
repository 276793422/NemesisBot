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

// ── BUG-20261125-003 回归锁：update_loop 锁内 RMW ──────────────────

use crate::store::{create_loop, update_loop};

#[test]
fn update_loop_normal_rm_writes_back() {
    let dir = tempfile::tempdir().unwrap();
    let created = create_loop(dir.path(), |id| Ok(st(&id, "k"))).unwrap();
    let updated = update_loop(dir.path(), &created.id, |s| {
        s.status = LoopStatus::Paused;
        s.push_progress(2000, "[paused] test".to_string());
        Ok(true)
    })
    .unwrap();
    assert_eq!(updated.status, LoopStatus::Paused);
    let on_disk = load_loop(dir.path(), &created.id).unwrap().unwrap();
    assert_eq!(on_disk.status, LoopStatus::Paused);
    assert_eq!(on_disk.progress_log.len(), 1);
}

#[test]
fn update_loop_err_skips_write() {
    let dir = tempfile::tempdir().unwrap();
    let created = create_loop(dir.path(), |id| Ok(st(&id, "k"))).unwrap();
    let err = update_loop(dir.path(), &created.id, |s| {
        s.status = LoopStatus::Done; // mutate 先发生
        Err("状态机校验失败".to_string()) // 但裁决失败
    })
    .unwrap_err();
    assert!(err.contains("状态机校验失败"));
    // 盘上未变：mutate 不落盘（裁决失败 = 放弃整个临界区）。
    let on_disk = load_loop(dir.path(), &created.id).unwrap().unwrap();
    assert_eq!(on_disk.status, LoopStatus::Active);
    assert!(on_disk.progress_log.is_empty());
}

#[test]
fn update_loop_false_skips_write() {
    let dir = tempfile::tempdir().unwrap();
    let created = create_loop(dir.path(), |id| Ok(st(&id, "k"))).unwrap();
    let updated = update_loop(dir.path(), &created.id, |_s| Ok(false)).unwrap();
    // 返回锁内终态但未落盘（幂等命中省写）。
    assert_eq!(updated.status, LoopStatus::Active);
    let on_disk = load_loop(dir.path(), &created.id).unwrap().unwrap();
    assert_eq!(on_disk.status, LoopStatus::Active);
}

#[test]
fn update_loop_missing_id_errs() {
    let dir = tempfile::tempdir().unwrap();
    let err = update_loop(dir.path(), "L99", |_s| Ok(true)).unwrap_err();
    assert!(err.contains("不存在"), "诚实报错：{err}");
}

#[test]
fn remove_then_update_errs_no_resurrect() {
    // remove 与 update_loop 同锁串行：删除后 RMW 不复活已删文件。
    let dir = tempfile::tempdir().unwrap();
    let created = create_loop(dir.path(), |id| Ok(st(&id, "k"))).unwrap();
    assert!(remove_loop(dir.path(), &created.id).unwrap());
    let err = update_loop(dir.path(), &created.id, |s| {
        s.status = LoopStatus::Done;
        Ok(true)
    })
    .unwrap_err();
    assert!(err.contains("不存在"), "remove 后 RMW 必须诚实报错：{err}");
    assert!(
        load_loop(dir.path(), &created.id).unwrap().is_none(),
        "已删 loop 不得被 update 写回复活"
    );
}

#[test]
fn update_loop_concurrent_threads_serialize_no_lost_update() {
    // 双写者并发 RMW：8 线程各 push 一条 progress——锁串行化后全部
    // 16 条（每线程 2 条）都在盘上；裸 RMW 会丢到只剩部分。
    let dir = tempfile::tempdir().unwrap();
    let created = create_loop(dir.path(), |id| Ok(st(&id, "k"))).unwrap();
    let dir_path = dir.path().to_path_buf();
    let id = created.id.clone();
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let p = dir_path.clone();
            let id = id.clone();
            std::thread::spawn(move || {
                for j in 0..2 {
                    update_loop(&p, &id, |s| {
                        s.push_progress(1000 + (i * 2 + j) as i64, format!("t{i}-j{j}"));
                        Ok(true)
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let on_disk = load_loop(dir.path(), &created.id).unwrap().unwrap();
    assert_eq!(on_disk.progress_log.len(), 16, "锁内 RMW 下并发写入零丢失");
}
