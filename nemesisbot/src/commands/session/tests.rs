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
    assert!(!created, "bail must precede SessionStore::new_with_storage dir creation");
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
