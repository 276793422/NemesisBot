//! `nemesisbot history search` — U20 (sixth batch) CLI over the session
//! full-text index (`nemesis_agent::history_search`).

use crate::common;
use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum HistoryAction {
    /// Full-text search across all session chat logs.
    Search {
        /// Search text (Chinese or English).
        query: String,
        /// Max hits (default 20).
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Force a full re-index of the session logs (mtime-incremental
    /// normally handles this; `reindex` rebuilds from scratch after schema
    /// changes or a deleted index db).
    Reindex,
}

pub async fn run(action: HistoryAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);

    match action {
        HistoryAction::Search { query, limit } => {
            // Lazy index (first call full-scans, later calls incremental).
            let changed = nemesis_agent::history_search::reindex_session_logs();
            if changed > 0 {
                println!("（本次新索引 {} 个会话文件）", changed);
            }
            let hits = nemesis_agent::history_search::search(&query, limit);
            if hits.is_empty() {
                println!("没有找到匹配「{}」的历史消息。", query);
                return Ok(());
            }
            println!("找到 {} 条匹配「{}」：", hits.len(), query);
            for h in &hits {
                println!(
                    "  [{}] {}  (seq={} time={})",
                    h.role, h.session_key, h.seq, h.timestamp
                );
                println!("    {}", h.snippet.replace('\n', " "));
            }
            println!("\n（home: {}）", home.display());
        }
        HistoryAction::Reindex => {
            let n = nemesis_agent::history_search::reindex_session_logs();
            println!(
                "重建索引完成：{} 个文件，索引库 {}",
                n,
                nemesis_agent::history_search::index_db_path().display()
            );
        }
    }
    Ok(())
}

// S11c（quality-hardening goal 冲刺 S11）：声明式测试挂载，无内联测试。
// run() 两个分支都经 nemesis_agent::history_search 的进程级单例
// （default_path_manager OnceLock，无 setter）读写索引库——测试二进制里
// 单例 home 非确定（取决于首个触碰它的测试），可能写进真实 ~/.nemesisbot，
// 属结构性不可隔离，详见 tests.rs 头注。
#[cfg(test)]
mod tests;
