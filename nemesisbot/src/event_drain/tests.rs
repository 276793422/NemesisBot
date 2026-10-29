//! `event_drain` 测试：排干契约——Lagged（环形缓冲覆盖）不得终止排干。

use super::*;

/// 红证场景（旧 `while let Ok` 形态在此 FAILED）：消费落后 → 缓冲被覆盖 →
/// 首次 `try_recv()` 返回 `Err(Lagged)`——旧形态在这一点提前退出，尾部
/// 存活事件（典型：配对的 tool_end）全部丢失。正确契约：Lagged 后继续
/// 排干，读到最旧存活事件起的全部尾部。
#[tokio::test]
async fn drain_continues_past_lagged_and_reads_tail() {
    let (tx, mut rx) = tokio::sync::broadcast::channel::<usize>(8);
    // 先发 6 条，消费 3 条（游标停在 3，缓冲持有 0..6）。
    for i in 0..6 {
        tx.send(i).unwrap();
    }
    assert_eq!(rx.try_recv().unwrap(), 0);
    assert_eq!(rx.try_recv().unwrap(), 1);
    assert_eq!(rx.try_recv().unwrap(), 2);
    // 覆盖式灌入 15 条（6..=21）：容量 8 < 15 → 游标 3..13 区间被覆盖。
    for i in 6..21 {
        tx.send(i).unwrap();
    }
    let drained = drain_residual(&mut rx);
    assert!(
        !drained.is_empty(),
        "Lagged 后尾部存活事件必须可读（旧 while-let-Ok 形态在此返回空）"
    );
    // 缓冲 8 容量持有最旧存活 13..21；游标 3 之后的 3..13 已覆盖 →
    // Lagged 跳到 13，排干应恰好得到 13..21。
    assert_eq!(drained, (13..21).collect::<Vec<_>>());
}

/// 无覆盖的正常排干：全部可读事件按序交付，Empty 终止。
#[tokio::test]
async fn drain_reads_all_without_lag_and_stops_at_empty() {
    let (tx, mut rx) = tokio::sync::broadcast::channel::<usize>(32);
    for i in 0..5 {
        tx.send(i).unwrap();
    }
    drop(tx); // Closed 兜底语义：sender 全 drop 后已入缓冲事件仍可读尽
    let drained = drain_residual(&mut rx);
    assert_eq!(drained, (0..5).collect::<Vec<_>>());
}

/// 第二次排干：排干到 Empty 后再次调用 = 空（不重复、不挂起）。
#[tokio::test]
async fn drain_is_idempotent_at_empty() {
    let (tx, mut rx) = tokio::sync::broadcast::channel::<usize>(4);
    tx.send(7).unwrap();
    assert_eq!(drain_residual(&mut rx), vec![7]);
    assert!(drain_residual(&mut rx).is_empty());
}
