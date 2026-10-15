//! Connector 注册表 e2e（行动代理 P0，2026-10-14）。
//!
//! 真机全链验证三域 demo 的「发现 → 低危直通 → 强审批批准 → 预算熔断 →
//! 审批拒绝」五幕，断言走 **WSAPI 数据面**（PLAN §6.2 冻结契约：
//! `connectors.list` / `connectors.receipts.list` / `paperbroker.ledger`）：
//!   ① FIND：connector_find 列全部 connector——注册表面 connectors.list
//!      三域在场且零损坏条目；
//!   ② QUOTE：paper-broker/quote（rest http 绑定）→ executed 凭条（SSRF
//!      层保活 + allowed_hosts 白名单放行回环 + base 改写指 TestAIServer
//!      /quote——层关会触发 connector fail-closed 全拒，见 setup 注释）；
//!   ③ ORDER：place_order（critical + confirm=required + budget=finance）
//!      → 审批卡 → 批准 → executed 凭条 + 账本三处一致（cash 减少 /
//!      持仓出现 / 订单 filled）；
//!   ④ NOBUDGET：qty 超限单 → budget_blocked 凭条（预算闸先于 dispatch，
//!      不弹卡）+ 账本零新增（不批准/被拦 = 有凭条无副作用）；
//!   ⑤ APPLY：local-fs/move（high + confirm=required）→ 审批卡 → 拒绝 →
//!      denied_approval 凭条（dispatch 代写臂）。
//!
//! 驱动模型 testai-conn-1.0：CONN 标记状态机（每轮用户消息带标记 → 对应
//! 真实工具调用；工具结果回灌轮固定回 CONN_DEMO_DONE 终端文本，e2e 以此
//! 断言回合收口）。分支判别全在凭条面/账本面/审批生命周期，不依赖回复措辞。
//!
//! `#[ignore]` 惯例（与 preexec_scan/wal_crash 一致），显式运行：
//!   cargo test -p nemesisbot-tests --test connector_flow -- --ignored
//! 前置：testaiserver.exe（含 testai-conn-1.0 与 /quote 端点）+
//! target/release/nemesisbot.exe 已重编到位；Windows。
//!
//! 端口组 18094/49035/49037/18795（避让其他 e2e 与用户在跑的 49000/49001）。

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::time::Duration;
use test_harness::{
    ManagedProcess, TestWorkspace, WsStream, chat_receive_match, cleanup_ports,
    resolve_ai_server_bin, resolve_nemesisbot_bin, wait_for_http, ws_connect, ws_recv_matching,
    ws_send_json,
};

const AI_PORT: u16 = 18094;
const WEB_PORT: u16 = 49035;
const WS_CHANNEL_PORT: u16 = 49037;
const HEALTH_PORT: u16 = 18795;

const SESSION: &str = "conn_e2e";
const REPLY_TIMEOUT: Duration = Duration::from_secs(90);

/// demo manifest 的 quote base 占位——e2e 改写为回环 TestAIServer。
const DEMO_QUOTE_BASE: &str = "https://quotes.example.com";

async fn send_chat(stream: &mut WsStream, content: &str) -> Result<()> {
    let msg = json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": { "content": content, "session_id": SESSION },
    });
    ws_send_json(stream, &msg).await
}

/// 等下一条 assistant receive 帧（回合最终回复；user 回声/推送帧由谓词
/// 天然过滤）。conn 状态机每回合恰一条 assistant（CONN_DEMO_DONE）。
async fn wait_assistant(stream: &mut WsStream, what: &str) -> Result<String> {
    let frame = ws_recv_matching(stream, REPLY_TIMEOUT, what, |v| {
        chat_receive_match(v, Some(SESSION), Some("assistant"), "")
    })
    .await?;
    Ok(frame["data"]["content"].as_str().unwrap_or("").to_string())
}

/// WSAPI 请求帧（type="request"；type=message 走 legacy 总线 → unknown
/// module）+ 等匹配 reqId 的响应帧。
async fn wsapi_call(
    stream: &mut WsStream,
    module: &str,
    cmd: &str,
    data: Value,
    what: &str,
) -> Result<Value> {
    let req_id = format!("conn-e2e-{module}-{cmd}-{}", std::process::id());
    let mut frame = json!({
        "type": "request", "module": module, "cmd": cmd, "reqId": req_id,
    });
    if !data.is_null() {
        frame["data"] = data;
    }
    ws_send_json(stream, &frame).await?;
    let resp = ws_recv_matching(stream, Duration::from_secs(15), what, |v| {
        v["type"] == "response" && v["reqId"] == req_id
    })
    .await?;
    if let Some(err) = resp["error"].as_str() {
        bail!("{what} failed: {err}");
    }
    Ok(resp["data"].clone())
}

/// 轮询 approval.pending 直到出现点名 `op_name` 的 connector 审批卡
/// （force_approval reason 携带「connector `<id>` op `<op>` 声明强制人工
/// 确认」文案；connector 卡 target 为空串，按 reason 过滤）。回合阻塞在
/// 审批等待时无聊天帧流动，请求/响应交替安全。
async fn poll_card(stream: &mut WsStream, op_name: &str, timeout_secs: u64) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut n = 0u64;
    loop {
        n += 1;
        let resp = wsapi_call(
            stream,
            "approval",
            "pending",
            json!(null),
            &format!("approval.pending#{n}"),
        )
        .await
        .context("approval.pending call failed")?;
        if let Some(card) = resp["pending"].as_array().and_then(|arr| {
            arr.iter()
                .find(|c| c["reason"].as_str().is_some_and(|r| r.contains(op_name)))
        }) {
            return Ok(card.clone());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "timeout ({timeout_secs}s) waiting for approval card naming {op_name:?} — \
                 confirm=required did not force a card (raw pending: {resp})"
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// 送裁决并等回执；回执 error 非空即失败。
async fn respond_card(stream: &mut WsStream, request_id: &str, approved: bool) -> Result<()> {
    let tag = format!("respond-{}", if approved { "yes" } else { "no" });
    let req_id = format!("conn-e2e-{tag}");
    let frame = json!({
        "type": "request", "module": "approval", "cmd": "respond", "reqId": req_id,
        "data": { "request_id": request_id, "approved": approved },
    });
    ws_send_json(stream, &frame).await?;
    let resp = ws_recv_matching(
        stream,
        Duration::from_secs(10),
        "approval.respond response",
        |v| v["type"] == "response" && v["reqId"] == req_id,
    )
    .await?;
    if let Some(err) = resp["error"].as_str() {
        bail!("approval.respond({tag}, approved={approved}) failed: {err}");
    }
    Ok(())
}

/// 轮询回执流水直到 connector+op 凭条数 ≥ `min_count`，返回**最新一条**。
///
/// 同 (connector, op) 幕间会累积（③ AAPL 单 vs ④ MSFT 单都是 place_order），
/// `find` 第一条会把上一幕的旧凭条误当本幕结果；且上一幕排队中的消息会让
/// 本幕凭条晚落盘——轮询 + 计数下限一起把时序和取数两个坑一起封掉。
async fn find_receipt(
    stream: &mut WsStream,
    connector: &str,
    op: &str,
    min_count: usize,
) -> Result<Value> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let data = wsapi_call(
            stream,
            "connectors",
            "receipts.list",
            json!({ "limit": 50 }),
            "receipts.list",
        )
        .await?;
        let matches: Vec<Value> = data["receipts"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter(|r| r["connector"] == connector && r["op"] == op)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if matches.len() >= min_count {
            // receipts.list 新→旧：首个匹配元素即最新凭条。
            return Ok(matches.into_iter().next().expect("min_count >= 1"));
        }
        if std::time::Instant::now() > deadline {
            anyhow::bail!(
                "receipt {connector}/{op} never reached count {min_count} (got {}) within 30s",
                matches.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[tokio::test]
#[ignore = "connector 注册表真机：需 testaiserver.exe（含 testai-conn-1.0）+ target/release/nemesisbot.exe"]
async fn connector_registry_flow_e2e() -> Result<()> {
    let bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;
    cleanup_ports(&[AI_PORT, WEB_PORT, WS_CHANNEL_PORT, HEALTH_PORT]);

    let ws = TestWorkspace::new()?;
    let root = ws.path().to_path_buf();

    let mut ai = ManagedProcess::spawn(
        "testaiserver",
        &ai_bin,
        &["--port", &AI_PORT.to_string()],
        &root,
    )?;
    wait_for_http(
        &format!("http://127.0.0.1:{AI_PORT}/health"),
        Duration::from_secs(20),
    )
    .await
    .context("TestAIServer failed to start")?;

    let out = ws.run_cli(&bin, &["onboard", "default"]).await;
    anyhow::ensure!(
        out.success(),
        "onboard default failed\nstdout: {}\nstderr: {}",
        out.stdout,
        out.stderr
    );

    // 三域 demo manifest 随 workspace 模板提取——缺失即模板坏了，先炸这里。
    let conn_dir = ws.workspace().join("connectors");
    for id in ["monitor", "local-fs", "paper-broker"] {
        ensure!(
            conn_dir.join(id).join("connector.toml").is_file(),
            "demo connector `{id}` missing from workspace template (connectors/ not embedded?)"
        );
    }

    // quote base 改写指回环 TestAIServer /quote（回环可达性由安全基座的
    // SSRF allowed_hosts 白名单放行，见下方 config.security.json 段）。
    let manifest_path = conn_dir.join("paper-broker").join("connector.toml");
    let manifest = std::fs::read_to_string(&manifest_path)?;
    ensure!(
        manifest.contains(DEMO_QUOTE_BASE),
        "paper-broker manifest does not carry the demo quote base — template drifted?"
    );
    std::fs::write(
        &manifest_path,
        manifest.replace(DEMO_QUOTE_BASE, &format!("http://127.0.0.1:{AI_PORT}")),
    )?;

    let out = ws
        .run_cli(
            &bin,
            &[
                "model",
                "add",
                "--model",
                "test/testai-conn-1.0",
                "--base",
                &format!("http://127.0.0.1:{AI_PORT}/v1"),
                "--key",
                "test-key",
                "--default",
            ],
        )
        .await;
    anyhow::ensure!(
        out.success(),
        "model add failed\nstdout: {}\nstderr: {}",
        out.stdout,
        out.stderr
    );

    // 端口 + connector 限额（ORDER qty=10 过 / NOBUDGET qty=999999999 撞
    // max_order_qty=100 熔断）。
    let cfg_path = ws.config_path();
    let mut cfg: Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path)?)?;
    cfg["channels"]["web"]["port"] = json!(WEB_PORT);
    cfg["channels"]["websocket"]["port"] = json!(WS_CHANNEL_PORT);
    cfg["gateway"]["port"] = json!(HEALTH_PORT);
    cfg["connectors"] = json!({
        "enabled": true,
        "max_ops_per_hour": 50,
        "finance": { "max_order_qty": 100.0, "max_orders_per_day": 5 },
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)?)?;

    // 安全基座：LOW 直通（default_action=allow）+ guardian 关（CRITICAL
    // 不进 LLM 语义二审——force_approval 卡是唯一等待点）+ 卡片超时给足
    // 轮询余量。SSRF 层保持开启（层关 → guard=None → connector http 绑定
    // fail-closed 全拒），白名单放行回环 TestAIServer——这正是 e2e 首跑
    // 抓出的姿势差：放行内网端点走 allowed_hosts，不 gut 整层。
    let sec_path = ws
        .home()
        .join("workspace")
        .join("config")
        .join("config.security.json");
    let mut sec: Value = serde_json::from_str(&std::fs::read_to_string(&sec_path).context(
        "config.security.json missing after onboard — security template not extracted?",
    )?)?;
    sec["default_action"] = json!("allow");
    sec["exec_unknown_policy"] = json!("allow");
    sec["guardian_mode"] = json!("off");
    sec["layers"]["ssrf"] = json!({ "enabled": true, "allowed_hosts": ["127.0.0.1"] });
    sec["approval_timeout_seconds"] = json!(120);
    std::fs::write(&sec_path, serde_json::to_string_pretty(&sec)?)?;

    let mut gw = ManagedProcess::spawn("nemesisbot-gateway", &bin, &["--local", "gateway"], &root)?;
    wait_for_http(
        &format!("http://127.0.0.1:{WEB_PORT}/"),
        Duration::from_secs(60),
    )
    .await
    .context("gateway failed to start (web port)")?;

    let token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut stream = ws_connect(WEB_PORT, &token)
        .await
        .context("WS connect to test gateway failed")?;

    let result = run_flow(&mut stream).await;
    let _ = ai.kill().await;
    let _ = gw.kill().await;
    result
}

async fn run_flow(stream: &mut WsStream) -> Result<()> {
    // ── ① FIND：connector_find 列全部 → 注册表面三域在场、零损坏 ──────
    send_chat(stream, "<CONN_FIND>").await?;
    let reply = wait_assistant(stream, "find turn reply").await?;
    ensure!(
        reply.contains("CONN_DEMO_DONE"),
        "find turn must close with the terminal marker, got {reply:?}"
    );
    let registry = wsapi_call(stream, "connectors", "list", json!(null), "connectors.list").await?;
    let ids: Vec<String> = registry["connectors"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    for id in ["monitor", "local-fs", "paper-broker"] {
        ensure!(
            ids.iter().any(|i| i == id),
            "connector `{id}` missing from live registry: {ids:?}"
        );
    }
    let corrupt: Vec<&Value> = registry["connectors"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|c| c["error"].as_str().is_some_and(|e| !e.is_empty()))
                .collect()
        })
        .unwrap_or_default();
    ensure!(
        corrupt.is_empty(),
        "registry carries corrupt entries: {corrupt:?}"
    );

    // ── ② QUOTE：rest http 绑定 → executed 凭条（低危直通，无卡）──────
    send_chat(stream, "<CONN_QUOTE>").await?;
    wait_assistant(stream, "quote turn reply").await?;
    let r = find_receipt(stream, "paper-broker", "quote", 1).await?;
    ensure!(
        r["outcome"] == "executed",
        "quote must execute against the rewritten loopback base, got: {r}"
    );

    // ── ③ ORDER：critical + confirm=required → 卡 → 批准 → 账本三处一致
    send_chat(stream, "<CONN_ORDER>").await?;
    let card = poll_card(stream, "place_order", 60).await?;
    ensure!(
        card["reason"]
            .as_str()
            .is_some_and(|s| s.contains("confirm=required")),
        "order card reason must carry the confirm=required verdict: {card}"
    );
    let request_id = card["request_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("order card has empty request_id: {card}"))?
        .to_string();
    respond_card(stream, &request_id, true).await?;
    wait_assistant(stream, "order turn reply").await?;
    let r = find_receipt(stream, "paper-broker", "place_order", 1).await?;
    ensure!(
        r["outcome"] == "executed",
        "approved order must execute, got: {r}"
    );
    ensure!(
        r["approval_ref"].as_str().is_some_and(|s| !s.is_empty()),
        "approved order receipt must reference the approval: {r}"
    );
    let ledger = wsapi_call(
        stream,
        "paperbroker",
        "ledger",
        json!(null),
        "paperbroker.ledger",
    )
    .await?;
    ensure!(
        ledger["cash"] == json!(998_500.0),
        "cash must drop by 10×150 after the approved buy, got: {}",
        ledger["cash"]
    );
    let positions = ledger["positions"].as_array().cloned().unwrap_or_default();
    ensure!(
        positions.len() == 1
            && positions[0]["symbol"] == "AAPL"
            && positions[0]["qty"] == json!(10.0),
        "AAPL×10 position expected after the approved buy: {positions:?}"
    );
    let orders = ledger["orders"].as_array().cloned().unwrap_or_default();
    ensure!(
        orders.len() == 1 && orders[0]["status"] == "filled",
        "exactly one filled order expected: {orders:?}"
    );

    // ── ④ NOBUDGET：qty 超限 → budget_blocked 凭条 + 账本零新增 ──────
    // 先人工后限额：CRITICAL 单的审批闸在预算闸之前，超额单同样先弹卡
    // （无人批 = 工具挂到 120s 审批超时）——批准后预算闸才轮到拒绝。
    send_chat(stream, "<CONN_NOBUDGET>").await?;
    let card = poll_card(stream, "place_order", 60).await?;
    let request_id = card["request_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("nobudget card has empty request_id: {card}"))?
        .to_string();
    respond_card(stream, &request_id, true).await?;
    wait_assistant(stream, "nobudget turn reply").await?;
    let r = find_receipt(stream, "paper-broker", "place_order", 2).await?;
    ensure!(
        r["outcome"] == "budget_blocked",
        "over-limit order must be budget-blocked, got: {r}"
    );
    let ledger = wsapi_call(
        stream,
        "paperbroker",
        "ledger",
        json!(null),
        "paperbroker.ledger#2",
    )
    .await?;
    let orders = ledger["orders"].as_array().cloned().unwrap_or_default();
    ensure!(
        orders.len() == 1,
        "blocked order must leave the ledger untouched (orders={}), got {}",
        orders.len(),
        orders.len()
    );

    // ── ⑤ APPLY：local-fs/move → 卡 → 拒绝 → denied_approval 凭条 ────
    send_chat(stream, "<CONN_APPLY>").await?;
    let card = poll_card(stream, "move", 60).await?;
    let request_id = card["request_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("move card has empty request_id: {card}"))?
        .to_string();
    respond_card(stream, &request_id, false).await?;
    wait_assistant(stream, "apply turn reply").await?;
    let r = find_receipt(stream, "local-fs", "move", 1).await?;
    ensure!(
        r["outcome"] == "denied_approval",
        "denied move must carry the dispatch-written denied_approval receipt, got: {r}"
    );

    Ok(())
}
