//! S9 覆盖率批次：replay.rs 剩余未覆盖行。
//! - 165：append_projection_record 的 ledger open 失败 warn——ledger 路径
//!   预置为目录 → OpenOptions open 失败（Windows 打开目录 → access denied）。
//! - 356-359 / 499-503：结构性不可达（LlmMessage 序列化永不失败；
//!   rebuild_request_messages 所有路径都返回 Ok），见报告豁免组。

use super::*;
use crate::test_support::capture_logs;
use nemesis_path::default_path_manager;

/// append_projection_record：ledger 文件位置是目录 → open 失败 → warn +
/// return，不 panic、不影响调用方（165）。
#[test]
fn append_projection_record_blocked_by_directory_warns() {
    let _logs = capture_logs();
    let key = format!("test:s9:replay:{}", std::process::id());
    let safe_key = key.replace(':', "_");
    let ledger = default_path_manager()
        .boundary_events_dir()
        .join(format!("{}.replay.jsonl", safe_key));
    let _ = std::fs::remove_file(&ledger);
    let _ = std::fs::remove_dir_all(&ledger);
    std::fs::create_dir_all(&ledger).unwrap(); // 目录挡住 open

    let rec = RequestProjectionRecord {
        trace_id: "trace-s9".to_string(),
        session_key: key.clone(),
        round: 1,
        ts: now_rfc3339(),
        messages_count: 2,
        roles: vec!["system".to_string(), "user".to_string()],
        history_len_at_build: 2,
        injections: Vec::new(),
        voice_append: None,
        summary_as_of: None,
        vision_projected: false,
    };
    append_projection_record(&rec); // 必须不 panic
    assert!(ledger.is_dir(), "blocker intact — open failed as intended");

    // 清理 + 对照：解除阻塞后正常追加
    std::fs::remove_dir(&ledger).unwrap();
    append_projection_record(&rec);
    let loaded = load_projection_records(&key);
    assert_eq!(loaded.len(), 1, "unblocked ledger appends fine");
    assert_eq!(loaded[0].trace_id, "trace-s9");
    let _ = std::fs::remove_file(&ledger);
}
