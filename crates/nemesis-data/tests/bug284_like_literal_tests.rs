//! `query_logs` 用户搜索词 LIKE 字面化回归测试。
//!
//! 口径对齐 board /BUG-163：`model` / `session_key` 是用户可输入的
//! 子串搜索词，`%`/`_` 不转义时按 SQL LIKE 通配符解释——搜字面 `_`
//! 误命中无关行、搜字面 `%` 变全量。转义 + `ESCAPE '\'` 成对后，
//! 搜索词只能按字面命中。

use nemesis_data::{DataStore, LogFilter, RequestLog};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// 与 unit_tests.rs / m5_session_usage_tests.rs 同款唯一路径（pid + 进程内
/// 单调计数器，不用时间戳——Windows 计时精度窗口碰撞教训）。
fn temp_db_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nemesis_data_bug284_like_literal_test_{}_{}.db",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    let _ = fs::remove_file(&path);
    path
}

fn log(model: &str, session_key: &str) -> RequestLog {
    RequestLog {
        trace_id: format!("bug284-{model}-{session_key}"),
        model: model.to_string(),
        provider_type: "test".to_string(),
        status_code: 200,
        session_key: session_key.to_string(),
        ..Default::default()
    }
}

/// 三条账本行覆盖通配字符的两种角色：
/// A：model 含字面 `_`（下划线命中的目标行）
/// B：model 同形但对应位置是字母 `x`（未转义时 `_` 会误命中它）
/// C：model 含字面 `%`（百分号命中的目标行）
fn seed(store: &DataStore) {
    store
        .insert_request_log(&log("test_a", "agent_x1"))
        .unwrap();
    store
        .insert_request_log(&log("testxb", "agent-x1"))
        .unwrap();
    store.insert_request_log(&log("100%", "sk%pct")).unwrap();
}

fn models(rows: &[RequestLog]) -> Vec<&str> {
    rows.iter().map(|r| r.model.as_str()).collect()
}

#[test]
fn model_search_underscore_matches_only_literal() {
    let store = DataStore::open(&temp_db_path()).expect("open db");
    seed(&store);

    // 精确词 `test_a`：只有 A（未转义时 `_` 当单字符通配，B 也命中——
    // 本测试的回归钉核心）。
    let (rows, total) = store
        .query_logs(
            0,
            i64::MAX,
            1,
            50,
            &LogFilter {
                model: Some("test_a".into()),
                ..Default::default()
            },
        )
        .expect("query");
    assert_eq!(total, 1, "搜 `test_a` 只命中 A，不误命中 B");
    assert_eq!(models(&rows), vec!["test_a"]);

    // 裸 `_`：只命中含字面下划线的行（未转义时等价 `%_%` 全量）。
    let (rows, total) = store
        .query_logs(
            0,
            i64::MAX,
            1,
            50,
            &LogFilter {
                model: Some("_".into()),
                ..Default::default()
            },
        )
        .expect("query");
    assert_eq!(total, 1, "裸 `_` 按字面命中，非通配全量");
    assert_eq!(models(&rows), vec!["test_a"]);
}

#[test]
fn model_search_percent_matches_only_literal() {
    let store = DataStore::open(&temp_db_path()).expect("open db");
    seed(&store);

    // 裸 `%`：只命中含字面百分号的行（未转义时等价 `%%%` = 全量 3 行）。
    let (rows, total) = store
        .query_logs(
            0,
            i64::MAX,
            1,
            50,
            &LogFilter {
                model: Some("%".into()),
                ..Default::default()
            },
        )
        .expect("query");
    assert_eq!(total, 1, "裸 `%` 按字面命中，非通配全量");
    assert_eq!(models(&rows), vec!["100%"]);
}

#[test]
fn session_key_search_underscore_matches_only_literal() {
    let store = DataStore::open(&temp_db_path()).expect("open db");
    seed(&store);

    // `agent_`：只有 A 的会话键含字面下划线；B 是 `agent-x1`，未转义时
    // `_` 会让它也命中。
    let (rows, total) = store
        .query_logs(
            0,
            i64::MAX,
            1,
            50,
            &LogFilter {
                session_key: Some("agent_".into()),
                ..Default::default()
            },
        )
        .expect("query");
    assert_eq!(total, 1, "搜 `agent_` 只命中 A 会话");
    assert_eq!(models(&rows), vec!["test_a"]);
}

#[test]
fn no_wildcards_search_still_matches_normally() {
    let store = DataStore::open(&temp_db_path()).expect("open db");
    seed(&store);

    // 无通配字符的普通子串照常子串匹配（修复不改变正常搜索语义）。
    let (rows, total) = store
        .query_logs(
            0,
            i64::MAX,
            1,
            50,
            &LogFilter {
                model: Some("test".into()),
                ..Default::default()
            },
        )
        .expect("query");
    assert_eq!(total, 2, "普通子串 `test` 命中 A/B 两行");
    // created_at 全 0 时 ORDER BY 的并列顺序是实现细节，断言排序后集合。
    let mut got = models(&rows)
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    got.sort();
    assert_eq!(got, vec!["test_a".to_string(), "testxb".to_string()]);
}
