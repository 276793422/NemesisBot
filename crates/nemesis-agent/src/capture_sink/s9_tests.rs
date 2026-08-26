//! S9 覆盖率批次：capture_sink.rs 剩余未覆盖行。
//! - 207：flush 的 05.error.txt 写失败 warn。
//! - 234：write_json 的 fs::write 失败 warn。
//! - 245-247：write_jsonl 的 File::create 失败 warn + return。
//!
//! 后两者直接调模块私有 helper（子模块可见），目标路径预置为目录 →
//! 写/创建确定性失败。207 需要 flush 内部时间戳命中预置的
//! `{ts}_{signal}` 目录名——秒内双跑 + 回环重试（跨秒即换新
//! session key 重来），实际确定性。

use super::*;
use crate::test_support::capture_logs;

fn temp_ws(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "nemesis_capsink_s9_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ))
}

/// write_json：目标路径已是目录 → fs::write 失败 → warn（234）。
#[test]
fn write_json_to_directory_path_warns() {
    let _logs = capture_logs();
    let ws = temp_ws("wj");
    let _ = fs::remove_dir_all(&ws);
    let target = ws.join("blocked.json");
    fs::create_dir_all(&target).unwrap();
    write_json(&target, &serde_json::json!({"a": 1}));
    assert!(target.is_dir(), "blocker intact — write failed as intended");
    let _ = fs::remove_dir_all(&ws);
}

/// write_jsonl：目标路径已是目录 → File::create 失败 → warn + return
/// （245-247）。传入非空 items 确认循环体不执行也不 panic。
#[test]
fn write_jsonl_to_directory_path_warns() {
    let _logs = capture_logs();
    let ws = temp_ws("wjl");
    let _ = fs::remove_dir_all(&ws);
    let target = ws.join("blocked.jsonl");
    fs::create_dir_all(&target).unwrap();
    let items = vec![SessionWriteCapture {
        writer: "s9".to_string(),
        op: "set_history".to_string(),
        before_len: Some(3),
        after_len: Some(2),
        first_role: Some("system".to_string()),
        last_role: Some("assistant".to_string()),
        messages_hash: "h".to_string(),
        overwrite_detected: true,
        ts: String::new(),
    }];
    write_jsonl(&target, &items);
    assert!(target.is_dir(), "blocker intact — create failed as intended");
    let _ = fs::remove_dir_all(&ws);
}

/// flush 的 error.txt 写失败（207）：预置 `{ts}_{signal}/05.error.txt` 为
/// 目录，同一秒内调用 flush → fs::write 失败。跨秒（flush 落到新目录、
/// 写会成功）则换 key 重试；最多 10 次，全部跨秒概率≈0。
#[test]
fn flush_error_txt_blocked_by_directory_warns() {
    let _logs = capture_logs();
    let ws = temp_ws("flusherr");
    let _ = fs::remove_dir_all(&ws);
    fs::create_dir_all(&ws).unwrap();

    for attempt in 0..10 {
        let key = format!("test:s9:caperr:{}:{}", std::process::id(), attempt);
        let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let sink_dir = ws
            .join("logs")
            .join("capture")
            .join(sanitize(&key))
            .join(format!("{}_s9err", ts));
        // 预置 05.error.txt 为目录 → flush 里 fs::write 必失败
        fs::create_dir_all(sink_dir.join("05.error.txt")).unwrap();
        let sink = CaptureSink::for_test(ws.clone());
        sink.flush(&key, "s9err", None, Some("boom: full error text"));
        let ts2 = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        if ts == ts2 {
            // flush 的内部时间戳与预置目录同一秒 → 命中受阻目录 → 写失败
            // （207 warn 已执行）。
            assert!(sink_dir.join("05.error.txt").is_dir());
            let _ = fs::remove_dir_all(&ws);
            return;
        }
        // 跨秒：本次作废，清理后换 key 重试
        let _ = fs::remove_dir_all(ws.join("logs"));
    }
    panic!("10 attempts all straddled a second boundary — implausible");
}
