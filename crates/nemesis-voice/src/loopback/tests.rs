//! Tests for `loopback`（S12 覆盖率冲刺，2026-08-26；R6 增补 2026-08-27）。
//!
//! 可达面：slot 管理 + 幂等早退 + 停止标志（预置 slot 状态绕开真实采集）；
//! R6 增补真线程生命周期测试（start → spawn → stop → 线程自清 slot）。
//!
//! 结构性豁免：`run_loopback_inner` 的 WASAPI 深层（真 Render 设备 loopback
//! capture 流 + bytes→mono 降混 + far_end 灌入）——真声卡硬件。真线程测试
//! 在有声卡机器上会走到事件循环（100ms 周期），无声卡机器走 Err 早退，
//! 两种机器态都只断言 slot 生命周期（对两条路径都成立）。

use super::*;
use std::sync::atomic::AtomicBool;

/// 所有触碰 loopback_slot 全局态的测试必须先拿这把锁串行（同二进制并行默认）。
/// 已有测试预置 slot、新测试真启动线程——并行会互相踩（早退/清 slot 竞争）。
static LOOPBACK_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn stop_loopback_without_start_is_noop() {
    let _guard = LOOPBACK_TEST_LOCK.lock().unwrap();
    // slot 为 None → if let 不命中 → 直接返回（不 panic）
    stop_loopback();
}

#[test]
fn start_loopback_already_running_returns_early_without_spawning() {
    let _guard = LOOPBACK_TEST_LOCK.lock().unwrap();
    // 33 行 tracing::debug! 参数求值需要 subscriber（lcov 机制，见 test_util.rs）
    crate::test_util::ensure_global_subscriber();
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

#[test]
fn start_loopback_spawns_thread_and_stop_clears_slot() {
    // R6：真线程生命周期。start → spawn 成功则 slot=Some(flag)；
    // run_loopback 结束（有声卡：stop 置位后事件循环 ≤100ms 退出；
    // 无声卡：get_default_device Err 早退）后线程把 slot 清回 None。
    let _guard = LOOPBACK_TEST_LOCK.lock().unwrap();
    *loopback_slot().lock().unwrap() = None; // 从干净态开始

    start_loopback();
    // start_loopback 在 spawn 前同步置位 slot——"start 后 slot 必为 Some"
    // 的断言必须在 sleep 前做：无渲染设备的机器上 run_loopback 线程会立即
    // Err 早退并自清 slot，sleep 后再断言就成了对机器音频态的依赖
    // （2026-08-29 无声卡态下必失败，R6 增补时的时序缺陷）。
    {
        let slot = loopback_slot().lock().unwrap();
        assert!(slot.is_some(), "slot must be Some after start_loopback");
    }
    // 给线程时间进入事件循环（有声卡）或完成 Err 早退（无声卡）。
    std::thread::sleep(std::time::Duration::from_millis(300));

    stop_loopback();

    // 轮询等线程自清（有声卡 stop 后 ≤100ms + 收尾；无声卡可能已早退清过了）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        {
            let slot = loopback_slot().lock().unwrap();
            if slot.is_none() {
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "loopback thread did not clear its slot within 5s after stop"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // 33 行贯穿区：slot 持有的是「已请求停止」的标志（true）→ 幂等早退不命中，
    // 继续走重新拉起路径（新标志 + 真线程）。stop 后同样等自清。
    {
        let mut slot = loopback_slot().lock().unwrap();
        *slot = Some(Arc::new(AtomicBool::new(true)));
    }
    start_loopback();
    {
        let slot = loopback_slot().lock().unwrap();
        let flag = slot.as_ref().expect("restart must install a new flag");
        assert!(!flag.load(Ordering::SeqCst), "new flag must start un-stopped");
    }
    stop_loopback();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        {
            let slot = loopback_slot().lock().unwrap();
            if slot.is_none() {
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "restarted loopback thread did not clear its slot within 5s after stop"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
