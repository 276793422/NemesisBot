//! alert 模块测试：规则路由 / cooldown / budget+digest / jsonl 落盘 /
//! 出口派发。静态注册表 → 每用例开头 `reset_for_tests()` + `configure`
//! 最小配置（与其他静态注册表模块同一测试纪律）。

use super::*;
use nemesis_config::eval_rules::Condition;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 进程级 HUB 的测试串行锁（B4 background_registry 同款纪律，2026-11-20）：
/// HUB 是静态注册表，reset_for_tests/configure 在并行测试线程上互相冲掉
/// 对方装配（实录：6 用例并行互踩假红，计数 1 vs 3）。每个用例首行持锁。
static TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn cond(field: &str, op: &str, value: serde_json::Value) -> Condition {
    Condition {
        field: field.to_string(),
        op: op.to_string(),
        value,
    }
}

fn rule(id: &str, conds: Vec<Condition>) -> AlertRuleConfig {
    AlertRuleConfig {
        id: id.to_string(),
        when: conds,
        actions: vec!["alert".to_string()],
    }
}

fn minimal_cfg(rules: Vec<AlertRuleConfig>) -> SecurityAlertsConfig {
    SecurityAlertsConfig {
        enabled: true,
        webhooks: Vec::new(),
        cooldown_secs: 0,
        budget_per_min: 0,
        rules,
    }
}

fn capture_outlet(counter: Arc<AtomicUsize>) -> Arc<dyn Fn(&SecurityAlertEvent) + Send + Sync> {
    Arc::new(move |_e: &SecurityAlertEvent| {
        counter.fetch_add(1, Ordering::SeqCst);
    })
}

// ---------------------------------------------------------------------------
// 规则路由
// ---------------------------------------------------------------------------

#[test]
fn builtin_rule_only_critical_when_no_rules() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    let cfg = minimal_cfg(Vec::new());
    configure(&cfg, std::env::temp_dir().join("alert-test-builtin"));
    // critical 命中内置规则；high 不命中。
    emit(AlertKind::EstopTriggered, AlertLevel::Critical, "", "", "x");
    emit(AlertKind::ApprovalDenied, AlertLevel::High, "s1", "", "y");
    // 出口没注册 → 无可断言的派发计数；这里断言路由判定本身。
    let mut st = HUB.lock();
    let ev_critical = SecurityAlertEvent::now(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "",
        "",
        String::new(),
    );
    assert_eq!(
        first_matching_rule(&st, &ev_critical).as_deref(),
        Some(BUILTIN_RULE_ID)
    );
    let ev_high = SecurityAlertEvent::now(
        AlertKind::ApprovalDenied,
        AlertLevel::High,
        "s1",
        "",
        String::new(),
    );
    assert_eq!(first_matching_rule(&st, &ev_high), None);
    st.enabled = false;
}

#[test]
fn user_rule_matches_kind_and_level() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    let cfg = minimal_cfg(vec![rule(
        "estop-all",
        vec![cond("kind", "equals", serde_json::json!("estop_triggered"))],
    )]);
    configure(&cfg, std::env::temp_dir().join("alert-test-user-rule"));
    let counter = Arc::new(AtomicUsize::new(0));
    register_outlet("test", capture_outlet(counter.clone()));
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Low,
        "",
        "",
        "estop happened",
    );
    emit(
        AlertKind::ApprovalDenied,
        AlertLevel::Critical,
        "",
        "",
        "not matched",
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    reset_for_tests();
}

#[test]
fn non_empty_rules_replace_builtin() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    // 用户规则只匹配 approval_denied；critical estop 不再由内置规则接管。
    let cfg = minimal_cfg(vec![rule(
        "denials",
        vec![cond("kind", "equals", serde_json::json!("approval_denied"))],
    )]);
    configure(&cfg, std::env::temp_dir().join("alert-test-replace"));
    let counter = Arc::new(AtomicUsize::new(0));
    register_outlet("test", capture_outlet(counter.clone()));
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "",
        "",
        "critical but no rule",
    );
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    emit(
        AlertKind::ApprovalDenied,
        AlertLevel::High,
        "",
        "",
        "denied",
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    reset_for_tests();
}

// ---------------------------------------------------------------------------
// cooldown 闸
// ---------------------------------------------------------------------------

#[test]
fn cooldown_suppresses_same_rule_and_session() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    let mut cfg = minimal_cfg(Vec::new());
    cfg.cooldown_secs = 3600; // 测试窗口内不会滚出
    configure(&cfg, std::env::temp_dir().join("alert-test-cooldown"));
    let counter = Arc::new(AtomicUsize::new(0));
    register_outlet("test", capture_outlet(counter.clone()));
    // 内置规则只路由 critical（rules 空）——用 critical 级事件测 cooldown。
    // cooldown 键 = (rule_id, session_key)：kind 不同、rule 同、session 同照样压。
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "sess-a",
        "",
        "first",
    );
    emit(
        AlertKind::ChainVerifyFailed,
        AlertLevel::Critical,
        "sess-a",
        "",
        "suppressed",
    );
    emit(
        AlertKind::MemoryQuarantine,
        AlertLevel::Critical,
        "sess-b",
        "",
        "other session passes",
    );
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    reset_for_tests();
}

// ---------------------------------------------------------------------------
// budget 闸 + digest
// ---------------------------------------------------------------------------

#[test]
fn budget_overflow_aggregates_digest() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    let mut cfg = minimal_cfg(Vec::new());
    cfg.budget_per_min = 3;
    cfg.cooldown_secs = 0; // 关 cooldown——专注 budget 面（0 = 缺省 60s？不，0 会被归一成默认…）
    // 注意：cooldown_secs=0 视为缺省 60s，这里用不同 session 绕开 cooldown。
    configure(&cfg, std::env::temp_dir().join("alert-test-budget"));
    let counter = Arc::new(AtomicUsize::new(0));
    register_outlet("test", capture_outlet(counter.clone()));
    for i in 0..6 {
        emit(
            AlertKind::PipelineIntercept,
            AlertLevel::Critical,
            &format!("s{i}"),
            "",
            format!("event {i}"),
        );
    }
    assert_eq!(counter.load(Ordering::SeqCst), 3, "budget=3 → 只派发 3 条");
    let digests = take_pending_digests();
    assert!(digests.is_empty(), "窗口未滚动 digest 不该提前物化");
    reset_for_tests();
}

#[test]
fn digest_materializes_on_window_expiry_via_pump() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    let mut cfg = minimal_cfg(Vec::new());
    cfg.budget_per_min = 1;
    configure(&cfg, std::env::temp_dir().join("alert-test-digest"));
    let counter = Arc::new(AtomicUsize::new(0));
    register_outlet("test", capture_outlet(counter.clone()));
    // 内置规则只路由 critical（rules 空）——两条都用 critical 才能走到
    // budget 面；不同 session 绕开 cooldown（cooldown_secs=0 = 缺省 60s）。
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "a",
        "",
        "first",
    );
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "b",
        "",
        "overflowed",
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    // 手动把窗口推过期，再走水泵（惰性滚动路径）。
    {
        let mut st = HUB.lock();
        for (_, w) in st.budgets.iter_mut() {
            w.window_start = Instant::now() - Duration::from_secs(120);
        }
    }
    let digests = take_pending_digests();
    assert_eq!(digests.len(), 1, "超限 1 条 → 一个 digest");
    assert_eq!(digests[0].suppressed, 1);
    assert_eq!(digests[0].last_summary, "overflowed");
    reset_for_tests();
}

/// emit 驱动的窗口翻滚（生产风暴持续期的主形态：泵 tick 之前新告警先到）
/// 必须在 budget_allows 翻滚点物化 digest——BUG-20261121-072 红证：
/// 修复前 reset 在 snapshot 之前执行，suppressed_before 恒 0，digest 被静默吞掉。
#[test]
fn digest_materializes_on_emit_driven_rollover() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    let mut cfg = minimal_cfg(Vec::new());
    cfg.budget_per_min = 1;
    configure(&cfg, std::env::temp_dir().join("alert-test-emit-rollover"));
    let counter = Arc::new(AtomicUsize::new(0));
    register_outlet("test", capture_outlet(counter.clone()));
    // 窗口 1：第一条派发，第二/三条超限压制（不同 session 绕开 cooldown）。
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "a",
        "",
        "first",
    );
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "b",
        "",
        "overflowed-1",
    );
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "c",
        "",
        "overflowed-2",
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    // 把窗口推过期（模拟 60s 流逝），然后新 emit 走 budget_allows 翻滚路径。
    {
        let mut st = HUB.lock();
        for (_, w) in st.budgets.iter_mut() {
            w.window_start = Instant::now() - Duration::from_secs(120);
        }
    }
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "d",
        "",
        "post-rollover",
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "翻滚后预算重置，新事件派发"
    );
    // 翻滚时旧窗口的 2 条压制必须落 digest（take_pending_digests 消费）。
    let digests = take_pending_digests();
    assert_eq!(
        digests.len(),
        1,
        "emit 驱动翻滚必须物化旧窗口 digest（BUG-20261121-072）"
    );
    assert_eq!(digests[0].suppressed, 2);
    assert_eq!(digests[0].last_summary, "overflowed-2");
    reset_for_tests();
}

// ---------------------------------------------------------------------------
// jsonl 落盘
// ---------------------------------------------------------------------------

#[test]
fn jsonl_persists_emitted_events() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    let dir = std::env::temp_dir().join(format!("alert-test-jsonl-{}", std::process::id()));
    let cfg = minimal_cfg(Vec::new());
    configure(&cfg, dir.clone());
    emit(
        AlertKind::ChainVerifyFailed,
        AlertLevel::Critical,
        "s",
        "tool-x",
        "tamper suspect",
    );
    // 行存在且可反序列化回事件。
    let date = chrono::Utc::now().format("%Y-%m-%d");
    let path = dir.join(format!("{date}.jsonl"));
    let content = std::fs::read_to_string(&path).expect("jsonl written");
    let ev: SecurityAlertEvent =
        serde_json::from_str(content.lines().last().expect("one line")).expect("valid json");
    assert_eq!(ev.kind, "chain_verify_failed");
    assert_eq!(ev.level, "critical");
    assert_eq!(ev.tool, "tool-x");
    reset_for_tests();
}

// ---------------------------------------------------------------------------
// 配置编译
// ---------------------------------------------------------------------------

#[test]
fn invalid_rules_dropped_loudly() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    let cfg = minimal_cfg(vec![
        rule(
            "bad-op",
            vec![cond("kind", "not-an-op", serde_json::json!("x"))],
        ),
        rule(
            "bad-regex",
            vec![cond("summary", "regex", serde_json::json!("([unclosed"))],
        ),
        rule(
            "ok",
            vec![cond("level", "equals", serde_json::json!("critical"))],
        ),
    ]);
    configure(&cfg, std::env::temp_dir().join("alert-test-compile"));
    let (rules, _) = status_for_tests();
    assert_eq!(rules, 1, "非法规则丢弃，只留 ok");
    reset_for_tests();
}

#[test]
fn configure_is_idempotent_and_clears_state() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    let dir = std::env::temp_dir().join("alert-test-idempotent");
    let mut cfg = minimal_cfg(Vec::new());
    cfg.cooldown_secs = 3600;
    configure(&cfg, dir.clone());
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "k",
        "",
        "a",
    );
    // 重新装配后 cooldown 簿记清空 → 同键可再发。
    configure(&cfg, dir);
    let counter = Arc::new(AtomicUsize::new(0));
    register_outlet("test", capture_outlet(counter.clone()));
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "k",
        "",
        "b",
    );
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    reset_for_tests();
}

// ---------------------------------------------------------------------------
// webhook 出口效果面（计划 §二交付物：httptest server 真收断言）
// ---------------------------------------------------------------------------

/// 从裸 TCP 流读一个 HTTP 请求（头 + 按 Content-Length 的 body）。
/// 测试专用：只够 reqwest 单请求形态，不追求通用解析。
fn read_one_http_request(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    // 读到头结束。
    let header_end = loop {
        let n = stream.read(&mut chunk).expect("read headers");
        assert!(n > 0, "connection closed before headers complete");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    // Content-Length（大小写不敏感）。
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let len: usize = headers
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    while buf.len() < header_end + len {
        let n = stream.read(&mut chunk).expect("read body");
        assert!(n > 0, "connection closed before body complete");
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8_lossy(&buf[header_end..header_end + len]).into_owned()
}

#[tokio::test]
async fn webhook_delivery_reaches_http_server() {
    let _serial = TEST_LOCK.lock();
    reset_for_tests();
    // 裸 TcpListener 扮 webhook 接收端：收一请求回 200，body 经 oneshot
    // 送回断言（tokio::test 是 current-thread runtime——主测必须 await
    // 让出，spawn 出去的 POST 任务才被驱动；阻塞 recv 会死锁）。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let body = read_one_http_request(&mut stream);
        let _ = std::io::Write::write_all(
            &mut stream,
            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let _ = tx.send(body);
    });
    let mut cfg = minimal_cfg(Vec::new());
    cfg.webhooks = vec![format!("http://{addr}/hook")];
    configure(&cfg, std::env::temp_dir().join("alert-test-webhook"));
    emit(
        AlertKind::EstopTriggered,
        AlertLevel::Critical,
        "",
        "estop",
        "webhook e2e body",
    );
    let body = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
        .await
        .expect("webhook delivered within 5s")
        .expect("server sent body");
    assert!(
        body.contains("\"kind\":\"estop_triggered\""),
        "body: {body}"
    );
    assert!(body.contains("\"level\":\"critical\""), "body: {body}");
    assert!(body.contains("webhook e2e body"), "body: {body}");
    reset_for_tests();
}
