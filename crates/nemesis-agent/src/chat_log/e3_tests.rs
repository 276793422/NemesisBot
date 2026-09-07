//! E3（devtool-upgrade 阶段 5）：消息级回退的 chat_log 侧测试。
//!
//! - `checkpoint_turn` 字段：`ChatLogMeta` 携带时写入（user 行的行→turn
//!   定位锚），`None` 不写（旧行宽容读契约不变）。
//! - `truncate_chat_log_rows`：原位截断 VERBATIM 保留保留侧行 + 计数；
//!   空 kept = 清空文件但文件仍在（会话可用）；重写不走 FTS 增量（懒索
//!   引自愈，与 fork 的 `write_chat_log_rows` 同边界）。

use super::*;

fn e3_uniq_key(tag: &str) -> String {
    format!(
        "test:e3:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// ChatLogMeta.checkpoint_turn Some → 行带 `checkpoint_turn` 数值字段；
/// None / 旧签名 → 不带。
#[test]
fn test_e3_checkpoint_turn_field_written_and_omitted() {
    let key = e3_uniq_key("stamp");
    delete_chat_log(&key);

    append_chat_log_meta(
        &key,
        "user",
        "do it",
        &ChatLogMeta {
            checkpoint_turn: Some(7),
            ..Default::default()
        },
    );
    append_chat_log_meta(
        &key,
        "assistant",
        "done",
        &ChatLogMeta {
            checkpoint_turn: None, // 标记只在 user 行
            ..Default::default()
        },
    );
    append_chat_log(&key, "user", "legacy row");

    let (rows, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 3);
    assert_eq!(rows[0]["checkpoint_turn"], serde_json::Value::from(7));
    assert!(rows[1].get("checkpoint_turn").is_none());
    assert!(rows[2].get("checkpoint_turn").is_none());

    delete_chat_log(&key);
}

/// truncate_chat_log_rows：保留侧 VERBATIM（字段逐个一致）+ 返回写入数；
/// 被截掉的行消失。
#[test]
fn test_e3_truncate_keeps_prefix_verbatim() {
    let key = e3_uniq_key("truncate");
    delete_chat_log(&key);

    let turns = [
        ("user", "turn 1 question", Some(1usize)),
        ("assistant", "turn 1 answer", None),
        ("user", "turn 2 question", Some(2)),
        ("assistant", "turn 2 answer", None),
    ];
    for (role, content, turn) in turns {
        append_chat_log_meta(
            &key,
            role,
            content,
            &ChatLogMeta {
                checkpoint_turn: turn,
                ..Default::default()
            },
        );
    }
    let (all, total, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total, 4);

    let n = truncate_chat_log_rows(&key, &all[..2]);
    assert_eq!(n, 2);
    let (kept, total2, _, _) = read_chat_log(&key, 100, None);
    assert_eq!(total2, 2);
    assert_eq!(kept[0]["content"], "turn 1 question");
    assert_eq!(kept[0]["checkpoint_turn"], serde_json::Value::from(1));
    assert_eq!(kept[1]["content"], "turn 1 answer");
    assert!(kept[1].get("checkpoint_turn").is_none());

    delete_chat_log(&key);
}

/// 空 kept = 文件清空但保留（会话仍可用；read 返回 0 行不报错）。
#[test]
fn test_e3_truncate_empty_keeps_file() {
    let key = e3_uniq_key("empty");
    delete_chat_log(&key);

    append_chat_log(&key, "user", "only row");
    let n = truncate_chat_log_rows(&key, &[]);
    assert_eq!(n, 0);
    assert!(chat_log_exists(&key), "文件保留");
    let (rows, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 0);
    assert!(rows.is_empty());

    delete_chat_log(&key);
}
