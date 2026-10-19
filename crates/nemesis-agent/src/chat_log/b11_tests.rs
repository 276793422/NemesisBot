//! B11（2026-10-18 snooze）：SessionMeta 暂停标记三函数测试——write 幂等
//! upsert / clear 摘除返回原值 / snoozed_sessions 扫描过滤。只允许本文件
//! 写 snooze 标记（断言按 stem 过滤，与其他并行测试的 meta 读写互不干扰；
//! META_LOCK 保护 RMW 原子性），unique key + delete 清理（本目录测试惯例
//! ——走进程单例 home，不重定向）。

use super::*;

fn unique_key(tag: &str) -> String {
    format!(
        "test:b11:{tag}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[test]
fn b11_snooze_write_read_roundtrip() {
    let key = unique_key("roundtrip");
    let until = "2026-10-19T09:00:00+08:00";

    // 无 meta 文件时直接 snooze：upsert 建档。
    write_session_snooze(&key, until, Some("等部署窗口"));
    let meta = read_session_meta_full(&key).expect("meta 应存在");
    assert_eq!(meta.snoozed_until.as_deref(), Some(until));
    assert_eq!(meta.snooze_note.as_deref(), Some("等部署窗口"));

    // 幂等 upsert：改期 + 清 note（覆盖语义——旧 note 不残留）。
    let until2 = "2026-10-20T10:00:00+08:00";
    write_session_snooze(&key, until2, None);
    let meta = read_session_meta_full(&key).expect("meta 应存在");
    assert_eq!(meta.snoozed_until.as_deref(), Some(until2));
    assert_eq!(meta.snooze_note, None, "覆盖写清掉旧 note");

    // serde 平面字段形态：note=None 时不落盘（skip_serializing_if）；
    // until=Some 时照常落盘。
    let raw = std::fs::read_to_string(super::meta_path(&key)).unwrap();
    assert!(raw.contains("snoozed_until"), "Some 应落盘: {raw}");
    assert!(
        !raw.contains("snooze_note"),
        "None 时不应落盘（skip_serializing_if）: {raw}"
    );

    delete_chat_log(&key); // 连带 meta 清理
}

#[test]
fn b11_clear_returns_original_and_is_idempotent() {
    let key = unique_key("clear");
    let until = "2026-10-19T09:00:00+08:00";
    write_session_snooze(&key, until, Some("note-a"));

    let cleared = clear_session_snooze(&key).expect("有标记应返回 Some");
    assert_eq!(cleared, (until.to_string(), Some("note-a".to_string())));
    // 摘除后 meta 文件保留（title 等同批字段不丢），两键归 None。
    let meta = read_session_meta_full(&key).expect("meta 应保留");
    assert_eq!(meta.snoozed_until, None);
    assert_eq!(meta.snooze_note, None);

    // 二次摘除 = None（幂等，不空写）。
    assert_eq!(clear_session_snooze(&key), None);
    // 无 meta 文件 = None。
    let missing = unique_key("missing");
    assert_eq!(clear_session_snooze(&missing), None);

    delete_chat_log(&key);
}

#[test]
fn b11_snoozed_sessions_scans_and_filters() {
    let key_a = unique_key("scan-a");
    let key_b = unique_key("scan-b");
    let until = "2026-12-01T00:00:00+08:00";

    write_session_snooze(&key_a, until, None);
    write_session_snooze(&key_b, until, Some("nb"));

    let found = snoozed_sessions();
    let stem_a = nemesis_utils::sanitize::sanitize_path_segment(&key_a);
    let stem_b = nemesis_utils::sanitize::sanitize_path_segment(&key_b);
    let hit_a = found.iter().find(|(s, _, _)| *s == stem_a);
    let hit_b = found.iter().find(|(s, _, _)| *s == stem_b);
    assert_eq!(
        hit_a.map(|(_, u, n)| (u.as_str(), n.clone())),
        Some((until, None))
    );
    assert_eq!(
        hit_b.map(|(_, u, n)| (u.as_str(), n.clone())),
        Some((until, Some("nb".to_string())))
    );
    // 输出按 stem 排序（测试确定性——消费与顺序无关，排序只为此）。
    let mut sorted = found.iter().map(|(s, _, _)| s.clone()).collect::<Vec<_>>();
    sorted.sort();
    let mut actual = found.iter().map(|(s, _, _)| s.clone()).collect::<Vec<_>>();
    actual.sort();
    assert_eq!(actual, sorted);

    // 摘除后扫描不再含该项。
    clear_session_snooze(&key_a);
    let found = snoozed_sessions();
    assert!(!found.iter().any(|(s, _, _)| *s == stem_a));

    delete_chat_log(&key_a);
    delete_chat_log(&key_b);
}

#[test]
fn b11_delete_chat_log_removes_snooze_marker() {
    // 删除会话连带 meta（既有语义）→ snooze 标记一并消失，扫描无残留。
    let key = unique_key("delete");
    write_session_snooze(&key, "2026-12-01T00:00:00+08:00", None);
    delete_chat_log(&key);
    let stem = nemesis_utils::sanitize::sanitize_path_segment(&key);
    assert!(!snoozed_sessions().iter().any(|(s, _, _)| *s == stem));
}
