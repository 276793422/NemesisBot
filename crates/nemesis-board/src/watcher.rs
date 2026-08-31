//! board.db 变更侦测（W2.5 数据推送的后端半）。
//!
//! 原理：`PRAGMA data_version` 在**其他连接**提交写事务时递增（本连接自身
//! 的写不 bump）。watcher 用一条从不写入的独立只读连接轮询该值——gateway
//! 进程内的全部写入方（WSAPI handler、集群 peer_chat 写回、autopilot cron、
//! 派发 sweep）都走 `BoardStore` 自己的连接（相对 watcher 是"其他连接"），
//! CLI 子命令再开一条连接更是另一个进程——**全部写入方都会被看见**，无需
//! 在每个写路径埋事件。
//!
//! 轮询循环（tokio）在 gateway 侧（nemesis-board 不依赖 tokio，与派发
//! sweep 同一先例）；这里只提供连接构造 + 读数两个纯原语。发现变化后由
//! gateway 向 SSE EventHub 发 `board-changed`，前端各面板 200ms 防抖刷新。

use rusqlite::Connection;
use std::path::Path;

/// 打开 watcher 专用的轮询连接：普通打开 + busy_timeout，**不跑 schema
/// 迁移**（`db::init_db` 会写 `user_version`——watcher 连接必须保持零写入，
/// 否则自己污染自己的 data_version 观察）。库文件不存在时 SQLite 建空库，
/// data_version 照常工作（文件级 pragma，不依赖表）；注意 SQLite 不建缺失
/// 的父目录，这里补 `create_dir_all`（文件系统操作，不算数据库写）。
pub fn open_conn(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("watcher create board dir failed: {e}"))?;
    }
    let conn = Connection::open(db_path)
        .map_err(|e| format!("watcher open board.db failed: {e}"))?;
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
