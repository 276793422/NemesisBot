//! (BUG-20261115-027) 全库日志捕获测试互斥闸。
//!
//! tracing callsite interest 缓存是进程级单值：任一捕获测试的
//! guard drop 触发全 callsite 重建，此刻该线程栈已无 subscriber，
//! 重建可把二进制内全部 callsite（含他测正在 emit 的那条）打成
//! NEVER，与对方「set_default→emit」窗口重叠即假红（buf 全空，
//! 断言全过——nemesis-state workspace_state CI coverage 实证同族）。
//! signature 与 clamav::updater 的捕获测试同属本测试二进制，必须
//! 共用本闸串行；guard 须锁内 drop。同步测试用 `blocking_lock()`
//! （#[test] 无 runtime 上下文，合法）；异步捕获测试用
//! `lock().await`（tokio Mutex guard 跨 await 不触发
//! await_holding_lock——std/parking_lot 会开火）。

// (BUG-20261126-083) 自 security/lib.rs 内联 mod 迁出（声明式，模块路径
// `crate::test_capture_sync::SERIAL` 不变，消费方零改动）。
pub(crate) static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
