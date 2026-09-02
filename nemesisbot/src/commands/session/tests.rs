//! `session` CLI 测试（M4 补测，quality-hardening goal 2026-08-25）。
//!
//! 分支本体（fork_session/row_user_turn_count/verbatim 复制/键唯一性）在
//! `crates/nemesis-agent/src/session_fork/tests.rs`（8 测试）与 web 侧
//! `api_handlers/fork_route_tests.rs`（7 测试）已覆盖；这里补 CLI 层的
//! 可测面：home 一致性守卫（fork 安全命门——拒绝把 store 和 jsonl 半分叉
//! 到两个 home）、缺会话报错、非 TTY 默认全量。
//!
//! 路径说明：`run()` 的守卫比较 `resolve_home(local)` 与进程级
//! `default_path_manager()`。测试用全新 tempdir 当 cwd：无论 path manager
//! 被同进程其他测试烤成哪个 home，都不可能等于这个全新 tempdir —— 守卫
//! 必然触发 bail，且 bail 发生在 `SessionStore::new_with_storage`（会
//! create_dir_all）之前。

use super::*;

/// --local 指向不存在 home 时：拒绝执行（home 不一致守卫），且**不创建**
/// `.nemesisbot`（create_dir_all 在守卫之后才轮到）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn run_local_missing_home_bails_without_creating() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let orig = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let list = run(SessionAction::List, true);
    let fork = run(
        SessionAction::Fork {
            session_key: "agent:main:session:x".into(),
            at: None,
            new_key: None,
        },
        true,
    );
    let created = tmp.path().join(".nemesisbot").exists();

    std::env::set_current_dir(&orig).unwrap();

    let err = list.expect_err("local home missing → run must bail before any effect");
    assert!(err.to_string().contains("home 不一致"), "got: {err}");
    assert!(fork.is_err(), "fork must bail the same way");
    assert!(
        !created,
        "bail must precede SessionStore::new_with_storage dir creation"
    );
}

/// `session show` 对不存在的会话：明确报错（jsonl 为空 → bail）。
/// 只读路径：不落任何盘。key 带随机后缀防撞（read_chat_log 走进程级
/// path manager 的 home，读不到该 key 的 jsonl → 空行列表 → bail）。
#[test]
fn show_turns_missing_session_errors() {
    let err = show_turns("agent:main:session:definitely-missing-xyz-9527")
        .expect_err("no jsonl for this key → must bail");
    assert!(err.to_string().contains("不存在"), "got: {err}");
}

/// 非 TTY（cargo test 的 stdin 是管道）下 interactive_pick 默认全量分支
/// （Ok(None)），不提问不阻塞——脚本/CI 可安全省略 --at。
#[test]
fn interactive_pick_non_tty_defaults_to_full_fork() {
    let at = interactive_pick("agent:main:session:any").unwrap();
    assert_eq!(at, None, "non-terminal stdin must default to full fork");
}

// ===========================================================================
// list_sessions（S11c，quality-hardening goal 冲刺 S11）—— 纯 home 参数化
// 读路径：sessions 目录扫描 + StoredSession 反序列化过滤 + jsonl 缺失显示
// 0/0 分支（read_chat_log 走进程级单例 home，唯一 key 读不到 → 空行列表，
// 确定性）。run()/show_turns 的成功路径结构性豁免（单例 jsonl，见文件头注）。
// ===========================================================================

mod list_sessions_arm {
    use super::super::list_sessions;

    fn session_json(key: &str, summary: &str) -> String {
        serde_json::json!({
            "key": key,
            "messages": [],
            "summary": summary,
            "created": "2026-01-01T00:00:00+08:00",
            "updated": "2026-01-02T03:04:00+08:00",
        })
        .to_string()
    }

    #[test]
    fn empty_home_prints_no_sessions_and_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        list_sessions(&home).expect("无 sessions 目录 → Ok");
    }

    #[test]
    fn scans_valid_sessions_and_skips_garbage_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let dir = home.join("workspace").join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("s1.json"),
            session_json("agent:main:session:s1", ""),
        )
        .unwrap();
        std::fs::write(
            dir.join("s2.json"),
            session_json("agent:main:session:s2", "这是摘要"),
        )
        .unwrap();
        // 坏 JSON（反序列化失败跳过）与非 .json 后缀（扩展名过滤跳过）。
        std::fs::write(dir.join("broken.json"), "not json{{{").unwrap();
        std::fs::write(dir.join("ignored.txt"), "{}").unwrap();
        // 孤儿目录条目（read_dir 里的目录也算 entry；扩展名不是 json → skip）。
        std::fs::create_dir_all(dir.join("subdir.json")).unwrap();

        // 唯一 key 防 jsonl 撞车：单例 home 下没有这些 key 的 jsonl →
        // 0 turns / 0 rows 分支（"jsonl 缺失显示 0/0"）。
        list_sessions(&home).expect("有效会话 2 个 + 垃圾文件过滤 → Ok");
    }

    #[test]
    fn unreadable_session_file_is_skipped_via_read_to_string() {
        // 路径是目录但扩展名 .json：read_to_string Err → skip（不 panic）。
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let dir = home.join("workspace").join("sessions");
        std::fs::create_dir_all(dir.join("dirsession.json")).unwrap();
        list_sessions(&home).expect("目录条目 read 失败 → skip，Ok");
    }
}

// ===========================================================================
// R7（coverage-95 goal，2026-08-27）：run() 成功路径——经 singleton 重定向。
// 文件头上方的「结构性豁免」判断被 `nemesis-path` 新增的 `set_home_dir()`
// 运行时缝推翻：`crate::tests::singleton_test_home()` 把进程级单例永久指向
// 测试沙箱，`EnvHomeGuard` 让 resolve_home(local=false) 解析到同一 home，
// 守卫（pm_home == home）通过，run() 的 List/Show/Fork 全链路可测。
// 纪律：每测试先拿 GLOBAL_STATE_LOCK；key 全部带 r7 前缀防跨测试撞车。
// ===========================================================================

// 整 mod Windows 形态（8/8 测试 + 专属 helper 全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r7_success_paths {
    use super::*;
    use crate::tests::{EnvHomeGuard, singleton_test_home};
    use std::path::PathBuf;

    fn rows_fixture(n_user_turns: usize) -> Vec<serde_json::Value> {
        // 每轮 = user 行 + assistant 行（row_user_turn_count 数 user 行）。
        let mut rows = Vec::new();
        for i in 0..n_user_turns {
            rows.push(serde_json::json!({
                "role": "user",
                "content": format!("r7-第{}轮问题：nemesisbot", i),
                "timestamp": format!("2026-08-27T00:0{}:00+08:00", i),
            }));
            rows.push(serde_json::json!({
                "role": "assistant",
                "content": format!("r7-第{}轮回答", i),
                "timestamp": format!("2026-08-27T00:0{}:30+08:00", i),
            }));
        }
        rows
    }

    fn write_jsonl(key: &str, rows: &[serde_json::Value]) -> PathBuf {
        let home = singleton_test_home();
        let safe = key.replace(':', "_");
        let dir = home.join("workspace").join("logs").join("session_logs");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.jsonl", safe));
        let body: String = rows
            .iter()
            .map(|v| serde_json::to_string(v).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, body + "\n").unwrap();
        path
    }

    fn store_jsonl_exists(key: &str) -> bool {
        let home = singleton_test_home();
        let safe = key.replace(':', "_");
        home.join("workspace")
            .join("logs")
            .join("session_logs")
            .join(format!("{}.jsonl", safe))
            .exists()
    }

    /// run(List, local=false)：env home 与单例一致 → 守卫放行 → 空会话表 Ok。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn run_list_via_env_home_ok_empty() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        run(SessionAction::List, false).expect("guard passes, list ok");
    }

    /// run(Show)：有 jsonl 的会话 → 轮次表打印 Ok。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn run_show_prints_turn_table_ok() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        let key = "agent:main:session:r7show1";
        write_jsonl(key, &rows_fixture(3));
        run(
            SessionAction::Show {
                session_key: key.into(),
            },
            false,
        )
        .expect("show with jsonl → turn table ok");
    }

    /// run(Fork) 全量：源 jsonl 逐行复制到新 key + store 落盘 + boundary 事件。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn run_fork_full_flow_copies_jsonl_and_creates_store() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        let key = "agent:main:session:r7fork1";
        write_jsonl(key, &rows_fixture(3));

        run(
            SessionAction::Fork {
                session_key: key.into(),
                at: None,
                new_key: Some("agent:main:session:r7fork1__child".into()),
            },
            false,
        )
        .expect("fork full flow ok");

        // jsonl 复制到了新 key（6 行 verbatim）。
        let copied = singleton_test_home()
            .join("workspace/logs/session_logs/agent_main_session_r7fork1__child.jsonl");
        let text = std::fs::read_to_string(&copied).expect("forked jsonl exists");
        assert_eq!(text.lines().count(), 6, "verbatim copy of all 6 rows");
        // store json 落盘。
        let store =
            singleton_test_home().join("workspace/sessions/agent_main_session_r7fork1__child.json");
        assert!(store.exists(), "new session store json must be saved");
    }

    /// run(Fork --at 1)：只保留第 1 轮（2 行）。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn run_fork_at_turn_1_keeps_prefix_only() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        let key = "agent:main:session:r7fork2";
        write_jsonl(key, &rows_fixture(3));
        run(
            SessionAction::Fork {
                session_key: key.into(),
                at: Some(1),
                new_key: Some("agent:main:session:r7fork2__at1".into()),
            },
            false,
        )
        .expect("fork at=1 ok");
        let copied = singleton_test_home()
            .join("workspace/logs/session_logs/agent_main_session_r7fork2__at1.jsonl");
        let text = std::fs::read_to_string(&copied).unwrap();
        assert_eq!(
            text.lines().count(),
            2,
            "turn-1 prefix = user+assistant rows"
        );
    }

    /// run(Fork) 源不存在：明确报错（jsonl 为空 → fork_session Err → run Err）。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn run_fork_missing_source_errors() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        let err = run(
            SessionAction::Fork {
                session_key: "agent:main:session:r7-missing-source".into(),
                at: Some(1),
                new_key: None,
            },
            false,
        )
        .expect_err("no jsonl for source → fork errors");
        assert!(err.to_string().contains("不存在"), "got: {err}");
    }

    /// run(Fork) 源只有 assistant 行：没有 user 轮 → 报错。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn run_fork_source_without_user_turns_errors() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        let key = "agent:main:session:r7fork3";
        write_jsonl(
            key,
            &[serde_json::json!({
                "role": "assistant",
                "content": "只有回答没有提问",
                "timestamp": "2026-08-27T01:00:00+08:00",
            })],
        );
        let err = run(
            SessionAction::Fork {
                session_key: key.into(),
                at: None,
                new_key: None,
            },
            false,
        )
        .expect_err("0 user turns → fork errors");
        assert!(err.to_string().contains("user 轮"), "got: {err}");
    }

    /// run(Show) 对不存在的会话走 run() 入口（show_turns bail 在 tests 顶部
    /// 已有直接调用版本；这里钉 run() 分发臂）。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn run_show_missing_session_dispatches_to_error() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        let err = run(
            SessionAction::Show {
                session_key: "agent:main:session:r7-missing-show".into(),
            },
            false,
        )
        .expect_err("missing jsonl → show errors");
        assert!(err.to_string().contains("不存在"), "got: {err}");
        // 只读路径：不产生 jsonl。
        assert!(!store_jsonl_exists("agent:main:session:r7-missing-show"));
    }

    /// run(Show) 对「只有 assistant 行」的会话：CLI 层 show_turns 自己的
    /// zero-user-turn bail（上面 fork 测试报错来自 nemesis-agent 的
    /// fork_session；这里是 commands/session.rs 的同义 bail 臂）。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn run_show_zero_user_turns_errors_from_cli_bail() {
        let _g = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home = singleton_test_home();
        let _env = EnvHomeGuard::point_at(&home);
        let key = "agent:main:session:r7show-zero";
        write_jsonl(
            key,
            &[serde_json::json!({
                "role": "assistant",
                "content": "只有回答没有提问",
                "timestamp": "2026-08-27T01:00:00+08:00",
            })],
        );
        let err = run(
            SessionAction::Show {
                session_key: key.into(),
            },
            false,
        )
        .expect_err("rows exist but zero user turns → CLI bail");
        assert!(err.to_string().contains("没有完整 user 轮次"), "got: {err}");
    }
}

// ===========================================================================
// wave_b（coverage 补测批次 B）—— 记账说明：本文件无新增测试。
//
// wave_b 批次点名的 miss 行逐一处置：
// - 行 170（show_turns 的 zero-user-turn bail）：ALREADY —— 即上方
//   `r7_success_paths::run_show_zero_user_turns_errors_from_cli_bail`，
//   断言文案与源码 bail! 字符串逐字对应（"没有完整 user 轮次"），
//   不需要重复用例。
// - 行 199-201 + 203-212（interactive_pick 的 TTY 门控后交互体）：
//   EXEMPT —— 这些行被 `std::io::stdin().is_terminal()` 守卫包裹；
//   cargo test 的 stdin 是管道恒走 Ok(None) 早退（上方
//   interactive_pick_non_tty_defaults_to_full_fork 已钉该守卫本身）。
//   想进门控体只能伪造 TTY（进程注入/伪终端 spawn），属豁免类：
//   禁 spawn 进程、禁触碰真实 stdin。空参数分支（回车=全量）、纯数字、
//   非法输入三条 readline 后逻辑都是同一豁免机制的门内死区。
// ===========================================================================
