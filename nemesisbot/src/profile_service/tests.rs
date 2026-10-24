//! profile_service 单测：collect_user_messages_after 的时间序与证据窗边界
//! （BUG-20261025-21：此前按文件序返回，反思通道「近期」采样取到的是
//! session key 字典序尾部的会话尾巴，不是时间上最新的消息）。

use super::*;

/// tempdir 内摆 session_logs 夹具：`name` 为文件名（.jsonl），rows 为
/// (RFC3339 ts, content)。
fn seed_log(ws: &Path, name: &str, rows: &[(&str, &str)]) {
    let dir = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&dir).unwrap();
    let body: String = rows
        .iter()
        .map(|(ts, content)| {
            serde_json::json!({"role": "user", "timestamp": ts, "content": content}).to_string()
        })
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(dir.join(name), body).unwrap();
}

/// 时间升序：字典序靠前的文件（a.jsonl）装**新**消息、靠后的（z.jsonl）
/// 装**旧**消息时，返回序必须仍是时间序（旧实现按文件序返回 → 反思采样
/// rev().take(60) 取到旧会话尾巴、漏掉新会话）。
#[test]
fn collect_orders_by_timestamp_not_filename() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_log(
        ws,
        "a.jsonl",
        &[
            ("2026-10-25T10:00:00Z", "新会话消息一"),
            ("2026-10-25T10:01:00Z", "新会话消息二"),
        ],
    );
    seed_log(
        ws,
        "z.jsonl",
        &[
            ("2026-10-25T08:00:00Z", "旧会话消息一"),
            ("2026-10-25T08:01:00Z", "旧会话消息二"),
        ],
    );
    let out = collect_user_messages_after(ws, 0).unwrap();
    assert_eq!(
        out,
        vec![
            "旧会话消息一",
            "旧会话消息二",
            "新会话消息一",
            "新会话消息二"
        ],
        "按时间升序，与文件名字典序解耦"
    );
}

/// 证据窗边界：早于 window_start 的行被排除（毫秒精度）。
#[test]
fn collect_respects_window_start_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_log(
        ws,
        "a.jsonl",
        &[
            ("2026-10-25T10:00:00Z", "窗内"),
            ("2026-10-25T09:00:00Z", "窗外"),
        ],
    );
    // 2026-10-25T10:00:00Z = 1792922400000 ms。
    let out = collect_user_messages_after(ws, 1_792_922_400_000).unwrap();
    assert_eq!(out, vec!["窗内"]);
    // 窗起点 = 窗外消息自身的时间戳（09:00Z）→ 恰好含（>= 语义），时间升序。
    let out = collect_user_messages_after(ws, 1_792_918_800_000).unwrap();
    assert_eq!(out, vec!["窗外", "窗内"]);
}

/// 非 jsonl / 坏行 / 非 user 行跳过；缺目录 = Ok(空)。
#[test]
fn collect_skips_non_user_and_missing_dir_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert_eq!(
        collect_user_messages_after(ws, 0).unwrap(),
        Vec::<String>::new()
    );

    let log_dir = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(
        log_dir.join("a.jsonl"),
        concat!(
            "not json\n",
            "{\"role\":\"assistant\",\"timestamp\":\"2026-10-25T10:00:00Z\",\"content\":\"bot\"}\n",
            "{\"role\":\"user\",\"timestamp\":\"garbage\",\"content\":\"无时间锚不参与\"}\n",
            "{\"role\":\"user\",\"timestamp\":\"2026-10-25T10:00:00Z\",\"content\":\"有效\"}\n",
        ),
    )
    .unwrap();
    std::fs::write(log_dir.join("b.txt"), "ignored\n").unwrap();
    let out = collect_user_messages_after(ws, 0).unwrap();
    assert_eq!(out, vec!["有效"]);
}
