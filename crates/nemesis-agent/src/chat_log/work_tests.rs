//! Work 时间线（2026-10-30 ChatPanel 长任务时间线改造）chat_log 侧测试。
//!
//! - `append_chat_log_meta` 全字段入口：assistant 行 `"work"` 键写入形状
//!   （WorkTimeline serde 形态），`None`/直答行**不带**该键（宽容读契约，
//!   与 file_changes 同一形态）。
//! - 旧签名行（append_chat_log / _with_model）不带 work 键——存量调用点
//!   行为零变化。
//! - 这里只钉 jsonl 行形状；累加器分组语义在 work_timeline_tests。

use super::*;
use crate::work_timeline::{WorkRound, WorkToolRecord};

fn work_uniq_key(tag: &str) -> String {
    format!(
        "test:worktl:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn sample_timeline() -> WorkTimeline {
    WorkTimeline {
        started_at_ms: 1_730_000_000_000,
        ended_at_ms: 1_730_000_073_500,
        rounds: vec![
            WorkRound {
                text: "我先核对入口".to_string(),
                reasoning: "应该先看配置再动手".to_string(),
                tools: vec![WorkToolRecord {
                    tool: "exec".to_string(),
                    call_id: "c1".to_string(),
                    ok: true,
                    duration_ms: 42,
                    args_preview: "ls".to_string(),
                    result_preview: "out".to_string(),
                    running: false,
                }],
            },
            WorkRound {
                text: "纯工具轮".to_string(),
                reasoning: String::new(),
                tools: vec![],
            },
        ],
        truncated: false,
    }
}

/// assistant 行带 work 键；同 turn 的 user 行（旧签名）不带。
#[test]
fn test_work_meta_entry_writes_work_key_legacy_rows_omit() {
    let key = work_uniq_key("write");
    delete_chat_log(&key);

    let tl = sample_timeline();
    append_chat_log_meta(
        &key,
        "assistant",
        "干完了",
        &ChatLogMeta {
            model: Some("prov/model"),
            work: Some(&tl),
            ..Default::default()
        },
    );
    append_chat_log(&key, "user", "do the task");

    let (rows, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 2);

    let a = &rows[0];
    assert_eq!(a["role"].as_str(), Some("assistant"));
    let work = a.get("work").expect("work key on assistant row");
    assert_eq!(work["started_at_ms"], 1_730_000_000_000i64);
    assert_eq!(work["ended_at_ms"], 1_730_000_073_500i64);
    let rounds = work["rounds"].as_array().expect("rounds array");
    assert_eq!(rounds.len(), 2);
    // 批次二（2026-10-31）：reasoning 字段 roundtrip——有值落键、空省略
    // （旧 jsonl 无该键照常解析 = 宽容读契约）。
    assert_eq!(rounds[0]["text"], "我先核对入口");
    assert_eq!(rounds[0]["reasoning"], "应该先看配置再动手");
    assert_eq!(rounds[1]["text"], "纯工具轮");
    assert!(
        rounds[1].get("reasoning").is_none(),
        "空思考省略 reasoning 键"
    );
    let tools = rounds[0]["tools"].as_array().expect("tools array");
    assert_eq!(tools[0]["tool"], "exec");
    assert_eq!(tools[0]["ok"], true);
    assert_eq!(tools[0]["duration_ms"], 42);
    assert!(tools[0].get("running").is_none(), "落盘形态无 running 旗标");
    assert!(work.get("truncated").is_none(), "未截断不落键");

    assert!(rows[1].get("work").is_none(), "旧签名行不得带 work 键");

    delete_chat_log(&key);
}

/// 直答（work: None）行不带 work 键——serde 序列化 Only 有中间轮才发生。
#[test]
fn test_work_none_omits_field() {
    let key = work_uniq_key("none");
    delete_chat_log(&key);
    append_chat_log_meta(
        &key,
        "assistant",
        "直接回答",
        &ChatLogMeta {
            work: None,
            ..Default::default()
        },
    );
    let (rows, _, _, _) = read_chat_log(&key, 10, None);
    assert!(rows[0].get("work").is_none(), "None 不写 work 键");
    delete_chat_log(&key);
}

/// truncated 时间线落 truncated 键（诚实注记随行）。
#[test]
fn test_work_truncated_flag_persists() {
    let key = work_uniq_key("trunc");
    delete_chat_log(&key);
    let tl = WorkTimeline {
        started_at_ms: 1,
        ended_at_ms: 2,
        rounds: vec![],
        truncated: true,
    };
    append_chat_log_meta(
        &key,
        "assistant",
        "截断的活",
        &ChatLogMeta {
            work: Some(&tl),
            ..Default::default()
        },
    );
    let (rows, _, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(rows[0]["work"]["truncated"], true);
    delete_chat_log(&key);
}

/// 读侧宽容：旧行无 work 键照常解析（与既有行共存的回归锁）。
#[test]
fn test_legacy_row_without_work_still_parses() {
    let key = work_uniq_key("legacy");
    delete_chat_log(&key);
    append_chat_log(&key, "assistant", "改造前的回复");
    let (rows, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 1);
    assert_eq!(rows[0]["content"].as_str(), Some("改造前的回复"));
    assert!(rows[0].get("work").is_none());
    delete_chat_log(&key);
}
