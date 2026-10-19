//! F7：WebQuestionBroker 单元测试。
//!
//! 覆盖八条路径：作答送达 + pending 清理 / 超时 Timeout 结局 + 迟到 respond
//! unknown / 双 respond 竞速先到先得 / 载荷校验（空选择、非候选、单选多项
//! ——校验失败请求留在 pending 可重试）/ pending 列表元数据 / 广播事件
//! （QuestionAsked 载荷 + QuestionResolved answered/timeout 两路）。等待侧
//! 测试在纯同步线程跑（走 `recv` 直等分支）。
//!
//! B8（提问两形态）：表单作答送达（AnsweredForm）+ 表单校验（未知键/必填
//! 空缺/select 非候选——留卡可重试）+ 两形态互斥守卫（选项卡收表单作答
//! 或反之诚实报错）+ trim 规约与可选空值剔除 + 广播/pending 携带 fields。

use super::WebQuestionBroker;
use nemesis_types::agent::{
    AgentEvent, QuestionAsker, QuestionField, QuestionOutcome, QuestionRequest, QuestionResponder,
};
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::Duration;

fn make_req(id: &str, multi: bool) -> QuestionRequest {
    QuestionRequest {
        question_id: id.to_string(),
        question: "用哪个包管理器?".to_string(),
        options: vec!["pnpm".to_string(), "npm".to_string()],
        multi,
        fields: Vec::new(),
        chat_id: "chat-1".to_string(),
        session_key: "web:chat-1".to_string(),
        timeout_secs: 10,
    }
}

/// B8：表单形态请求（select 必填 + text 可选 + select 可选三字段）。
fn make_form_req(id: &str) -> QuestionRequest {
    QuestionRequest {
        question_id: id.to_string(),
        question: "部署参数?".to_string(),
        options: Vec::new(),
        multi: false,
        fields: vec![
            QuestionField {
                key: "env".to_string(),
                label: "环境".to_string(),
                field_type: "select".to_string(),
                required: true,
                options: vec!["prod".to_string(), "staging".to_string()],
            },
            QuestionField {
                key: "note".to_string(),
                label: "备注".to_string(),
                field_type: "text".to_string(),
                required: false,
                options: Vec::new(),
            },
            QuestionField {
                key: "region".to_string(),
                label: "区域".to_string(),
                field_type: "select".to_string(),
                required: false,
                options: vec!["cn".to_string(), "eu".to_string()],
            },
        ],
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
///
/// §2 免打扰短路测试会置换进程级 APPROVAL_AUTO 槽（nemesis-web 全局态）
/// ——所有 ask 路径统一在 crate 级 APPROVAL_AUTO_SLOT_LOCK 下跑（board_bus
/// 置换测试同锁），避免与置换窗口交叠 flake。
fn slot_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::APPROVAL_AUTO_SLOT_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

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
        let _serial = slot_lock();
        let _ = tx.send(broker.ask(req));
    });
    rx
}

/// B8：表单形态 ask（同 ask_async 的锁与线程纪律）。
fn ask_form_async(
    broker: std::sync::Arc<WebQuestionBroker>,
    id: &str,
) -> mpsc::Receiver<Result<QuestionOutcome, String>> {
    let (tx, rx) = mpsc::channel();
    let req = make_form_req(id);
    std::thread::spawn(move || {
        let _serial = slot_lock();
        let _ = tx.send(broker.ask(req));
    });
    rx
}

/// 等待提问线程进入 pending（轮询代替裸 sleep，避免慢机 flake）。
///
/// 预算 30s 而非 1s：ask 线程要先过 crate 级 APPROVAL_AUTO_SLOT_LOCK——
/// 全并行下锁被其他 ask 路径测试持到各自 respond 到达为止，排队深度 =
/// 并发 ask 测试数，1s 预算会把「还在排队」误判成「没进 pending」假红
/// （本地全量并行实证：wait_pending 1s 红 → 级联 3-4 测，2026-10-14）。
fn wait_pending(broker: &WebQuestionBroker, want: usize) {
    for _ in 0..600 {
        if broker.pending().len() >= want {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("pending did not reach {} in time", want);
}

/// 有界等待广播事件（正常毫秒级到达）。超时即 panic 带定性提示，而不是让
/// 整条 libtest 永久挂起——run3 实证：APPROVAL_AUTO enabled 态外借窗口把
/// 并行 ask 短路成 Timeout（不广播），无界 recv 就把全套件拖死（dump 级
/// 定案，2026-10-13）；挂死远比断言红难排查。
async fn recv_scoped<T>(rx: &mut tokio::sync::mpsc::Receiver<T>) -> T {
    tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "广播事件 15s 未到达：疑 APPROVAL_AUTO enabled 槽被并行置换窗口外借，\
                 ask 被短路成 Timeout 不广播（置换纪律见 auto_approve_on 注释）"
            )
        })
        .unwrap()
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
        _ => panic!("answered flow must not yield Timeout"),
    }
    assert!(broker.pending().is_empty());
}

#[test]
fn multi_select_flow_delivers_all_selections() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));
    let (tx, rx) = mpsc::channel();
    let b = broker.clone();
    std::thread::spawn(move || {
        let _serial = slot_lock();
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
        _ => panic!("answered flow must not yield Timeout"),
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
    // B8：选项形态 fields 空数组照常透传（前端按选项卡渲染）。
    assert_eq!(list[0]["fields"], serde_json::Value::Array(Vec::new()));
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
    assert_eq!(
        asked["data"]["fields"],
        serde_json::Value::Array(Vec::new())
    );
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
    let waiter = tokio::spawn(async move {
        let _serial = slot_lock();
        b2.ask(make_req("q-bip", false))
    });
    // pending 未现前禁止盲目 respond：ask 线程可能还在 slot_lock 队列里，
    // 此时 respond 必然 unknown 假红，且失败后 waiter 继续持锁到
    // recv_timeout 到期（最长 10s），把锁质押给整条并行队列形成级联
    // （本地全量并行实证：本测 270 行 unwrap 假红连锁 3-4 测，2026-10-14）。
    // 定性 panic 优于误导性 unknown；waiter 自身 recv_timeout 会到期放锁。
    let mut entered = false;
    for _ in 0..600 {
        if !broker.pending().is_empty() {
            entered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        entered,
        "ask 未进入 pending（疑 slot_lock 排队超 30s）：不 respond，\
         避免盲目 unknown 假红 + 锁质押级联"
    );
    assert!(broker.respond("q-bip", vec!["pnpm".into()]).unwrap());
    match waiter.await.unwrap().unwrap() {
        QuestionOutcome::Answered(sel) => assert_eq!(sel, vec!["pnpm".to_string()]),
        _ => panic!("answered flow must not yield Timeout"),
    }
}

// ---------------------------------------------------------------------------
// §2 免打扰（2026-10-11 无人值守计划 Part 2）：开关开 = 提问短路超时语义
// ---------------------------------------------------------------------------

/// 置换窗口守卫：装 enabled 态，Drop 时恢复 Some(disabled)——对一切生产
/// 消费方行为等价未装配（都查 enabled()）。守卫是必需的而非装饰：窗口内
/// panic 若把进程级槽留在 enabled，会毒化后续所有 ask 路径测试（run2 实证
/// ——1 测窗口内 panic → 5 测失败 + broadcast_carries 永久挂起）。
struct AutoApproveSwapGuard;

impl AutoApproveSwapGuard {
    fn install() -> Self {
        nemesis_web::handlers::security::install_auto_approve(std::sync::Arc::new(
            nemesis_security::approval_auto::ApprovalAutoApproveState::new(true),
        ));
        AutoApproveSwapGuard
    }
}

impl Drop for AutoApproveSwapGuard {
    fn drop(&mut self) {
        nemesis_web::handlers::security::install_auto_approve(std::sync::Arc::new(
            nemesis_security::approval_auto::ApprovalAutoApproveState::new(false),
        ));
    }
}

#[test]
fn auto_approve_on_short_circuits_ask_to_timeout_without_pending() {
    let _serial = slot_lock();
    let _swapped = AutoApproveSwapGuard::install();
    // 直连 broadcast 接收端（try_recv 同步可用）——不走 broker_with_events
    // 的 tokio::spawn forwarder（同步 #[test] 无运行时上下文，spawn 必
    // panic，且 panic 点在 restore 之前会泄漏 enabled 槽）。
    let (btx, mut brx) = tokio::sync::broadcast::channel::<AgentEvent>(16);
    let broker = std::sync::Arc::new(WebQuestionBroker::new(Some(btx)));

    let outcome = broker
        .ask(make_req("q-auto", false))
        .expect("short-circuit must not error");
    // 锁必须持到函数结束：局部变量逆序 drop 保证 `_swapped`（enabled 态
    // 守卫）先恢复、`_serial` 后释放，置换窗口全程被锁覆盖。提前 drop 锁
    // 会留下「锁已空闲但 enabled 仍在装」的窗口，并行 ask 路径测试撞进短
    // 路分支（不广播不登记）→ broadcast_carries 挂死（run3 实证，dump 级
    // 定案 2026-10-13）。

    assert_eq!(
        outcome,
        QuestionOutcome::Timeout,
        "免打扰开：提问必须短路成超时语义（模型按最佳判断继续）"
    );
    assert!(
        broker.pending().is_empty(),
        "短路路径不得进 pending（无人值守不欠人一张卡）"
    );
    // 不广播提问事件（无 SSE 卡片数据源）。
    assert!(brx.try_recv().is_err(), "短路路径不得广播 QuestionAsked");
    // 迟到 respond 诚实报 unknown（请求从未登记）。
    assert!(broker.respond("q-auto", vec!["pnpm".into()]).is_err());
}

// ---------------------------------------------------------------------------
// B8（提问两形态）：表单作答 / 校验 / 两形态互斥
// ---------------------------------------------------------------------------

#[test]
fn form_flow_delivers_values_and_clears_pending() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));
    let result_rx = ask_form_async(broker.clone(), "q-f1");
    wait_pending(&broker, 1);
    let mut values = BTreeMap::new();
    values.insert("env".to_string(), "prod".to_string());
    values.insert("note".to_string(), " 带空格的备注 ".to_string());
    // 可选 select 缺席 = 合法（未选）。
    assert!(broker.respond_form("q-f1", &values).unwrap());
    match result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap()
    {
        QuestionOutcome::AnsweredForm(v) => {
            // text 值 trim；可选缺席不补空。
            assert_eq!(v.get("env").map(String::as_str), Some("prod"));
            assert_eq!(v.get("note").map(String::as_str), Some("带空格的备注"));
            assert!(
                !v.contains_key("region"),
                "absent optional field must stay absent"
            );
        }
        other => panic!("form flow must yield AnsweredForm, got {:?}", other),
    }
    assert!(broker.pending().is_empty());
}

#[test]
fn form_validation_rejects_and_keeps_pending_retryable() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));
    let result_rx = ask_form_async(broker.clone(), "q-f2");
    wait_pending(&broker, 1);

    // 未知键 → 拒绝，留卡可重试。
    let err = broker
        .respond_form(
            "q-f2",
            &BTreeMap::from([("nope".to_string(), "x".to_string())]),
        )
        .unwrap_err();
    assert!(err.contains("unknown field"), "err: {}", err);
    assert_eq!(broker.pending().len(), 1);

    // 必填缺席 → 拒绝（前端只交已填项也须带空串占位）。
    let err = broker
        .respond_form(
            "q-f2",
            &BTreeMap::from([("note".to_string(), "hi".to_string())]),
        )
        .unwrap_err();
    assert!(err.contains("required"), "err: {}", err);
    assert_eq!(broker.pending().len(), 1);

    // 必填空串（纯空白）→ 拒绝。
    let err = broker
        .respond_form(
            "q-f2",
            &BTreeMap::from([
                ("env".to_string(), "   ".to_string()),
                ("note".to_string(), "hi".to_string()),
            ]),
        )
        .unwrap_err();
    assert!(err.contains("required"), "err: {}", err);
    assert_eq!(broker.pending().len(), 1);

    // select 非候选 → 拒绝（含可选字段）。
    let err = broker
        .respond_form(
            "q-f2",
            &BTreeMap::from([
                ("env".to_string(), "prod".to_string()),
                ("region".to_string(), "mars".to_string()),
            ]),
        )
        .unwrap_err();
    assert!(
        err.contains("not one of the offered options"),
        "err: {}",
        err
    );
    assert_eq!(broker.pending().len(), 1);

    // 修正后重试成功。
    let values = BTreeMap::from([
        ("env".to_string(), "staging".to_string()),
        ("region".to_string(), "eu".to_string()),
    ]);
    assert!(broker.respond_form("q-f2", &values).unwrap());
    match result_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap()
    {
        QuestionOutcome::AnsweredForm(v) => {
            assert_eq!(v.get("env").map(String::as_str), Some("staging"));
            assert_eq!(v.get("region").map(String::as_str), Some("eu"));
        }
        other => panic!("form flow must yield AnsweredForm, got {:?}", other),
    }
}

#[test]
fn form_and_selection_shapes_are_mutually_exclusive() {
    let broker = std::sync::Arc::new(WebQuestionBroker::new(None));

    // 选项卡收表单作答 → 诚实报错，留卡。
    let result_rx = ask_async(broker.clone(), "q-x1", 10);
    wait_pending(&broker, 1);
    let err = broker
        .respond_form(
            "q-x1",
            &BTreeMap::from([("env".to_string(), "prod".to_string())]),
        )
        .unwrap_err();
    assert!(err.contains("expects a selection"), "err: {}", err);
    assert_eq!(broker.pending().len(), 1);
    // 修正：走正确形态仍可送达。
    assert!(broker.respond("q-x1", vec!["npm".into()]).unwrap());
    let _ = result_rx.recv_timeout(Duration::from_secs(2));

    // 表单卡收选项作答 → 诚实报错，留卡。
    let result_rx = ask_form_async(broker.clone(), "q-x2");
    wait_pending(&broker, 1);
    let err = broker.respond("q-x2", vec!["pnpm".into()]).unwrap_err();
    assert!(err.contains("form values"), "err: {}", err);
    assert_eq!(broker.pending().len(), 1);
    let values = BTreeMap::from([("env".to_string(), "prod".to_string())]);
    assert!(broker.respond_form("q-x2", &values).unwrap());
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[tokio::test]
async fn form_pending_and_broadcast_carry_fields() {
    let (broker, mut events) = broker_with_events();
    let result_rx = ask_form_async(broker.clone(), "q-fb");
    let asked = recv_scoped(&mut events).await;
    assert_eq!(asked["data"]["question_id"], "q-fb");
    let fields = asked["data"]["fields"].as_array().expect("fields array");
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0]["key"], "env");
    assert_eq!(fields[0]["type"], "select");
    assert_eq!(fields[0]["required"], true);
    assert_eq!(fields[0]["options"][1], "staging");
    assert_eq!(fields[1]["type"], "text");
    assert_eq!(fields[1]["required"], false);

    // pending 投影同样携带 fields（刷新/恢复用）。
    let list = broker.pending();
    assert_eq!(list.len(), 1);
    let pf = list[0]["fields"].as_array().expect("pending fields array");
    assert_eq!(pf[2]["key"], "region");
    assert_eq!(pf[2]["options"][0], "cn");

    let values = BTreeMap::from([("env".to_string(), "prod".to_string())]);
    assert!(broker.respond_form("q-fb", &values).unwrap());
    let resolved = recv_scoped(&mut events).await;
    assert_eq!(resolved["kind"], "QuestionResolved");
    assert_eq!(resolved["data"]["decision"], "answered");
    let _ = result_rx.recv_timeout(Duration::from_secs(2));
}

#[test]
fn respond_form_without_request_is_unknown_error() {
    let broker = WebQuestionBroker::new(None);
    let err = broker
        .respond_form(
            "nope",
            &BTreeMap::from([("env".to_string(), "x".to_string())]),
        )
        .unwrap_err();
    assert!(err.contains("unknown question"), "err: {}", err);
}
