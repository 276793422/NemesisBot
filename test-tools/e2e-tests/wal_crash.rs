//! WAL 崩溃注入 e2e（L8，2026-10-13 三方学习 T3）。
//!
//! 真机全链验证 turn WAL 的「断头留痕 → 重启恢复 → 续轮闭合」：
//!   ① WS chat.send（显式 session_id=walcrash）→ testai-wal-1.0 下发
//!      sleep{"duration":20} 长工具调用；
//!   ② 轮询 wal.jsonl 到 `started`（确定性门：工具确已在飞，不睡固定时长）；
//!   ③ `ManagedProcess::kill()` 注入崩溃（TerminateProcess，等价
//!      taskkill /F——无清理钩子、tokio 任务直接蒸发）；
//!   ④ 断言断头现场：该 call_id 有 admitted+started、无 finished；
//!   ⑤ 重启 gateway（同 home 同端口）→ 恢复链：补写 interrupted 行 +
//!      合成续轮消息回原会话 → testai-wal-1.0 见「⟳ 系统续轮」标记 →
//!      WAL_RESUMED:OK；
//!   ⑥ 断言恢复闭合：wal.jsonl 出现 interrupted 行；续轮最终回复落盘
//!      session_logs/agent_main_session_walcrash.jsonl（assistant 行）。
//!      持久化是断言通道而非 WS 投递——恢复发生在新连接建立前，WS 帧没
//!      有订阅者，但 chat_log 落盘与出站投递无关（turn.rs 在 agent loop
//!      内追加，先于/独立于通道分发）。
//!
//! `#[ignore]` 惯例（与 v_batch 一致），显式运行：
//!   cargo test -p nemesisbot-tests --test wal_crash -- --ignored
//! 前置：testaiserver.exe 已含 testai-wal-1.0（go build）；
//!       target/release/nemesisbot.exe 已含 turn WAL（agents.turn_wal 默认
//!       true，gateway 装配即挂）。
//!
//! 端口组 18098/49030/49031/18799（v_batch 六组 + Z1 之外，避让用户在跑的
//! 49000/49001/18790）。

use anyhow::{Context, Result, bail};
use futures::SinkExt;
use std::path::Path;
use std::time::Duration;
use test_harness::{
    ManagedProcess, TestWorkspace, cleanup_ports, resolve_ai_server_bin, resolve_nemesisbot_bin,
    wait_for_http, ws_connect,
};
use tokio_tungstenite::tungstenite::Message;

const AI_PORT: u16 = 18098;
const WEB_PORT: u16 = 49030;
const WS_CHANNEL_PORT: u16 = 49031;
const HEALTH_PORT: u16 = 18799;

/// 轮询 wal.jsonl 直到某行满足 `pred`，返回该行 JSON。文件暂缺/半行照常
/// 重试（append 写入方逐行落，读侧可能读到尾部半行——解析失败行跳过）。
async fn wait_wal_line(
    wal: &Path,
    timeout_secs: u64,
    pred: impl Fn(&serde_json::Value) -> bool,
) -> Result<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if let Ok(body) = std::fs::read_to_string(wal) {
            for line in body.lines().filter(|l| !l.trim().is_empty()) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
                    && pred(&v)
                {
                    return Ok(v);
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "timeout ({timeout_secs}s) waiting for matching wal line in {}",
                wal.display()
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 轮询文件直到内容包含 `needle`（恢复链的最终落盘断言通道）。
async fn wait_file_contains(p: &Path, needle: &str, timeout_secs: u64) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if let Ok(body) = std::fs::read_to_string(p)
            && body.contains(needle)
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "timeout ({timeout_secs}s) waiting for {needle:?} in {}",
                p.display()
            );
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[tokio::test]
#[ignore = "WAL 真机崩溃注入：需重编后的 testaiserver.exe（含 testai-wal-1.0）+ target/release/nemesisbot.exe"]
async fn wal_crash_injection_interrupt_and_resume_e2e() -> Result<()> {
    let bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;
    cleanup_ports(&[AI_PORT, WEB_PORT, WS_CHANNEL_PORT, HEALTH_PORT]);

    let ws = TestWorkspace::new()?;
    let root = ws.path().to_path_buf();

    // TestAIServer 全程存活（重启后的续轮 LLM 调用也要打到它）。
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
    let base = format!("http://127.0.0.1:{AI_PORT}/v1");
    let out = ws
        .run_cli(
            &bin,
            &[
                "model",
                "add",
                "--model",
                "test/testai-wal-1.0",
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

    let wal_path = ws
        .workspace()
        .join("logs")
        .join("turn_wal")
        .join("wal.jsonl");
    let session_jsonl = ws
        .workspace()
        .join("logs")
        .join("session_logs")
        .join("agent_main_session_walcrash.jsonl");

    let result = run_crash_flow(&mut gw, &bin, &root, &cfg, &wal_path, &session_jsonl).await;
    let _ = ai.kill().await;
    result
}

async fn run_crash_flow(
    gw: &mut ManagedProcess,
    bin: &Path,
    root: &Path,
    cfg: &serde_json::Value,
    wal_path: &Path,
    session_jsonl: &Path,
) -> Result<()> {
    // —— ① 首轮：发起长工具调用（连接保持到崩溃注入之后）——
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
        "data": { "content": "start long task", "session_id": "walcrash" },
    });
    stream
        .send(Message::Text(msg.to_string().into()))
        .await
        .context("chat.send failed")?;

    // —— ② 确定性门：sleep 工具已进 started（WAL 两行已落盘）——
    let started = wait_wal_line(wal_path, 30, |v| {
        v["tool"] == "sleep" && v["state"] == "started"
    })
    .await
    .context("sleep tool never reached started state — turn WAL not attached?")?;
    let call_id = started["call_id"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(
        !call_id.is_empty(),
        "started record has empty call_id: {started}"
    );

    // —— ③ 崩溃注入：TerminateProcess（等价 taskkill /F，无清理钩子）——
    // kill().await 等进程真正退出，端口随即释放。
    let _ = gw.kill().await;
    drop(stream);
    tokio::time::sleep(Duration::from_millis(500)).await;

    // —— ④ 断头现场：admitted+started 已落盘，收尾态永远没来 ——
    let body = std::fs::read_to_string(wal_path)
        .context("wal.jsonl missing after crash — WAL not written at all")?;
    let recs: Vec<serde_json::Value> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let mine: Vec<&serde_json::Value> = recs
        .iter()
        .filter(|r| r["call_id"].as_str() == Some(call_id.as_str()))
        .collect();
    anyhow::ensure!(
        mine.iter().any(|r| r["state"] == "admitted"),
        "crashed call must have an admitted record: {mine:?}"
    );
    anyhow::ensure!(
        mine.iter().any(|r| r["state"] == "started"),
        "crashed call must have a started record: {mine:?}"
    );
    anyhow::ensure!(
        !mine
            .iter()
            .any(|r| r["state"] == "finished" || r["state"] == "interrupted"),
        "crashed call must have NO terminal record: {mine:?}"
    );

    // —— ⑤ 重启（同 home 同端口）：恢复链应在泵启动早期接管 ——
    let mut gw2 = ManagedProcess::spawn(
        "nemesisbot-gateway-restart",
        bin,
        &["--local", "gateway"],
        root,
    )?;
    let _ = wait_for_http(
        &format!("http://127.0.0.1:{WEB_PORT}/"),
        Duration::from_secs(60),
    )
    .await
    .context("restarted gateway failed to start");

    // interrupted 补写（恢复扫描 → append 收尾行）。
    wait_wal_line(wal_path, 30, |v| {
        v["call_id"].as_str() == Some(call_id.as_str()) && v["state"] == "interrupted"
    })
    .await
    .context("recovery did not append interrupted record for the crashed call")?;

    // —— ⑥ 续轮闭合：恢复消息回原会话 → LLM → WAL_RESUMED:OK 落盘 ——
    // 持久化是断言通道（见文件头）：assistant 行落 chat_log 与 WS 投递无关。
    wait_file_contains(session_jsonl, "WAL_RESUMED:OK", 90)
        .await
        .context("resumed turn reply never persisted to session chat_log")?;
    let sess = std::fs::read_to_string(session_jsonl)?;
    anyhow::ensure!(
        sess.lines()
            .any(|l| l.contains("\"assistant\"") && l.contains("WAL_RESUMED:OK")),
        "WAL_RESUMED:OK must appear in an assistant row: {sess}"
    );
    anyhow::ensure!(
        sess.lines()
            .any(|l| l.contains("⟳ 系统续轮") && l.contains("\"user\"")),
        "resume synthesis message must be recorded as the user row: {sess}"
    );

    let _ = gw2.kill().await;
    Ok(())
}
