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
///
/// timeout 一律传 300：这是「不被超时打断」的安全边际而非被测值——
/// 真实墙钟超时在慢机/高负载 CI 上会被调度延迟击穿（实录 ET Linux
/// 2727 测试并行时 10s 竞态红，timeout 先于 respond 触发 → pending
/// 被清 → respond 诚实报 unknown → unwrap 假红）。只有显式测超时
/// 行为本身的用例才传小值（1s）。
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
    let result_rx = request_async(mgr.clone(), "req-a", 300);
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
    let result_rx = request_async(mgr.clone(), "req-d", 300);
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
    let result_rx = request_async(mgr.clone(), "req-r", 300);
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
    let result_rx = request_async(mgr.clone(), "req-p", 300);
    wait_pending(&mgr, 1);
    let list = mgr.pending();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["request_id"], "req-p");
    assert_eq!(list[0]["operation"], "process_exec");
    assert_eq!(list[0]["target"], "cargo publish");
    assert_eq!(list[0]["risk_level"], "HIGH");
    assert_eq!(list[0]["timeout_secs"], 300);
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
            let r = m.request_approval_sync(
                "req-b",
                "file_write",
                "/tmp/x",
                "CRITICAL",
                "rule: w",
                300,
            );
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
    assert_eq!(payload["data"]["timeout_secs"], 300);
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
        m2.request_approval_sync("req-bip", "process_exec", "x", "HIGH", "r", 300)
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
    let result_rx = request_async(mgr.clone(), "req-always", 300);
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
    let result_rx = request_async(mgr.clone(), "req-once", 300);
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
    let result_rx = request_async(mgr.clone(), "req-deny", 300);
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
            300,
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
    let result_rx = request_async(mgr.clone(), "req-nopath", 300);
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
        let result_rx = request_async(mgr.clone(), rid, 300);
        wait_pending(&mgr, 1);
        mgr.respond(rid, true, true, None).unwrap();
        let _ = result_rx.recv_timeout(Duration::from_secs(2));
    }

    let raw = std::fs::read_to_string(&path).unwrap();
    let rules: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    assert_eq!(rules.len(), 1, "same command must upsert not duplicate");
}

/// skills 安装门：安装类操作**永不自动放行**（2026-09-30 复查 #12——原
/// M3 语义「规则命中不弹卡直接 Approve」已废除：source 目录/URL 记忆 ≠
/// 载荷记忆，同一路径明天可以装不同内容的技能包）。手写规则（历史遗留/
/// 绕开写入面）也是死信；无人裁决 = 弹卡超时 Deny（fail-closed 不变）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_skills_gate_ignores_install_rules_always_card() {
    use super::LateWebSkillsGate;
    use nemesis_skills::install_gate::{InstallDecision, InstallGate, InstallPlan};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, Some(path.clone())));

    // 手写一条 skills.install 规则（写入侧 rule_permitted_for 现已拒绝
    // 新增——这里直测消费侧纵深：规则在场也必须弹卡）。
    let rules = vec![nemesis_security::approval_rules::ApprovalRule {
        op: "skills.install".to_string(),
        pattern: "github:acme/demo".to_string(),
        action: "allow".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
    }];
    nemesis_security::approval_rules::save_rules(&path, &rules).unwrap();

    let gate = LateWebSkillsGate::new(1);
    gate.bind(mgr);

    let plan = InstallPlan {
        slug: "demo".to_string(),
        source: "github:acme/demo".to_string(),
        requested_ref: String::new(),
        commit: "c0ffee".to_string(),
        published_at: None,
        age_days: None,
        age_decision: String::new(),
        files: Vec::new(),
        total_bytes: 0,
        trust: nemesis_skills::trust::TrustState::ReviewRequired,
        trust_detail: String::new(),
        security: nemesis_skills::security_check::check_skill_security("", "demo", ""),
        warnings: Vec::new(),
    };

    match gate.decide(&plan).await {
        InstallDecision::Approve => {
            panic!("skills.install 规则在也必须弹卡（安装类永不自动放行）")
        }
        InstallDecision::Deny { .. } => {}
    }
}

// ---------------------------------------------------------------------------
// F6（devtool-upgrade 阶段 5）：拒绝备注 + 裁决广播
// ---------------------------------------------------------------------------

#[test]
fn deny_note_flows_to_waiter_verdict() {
    // 备注只在 denied 时随 verdict 送达；auditor 侧拼进拒绝消息回灌模型。
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    let result_rx = request_async(mgr.clone(), "req-note", 300);
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
    let result_rx = request_async(mgr.clone(), "req-blank", 300);
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
    let result_rx = request_async(mgr.clone(), "req-appr", 300);
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
    let result_rx = request_async(mgr.clone(), "req-res1", 300);
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

// ---------------------------------------------------------------------------
// approval_rules.json 写串行锁并发回归钉（2026-10-13 横扫四期）：该文件的
// 两个写者——本管理器的 save_always_rule（rules_write 叠加 config_write_lock）
// 与 security.approvals.clear（config_write_lock）——必须互斥。本钉用「外部
// 持 config_write_lock 手写规则文件」模拟 clear 臂，验证 save_always_rule
// 的 RMW 与 config 家族锁互斥（修复前 save_always_rule 只认自身
// rules_write，外部写者落在其读写窗口内会被冲掉/把它冲掉）。
// ---------------------------------------------------------------------------

#[test]
fn save_always_rule_is_exclusive_with_config_family_lock() {
    let dir = tempfile::tempdir().unwrap();
    let rules_path = dir.path().join("config/approval_rules.json");
    std::fs::create_dir_all(rules_path.parent().unwrap()).unwrap();

    let mgr = WebApprovalManager::new(None, Some(rules_path.clone()));

    // 外部写者持 config 家族锁整写文件（模拟 approvals.clear 的读→写）
    // 与 save_always_rule 并发跑。save 5 次同一规则（upsert 幂等收敛 1 条）。
    let writer_path = rules_path.clone();
    std::thread::scope(|s| {
        let sa = s.spawn(|| {
            for _ in 0..5 {
                mgr.save_always_rule("process_exec", "cmd-0", "medium")
                    .expect("save rule ok");
            }
        });
        let sb = s.spawn(move || {
            for _ in 0..5 {
                let _g = nemesis_config::config_write_lock();
                // clear 臂形态：持锁读→整写（这里写保留性内容，语义为
                // 「外部写者与 RMW 串行」，不销毁断言前置）。
                let mut v: serde_json::Value = std::fs::read_to_string(&writer_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or(serde_json::json!([]));
                if let Some(a) = v.as_array_mut() {
                    // 必须是合法完整 ApprovalRule（四字段齐）：load_rules 对
                    // 解析失败的文件整表按空处理，缺字段条目会被 save 的
                    // typed 全写抹掉——那是内容形态问题不是锁问题。
                    a.push(serde_json::json!({
                        "op": "marker",
                        "pattern": "clear-arm *",
                        "action": "allow",
                        "created_at": "2026-10-13T00:00:00+08:00"
                    }));
                }
                std::fs::write(&writer_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
            }
        });
        sa.join().unwrap();
        sb.join().unwrap();
    });

    // 串行后两条线程的效果都必须存活：1 条 always 规则 + 5 个 marker 条目。
    let stored: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(&rules_path).unwrap()).unwrap();
    let rule_hits = stored
        .iter()
        .filter(|r| {
            r["op"] == "process_exec"
                && r["pattern"]
                    .as_str()
                    .map(|p| p.starts_with("cmd-0"))
                    .unwrap_or(false)
        })
        .count();
    let markers = stored.iter().filter(|r| r["op"] == "marker").count();
    assert_eq!(
        rule_hits, 1,
        "upsert 幂等：5 次保存收敛为 1 条（实际 {rule_hits}）"
    );
    assert_eq!(
        markers, 5,
        "外部写者的 5 条 marker 不得被 RMW 写回冲掉（实际 {markers}）"
    );
}

// ---------------------------------------------------------------------------
// P1 统一任务层：审批台账（tasks.db approvals 表）
// ---------------------------------------------------------------------------

fn journal_at(
    path: &std::path::Path,
) -> std::sync::Arc<dyn nemesis_types::approval_journal::ApprovalJournal> {
    let db = nemesis_taskstore::TaskDb::open(path).expect("open tasks.db");
    std::sync::Arc::new(nemesis_taskstore::SqliteApprovalJournal::from_db(db))
}

#[test]
fn journal_records_pending_and_live_decision() {
    let dir = tempfile::tempdir().unwrap();
    let journal = journal_at(&dir.path().join("tasks.db"));
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    mgr.set_journal(journal.clone());

    let result_rx = request_async(mgr.clone(), "req-jl", 300);
    wait_pending(&mgr, 1);
    // pending 已落库（broker=web、status=pending）。
    let e = journal.entry("req-jl").unwrap();
    assert_eq!(e.broker, "web");
    assert_eq!(e.status, nemesis_types::approval_journal::status::PENDING);

    assert!(mgr.respond("req-jl", true, false, None).unwrap());
    let e = journal.entry("req-jl").unwrap();
    assert_eq!(e.status, nemesis_types::approval_journal::status::APPROVED);
    assert_eq!(e.decided_by.as_deref(), Some("web-user"));
    let _ = result_rx.recv_timeout(Duration::from_secs(2));

    // 晚到的二次 respond：map-miss → 重启补裁路径撞「已裁决」= 诚实报错
    // （幂等条件更新先到先得，裁决只生效一次）。
    assert!(mgr.respond("req-jl", true, false, None).is_err());
}

#[test]
fn journal_timeout_records_expired() {
    let dir = tempfile::tempdir().unwrap();
    let journal = journal_at(&dir.path().join("tasks.db"));
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    mgr.set_journal(journal.clone());

    let result_rx = request_async(mgr.clone(), "req-jt", 1);
    let v = result_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(!v.approved, "timeout must deny");
    let e = journal.entry("req-jt").unwrap();
    assert_eq!(e.status, nemesis_types::approval_journal::status::EXPIRED);
    assert_eq!(e.decided_by.as_deref(), Some("timeout"));
    // 超时后晚到 respond 走补裁 → 撞 expired → 诚实报错（不二次生效）。
    assert!(mgr.respond("req-jt", true, false, None).is_err());
}

#[test]
fn journal_restart_resume_stale_card_decidable_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("tasks.db");

    // 「重启前」：pending 记账后进程死亡（无活 ask 线程——记账与等待解耦，
    // 直接走记账入口；内存 pending 随 mgr drop 消失）。
    {
        let mgr = WebApprovalManager::new(None, None);
        mgr.set_journal(journal_at(&db_path));
        let entry = WebApprovalManager::journal_entry(
            "req-stale-1",
            "process_exec",
            "cargo publish",
            "HIGH",
            "rule: exec-publish",
            "cargo publish *",
            "",
            "",
        );
        mgr.journal_record_pending(&entry);
        // drop = 进程死亡。
    }

    // 「重启后」：新进程从盘上重开 tasks.db（真重开验证落盘耐久，非同
    // 连接句柄自证）。
    let mgr = WebApprovalManager::new(None, None);
    mgr.set_journal(journal_at(&db_path));

    // stale 卡片可见（stale: true 供前端注记「原操作已失效」）。
    let pending_cards = mgr.pending();
    let stale: Vec<&serde_json::Value> = pending_cards
        .iter()
        .filter(|c| c["request_id"] == "req-stale-1")
        .collect();
    assert_eq!(stale.len(), 1, "restarted gateway must surface stale card");
    assert_eq!(stale[0]["stale"], true);
    assert_eq!(stale[0]["operation"], "process_exec");

    // 补裁恰好生效一次（记账性质；二次 = already decided 诚实报错）。
    assert!(mgr.respond("req-stale-1", true, false, None).unwrap());
    assert!(
        mgr.respond("req-stale-1", true, false, None).is_err(),
        "second stale respond must see already-decided"
    );
    // 终态落账；二次 pending() 不再出卡。
    assert_eq!(mgr.pending().len(), 0);
}

// ---------------------------------------------------------------------------
// R3（2026-10-18 三方复评）：落库时序与裁决竞争
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

/// 可闸门的 journal 包装：按键阻塞 `record_pending`（键 `__pending`）与
/// `record_decision`（键 = decided_by），把「先落库后可见」「respond 先记账
/// vs 超时先记账」的竞争摆成确定性时序。未注册的键直通。底层真 SQLite——
/// 条件更新语义（Ok(true)/Ok(false)）不被 mock 掉。
struct GateJournal {
    inner: std::sync::Arc<dyn nemesis_types::approval_journal::ApprovalJournal>,
    gates: Mutex<HashMap<String, std::sync::Arc<(Mutex<bool>, Condvar)>>>,
    in_gate: AtomicUsize,
}

impl GateJournal {
    fn new(
        inner: std::sync::Arc<dyn nemesis_types::approval_journal::ApprovalJournal>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            inner,
            gates: Mutex::new(HashMap::new()),
            in_gate: AtomicUsize::new(0),
        })
    }

    /// 注册闸门（默认关闭）。
    fn add_gate(&self, key: &str) {
        self.gates.lock().unwrap().insert(
            key.to_string(),
            std::sync::Arc::new((Mutex::new(false), Condvar::new())),
        );
    }

    fn open_gate(&self, key: &str) {
        let g = self.gates.lock().unwrap().get(key).cloned();
        if let Some(g) = g {
            let (m, cv) = &*g;
            let mut open = m.lock().unwrap();
            *open = true;
            cv.notify_all();
        }
    }

    fn in_gate_count(&self) -> usize {
        self.in_gate.load(Ordering::SeqCst)
    }

    fn wait_in_gate(&self, want: usize) {
        for _ in 0..500 {
            if self.in_gate_count() >= want {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("in-gate count did not reach {want}");
    }

    fn park(&self, key: &str) {
        let g = {
            let map = self.gates.lock().unwrap();
            map.get(key).cloned()
        };
        if let Some(g) = g {
            let (m, cv) = &*g;
            let mut open = m.lock().unwrap();
            self.in_gate.fetch_add(1, Ordering::SeqCst);
            while !*open {
                open = cv.wait(open).unwrap();
            }
            self.in_gate.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl nemesis_types::approval_journal::ApprovalJournal for GateJournal {
    fn record_pending(
        &self,
        entry: &nemesis_types::approval_journal::ApprovalJournalEntry,
    ) -> Result<(), String> {
        self.park("__pending");
        self.inner.record_pending(entry)
    }
    fn record_decision(
        &self,
        id: &str,
        decision: nemesis_types::approval_journal::ApprovalDecision,
        note: Option<&str>,
        decided_by: &str,
    ) -> Result<bool, String> {
        self.park(decided_by);
        self.inner.record_decision(id, decision, note, decided_by)
    }
    fn entry(
        &self,
        id: &str,
    ) -> Result<nemesis_types::approval_journal::ApprovalJournalEntry, String> {
        self.inner.entry(id)
    }
    fn pending_entries(&self) -> Vec<nemesis_types::approval_journal::ApprovalJournalEntry> {
        self.inner.pending_entries()
    }
    fn find_pending(
        &self,
        id_or_short: &str,
    ) -> Option<nemesis_types::approval_journal::ApprovalJournalEntry> {
        self.inner.find_pending(id_or_short)
    }
}

/// R3 不变量：**内存可见（pending 表/广播）⇒ DB pending 已在**。闸住
/// record_pending 时，pending 表不得出现、ApprovalRequested 不得广播
/// （旧序 = 先入册+广播后落库，respond 可在 INSERT 前完成裁决 → 幽灵
/// pending）；放行后链路照常走通。
#[test]
fn journal_pending_persists_before_map_visible_and_broadcast() {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = journal_at(&dir.path().join("tasks.db"));
    let gated = GateJournal::new(sqlite.clone());
    gated.add_gate("__pending");

    let (btx, mut brx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let mgr = std::sync::Arc::new(WebApprovalManager::new(Some(btx), None));
    mgr.set_journal(gated.clone());

    let result_rx = request_async(mgr.clone(), "req-ord", 300);
    gated.wait_in_gate(1);
    assert!(
        mgr.pending().is_empty(),
        "record_pending 在闸内时内存 pending 表不得可见"
    );
    assert!(
        brx.try_recv().is_err(),
        "record_pending 在闸内时不得广播 ApprovalRequested（先 durable 后可见）"
    );

    gated.open_gate("__pending");
    wait_pending(&mgr, 1);
    let e = sqlite.entry("req-ord").unwrap();
    assert_eq!(e.status, nemesis_types::approval_journal::status::PENDING);

    assert!(mgr.respond("req-ord", true, false, None).unwrap());
    let e = sqlite.entry("req-ord").unwrap();
    assert_eq!(e.status, nemesis_types::approval_journal::status::APPROVED);
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

/// R3：respond 撞上已记账的 Expired（超时先到）→ `Ok(false)` 显式处理：
/// 不送裁决、诚实报 already decided、台账终态保持 expired。旧代码
/// send-then-record 会把裁决送进已超时的通道并静默忽略 Ok(false)。
#[test]
fn respond_after_expired_records_no_verdict_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = journal_at(&dir.path().join("tasks.db"));
    let gated = GateJournal::new(sqlite.clone());
    gated.add_gate("web-user");

    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    mgr.set_journal(gated.clone());

    let result_rx = request_async(mgr.clone(), "req-race1", 1);
    wait_pending(&mgr, 1);

    let respond_rx = {
        let (tx, rx) = mpsc::channel();
        let m = mgr.clone();
        std::thread::spawn(move || {
            let r = m.respond("req-race1", true, false, None);
            let _ = tx.send(r);
        });
        rx
    };
    // respond 卡进记账闸（map 已摘除、verdict 已构建）。
    gated.wait_in_gate(1);
    // 放过请求方的 1s 超时 → Expired 记账（"timeout" 不在闸内）→ 正常
    // 超时路径收场。
    std::thread::sleep(Duration::from_millis(1300));
    let v = result_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(!v.approved, "timeout must deny");
    let e = sqlite.entry("req-race1").unwrap();
    assert_eq!(e.status, nemesis_types::approval_journal::status::EXPIRED);

    // 放行 respond 的记账 → 条件更新败方（Ok(false)）→ 诚实报错。
    gated.open_gate("web-user");
    let r = respond_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let err = r.expect_err("败方 respond 必须报 already decided");
    assert!(err.contains("already decided"), "err: {err}");
    // 台账终态不被败方覆盖。
    let e = sqlite.entry("req-race1").unwrap();
    assert_eq!(e.status, nemesis_types::approval_journal::status::EXPIRED);
}

/// R3：超时分支撞上抢先的 respond（respond 的记账先到）→ `Ok(false)` 被尊重：
/// **不广播假 timeout**、宽限窗收回在途裁决返回真实结果。旧代码无条件广播
/// 「timeout」+ 强制 deny——用户批了的操作被谎报成超时拒绝。
#[test]
fn timeout_honors_raced_respond_and_recovers_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = journal_at(&dir.path().join("tasks.db"));
    let gated = GateJournal::new(sqlite.clone());
    gated.add_gate("web-user");
    gated.add_gate("timeout");

    let (btx, mut brx) = tokio::sync::broadcast::channel::<AgentEvent>(32);
    let mgr = std::sync::Arc::new(WebApprovalManager::new(Some(btx), None));
    mgr.set_journal(gated.clone());

    let result_rx = request_async(mgr.clone(), "req-race2", 1);
    wait_pending(&mgr, 1);

    let respond_rx = {
        let (tx, rx) = mpsc::channel();
        let m = mgr.clone();
        std::thread::spawn(move || {
            let r = m.respond("req-race2", true, false, None);
            let _ = tx.send(r);
        });
        rx
    };
    // respond 卡进 web-user 记账闸。
    gated.wait_in_gate(1);
    // 请求方 1s 超时到期 → 卡进 timeout 记账闸（两笔记账都在闸内）。
    std::thread::sleep(Duration::from_millis(1300));
    gated.wait_in_gate(2);

    // 先放行 respond 的记账 → Ok(true) → 送裁决 + 广播 approved。
    gated.open_gate("web-user");
    assert!(
        respond_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .is_ok(),
        "先到的 respond 必须成功"
    );
    // 再放行超时分支的记账 → Ok(false) → 宽限收回在途裁决。
    gated.open_gate("timeout");
    let v = result_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(
        v.approved,
        "超时分支必须尊重 respond 的抢先裁决并收回真实结果，得到: {v:?}"
    );

    // 广播面：requested + resolved(approved)，**没有** resolved(timeout)。
    let mut kinds = Vec::new();
    while let Ok(ev) = brx.try_recv() {
        let k = serde_json::to_value(&ev).unwrap();
        kinds.push((
            k["kind"].as_str().unwrap_or("").to_string(),
            k["data"]["decision"].as_str().unwrap_or("").to_string(),
        ));
    }
    assert!(
        !kinds
            .iter()
            .any(|(k, d)| k == "ApprovalResolved" && d == "timeout"),
        "败方超时分支不得广播假 timeout: {kinds:?}"
    );
    assert!(
        kinds
            .iter()
            .any(|(k, d)| k == "ApprovalResolved" && d == "approved"),
        "respond 的真实裁决广播必须在场: {kinds:?}"
    );
    let e = sqlite.entry("req-race2").unwrap();
    assert_eq!(e.status, nemesis_types::approval_journal::status::APPROVED);
    assert_eq!(e.decided_by.as_deref(), Some("web-user"));
}

/// R3 可用性立场：台账写失败（Err）时裁决照常送达（审批可用性不绑
/// tasks.db），审计缺失 loud 留痕——响应仍成功。
struct ErrJournal;

impl nemesis_types::approval_journal::ApprovalJournal for ErrJournal {
    fn record_pending(
        &self,
        _entry: &nemesis_types::approval_journal::ApprovalJournalEntry,
    ) -> Result<(), String> {
        Err("db down".to_string())
    }
    fn record_decision(
        &self,
        _id: &str,
        _decision: nemesis_types::approval_journal::ApprovalDecision,
        _note: Option<&str>,
        _decided_by: &str,
    ) -> Result<bool, String> {
        Err("db down".to_string())
    }
    fn entry(
        &self,
        _id: &str,
    ) -> Result<nemesis_types::approval_journal::ApprovalJournalEntry, String> {
        Err("db down".to_string())
    }
    fn pending_entries(&self) -> Vec<nemesis_types::approval_journal::ApprovalJournalEntry> {
        Vec::new()
    }
    fn find_pending(
        &self,
        _id_or_short: &str,
    ) -> Option<nemesis_types::approval_journal::ApprovalJournalEntry> {
        None
    }
}

#[test]
fn respond_still_delivers_when_journal_errors() {
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    mgr.set_journal(std::sync::Arc::new(ErrJournal));

    let result_rx = request_async(mgr.clone(), "req-dberr", 300);
    wait_pending(&mgr, 1);
    assert!(
        mgr.respond("req-dberr", true, false, None).unwrap(),
        "台账 Err 不得阻断裁决送达"
    );
    let v = result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(v.approved);
}

// ---------------------------------------------------------------------------
// 审批续执行（2026-10-22 拍板「批准+拒绝都回灌」）：stale 裁决回灌原会话
// ---------------------------------------------------------------------------

/// 捕获型注入器：记录 (session_key, content)，送达结果可编程（true=会话
/// 在位送达 / false=会话已不在，模拟 gateway 闭包的 chat_log 在位检查）。
struct CapturingInjector {
    calls: std::sync::Mutex<Vec<(String, String)>>,
    deliverable: bool,
}

impl CapturingInjector {
    fn new(deliverable: bool) -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
            deliverable,
        }
    }
    fn install(self: &std::sync::Arc<Self>, mgr: &WebApprovalManager) {
        let this = self.clone();
        mgr.set_continuation_injector(std::sync::Arc::new(move |sk, content| {
            this.calls
                .lock()
                .unwrap()
                .push((sk.to_string(), content.to_string()));
            this.deliverable
        }));
    }
    fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

/// 「重启前」夹具：pending 记账后进程死亡（内存 pending 随 drop 消失），
/// 重开后从盘上重开 tasks.db（真落盘耐久，非同句柄自证）。
fn restart_with_stale_card(
    db_path: &std::path::Path,
    request_id: &str,
    session_key: &str,
) -> WebApprovalManager {
    {
        let mgr = WebApprovalManager::new(None, None);
        mgr.set_journal(journal_at(db_path));
        let entry = WebApprovalManager::journal_entry(
            request_id,
            "process_exec",
            "cargo publish",
            "HIGH",
            "rule: exec-publish",
            "cargo publish *",
            session_key,
            session_key,
        );
        mgr.journal_record_pending(&entry);
    }
    let mgr = WebApprovalManager::new(None, None);
    mgr.set_journal(journal_at(db_path));
    mgr
}

#[test]
fn stale_approve_delivers_continuation_to_session() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = restart_with_stale_card(
        &dir.path().join("tasks.db"),
        "req-ba",
        "agent:main:session:s1",
    );
    let inj = std::sync::Arc::new(CapturingInjector::new(true));
    inj.install(&mgr);

    assert!(mgr.respond("req-ba", true, false, None).unwrap());
    let calls = inj.calls();
    assert_eq!(calls.len(), 1, "批准回灌恰好一条（实际 {calls:?}）");
    assert_eq!(calls[0].0, "agent:main:session:s1");
    assert!(calls[0].1.contains("已被批准"), "msg = {}", calls[0].1);
    assert!(
        calls[0].1.contains("cargo publish"),
        "回灌必须带操作摘要（msg = {}）",
        calls[0].1
    );
    assert!(
        calls[0].1.contains("重新发起"),
        "批准臂必须带可重新发起提示（拍板 1；msg = {}）",
        calls[0].1
    );
}

#[test]
fn stale_deny_delivers_denial_notice_to_session() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = restart_with_stale_card(
        &dir.path().join("tasks.db"),
        "req-bd",
        "agent:main:session:s2",
    );
    let inj = std::sync::Arc::new(CapturingInjector::new(true));
    inj.install(&mgr);

    assert!(!mgr.respond("req-bd", false, false, None).unwrap());
    let calls = inj.calls();
    assert_eq!(calls.len(), 1, "拒绝回灌恰好一条（实际 {calls:?}）");
    assert_eq!(calls[0].0, "agent:main:session:s2");
    assert!(calls[0].1.contains("已被拒绝"), "msg = {}", calls[0].1);
    assert!(
        !calls[0].1.contains("重新发起"),
        "拒绝臂不带重新发起提示（msg = {}）",
        calls[0].1
    );
}

#[test]
fn stale_legacy_entry_without_session_key_is_audit_only() {
    let dir = tempfile::tempdir().unwrap();
    // session_key 空 = 续执行功能落地前的历史条目形态。
    let mgr = restart_with_stale_card(&dir.path().join("tasks.db"), "req-leg", "");
    let inj = std::sync::Arc::new(CapturingInjector::new(true));
    inj.install(&mgr);

    assert!(mgr.respond("req-leg", true, false, None).unwrap());
    assert!(
        inj.calls().is_empty(),
        "无 session_key 的历史条目不得回灌（诚实留账，不造假投递）"
    );
}

#[test]
fn stale_undeliverable_session_degrades_and_stays_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = restart_with_stale_card(
        &dir.path().join("tasks.db"),
        "req-gone",
        "agent:main:session:deleted",
    );
    // 注入器在但返回 false = gateway 侧会话在位检查不过（chat_log 已删）。
    let inj = std::sync::Arc::new(CapturingInjector::new(false));
    inj.install(&mgr);

    assert!(mgr.respond("req-gone", true, false, None).unwrap());
    let calls = inj.calls();
    assert_eq!(calls.len(), 1, "投递尝试恰好一次（实际 {calls:?}）");
    // 裁决照常生效（记账不受投递失败影响）；二次 respond = already decided。
    assert!(mgr.respond("req-gone", true, false, None).is_err());
    assert_eq!(mgr.pending().len(), 0);
}

#[test]
fn ctx_request_routes_session_key_into_entry_and_journal() {
    let dir = tempfile::tempdir().unwrap();
    let journal = journal_at(&dir.path().join("tasks.db"));
    let mgr = std::sync::Arc::new(WebApprovalManager::new(None, None));
    mgr.set_journal(journal.clone());

    // 带 ctx 的请求（loop dispatch 从 RequestContext 穿线后的真实形态）。
    let (tx, rx) = mpsc::channel();
    {
        let mgr = mgr.clone();
        std::thread::spawn(move || {
            let ctx = nemesis_security::auditor::ApprovalContext {
                channel: "web".to_string(),
                chat_id: "sess-9".to_string(),
                sender_id: "user-1".to_string(),
                session_key: "agent:main:session:sess-9".to_string(),
            };
            let r = mgr.request_approval_sync_ctx(
                "req-ctx",
                "process_exec",
                "cargo publish",
                "HIGH",
                "rule: exec-publish",
                300,
                &ctx,
            );
            let _ = tx.send(r);
        });
    }
    wait_pending(&mgr, 1);

    // PendingEntry → 事件/卡片带真实 session_key。
    let card = mgr
        .pending()
        .into_iter()
        .find(|c| c["request_id"] == "req-ctx")
        .expect("card visible");
    assert_eq!(card["session_key"], "agent:main:session:sess-9");

    // 台账 payload 带真实 session_key（重启补裁回灌的路由依据）。
    let e = journal.find_pending("req-ctx").expect("journal pending");
    assert_eq!(
        e.payload["session_key"], "agent:main:session:sess-9",
        "payload = {}",
        e.payload
    );

    assert!(mgr.respond("req-ctx", true, false, None).unwrap());
    let v = rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    assert!(v.approved);
}
