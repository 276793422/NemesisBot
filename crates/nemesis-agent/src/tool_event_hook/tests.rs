//! ToolEventHook 单元测试。

use std::sync::Arc;

use super::*;

/// 构造一个恒定返回 `output` 的链尾 next 闭包（around 测试桩）。
fn next_returns(
    output: impl Into<String> + Send + 'static,
) -> Arc<
    dyn Fn(HookToolCall) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
        + Send
        + Sync,
> {
    let output = output.into();
    Arc::new(move |_call| {
        let output = output.clone();
        Box::pin(async move { output })
    })
}

fn sample_call(args: &str) -> HookToolCall {
    HookToolCall {
        name: "exec".to_string(),
        arguments: args.to_string(),
        channel: "web".to_string(),
        chat_id: "web:s1".to_string(),
        session_key: "web:s1:main".to_string(),
        dispatch_started: None,
        call_id: None,
    }
}

#[tokio::test]
async fn started_then_finished_emitted_in_order_with_fields() {
    let (tx, mut rx) = broadcast::channel(16);
    let hook = ToolEventHook::new(tx, crate::work_timeline::WorkRecorder::default());

    let result = hook
        .around_tool_use(
            sample_call(r#"{"command":"ls"}"#),
            next_returns("file list"),
        )
        .await;
    assert_eq!(result, "file list", "观察 hook 不得改写结果");

    let started = rx.recv().await.unwrap();
    assert_eq!(started.kind(), "ToolStarted");
    match &started {
        AgentEvent::ToolStarted {
            session_key,
            chat_id,
            tool,
            args_preview,
            ..
        } => {
            assert_eq!(session_key, "web:s1:main");
            assert_eq!(chat_id, "web:s1");
            assert_eq!(tool, "exec");
            assert_eq!(args_preview, r#"{"command":"ls"}"#);
        }
        other => panic!("expected ToolStarted, got {}", other.kind()),
    }

    let finished = rx.recv().await.unwrap();
    assert_eq!(finished.kind(), "ToolFinished");
    match &finished {
        AgentEvent::ToolFinished {
            ok, result_preview, ..
        } => {
            assert!(ok);
            assert_eq!(result_preview, "file list");
        }
        other => panic!("expected ToolFinished, got {}", other.kind()),
    }

    // 同一 call_id 串起 Started/Finished 配对。
    let sid = match (&started, &finished) {
        (
            AgentEvent::ToolStarted { call_id: a, .. },
            AgentEvent::ToolFinished { call_id: b, .. },
        ) => a == b,
        _ => false,
    };
    assert!(sid, "Started/Finished call_id 必须一致");
}

#[tokio::test]
async fn tool_error_and_security_block_mark_ok_false() {
    let (tx, mut rx) = broadcast::channel(16);
    let hook = ToolEventHook::new(tx, crate::work_timeline::WorkRecorder::default());

    hook.around_tool_use(sample_call("{}"), next_returns("Tool error: boom"))
        .await;
    hook.around_tool_use(sample_call("{}"), next_returns("⛔ SECURITY BLOCKED: no"))
        .await;

    // 第一个调用的 Finished：ok=false，原样透传。
    let _started = rx.recv().await.unwrap();
    let fin1 = rx.recv().await.unwrap();
    match fin1 {
        AgentEvent::ToolFinished {
            ok, result_preview, ..
        } => {
            assert!(!ok);
            assert_eq!(result_preview, "Tool error: boom");
        }
        other => panic!("expected ToolFinished, got {}", other.kind()),
    }

    // 第二个调用的 Started + Finished。
    let _started = rx.recv().await.unwrap();
    let fin2 = rx.recv().await.unwrap();
    match fin2 {
        AgentEvent::ToolFinished { ok, .. } => assert!(!ok, "SECURITY BLOCKED 文案应判失败"),
        other => panic!("expected ToolFinished, got {}", other.kind()),
    }
}

// BUG-20261112-109 回归锁（事件活面）：未知工具拒绝（dispatch registry
// None 臂回灌 `Error: Unknown tool '<名>'`）经 around 链发 ToolFinished
// 必须 ok=false——此前 classify_result 漏配该前缀，落到默认成功臂，观测
// 面假绿（Dashboard 工具卡片把「调用根本没执行」显示成成功）。
#[tokio::test]
async fn unknown_tool_rejection_finish_event_is_not_ok() {
    let (tx, mut rx) = broadcast::channel(16);
    let hook = ToolEventHook::new(tx, crate::work_timeline::WorkRecorder::default());

    let result = hook
        .around_tool_use(
            sample_call("{}"),
            next_returns("Error: Unknown tool 'append_file'"),
        )
        .await;
    assert_eq!(result, "Error: Unknown tool 'append_file'");

    let _started = rx.recv().await.unwrap();
    let finished = rx.recv().await.unwrap();
    match finished {
        AgentEvent::ToolFinished {
            ok, result_preview, ..
        } => {
            assert!(!ok, "未知工具拒绝必须判失败（观测面不得假绿）");
            assert!(result_preview.contains("Unknown tool"));
        }
        other => panic!("expected ToolFinished, got {}", other.kind()),
    }
}

#[tokio::test]
async fn previews_truncated_on_char_boundary() {
    let (tx, mut rx) = broadcast::channel(16);
    let hook = ToolEventHook::new(tx, crate::work_timeline::WorkRecorder::default());

    // 多字节字符 + 超长：截断不得 panic（str-slice 教训）。
    let long_args = "路".repeat(ARGS_PREVIEW_CAP + 50);
    let long_result = "∞".repeat(RESULT_PREVIEW_CAP + 100);
    hook.around_tool_use(sample_call(&long_args), next_returns(long_result.clone()))
        .await;

    let started = rx.recv().await.unwrap();
    match started {
        AgentEvent::ToolStarted { args_preview, .. } => {
            assert_eq!(args_preview.chars().count(), ARGS_PREVIEW_CAP + 1); // + 截断符
        }
        other => panic!("expected ToolStarted, got {}", other.kind()),
    }
    let finished = rx.recv().await.unwrap();
    match finished {
        AgentEvent::ToolFinished { result_preview, .. } => {
            assert_eq!(result_preview.chars().count(), RESULT_PREVIEW_CAP + 1);
        }
        other => panic!("expected ToolFinished, got {}", other.kind()),
    }
}

#[tokio::test]
async fn no_subscriber_is_not_an_error() {
    let (tx, _rx) = broadcast::channel::<AgentEvent>(16);
    drop(_rx); // 无订阅者：send 返回 Err，hook 必须照常工作。

    let hook = ToolEventHook::new(tx, crate::work_timeline::WorkRecorder::default());
    let result = hook
        .around_tool_use(sample_call("{}"), next_returns("fine"))
        .await;
    assert_eq!(result, "fine");
}

#[test]
fn serde_kind_tag_and_chat_id_accessor() {
    let ev = AgentEvent::ToolStarted {
        session_key: "web:s1:main".into(),
        chat_id: "web:s1".into(),
        call_id: "c1".into(),
        tool: "exec".into(),
        args_preview: "{}".into(),
    };
    let json = serde_json::to_value(&ev).unwrap();
    assert_eq!(json["kind"], "ToolStarted");
    assert_eq!(json["data"]["tool"], "exec");
    assert_eq!(ev.chat_id(), "web:s1");

    let round: AgentEvent = serde_json::from_value(json).unwrap();
    assert_eq!(round.kind(), "ToolStarted");

    let done = AgentEvent::TodoUpdated {
        session_key: "s".into(),
        chat_id: "web:s2".into(),
        todos: Vec::new(),
    };
    assert_eq!(done.kind(), "TodoUpdated");
    assert_eq!(done.chat_id(), "web:s2");
}

// wave5：ToolHook::name 恒等（57-59）。
#[tokio::test]
async fn hook_name_is_tool_event() {
    let (tx, _rx) = tokio::sync::broadcast::channel(4);
    let hook = ToolEventHook::new(tx, crate::work_timeline::WorkRecorder::default());
    assert_eq!(ToolHook::name(&hook), "tool-event");
}
