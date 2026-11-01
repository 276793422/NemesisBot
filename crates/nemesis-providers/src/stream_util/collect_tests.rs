//! L02（2026-11-01）流式中断部分输出保留：`collect_stream_response` 两臂
//! 改造回归钉——流中途 Err / channel 关闭无终态时，已聚合 partial 不再
//! 丢弃，包成 `StreamInterrupted` 随错误值携带；空 content 维持旧语义。

use super::collect_stream_response;
use crate::failover::FailoverError;
use crate::http_provider::StreamChunk;
use crate::types::ToolCall;

fn chunk(delta: &str) -> StreamChunk {
    StreamChunk {
        delta: delta.to_string(),
        ..Default::default()
    }
}

fn unknown_err() -> FailoverError {
    FailoverError::Unknown {
        provider: "p".to_string(),
        message: "boom".to_string(),
    }
}

#[tokio::test]
async fn err_arm_empty_content_passes_original_error() {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(Err(unknown_err())).await.unwrap();
    drop(tx);
    let err = collect_stream_response("p", "m", rx).await.unwrap_err();
    match err {
        FailoverError::Unknown { message, .. } => assert_eq!(message, "boom"),
        other => panic!("空 content 必须原错误原样上抛（诚实分类不吞）: {other:?}"),
    }
}

#[tokio::test]
async fn err_arm_whitespace_only_content_treated_as_empty() {
    // 对抗 A5：空白 partial 视同空（trim 判空口径与 None 臂一致）。
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(Ok(chunk("  \n\t"))).await.unwrap();
    tx.send(Err(unknown_err())).await.unwrap();
    drop(tx);
    let err = collect_stream_response("p", "m", rx).await.unwrap_err();
    match err {
        FailoverError::Unknown { message, .. } => assert_eq!(message, "boom"),
        other => panic!("空白 partial 视同空，原错误原样上抛: {other:?}"),
    }
}

#[tokio::test]
async fn err_arm_partial_carried_as_stream_interrupted() {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(Ok(chunk("第一段"))).await.unwrap();
    tx.send(Ok(chunk("第二段"))).await.unwrap();
    tx.send(Err(unknown_err())).await.unwrap();
    drop(tx);
    let err = collect_stream_response("p", "m", rx).await.unwrap_err();
    match err {
        FailoverError::StreamInterrupted {
            provider,
            model,
            reason,
            partial_content,
            has_partial_tool_calls,
            ..
        } => {
            assert_eq!(provider, "p");
            assert_eq!(model, "m");
            assert_eq!(
                reason, "unknown error from provider p: boom",
                "根因链不丢（lane 原错误完整 Display 入 reason）"
            );
            assert_eq!(partial_content, "第一段第二段");
            assert!(!has_partial_tool_calls);
        }
        other => panic!("非空 partial 必须包成 StreamInterrupted: {other:?}"),
    }
}

#[tokio::test]
async fn err_arm_tool_calls_flag_set() {
    let mut with_tools = chunk("");
    with_tools.tool_calls = vec![ToolCall {
        id: "call_1".to_string(),
        call_type: Some("function".to_string()),
        function: None,
        name: Some("exec".to_string()),
        arguments: None,
    }];
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(Ok(chunk("部分文本"))).await.unwrap();
    tx.send(Ok(with_tools)).await.unwrap();
    tx.send(Err(unknown_err())).await.unwrap();
    drop(tx);
    let err = collect_stream_response("p", "m", rx).await.unwrap_err();
    match err {
        FailoverError::StreamInterrupted {
            has_partial_tool_calls,
            ..
        } => assert!(has_partial_tool_calls, "tool_calls 出现过必须置旗标"),
        other => panic!("expected StreamInterrupted: {other:?}"),
    }
}

#[tokio::test]
async fn none_arm_empty_is_timeout() {
    // channel 关闭无终态 + 空 content → 维持旧语义 Timeout（可重试）。
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    drop(tx);
    let err = collect_stream_response("p", "m", rx).await.unwrap_err();
    assert!(
        matches!(err, FailoverError::Timeout { .. }),
        "空 content 无终态维持旧 Timeout 语义: {err:?}"
    );
}

#[tokio::test]
async fn none_arm_partial_carried_with_fixed_reason() {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(Ok(chunk("半截"))).await.unwrap();
    drop(tx);
    let err = collect_stream_response("p", "m", rx).await.unwrap_err();
    match err {
        FailoverError::StreamInterrupted {
            reason,
            partial_content,
            has_partial_tool_calls,
            ..
        } => {
            assert_eq!(reason, "stream closed without finish_reason");
            assert_eq!(partial_content, "半截");
            assert!(!has_partial_tool_calls);
        }
        other => panic!("非空 partial 无终态必须包成 StreamInterrupted: {other:?}"),
    }
}

#[tokio::test]
async fn happy_path_with_finish_reason_unchanged() {
    // 正常完成路径零变化（guard：改造没有误伤 happy path）。
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(Ok(chunk("你好"))).await.unwrap();
    tx.send(Ok(StreamChunk {
        delta: "，世界".to_string(),
        finish_reason: Some("stop".to_string()),
        ..Default::default()
    }))
    .await
    .unwrap();
    drop(tx);
    let resp = collect_stream_response("p", "m", rx).await.unwrap();
    assert_eq!(resp.content, "你好，世界");
    assert_eq!(resp.finish_reason, "stop");
}
