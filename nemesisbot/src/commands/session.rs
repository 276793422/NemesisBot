//! `nemesisbot session` — Z1 (Phase4-d) 会话分支 CLI。
//!
//! `session fork` 把一个会话在选定轮次边界处分支成新会话（真分支，非
//! 回滚）：原会话不动，新会话拿到到该轮为止的完整上下文。**2026-08-25
//! 第三轮修复：轮次语义以 chat_log jsonl 为唯一真相源**（UI 渲染、弹窗
//! 计数、分叉切口三者同源）——分叉时 jsonl 前缀**逐行原样复制**，新会话
//! 的 SessionStore 用同一批行经共享映射重建（与自愈重建同构；store 只是
//! 可重建缓存，会被压缩折叠/被 7 天 TTL 删除，绝不能定义"第 N 轮"）。
//! 双方 boundary 事件照记。`session list` 列出可分支的会话（轮数按
//! jsonl 计），`session show` 展示一个会话的轮次边界表（选 `--at` 的
//! 辅助）。
//!
//! 网关可同时保持运行：fork 由本 CLI 进程直接落盘，运行中的网关靠
//! SessionStore 的「内存未命中→磁盘回退」（Z1 新增）在下一条消息时
//! 载入新会话，不会用空会话覆盖 fork 文件。

use crate::common;
use anyhow::{Result, bail};
use clap::Subcommand;
use nemesis_agent::session::SessionStore;
use nemesis_agent::session_fork::{fork_session, row_user_turn_count};
use std::io::IsTerminal;
use std::path::Path;

#[derive(Subcommand)]
pub enum SessionAction {
    /// List sessions in this home (key / turns / messages / updated).
    List,
    /// Show one session's turn-boundary table (helper for choosing --at).
    Show {
        /// Session key, e.g. agent:main:session:legacy
        session_key: String,
    },
    /// Fork a session at a turn boundary into a NEW session key.
    ///
    /// The source session is never modified (true branch, not a rollback).
    /// Without --at the whole history is copied; in a terminal, omitting
    /// --at opens an interactive turn picker.
    Fork {
        /// Source session key, e.g. agent:main:session:legacy
        session_key: String,
        /// Fork at this 1-based COMPLETE user turn (keeps turns 1..N;
        /// default: all turns).
        #[arg(long)]
        at: Option<usize>,
        /// Requested new session key (default: {source}__fork; suffixed
        /// _2/_3/... when taken).
        #[arg(long)]
        new_key: Option<String>,
    },
}

pub fn run(action: SessionAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);

    // The fork copies BOTH the SessionStore file (explicit home below) and
    // the chat_log jsonl (via the process-global path manager). Refuse to
    // half-fork into two different homes — in the deployed layout both
    // resolve to the exe-adjacent home, but a mismatched cwd/NEMESISBOT_HOME
    // combo could split them; fail loudly instead.
    let pm_home = nemesis_path::default_path_manager().home_dir();
    if pm_home != home {
        bail!(
            "home 不一致：SessionStore 将写入 {}，但 chat_log 路径解析到 {}。\n\
             请在与目标 home 一致的环境下运行（对应目录执行 / 设 NEMESISBOT_HOME）。",
            home.display(),
            pm_home.display()
        );
    }

    let store = SessionStore::new_with_storage(common::sessions_dir(&home));

    match action {
        SessionAction::List => list_sessions(&home),
        SessionAction::Show { session_key } => show_turns(&session_key),
        SessionAction::Fork {
            session_key,
            at,
            new_key,
        } => {
            let at = match at {
                Some(n) => Some(n),
                None => interactive_pick(&session_key)?,
            };
            let info = fork_session(&store, &session_key, new_key, at)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("✅ 会话分支完成：");
            println!("  源会话   : {}（未改动）", info.source_key);
            println!("  新会话   : {}", info.new_key);
            println!(
                "  边界     : 第 {} 轮（保留前 {} 行 / 源会话共 {} 行）",
                info.at_turn,
                info.kept_messages,
                info.kept_messages + info.dropped_messages
            );
            println!("  摘要缓存 : 未携带（分叉上下文以聊天记录行为准）");
            println!(
                "  聊天记录 : 逐行原样复制 {} 行（时间戳等原字段保真）",
                info.chat_log_lines
            );
            if let Some(ws_id) = info.new_key.strip_prefix("agent:main:session:") {
                println!(
                    "  打开方式 : WebSocket 消息 metadata.session_id={}（Dashboard/Chat 直接可用）",
                    ws_id
                );
            }
            println!("  home     : {}", home.display());
            Ok(())
        }
    }
}

/// `session list`：读 sessions 目录（元数据：key / updated / 摘要标签），
/// 轮数与行数按各会话的 chat_log jsonl 统计（2026-08-25 第三轮：jsonl 是
/// 轮次语义的真相源——store 被压缩/TTL 删过之后其条数不代表可分支内
/// 容）。jsonl 缺失（纯 store 残留）显示 0/0。按 updated 倒序。
fn list_sessions(home: &Path) -> Result<()> {
    let dir = common::sessions_dir(home);
    let mut rows: Vec<(String, usize, usize, String, String)> = Vec::new();
    if dir.is_dir() {
        for entry in std::fs::read_dir(&dir)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(data) = std::fs::read_to_string(&path) else { continue };
            let Ok(s) = serde_json::from_str::<nemesis_agent::session::StoredSession>(&data)
            else {
                continue;
            };
            let summary_tag = if s.summary.is_empty() {
                String::new()
            } else {
                format!("（摘要 {}字）", s.summary.chars().count())
            };
            let (log_rows, _, _, _) =
                nemesis_agent::chat_log::read_chat_log(&s.key, usize::MAX, None);
            rows.push((
                s.key.clone(),
                row_user_turn_count(&log_rows),
                log_rows.len(),
                s.updated.format("%Y-%m-%d %H:%M").to_string(),
                summary_tag,
            ));
        }
    }
    if rows.is_empty() {
        println!("（{} 下没有会话文件）", dir.display());
        return Ok(());
    }
    rows.sort_by(|a, b| b.3.cmp(&a.3));
    println!("{:<44} {:>6} {:>9}  {:<16} {}", "SESSION KEY", "TURNS", "ROWS", "UPDATED", "");
    for (key, turns, msgs, updated, tag) in &rows {
        println!("{:<44} {:>6} {:>9}  {:<16} {}", key, turns, msgs, updated, tag);
    }
    println!("\n（TURNS/ROWS 按 chat_log 统计；home: {}）", home.display());
    Ok(())
}

/// `session show`：轮次边界表 — 每个完整 user 轮一行（轮号 / 边界含义 /
/// 首条消息预览）。按 chat_log jsonl 行统计（与 fork 切口同源）。
fn show_turns(session_key: &str) -> Result<()> {
    let (rows, _, _, _) = nemesis_agent::chat_log::read_chat_log(session_key, usize::MAX, None);
    if rows.is_empty() {
        bail!("会话 {:?} 不存在或聊天记录（jsonl）为空（用 session list 查看可用 key）", session_key);
    }
    let turns = row_user_turn_count(&rows);
    if turns == 0 {
        bail!("会话 {:?} 没有完整 user 轮次，无可分支内容", session_key);
    }
    println!("会话 {} 共 {} 轮 / {} 行聊天记录：", session_key, turns, rows.len());
    let mut turn = 0usize;
    let mut preview: Option<&str> = None;
    let mut acc = 0usize;
    for v in &rows {
        let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
        if role == "user" {
            if let Some(p) = preview.take() {
                println!("  --at {:<3} 保留前 {} 轮（{} 行）  首条: {}", turn, turn, acc, p);
                acc = 0;
            }
            turn += 1;
            preview = v.get("content").and_then(|c| c.as_str());
        }
        acc += 1;
    }
    if let Some(p) = preview {
        println!("  --at {:<3} 保留前 {} 轮（{} 行）  首条: {}", turn, turn, acc, p);
    }
    println!("  （--at {} = 全量分支；省略 --at 默认全量）", turns);
    Ok(())
}

/// 交互选边界：仅在 TTY 下询问；非 TTY（脚本/管道）默认全量。
fn interactive_pick(session_key: &str) -> Result<Option<usize>> {
    if !std::io::stdin().is_terminal() {
        return Ok(None);
    }
    show_turns(session_key)?;
    print!("\n要分支到第几轮？（回车 = 全量分支）> ");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    match line.parse::<usize>() {
        Ok(n) if n >= 1 => Ok(Some(n)),
        _ => bail!("无效轮次 {:?}（需要 >= 1 的整数，或回车全量）", line),
    }
}
