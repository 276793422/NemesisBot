//! detach（2026-10-20「脱离父分支」）：clear_session_lineage 四字段摘除
//! 测试——血缘四键（parent / forked_at_turn / fork_reason / branch_summary）
//! 清空、其余 meta（title / project 归属 / snooze / last_rewind）保留、
//! 无血缘 / 无 meta 诚实返 false。隔离纪律同 b11_tests：unique key +
//! delete_chat_log 全清（连带 sidecar meta），META_LOCK 保护 RMW 原子性。

use super::*;

fn unique_key(tag: &str) -> String {
    format!(
        "test:detach:{tag}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[test]
fn detach_clears_all_four_lineage_fields_and_keeps_the_rest() {
    let key = unique_key("full");
    let parent = unique_key("parent");

    // 摆满四字段血缘 + 四类应保留字段。
    write_session_parent(&key, &parent, 3);
    write_session_fork_reason(&key, "从这个结论另开一路");
    write_session_branch_summary(&key, "父会话走了什么");
    write_session_meta(&key, "分支会话");
    write_session_project(&key, "p1", "C:/proj");
    write_session_snooze(&key, "2026-12-01T00:00:00+08:00", Some("挂起"));
    write_session_rewind(&key, 5, 2, 1);

    assert!(clear_session_lineage(&key), "有血缘应返 true");

    let meta = read_session_meta_full(&key).expect("meta 应保留");
    assert_eq!(meta.parent, None, "parent 摘除");
    assert_eq!(meta.forked_at_turn, None, "forked_at_turn 摘除");
    assert_eq!(meta.fork_reason, None, "fork_reason 摘除");
    assert_eq!(meta.branch_summary, None, "branch_summary 摘除");
    assert_eq!(meta.title.as_deref(), Some("分支会话"), "标题保留");
    assert_eq!(meta.project_id.as_deref(), Some("p1"), "项目归属保留");
    assert_eq!(
        meta.project_path.as_deref(),
        Some("C:/proj"),
        "项目路径保留"
    );
    assert!(meta.snoozed_until.is_some(), "snooze 标记与血缘互不相干");
    assert!(meta.last_rewind.is_some(), "回退记录保留");

    // 摘除后可重新 fork 上血缘（clear 是摘除不是封禁）。
    write_session_parent(&key, &parent, 1);
    assert_eq!(
        read_session_meta_full(&key).unwrap().parent.as_deref(),
        Some(parent.as_str())
    );

    delete_chat_log(&key);
    delete_chat_log(&parent);
}

#[test]
fn detach_without_lineage_returns_false() {
    let key = unique_key("nolineage");

    // 无 meta 文件：诚实返 false。
    assert!(!clear_session_lineage(&key));
    assert!(read_session_meta_full(&key).is_none(), "不空写 meta");

    // 只有 title（无血缘）：false 且不空写。
    write_session_meta(&key, "普通会话");
    assert!(!clear_session_lineage(&key));
    let meta = read_session_meta_full(&key).expect("meta 应保留");
    assert_eq!(meta.title.as_deref(), Some("普通会话"));

    // 只有 fork_reason（无 parent——理论畸形态）也算有血缘可清。
    write_session_fork_reason(&key, "孤儿缘由");
    assert!(clear_session_lineage(&key), "任一血缘字段在即可清");
    assert_eq!(
        read_session_meta_full(&key).unwrap().fork_reason,
        None,
        "缘由随血缘一并摘除"
    );

    // 二次摘除 = false（幂等，不空写）。
    assert!(!clear_session_lineage(&key));

    delete_chat_log(&key);
}
