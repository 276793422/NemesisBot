//! BUG-7（2026-11-25 远程 home 取证）回归：rendezvous 名册循环连败指数
//! 退避在此钉死。此前固定 30s 一拍无限重试、每 10 连败打一条告警——中继
//! 不可达数天刷 2000+ 条 WARN/ERROR 淹没信号（本案取证干扰判读）。
//!
//! 钉死的语义（[`registry_client::backoff_after_fail`] 纯函数 + 循环日志
//! 条件由它派生）：
//! 1. 首败不追加延迟（2^0 = 1 倍）；第 N 连败 = base × 2^(N-1)；
//! 2. 封顶 5min（base 30s：30→60→120→240→300，第 5 连败触顶）；
//! 3. 封顶取 max(base, 5min)——用户显式配的慢节拍不被退避改短；
//! 4. 连败清零（成功）→ 回到 base。

use crate::registry_client::backoff_after_fail;

use std::time::Duration;

/// 语义 1+2：默认 30s 基拍的完整阶梯。
#[test]
fn backoff_ladder_default_base() {
    let base = Duration::from_secs(30);
    assert_eq!(backoff_after_fail(base, 0), base, "无失败 = 基拍");
    assert_eq!(
        backoff_after_fail(base, 1),
        Duration::from_secs(30),
        "首败不加码"
    );
    assert_eq!(backoff_after_fail(base, 2), Duration::from_secs(60));
    assert_eq!(backoff_after_fail(base, 3), Duration::from_secs(120));
    assert_eq!(backoff_after_fail(base, 4), Duration::from_secs(240));
    assert_eq!(
        backoff_after_fail(base, 5),
        Duration::from_secs(300),
        "第 5 连败触顶（480 → 封 300）"
    );
    assert_eq!(
        backoff_after_fail(base, 100),
        Duration::from_secs(300),
        "长连败恒封顶 5min"
    );
}

/// 语义 3：用户显式慢节拍（base > 5min）不被退避改短也不加码。
#[test]
fn backoff_respects_user_slow_base() {
    let base = Duration::from_secs(600);
    assert_eq!(backoff_after_fail(base, 1), base);
    assert_eq!(
        backoff_after_fail(base, 2),
        base,
        "base 超封顶时恒等基拍（不加码不改短）"
    );
    assert_eq!(backoff_after_fail(base, 50), base);
}

/// 语义 4：成功清零 → 回基拍（纯函数视角：streak 0 = base）。
/// 循环侧 fail_streak 归零路径（run_loop Ok 臂）就是这一语义。
#[test]
fn backoff_resets_to_base_after_success() {
    let base = Duration::from_secs(30);
    // 模拟：连败 5 次触顶后成功一次，下一拍必须回基拍。
    let _degraded = backoff_after_fail(base, 5);
    assert_eq!(backoff_after_fail(base, 0), base);
}
