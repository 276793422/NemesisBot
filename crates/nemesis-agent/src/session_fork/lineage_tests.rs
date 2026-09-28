//! WS9/P17+P18：fork 侧谱系扩展测试（独立测试文件——禁内联测试仓库门禁）。
//!
//! 覆盖：ForkInfo.dropped_user_turns 换算（分支摘要阈值判定的输入）、
//! fork_reason 落盘 + upsert 保留语义、链式 fork 的祖先键链、
//! write_session_branch_summary 的空跳过 + 硬上限截断。
//!
//! 隔离纪律同 `tests.rs`：SessionStore 走 tempdir，chat_log/meta 走进程
//! 全局 path manager（本 lib 二进制共享 home）——nanos+pid+seq 唯一键 +
//! 结尾 delete_chat_log 全清（连带 sidecar meta）。

use super::*;
use crate::chat_log::{
    BRANCH_SUMMARY_MAX_CHARS, append_chat_log, delete_chat_log, read_session_meta_full,
    write_session_branch_summary, write_session_fork_reason,
};
use crate::session::SessionStore;

/// 唯一键（同 tests.rs 家族：pid + nanos + 进程内原子序号，防 Windows 时
/// 钟 tick 粗粒度并行撞键）。
fn uniq(tag: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(
        "ws9lin:{tag}:{}:{}:{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        seq
    )
}

/// 摆 n 个完整 (user, assistant) 轮。
fn seed_turns(key: &str, turns: usize) {
    for i in 1..=turns {
        append_chat_log(key, "user", &format!("turn {i} question"));
        append_chat_log(key, "assistant", &format!("turn {i} answer"));
    }
}

/// P18：ForkInfo.dropped_user_turns = 被排除行的完整 user 轮数（摘要触
/// 发阈值 ≥3 的判定输入）。
#[test]
fn fork_reports_dropped_user_turns() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = uniq("drops");
    seed_turns(&src, 5);

    // 在第 2 轮边界分叉：保留前 2 轮，遗弃后 3 轮。
    let info = fork_session(&store, &src, None, Some(2)).unwrap();
    assert_eq!(info.dropped_messages, 6, "遗弃 3 轮 × 2 行");
    assert_eq!(info.dropped_user_turns, 3, "遗弃行换算成完整 user 轮");

    // fork 在头（不遗弃）与中间零轮遗弃的边界：全量 fork = 0 轮。
    let info_full = fork_session(&store, &src, None, None).unwrap();
    assert_eq!(info_full.dropped_user_turns, 0);

    delete_chat_log(&src);
    delete_chat_log(&info.new_key);
    delete_chat_log(&info_full.new_key);
}

/// P17：fork_reason 落盘（新会话 meta），upsert 保留已有字段；空白缘由
/// 诚实不写（谱系视图回退通用「fork」）。
#[test]
fn fork_reason_written_and_upsert_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let src = uniq("reason");
    seed_turns(&src, 2);

    let info = fork_session(&store, &src, None, None).unwrap();
    let new_key = info.new_key.clone();

    // 先写血缘（fork_session 已写 parent/forked_at_turn），再写缘由和
    // 标题——三者必须共存（upsert 语义）。
    write_session_fork_reason(&new_key, "从这个结论出发另开一路");
    crate::chat_log::write_session_meta(&new_key, "分支标题");

    let meta = read_session_meta_full(&new_key).expect("meta 应存在");
    assert_eq!(
        meta.fork_reason.as_deref(),
        Some("从这个结论出发另开一路"),
        "缘由落盘"
    );
    assert_eq!(meta.parent.as_deref(), Some(src.as_str()), "血缘保留");
    assert_eq!(meta.title.as_deref(), Some("分支标题"), "标题共存");

    // 空白缘由不写（不产生 meta 文件 / 不清空已有值）。
    write_session_fork_reason(&new_key, "   ");
    let meta2 = read_session_meta_full(&new_key).expect("meta 应存在");
    assert_eq!(
        meta2.fork_reason.as_deref(),
        Some("从这个结论出发另开一路"),
        "空白缘由跳过，不清空已有值"
    );

    delete_chat_log(&src);
    delete_chat_log(&new_key);
}

/// P17：链式 fork——A → B → C 各级 parent 链正确，且各级缘由互不串写
/// （每个新会话自己的 meta 记自己的缘由）。
#[test]
fn chained_fork_parent_chain_and_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let a = uniq("chain-a");
    seed_turns(&a, 4);

    let b_info = fork_session(&store, &a, None, Some(2)).unwrap();
    write_session_fork_reason(&b_info.new_key, "第一层分支缘由");
    let c_info = fork_session(&store, &b_info.new_key, None, Some(1)).unwrap();
    write_session_fork_reason(&c_info.new_key, "第二层分支缘由");

    let b_meta = read_session_meta_full(&b_info.new_key).expect("B meta");
    let c_meta = read_session_meta_full(&c_info.new_key).expect("C meta");
    assert_eq!(b_meta.parent.as_deref(), Some(a.as_str()));
    assert_eq!(c_meta.parent.as_deref(), Some(b_info.new_key.as_str()));
    assert_eq!(b_meta.fork_reason.as_deref(), Some("第一层分支缘由"));
    assert_eq!(c_meta.fork_reason.as_deref(), Some("第二层分支缘由"));
    // C 的 meta 不该串到 A 的缘由（各层独立）。
    assert_ne!(c_meta.fork_reason.as_deref(), Some("第一层分支缘由"));

    delete_chat_log(&a);
    delete_chat_log(&b_info.new_key);
    delete_chat_log(&c_info.new_key);
}

/// P18：分支摘要写入——空串跳过（不产生无意义 meta）、超长截断到
/// BRANCH_SUMMARY_MAX_CHARS（防御性二次钳制）。
#[test]
fn branch_summary_writer_skips_empty_and_caps_length() {
    let key = uniq("summary");

    // 空白不写。
    write_session_branch_summary(&key, "  \n  ");
    assert!(
        read_session_meta_full(&key).is_none(),
        "空白摘要不落 meta 文件"
    );

    // 超长截断（chars 语义，多字节字符不劈开）。
    let long = "目".repeat(BRANCH_SUMMARY_MAX_CHARS + 500);
    write_session_branch_summary(&key, &long);
    let meta = read_session_meta_full(&key).expect("超长摘要应落盘");
    let got = meta.branch_summary.expect("branch_summary 字段存在");
    assert_eq!(got.chars().count(), BRANCH_SUMMARY_MAX_CHARS, "截断到上限");
    // 正文照常共存（upsert）。
    crate::chat_log::write_session_meta(&key, "带摘要的会话");
    let meta2 = read_session_meta_full(&key).expect("meta");
    assert_eq!(meta2.title.as_deref(), Some("带摘要的会话"));
    assert!(meta2.branch_summary.is_some(), "标题写入不冲掉摘要");

    delete_chat_log(&key);
}
