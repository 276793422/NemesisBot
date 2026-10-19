//! wait_for_ready 三臂 + 纪律单元测试（纯逻辑，不起真实子进程）：
//! 就绪即返 / 早死带现场 / 到点 Timeout，外加「探针超窗取消重试不挂死」
//! 与「就绪达成前存活断言必须每 tick 在场」两条纪律锁。

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 就绪探针首次即中：Ok 直接返回，存活断言从未参与。
#[tokio::test]
async fn ready_on_first_poll_returns_immediately() {
    let aliveness_calls = Arc::new(AtomicUsize::new(0));
    let calls = aliveness_calls.clone();
    let mut poll = || std::future::ready(Some(42));
    let mut aliveness = || {
        calls.fetch_add(1, Ordering::Relaxed);
        std::future::ready(None::<String>)
    };
    let v = wait_for_ready(
        Duration::from_millis(50),
        Duration::from_secs(5),
        &mut poll,
        &mut aliveness,
    )
    .await
    .expect("first poll ready");
    assert_eq!(v, 42);
    assert_eq!(aliveness_calls.load(Ordering::Relaxed), 0);
}

/// A4 纪律核心：子进程早死 = 立即 ChildDied 带调用方给的现场，绝不傻等
/// deadline（deadline 5s，断言总耗不受虚拟时钟影响，这里 time 自动推进）。
#[tokio::test(start_paused = true)]
async fn child_died_surfaces_detail_immediately() {
    let mut poll = || std::future::ready::<Option<()>>(None);
    let mut aliveness = || std::future::ready(Some("exit code: 31337 (startup panic)".to_string()));
    let err = wait_for_ready(
        Duration::from_millis(100),
        Duration::from_secs(5),
        &mut poll,
        &mut aliveness,
    )
    .await
    .expect_err("dead child must fail fast");
    assert_eq!(
        err,
        ReadyError::ChildDied {
            detail: "exit code: 31337 (startup panic)".to_string()
        }
    );
    assert!(
        err.to_string().contains("31337"),
        "Display must carry the exit-code detail: {err}"
    );
}

/// 子进程全程存活但一直不就绪 → 到点 Timeout（而非 ChildDied / 挂死）。
#[tokio::test(start_paused = true)]
async fn timeout_when_child_stays_alive_but_never_ready() {
    let mut poll = || std::future::ready::<Option<()>>(None);
    let mut aliveness = || std::future::ready(None::<String>);
    let err = wait_for_ready(
        Duration::from_millis(100),
        Duration::from_secs(2),
        &mut poll,
        &mut aliveness,
    )
    .await
    .expect_err("deadline must produce Timeout");
    assert_eq!(err, ReadyError::Timeout);
}

/// 探针前两轮 None、第三轮 Some：重试语义成立（真实轮询到达形态）。
#[tokio::test]
async fn poll_retried_until_ready() {
    let n = Arc::new(AtomicUsize::new(0));
    let n_poll = n.clone();
    let mut poll = move || {
        let k = n_poll.fetch_add(1, Ordering::Relaxed);
        std::future::ready(if k >= 2 { Some("ready") } else { None })
    };
    let mut aliveness = || std::future::ready(None::<String>);
    let v = wait_for_ready(
        Duration::from_millis(20),
        Duration::from_secs(5),
        &mut poll,
        &mut aliveness,
    )
    .await
    .expect("third poll must win");
    assert_eq!(v, "ready");
    assert_eq!(n.load(Ordering::Relaxed), 3);
}

/// 探针单次超窗被取消（future 永不完成也挂不死 helper）：下一 tick 存活
/// 断言照常出场，早死照样即时暴露——这是管道 connect 挂等场景的纪律锁。
#[tokio::test(start_paused = true)]
async fn poll_future_hanging_is_cancelled_aliveness_still_runs() {
    let mut poll = || std::future::pending::<Option<()>>();
    let mut aliveness = || std::future::ready(Some("exit code: 1".to_string()));
    let err = wait_for_ready(
        Duration::from_millis(100),
        Duration::from_secs(3),
        &mut poll,
        &mut aliveness,
    )
    .await
    .expect_err("hanging probe must not block the aliveness assertion");
    assert_eq!(
        err,
        ReadyError::ChildDied {
            detail: "exit code: 1".to_string()
        }
    );
}
