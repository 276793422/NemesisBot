//! Tests for `loopback`（S12 覆盖率冲刺，2026-08-26）。
//!
//! 可达面：slot 管理 + 幂等早退 + 停止标志。通过**预置 slot 状态**绕开
//! 真实采集线程——`start_loopback` 在"已在跑"时直接返回，不 spawn 线程，
//! 不碰 WASAPI。
//!
//! 结构性豁免：`start_loopback` 的 spawn 臂 + `run_loopback`/
//! `run_loopback_inner`（initialize_mta + 默认 Render 设备 COM 初始化 +
//! loopback capture 流）——真声卡硬件，且停止标志是 spawn 内部新建的
//! Arc（外部无法预置），无注入缝。

use super::*;
use std::sync::atomic::AtomicBool;

#[test]
fn stop_loopback_without_start_is_noop() {
    // slot 为 None → if let 不命中 → 直接返回（不 panic）
    stop_loopback();
}

#[test]
fn start_loopback_already_running_returns_early_without_spawning() {
    // 预置"运行中"标志（false = 未请求停止）→ start 走幂等早退，
    // 绝不 spawn 真实 WASAPI 线程
    {
        let mut slot = loopback_slot().lock().unwrap();
        *slot = Some(Arc::new(AtomicBool::new(false)));
    }
    start_loopback(); // 若误 spawn 会真开声卡采集——早退则无副作用

    // stop 置位停止标志（true 分支）
    stop_loopback();
    {
        let slot = loopback_slot().lock().unwrap();
        let flag = slot.as_ref().expect("slot must still hold the flag");
        assert!(flag.load(Ordering::SeqCst), "stop must set the flag");
    }

    // 清理，避免污染其他测试
    *loopback_slot().lock().unwrap() = None;
}
