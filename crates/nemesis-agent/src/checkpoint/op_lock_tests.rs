//! BUG-20261119-205 回归锁：影子库 git 操作 op_lock 串行契约。
//!
//! 旧形态 `repo: Mutex<git2::Repository>` 结构性串行化**全部** git 操作；
//! 非驻留化重构（5d3e957d，2026-10-31）换成 op_lock + 每操作临时 open 后
//! 只给 6 个调用点补锁，四处漏接（begin_for_session / restore_code /
//! current_tree_hex / conflict_scan_scoped 全树补充）+ gc 重建只在换名窗
//! 口持锁。本文件把「一切影子库 git 操作必须先持 op_lock 再 open()」的
//! 契约钉进测试面：
//!
//! - **阻塞探测**：外部先持 op_lock → 被测操作在旁路线程跑 → 600ms 内
//!   必须不完成（阻塞 = 尊重锁；完成 = 绕锁）。先 sleep 100ms 让线程到
//!   达锁点，避免「线程还没起」稀释探测力。
//! - **gc 全程持锁**：持锁窗口内轮询临时重建库（`checkpoints.git.gc-tmp`）
//!   **不得出现**——旧形态走查段绕锁，小仓 walk 毫秒级走完、tmp 在换名
//!   窗口取锁前就落盘。
//! - **并发 begin 无败者**：8 线程并发 begin_for_session，全部 cur 桶
//!   tree Some（旧形态 index.lock 竞态败者 tree=None → rewind 落到更早
//!   tree 过度回退）。

use std::sync::Arc;
use std::time::Duration;

use super::*;

/// 建一个带真 git 仓的临时 workspace + git 影子库形态的 store。
fn git_store() -> (tempfile::TempDir, Arc<CheckpointStore>) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    let store = Arc::new(CheckpointStore::new(
        Some(dir.path().join("logs").join("checkpoints")),
        root.clone(),
    ));
    assert_eq!(store.backend(), CheckpointBackend::Git);
    (dir, store)
}

/// 线程到达锁点的缓冲（探测窗口从这之后才开始计）。
const THREAD_START_PAD: Duration = Duration::from_millis(100);
/// 阻塞探测窗口：窗口内完成 = 绕锁（BUG 复现）；超时 = 尊重锁（健康）。
const BLOCK_PROBE: Duration = Duration::from_millis(600);
/// 放锁后的完成时限（本地毫秒级，给慢机留余量）。
const FINISH_LIMIT: Duration = Duration::from_secs(10);

/// begin 的 turn 树写入必须持 op_lock（漏接点 1/4：并发会话 begin 撞
/// index.lock → 败者 turn tree=None）。
#[test]
fn begin_blocks_while_op_lock_held() {
    let (_dir, store) = git_store();
    let guard = store.git.as_ref().unwrap().op_lock.lock();

    let (tx, rx) = std::sync::mpsc::channel();
    {
        let store = store.clone();
        std::thread::spawn(move || {
            store.begin_for_session(1, "t1", Some("s1"));
            tx.send(()).unwrap();
        });
    }
    std::thread::sleep(THREAD_START_PAD);
    assert!(
        rx.recv_timeout(BLOCK_PROBE).is_err(),
        "begin_for_session 未持 op_lock 就完成了影子 tree 写入（串行语义丢失）"
    );
    drop(guard);
    rx.recv_timeout(FINISH_LIMIT)
        .expect("放锁后 begin 未在时限内完成");
    let cp = store
        .inner
        .lock()
        .cur
        .get(&Some("s1".to_string()))
        .cloned()
        .expect("cur 桶缺失");
    assert!(
        cp.tree.is_some(),
        "begin 完成但 turn tree=None（影子写入失败）"
    );
}

/// restore_code 的 tree diff + checkout 必须持 op_lock（漏接点 2/4：
/// begin×restore 并发 → checkout 覆写中哈希 → 撕裂 begin 树成为回滚锚）。
#[test]
fn restore_code_blocks_while_op_lock_held() {
    let (_dir, store) = git_store();
    std::fs::write(store.root().join("a.txt"), "v1").unwrap();
    store.begin_for_session(1, "t1", None);
    assert!(store.current_tree_hex().is_some(), "前置 begin 未拿到 tree");

    let guard = store.git.as_ref().unwrap().op_lock.lock();
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let store = store.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let report = rt.block_on(store.restore_code(1));
            tx.send(report).unwrap();
        });
    }
    std::thread::sleep(THREAD_START_PAD);
    assert!(
        rx.recv_timeout(BLOCK_PROBE).is_err(),
        "restore_code 未持 op_lock 就完成了 checkout（串行语义丢失）"
    );
    drop(guard);
    let report = rx
        .recv_timeout(FINISH_LIMIT)
        .expect("放锁后 restore_code 未在时限内完成");
    assert!(report.ok(), "restore_code 失败面: {:?}", report.failures);
}

/// current_tree_hex（redo 实时树采集）必须持 op_lock（漏接点 3/4：与并发
/// turn begin 同窗并发是常态序列，index 写入互斥）。
#[test]
fn current_tree_hex_blocks_while_op_lock_held() {
    let (_dir, store) = git_store();
    store.begin_for_session(1, "t1", None);

    let guard = store.git.as_ref().unwrap().op_lock.lock();
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let store = store.clone();
        std::thread::spawn(move || {
            tx.send(store.current_tree_hex()).unwrap();
        });
    }
    std::thread::sleep(THREAD_START_PAD);
    assert!(
        rx.recv_timeout(BLOCK_PROBE).is_err(),
        "current_tree_hex 未持 op_lock 就完成了实时树写入（串行语义丢失）"
    );
    drop(guard);
    let hex = rx
        .recv_timeout(FINISH_LIMIT)
        .expect("放锁后 current_tree_hex 未在时限内完成");
    assert!(hex.is_some(), "放锁后实时影子 tree 写入失败");
}

/// conflict_scan_scoped 的全树 diff 补充段必须持 op_lock（漏接点 4/4：
/// tree_diff_unchecked 内部有 write_current_tree 的 index 写入）。
#[test]
fn conflict_scan_git_diff_blocks_while_op_lock_held() {
    let (_dir, store) = git_store();
    std::fs::write(store.root().join("a.txt"), "v1").unwrap();
    store.begin_for_session(1, "t1", Some("s1"));

    let guard = store.git.as_ref().unwrap().op_lock.lock();
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let store = store.clone();
        std::thread::spawn(move || {
            let report = store.conflict_scan_scoped(1, Some("s1"));
            tx.send(report).unwrap();
        });
    }
    std::thread::sleep(THREAD_START_PAD);
    assert!(
        rx.recv_timeout(BLOCK_PROBE).is_err(),
        "冲突预检全树补充段未持 op_lock（串行语义丢失）"
    );
    drop(guard);
    let report = rx
        .recv_timeout(FINISH_LIMIT)
        .expect("放锁后 conflict_scan_scoped 未在时限内完成");
    assert!(
        report.conflicts.is_empty(),
        "同树无变更不应报冲突: {:?}",
        report.conflicts
    );
}

/// gc 重建**全程**持 op_lock：持锁窗口内临时重建库（`checkpoints.git.gc-tmp`）
/// 不得出现（旧形态只在换名窗口取锁，walk→rename 窗口内并发 begin 写入旧
/// objects 的影子对象随换名进 `.gc-old` 被 cleanup 销毁 = dangling tree）。
#[test]
fn gc_rebuild_holds_op_lock_full_duration() {
    let (_dir, store) = git_store();
    std::fs::write(store.root().join("f1.txt"), "v1").unwrap();
    store.begin_for_session(1, "t1", None);
    std::fs::write(store.root().join("f2.txt"), "v2").unwrap();
    store.begin_for_session(2, "t2", None);

    // keep 全关：只有 cur（protected）turn 存活 → turn1 可清、turn2 受保护。
    store.set_gc(
        nemesis_config::CheckpointGcConfig {
            enabled: true,
            keep_turns: 0,
            keep_days: 0,
        },
        Arc::new(Vec::<String>::new),
    );

    let guard = store.git.as_ref().unwrap().op_lock.lock();
    let tmp_dir = store.root().join("logs").join("checkpoints.git.gc-tmp");
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let store = store.clone();
        std::thread::spawn(move || {
            let report = store.gc_now();
            tx.send(report).unwrap();
        });
    }
    // 持锁窗口内轮询：tmp 库出现 = 重建绕锁开跑（BUG 复现）。
    let deadline = std::time::Instant::now() + BLOCK_PROBE;
    while std::time::Instant::now() < deadline {
        assert!(
            !tmp_dir.exists(),
            "gc 重建在未持 op_lock 时已开始（走查/临时库创建绕锁）"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(guard);
    let report = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("放锁后 gc 未在时限内完成");
    assert_eq!(report.backend, "git", "应为 git 影子形态重建");
    assert_eq!(
        report.dropped_turns, 1,
        "turn1 应被清扫（keep 全关，cur 保护）"
    );
    assert!(
        report.trees_kept >= 1,
        "cur turn 的 tree 必须进保留集重建: trees_kept={}",
        report.trees_kept
    );
    assert!(
        store.current_tree_hex().is_some(),
        "gc 换名后影子库仍应可用（对象集换名无撕裂）"
    );
}

/// 并发 begin 全拿到树：8 会话并发 begin_for_session，串行化后无
/// index.lock 竞态败者（旧形态败者 tree=None → rewind 落到更早 tree
/// 过度回退）。
#[test]
fn concurrent_begins_all_get_trees() {
    let (_dir, store) = git_store();
    let root = store.root().to_path_buf();
    let mut handles = Vec::new();
    for i in 0..8usize {
        let store = store.clone();
        let root = root.clone();
        handles.push(std::thread::spawn(move || {
            std::fs::write(root.join(format!("f{i}.txt")), format!("v{i}")).unwrap();
            store.begin_for_session(10 + i, format!("t{i}"), Some(&format!("s{i}")));
        }));
    }
    for h in handles {
        h.join().expect("并发 begin 线程崩溃");
    }
    let guard = store.inner.lock();
    for i in 0..8usize {
        let cp = guard
            .cur
            .get(&Some(format!("s{i}")))
            .unwrap_or_else(|| panic!("会话 s{i} 的 cur 桶缺失"));
        assert!(
            cp.tree.is_some(),
            "会话 s{i} 的 begin tree=None（index.lock 竞态败者）"
        );
    }
}
