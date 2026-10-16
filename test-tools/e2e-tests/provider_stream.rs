//! Provider 流式输出事件 e2e（L10，2026-10-13 三方学习 T3）。
//!
//! 真机全链验证 `provider_stream_event` push 帧形态（验收线的「ws-api-send
//! 断言」项：ws-api-send 是请求/响应工具拿不到 push 帧，帧形态断言落这里
//! ——记录在案的偏差，与 L10 计划验收条目对应）：
//!   ① WS chat.send（session_id=streamtest）→ testai-1.2（流式模型）；
//!   ② 监听同连接 push 帧（type=push/module=chat/cmd=provider_stream_event）；
//!   ③ 断言帧形态与相位序列：
//!      - 信封三字段齐全；data 含 session_id/model/seq/phase/text；
//!      - 首帧 phase=start（前端 reset 信号）；
//!      - seq 严格单调（0 起）；
//!      - 存在 end 帧且 elapsed_ms 在场，end 为末帧（error 才是另一终态）；
//!      - delta+end 文本拼起来非空（真实增量到达过）。
//!
//! `#[ignore]` 惯例（与 v_batch / wal_crash 一致），显式运行：
//!   cargo test -p nemesisbot-tests --test provider_stream -- --ignored
//! 前置：testaiserver.exe（testai-1.2 内置）+ target/release/nemesisbot.exe
//!（agents.provider_stream_events 默认 true，gateway 装配即挂）。
//!
//! 端口组 18099/49034/49035/18800（v_batch 六组 + Z1 + wal_crash 之外）。

use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use test_harness::{
    ManagedProcess, TestWorkspace, cleanup_ports, resolve_ai_server_bin, resolve_nemesisbot_bin,
    verify_gateway_web_identity, wait_for_http, ws_connect,
};
use tokio_tungstenite::tungstenite::Message;

const AI_PORT: u16 = 18099;
const WEB_PORT: u16 = 49034;
const WS_CHANNEL_PORT: u16 = 49035;
const HEALTH_PORT: u16 = 18800;

#[tokio::test]
#[ignore = "provider 流式事件真机 e2e：需 testaiserver.exe + target/release/nemesisbot.exe"]
async fn provider_stream_push_frame_shape_e2e() -> Result<()> {
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
    // testai-1.2 = 流式回复测试模型（TestAIServer 内置）。
    let base = format!("http://127.0.0.1:{AI_PORT}/v1");
    let out = ws
        .run_cli(
            &bin,
            &[
                "model",
                "add",
                "--model",
                "test/testai-1.2",
                "--base",
                &base,
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

    let cfg_path = ws.config_path();
    let mut cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path)?)?;
    cfg["channels"]["web"]["port"] = serde_json::json!(WEB_PORT);
    cfg["channels"]["websocket"]["port"] = serde_json::json!(WS_CHANNEL_PORT);
    cfg["gateway"]["port"] = serde_json::json!(HEALTH_PORT);
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)?)?;

    let mut gw = ManagedProcess::spawn("nemesisbot-gateway", &bin, &["--local", "gateway"], &root)?;
    wait_for_http(
        &format!("http://127.0.0.1:{WEB_PORT}/"),
        Duration::from_secs(60),
    )
    .await
    .context("gateway failed to start (web port)")?;
    // web 口 HTTP 200 ≠ 自己人（IT 幽灵网关事故同款暴露面）：核对 LISTEN 持有者 PID。
    verify_gateway_web_identity(WEB_PORT, gw.id())
        .map_err(|e| anyhow::anyhow!("gateway web identity check failed: {e}"))?;

    let result = run_stream_flow(&cfg).await;
    let _ = gw.kill().await;
    let _ = ai.kill().await;
    result
}

async fn run_stream_flow(cfg: &serde_json::Value) -> Result<()> {
    let token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut stream = ws_connect(WEB_PORT, &token)
        .await
        .context("WS connect to test gateway failed")?;
    let msg = serde_json::json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": { "content": "hello streaming", "session_id": "streamtest" },
    });
    stream
        .send(Message::Text(msg.to_string().into()))
        .await
        .context("chat.send failed")?;

    // 监听 push 帧：provider_stream_event 序列（delta 流 + 终答回复帧混在
    // 同一连接上，只取 cmd 命中的）。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let mut frames: Vec<serde_json::Value> = Vec::new();
    // 现场清单（失败诊断用）：收到的每个帧的 (type,module,cmd) 三元组。
    let mut seen_kinds: Vec<String> = Vec::new();
    // 现场清单：终答回复的 data 全文（回复文本直接指认来源路径——错误文案
    // vs 模型正常回复，一眼定性）。
    let mut reply_data: Option<serde_json::Value> = None;
    let mut saw_final_reply = false;
    while tokio::time::Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_secs(15), stream.next()).await;
        let Some(Ok(Message::Text(text))) = next.ok().flatten() else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        seen_kinds.push(format!(
            "{}/{}/{}",
            v["type"].as_str().unwrap_or("?"),
            v["module"].as_str().unwrap_or("?"),
            v["cmd"].as_str().unwrap_or("?")
        ));
        if v["type"] == "push" && v["cmd"] == "provider_stream_event" {
            frames.push(v["data"].clone());
        }
        // 错误帧 loud bail（v_batch 同款）：agent 装配/LLM 调用失败时不等满
        // 90s，直接带现场报错。
        if v["type"] == "system" && v["module"] == "error" {
            anyhow::bail!("gateway error frame: {v}");
        }
        // WS 三级协议信封：终答回复是 {type:"message", module:"chat",
        // cmd:"receive"}。注意 **role 判别必须带上**：用户消息回显也是
        // receive 帧（data.role="user"，seq 同一历史流水），只看信封会把
        // 回显当终答回复、提前 1s 断线（首跑实录：LLM 还在飞就退了）。
        if v["type"] == "message" && v["module"] == "chat" && v["cmd"] == "receive" {
            if v["data"]["role"].as_str() != Some("assistant") {
                continue;
            }
            // 终答回复已到：再宽限一小段时间收尾帧（end 可能与 receive
            // 竞态——sink.finish 先于出站投递，但帧到客户端有先后）。
            reply_data = Some(v["data"].clone());
            saw_final_reply = true;
            tokio::time::sleep(Duration::from_millis(800)).await;
            while let Ok(m) = tokio::time::timeout(Duration::from_millis(200), stream.next()).await
            {
                match m {
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(x) = serde_json::from_str::<serde_json::Value>(&t)
                            && x["type"] == "push"
                            && x["cmd"] == "provider_stream_event"
                        {
                            frames.push(x["data"].clone());
                        }
                    }
                    _ => break,
                }
            }
            break;
        }
    }
    anyhow::ensure!(
        saw_final_reply,
        "never received final reply within 90s — turn did not complete"
    );
    anyhow::ensure!(
        !frames.is_empty(),
        "no provider_stream_event push frames received — streaming lane not wired?\n\
         收到帧清单（type/module/cmd）：{seen_kinds:#?}\n\
         终答回复 data：{reply_data:#?}"
    );

    // —— 帧形态断言 ——
    // ① 信封 + data 字段
    for (i, f) in frames.iter().enumerate() {
        for key in ["session_id", "model", "seq", "phase", "text"] {
            anyhow::ensure!(f.get(key).is_some(), "frame #{i} missing data.{key}: {f}");
        }
        anyhow::ensure!(
            f["session_id"] == "streamtest",
            "frame #{i} session_id mismatch: {f}"
        );
        // model = wire 名（provider 前缀在路由期已剥，chat 调用拿到的是
        // 裸名——帧忠实记录 provider 实际被调用的模型名）。
        anyhow::ensure!(f["model"] == "testai-1.2", "frame #{i} model mismatch: {f}");
    }
    // ② 首帧 start
    anyhow::ensure!(
        frames[0]["phase"] == "start",
        "first frame must be phase=start, got: {}",
        frames[0]
    );
    // ③ seq 严格单调（0 起；同一 chat() 调用的 seq 空间——直接拿帧下标
    // 当期望值，enumerate 计数器即期望 seq，不另设变量）
    for (i, f) in frames.iter().enumerate() {
        let seq = f["seq"]
            .as_u64()
            .with_context(|| format!("frame #{i} seq not u64: {f}"))?;
        anyhow::ensure!(
            seq == i as u64,
            "frame #{i} seq {seq} != expected {i} (strictly monotonic from 0)"
        );
    }
    // ④ end 帧在场、带 elapsed、为末帧
    let last = frames.last().unwrap();
    anyhow::ensure!(
        last["phase"] == "end",
        "last frame must be phase=end (error is the only other terminal), got: {last}"
    );
    anyhow::ensure!(
        last.get("elapsed_ms").and_then(|v| v.as_u64()).is_some(),
        "end frame must carry elapsed_ms: {last}"
    );
    // ⑤ delta+end 文本拼起来非空
    let total_text: String = frames
        .iter()
        .filter(|f| f["phase"] == "delta" || f["phase"] == "end")
        .filter_map(|f| f["text"].as_str())
        .collect();
    anyhow::ensure!(
        !total_text.trim().is_empty(),
        "concatenated stream text is empty — no real delta ever arrived"
    );

    println!(
        "provider_stream e2e: {} frames, phases ok, total_text={} chars",
        frames.len(),
        total_text.chars().count()
    );
    Ok(())
}
