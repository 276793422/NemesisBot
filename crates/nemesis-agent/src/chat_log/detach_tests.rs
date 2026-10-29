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

/// BUG-20261030-197 红证：detach 后迟到的分支摘要后台写不得复活
/// branch_summary——detach 契约「Branch Context 停止注入」不被 in-flight
/// 摘要（spawn 于 fork/rewind、LLM 往返数秒后落盘）击穿；re-fork
/// （write_session_parent）解除封印。
#[test]
fn bug197_late_branch_summary_after_detach_does_not_resurrect() {
    let key = unique_key("late");
    let parent = unique_key("lateparent");

    // fork 血缘在册，摘要在飞（模拟 spawn_write 尚未落盘）。
    write_session_parent(&key, &parent, 4);
    // 用户 detach（摘要还在 LLM 途中）。
    assert!(clear_session_lineage(&key));
    // 迟到的摘要落盘——不得复活。
    write_session_branch_summary(&key, "迟到的分支前情提要");
    assert_eq!(
        read_session_meta_full(&key).unwrap().branch_summary,
        None,
        "detach 后迟到的分支摘要不落盘"
    );

    // re-fork 上血缘后封印解除（新摘要可落盘；「clear 是摘除不是封禁」
    // 契约的镜像面）。
    write_session_parent(&key, &parent, 2);
    write_session_branch_summary(&key, "重新分支后的摘要");
    assert_eq!(
        read_session_meta_full(&key)
            .unwrap()
            .branch_summary
            .as_deref(),
        Some("重新分支后的摘要"),
        "re-fork 解除封印"
    );

    delete_chat_log(&key);
    delete_chat_log(&parent);
}

/// BUG-20261030-197 对照：未 detach 的会话摘要照常落盘——fork 血缘在册
/// 与无血缘（rewind 摘要形态）两臂都不受封印影响。
#[test]
fn bug197_branch_summary_writes_normally_without_detach() {
    let key = unique_key("normal");
    write_session_parent(&key, &unique_key("normalparent"), 1);
    write_session_branch_summary(&key, "正常摘要");
    assert_eq!(
        read_session_meta_full(&key)
            .unwrap()
            .branch_summary
            .as_deref(),
        Some("正常摘要"),
        "血缘在册 + 未 detach → 照常落盘"
    );

    let plain = unique_key("plain");
    write_session_branch_summary(&plain, "回退摘要");
    assert_eq!(
        read_session_meta_full(&plain)
            .unwrap()
            .branch_summary
            .as_deref(),
        Some("回退摘要"),
        "无血缘会话（rewind 摘要形态）→ 照常落盘"
    );

    delete_chat_log(&key);
    delete_chat_log(&plain);
}
