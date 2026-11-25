//! 会话错误记录框架单测（v1，2026-11-25）。
//!
//! 纪律：直接构造 `SessionErrorRecorder::new` 实例，**不碰 OnceLock 全局**
//! （与 session/tests.rs 的 CaptureSink「测试二进制唯一 init 调用者」毒化
//! 规避同款）；生产路径的薄包装（global()）由编译器保证形态，行为与实例
//! 方法一致。覆盖：四元组完整性 / JSONL 两行追加 / disabled 零副作用 /
//! fail-open / 图片引用降维 / 分类锚点 / hook_block 合并行。

use std::path::PathBuf;

use super::llm_types::{LlmMessage, LlmResponse};
use super::session_error_recorder::{
    SessionErrorOutcome, SessionErrorRecorder, classify_session_error, snapshot_request,
};
use crate::image_attach::LlmImage;
use crate::types::{ChatOptions, ToolDefinition, ToolFunctionDef};

fn temp_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "nb-se-recorder-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn today_file(dir: &std::path::Path) -> PathBuf {
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    dir.join(format!("{}.jsonl", day))
}

fn read_lines(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let content = std::fs::read_to_string(today_file(dir)).unwrap();
    content
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn sample_message() -> LlmMessage {
    LlmMessage {
        role: "user".to_string(),
        content: "帮我把登录页改成深色主题".to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }
}

fn sample_tool_defs() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDef {
            name: "exec".to_string(),
            description: "run a shell command".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }]
}

fn sample_response() -> LlmResponse {
    LlmResponse {
        content: "被 hook 拒绝的响应内容".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        finish_reason: Some("stop".to_string()),
        escalated_to: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

#[test]
fn occurrence_then_outcome_recovered_two_lines() {
    let dir = temp_dir("two-lines");
    let rec = SessionErrorRecorder::new(dir.clone(), true);
    let opts = ChatOptions {
        reasoning_effort: Some("medium".to_string()),
        ..Default::default()
    };
    let snap = snapshot_request("m-1", &[sample_message()], &sample_tool_defs(), &opts);
    rec.record_occurrence(
        "agent:main:session:s1",
        "trace-x",
        3,
        "m-1",
        Some(&snap),
        "429 Too Many Requests: rate limited",
    );
    rec.record_outcome(
        "trace-x",
        3,
        &SessionErrorOutcome::recovered("rate_limit", None),
    );

    let lines = read_lines(&dir);
    assert_eq!(lines.len(), 2, "episode = 发生行 + outcome 行");

    let occ = &lines[0];
    assert_eq!(occ["kind"], "occurrence");
    assert_eq!(occ["trace_id"], "trace-x");
    assert_eq!(occ["round"], 3);
    assert_eq!(occ["model"], "m-1");
    assert_eq!(occ["sid"], "agent:main:session:s1");
    assert_eq!(occ["event"]["class"], "rate_limit");
    assert!(
        occ["event"]["raw"]
            .as_str()
            .unwrap()
            .contains("429 Too Many Requests")
    );
    // 四元组之请求：messages 完整 + tool_defs 完整 + opts 完整 + model。
    assert_eq!(occ["request"]["model"], "m-1");
    assert_eq!(
        occ["request"]["messages"][0]["content"],
        "帮我把登录页改成深色主题"
    );
    assert_eq!(occ["request"]["messages"].as_array().unwrap().len(), 1);
    assert_eq!(occ["request"]["tool_defs"][0]["function"]["name"], "exec");
    assert_eq!(occ["request"]["opts"]["reasoning_effort"], "medium");
    // 四元组之应答：错误 episode 的应答 = 上游错误原文。
    assert_eq!(
        occ["response"]["raw"],
        "429 Too Many Requests: rate limited"
    );
    assert_eq!(occ["outcome"], "pending");

    let out = &lines[1];
    assert_eq!(out["kind"], "outcome");
    assert_eq!(out["trace_id"], "trace-x");
    assert_eq!(out["round"], 3);
    assert_eq!(out["outcome"]["recovered_by"], "rate_limit");
    // 4b 次数不外露：retries 键诚实省略（skip_serializing_if）。
    assert!(out["outcome"].get("retries").is_none());
    assert!(out["outcome"].get("terminal").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn terminal_outcome_includes_outbound() {
    let dir = temp_dir("terminal");
    let rec = SessionErrorRecorder::new(dir.clone(), true);
    rec.record_occurrence(
        "sid-t",
        "trace-t",
        1,
        "m-1",
        None,
        "session_blocked_by_cyber_policy: content blocked",
    );
    rec.record_outcome(
        "trace-t",
        1,
        &SessionErrorOutcome::terminal(
            "no_retry",
            None,
            Some("Error: 已被拒绝（提示）".to_string()),
        ),
    );

    let lines = read_lines(&dir);
    assert_eq!(lines.len(), 2);
    // 会话级封禁：sub 锚命中 + class 归 Forbidden。
    assert_eq!(lines[0]["event"]["class"], "forbidden");
    assert_eq!(lines[0]["event"]["sub"], "session_blocked");
    let out = &lines[1];
    assert_eq!(out["outcome"]["terminal"], "no_retry");
    assert_eq!(out["outcome"]["outbound"], "Error: 已被拒绝（提示）");
    assert!(out["outcome"].get("recovered_by").is_none());
    // 无环终局：retries 键诚实省略。
    assert!(out["outcome"].get("retries").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn disabled_recorder_writes_nothing() {
    let dir = temp_dir("disabled");
    let rec = SessionErrorRecorder::new(dir.clone(), false);
    assert!(!rec.is_enabled());
    let snap = snapshot_request("m", &[sample_message()], &[], &ChatOptions::default());
    rec.record_occurrence("sid", "trace", 0, "m", Some(&snap), "boom");
    rec.record_outcome(
        "trace",
        0,
        &SessionErrorOutcome::recovered("transient", Some(2)),
    );
    rec.record_hook_block(
        "sid",
        "trace",
        0,
        "m",
        Some(&snap),
        "reason",
        &sample_response(),
    );
    // 零成本旁路：文件与目录都不落。
    assert!(!today_file(&dir).exists());
    assert!(!dir.exists());
}

#[test]
fn fail_open_broken_dir_no_panic() {
    // 目录位置被一个**文件**占用：create_dir_all/open 都会失败——记录调用
    // 必须静默丢弃（warn），绝不 panic、绝不影响 turn。
    let base = temp_dir("fail-open");
    std::fs::create_dir_all(&base).unwrap();
    let occupied = base.join("occupied");
    std::fs::write(&occupied, "i am a file").unwrap();
    let rec = SessionErrorRecorder::new(occupied.clone(), true);
    rec.record_occurrence("sid", "trace", 0, "m", None, "boom");
    rec.record_outcome(
        "trace",
        0,
        &SessionErrorOutcome::terminal("no_retry", None, None),
    );
    // 文件原样存在（没有被目录化/破坏）。
    assert!(occupied.is_file());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn snapshot_image_downscale_hash_not_bytes() {
    // 图片引用降维（拍板 A）：base64 字节不落记录，换 sha256 引用 + 长度；
    // path/media_type 保留；正文文本字节完整。
    let msg = LlmMessage {
        role: "user".to_string(),
        content: "看这张图".to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: vec![LlmImage {
            path: "/uploads/a.png".to_string(),
            media_type: "image/png".to_string(),
            data: "aGVsbG8gcG5nIGJ5dGVz".to_string(),
        }],
    };
    let snap = snapshot_request("m-v", &[msg], &[], &ChatOptions::default());
    let v = serde_json::to_value(&snap).unwrap();
    let img = &v["messages"][0]["images"][0];
    assert_eq!(img["path"], "/uploads/a.png");
    assert_eq!(img["media_type"], "image/png");
    assert_eq!(img["data_base64_len"], 20);
    // data 换降维注记，base64 字节绝不入记录。
    assert!(img["data"].as_str().unwrap().contains("引用降维"));
    assert!(
        !serde_json::to_string(&v)
            .unwrap()
            .contains("aGVsbG8gcG5nIGJ5dGVz")
    );
    // hash 可对账：对被降维的同一 base64 串求 sha256 应一致。
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"aGVsbG8gcG5nIGJ5dGVz");
    assert_eq!(img["data_sha256"], format!("{:x}", hasher.finalize()));
    // 正文文本不裁剪。
    assert_eq!(v["messages"][0]["content"], "看这张图");
}

#[test]
fn classify_anchors_sub_and_class() {
    // 会话级封禁（码族形态起头 → class_by_code 命中 Forbidden）。
    let (c, sub) = classify_session_error("session_blocked_by_cyber_policy: content blocked");
    assert_eq!(c, "forbidden");
    assert_eq!(sub, Some("session_blocked"));
    // connect error（诚实化前缀）→ timeout + sub。
    let (c, sub) = classify_session_error("connect error (model m): tcp reset by peer");
    assert_eq!(c, "timeout");
    assert_eq!(sub, Some("connect_error"));
    // 流中断前缀 → timeout + sub。
    let (c, sub) =
        classify_session_error("Stream interrupted from provider x: partial content saved");
    assert_eq!(c, "timeout");
    assert_eq!(sub, Some("stream_interrupted"));
    // 已知类无子形态。
    let (c, sub) = classify_session_error("429 Too Many Requests");
    assert_eq!(c, "rate_limit");
    assert_eq!(sub, None);
    // 分类盲区 → unknown（原文全文在 event.raw 不丢信息）。
    let (c, sub) = classify_session_error("mysterious upstream hiccup");
    assert_eq!(c, "unknown");
    assert_eq!(sub, None);
}

#[test]
fn hook_block_merged_line() {
    let dir = temp_dir("hook-block");
    let rec = SessionErrorRecorder::new(dir.clone(), true);
    let snap = snapshot_request("m-h", &[sample_message()], &[], &ChatOptions::default());
    rec.record_hook_block(
        "sid-h",
        "trace-h",
        2,
        "m-h",
        Some(&snap),
        "policy violated",
        &sample_response(),
    );

    let lines = read_lines(&dir);
    assert_eq!(lines.len(), 1, "hook Block = 发生+终局合并单行");
    let l = &lines[0];
    assert_eq!(l["kind"], "hook_block");
    assert_eq!(l["event"]["class"], "hook_block");
    assert_eq!(l["event"]["sub"], serde_json::Value::Null);
    assert_eq!(l["event"]["raw"], "policy violated");
    assert_eq!(l["sid"], "sid-h");
    assert_eq!(l["request"]["model"], "m-h");
    assert_eq!(l["response"]["raw"], "被 hook 拒绝的响应内容");
    assert_eq!(l["outcome"]["terminal"], "hook_block");

    let _ = std::fs::remove_dir_all(&dir);
}
