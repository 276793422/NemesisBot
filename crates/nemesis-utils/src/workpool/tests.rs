//! L19 WorkPool 设施测试（手法1-4 逐条钉子）。

use super::*;
use std::time::Duration;

// ---------- 手法1：CPU 准入 ----------

#[test]
fn cpu_admission_is_max_one_n_minus_one() {
    let n = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(1);
    let got = cpu_admission_permits();
    assert!(got >= 1, "至少 1 个许可: {got}");
    assert!(got <= n, "不超过物理核数: {got} > {n}");
    assert_eq!(got, (n.saturating_sub(1)).max(1), "公式 max(1, N-1)");
    // 三角色默认容量与准入一致性。
    assert_eq!(PoolLimits::cpu().permits, got);
    assert_eq!(PoolLimits::reader().permits, 2, "reader ≤2 多核不加");
    assert_eq!(PoolLimits::writer().permits, 1, "writer 串行");
}

// ---------- 手法3：双上限背压（spawn 形态） ----------

#[tokio::test]
async fn pending_cap_rejects_typed_not_queued() {
    let pool = WorkPool::new(PoolLimits {
        permits: 4,
        max_pending: 1,
        max_input_bytes: 1024,
    });
    // 第一个任务占满 pending 账本（账本在 submit 时同步预留，无需等 worker 起跑）。
    let (rel_tx, rel_rx) = oneshot::channel::<()>();
    let a = pool
        .try_submit(10, async move {
            let _ = rel_rx.await;
            1
        })
        .expect("首个提交成功");
    let err = pool
        .try_submit(10, async { 2 })
        .expect_err("pending 满 = 立即拒绝");
    assert_eq!(err.pending, 1);
    assert_eq!(err.max_pending, 1);
    assert_eq!(err.retained_input_bytes, 10);
    assert_eq!(err.max_input_bytes, 1024);
    let msg = err.to_string();
    assert!(msg.contains("rejected, not queued"), "{msg}");
    assert!(msg.contains("1/1"), "{msg}");

    // 释放后账本回落，提交恢复。
    let _ = rel_tx.send(());
    a.settle().await;
    let b = pool.try_submit(10, async { 3 }).expect("释放后可提交");
    assert_eq!(b.join().await, Ok(3));
}

#[tokio::test]
async fn input_bytes_cap_rejects_and_failed_reserve_leaves_no_leak() {
    let pool = WorkPool::new(PoolLimits {
        permits: 4,
        max_pending: 8,
        max_input_bytes: 100,
    });
    let err = pool.try_reserve(101).expect_err("超字节上限 = 立即拒绝");
    assert_eq!(err.retained_input_bytes, 0);
    assert_eq!(err.max_input_bytes, 100);
    // 失败预留不泄漏：随后小额提交照常成功。
    let r = pool.try_reserve(40).expect("失败预留零泄漏");
    assert_eq!(r.input_bytes(), 40);
    // 累计口径：40+40=80 可；+30=110 超。
    let _r2 = pool.try_reserve(40).expect("累计 80 可");
    assert!(pool.try_reserve(30).is_err(), "累计 110 超 100 拒绝");
    drop(_r2);
}

// ---------- 手法3：spawn 池借形（try_reserve + permits 共享准入） ----------

#[tokio::test]
async fn borrowed_shape_shares_admission_and_gauge() {
    let pool = WorkPool::new(PoolLimits {
        permits: 1,
        max_pending: 8,
        max_input_bytes: 1024,
    });
    let permits = pool.permits();
    let r1 = pool.try_reserve(5).expect("预留成功");
    // spawn 一个持有许可的 worker（借用形态的执行体）。
    let holder = tokio::spawn(async move {
        let _permit = permits.acquire_owned().await;
        tokio::time::sleep(Duration::from_millis(30)).await;
    });
    // 第二个执行体：预留成功（账本独立于许可），acquire 阻塞到持有者释放。
    let r2 = pool.try_reserve(5).expect("账本 pending 未满可预留");
    let p2 = pool.permits();
    let waiter = tokio::spawn(async move {
        let _permit = p2.acquire_owned().await;
        true
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!waiter.is_finished(), "许可被持有 → 第二执行体未获得准入");
    holder.await.unwrap();
    assert!(waiter.await.unwrap(), "持有者释放后准入恢复");
    drop(r1);
    drop(r2);
}

// ---------- 手法4：取消三事实分离 ----------

#[tokio::test]
async fn cancel_rejects_result_then_resources_settle() {
    let pool = WorkPool::new(PoolLimits {
        permits: 2,
        max_pending: 1,
        max_input_bytes: 1024,
    });
    let (rel_tx, rel_rx) = oneshot::channel::<()>();
    let mut a = pool
        .try_submit(10, async move {
            let _ = rel_rx.await;
            1
        })
        .expect("提交成功");
    // 事实一：取消 → join 拒绝（结果面）。join 解析即 worker 已停止
    // （guard 已随 future drop）——预留保留到 worker 停止的结构性锚点。
    a.cancel();
    let joined = a.join().await;
    assert!(joined.is_err(), "取消 = 结果拒绝");
    // 事实三：账本回落（max_pending=1 → 提交成功即释放证明）。
    let b = pool
        .try_submit(10, async { 3 })
        .expect("worker 停止后账本回落");
    assert_eq!(b.join().await, Ok(3));
    drop(rel_tx);
}

#[tokio::test]
async fn cancel_after_completion_does_not_retroact() {
    let pool = WorkPool::new(PoolLimits::cpu());
    let mut t = pool.try_submit(1, async { 42 }).expect("提交成功");
    // 让 worker 先跑完。
    tokio::time::sleep(Duration::from_millis(50)).await;
    t.cancel();
    // 事实二：执行结果独立于取消请求——完成后取消不追溯。
    assert_eq!(t.join().await, Ok(42));
}

#[tokio::test]
async fn drop_task_abandons_and_releases_eventually() {
    let pool = WorkPool::new(PoolLimits {
        permits: 2,
        max_pending: 1,
        max_input_bytes: 1024,
    });
    let (rel_tx, rel_rx) = oneshot::channel::<()>();
    let task = pool
        .try_submit(10, async move {
            let _ = rel_rx.await;
            1
        })
        .expect("提交成功");
    drop(task); // 放弃 = 取消
    drop(rel_tx); // 解除 worker 悬挂 → abort/完成 → 账本释放
    // 资源释放异步落定：轮询直到账本回落（提交成功）。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if pool.try_reserve(10).is_ok() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "2s 内账本未回落（drop 后资源释放失败）"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn worker_panic_propagates_through_join() {
    let pool = WorkPool::new(PoolLimits::cpu());
    let t = pool
        .try_submit(1, async { panic!("worker exploded") })
        .expect("提交成功");
    // panic 经 join 重放（spawn 形态）——内层 unwind 由外层 JoinError 捕获。
    let join_err = tokio::spawn(async move { t.join().await })
        .await
        .expect_err("内层 panic → 外层 JoinError（非 Ok 返回）");
    assert!(join_err.is_panic(), "内层 panic → 外层 JoinError");
    let payload = join_err.into_panic();
    let msg = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(msg.contains("worker exploded"), "panic 消息重放: {msg}");
}

// ---------- 手法2：writer 池（串行保序） ----------

#[tokio::test]
async fn writer_pool_preserves_fifo_order() {
    let pool = WorkPool::new(PoolLimits::writer());
    let order = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mut tasks = Vec::new();
    for i in 0..6u64 {
        let order = Arc::clone(&order);
        // 逆序 sleep：若执行序被打乱，完成序必乱——保序由提交序钉死。
        let sleep_ms = (6 - i) * 20;
        tasks.push(
            pool.submit_ordered(8, async move {
                tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                order.lock().push(i);
                i
            })
            .expect("writer 提交成功"),
        );
    }
    for (i, t) in tasks.into_iter().enumerate() {
        assert_eq!(t.join().await, Ok(i as u64), "完成序=提交序");
    }
    assert_eq!(
        *order.lock(),
        vec![0, 1, 2, 3, 4, 5],
        "执行序=提交序（严格 FIFO）"
    );
}

#[tokio::test]
async fn writer_pool_overload_and_queued_cancel() {
    let pool = WorkPool::new(PoolLimits {
        permits: 1,
        max_pending: 2,
        max_input_bytes: 1024,
    });
    let (rel_tx, rel_rx) = oneshot::channel::<()>();
    let a = pool
        .submit_ordered(8, async move {
            let _ = rel_rx.await;
            'a'
        })
        .expect("A 占住执行位");
    // 让 A 真正起跑（驱动出队）。
    tokio::time::sleep(Duration::from_millis(30)).await;
    let mut b = pool
        .submit_ordered(8, async { 'b' })
        .expect("B 排队（pending 2/2）");
    let err = pool
        .submit_ordered(8, async { 'c' })
        .expect_err("队列满 = 立即拒绝");
    assert_eq!(err.max_pending, 2);
    assert!(err.to_string().contains("2/2"), "{err}");

    // 排队中的 B 取消：驱动推进到 B 时查 flag 跳过，join 拒绝。
    b.cancel();
    let _ = rel_tx.send(()); // A 先完成 → 驱动推进 → B 被跳过（join 否则会等到 A 完成）
    let r = b.join().await;
    assert!(r.is_err(), "排队中被取消 = 结果拒绝");

    // A 不受影响（身份核验：cancel(B) 不能误杀执行中的 A）。
    assert_eq!(a.join().await, Ok('a'), "cancel 排队任务不误杀执行中任务");
}

#[tokio::test]
async fn writer_pool_cancel_running_task_releases_after_settle() {
    let pool = WorkPool::new(PoolLimits {
        permits: 1,
        max_pending: 4,
        max_input_bytes: 1024,
    });
    let (rel_tx, rel_rx) = oneshot::channel::<()>();
    let mut a = pool
        .submit_ordered(8, async move {
            let _ = rel_rx.await;
            'a'
        })
        .expect("A 提交");
    tokio::time::sleep(Duration::from_millis(30)).await; // A 起跑
    a.cancel(); // 中止执行中的 A
    a.settle().await; // 资源释放落定
    // 账本回落：后续提交成功。
    let b = pool
        .submit_ordered(8, async { 'b' })
        .expect("settle 后可提交");
    assert_eq!(b.join().await, Ok('b'));
    drop(rel_tx);
}

// ---------- 池生命周期边界 ----------

#[tokio::test]
async fn pool_clone_shares_gauge_and_admission() {
    let pool = WorkPool::new(PoolLimits {
        permits: 4,
        max_pending: 1,
        max_input_bytes: 1024,
    });
    let clone = pool.clone();
    let (rel_tx, rel_rx) = oneshot::channel::<()>();
    let a = pool
        .try_submit(10, async move {
            let _ = rel_rx.await;
            1
        })
        .expect("原池提交");
    // clone 共享同一账本：pending 满对 clone 一样拒绝。
    assert!(
        clone.try_submit(10, async { 2 }).is_err(),
        "账本跨 clone 共享"
    );
    let _ = rel_tx.send(());
    a.settle().await;
    assert!(clone.try_submit(10, async { 3 }).is_ok(), "释放后恢复");
}

#[test]
fn limits_normalization_clamps_zero() {
    let l = PoolLimits {
        permits: 0,
        max_pending: 0,
        max_input_bytes: 0,
    }
    .normalized();
    assert_eq!(
        l,
        PoolLimits {
            permits: 1,
            max_pending: 1,
            max_input_bytes: 1,
        }
    );
}

#[tokio::test]
async fn overloaded_display_carries_four_facts() {
    let err = PoolOverloaded {
        pending: 3,
        max_pending: 5,
        retained_input_bytes: 700,
        max_input_bytes: 1000,
    };
    let msg = err.to_string();
    assert!(msg.contains("3/5"), "{msg}");
    assert!(msg.contains("700/1000"), "{msg}");
}
