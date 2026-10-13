//! U-B 探针纯函数单测。探针主循环（tokio 循环 + provider 裸调用）与
//! resume 全链路属集成面——V15 E2E 真机演练覆盖（GLM 端口改错 → 封存 →
//! 恢复 → 探针解封续做）；此处只钉可独立判定的决策函数。

use super::{probe_tick_secs, resume_dispatchable};

#[test]
fn tick_granularity_clamped() {
    // 极小值按 5s 下限（不空转）。
    assert_eq!(probe_tick_secs(0), 5);
    assert_eq!(probe_tick_secs(3), 5);
    assert_eq!(probe_tick_secs(5), 5);
    // 中间值原样。
    assert_eq!(probe_tick_secs(7), 7);
    assert_eq!(probe_tick_secs(29), 29);
    // 大值按 30s 上限（estop/放行状态变化快速感知）。
    assert_eq!(probe_tick_secs(30), 30);
    assert_eq!(probe_tick_secs(300), 30);
    assert_eq!(probe_tick_secs(3600), 30);
}

#[test]
fn resume_dispatch_requires_master_and_known_action() {
    // 三种已知动作 master 全可发起。
    for p in ["review", "plan", "project"] {
        assert!(resume_dispatchable(p, true), "{p} on master");
    }
    // worker 镜像一律不发起（board 写路径 master 专属）。
    for p in ["review", "plan", "project"] {
        assert!(!resume_dispatchable(p, false), "{p} on worker");
    }
    // 未知 pending 不猜（档案保留人工处理）。
    assert!(!resume_dispatchable("", true));
    assert!(!resume_dispatchable("unknown", true));
    assert!(!resume_dispatchable("Review", true)); // 大小写敏感（写侧契约恒小写）
}
