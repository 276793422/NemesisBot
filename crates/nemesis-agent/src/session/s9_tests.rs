//! S9 覆盖率批次：session.rs 剩余未覆盖行。
//! - 445：自愈重建 info! 的 replayed 字段行（store json 缺 + chat_log 有
//!   → get_or_create 触发 rebuild，需 subscriber）。
//! - 545/547：add_message 的 capture 分支收尾（CaptureSink::global 初始化
//!   后 capture_on 为真）。
//! - 793-794/800：delete_session 的 json remove 失败 warn 字段行（路径
//!   预置为目录 → remove_file 失败非 NotFound）。
//! - 829-830：clear_session 同上。
//! - 861-883：migrate_legacy_main 的 storage_dir json 部分（读成功改 key /
//!   读失败 warn）。⚠️ 849-858 的 jsonl 分支走 default_path_manager 的真实
//!   sessions_log_dir——测试不可造 legacy 文件（触生产数据），见报告环境
//!   依赖组；真实目录存在 agent_main_main.jsonl 时本测试跳过（防误迁移）。
//! - 916-917/920：cleanup_old_sessions 的 read_dir 失败 warn（storage_dir
//!   指向普通文件）。
//! - 959-960/964/986：cleanup 过期文件删除失败 warn（readonly，探针门控）
//!   + 删除完成 info 字段行。
//! - 1548：force_compress_turns 的 info 字段行。
//! - 1512：force_compress_turns 的 conversation 空判——len>4 时切片恒非空，
//!   结构性不可达（见报告豁免组）。

use super::*;
use crate::test_support::capture_logs;
use nemesis_path::default_path_manager;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "nemesis_sess_s9_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ))
}

fn key(tag: &str) -> String {
    format!("test:s9:{}:{}", tag, std::process::id())
}

/// 自愈重建：store json 缺、chat_log 有 → get_or_create 走
/// rebuild_from_chat_log（443-448 info 字段行求值）。
#[test]
fn self_heal_rebuild_logs_replayed_fields() {
    let _logs = capture_logs();
    let k = key("heal");
    crate::chat_log::delete_chat_log(&k);
    let store = SessionStore::new_with_storage(temp_dir("heal"));

    crate::chat_log::append_chat_log(&k, "user", "s9 heal question");
    crate::chat_log::append_chat_log(&k, "assistant", "s9 heal answer");

    let sess = store.get_or_create(&k);
    assert!(
        sess.messages
            .iter()
            .any(|m| m.content.contains("s9 heal question")),
        "rebuilt from chat_log"
    );
    store.delete_session(&k);
    crate::chat_log::delete_chat_log(&k);
    let _ = std::fs::remove_dir_all(temp_dir("heal"));
}

/// （曾有一个 add_message capture 分支测试在此调用 CaptureSink::init，
/// 2026-08-26 删除：CaptureSink::GLOBAL 是进程级 OnceLock，session/tests.rs
/// 的 test_capture_records_session_writes_when_enabled 自称「本测试二进制
/// 唯一 init 调用者」并断言其临时目录下的落盘文件；两个 init 竞争会让
/// 先跑的一方把全局指到已 drop 的 tempdir → 对方 read_dir NotFound。
/// 该测试已覆盖 add_message 的 capture 分支，此处不再重复初始化全局。）
///
/// delete_session：json 路径预置为目录 → remove 失败非 NotFound → warn
/// 字段行（793-795）+ 块收尾（800）。
#[test]
fn delete_session_json_path_blocked_by_directory_warns() {
    let _logs = capture_logs();
    let dir = temp_dir("delblock");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = SessionStore::new_with_storage(&dir);
    let k = key("delblock");
    let blocked = dir.join(format!("{}.json", sanitize_filename(&k)));
    std::fs::create_dir_all(&blocked).unwrap();

    let existed = store.delete_session(&k); // 必须不 panic
    assert!(!existed, "never in memory");
    assert!(blocked.is_dir(), "blocker intact");

    let _ = std::fs::remove_dir_all(&dir);
}

/// clear_session：同上（829-831）。
#[test]
fn clear_session_json_path_blocked_by_directory_warns() {
    let _logs = capture_logs();
    let dir = temp_dir("clrblock");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = SessionStore::new_with_storage(&dir);
    let k = key("clrblock");
    let blocked = dir.join(format!("{}.json", sanitize_filename(&k)));
    std::fs::create_dir_all(&blocked).unwrap();

    store.clear_session(&k); // 必须不 panic
    assert!(blocked.is_dir(), "blocker intact");
    let _ = std::fs::remove_dir_all(&dir);
}

/// migrate_legacy_main 的 storage_dir json 部分：读成功改 key 迁移
/// （863-880）；读失败（路径是目录）warn（881）。
#[test]
fn migrate_legacy_main_moves_storage_json() {
    // 守卫：真实 sessions_log_dir 若存在 legacy jsonl，本测试会触发真实
    // rename（动生产数据）→ 跳过（该场景由环境决定，见报告）。
    let real_legacy = default_path_manager()
        .sessions_log_dir()
        .join("agent_main_main.jsonl");
    if real_legacy.exists() {
        eprintln!("[s9] real agent_main_main.jsonl exists; skipping migrate test");
        return;
    }

    // 1) 正常迁移：main json 带 key 字段
    let dir = temp_dir("migrate");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("agent_main_main.json"),
        r#"{"key":"agent:main:main","messages":[],"updated":"2026-01-01T00:00:00+08:00"}"#,
    )
    .unwrap();
    SessionStore::migrate_legacy_main(&dir);
    let legacy = dir.join("agent_main_session_legacy.json");
    assert!(legacy.exists(), "legacy json written");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&legacy).unwrap()).unwrap();
    assert_eq!(
        v["key"].as_str(),
        Some("agent:main:session:legacy"),
        "key field rewritten"
    );
    assert!(!dir.join("agent_main_main.json").exists(), "main removed");

    // 2) 幂等：main 已不在 → 不动
    SessionStore::migrate_legacy_main(&dir);
    assert!(legacy.exists());

    let _ = std::fs::remove_dir_all(&dir);

    // 3) 读失败：main json 是目录 → warn（881）
    let _logs = capture_logs();
    let dir2 = temp_dir("migrbad");
    let _ = std::fs::remove_dir_all(&dir2);
    std::fs::create_dir_all(dir2.join("agent_main_main.json")).unwrap();
    SessionStore::migrate_legacy_main(&dir2); // 必须不 panic
    let _ = std::fs::remove_dir_all(&dir2);
}

/// cleanup_old_sessions：storage_dir 是普通文件 → read_dir 失败 → warn
/// 字段行（916-917）+ return 0（920）。
#[test]
fn cleanup_old_sessions_storage_dir_is_file_warns() {
    let _logs = capture_logs();
    let not_a_dir = temp_dir("notdir");
    let _ = std::fs::remove_dir_all(&not_a_dir);
    std::fs::write(&not_a_dir, "i am a file").unwrap();
    let store = SessionStore::new_with_storage(&not_a_dir);
    assert_eq!(store.cleanup_old_sessions(7), 0);
    let _ = std::fs::remove_file(&not_a_dir);
}

/// 探针：本机文件系统是否执行 readonly-不可删语义（ReFS/Dev Drive 可能
/// 不执行）。探针用独立文件，不碰被测文件。
fn readonly_delete_enforced(dir: &std::path::Path) -> bool {
    let probe = dir.join(format!("s9_probe_{}.probe", std::process::id()));
    std::fs::write(&probe, "x").unwrap();
    let meta = std::fs::metadata(&probe).unwrap();
    let mut perm = meta.permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&probe, perm).unwrap();
    let blocked = std::fs::remove_file(&probe).is_err();
    if !blocked {
        let _ = std::fs::remove_file(&probe);
    }
    blocked
}

/// cleanup_old_sessions：过期 session 删除成功 + info 字段行（983-989）；
/// 过期 readonly 文件删除失败 warn（958-964，探针门控）。
#[test]
fn cleanup_old_sessions_deletes_expired_and_warns_on_readonly() {
    let _logs = capture_logs();
    let dir = temp_dir("cleanup");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let old_ts = "2020-01-01T00:00:00+08:00";

    // 可删的过期 session
    let k1 = key("cleanup1");
    std::fs::write(
        dir.join(format!("{}.json", sanitize_filename(&k1))),
        format!(r#"{{"key":"{k1}","messages":[],"updated":"{old_ts}"}}"#),
    )
    .unwrap();

    // readonly 过期 session（先写内容再设属性，之后绝不动它）
    let k2 = key("cleanup2");
    let ro_path = dir.join(format!("{}.json", sanitize_filename(&k2)));
    std::fs::write(
        &ro_path,
        format!(r#"{{"key":"{k2}","messages":[],"updated":"{old_ts}"}}"#),
    )
    .unwrap();
    let meta = std::fs::metadata(&ro_path).unwrap();
    let mut perm = meta.permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&ro_path, perm).unwrap();

    // 新鲜 session（age 为负 → 保留，走同循环的非删除侧）
    let k3 = key("cleanup3");
    std::fs::write(
        dir.join(format!("{}.json", sanitize_filename(&k3))),
        r#"{"key":"x","messages":[],"updated":"2099-01-01T00:00:00+08:00"}"#,
    )
    .unwrap();

    let ro_blocked = readonly_delete_enforced(&dir);
    // 探针残留清理：探针被拒时它自己还在，恢复属性删掉，避免干扰计数。
    let probe = dir.join(format!("s9_probe_{}.probe", std::process::id()));
    if probe.exists() {
        let m = std::fs::metadata(&probe).unwrap();
        let mut p = m.permissions();
        p.set_readonly(false);
        let _ = std::fs::set_permissions(&probe, p);
        let _ = std::fs::remove_file(&probe);
    }

    let store = SessionStore::new_with_storage(&dir);
    let deleted = store.cleanup_old_sessions(7);

    if ro_blocked {
        assert_eq!(deleted, 1, "only writable expired file deleted");
        assert!(ro_path.exists(), "readonly file survived with warn");
    } else {
        assert_eq!(deleted, 2, "both expired files deleted (fs lenient)");
        assert!(!ro_path.exists());
    }
    assert!(
        dir.join(format!("{}.json", sanitize_filename(&k3)))
            .exists(),
        "fresh session kept"
    );

    // 再跑一次：无过期可删 → 0（不进 info 分支的对照）。
    let store2 = SessionStore::new_with_storage(&dir);
    assert_eq!(store2.cleanup_old_sessions(7), 0);

    // 收尾：恢复 readonly 以便 remove_dir_all 成功。
    if ro_path.exists() {
        let m = std::fs::metadata(&ro_path).unwrap();
        let mut p = m.permissions();
        p.set_readonly(false);
        let _ = std::fs::set_permissions(&ro_path, p);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// force_compress_turns：info 字段行（1545-1549）+ 压缩语义。
#[test]
fn force_compress_turns_logs_and_drops_half() {
    let _logs = capture_logs();
    let mut hist = Vec::new();
    for i in 0..10 {
        hist.push(ConversationTurn {
            role: if i == 0 {
                "system".to_string()
            } else if i % 2 == 1 {
                "user".to_string()
            } else {
                "assistant".to_string()
            },
            content: format!("msg {i}"),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        });
    }
    let out = force_compress_turns(&hist);
    assert!(
        out.len() < hist.len(),
        "compressed {} -> {}",
        hist.len(),
        out.len()
    );
    assert_eq!(out[0].role, "system");
    assert!(out[1].content.contains("Emergency compression"));
    // 短历史原样返回（1505-1506 对照）
    let short = force_compress_turns(&hist[..3]);
    assert_eq!(short.len(), 3);
}
