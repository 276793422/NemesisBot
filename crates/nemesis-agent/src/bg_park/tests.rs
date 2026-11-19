//! bg_park 单测（park 注册表状态机：注册→等待→投递/取消/等待者死亡）。

use super::*;

#[tokio::test]
async fn deliver_to_waiting_waiter_succeeds() {
    let park = ParkRegistry::default();
    park.register("t1", "sess-a", "chat-1");
    let rx = park.wait("t1").expect("rx expected");
    assert!(park.contains("t1"));

    let outcome = park.deliver(
        "t1",
        ParkParcel {
            content: "done".to_string(),
            failed: false,
            error: None,
        },
    );
    assert!(matches!(
        outcome,
        DeliverOutcome::Delivered { ref session_key, ref chat_id }
            if session_key == "sess-a" && chat_id == "chat-1"
    ));
    // 条目摘除；parcel 送达。
    assert!(!park.contains("t1"));
    let parcel = rx.await.expect("parcel expected");
    assert_eq!(parcel.content, "done");
    assert!(!parcel.failed);
}

#[tokio::test]
async fn deliver_without_waiter_is_waiter_gone() {
    let park = ParkRegistry::default();
    park.register("t2", "sess-b", "chat-2");
    // 从不 wait → tx None → WaiterGone。
    let outcome = park.deliver(
        "t2",
        ParkParcel {
            content: "late".to_string(),
            failed: false,
            error: None,
        },
    );
    assert!(matches!(outcome, DeliverOutcome::WaiterGone));
    assert!(!park.contains("t2"));
}

#[tokio::test]
async fn deliver_after_cancelled_never_resumes() {
    let park = ParkRegistry::default();
    park.register("t3", "sess-c", "chat-3");
    let rx = park.wait("t3").expect("rx expected");
    park.mark_cancelled("t3");
    // mark_cancelled 弃 tx → rx 端收 Err。
    assert!(rx.await.is_err());
    let outcome = park.deliver(
        "t3",
        ParkParcel {
            content: "late".to_string(),
            failed: false,
            error: None,
        },
    );
    assert!(matches!(outcome, DeliverOutcome::CancelledNoResume));
    assert!(!park.contains("t3"));
}

#[tokio::test]
async fn deliver_unknown_task_is_not_parked() {
    let park = ParkRegistry::default();
    let outcome = park.deliver(
        "nope",
        ParkParcel {
            content: "x".to_string(),
            failed: false,
            error: None,
        },
    );
    assert!(matches!(outcome, DeliverOutcome::NotParked));
}

#[tokio::test]
async fn wait_after_cancelled_returns_none() {
    let park = ParkRegistry::default();
    park.register("t4", "s", "c");
    park.mark_cancelled("t4");
    assert!(park.wait("t4").is_none());
}

#[tokio::test]
async fn dropped_rx_then_deliver_is_waiter_gone() {
    let park = ParkRegistry::default();
    park.register("t5", "s", "c");
    {
        let _rx = park.wait("t5").expect("rx expected");
        // rx 在作用域尾 drop（模拟 estop 打断 park）。
    }
    let outcome = park.deliver(
        "t5",
        ParkParcel {
            content: "late".to_string(),
            failed: true,
            error: Some("boom".to_string()),
        },
    );
    assert!(matches!(outcome, DeliverOutcome::WaiterGone));
    assert!(!park.contains("t5"));
}

#[test]
fn arrival_line_success_and_failure_forms() {
    let ok = arrival_line(false, "首行结果\n第二行", None);
    assert!(ok.starts_with("✅ 后台子代理返回："));
    assert!(ok.contains("首行结果"));
    assert!(!ok.contains("第二行"));

    let bad = arrival_line(true, "Error: x", Some("boom detail"));
    assert!(bad.starts_with("❌ 后台子代理失败："));
    assert!(bad.contains("boom detail"));

    // 空回退：failed 但 error 空字符串 → 取 content。
    let fallback = arrival_line(true, "Error: y", Some("  "));
    assert!(fallback.contains("Error: y"));
}

#[tokio::test]
async fn register_overwrites_same_id() {
    let park = ParkRegistry::default();
    park.register("t6", "old-sess", "old-chat");
    park.register("t6", "new-sess", "new-chat");
    let rx = park.wait("t6").expect("rx expected");
    let outcome = park.deliver(
        "t6",
        ParkParcel {
            content: "v".to_string(),
            failed: false,
            error: None,
        },
    );
    assert!(matches!(
        outcome,
        DeliverOutcome::Delivered { ref session_key, .. } if session_key == "new-sess"
    ));
    assert!(rx.await.is_ok());
}
