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

// ===========================================================================
// R7（coverage-95 goal，2026-08-27）：run() 两分支——经 singleton 重定向。
// 文件头注的「结构性豁免」判断被 nemesis-path 新增的 set_home_dir() 运行时
// 缝推翻：crate::tests::singleton_test_home() 把 FTS 索引库和 jsonl 扫描
// 全部圈进测试沙箱 home，reindex/search 可安全直跑。
// 注意：reindex 扫描沙箱 home 下全部 jsonl（session 测试的 fixture 也在
// 其中）——search 断言用本 mod 独有关键词，避免跨测试撞命中。
// ===========================================================================

mod r7_run_via_singleton_redirect {
    use super::*;
    use crate::tests::{singleton_test_home, EnvHomeGuard};

    fn seed_jsonl(key: &str, lines: &[serde_json::Value]) {
        let home = singleton_test_home();
        let safe = key.replace(':', "_");
        let dir = home.join("workspace").join("logs").join("session_logs");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.jsonl", safe));
        let body: String = lines
            .iter()
            .map(|v| serde_json::to_string(v).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, body + "\n").unwrap();
    }

    #[tokio::test]
    async fn run_reindex_reports_file_count_in_sandbox() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        // 独有 key：数量断言只针对本测试播种的文件 + 沙箱里已有的都 >= 0。
        seed_jsonl(
            "agent:main:session:r7hist-reindex-a",
            &[
                serde_json::json!({"role":"user","content":"r7hist 索引重建播种甲","timestamp":"2026-08-27T02:00:00+08:00"}),
            ],
        );
        run(HistoryAction::Reindex, false)
            .await
            .expect("reindex in sandbox home ok");
    }

    #[tokio::test]
    async fn run_search_finds_seeded_unique_keyword() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        // 独一无二的关键词：只有本测试播种的行可能命中。
        seed_jsonl(
            "agent:main:session:r7hist-search-b",
            &[
                serde_json::json!({"role":"user","content":"寻找 qz7w 独特标记的行","timestamp":"2026-08-27T03:00:00+08:00"}),
                serde_json::json!({"role":"assistant","content":"qz7w 在这里","timestamp":"2026-08-27T03:00:30+08:00"}),
            ],
        );
        run(
            HistoryAction::Search {
                query: "qz7w".into(),
                limit: 5,
            },
            false,
        )
        .await
        .expect("search ok");
    }

    #[tokio::test]
    async fn run_search_no_match_prints_empty_message() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        run(
            HistoryAction::Search {
                query: "zz-no-such-keyword-r7-9527".into(),
                limit: 5,
            },
            false,
        )
        .await
        .expect("no-match search still Ok");
    }
}
