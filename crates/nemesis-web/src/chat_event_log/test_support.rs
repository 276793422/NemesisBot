//! chat_event_log 测试专用串行闸。
//!
//! (BUG-20261126-083) 自 chat_event_log.rs 内联 `pub(crate) mod test_support`
//! 迁出（声明式，模块路径不变，消费方零改动）。声明侧 `#[cfg(test)]` 门控，
//! 生产构建整文件不编译。

/// 测试专用串行闸：SESSION_LOGS 是进程级全局表且带逐出——并行测试
/// 不互斥会互相逐掉对方会话（env-test-race 家族）。tokio Mutex：
/// async 测试持锁跨 await 合法（clippy await_holding_lock 不碰 tokio 闸）。
pub static GLOBAL_TABLE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
