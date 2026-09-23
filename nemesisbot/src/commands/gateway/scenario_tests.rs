//! 场景级真机 E2E —— 2026-09-23 多会话并行清账批的「原始场景复现」。
//!
//! 单测/集成测试钉的是函数级契约；本文件把四个 BUG 的**用户原始场景**在真
//! gateway 子进程（全装配 web+agent+MockAi）里逐条重演，经持久 WS 连接驱动
//! chat.send / WSAPI，断言修复行为在真实装配链路上成立：
//!
//! | 场景 | 原始 BUG | 断言核心 |
//! |------|----------|----------|
//! | s1 | 泵级串行：一个长 turn 堵死所有自由会话 → 消息「消失」 | 会话 A 的 6s 长 turn 在飞时，会话 B 的消息 4s 内得到真实回答（并行），两会话历史各自完整（不消失） |
//! | s2 | 排队消息零落盘，切会话即丢 | Queue 模式（缺省）同会话连发：排队回执即时、两条 turn 顺序完成、历史恰 4 行按序（回执不入史） |
//! | s3 | Reject 忙弹回零留痕 | reject 覆盖下第二条消息即时收到 BUSY 弹回，历史里有成对 user 行 + 弹回行（D-2 诚实留痕） |
//! | s4 | 并发上限（D-4）真机形态 | max_concurrent_turns=1 时 B 的 turn 排许可、A 后顺序完成，零丢失 |
//! | s5 | 绑定真相源在 localStorage：换机器/F5 漂移复制 | WSAPI 驱动绑定注册表全生命周期（get-or-create 幂等 / set_binding 让键 / remove_binding 幂等 / delete 级联）+ **gateway 重启后绑定依旧**（服务端真相源） |
//!
//! 与 R9 live 组的差异：R9 因「tokio-tungstenite 只在 test-harness 内部可用」
//! 而用预种子 cron 驱动入站；本批 test-harness 已补持久 WS 驱动原语
//! （ws_send_json/ws_recv_matching/chat_receive_match）与 MockAiReply::Delay
//! （可控延时 LLM 应答），故直接走前端同款 WS 链路（chat.send → P8 回声 →
//! gate → turn → assistant receive 帧），更贴近真实用户路径。
//!
//! 纪律：与 R9 组共用 `live_gate()` 互斥闸（同一时刻至多一个真 gateway 编排）；
//! home 隔离走 `--local` + tempdir cwd（r9_live_tests 先例，绝不碰真实 home）；
//! web 端口 0（OS 分配）；不占生产禁区端口。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use test_harness::mock_ai::{MockAiReply, MockAiServer};
use test_harness::{
    ManagedProcess, TestWorkspace, WsStream, graceful_shutdown_gateway, resolve_nemesisbot_bin,
    ws_connect, ws_recv_matching, ws_send_json,
};

use super::r9_live_tests::{
    install_home_config, live_gate, live_gateway_config, spawn_gateway, wait_for_web_port,
};

/// 优雅停机后等子进程自行退出的预算（对齐 r9_live_tests::EXIT_TIMEOUT_SECS）。
const EXIT_TIMEOUT_SECS: u64 = 40;

/// 场景网关夹具：真 gateway 子进程 + 可控脚本 mock LLM + 隔离 temp home。
///
/// 字段序 = Drop 序（mock 先关 → 子进程后杀 → 临时目录最后删）：杀进程前
/// 先断 mock、避免子进程在临时目录消失后还持句柄。
struct ScenarioGateway {
    mock: MockAiServer,
    proc: ManagedProcess,
    bin: PathBuf,
    token: String,
    ws: TestWorkspace,
    web_port: u16,
    home: PathBuf,
}

impl ScenarioGateway {
    /// 起一个带脚本 mock 的真 gateway，阻塞到 web bind 就绪。
    ///
    /// `cfg_tweak` 在 install 前改写配置（S3 的 reject 模式 / S4 的并发上限）。
    async fn start(
        name: &'static str,
        script: Vec<MockAiReply>,
        cfg_tweak: impl FnOnce(&mut Value),
    ) -> Self {
        let bin = resolve_nemesisbot_bin().expect("resolve nemesisbot binary");
        let ws = TestWorkspace::new().expect("temp workspace");
        let mock = MockAiServer::start(script).expect("mock ai server");
        let token = format!("scn-{}", std::process::id());
        let mut cfg = live_gateway_config(
            &ws.home().join("workspace"),
            &mock.base_url(),
            "scn-model",
            "main",
            &token,
            false, // heartbeat 关：脚本只被测试驱动的 turn 消费
            false, // cluster 关
            false, // security 关：减少无关拦截面
        );
        cfg_tweak(&mut cfg);
        install_home_config(&ws.home(), &cfg);
        let mut proc = spawn_gateway(name, &bin, &ws);
        let home = ws.home();
        let web_port = wait_for_web_port(&home).await;
        assert!(
            proc.is_running().await,
            "{name} died during boot (web_port={web_port})"
        );
        Self {
            mock,
            proc,
            bin,
            token,
            ws,
            web_port,
            home,
        }
    }

    /// 前端同款：连 /ws?token=<token>，拿持久连接。
    async fn connect(&self) -> WsStream {
        ws_connect(self.web_port, &self.token)
            .await
            .expect("ws connect to scenario gateway")
    }

    /// mock 脚本剩余条数（消费进度门：等某条 turn 的 LLM 请求真到了 mock）。
    fn remaining(&self) -> usize {
        self.mock.remaining()
    }

    /// 轮询等 mock 脚本消费到 `n` 条剩余（50ms 步进，5s 预算）。
    async fn wait_remaining(&self, n: usize, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.remaining() <= n {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "mock script not consumed to {n} within 5s ({what}; remaining={})",
                self.remaining()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// 优雅停机 → 等子进程退出 → 同 home 重启 → 等 web 重新 bind。
    /// 端口可能变化（web 绑 0），重启后必须用新 web_port 重连。
    async fn restart(&mut self) {
        graceful_shutdown_gateway(self.web_port, &self.token)
            .await
            .expect("graceful shutdown request");
        self.proc
            .wait_for_exit(Duration::from_secs(EXIT_TIMEOUT_SECS))
            .await
            .expect("gateway exited after graceful shutdown");
        self.proc = ManagedProcess::spawn(
            "scn_gw_restarted",
            &self.bin,
            &["--local", "gateway"],
            self.ws.path(),
        )
        .expect("respawn gateway");
        self.web_port = wait_restarted_web_port(&self.home, &self.token).await;
    }
}

/// 重启专用 web 就绪等待：`gateway.json` 在重启窗口里**残留旧进程的端口**
/// （子进程 Step 6 先写 port 0、bind 后才写真实端口——旧值在被覆写前一直
/// 在盘上），盲读会拿到死端口（上轮 s5 通过属时序运气，非结构保证）。
/// 旧进程已被 `wait_for_exit` 确认退出，所以「能真正完成 WS 握手」的端口
/// 必是新进程——文件读候选端口 + 实连验证双条件轮询，杜绝竞态。
async fn wait_restarted_web_port(home: &std::path::Path, token: &str) -> u16 {
    let state = home.join("workspace").join("state").join("gateway.json");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Ok(txt) = std::fs::read_to_string(&state)
            && let Ok(v) = serde_json::from_str::<Value>(&txt)
            && let Some(p) = v.get("web_port").and_then(|x| x.as_u64())
            && p > 0
            && p <= u16::MAX as u64
        {
            let candidate = p as u16;
            // 3s 上限只约束单次握手（localhost 死端口立即 ECONNREFUSED，
            // 不会真等满）；失败即回睡重等下一轮文件更新。
            let reachable = matches!(
                tokio::time::timeout(Duration::from_secs(3), ws_connect(candidate, token)).await,
                Ok(Ok(_))
            );
            if reachable {
                return candidate;
            }
        }
        assert!(
            Instant::now() < deadline,
            "restarted gateway web not reachable within 120s (state={:?})",
            std::fs::read_to_string(&state).ok()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ---------------------------------------------------------------------------
// WS 驱动小组件（前端同款帧形态）
// ---------------------------------------------------------------------------

/// chat.send（多会话形态：data.session_id 定向）。
async fn chat_send(stream: &mut WsStream, content: &str, session_id: &str) {
    ws_send_json(
        stream,
        &json!({
            "type": "message", "module": "chat", "cmd": "send",
            "data": { "content": content, "session_id": session_id },
        }),
    )
    .await
    .expect("chat.send frame write");
}

/// 等一帧指定会话/角色的 assistant receive 帧并返回其内容
/// （user 回声与无关帧在谓词里被跳过；帧序 = 断言序）。
async fn wait_assistant(stream: &mut WsStream, sid: &str, contains: &str, timeout: Duration) {
    let what = format!("assistant frame [{sid}] ~ \"{contains}\"");
    ws_recv_matching(stream, timeout, &what, |v| {
        test_harness::chat_receive_match(v, Some(sid), Some("assistant"), contains)
    })
    .await
    .expect(&what);
}

/// chat.history_request → message/chat/history 帧 → (role, content) 行序。
/// 历史读的是磁盘 chat_log（与前端切会话加载同一真相源）。
async fn fetch_history(stream: &mut WsStream, sid: &str) -> Vec<(String, String)> {
    static N: AtomicU64 = AtomicU64::new(0);
    let rid = format!("scn-hist-{}", N.fetch_add(1, Ordering::Relaxed));
    ws_send_json(
        stream,
        &json!({
            "type": "message", "module": "chat", "cmd": "history_request",
            "data": { "request_id": rid, "limit": 50, "session_id": sid },
        }),
    )
    .await
    .expect("history_request frame write");
    let frame = ws_recv_matching(
        stream,
        Duration::from_secs(10),
        &format!("history response [{sid}]"),
        |v| {
            v.get("type").and_then(|t| t.as_str()) == Some("message")
                && v.get("module").and_then(|m| m.as_str()) == Some("chat")
                && v.get("cmd").and_then(|c| c.as_str()) == Some("history")
                && v["data"]["request_id"].as_str() == Some(rid.as_str())
        },
    )
    .await
    .expect("history response frame");
    frame["data"]["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("history [{sid}] missing messages array: {frame}"))
        .iter()
        .map(|m| {
            (
                m["role"].as_str().unwrap_or_default().to_string(),
                m["content"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// WSAPI request/response 一问一答（持久连接上按 reqId 认领响应帧）。
async fn wsapi(
    stream: &mut WsStream,
    module: &str,
    cmd: &str,
    data: Value,
) -> Result<Value, String> {
    static N: AtomicU64 = AtomicU64::new(0);
    let rid = format!("scn-req-{}", N.fetch_add(1, Ordering::Relaxed));
    let claimed = rid.clone();
    ws_send_json(
        stream,
        &json!({
            "type": "request", "module": module, "cmd": cmd,
            "reqId": rid, "data": data,
        }),
    )
    .await
    .map_err(|e| e.to_string())?;
    let frame = ws_recv_matching(
        stream,
        Duration::from_secs(10),
        &format!("{module}.{cmd} response"),
        move |v| {
            v.get("type").and_then(|t| t.as_str()) == Some("response")
                && v.get("reqId").and_then(|r| r.as_str()) == Some(claimed.as_str())
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    if let Some(err) = frame.get("error").and_then(|e| e.as_str()) {
        return Err(format!("{module}.{cmd} error: {err}"));
    }
    Ok(frame.get("data").cloned().unwrap_or(Value::Null))
}

/// 历史 (role, content) 行序整断言（内容按包含匹配，避开时间戳噪声）。
fn expect_rows(rows: &[(String, String)], expected: &[(&str, &str)], ctx: &str) {
    assert_eq!(
        rows.len(),
        expected.len(),
        "{ctx}: history row count mismatch (got {rows:?})"
    );
    for (i, ((role, content), (erole, econtains))) in rows.iter().zip(expected.iter()).enumerate() {
        assert_eq!(role, erole, "{ctx}: row {i} role mismatch ({rows:?})");
        assert!(
            content.contains(econtains),
            "{ctx}: row {i} content {content:?} does not contain {econtains:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// s1 —— 主根因：统一泵跨会话并行（原 BUG：数分钟长 turn 堵死全部会话）
// ---------------------------------------------------------------------------

/// 会话 A 挂一个 6s 的长 turn；A 在飞期间会话 B 发消息。
/// 修复前（泵级串行）：B 的消息在 A 的 turn 结束前永不出队——B 要么干等
/// 6s+ 要么被当成「忙」弹回；修复后：B 在 4s 内得到真实回答（并行），
/// 且两会话历史各自完整（原始「消息消失」现象的回归钉）。
#[tokio::test]
async fn s1_cross_session_turns_run_in_parallel() {
    let _gate = live_gate().await;
    let gw = ScenarioGateway::start(
        "scn_s1_gw",
        vec![
            // A 的 turn：延时 6s 再回——给「B 在 A 飞行中插入」留窗口。
            MockAiReply::Delay {
                secs: 6,
                reply: Box::new(MockAiReply::Text("甲-完成".into())),
            },
            // B 的 turn：即时回。
            MockAiReply::Text("乙-完成".into()),
        ],
        |_| {},
    )
    .await;
    let mut conn = gw.connect().await;

    chat_send(&mut conn, "甲长任务", "scn-a").await;
    // 等 A 的 LLM 请求真到 mock（脚本 2→1），保证 FIFO 脚本次序 = A 先 B 后。
    gw.wait_remaining(1, "A turn LLM request").await;

    let t_send_b = Instant::now();
    chat_send(&mut conn, "乙插话", "scn-b").await;
    // 并行性核心断言：B 的真实回答在 4s 内到达（串行世界 B 必须 ≥6s 后才
    // 轮到；4s 界两侧间隔充足，不受 CI CPU 抖动影响）。
    let b_frame = tokio::time::timeout(Duration::from_secs(8), async {
        ws_recv_matching(
            &mut conn,
            Duration::from_secs(8),
            "assistant frame [scn-b]",
            |v| test_harness::chat_receive_match(v, Some("scn-b"), Some("assistant"), "乙-完成"),
        )
        .await
        .expect("assistant frame [scn-b]")
    })
    .await
    .expect("B reply within 8s wall clock");
    let b_ms = t_send_b.elapsed().as_millis();
    assert!(
        b_ms < 4000,
        "跨会话并行破坏：B 等了 {b_ms}ms 才得到回答（>4s 说明仍被 A 的长 turn 串行阻塞）；frame={b_frame}"
    );

    // A 的长 turn 正常收尾（不算丢失）。
    wait_assistant(&mut conn, "scn-a", "甲-完成", Duration::from_secs(15)).await;

    // 「消息不消失」：两会话历史各自含完整 user/assistant 行。
    let ha = fetch_history(&mut conn, "scn-a").await;
    expect_rows(
        &ha,
        &[("user", "甲长任务"), ("assistant", "甲-完成")],
        "scn-a",
    );
    let hb = fetch_history(&mut conn, "scn-b").await;
    expect_rows(
        &hb,
        &[("user", "乙插话"), ("assistant", "乙-完成")],
        "scn-b",
    );

    // 脚本恰好被两个 turn 消费（多余 LLM 调用 = 标题生成/安全链等意外消费面，响失败）。
    assert_eq!(gw.remaining(), 0, "mock 脚本应被恰好消费完");
}

// ---------------------------------------------------------------------------
// s2 —— 同会话排队（原 BUG：排队消息零落盘，切会话回来历史里查无此行）
// ---------------------------------------------------------------------------

/// Queue（缺省）模式下同会话连发两条：第二条即时拿到「已排队」回执，A 的
/// turn 结束后顺序执行；历史恰好 4 行按时间序——回执不入史、排队消息的
/// user 行与最终回答都在（原始「消失」现象的回归钉）。
#[tokio::test]
async fn s2_same_session_queue_preserves_and_answers() {
    let _gate = live_gate().await;
    let gw = ScenarioGateway::start(
        "scn_s2_gw",
        vec![
            MockAiReply::Delay {
                secs: 4,
                reply: Box::new(MockAiReply::Text("一-完成".into())),
            },
            MockAiReply::Text("二-完成".into()),
        ],
        |_| {},
    )
    .await;
    let mut conn = gw.connect().await;

    chat_send(&mut conn, "消息一", "scn-q").await;
    gw.wait_remaining(1, "first turn LLM request").await;
    chat_send(&mut conn, "消息二", "scn-q").await;

    // 排队回执即时（同会话忙处置；不阻塞在 turn 上）。
    wait_assistant(&mut conn, "scn-q", "已排队", Duration::from_secs(3)).await;
    // 两条回答按序完成。
    wait_assistant(&mut conn, "scn-q", "一-完成", Duration::from_secs(15)).await;
    wait_assistant(&mut conn, "scn-q", "二-完成", Duration::from_secs(15)).await;

    // 历史恰 4 行按序：排队消息没丢、回执不污染历史。
    let rows = fetch_history(&mut conn, "scn-q").await;
    expect_rows(
        &rows,
        &[
            ("user", "消息一"),
            ("assistant", "一-完成"),
            ("user", "消息二"),
            ("assistant", "二-完成"),
        ],
        "scn-q",
    );
}

// ---------------------------------------------------------------------------
// s3 —— Reject 模式诚实留痕（原 BUG：忙弹回零落盘，切会话即「消失」）
// ---------------------------------------------------------------------------

/// 显式 reject 覆盖：同会话第二条消息即时收到 BUSY 弹回，且历史里有成对
/// 痕迹（user 行 + 弹回行）——D-2 的用户可见契约。
#[tokio::test]
async fn s3_reject_mode_bounce_leaves_paired_trace() {
    let _gate = live_gate().await;
    let gw = ScenarioGateway::start(
        "scn_s3_gw",
        vec![MockAiReply::Delay {
            secs: 3,
            reply: Box::new(MockAiReply::Text("一-完成".into())),
        }],
        |cfg| {
            cfg["agents"]["defaults"]["concurrent_request_mode"] = json!("reject");
        },
    )
    .await;
    let mut conn = gw.connect().await;

    chat_send(&mut conn, "消息一", "scn-r").await;
    gw.wait_remaining(0, "first turn LLM request").await;
    chat_send(&mut conn, "消息二", "scn-r").await;

    // 即时弹回（Reject 是会话级处置，不再有泵级等待）。
    wait_assistant(
        &mut conn,
        "scn-r",
        "AI is processing a previous request",
        Duration::from_secs(3),
    )
    .await;
    // 第一条 turn 正常完成。
    wait_assistant(&mut conn, "scn-r", "一-完成", Duration::from_secs(15)).await;

    // D-2 留痕：弹回的消息在历史里可追溯（user 行 + 弹回行成对）。
    // 时序 = turn 开始落 user 一 → 弹回落 user 二 + BUSY 行 → turn 完成落回答。
    let rows = fetch_history(&mut conn, "scn-r").await;
    expect_rows(
        &rows,
        &[
            ("user", "消息一"),
            ("user", "消息二"),
            ("assistant", "AI is processing a previous request"),
            ("assistant", "一-完成"),
        ],
        "scn-r",
    );
}

// ---------------------------------------------------------------------------
// s4 —— 并发上限真机形态（D-4：上限只排队、不丢失）
// ---------------------------------------------------------------------------

/// max_concurrent_turns=1：B 会话的 turn 在许可上排队，A 完成后立刻执行；
/// 两条都完整落历史——「上限不丢消息」的真机回归钉。
#[tokio::test]
async fn s4_turn_limit_caps_concurrency_without_loss() {
    let _gate = live_gate().await;
    let gw = ScenarioGateway::start(
        "scn_s4_gw",
        vec![
            MockAiReply::Delay {
                secs: 2,
                reply: Box::new(MockAiReply::Text("甲-完成".into())),
            },
            MockAiReply::Text("乙-完成".into()),
        ],
        |cfg| {
            cfg["agents"]["defaults"]["max_concurrent_turns"] = json!(1);
        },
    )
    .await;
    let mut conn = gw.connect().await;

    chat_send(&mut conn, "甲长任务", "scn-a").await;
    gw.wait_remaining(1, "A turn LLM request").await;
    chat_send(&mut conn, "乙插话", "scn-b").await;

    // 上限 1 → B 的 turn 必须等 A 的 permit：A 先完成，B 紧随（不丢、不弹回）。
    wait_assistant(&mut conn, "scn-a", "甲-完成", Duration::from_secs(15)).await;
    wait_assistant(&mut conn, "scn-b", "乙-完成", Duration::from_secs(15)).await;

    let ha = fetch_history(&mut conn, "scn-a").await;
    expect_rows(
        &ha,
        &[("user", "甲长任务"), ("assistant", "甲-完成")],
        "scn-a",
    );
    let hb = fetch_history(&mut conn, "scn-b").await;
    expect_rows(
        &hb,
        &[("user", "乙插话"), ("assistant", "乙-完成")],
        "scn-b",
    );
    assert_eq!(gw.remaining(), 0);
}

// ---------------------------------------------------------------------------
// s5 —— 绑定注册表全生命周期（原 BUG：真相源在 localStorage，换机器/F5
//       漂移复制出成批空会话；保存工作流后仍绑【新建工作流】）
// ---------------------------------------------------------------------------

/// WSAPI 驱动 sessions.create(binding_key) 的原子 get-or-create、set_binding
/// 让键、remove_binding 幂等释放、delete 级联；再重启 gateway 验证绑定
/// 持久（服务端真相源，换进程不漂移）。
#[tokio::test]
async fn s5_binding_registry_lifecycle_survives_restart() {
    let _gate = live_gate().await;
    let mut gw = ScenarioGateway::start("scn_s5_gw", vec![], |_| {}).await;
    let mut conn = gw.connect().await;

    // 1. 带键创建 → 新会话；同键再创建 → 同会话 reused=true（零新建）。
    let first = wsapi(
        &mut conn,
        "sessions",
        "create",
        json!({ "binding_key": "wf_agentgen:__new__", "title": "对话生成：新建工作流" }),
    )
    .await
    .expect("create #1");
    let s1 = first["session_id"]
        .as_str()
        .expect("session_id #1")
        .to_string();
    assert_eq!(first["reused"], json!(false), "首建必须 created");

    let again = wsapi(
        &mut conn,
        "sessions",
        "create",
        json!({ "binding_key": "wf_agentgen:__new__", "title": "对话生成：新建工作流" }),
    )
    .await
    .expect("create #2");
    assert_eq!(
        again["session_id"].as_str(),
        Some(s1.as_str()),
        "同键重复创建必须命中同一会话（复制机器漂移复制的回归钉）"
    );
    assert_eq!(again["reused"], json!(true));

    // 2. 另一键 → 另一会话（键隔离）。
    let second = wsapi(
        &mut conn,
        "sessions",
        "create",
        json!({ "binding_key": "wf_agentgen:draft", "title": "对话生成：draft" }),
    )
    .await
    .expect("create #3");
    let s2 = second["session_id"]
        .as_str()
        .expect("session_id #2")
        .to_string();
    assert_ne!(s1, s2);

    // 3. set_binding 让键（draft_apply 场景：__new__ 接管到正式会话）。
    let ok = wsapi(
        &mut conn,
        "sessions",
        "set_binding",
        json!({ "binding_key": "wf_agentgen:__new__", "session_id": s2 }),
    )
    .await
    .expect("set_binding");
    assert_eq!(ok["ok"], json!(true));
    let list = wsapi(&mut conn, "sessions", "list", json!({}))
        .await
        .expect("list");
    assert_eq!(
        list["bindings"]["wf_agentgen:__new__"].as_str(),
        Some(s2.as_str()),
        "__new__ 键已让渡到正式会话"
    );
    assert_eq!(
        list["bindings"]["wf_agentgen:draft"].as_str(),
        Some(s2.as_str())
    );

    // 4. remove_binding 幂等释放。
    let rm = wsapi(
        &mut conn,
        "sessions",
        "remove_binding",
        json!({ "binding_key": "wf_agentgen:__new__" }),
    )
    .await
    .expect("remove_binding #1");
    assert_eq!(rm["removed"], json!(true));
    let rm2 = wsapi(
        &mut conn,
        "sessions",
        "remove_binding",
        json!({ "binding_key": "wf_agentgen:__new__" }),
    )
    .await
    .expect("remove_binding #2");
    assert_eq!(rm2["removed"], json!(false), "重复摘键必须幂等 false");

    // 5. delete 级联：删掉 s2 后 draft 键一并消失。
    wsapi(&mut conn, "sessions", "delete", json!({ "session_id": s2 }))
        .await
        .expect("delete s2");
    let list = wsapi(&mut conn, "sessions", "list", json!({}))
        .await
        .expect("list after delete");
    assert!(
        list["bindings"].get("wf_agentgen:draft").is_none(),
        "delete 必须级联摘键（残留 = 孤儿绑定）：{}",
        list["bindings"]
    );

    // 6. 重启 gateway（换进程 ≈ 用户重启/换机器场景）→ 绑定持久、reused 命中。
    let third = wsapi(
        &mut conn,
        "sessions",
        "create",
        json!({ "binding_key": "scn:persist", "title": "持久性探针" }),
    )
    .await
    .expect("create pre-restart");
    let s3 = third["session_id"]
        .as_str()
        .expect("session_id #3")
        .to_string();

    gw.restart().await;
    let mut conn2 = gw.connect().await;
    let after = wsapi(
        &mut conn2,
        "sessions",
        "create",
        json!({ "binding_key": "scn:persist", "title": "持久性探针" }),
    )
    .await
    .expect("create post-restart");
    assert_eq!(
        after["session_id"].as_str(),
        Some(s3.as_str()),
        "重启后同键必须命中同一会话（绑定真相源在服务端，不在客户端内存/localStorage）"
    );
    assert_eq!(after["reused"], json!(true));
}
