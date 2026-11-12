//! PLAN 234 C4（2026-11-12）：早落盘 user 行 `images` 回补回归锁。
//!
//! 背景实锤：admission 的 user 行落盘点在 attach 之前（聊天切会话竞态），
//! 带图消息的 jsonl 行 `images` 恒缺 → fork 复制 / self-heal 重建（共享
//! 映射 `projected_messages_from_rows`，F-E 语义）还原出的 `image_refs`
//! 为空——分叉会话带图上下文凭空消失（源 store 有图、fork store 无图、
//! fork 内续问回 NO_IMAGE）。本组测试钉住回补原语 + F-E 还原链路。
//!
//! （沿用 s9/t6_tests 的真路径 + 唯一键 + 清理形态——不触碰全局路径单例。）

use super::*;
use serde_json::json;

fn key(tag: &str) -> String {
    format!("test:c4img:{}_{}", tag, std::process::id())
}

fn cleanup(k: &str) {
    delete_chat_log(k);
}

fn rows_of(k: &str) -> Vec<Value> {
    let raw = std::fs::read_to_string(log_path(k)).expect("jsonl 应存在");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("行必须可解析"))
        .collect()
}

/// 回补主链：早落盘形态（user 行无 images）→ patch → 行内 images 到位 →
/// projected_messages_from_rows 还原出 image_refs（fork / self-heal 共享
/// 映射的 F-E 语义端到端）。
#[test]
fn patch_restores_images_for_fork_projection() {
    let k = key("main");
    cleanup(&k);
    // 模拟 admission 早落盘形态：user 行无 images；随后 assistant 行。
    append_chat_log(&k, "user", "描述这张图");
    append_chat_log(&k, "assistant", "VISION_OK:1");

    let refs = vec![r"C:\home\uploads\web_1.png".to_string()];
    patch_last_user_row_images(&k, &refs);

    let rows = rows_of(&k);
    assert_eq!(rows.len(), 2, "行数不变（原位回补非追加）");
    assert_eq!(
        rows[0]["images"],
        json!(["C:\\home\\uploads\\web_1.png"]),
        "user 行 images 回补到位"
    );
    assert!(rows[1].get("images").is_none(), "assistant 行不受影响");

    // F-E 共享映射端到端：fork / self-heal 重建据行还原 image_refs。
    let msgs = crate::session::projected_messages_from_rows(&rows);
    assert_eq!(msgs.len(), 2, "投影行数守恒");
    assert_eq!(msgs[0].image_refs, refs, "fork store 侧 image_refs 还原");
    assert!(msgs[1].image_refs.is_empty());
    cleanup(&k);
}

/// 幂等：已回补行不再二次改写（文件字节零变化）；空 images 表是 no-op。
#[test]
fn patch_is_idempotent_and_noop_on_empty() {
    let k = key("idem");
    cleanup(&k);
    append_chat_log(&k, "user", "带图轮");
    let refs = vec!["p.png".to_string()];
    patch_last_user_row_images(&k, &refs);
    let before = std::fs::read_to_string(log_path(&k)).expect("jsonl");

    patch_last_user_row_images(&k, &refs);
    let after = std::fs::read_to_string(log_path(&k)).expect("jsonl");
    assert_eq!(before, after, "已回补行不二次改写");

    patch_last_user_row_images(&k, &[]);
    assert_eq!(
        std::fs::read_to_string(log_path(&k)).expect("jsonl"),
        after,
        "空 images 表 no-op"
    );
    cleanup(&k);
}

/// steer 行（steer=true 且自带 images）不作为回补目标——更早的未回补
/// user 行才是（rev 扫描跳过 steer 命中真正的早落盘行）。
#[test]
fn patch_skips_steer_row_targets_plain_user_row() {
    let k = key("steer");
    cleanup(&k);
    append_chat_log(&k, "user", "轮首提问");
    append_chat_log_meta(
        &k,
        "user",
        "[steer] 插话",
        &ChatLogMeta {
            images: &["steer.png".to_string()],
            steer: true,
            ..Default::default()
        },
    );

    patch_last_user_row_images(&k, &["turn.png".to_string()]);

    let rows = rows_of(&k);
    assert_eq!(rows[0]["images"], json!(["turn.png"]), "轮首早落盘行被回补");
    assert_eq!(
        rows[1]["images"],
        json!(["steer.png"]),
        "steer 行自带 images 原样不动"
    );
    cleanup(&k);
}

/// 保守边界：文件含解析不出 JSON 的行 → 整体放弃（不做部分改写，
/// 字节零变化——防 filter_map 静默丢行）。
#[test]
fn patch_aborts_on_malformed_line() {
    let k = key("malformed");
    cleanup(&k);
    append_chat_log(&k, "user", "带图轮");
    // 直写追加一行裸垃圾（绕过 append 原语模拟磁盘损坏形态）。
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(log_path(&k))
            .expect("open for append");
        use std::io::Write;
        writeln!(f, "{{not-json").expect("append garbage");
    }
    let before = std::fs::read_to_string(log_path(&k)).expect("jsonl");

    patch_last_user_row_images(&k, &["p.png".to_string()]);
    assert_eq!(
        std::fs::read_to_string(log_path(&k)).expect("jsonl"),
        before,
        "存在坏行 = 整体放弃，字节零变化"
    );
    cleanup(&k);
}
