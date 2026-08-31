//! nemesisbot_data.db 变更侦测（A3 请求明细实时刷新的后端半）。
//!
//! 原理与 `nemesis-board::watcher` 同源（W2.5 先例的拷贝适配）：
//! `PRAGMA data_version` 在**其他连接**提交写事务时递增（本连接自身的写不
//! bump）。watcher 用一条从不写入的独立连接轮询该值——gateway 进程内的
//! 全部写入方（AgentLoop / workflow LLM 节点经 `DataStore` 自己的连接落库）
//! 都会被看见，无需在每个写路径埋事件。
//!
//! 轮询循环（tokio）在 gateway 侧（nemesis-data 不依赖 tokio，与 board
//! watcher / 派发 sweep 同一先例）；这里只提供连接构造 + 读数两个纯原语。
//! 发现变化后由 gateway 向 SSE EventHub 发 `usage-changed`，前端请求明细
//! tab 200ms 防抖静默刷新。

use rusqlite::Connection;
use std::path::Path;

/// 打开 watcher 专用的轮询连接：普通打开 + busy_timeout，**不跑 schema
/// 迁移**（`db::init_db` 会写 `user_version` 并建表——watcher 连接必须保持
/// 零写入，否则自己污染自己的 data_version 观察；首次打开时库文件由
/// `DataStore::open` 负责初始化，这里晚于它执行）。注意 SQLite 不建缺失
/// 的父目录，这里补 `create_dir_all`（文件系统操作，不算数据库写）。
pub fn open_conn(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("watcher create data dir failed: {e}"))?;
    }
    let conn = Connection::open(db_path)
        .map_err(|e| format!("watcher open nemesisbot_data.db failed: {e}"))?;
    conn.execute_batch("PRAGMA busy_timeout = 5000;")
        .map_err(|e| format!("watcher set busy_timeout failed: {e}"))?;
    Ok(conn)
}

/// 读当前 data_version（其他连接的累计写入代数）。
pub fn data_version(conn: &Connection) -> Result<i64, String> {
    conn.pragma_query_value(None, "data_version", |row| row.get(0))
        .map_err(|e| format!("watcher read data_version failed: {e}"))
}

#[cfg(test)]
mod tests;
