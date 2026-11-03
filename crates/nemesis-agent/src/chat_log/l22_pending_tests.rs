//! L22（2026-11-03 snooze 排队）：暂停期入站 pending sidecar 设施测试。
//!
//! append/take 原子取走（rename 防双投 + 崩溃残留恢复臂）/ 上限诚实拒收 /
//! 行内会话键烧入（pending_sessions 逆清洗还原）/ delete_chat_log 级联 /
//! session_file_changes 聚合投影。unique key + delete 清理（本目录测试
//! 惯例——走进程单例 home，不重定向）。

use super::*;
use nemesis_types::channel::InboundMessage;

fn unique_key(tag: &str) -> String {
    format!(
        "test:l22pending:{tag}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn msg(content: &str) -> InboundMessage {
    InboundMessage {
        channel: "web".to_string(),
        sender_id: "l22user".to_string(),
        chat_id: "l22chat".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

#[test]
fn l22_pending_append_take_roundtrip() {
    let key = unique_key("roundtrip");
    assert_eq!(session_pending_count(&key), 0, "无文件 = 0");
    assert!(take_session_pending(&key).is_empty(), "无文件 take = 空");

    // 追加三条：返回排队后条数（1/2/3），行内会话键烧入。
    assert_eq!(append_session_pending(&key, &msg("m1")).unwrap(), 1);
    assert_eq!(append_session_pending(&key, &msg("m2")).unwrap(), 2);
    assert_eq!(append_session_pending(&key, &msg("m3")).unwrap(), 3);
    assert_eq!(session_pending_count(&key), 3);

    // take：FIFO 全取 + 键戳还原 + 文件消失（单次消费）。
    let rows = take_session_pending(&key);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].content, "m1");
    assert_eq!(rows[1].content, "m2");
    assert_eq!(rows[2].content, "m3");
    for r in &rows {
        assert_eq!(r.session_key, key, "行内键应烧入裁决键（msg 原字段为空）");
    }
    assert_eq!(session_pending_count(&key), 0, "take 后文件应消失");
    assert!(take_session_pending(&key).is_empty(), "二次 take = 空");

    delete_chat_log(&key);
}

#[test]
fn l22_pending_cap_refuses_honestly() {
    let key = unique_key("cap");
    for i in 0..SESSION_PENDING_CAP {
        append_session_pending(&key, &msg(&format!("m{i}"))).expect("上限内应成功");
    }
    assert_eq!(session_pending_count(&key), SESSION_PENDING_CAP);
    // 超限：Err（诚实拒收），不挤掉旧条、条数不动。
    let err = append_session_pending(&key, &msg("overflow")).unwrap_err();
    assert!(err.contains("排队已满"), "got: {err}");
    assert_eq!(session_pending_count(&key), SESSION_PENDING_CAP);
    // 取走后配额释放。
    take_session_pending(&key);
    append_session_pending(&key, &msg("fresh")).expect("清空后应可再排");

    delete_chat_log(&key);
}

#[test]
fn l22_pending_take_crash_residue_arm() {
    let key = unique_key("residue");
    // 崩溃残留形态：只有 `.taking` 在（rename 后、删除前进程终止的产物），
    // 原文件不在——take 应优先收走残留。
    let path = pending_path(&key);
    let tmp = path.with_extension("taking");
    if let Some(parent) = tmp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&tmp, serde_json::to_string(&msg("orphan")).unwrap()).unwrap();
    let rows = take_session_pending(&key);
    assert_eq!(rows.len(), 1, "残留臂应收走 .taking");
    assert_eq!(rows[0].content, "orphan");
    assert!(!tmp.exists(), "残留取走后应删除");

    // 残留 + 原文件并存：先收残留，原文件留待下次（Windows rename 不覆盖
    // 已存在目标——先收残留是顺序必需）。
    append_session_pending(&key, &msg("live")).unwrap();
    std::fs::write(&tmp, serde_json::to_string(&msg("orphan2")).unwrap()).unwrap();
    let rows = take_session_pending(&key);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].content, "orphan2", "残留优先");
    let rows = take_session_pending(&key);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].content, "live", "原文件随后收走");

    delete_chat_log(&key);
}

#[test]
fn l22_pending_sessions_restores_stamped_keys() {
    let key_a = unique_key("scan-a");
    let key_b = unique_key("scan-b");
    // msg 自带 session_key 为空——append 烧入裁决键后 pending_sessions
    // 必须按行内键还原（文件名 stem 是 sanitize 有损形态）。
    append_session_pending(&key_a, &msg("a1")).unwrap();
    append_session_pending(&key_b, &msg("b1")).unwrap();

    let keys = pending_sessions();
    assert!(keys.contains(&key_a), "应含 {key_a}: {keys:?}");
    assert!(keys.contains(&key_b), "应含 {key_b}: {keys:?}");
    // 排序去重契约。
    let mut sorted = keys.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(keys, sorted);

    take_session_pending(&key_a);
    take_session_pending(&key_b);
    let keys = pending_sessions();
    assert!(!keys.contains(&key_a) && !keys.contains(&key_b));
}

#[test]
fn l22_delete_chat_log_cascades_pending() {
    let key = unique_key("cascade");
    append_session_pending(&key, &msg("bye")).unwrap();
    append_chat_log(&key, "user", "hi");
    assert_eq!(session_pending_count(&key), 1);
    delete_chat_log(&key);
    assert_eq!(session_pending_count(&key), 0, "会话删除应级联清 pending");
    assert!(!pending_path(&key).exists());
}

#[test]
fn l22_session_snoozed_until_single_read() {
    let key = unique_key("until");
    assert_eq!(session_snoozed_until(&key), None, "无 meta = None");
    write_session_snooze(&key, "2099-01-01T00:00:00+08:00", Some("n"));
    assert_eq!(
        session_snoozed_until(&key),
        Some((
            "2099-01-01T00:00:00+08:00".to_string(),
            Some("n".to_string())
        ))
    );
    let removed = clear_session_snooze(&key);
    assert_eq!(
        removed,
        Some((
            "2099-01-01T00:00:00+08:00".to_string(),
            Some("n".to_string())
        ))
    );
    assert_eq!(session_snoozed_until(&key), None, "摘除后 = None");
    delete_chat_log(&key);
}

#[test]
fn l22_session_file_changes_aggregates_unique_in_order() {
    let key = unique_key("prfiles");
    // assistant 行 file_changes 聚合：去重保首次出现序；user 行/无字段行
    // 跳过。
    let c1 = vec![
        crate::r#loop::FileChange {
            path: "src/a.rs".to_string(),
            kind: crate::r#loop::FileChangeKind::Modify,
        },
        crate::r#loop::FileChange {
            path: "src/b.rs".to_string(),
            kind: crate::r#loop::FileChangeKind::Create,
        },
    ];
    let c2 = vec![crate::r#loop::FileChange {
        path: "src/a.rs".to_string(), // 跨 turn 重复 → 去重
        kind: crate::r#loop::FileChangeKind::Modify,
    }];
    append_chat_log_meta(
        &key,
        "assistant",
        "r1",
        &ChatLogMeta {
            file_changes: &c1,
            ..Default::default()
        },
    );
    append_chat_log(&key, "user", "more");
    append_chat_log_meta(
        &key,
        "assistant",
        "r2",
        &ChatLogMeta {
            file_changes: &c2,
            ..Default::default()
        },
    );

    let files = session_file_changes(&key);
    assert_eq!(files, vec!["src/a.rs".to_string(), "src/b.rs".to_string()]);

    delete_chat_log(&key);
}
