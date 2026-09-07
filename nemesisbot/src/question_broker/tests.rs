//! F7：WebQuestionBroker 单元测试。
//!
//! 覆盖八条路径：作答送达 + pending 清理 / 超时 Timeout 结局 + 迟到 respond
//! unknown / 双 respond 竞速先到先得 / 载荷校验（空选择、非候选、单选多项
//! ——校验失败请求留在 pending 可重试）/ pending 列表元数据 / 广播事件
//! （QuestionAsked 载荷 + QuestionResolved answered/timeout 两路）。等待侧
//! 测试在纯同步线程跑（走 `recv` 直等分支）。

use super::WebQuestionBroker;
use nemesis_types::agent::{
    AgentEvent, QuestionAsker, QuestionOutcome, QuestionRequest, QuestionResponder,
};
use std::sync::mpsc;
use std::time::Duration;

fn make_req(id: &str, multi: bool) -> QuestionRequest {
    QuestionRequest {
        question_id: id.to_string(),
        question: "用哪个包管理器?".to_string(),
        options: vec!["pnpm".to_string(), "npm".to_string()],
        multi,
        chat_id: "chat-1".to_string(),
        session_key: "web:chat-1".to_string(),
        timeout_secs: 10,
    }
}

fn broker_with_events() -> (
    std::sync::Arc<WebQuestionBroker>,
    tokio::sync::mpsc::Receiver<serde_json::Value>,
) {
    let (btx, mut brx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let (ctx, crx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        while let Ok(ev) = brx.recv().await {
            let _ = ctx.send(serde_json::to_value(&ev).unwrap()).await;
        }
    });
    (std::sync::Arc::new(WebQuestionBroker::new(Some(btx))), crx)
}

/// 在独立线程发起提问（模拟工具 spawn_blocking 上下文）。
fn ask_async(
    broker: std::sync::Arc<WebQuestionBroker>,
    id: &str,
    timeout_secs: u64,
) -> mpsc::Receiver<Result<QuestionOutcome, String>> {
    let (tx, rx) = mpsc::channel();
    let req = {
        let mut r = make_req(id, false);
        r.timeout_secs = timeout_secs;
        r
    };
    std::thread::spawn(move || {
        let _ = tx.send(broker.ask(req));
    });
    rx
}

/// 等待提问线程进入 pending（轮询代替裸 sleep，避免慢机 flake）。
fn wait_pending(broker: &WebQuestionBroker, want: usize) {
    for _ in 0..100 {
        if broker.pending().len() >= want {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("pending did not reach {} in time", want);
}

async fn recv_scoped<T>(rx: &mut tokio::sync::mpsc::Receiver<T>) -> T {
    rx.recv().await.unwrap()
}

#[test]
fn answer_flow_resolves_selected_and_clears_pending() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));
    let result_rx = ask_async(broker.clone(), "q-a", 10);
    wait_pending(&broker, 1);
    assert!(broker.respond("q-a", vec!["pnpm".into()]).unwrap());
    match result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap()
    {
        QuestionOutcome::Answered(sel) => assert_eq!(sel, vec!["pnpm".to_string()]),
        QuestionOutcome::Timeout => panic!("answered flow must not yield Timeout"),
    }
    assert!(broker.pending().is_empty());
}

#[test]
fn multi_select_flow_delivers_all_selections() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));
    let (tx, rx) = mpsc::channel();
    let b = broker.clone();
    std::thread::spawn(move || {
        let _ = tx.send(b.ask(make_req("q-multi", true)));
    });
    wait_pending(&broker, 1);
    broker
        .respond("q-multi", vec!["pnpm".into(), "npm".into()])
        .unwrap();
    match rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap() {
        QuestionOutcome::Answered(sel) => {
            assert_eq!(sel.len(), 2, "multi select must carry both items");
        }
        QuestionOutcome::Timeout => panic!("answered flow must not yield Timeout"),
    }
}

#[test]
fn timeout_returns_timeout_outcome_and_removes_pending() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));
    let result_rx = ask_async(broker.clone(), "q-t", 1);
    let outcome = result_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(outcome, QuestionOutcome::Timeout, "timeout must not be Err");
    assert!(broker.pending().is_empty(), "timeout must clean pending");
    // 超时后迟到的 respond 诚实报 unknown。
    assert!(broker.respond("q-t", vec!["pnpm".into()]).is_err());
}

#[test]
fn double_respond_is_first_wins() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));
    let result_rx = ask_async(broker.clone(), "q-r", 10);
    wait_pending(&broker, 1);
    assert!(broker.respond("q-r", vec!["pnpm".into()]).is_ok());
    assert!(
        broker.respond("q-r", vec!["npm".into()]).is_err(),
        "second respond must see unknown question"
    );
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[test]
fn respond_validation_rejects_and_keeps_pending_retryable() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));
    let result_rx = ask_async(broker.clone(), "q-v", 10);
    wait_pending(&broker, 1);

    // 空选择 → 拒绝，请求留在 pending。
    let err = broker.respond("q-v", vec![]).unwrap_err();
    assert!(err.contains("empty"), "err: {}", err);
    assert_eq!(broker.pending().len(), 1);

    // 非候选选项 → 拒绝，请求仍在。
    let err = broker.respond("q-v", vec!["yarn".into()]).unwrap_err();
    assert!(
        err.contains("not one of the offered options"),
        "err: {}",
        err
    );
    assert_eq!(broker.pending().len(), 1);

    // 单选多项 → 拒绝，请求仍在。
    let err = broker
        .respond("q-v", vec!["pnpm".into(), "npm".into()])
        .unwrap_err();
    assert!(err.contains("single choice"), "err: {}", err);
    assert_eq!(broker.pending().len(), 1);

    // 修正后重试成功。
    assert!(broker.respond("q-v", vec!["npm".into()]).unwrap());
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[test]
fn pending_lists_question_metadata() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));
    let result_rx = ask_async(broker.clone(), "q-p", 10);
    wait_pending(&broker, 1);
    let list = broker.pending();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["question_id"], "q-p");
    assert_eq!(list[0]["question"], "用哪个包管理器?");
    assert_eq!(list[0]["options"][0], "pnpm");
    assert_eq!(list[0]["multi"], false);
    assert_eq!(list[0]["timeout_secs"], 10);
    assert_eq!(list[0]["chat_id"], "chat-1");
    assert_eq!(list[0]["session_key"], "web:chat-1");
    assert!(broker.respond("q-p", vec!["pnpm".into()]).unwrap());
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[test]
fn respond_without_request_is_unknown_error() {
    let broker = WebQuestionBroker::new(None);
    let err = broker.respond("nope", vec!["pnpm".into()]).unwrap_err();
    assert!(err.contains("unknown question"), "err: {}", err);
}

#[tokio::test]
async fn broadcast_carries_ask_payload_and_resolved_on_answer_and_timeout() {
    let (broker, mut events) = broker_with_events();
    let result_rx = ask_async(broker.clone(), "q-b1", 10);
    let asked = recv_scoped(&mut events).await;
    assert_eq!(asked["kind"], "QuestionAsked");
    assert_eq!(asked["data"]["question_id"], "q-b1");
    assert_eq!(asked["data"]["question"], "用哪个包管理器?");
    assert_eq!(asked["data"]["options"][1], "npm");
    assert_eq!(asked["data"]["multi"], false);
    assert_eq!(asked["data"]["timeout_secs"], 10);
    assert_eq!(asked["data"]["chat_id"], "chat-1");
    assert!(broker.respond("q-b1", vec!["npm".into()]).unwrap());
    let resolved = recv_scoped(&mut events).await;
    assert_eq!(resolved["kind"], "QuestionResolved");
    assert_eq!(resolved["data"]["question_id"], "q-b1");
    assert_eq!(resolved["data"]["decision"], "answered");
    let _ = result_rx.recv_timeout(Duration::from_secs(2));

    // 超时路径：1s 无作答 → Timeout 结局 + 广播 timeout。
    let result_rx = ask_async(broker.clone(), "q-b2", 1);
    let asked = recv_scoped(&mut events).await;
    assert_eq!(asked["kind"], "QuestionAsked");
    let resolved = recv_scoped(&mut events).await;
    assert_eq!(resolved["kind"], "QuestionResolved");
    assert_eq!(resolved["data"]["question_id"], "q-b2");
    assert_eq!(resolved["data"]["decision"], "timeout");
    let outcome = result_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(outcome, QuestionOutcome::Timeout);
}

/// `block_in_place` 等待分支：提问期间 worker 被让出，同 runtime 上的其他
/// 任务（这里是 respond 调用本身）仍可推进——不死锁。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ask_in_tokio_context_uses_block_in_place_and_still_resolves() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));
    let b2 = broker.clone();
    let waiter = tokio::spawn(async move { b2.ask(make_req("q-bip", false)) });
    for _ in 0..100 {
        if !broker.pending().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(broker.respond("q-bip", vec!["pnpm".into()]).unwrap());
    match waiter.await.unwrap().unwrap() {
        QuestionOutcome::Answered(sel) => assert_eq!(sel, vec!["pnpm".to_string()]),
        QuestionOutcome::Timeout => panic!("answered flow must not yield Timeout"),
    }
}
