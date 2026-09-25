//! websocket_handler.rs AGT 覆盖率批次（2026-09-25）。与 tests / extra_tests /
//! s10b 三套互补，聚焦仍缺的确定性臂（直调私有函数，免真实套接字）：
//! - run_send_writer：lo 通道先关（hi 发送端存活）→ lo recv_many=0 的
//!   收尾 break 臂（既有测试只走过 hi 关闭臂）
//! - run_send_writer：hi 批量捎带臂（recv_many 吃满 SEND_QUEUE_HI_BATCH 后
//!   try_recv 仍取到剩余帧——需一次排入 >64 帧）
//! - handle_chat_send：media 引用解析失败 + content 非空 → 补换行注记臂；
//!   workflow_edit → metadata 写键臂
//! - broadcast_to_session：debug! 字段求值（无 subscriber 时 debug! 宏在
//!   字段求值前短路；用线程内 with_default 确定性装载，避免全局槽竞争）
//!
//! 结构性豁免（见报告）：
//! - 402 / 428 收括号行：lcov 归因伪零——if-let 体（400-401=2、424=1）均
//!   有正计数，体执行而括号行永不计数，任何测试无法改变。
//! - 503-509 读流 None 收尾臂 + `_ => {}` 兜底：主循环对 Close 帧即时
//!   break（486），优雅关闭永远先落 Close 臂；异常断连在 axum 侧以
//!   Some(Err) 呈现（494-501，已被覆盖）。None 臂要求「无 Close 帧的干净
//!   流尾」，tungstenite 客户端无法确定性制造；`_` 兜底对当前 Message
//!   枚举五变体为空集。两者均不可达。

use super::*;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// run_send_writer：假 sink + 双通道直驱
// ---------------------------------------------------------------------------

/// 假 sink：把喂进来的 Text 帧记进共享表，flush/close 全部成功。
struct AgtVecSink(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl futures::Sink<Message> for AgtVecSink {
    type Error = std::convert::Infallible;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        if let Message::Text(t) = item {
            self.0.lock().unwrap().push(t.to_string());
        }
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn agt_writer_lo_closed_breaks_writer() {
    let (hi_tx, hi_rx) = mpsc::channel(8);
    let (lo_tx, lo_rx) = mpsc::channel(8);
    let (done_tx, done_rx) = tokio::sync::watch::channel(false);

    // 1 帧 hi，lo 关闭（且已排空）；hi 发送端存活 → hi 臂阻塞时 lo 臂
    // recv_many=0 → 走 lo 收尾 break。
    hi_tx.send(b"hi-frame".to_vec()).await.unwrap();
    drop(lo_tx);

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = run_send_writer(
        AgtVecSink(seen.clone()),
        hi_rx,
        lo_rx,
        done_tx,
        "agt-lo".to_string(),
    );
    tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .expect("writer 必须经 lo 关闭臂退出（不能挂在 hi 臂上）");
    assert_eq!(*seen.lock().unwrap(), vec!["hi-frame".to_string()]);
    assert!(*done_rx.borrow(), "done 已置位");
}

#[tokio::test]
async fn agt_writer_hi_piggyback_takes_frames_beyond_batch() {
    let (hi_tx, hi_rx) = mpsc::channel(256);
    let (lo_tx, lo_rx) = mpsc::channel(256);
    let (done_tx, _done_rx) = tokio::sync::watch::channel(false);

    // 68 帧 hi（> 64 批量）：recv_many 吃 64，try_recv 捎带走剩余 4。
    for i in 0..(SEND_QUEUE_HI_BATCH + 4) {
        hi_tx.send(format!("h{i}").into_bytes()).await.unwrap();
    }
    for i in 0..3 {
        lo_tx.send(format!("l{i}").into_bytes()).await.unwrap();
    }
    drop(hi_tx);
    drop(lo_tx);

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = run_send_writer(
        AgtVecSink(seen.clone()),
        hi_rx,
        lo_rx,
        done_tx,
        "agt-hi".to_string(),
    );
    tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .expect("writer 应正常收尾");

    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen.len(),
        SEND_QUEUE_HI_BATCH + 7,
        "68 hi + 3 lo 全部送达: {seen:?}"
    );
    // hi 全部排在 lo 前（捎带帧并入同批、hi 优先）。
    assert!(
        seen[..SEND_QUEUE_HI_BATCH + 4]
            .iter()
            .all(|f| f.starts_with('h')),
        "前 68 帧必须是 hi: {seen:?}"
    );
    assert!(
        seen[SEND_QUEUE_HI_BATCH + 4..]
            .iter()
            .all(|f| f.starts_with('l')),
        "末 3 帧必须是 lo: {seen:?}"
    );
}

// ---------------------------------------------------------------------------
// handle_chat_send：无效 media 注记 + workflow_edit metadata
// ---------------------------------------------------------------------------

#[test]
fn agt_chat_send_invalid_media_notes_and_workflow_edit_metadata() {
    // 两个无效项：id 合法但文件不存在（上传表查无）+ 完全缺 id/path。
    // content 非空 → 注记前补 '\n'（648 臂）。
    let raw = serde_json::json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": {
            "content": "看这张图",
            "media": [{ "id": "agt-missing-9.png" }, { "foo": 1 }],
            "workflow_edit": { "workflow_name": "wf-agt" },
        },
    })
    .to_string();
    let out = handle_text_message("s1", "w:s1", "w:s1", raw.as_bytes())
        .unwrap()
        .unwrap();

    assert!(
        out.content.starts_with("看这张图\n"),
        "原内容保留且补换行: {}",
        out.content
    );
    assert!(
        out.content
            .contains("[图片未附加: 附件无效或已过期 (agt-missing-9.png)]"),
        "{:?}",
        out.content
    );
    assert!(
        out.content
            .contains("[图片未附加: 附件无效或已过期 (<无效项>)]"),
        "{:?}",
        out.content
    );
    // workflow_edit → metadata 唯一合法封装写键。
    assert_eq!(
        out.metadata.get("workflow_edit").map(String::as_str),
        Some(r#"{"workflow_name":"wf-agt"}"#)
    );
}

// ---------------------------------------------------------------------------
// broadcast_to_session：debug! 字段求值
// ---------------------------------------------------------------------------

/// 最小 tracing subscriber：放行 DEBUG 及以上，使 debug! 宏的字段表达式
/// 真正求值（无 subscriber 时宏在字段求值前短路）。
struct AgtDbgSubscriber;
impl tracing::Subscriber for AgtDbgSubscriber {
    fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
        meta.level() <= &tracing::Level::DEBUG
    }
    fn new_span(&self, _attrs: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::Id) {}
    fn exit(&self, _span: &tracing::Id) {}
}

#[test]
fn agt_broadcast_to_session_debug_fields_evaluated() {
    let sm = SessionManager::with_default_timeout();
    // 线程内确定性装载（不占全局槽，避免与并行测试互斥），同步驱动到完成
    // （无注册队列 → 前置 debug! + record 后诚实 Err，同既有无队列测试语义）。
    let fut = broadcast_to_session(&sm, "agt-bcast", "assistant", "hello broadcast");
    let res =
        tracing::subscriber::with_default(AgtDbgSubscriber, || futures::executor::block_on(fut));
    assert!(res.is_err(), "无注册队列必须 Err");
}
