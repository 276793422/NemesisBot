//! 回归锁：自动标题写入的 check-then-write 原子化。
//!
//! 旧形态：`auto_title_eligible`（锁外读）→ `upsert_meta`（锁内盲写）——
//! 检查与写盘之间的窗口里用户 rename（`write_session_meta_manual` 锁内置
//! `title_manual=true`）插入时，自动标题照样落盘覆盖手动名（「手动改名
//! 永不覆盖」契约击穿）。修复后写点在 [`META_LOCK`] 下判定+落盘原子完成
//! （与 `write_session_branch_summary` 的 detach 封印位同构），判定逻辑与
//! `auto_title_eligible` 共用 `auto_title_writable` 单一真相源。
//!
//! 毫秒级竞窗无法确定性重现；本组锁钉行为面：拒绝臂必须**不落盘**
//! （meta 逐字节保持）、判定与写点对全部 meta 形态裁决一致。

use super::*;

fn b385_uniq_key(tag: &str) -> String {
    format!(
        "test:b385:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// 核心回归锁：manual 会话上自动标题被拒后 meta **逐字节保持**——拒绝臂
/// 提前 return，不读-改-写、不重建文件（旧形态走 upsert 会以读到的基线
/// 重写整个文件，与 rename 写者交错时丢并发更新）。
#[test]
fn b385_auto_title_rejection_leaves_meta_untouched() {
    let key = b385_uniq_key("reject");
    delete_chat_log(&key);
    // 造一个带全字段的手动命名会话（title + lineage + project 归属）。
    write_session_meta_manual(&key, "用户手动命名");
    write_session_parent(&key, "agent:main:session:parent", 3);
    write_session_project(&key, "p1", "/tmp/proj");
    let before = read_meta_full(&key).unwrap();

    assert!(
        !write_session_meta_auto_title(&key, "迟到的自动标题"),
        "manual 会话必须拒绝自动标题"
    );

    let after = read_meta_full(&key).unwrap();
    assert_eq!(
        before, after,
        "拒绝臂不得触碰 meta（旧形态的锁外检查+锁内盲写窗口里，\
         upsert 重写会吞掉并发写入的字段）"
    );
    assert_eq!(after.title.as_deref(), Some("用户手动命名"));
    assert_eq!(after.parent.as_deref(), Some("agent:main:session:parent"));
    delete_chat_log(&key);
}

/// 判定与写点同源：对全部 meta 形态，`write_session_meta_auto_title` 的
/// 成败必须与 `auto_title_eligible` 的裁决一致（同一 `auto_title_writable`
/// 单一真相源；两消费点漂移 = spawn 预检与落盘裁决各说各话）。
#[test]
fn b385_eligibility_and_write_share_single_truth() {
    // 形态 1：无 meta → eligible 且写成功。
    let key = b385_uniq_key("absent");
    delete_chat_log(&key);
    assert!(auto_title_eligible(&key));
    assert!(write_session_meta_auto_title(&key, "自动A"));

    // 形态 2：真实自动标题在位 → 双双拒绝。
    assert!(!auto_title_eligible(&key));
    assert!(!write_session_meta_auto_title(&key, "自动A2"));

    // 形态 3：占位符（非 manual）→ 双双放行，覆盖落位。
    let key2 = b385_uniq_key("placeholder");
    delete_chat_log(&key2);
    write_session_meta(&key2, DEFAULT_SESSION_TITLE);
    assert!(auto_title_eligible(&key2));
    assert!(write_session_meta_auto_title(&key2, "自动B"));
    assert_eq!(read_session_meta(&key2).as_deref(), Some("自动B"));

    // 形态 4：manual=true 且标题恰为占位符文本 → 双双拒绝（用户意志
    // 不因文本碰巧等于占位符而失格——与 tests.rs 矩阵同口径）。
    let key3 = b385_uniq_key("manual-placeholder");
    delete_chat_log(&key3);
    write_session_meta_manual(&key3, DEFAULT_SESSION_TITLE);
    assert!(!auto_title_eligible(&key3));
    assert!(!write_session_meta_auto_title(&key3, "自动C"));

    // 形态 5：pre legacy meta（无 title_manual 字段，真实标题）→ 反
    // 序列化缺省 false → 双双拒绝（旧档兼容读侧保护）。
    let key4 = b385_uniq_key("legacy");
    delete_chat_log(&key4);
    std::fs::write(meta_path(&key4), r#"{"title":"旧标题"}"#).unwrap();
    assert!(!auto_title_eligible(&key4));
    assert!(!write_session_meta_auto_title(&key4, "自动D"));
    assert_eq!(read_session_meta(&key4).as_deref(), Some("旧标题"));

    for k in [&key, &key2, &key3, &key4] {
        delete_chat_log(k);
    }
}

/// 写成功路径仍走原子替换且保留既有字段（无 meta 起写 → 手动标记不置；
/// 对空 title 占位会话补写 → lineage 等既有字段不丢）。
#[test]
fn b385_auto_title_success_preserves_fields() {
    let key = b385_uniq_key("success");
    delete_chat_log(&key);
    // 占位符 + 既有血缘：自动标题覆盖 title，血缘原样保留。
    write_session_meta(&key, DEFAULT_SESSION_TITLE);
    write_session_parent(&key, "agent:main:session:pa", 1);

    assert!(write_session_meta_auto_title(&key, "自动E"));
    let m = read_meta_full(&key).unwrap();
    assert_eq!(m.title.as_deref(), Some("自动E"));
    assert_eq!(m.parent.as_deref(), Some("agent:main:session:pa"));
    assert_eq!(m.forked_at_turn, Some(1));
    assert!(!m.title_manual, "自动写入不置 manual 标记");

    delete_chat_log(&key);
}
