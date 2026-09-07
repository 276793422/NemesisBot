//! M7：WebApprovalManager 单元测试。
//!
//! 覆盖六条路径：approve / deny / 超时 deny + pending 清理 / 双 respond 竞速
//! 先到先得 / pending 列表 / ApprovalRequested 广播内容。等待侧测试在纯同步
//! 线程跑（走 `recv` 直等分支）；`block_in_place` 分支由多线程 flavor 的
//! async 测试单独覆盖。
//!
//! F6（devtool-upgrade 阶段 5）新增：拒绝备注随 verdict 送达（approve 时被
//! 忽略 / 空白备注归一 None）+ `ApprovalResolved` 裁决广播（respond 成功 /
//! 超时自动 deny 两条路径）。

use super::WebApprovalManager;
use nemesis_security::auditor::{ApprovalManager, ApprovalVerdict};
use nemesis_types::agent::{AgentEvent, ApprovalResponder};
use std::sync::mpsc;
use std::time::Duration;

fn mgr_with_events() -> (
    WebApprovalManager,
    tokio::sync::mpsc::Receiver<serde_json::Value>,
) {
    // 用一个独享的窄通道把 broadcast 事件转成可断言的 JSON 快照。
    let (btx, mut brx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let (ctx, crx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        while let Ok(ev) = brx.recv().await {
            let _ = ctx.send(serde_json::to_value(&ev).unwrap()).await;
        }
    });
    (WebApprovalManager::new(Some(btx), None), crx)
}

/// 在独立线程发起审批请求（模拟 auditor 的同步调用上下文）。
fn request_async(
    mgr: std::sync::Arc<WebApprovalManager>,
    request_id: &str,
    timeout_secs: u64,
) -> mpsc::Receiver<Result<ApprovalVerdict, String>> {
    let (tx, rx) = mpsc::channel();
    let id = request_id.to_string();
    std::thread::spawn(move || {
        let r = mgr.request_approval_sync(
            &id,
            "process_exec",
            "cargo publish",
            "HIGH",
            "rule: exec-publish",
            timeout_secs,
        );
        let _ = tx.send(r);
    });
    rx
}

async fn recv_scoped<T>(rx: &mut tokio::sync::mpsc::Receiver<T>) -> T {
    rx.recv().await.unwrap()
}

/// 等待请求线程进入 pending（轮询代替裸 sleep，避免慢机 flake）。
fn wait_pending(mgr: &WebApprovalManager, want: usize) {
    for _ in 0..100 {
        if mgr.pending().len() >= want {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("pending did not reach {} in time", want);
}

#[test]
fn approve_flow_resolves_true_and_clears_pending() {
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let result_rx = request_async(mgr.clone(), "req-a", 10);
    // 请求线程需要一点时间进入 pending。
    wait_pending(&mgr, 1);
    assert!(mgr.respond("req-a", true, false, None).unwrap());
    let v = result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(v.approved);
    assert!(v.note.is_none());
    assert!(mgr.pending().is_empty());
}

#[test]
fn deny_flow_resolves_false() {
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let result_rx = request_async(mgr.clone(), "req-d", 10);
    wait_pending(&mgr, 1);
    assert!(!mgr.respond("req-d", false, false, None).unwrap());
    let v = result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(!v.approved);
}

#[test]
fn timeout_denies_and_removes_pending() {
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let result_rx = request_async(mgr.clone(), "req-t", 1);
    let v = result_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(!v.approved, "timeout must deny");
    assert!(mgr.pending().is_empty(), "timeout must clean pending");
    // 超时后迟到的 respond 诚实报 unknown。
    assert!(mgr.respond("req-t", true, false, None).is_err());
}

#[test]
fn double_respond_is_first_wins() {
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let result_rx = request_async(mgr.clone(), "req-r", 10);
    wait_pending(&mgr, 1);
    assert!(
        mgr.respond("req-r", true, false, None).is_ok(),
        "first respond wins"
    );
    assert!(
        mgr.respond("req-r", false, false, None).is_err(),
        "second respond must see unknown request"
    );
    let v = result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(v.approved);
}

#[test]
fn pending_lists_request_metadata() {
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let result_rx = request_async(mgr.clone(), "req-p", 10);
    wait_pending(&mgr, 1);
    let list = mgr.pending();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["request_id"], "req-p");
    assert_eq!(list[0]["operation"], "process_exec");
    assert_eq!(list[0]["target"], "cargo publish");
    assert_eq!(list[0]["risk_level"], "HIGH");
    assert_eq!(list[0]["timeout_secs"], 10);
    // F3:「总是允许」pattern 随 pending 下发（B5 归约：cargo publish *）。
    assert_eq!(list[0]["pattern"], "cargo publish *");
    assert!(mgr.respond("req-p", true, false, None).unwrap());
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[tokio::test]
async fn broadcast_carries_full_request_payload() {
    let (mgr, mut events) = mgr_with_events();
    let mgr = std::sync::Arc::new(mgr);
    let result_rx = {
        let (tx, rx) = mpsc::channel();
        let m = mgr.clone();
        std::thread::spawn(move || {
            let r =
                m.request_approval_sync("req-b", "file_write", "/tmp/x", "CRITICAL", "rule: w", 10);
            let _ = tx.send(r);
        });
        rx
    };
    let payload = recv_scoped(&mut events).await;
    assert_eq!(payload["kind"], "ApprovalRequested");
    assert_eq!(payload["data"]["request_id"], "req-b");
    assert_eq!(payload["data"]["operation"], "file_write");
    assert_eq!(payload["data"]["risk_level"], "CRITICAL");
    assert_eq!(payload["data"]["reason"], "rule: w");
    assert_eq!(payload["data"]["timeout_secs"], 10);
    // F3: pattern 随事件下发。
    assert_eq!(payload["data"]["pattern"], "/tmp/x");
    assert!(mgr.respond("req-b", true, false, None).unwrap());
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[test]
fn respond_without_request_is_unknown_error() {
    let mgr = WebApprovalManager::new(None, None);
    let err = mgr.respond("nope", true, false, None).unwrap_err();
    assert!(err.contains("unknown approval request"), "err: {}", err);
}

/// `block_in_place` 等待分支：审批期间 worker 被让出，同 runtime 上的其他
/// 任务（这里是 respond 调用本身）仍可推进——不死锁。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_in_tokio_context_uses_block_in_place_and_still_resolves() {
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let m2 = mgr.clone();
    let waiter = tokio::spawn(async move {
        m2.request_approval_sync("req-bip", "process_exec", "x", "HIGH", "r", 10)
    });
    // 等待者进入 pending 后再 respond。
    for _ in 0..100 {
        if !mgr.pending().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(mgr.respond("req-bip", true, false, None).unwrap());
    assert!(waiter.await.unwrap().unwrap().approved);
}

// ---------------------------------------------------------------------------
// F3（devtool-upgrade 阶段 5）：「总是允许」规则写入
// ---------------------------------------------------------------------------

#[test]
fn respond_always_approved_writes_exec_prefix_rule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, Some(path.clone())));
    let result_rx = request_async(mgr.clone(), "req-always", 10);
    wait_pending(&mgr, 1);

    mgr.respond("req-always", true, true, None).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    let rules: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["op"], "process_exec");
    // B5 归约：`cargo publish` → 前缀 pattern（而非原文全文）。
    assert_eq!(rules[0]["pattern"], "cargo publish *");
    assert_eq!(rules[0]["action"], "allow");
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[test]
fn respond_always_false_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, Some(path.clone())));
    let result_rx = request_async(mgr.clone(), "req-once", 10);
    wait_pending(&mgr, 1);
    mgr.respond("req-once", true, false, None).unwrap();
    assert!(!path.exists(), "plain approve must not create rules file");
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[test]
fn respond_always_denied_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, Some(path.clone())));
    let result_rx = request_async(mgr.clone(), "req-deny", 10);
    wait_pending(&mgr, 1);
    mgr.respond("req-deny", false, true, None).unwrap();
    assert!(!path.exists(), "deny must not create rules file");
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[test]
fn respond_always_on_critical_nonexec_op_is_ignored() {
    // 层级安全门：CRITICAL 且非 process_exec → always 被忽略（不写规则），
    // 本次批准照常送达。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, Some(path.clone())));
    let (tx, rx) = mpsc::channel();
    let m = mgr.clone();
    std::thread::spawn(move || {
        let r = m.request_approval_sync(
            "req-crit",
            "file_write",
            "/tmp/x",
            "CRITICAL",
            "rule: w",
            10,
        );
        let _ = tx.send(r);
    });
    wait_pending(&mgr, 1);

    // respond 返回 Ok（批准生效）。
    assert!(mgr.respond("req-crit", true, true, None).unwrap());
    let _ = rx.recv_timeout(Duration::from_secs(2));
    assert!(
        !path.exists(),
        "CRITICAL non-exec op must never produce a rule"
    );
}

#[test]
fn respond_always_without_rules_path_is_honest_error() {
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let result_rx = request_async(mgr.clone(), "req-nopath", 10);
    wait_pending(&mgr, 1);
    let err = mgr.respond("req-nopath", true, true, None).unwrap_err();
    assert!(
        err.contains("approval rules not configured"),
        "err: {}",
        err
    );
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[test]
fn always_rule_upsert_is_idempotent() {
    // 同 pattern 二次「总是允许」：规则去重不翻倍。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, Some(path.clone())));

    for rid in ["req-u1", "req-u2"] {
        let result_rx = request_async(mgr.clone(), rid, 10);
        wait_pending(&mgr, 1);
        mgr.respond(rid, true, true, None).unwrap();
        let _ = result_rx.recv_timeout(Duration::from_secs(2));
    }

    let raw = std::fs::read_to_string(&path).unwrap();
    let rules: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    assert_eq!(rules.len(), 1, "same command must upsert not duplicate");
}

// ---------------------------------------------------------------------------
// F6（devtool-upgrade 阶段 5）：拒绝备注 + 裁决广播
// ---------------------------------------------------------------------------

#[test]
fn deny_note_flows_to_waiter_verdict() {
    // 备注只在 denied 时随 verdict 送达；auditor 侧拼进拒绝消息回灌模型。
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let result_rx = request_async(mgr.clone(), "req-note", 10);
    wait_pending(&mgr, 1);
    assert!(
        !mgr.respond("req-note", false, false, Some("这是生产库，别动".into()))
            .unwrap()
    );
    let v = result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(!v.approved);
    assert_eq!(v.note.as_deref(), Some("这是生产库，别动"));
}

#[test]
fn deny_blank_note_is_normalized_to_none() {
    // 空串/纯空白备注视同无备注（auditor 侧消息不带冒号尾巴）。
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let result_rx = request_async(mgr.clone(), "req-blank", 10);
    wait_pending(&mgr, 1);
    mgr.respond("req-blank", false, false, Some("   ".into()))
        .unwrap();
    let v = result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(!v.approved);
    assert!(v.note.is_none(), "whitespace note must be dropped");
}

#[test]
fn approve_ignores_note() {
    // approved=true 时备注被忽略（trait 契约：note 仅拒绝语义）。
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let result_rx = request_async(mgr.clone(), "req-appr", 10);
    wait_pending(&mgr, 1);
    assert!(
        mgr.respond("req-appr", true, false, Some("随手批的".into()))
            .unwrap()
    );
    let v = result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(v.approved);
    assert!(
        v.note.is_none(),
        "approved verdict must carry no note, got {:?}",
        v.note
    );
}

#[tokio::test]
async fn resolved_broadcast_fires_on_respond_and_timeout() {
    // 裁决广播：respond 成功 → decision=approved；无人裁决超时 → timeout。
    // 竞速败方窗口据此摘卡。
    let (mgr, mut events) = mgr_with_events();
    let mgr = std::sync::Arc::new(mgr);
    let result_rx = request_async(mgr.clone(), "req-res1", 10);
    // 首个事件是 ApprovalRequested。
    let req_ev = recv_scoped(&mut events).await;
    assert_eq!(req_ev["kind"], "ApprovalRequested");
    assert!(mgr.respond("req-res1", true, false, None).unwrap());
    let resolved = recv_scoped(&mut events).await;
    assert_eq!(resolved["kind"], "ApprovalResolved");
    assert_eq!(resolved["data"]["request_id"], "req-res1");
    assert_eq!(resolved["data"]["decision"], "approved");
    let _ = result_rx.recv_timeout(Duration::from_secs(2));

    // 超时路径：1s 无裁决 → 自动 deny + 广播 timeout。
    let result_rx = request_async(mgr.clone(), "req-res2", 1);
    let req_ev = recv_scoped(&mut events).await;
    assert_eq!(req_ev["kind"], "ApprovalRequested");
    let resolved = recv_scoped(&mut events).await;
    assert_eq!(resolved["kind"], "ApprovalResolved");
    assert_eq!(resolved["data"]["request_id"], "req-res2");
    assert_eq!(resolved["data"]["decision"], "timeout");
    let v = result_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(!v.approved, "timeout must deny");
}
