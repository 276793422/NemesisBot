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

// BUG-20261118-090 回归锁（出侧复扫同源预览净化）：本 hook 在 dispatch
// 尾部复扫之前拿到工具结果——净化器缺席时，复扫前的明文凭据会经
// result_preview 落进 work 时间线（session_log jsonl 持久化）与
// Dashboard WS/SSE 事件面，而模型上下文拿到的已是脱敏后文本（口径分裂，
// 泄漏面）。装配净化器（生产经 AgentLoop::output_preview_sanitizer 同源
// 注入；此处用打桩闭包钉 plumbing）后：事件与 work 记录的预览都必须是
// 净化后文本，返回给 dispatch 的原始 result 不得被改写（权威复扫仍在
// dispatch 尾，语义不变）。
#[tokio::test]
async fn sanitizer_masks_result_preview_but_not_chain_return() {
    let (tx, mut rx) = broadcast::channel(16);
    let work = crate::work_timeline::WorkRecorder::default();
    work.begin("web:s1:main");
    let raw = "model key: sk-live-abcdef0123456789 end";
    let hook = ToolEventHook::new_with_sanitizer(
        tx,
        work.clone(),
        Some(Arc::new(|_tool: &str, r: &str| {
            r.replace("sk-live-abcdef0123456789", "[REDACTED]")
        })),
    );

    let returned = hook
        .around_tool_use(sample_call("{}"), next_returns(raw))
        .await;
    assert_eq!(
        returned, raw,
        "hook 只净化预览副本，返回给 dispatch 的原始 result 不得改写"
    );

    let started = rx.recv().await.unwrap();
    match &started {
        AgentEvent::ToolStarted { args_preview, .. } => {
            assert_eq!(args_preview, "{}", "净化器只作用于 result 预览，args 不动");
        }
        other => panic!("expected ToolStarted, got {}", other.kind()),
    }
    let finished = rx.recv().await.unwrap();
    match &finished {
        AgentEvent::ToolFinished { result_preview, .. } => {
            assert!(
                !result_preview.contains("sk-live"),
                "事件预览不得携带复扫前原文: {result_preview}"
            );
            assert!(
                result_preview.contains("[REDACTED]"),
                "事件预览必须是净化后文本: {result_preview}"
            );
        }
        other => panic!("expected ToolFinished, got {}", other.kind()),
    }

    // work 时间线（session_log jsonl 持久化形态）同样只落净化后预览。
    let snap = work.snapshot_json("web:s1:main").expect("work snapshot");
    let serialized = serde_json::to_string(&snap).unwrap();
    assert!(
        !serialized.contains("sk-live"),
        "work 时间线不得携带复扫前原文: {serialized}"
    );
    assert!(serialized.contains("[REDACTED]"));
}

// 净化器在**截断之前**作用于完整结果：净化改写使长度越过预览上限时，
// 落盘预览的长度按净化后文本计算（先净化后截断，无截断边界漏字）。
#[tokio::test]
async fn sanitizer_runs_before_truncation() {
    let (tx, mut rx) = broadcast::channel(16);
    // 构造超长明文（> RESULT_PREVIEW_CAP），净化器把它整体替换成短标记——
    // 若先截断后净化，标记会被截掉/原文片段残留；先净化后截断则预览
    // 恰为标记 + 无原文。
    let secret = "s".repeat(RESULT_PREVIEW_CAP + 100);
    let secret_for_closure = secret.clone();
    let hook = ToolEventHook::new_with_sanitizer(
        tx,
        crate::work_timeline::WorkRecorder::default(),
        Some(Arc::new(move |_tool: &str, r: &str| {
            if r.contains(&secret_for_closure) {
                "[REDACTED_ALL]".to_string()
            } else {
                r.to_string()
            }
        })),
    );
    hook.around_tool_use(sample_call("{}"), next_returns(secret))
        .await;

    let _started = rx.recv().await.unwrap();
    let finished = rx.recv().await.unwrap();
    match finished {
        AgentEvent::ToolFinished { result_preview, .. } => {
            assert_eq!(
                result_preview, "[REDACTED_ALL]",
                "净化必须发生在截断之前（否则截断边界漏字/标记被切）"
            );
        }
        other => panic!("expected ToolFinished, got {}", other.kind()),
    }
}
