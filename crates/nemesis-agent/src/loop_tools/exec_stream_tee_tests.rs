//! exec 执行中输出流 tee 测试（2026-11-08 子进程实时输出与完成唤醒）。
//!
//! 覆盖：tee=None 与旧 drain_pipe 返回值逐字节一致（返回值路径不变）、
//! EOF 兜底 flush（节流窗内最后一段不丢）、多字节安全、16KB 窗丢头保尾
//! 加 total_bytes 全量计数、节流（300ms 间隔两段喂入产生两次以上发布）、
//! ExecTool hook 装配 e2e、registry exec_output_tx → ToolOutput 广播。

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::context::RequestContext;
use crate::r#loop::Tool;
use crate::loop_tools::{ExecTool, SharedToolConfig, register_shared_tools};

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

/// (tail, total_bytes) 捕获 sink。
type Sink = Arc<std::sync::Mutex<Vec<(String, u64)>>>;

fn recorder() -> (Sink, crate::loop_tools::ExecOutputHook) {
    let sink: Sink = Arc::new(std::sync::Mutex::new(Vec::new()));
    let hook_sink = Arc::clone(&sink);
    let hook = Arc::new(move |_ctx: &RequestContext, tail: &str, total: u64| {
        hook_sink.lock().unwrap().push((tail.to_string(), total));
    });
    (sink, hook)
}

fn emits(sink: &Sink) -> Vec<(String, u64)> {
    sink.lock().unwrap().clone()
}

/// 平台 echo 单行命令。
fn echo_cmd(payload: &str) -> String {
    // 单引号防 shell 元数据干扰；payload 不含引号字符。
    #[cfg(target_os = "windows")]
    {
        format!("echo {}", payload)
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("echo '{}'", payload)
    }
}

// ---------------------------------------------------------------------------
// 返回值路径不变：tee=None 与旧 drain_pipe 逐字节一致
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tee_none_return_path_matches_drain_pipe() {
    let payload = b"identity_check_line_b4\nsecond line\n";
    let (tx1, rx1) = tokio::io::duplex(65_536);
    let (tx2, rx2) = tokio::io::duplex(65_536);
    let mut w1 = tx1;
    let mut w2 = tx2;
    w1.write_all(payload).await.unwrap();
    w2.write_all(payload).await.unwrap();
    drop(w1);
    drop(w2);
    let old = crate::loop_tools::exec::drain_pipe(Some(rx1)).await;
    let new = super::spawn_drain_pipe_tee(Some(rx2), None).await.unwrap();
    assert_eq!(old, payload.to_vec());
    assert_eq!(new, old, "tee=None 必须与旧 drain_pipe 逐字节一致");
}

// ---------------------------------------------------------------------------
// EOF 兜底 flush + 多字节 + total_bytes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tee_finish_flush_emits_tail_with_multibyte() {
    let (sink, hook) = recorder();
    let (tx, rx) = tokio::io::duplex(65_536);
    // 多字节载荷（中文）+ 换行——lossy/char 边界不被劈开。
    let payload = "你好 exec_stream_b4\n世界\n";
    let mut w = tx;
    w.write_all(payload.as_bytes()).await.unwrap();
    drop(w); // EOF → drain 收尾

    let out = super::spawn_drain_pipe_tee(
        Some(rx),
        Some(Arc::new(std::sync::Mutex::new(
            crate::loop_tools::exec::StreamTeeState::new(hook, ctx()),
        ))),
    )
    .await
    .unwrap();
    assert_eq!(out, payload.as_bytes(), "返回值路径不受 tee 影响");

    let calls = emits(&sink);
    assert!(
        !calls.is_empty(),
        "EOF 兜底 flush 必须发布至少一次（节流不应吞掉最后一段）"
    );
    let (tail, total) = &calls[calls.len() - 1];
    assert_eq!(*total, payload.len() as u64, "total_bytes = 全量字节计数");
    assert!(tail.contains("你好 exec_stream_b4"), "tail: {}", tail);
    assert!(
        tail.contains("世界"),
        "多字节必须完整（不劈 char 边界）: {}",
        tail
    );
}

// ---------------------------------------------------------------------------
// 16KB 窗：丢头保尾 + total 全量
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tee_window_keeps_tail_only_and_full_total() {
    let (sink, hook) = recorder();
    let (tx, rx) = tokio::io::duplex(65_536);
    let head = "a".repeat(20_000);
    let payload = format!("HEAD_MARKER_b4\n{}\nWINDOW_TAIL_MARKER_b4\n", head);
    let mut w = tx;
    w.write_all(payload.as_bytes()).await.unwrap();
    drop(w);

    let out = super::spawn_drain_pipe_tee(
        Some(rx),
        Some(Arc::new(std::sync::Mutex::new(
            crate::loop_tools::exec::StreamTeeState::new(hook, ctx()),
        ))),
    )
    .await
    .unwrap();
    assert_eq!(out.len(), payload.len(), "返回值不受窗裁剪");

    let calls = emits(&sink);
    assert!(!calls.is_empty());
    let (tail, total) = &calls[calls.len() - 1];
    assert_eq!(*total, payload.len() as u64);
    assert!(
        tail.contains("WINDOW_TAIL_MARKER_b4"),
        "窗必须保尾（尾部标记在场）: {}..{}",
        &tail[..40.min(tail.len())],
        tail.len()
    );
    assert!(
        tail.chars().count() <= super::EXEC_TAIL_CHARS,
        "尾窗 ≤ {} 字符 (got {})",
        super::EXEC_TAIL_CHARS,
        tail.chars().count()
    );
    assert!(
        !tail.contains("HEAD_MARKER_b4"),
        "窗必须丢头（头部标记不得进尾窗）"
    );
}

// ---------------------------------------------------------------------------
// 节流：≥300ms 间隔的两段喂入 → 至少两次发布
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tee_throttle_allows_second_emit() {
    let (sink, hook) = recorder();
    let (tx, rx) = tokio::io::duplex(65_536);
    let state = Arc::new(std::sync::Mutex::new(
        crate::loop_tools::exec::StreamTeeState::new(hook, ctx()),
    ));
    let handle = super::spawn_drain_pipe_tee(Some(rx), Some(Arc::clone(&state)));
    let mut w = tx;
    w.write_all(b"chunk_one\n").await.unwrap();
    w.flush().await.unwrap();
    // 让第一段被读走（feed，节流静默），再隔过 300ms 窗写第二段。
    tokio::time::sleep(Duration::from_millis(400)).await;
    w.write_all(b"chunk_two\n").await.unwrap();
    drop(w);

    let _out = handle.await.unwrap();
    let calls = emits(&sink);
    assert!(
        calls.len() >= 2,
        "≥300ms 间隔的两段至少触发两次发布（周期窗 + EOF flush），got {}",
        calls.len()
    );
    let (tail, total) = &calls[calls.len() - 1];
    assert!(tail.contains("chunk_two"), "最终 tail 含末段: {}", tail);
    assert_eq!(*total, ("chunk_one\nchunk_two\n").len() as u64);
}

// ---------------------------------------------------------------------------
// ExecTool hook 装配 e2e：执行中流 + 返回值不变
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exec_tool_with_hook_streams_and_return_unchanged() {
    let (sink, hook) = recorder();
    let tool = ExecTool::new_with_output_hook(".", false, hook);
    let out = tool
        .execute(
            &serde_json::json!({"command": echo_cmd("exec_stream_tool_b4")}).to_string(),
            &ctx(),
        )
        .await
        .unwrap();
    assert!(
        out.contains("exec_stream_tool_b4"),
        "返回值路径不变（echo 回显在场）: {}",
        out
    );
    let calls = emits(&sink);
    assert!(
        calls.iter().any(|(t, _)| t.contains("exec_stream_tool_b4")),
        "hook 必须收到含回显的尾窗发布: {:?}",
        calls
    );
}

/// hook=None（旧构造）→ 无 tee 面（recorder 不存在即为旧行为；这里只断言
/// 返回值契约）。
#[tokio::test]
async fn exec_tool_plain_new_return_contract() {
    let tool = ExecTool::new(".", false);
    let out = tool
        .execute(
            &serde_json::json!({"command": echo_cmd("exec_plain_b4")}).to_string(),
            &ctx(),
        )
        .await
        .unwrap();
    assert!(out.contains("exec_plain_b4"), "out: {}", out);
}

// ---------------------------------------------------------------------------
// registry 接线：exec_output_tx Some → exec 广播 ToolOutput 事件
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registry_exec_output_tx_broadcasts_tool_output() {
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let cfg = SharedToolConfig {
        workspace: Some(".".to_string()),
        exec_output_tx: Some(tx),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    let tool = tools.get("exec").expect("exec registered");
    tool.execute(
        &serde_json::json!({"command": echo_cmd("tx_tool_output_b4")}).to_string(),
        &ctx(),
    )
    .await
    .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Ok(ev)) => {
                if let nemesis_types::agent::AgentEvent::ToolOutput {
                    session_key,
                    chat_id,
                    call_id,
                    tool: t,
                    tail,
                    total_bytes,
                } = &ev
                {
                    assert_eq!(session_key, "sess1");
                    assert_eq!(chat_id, "chat1");
                    assert_eq!(call_id, "", "call_id 置空（工具层不可见 dispatch id）");
                    assert_eq!(t, "exec");
                    assert_eq!(ev.chat_id(), "chat1");
                    assert_eq!(ev.session_key(), Some("sess1"));
                    assert_eq!(ev.kind(), "ToolOutput");
                    if tail.contains("tx_tool_output_b4") && *total_bytes > 0 {
                        found = true;
                        break;
                    }
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Err(_) => continue,
            Ok(Err(_)) => break,
        }
    }
    assert!(found, "broadcast 必须送达含回显的 ToolOutput 事件");
}
