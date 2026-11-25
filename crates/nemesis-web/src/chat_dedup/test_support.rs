//! chat_dedup 测试专用辅助面（串行闸 + 表/workspace 槽探针）。
//!
//! (BUG-20261126-083) 自 chat_dedup.rs 内联 `pub(crate) mod test_support`
//! 迁出（声明式，模块路径不变，消费方零改动）。声明侧 `#[cfg(test)]` 门控，
//! 生产构建整文件不编译。

use super::*;

/// 测试专用串行闸：进程级全局表 + workspace 槽，并行测试共用
/// （chat_event_log GLOBAL_TABLE_LOCK 同款纪律）。
pub static GLOBAL_TABLE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 测试专用 workspace 槽改写（None = 纯内存模式）。
pub fn set_workspace_for_tests(dir: Option<PathBuf>) {
    *WORKSPACE.write().unwrap() = dir;
}

/// 测试专用清表 + 清 workspace 槽（配合 set_workspace_for_tests(None)）。
pub fn reset_for_tests() {
    let mut t = table().lock().unwrap();
    t.map.clear();
    t.order.clear();
    drop(t);
    set_workspace_for_tests(None);
}

/// 测试专用：只清表内容（workspace 槽保留）——持久化测试清内存表模拟
/// 重启。字段私有，清理由本模块内聚完成。
pub fn clear_table_only() {
    let mut t = table().lock().unwrap();
    t.map.clear();
    t.order.clear();
}

/// 测试探针：map 大小（MAX_SESSIONS 逐出纪律回归锁用）。
pub fn table_size_for_tests() -> usize {
    table().lock().unwrap().map.len()
}

/// 测试探针：order 是否含指定键（map/order 同键集不变量回归锁用）。
pub fn order_contains_for_tests(session_key: &str) -> bool {
    table()
        .lock()
        .unwrap()
        .order
        .iter()
        .any(|k| k.as_str() == session_key)
}
