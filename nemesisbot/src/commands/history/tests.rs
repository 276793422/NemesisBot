//! `history` CLI 测试（S11c，quality-hardening goal 冲刺 S11）。
//!
//! **结构性豁免声明（run() 全部 32 个 MISS 行）**：run() 的 Search/Reindex
//! 两个分支都调用 `nemesis_agent::history_search::{reindex_session_logs,
//! search, index_db_path}`，后者经进程级 `default_path_manager()` 单例
//! （`crates/nemesis-path/src/paths.rs:19` OnceLock，`home_dir` 私有无
//! setter；见 `history_search.rs:55-56/160-161`）解析 sessions_log 目录并
//! **写** FTS 索引库。测试二进制中该单例 home 取决于首个触碰它的测试
//! （本 crate session/status 等测试在无 env 竞态下会把它烤成 ~/.nemesisbot），
//! 在单测里调 run() 会向真实 ~/.nemesisbot 落索引文件——违反"绝不触碰
//! ~/.nemesisbot"的硬纪律，故整体豁免（L2 集成测试用真进程覆盖）。
//!
//! 这里钉的是可离线确定的部分：CLI 参数面（clap 派生的 limit 默认值 20、
//! HistoryAction 枚举形态），防止未来改坏命令入口。

use super::*;
use clap::FromArgMatches;

fn build_cli() -> clap::Command {
    let mut cmd =
        <HistoryAction as clap::Subcommand>::augment_subcommands(clap::Command::new("history"));
    cmd.build();
    cmd
}

#[test]
fn cli_search_limit_defaults_to_20() {
    let m = build_cli()
        .try_get_matches_from(["history", "search", "你好"])
        .expect("search <query> parses");
    let action = HistoryAction::from_arg_matches(&m).expect("subcommand matches");
    match action {
        HistoryAction::Search { query, limit } => {
            assert_eq!(query, "你好");
            assert_eq!(limit, 20, "--limit 缺省必须是 20（与 U20 设计一致）");
        }
        other => match other { HistoryAction::Search { .. } => unreachable!(), HistoryAction::Reindex => panic!("expected Search, got Reindex") },
    }
}

#[test]
fn cli_search_limit_accepts_explicit_value() {
    let m = build_cli()
        .try_get_matches_from(["history", "search", "kw", "--limit", "5"])
        .expect("explicit --limit parses");
    let action = HistoryAction::from_arg_matches(&m).expect("subcommand matches");
    match action {
        HistoryAction::Search { query, limit } => {
            assert_eq!(query, "kw");
            assert_eq!(limit, 5);
        }
        other => match other { HistoryAction::Search { .. } => unreachable!(), HistoryAction::Reindex => panic!("expected Search, got Reindex") },
    }
}

#[test]
fn cli_reindex_takes_no_positional_args() {
    let m = build_cli()
        .try_get_matches_from(["history", "reindex"])
        .expect("reindex parses");
    match HistoryAction::from_arg_matches(&m).unwrap() {
        HistoryAction::Reindex => {}
        HistoryAction::Search { .. } => panic!("expected Reindex, got Search"),
    }
}
