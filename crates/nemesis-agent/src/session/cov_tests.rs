// session.rs 覆盖率补充测试（自愈重建 / legacy main 迁移双 store /
// cleanup 读目录失败·过期删除·删除失败 / delete·clear 删除失败 warn /
// force_compress 短史恒等与长史压缩 / save 往返）。
//
// 不调 CaptureSink::init——session::tests 的 capture 测试是该 OnceLock
// 在本测试二进制的唯一初始化点（先到先得，多初始化会互相毒化）。
//
// save() 的「sanitize 后仍非法」错误臂（733）为死防御——
// sanitize_path_segment 已保证输出非 `.`/`..` 且不含路径分隔符，无法从
// 外部触发；记豁免。

use super::*;
use std::path::PathBuf;
use std::time::Duration;

/// 自愈重建：内存与盘上 json 双 miss + chat_log 有行 → 从 chat_log 回放。
#[test]
fn self_heal_rebuilds_missing_store_from_chat_log() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let key = format!("agent:main:session:covheal-{}", std::process::id());

    crate::chat_log::append_chat_log(&key, "user", "自愈问");
    crate::chat_log::append_chat_log(&key, "assistant", "自愈答");

    let s = store.get_or_create(&key);
    assert!(
        s.messages.len() >= 2,
        "rebuilt from chat_log: {} rows",
        s.messages.len()
    );
    assert_eq!(s.messages[0].content, "自愈问");
}

/// legacy main 迁移：session_logs 与 sessions 两个 store 都搬家、key 改写。
#[test]
fn migrate_legacy_main_moves_log_and_json() {
    // JSON 侧：临时 storage dir，完全可控。
    let dir = tempfile::tempdir().unwrap();
    let main_json = dir.path().join("agent_main_main.json");
    std::fs::write(
        &main_json,
        r#"{"key":"agent:main:main","messages":[{"role":"user","content":"legacy"}]}"#,
    )
    .unwrap();
    SessionStore::migrate_legacy_main(dir.path());
    let legacy_json = dir.path().join("agent_main_session_legacy.json");
    assert!(legacy_json.exists(), "json migrated");
    assert!(!main_json.exists(), "main json removed after copy");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&legacy_json).unwrap()).unwrap();
    assert_eq!(v["key"], "agent:main:session:legacy");

    // JSONL 侧（全局 sessions_log_dir，幂等守卫下能搬则搬）。
    let logs_dir = nemesis_path::default_path_manager().sessions_log_dir();
    let _ = std::fs::create_dir_all(&logs_dir);
    let main_log = logs_dir.join("agent_main_main.jsonl");
    let legacy_log = logs_dir.join("agent_main_session_legacy.jsonl");
    let legacy_pre = legacy_log.exists();
    if !legacy_pre {
        std::fs::write(&main_log, "{\"role\":\"user\"}\n").unwrap();
    }
    SessionStore::migrate_legacy_main(dir.path());
    if !legacy_pre {
        assert!(legacy_log.exists(), "jsonl migrated");
        assert!(!main_log.exists());
    }
}

/// cleanup：storage_dir 是普通文件 → read_dir 失败诚实返回空。
#[test]
fn cleanup_with_unreadable_storage_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let fake_dir = dir.path().join("not-a-dir");
    std::fs::write(&fake_dir, "x").unwrap();
    let store = SessionStore::new_with_storage(&fake_dir);
    assert!(store.cleanup_old_sessions_detailed(7).is_empty());
}

/// cleanup：过期 json 删除 + 记账；读不了的过期文件跳过（删除失败 warn）。
#[test]
fn cleanup_deletes_expired_and_skips_undeletable() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());

    // 过期：updated = 30 天前。
    let old_key = "agent:main:session:covold";
    store.get_or_create(old_key);
    store.save(old_key).unwrap();
    let old_path = dir
        .path()
        .join(format!("{}.json", sanitize_filename(old_key)));
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&old_path).unwrap()).unwrap();
    v["updated"] =
        serde_json::json!((chrono::Local::now() - chrono::Duration::days(30)).to_rfc3339());
    std::fs::write(&old_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    // 不可删除面：Windows 用缺 FILE_SHARE_DELETE 的占用句柄制造
    // sharing violation；Unix 只读文件可被删（父目录权限才作数），该
    // 失败面不在本平台复现。句柄必须在 cleanup 之前就位。
    let stuck_key = "agent:main:session:covstuck";
    let stuck_path = dir
        .path()
        .join(format!("{}.json", sanitize_filename(stuck_key)));
    std::fs::write(
        &stuck_path,
        format!(
            r#"{{"key":"{stuck_key}","updated":{}}}"#,
            serde_json::json!((chrono::Local::now() - chrono::Duration::days(30)).to_rfc3339())
        ),
    )
    .unwrap();

    #[cfg(windows)]
    let held = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x0001 | 0x0002) // READ | WRITE，刻意无 DELETE
            .open(&stuck_path)
            .unwrap()
    };

    let deleted = store.cleanup_old_sessions_detailed(7);
    assert!(deleted.iter().any(|k| k == old_key), "deleted: {deleted:?}");
    assert!(!old_path.exists(), "expired file removed");

    #[cfg(windows)]
    {
        assert!(
            deleted.iter().all(|k| k != stuck_key),
            "undeletable skipped, got: {deleted:?}"
        );
        assert!(stuck_path.exists(), "undeletable file survives");
        drop(held);
        let _ = std::fs::remove_file(&stuck_path);
    }
    #[cfg(not(windows))]
    {
        let _ = std::fs::remove_file(&stuck_path);
    }
}

/// delete/clear：json 位置是目录 → 删除失败 warn（不 panic，继续清理内存）。
#[test]
fn delete_and_clear_warn_when_json_path_is_directory() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let key = "agent:main:session:covdir";
    let json_dir: PathBuf = dir.path().join(format!("{}.json", sanitize_filename(key)));
    std::fs::create_dir_all(json_dir.join("nested")).unwrap();

    store.get_or_create(key);
    assert!(store.delete_session(key), "in-memory entry existed");
    assert!(!store.sessions.read().unwrap().contains_key(key));

    store.get_or_create(key);
    store.clear_session(key);
    assert!(!store.sessions.read().unwrap().contains_key(key));
}

/// force_compress：短史（≤4）原样返回；长史压缩后 system+note+后半+末条。
#[test]
fn force_compress_short_identity_and_long_compression() {
    fn cv_turn(role: &str, content: &str) -> ConversationTurn {
        ConversationTurn {
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: chrono::Local::now().to_rfc3339(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        }
    }

    let short: Vec<ConversationTurn> = (0..4)
        .map(|i| cv_turn(if i == 0 { "system" } else { "user" }, &format!("m{i}")))
        .collect();
    let same = super::force_compress_turns(&short);
    assert_eq!(same.len(), 4);
    assert_eq!(same[0].content, "m0");

    // 6 条（system + 4 对话 + 末条）：mid=2 → 保留后半 2 条。
    let mut long = vec![cv_turn("system", "sys")];
    for i in 0..4 {
        long.push(cv_turn("user", &format!("u{i}")));
    }
    long.push(cv_turn("assistant", "final"));
    let out = super::force_compress_turns(&long);
    assert_eq!(out[0].content, "sys");
    assert!(out[1].content.contains("Emergency compression dropped 2"));
    assert_eq!(out[2].content, "u2");
    assert_eq!(out[3].content, "u3");
    assert_eq!(out[4].content, "final");
    assert_eq!(out.len(), 5);
}

/// save 后 get_or_create 完整往返（保存纪律 + 计数器防踩形态冒烟）。
#[test]
fn save_and_reload_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let key = "agent:main:session:covsave";
    {
        let store = SessionStore::new_with_storage(dir.path());
        let s = store.get_or_create(key);
        drop(s);
        store.add_message(key, "user", "persist me");
        store.save(key).unwrap();
    }
    let reloaded = SessionStore::new_with_storage(dir.path());
    let s = reloaded.get_or_create(key);
    assert_eq!(s.messages.len(), 1);
    assert_eq!(s.messages[0].content, "persist me");

    // 给 save 的并发防踩留一点间隔（计数器临时文件同名窗口）。
    std::thread::sleep(Duration::from_millis(5));
}
