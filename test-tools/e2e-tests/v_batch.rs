//! V 批真机 e2e（dsh-closure goal 第十批，2026-08-23）。
//!
//! V1 (B1) 大输出 prune/spill 两档：测试 home + 真实 gateway（--local，独立
//! 端口）+ 真实 WS 对话 + TestAIServer testai-8.0 脚本模型，断言：
//!   ① 8KB-64KB 档（exec 30032 字符）→ request_log 中发给模型的是
//!      head + 「结果过长已截断」标记 + tail，中段未发送；
//!   ② ≥64KB 档（exec 70032 字符）→ spill 文件落盘 `<home>/logs/spill/<session>/`
//!      且内容 = 完整工具结果；模型收到 locator；下一轮 read_file
//!      offset/limit 取回中段切片（request_log 断言精确切片内容）。
//!
//! V2 (B4) 记忆前置注入两态：测试 home enable enhanced_memory（真 ONNX 插件 +
//! all-MiniLM-L6-v2 模型）+ auto_inject=true → WSAPI（Dashboard 同款）
//! memory.entries.store 存一条记忆 → 相关提问断言 `# Memory Context` 块
//! 出现在 AI.Request.md + testai-9.0 报 MEM_INJECT_SEEN；不相关提问 →
//! 无注入块 + MEM_NO_INJECT。
//!
//! V3 (B6) 只读并发批：testai-2.1 单响应 3 个 web_fetch（/slow?secs=6|3|2）
//! → 墙钟 ≈ 6s（串行 11s+）；三个结果全部回灌收尾轮；安全审计事件按
//! 模型源序（6→3→2）落盘。pre-gateway 关 SSRF 层（loopback 会被拦）。
//!
//! V4 (B3) CC 委派真机（半边）：enable agents.claude_code_tool → testai-9.1
//! 发 claude_code 委派 → 真实 claude CLI 子进程在 cwd 建 cc_probe.txt →
//! 文件物证 + accept_edits 差分证明（非交互能写入 ⟹ --permission-mode
//! 真传到 CLI）+ 结果回灌 AI.Request.md。Codex 半边挂账。
//!
//! `#[ignore]` 惯例（与 e2e_ai_flow 一致），显式运行：
//!   cargo test -p nemesisbot-tests --test v_batch -- --ignored
//! 前置：testaiserver.exe 已含 testai-8.0/9.0/2.1（go build）；
//!       target/release/nemesisbot.exe 已含 read_file offset/limit + memory
//!       entries.store 走 live manager 的修复；
//!       V2 另需：target/e2e-cache/embedding/all-MiniLM-L6-v2/{model.onnx,
//!       tokenizer.json} + target/release/plugins/plugin_onnx.dll。
//!
//! 测试实例全走独立 home（TestWorkspace tempdir）+ 独立端口（V1 49010/49011/
//! 18791 + AI 18090；V2 49012/49013/18792 + AI 18091；V3 49014/49015/18793
//! + AI 18092，避开用户在跑的 gateway 49000/49001/18790）；进程 kill_on_drop
//!   自动清理。

use anyhow::{Context, Result, bail};
use futures::{SinkExt, StreamExt};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use test_harness::{
    ManagedProcess, TestWorkspace, cleanup_ports, resolve_ai_server_bin, resolve_nemesisbot_bin,
    resolve_project_root, wait_for_http, ws_connect,
};
use tokio_tungstenite::tungstenite::Message;

const AI_PORT: u16 = 18090;
const WEB_PORT: u16 = 49010;
const WS_CHANNEL_PORT: u16 = 49011;
const HEALTH_PORT: u16 = 18791;

const V2_AI_PORT: u16 = 18091;
const V2_WEB_PORT: u16 = 49012;
const V2_WS_CHANNEL_PORT: u16 = 49013;
const V2_HEALTH_PORT: u16 = 18792;

const V3_AI_PORT: u16 = 18092;
const V3_WEB_PORT: u16 = 49014;
const V3_WS_CHANNEL_PORT: u16 = 49015;
const V3_HEALTH_PORT: u16 = 18793;

const V4_AI_PORT: u16 = 18093;
const V4_WEB_PORT: u16 = 49016;
const V4_WS_CHANNEL_PORT: u16 = 49017;
const V4_HEALTH_PORT: u16 = 18794;

const V5_AI_PORT: u16 = 18094;
const V5_WEB_PORT: u16 = 49018;
const V5_WS_CHANNEL_PORT: u16 = 49019;
const V5_HEALTH_PORT: u16 = 18795;

// Separate port group for the STEER half of V5: cargo test runs the two
// #[tokio::test] fns in parallel and each setup calls cleanup_ports on its
// group — sharing one group let them kill each other's gateway (a flake
// that surfaced once the batch grew past 6 tests).
const V5S_AI_PORT: u16 = 18095;
const V5S_WEB_PORT: u16 = 49023;
const V5S_WS_CHANNEL_PORT: u16 = 49024;
const V5S_HEALTH_PORT: u16 = 18796;

/// 只发送一条聊天消息（不等回复）——busy/追发场景需要把「发」和「等」
/// 拆开：追发必须在 turn 1 还在跑的时候进队列，不能等 turn 1 的回复。
async fn ws_send_chat(stream: &mut test_harness::WsStream, content: &str) -> Result<()> {
    let msg = serde_json::json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": { "content": content },
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string()
    });
    stream.send(Message::Text(msg.to_string().into())).await?;
    Ok(())
}

/// 等待一条包含 `expect` 的最终回复（跳过中间空回复/进度/无关回复）。
async fn ws_wait_reply_containing(
    stream: &mut test_harness::WsStream,
    expect: &str,
    timeout_secs: u64,
) -> Result<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if v["type"] == "message" && v["module"] == "chat" && v["cmd"] == "receive" {
                    let c = v["data"]["content"].as_str().unwrap_or("");
                    if c.contains(expect) {
                        return Ok(c.to_string());
                    }
                    // 中间回复（空 / 进度）——继续等最终回复。
                    continue;
                }
                if v["type"] == "system" && v["module"] == "error" {
                    bail!(
                        "error response: {}",
                        v["data"]["content"].as_str().unwrap_or("?")
                    );
                }
            }
            Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => continue,
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => bail!("WebSocket error: {e}"),
            Ok(None) => bail!("WebSocket closed while waiting for {expect}"),
            Err(_) => bail!("Timeout ({timeout_secs}s) waiting for reply containing {expect}"),
        }
    }
}

/// 发送聊天消息并等待包含 `expect` 的最终回复。跳过中间空回复 / 进度消息
/// （agent 轮次的中间产物），只有含期望 marker 的 chat receive 才返回。
async fn ws_chat_until(
    stream: &mut test_harness::WsStream,
    content: &str,
    expect: &str,
    timeout_secs: u64,
) -> Result<String> {
    ws_send_chat(stream, content).await?;
    ws_wait_reply_containing(stream, expect, timeout_secs).await
}

/// 收集 `<home>/workspace/logs/request_logs/` 下全部 request .md（按文件名排序），
/// 返回 (文件名, 内容) 列表。detail_level=Full 时这些文件含发给模型的完整
/// messages —— 断言「模型实际看到什么」的真相源。
fn collect_request_mds(home: &Path) -> Vec<(String, String)> {
    let mut out = vec![];
    let base = home.join("workspace").join("logs").join("request_logs");
    let Ok(sessions) = std::fs::read_dir(&base) else {
        return out;
    };
    for sess in sessions.flatten() {
        let p = sess.path();
        if !p.is_dir() {
            continue;
        }
        if let Ok(files) = std::fs::read_dir(&p) {
            for f in files.flatten() {
                let fp = f.path();
                if fp.extension().and_then(|e| e.to_str()) == Some("md")
                    && let Ok(c) = std::fs::read_to_string(&fp)
                {
                    out.push((f.file_name().to_string_lossy().to_string(), c));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// 模型可见的 request log 文件（发给 LLM 的 messages 真相源）。
/// `NN.AI.Request.md` = 该轮完整 messages；`NN.request.md` = 用户请求。
/// `NN.Local.md` 是本地工具执行观测日志（记录工具原始 result，镜像 Go 行为），
/// 不发给模型，不参与「模型看到了什么」的断言。
fn is_model_facing(name: &str) -> bool {
    name.ends_with(".AI.Request.md") || name.ends_with(".request.md")
}

/// 取 `s` 中 `start` 与其后第一个 `end` 之间的子串。
fn extract_between<'a>(s: &'a str, start: &str, end: &str) -> &'a str {
    let Some(i) = s.find(start) else { return "" };
    let rest = &s[i + start.len()..];
    match rest.find(end) {
        Some(j) => &rest[..j],
        None => rest,
    }
}

/// 枚举 `<home>/logs/spill/<session>/` 下的 spill 文件。
fn collect_spill_files(home: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let Ok(sessions) = std::fs::read_dir(home.join("logs").join("spill")) else {
        return out;
    };
    for sess in sessions.flatten() {
        if let Ok(files) = std::fs::read_dir(sess.path()) {
            for f in files.flatten() {
                if f.path().extension().and_then(|e| e.to_str()) == Some("txt") {
                    out.push(f.path());
                }
            }
        }
    }
    out.sort();
    out
}

/// 起一套独立测试环境：TestWorkspace + onboard + model add + 端口改写 +
/// TestAIServer + gateway。返回 (workspace, ai 进程, gateway 进程, config)。
///
/// `pre_gateway` 在 config.json 端口改写之后、gateway 拉起之前执行——
/// 需要在启动前生效的配置/文件（如 V2 的 enhanced memory）在钩子里做。
async fn setup_test_home_ports<F>(
    model: &str,
    ai_port: u16,
    web_port: u16,
    ws_channel_port: u16,
    health_port: u16,
    pre_gateway: F,
) -> Result<(
    TestWorkspace,
    ManagedProcess,
    ManagedProcess,
    serde_json::Value,
)>
where
    F: FnOnce(&TestWorkspace) -> Result<()>,
{
    let bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;
    // 端口防冲突（独立于用户在跑的 gateway 49000/49001/18790）。
    cleanup_ports(&[ai_port, web_port, ws_channel_port, health_port]);

    let ws = TestWorkspace::new()?;
    let root = ws.path().to_path_buf();

    // TestAIServer（cwd 指向临时目录，日志不落仓库）。
    let ai = ManagedProcess::spawn(
        "testaiserver",
        &ai_bin,
        &["--port", &ai_port.to_string()],
        &root,
    )?;
    wait_for_http(
        &format!("http://127.0.0.1:{ai_port}/health"),
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

    let base = format!("http://127.0.0.1:{ai_port}/v1");
    let out = ws
        .run_cli(
            &bin,
            &[
                "model",
                "add",
                "--model",
                model,
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

    // 改写端口：web / websocket / gateway(health) 全部避开生产实例。
    let cfg_path = ws.config_path();
    let mut cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path)?)?;
    cfg["channels"]["web"]["port"] = serde_json::json!(web_port);
    cfg["channels"]["websocket"]["port"] = serde_json::json!(ws_channel_port);
    cfg["gateway"]["port"] = serde_json::json!(health_port);
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)?)?;

    // 启动前钩子（V2 在这里 enable enhanced memory + 落模型文件）。
    pre_gateway(&ws)?;

    // gateway（--local：home = <temp>/.nemesisbot；cwd 必须在 temp 下）。
    let gw = ManagedProcess::spawn("nemesisbot-gateway", &bin, &["--local", "gateway"], &root)?;
    wait_for_http(
        &format!("http://127.0.0.1:{web_port}/"),
        Duration::from_secs(60),
    )
    .await
    .context("gateway failed to start (web port)")?;

    Ok((ws, ai, gw, cfg))
}

/// V1 入口：默认端口组，无启动前钩子。
async fn setup_test_home(
    model: &str,
) -> Result<(
    TestWorkspace,
    ManagedProcess,
    ManagedProcess,
    serde_json::Value,
)> {
    setup_test_home_ports(
        model,
        AI_PORT,
        WEB_PORT,
        WS_CHANNEL_PORT,
        HEALTH_PORT,
        |_| Ok(()),
    )
    .await
}

#[tokio::test]
#[ignore = "V1 真机：需重编后的 testaiserver.exe（含 testai-8.0）+ target/release/nemesisbot.exe"]
async fn v1_big_output_prune_and_spill_e2e() -> Result<()> {
    let (ws, mut ai, mut gw, cfg) = setup_test_home("test/testai-8.0").await?;
    let result = run_v1(&ws, &cfg).await;
    // 无论断言成败都先收尾进程，保证端口释放（Drop 也会兜底）。
    let _ = gw.kill().await;
    let _ = ai.kill().await;
    result
}

async fn run_v1(ws: &TestWorkspace, cfg: &serde_json::Value) -> Result<()> {
    // —— fixture：两档大文件，head/tail marker 夹位置唯一填充块 ——
    // 填充块 = format!("{:09}|", i)（10 字符/块）：每块内容编码自身位置，
    // 「中段没发给模型」探针才不会误命中 head/tail 窗口里的相同字面量
    // （首轮版本用 0123456789 循环填充，任意窗口只有 10 种字面量，探针
    // 假阳性 —— 是断言的 bug，不是产品的 bug）。
    let head = "HEAD_MARKER_START";
    let tail = "TAIL_MARKER_END";
    let blocks = |n: usize| -> String {
        (0..n)
            .map(|i| format!("{:09}|", i))
            .collect::<Vec<_>>()
            .join("")
    };
    let prune_body = format!("{head}{}{tail}", blocks(3000)); // 17+30000+15=30032 字符（8KB-64KB 档）
    let spill_body = format!("{head}{}{tail}", blocks(7000)); // 17+70000+15=70032 字符（≥64KB 档）
    std::fs::write(ws.workspace().join("big_prune.txt"), &prune_body)?;
    std::fs::write(ws.workspace().join("big_spill.txt"), &spill_body)?;

    // read-back 精确切片期望（与 testai-8.0 的 offset=30000/limit=500 对齐）。
    let expected_slice: String = spill_body.chars().skip(30000).take(500).collect();
    // 「中段没发给模型」探针：位于 head 3600 / tail 3600 窗口之外。
    let prune_middle_probe: String = prune_body.chars().skip(15000).take(100).collect();
    let spill_middle_probe: String = spill_body.chars().skip(30000).take(100).collect();

    let token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut stream = ws_connect(WEB_PORT, &token)
        .await
        .context("WS connect to test gateway failed")?;

    // ============ ① prune 档（8KB-64KB）============
    let reply = ws_chat_until(
        &mut stream,
        r#"<BIG_OUT>{"command":"type big_prune.txt"}</BIG_OUT>"#,
        "PRUNE_SEEN",
        120,
    )
    .await?;
    assert!(
        reply.contains("PRUNE_SEEN"),
        "model should terminate with PRUNE_SEEN after seeing the prune marker, got: {reply}"
    );

    let mds = collect_request_mds(&ws.home());
    let (prune_file, prune_req) = mds
        .iter()
        .find(|(n, c)| is_model_facing(n) && c.contains("结果过长已截断"))
        .context("no request log contains the prune marker — pruning not applied?")?
        .clone();
    assert!(
        prune_req.contains(head),
        "{prune_file}: head marker must be sent to the model"
    );
    assert!(
        prune_req.contains(tail),
        "{prune_file}: tail marker must be sent to the model"
    );
    assert!(
        !prune_req.contains(&prune_middle_probe),
        "{prune_file}: middle of the pruned result must NOT be sent to the model"
    );
    for (name, content) in &mds {
        if !is_model_facing(name) {
            continue; // NN.Local.md 记录工具原始 result（观测日志），模型不可见
        }
        assert!(
            !content.contains(&prune_body),
            "{name}: full 30032-char body must never appear in a model-facing request log"
        );
    }

    // ============ ② spill 档（≥64KB）============
    let reply = ws_chat_until(
        &mut stream,
        r#"<BIG_OUT>{"command":"type big_spill.txt"}</BIG_OUT>"#,
        "READBACK_OK",
        120,
    )
    .await?;
    assert!(
        reply.contains("READBACK_OK"),
        "model should terminate with READBACK_OK after the segmented read-back, got: {reply}"
    );

    // spill 文件落盘且内容 = 完整工具结果。
    let spill_files = collect_spill_files(&ws.home());
    assert!(
        !spill_files.is_empty(),
        "no spill file under {} — spill not applied?",
        ws.home().join("logs").join("spill").display()
    );
    let spilled = std::fs::read_to_string(&spill_files[0])?;
    assert_eq!(
        spilled, spill_body,
        "spill file must hold the FULL 70032-char tool result"
    );

    // 模型收到 locator（preview + 路径），且中段未内联。
    let mds = collect_request_mds(&ws.home());
    let (spill_file, spill_req) = mds
        .iter()
        .find(|(n, c)| is_model_facing(n) && c.contains("已完整保存到："))
        .context("no request log contains the spill locator")?
        .clone();
    assert!(
        spill_req.contains(head),
        "{spill_file}: preview (first 2000 chars, contains head marker) must be sent"
    );
    assert!(
        !spill_req.contains(&spill_middle_probe),
        "{spill_file}: middle must NOT be inlined (only preview + locator)"
    );
    let loc = extract_between(&spill_req, "已完整保存到：", "。").trim();
    assert!(
        !loc.is_empty() && Path::new(loc).exists(),
        "locator path must point at a real spill file: {loc:?}"
    );

    // 下一轮 read_file offset/limit 分段取回：request log 断言精确中段切片。
    let (seg_file, seg_req) = mds
        .iter()
        .find(|(n, c)| is_model_facing(n) && c.contains("[read_file 分段]"))
        .context("no request log contains the segmented read_file result")?
        .clone();
    assert!(
        seg_req.contains("offset=30000"),
        "{seg_file}: segment header must state offset=30000"
    );
    assert!(
        seg_req.contains("chars_returned=500"),
        "{seg_file}: segment header must state chars_returned=500"
    );
    assert!(
        seg_req.contains(&expected_slice),
        "{seg_file}: the exact middle slice must be visible to the model"
    );

    Ok(())
}

// ===========================================================================
// V2 (B4) 记忆前置注入真机 —— 两态断言
// ===========================================================================

/// 与 V2 校准测试（crates/nemesis-memory/tests/real_plugin.rs
/// it_onnx_auto_inject_similarity_calibration）严格同源的三个字符串：
/// related 对 cosine=0.7198（过 VectorStore 0.7 查询阈值），
/// unrelated 对 0.0765（远低于注入线 0.35）。换措辞必须同步重校准。
const MEM_STORED: &str = "My favorite project codename is FALCON-77.";
const MEM_RELATED_QUERY: &str = "What is my favorite project codename? <MEM_CHECK>";
const MEM_UNRELATED_QUERY: &str = "What is the capital of France? <MEM_CHECK>";

/// 列出 `<home>/workspace/logs/request_logs/` 下全部 `NN.AI.Request.md` 的
/// (绝对路径, 内容)——「模型实际看到什么」的真相源。按绝对路径 key，
/// round 间 before/after 差分用（文件名跨会话目录可能重名）。
fn collect_ai_request_files(home: &Path) -> Vec<(PathBuf, String)> {
    let mut out = vec![];
    let base = home.join("workspace").join("logs").join("request_logs");
    let Ok(sessions) = std::fs::read_dir(&base) else {
        return out;
    };
    for sess in sessions.flatten() {
        if !sess.path().is_dir() {
            continue;
        }
        if let Ok(files) = std::fs::read_dir(sess.path()) {
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                if name.ends_with(".AI.Request.md")
                    && let Ok(c) = std::fs::read_to_string(f.path())
                {
                    out.push((f.path(), c));
                }
            }
        }
    }
    out.sort();
    out
}

/// 发送 WSAPI 请求（type=request + reqId）并等待匹配的 response。
/// 注意与 chat 消息（type=message）区分：WSAPI 命令走 request/response
/// 通道（useWSAPI.ts 同款协议）。返回 `data` 字段；`error` 非空则 bail。
async fn ws_api_call(
    stream: &mut test_harness::WsStream,
    module: &str,
    cmd: &str,
    req_id: &str,
    data: serde_json::Value,
    timeout_secs: u64,
) -> Result<serde_json::Value> {
    let msg = serde_json::json!({
        "type": "request",
        "module": module,
        "cmd": cmd,
        "reqId": req_id,
        "data": data,
    });
    stream.send(Message::Text(msg.to_string().into())).await?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if v["type"] == "response" && v["reqId"] == req_id {
                    if let Some(e) = v["error"].as_str() {
                        bail!("{module}.{cmd} failed: {e}");
                    }
                    return Ok(v["data"].clone());
                }
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => bail!("WebSocket error: {e}"),
            Ok(None) => bail!("WebSocket closed waiting for response {req_id}"),
            Err(_) => bail!("Timeout ({timeout_secs}s) waiting for response {req_id}"),
        }
    }
}

/// V2 启动前钩子：enable enhanced memory（主开关 + 子开关 + auto_inject）
/// + 落模型文件。全部在 gateway 拉起之前生效——向量库在启动时初始化，
///   失败会被 with_config 反手写 enabled=false 永久禁用。
fn v2_pre_gateway(ws: &TestWorkspace) -> Result<()> {
    let root = resolve_project_root()?;

    // ① 前置：模型缓存（一次性下载）+ ONNX 插件（gateway 以
    //    target/release/nemesisbot.exe 运行，插件按 {exe_dir}/plugins/ 解析）。
    let cache = root
        .join("target")
        .join("e2e-cache")
        .join("embedding")
        .join("all-MiniLM-L6-v2");
    for f in ["model.onnx", "tokenizer.json"] {
        let p = cache.join(f);
        anyhow::ensure!(
            p.exists(),
            "V2 前置缺失：{}（先下载到该路径：\n  \
             curl -L -o <path>/model.onnx https://hf-mirror.com/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx\n  \
             curl -L -o <path>/tokenizer.json https://hf-mirror.com/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json）",
            p.display()
        );
    }
    anyhow::ensure!(
        root.join("target")
            .join("release")
            .join("plugins")
            .join("plugin_onnx.dll")
            .exists(),
        "V2 前置缺失：target/release/plugins/plugin_onnx.dll（从 bin/bin_windows/plugins/ 拷贝）"
    );

    // ② config.json 主开关：memory.enabled = true（agent_factory 挂
    //    memory_manager 的前提；false 时 auto-inject 拿不到 manager）。
    let cfg_path = ws.config_path();
    let mut cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path)?)?;
    cfg["memory"]["enabled"] = serde_json::json!(true);
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)?)?;

    // ③ config.enhanced_memory.json：enabled + active=medium + auto_inject。
    //    仓库模板打底（含三档 models 定义），仅改需要的字段——模板缺
    //    auto_inject 键，json 索引赋值补上。
    let em_dir = ws.home().join("workspace").join("config");
    std::fs::create_dir_all(&em_dir)?;
    let template = std::fs::read_to_string(
        root.join("nemesisbot")
            .join("config")
            .join("config.enhanced_memory.default.json"),
    )?;
    let mut em: serde_json::Value = serde_json::from_str(&template)?;
    em["enabled"] = serde_json::json!(true);
    em["active"] = serde_json::json!("medium");
    em["auto_inject"] = serde_json::json!(true);
    em["auto_inject_top_k"] = serde_json::json!(3);
    std::fs::write(
        em_dir.join("config.enhanced_memory.json"),
        serde_json::to_string_pretty(&em)?,
    )?;

    // ④ 模型文件落位：resolve 的数据目录档
    //    `<workspace>/tools/memory/data/embedding/<model_name>/`。
    let dst = ws
        .workspace()
        .join("tools")
        .join("memory")
        .join("data")
        .join("embedding")
        .join("all-MiniLM-L6-v2");
    std::fs::create_dir_all(&dst)?;
    for f in ["model.onnx", "tokenizer.json"] {
        std::fs::copy(cache.join(f), dst.join(f))
            .with_context(|| format!("copying model file {f}"))?;
    }

    Ok(())
}

#[tokio::test]
#[ignore = "V2 真机：需 e2e-cache 模型（target/e2e-cache/embedding/all-MiniLM-L6-v2）+ target/release/plugins/plugin_onnx.dll + 重编后的 nemesisbot.exe（含 entries.store 走 live manager 的修复）"]
async fn v2_memory_auto_inject_e2e() -> Result<()> {
    let (ws, mut ai, mut gw, cfg) = setup_test_home_ports(
        "test/testai-9.0",
        V2_AI_PORT,
        V2_WEB_PORT,
        V2_WS_CHANNEL_PORT,
        V2_HEALTH_PORT,
        v2_pre_gateway,
    )
    .await?;
    let result = run_v2(&ws, &cfg, V2_WEB_PORT).await;
    // 无论断言成败都先收尾进程，保证端口释放（Drop 也会兜底）。
    let _ = gw.kill().await;
    let _ = ai.kill().await;
    result
}

async fn run_v2(ws: &TestWorkspace, cfg: &serde_json::Value, web_port: u16) -> Result<()> {
    let token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut stream = ws_connect(web_port, &token)
        .await
        .context("WS connect to test gateway failed")?;

    // ============ ① 真实 store：Dashboard 同款 WSAPI ============
    // 修复后 entries.store 走 live MemoryManager（store_entry：真 ONNX
    // embed + 进内存索引 + manager 自己的路径持久化）。修复前它裸写
    // <workspace>/memory/vector/ —— manager 永远不会加载的那棵树，
    // Dashboard 存的记忆 agent 搜索/注入永远看不见。
    let resp = ws_api_call(
        &mut stream,
        "memory",
        "entries.store",
        "v2-store-1",
        serde_json::json!({ "content": MEM_STORED }),
        60,
    )
    .await?;
    assert_eq!(
        resp["stored"],
        serde_json::json!(true),
        "entries.store must report stored=true, got: {resp}"
    );

    // manager 侧持久化断言（真 embed 路径的副产品：vector adapter 落盘）。
    // 注入成立即证明语义检索真跑通——keyword 回退路径 score=0.0 < 0.35
    // 注入线，结构上注入不了。
    let jsonl = ws
        .home()
        .join("workspace")
        .join("memory_vector")
        .join("vector")
        .join("vector_store.jsonl");
    let persisted = std::fs::read_to_string(&jsonl)
        .with_context(|| format!("manager JSONL missing: {}", jsonl.display()))?;
    assert!(
        persisted.contains("FALCON-77"),
        "persisted vector_store.jsonl must contain the stored content"
    );
    let legacy = ws
        .workspace()
        .join("memory")
        .join("vector")
        .join("vector_store.jsonl");
    assert!(
        !legacy.exists(),
        "pre-fix legacy path must NOT be written anymore: {}",
        legacy.display()
    );

    // ============ ② 相关提问 → 注入 ============
    // 快照是 build_messages 投影（ephemeral，不进 history 不落盘），所以
    // round A 断言看本轮新生成的 AI.Request.md；testai-9.0 全 messages 扫
    // "# Memory Context"（若有任何一处注入都会报 MEM_INJECT_SEEN）。
    let before_a: HashSet<PathBuf> = collect_ai_request_files(&ws.home())
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    let reply = ws_chat_until(&mut stream, MEM_RELATED_QUERY, "MEM_", 120).await?;
    assert!(
        reply.contains("MEM_INJECT_SEEN"),
        "related query must trigger memory injection, got: {reply}"
    );

    let new_a: Vec<(PathBuf, String)> = collect_ai_request_files(&ws.home())
        .into_iter()
        .filter(|(p, _)| !before_a.contains(p))
        .collect();
    assert!(
        !new_a.is_empty(),
        "round A must produce at least one AI.Request.md"
    );
    assert!(
        new_a
            .iter()
            .any(|(_, c)| c.contains("# Memory Context") && c.contains("FALCON-77")),
        "round-A AI.Request.md must contain the # Memory Context block with the stored content"
    );

    // ============ ③ 不相关提问 → 无注入 ============
    let before_b: HashSet<PathBuf> = collect_ai_request_files(&ws.home())
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    let reply = ws_chat_until(&mut stream, MEM_UNRELATED_QUERY, "MEM_", 120).await?;
    assert!(
        reply.contains("MEM_NO_INJECT"),
        "unrelated query must NOT trigger injection, got: {reply}"
    );

    let new_b: Vec<(PathBuf, String)> = collect_ai_request_files(&ws.home())
        .into_iter()
        .filter(|(p, _)| !before_b.contains(p))
        .collect();
    assert!(
        !new_b.is_empty(),
        "round B must produce at least one AI.Request.md"
    );
    for (p, c) in &new_b {
        assert!(
            !c.contains("# Memory Context"),
            "round-B request {} must NOT contain the memory block",
            p.display()
        );
    }

    Ok(())
}

// ===========================================================================
// V3 (B6) 只读并发真机 —— 墙钟 + 安全审计顺序
// ===========================================================================

/// V3 启动前钩子：关 SSRF 层。web_fetch 打 `127.0.0.1:<AI>/slow` 会被
/// SSRF guard 的 loopback 拦截（ssrf.rs `block_localhost` 默认 true）——
/// 本测试的目标是并发执行语义，不是 SSRF；其余安全层（含审计 ⑧）照跑。
fn v3_pre_gateway(ws: &TestWorkspace) -> Result<()> {
    let path = ws
        .home()
        .join("workspace")
        .join("config")
        .join("config.security.json");
    let mut cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    cfg["layers"]["ssrf"] = serde_json::json!({ "enabled": false });
    std::fs::write(&path, serde_json::to_string_pretty(&cfg)?)?;
    Ok(())
}

/// 从 `<home>/workspace/logs/security_logs/security_audit_*.log` 提取
/// `/slow?secs=N` 审计事件的 N 序列（按文件内 append 顺序）。管道分隔
/// 格式第 7 列是 TARGET；这里只按行内容匹配，不依赖列位。
/// 一次 pipeline execute 会落多条事件（如 DLP 观测 allowed + 终审
/// allowed/denied），故对相邻重复收敛——跨调用交错（6,3,6…）不会被
/// 收敛掉，仍会触发断言失败（那是真正的乱序）。
fn security_audit_slow_order(home: &Path) -> Vec<u32> {
    let mut raw = vec![];
    let base = home.join("workspace").join("logs").join("security_logs");
    let Ok(files) = std::fs::read_dir(&base) else {
        return raw;
    };
    let mut names: Vec<PathBuf> = files
        .flatten()
        .map(|f| f.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("security_audit_") && n.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();
    names.sort();
    for f in names {
        if let Ok(c) = std::fs::read_to_string(&f) {
            for line in c.lines() {
                if let Some(i) = line.find("/slow?secs=") {
                    let rest = &line[i + "/slow?secs=".len()..];
                    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = digits.parse::<u32>() {
                        raw.push(n);
                    }
                }
            }
        }
    }
    let mut out: Vec<u32> = Vec::with_capacity(raw.len());
    for n in raw {
        if out.last() != Some(&n) {
            out.push(n);
        }
    }
    out
}

#[tokio::test]
#[ignore = "V3 真机：需重编后的 testaiserver.exe（含 testai-2.1 + /slow 端点）+ target/release/nemesisbot.exe"]
async fn v3_readonly_concurrent_batch_e2e() -> Result<()> {
    let (ws, mut ai, mut gw, cfg) = setup_test_home_ports(
        "test/testai-2.1",
        V3_AI_PORT,
        V3_WEB_PORT,
        V3_WS_CHANNEL_PORT,
        V3_HEALTH_PORT,
        v3_pre_gateway,
    )
    .await?;
    let result = run_v3(&ws, &cfg, V3_AI_PORT, V3_WEB_PORT).await;
    // 无论断言成败都先收尾进程，保证端口释放（Drop 也会兜底）。
    let _ = gw.kill().await;
    let _ = ai.kill().await;
    result
}

async fn run_v3(
    ws: &TestWorkspace,
    cfg: &serde_json::Value,
    ai_port: u16,
    web_port: u16,
) -> Result<()> {
    let token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut stream = ws_connect(web_port, &token)
        .await
        .context("WS connect to test gateway failed")?;

    // 一条消息 → testai-2.1 单响应发 3 个 web_fetch（/slow?secs=6|3|2）→
    // 工具结果回灌后收尾轮报 PARALLEL_DONE。
    let content = format!("<PARALLEL>http://127.0.0.1:{ai_port}</PARALLEL>");
    let t0 = std::time::Instant::now();
    let _reply = ws_chat_until(&mut stream, &content, "PARALLEL_DONE", 120).await?;
    let elapsed = t0.elapsed();

    // ① 墙钟断言：并发 ≈ 最慢一个（6s）+ 2 次本地 LLM 往返 ≈ 7s；
    //    串行 = 6+3+2 = 11s+。上界 9.5s 与两者都有安全裕度。
    //    下界 5s 防「全失败秒回」的空洞通过（全失败时秒表 + 结果断言会抓）。
    assert!(
        elapsed >= Duration::from_secs(5),
        "slowest /slow?secs=6 must actually have slept — elapsed only {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(9500),
        "3-call batch must run concurrently (≈6s wall), not serially (≥11s) — elapsed {elapsed:?}"
    );

    // ② 三个结果都真实回灌：收尾轮（tool 结果之后的 LLM 调用）的
    //    AI.Request.md 必须含全部三个 /slow 响应体。
    let files = collect_ai_request_files(&ws.home());
    let final_req = files
        .iter()
        .map(|(_, c)| c.as_str())
        .find(|c| c.contains("slept 6 seconds"))
        .context(
            "no AI.Request.md contains the /slow?secs=6 result — batch results not fed back?",
        )?;
    for secs in [6u32, 3, 2] {
        assert!(
            final_req.contains(&format!("slept {secs} seconds")),
            "final round request must contain all three /slow results (missing secs={secs})"
        );
    }

    // ③ 安全审计顺序 = 模型源序：join_all 保序 + 串行 guard 回放按源序
    //    （loop.rs precompute_readonly_batch 注释的硬约束），落盘的审计
    //    事件序列应为 6 → 3 → 2（到达顺序实证——若这里翻车即为真 bug）。
    let order = security_audit_slow_order(&ws.home());
    assert_eq!(
        order,
        vec![6, 3, 2],
        "security audit events for the /slow batch must appear in model source order"
    );

    Ok(())
}

// ===========================================================================
// V4 (B3) Claude Code 委派真机（半边；Codex 挂账）
// ===========================================================================

/// V4 启动前钩子：enable `agents.claude_code_tool`（默认关 + PATH 探测注册；
/// claude CLI 已在 PATH 才会真注册）。
fn v4_pre_gateway(ws: &TestWorkspace) -> Result<()> {
    let cfg_path = ws.config_path();
    let mut cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path)?)?;
    cfg["agents"]["claude_code_tool"]["enabled"] = serde_json::json!(true);
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)?)?;
    Ok(())
}

/// 在 `root` 下递归找名为 `name` 的文件（tempdir 树很小）。CC 的 cwd =
/// gateway 进程 cwd（delegation_cwd 对非路径 session_key 的回退），即
/// tempdir 根——但子代理若自作主张换目录也兜得住。
fn find_file_recursive(root: &Path, name: &str) -> Option<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return None;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(hit) = find_file_recursive(&p, name) {
                return Some(hit);
            }
        } else if p.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(p);
        }
    }
    None
}

#[tokio::test]
#[ignore = "V4 真机：需 PATH 上的 claude CLI（真实子进程跑一次小任务）+ 重编后的 testaiserver.exe（含 testai-9.1）+ target/release/nemesisbot.exe"]
async fn v4_claude_code_delegation_e2e() -> Result<()> {
    let (ws, mut ai, mut gw, cfg) = setup_test_home_ports(
        "test/testai-9.1",
        V4_AI_PORT,
        V4_WEB_PORT,
        V4_WS_CHANNEL_PORT,
        V4_HEALTH_PORT,
        v4_pre_gateway,
    )
    .await?;
    let result = run_v4(&ws, &cfg, V4_WEB_PORT).await;
    // 无论断言成败都先收尾进程，保证端口释放（Drop 也会兜底）。
    let _ = gw.kill().await;
    let _ = ai.kill().await;
    result
}

async fn run_v4(ws: &TestWorkspace, cfg: &serde_json::Value, web_port: u16) -> Result<()> {
    let token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut stream = ws_connect(web_port, &token)
        .await
        .context("WS connect to test gateway failed")?;

    // 触发：testai-9.1 发 claude_code 委派（prompt = 在 cwd 建 cc_probe.txt
    // 内容 CC_SUBTASK_OK，回 DONE）→ 真实 claude CLI 子进程执行 → 结果
    // 回灌 → 模型报 CC_DELEGATION_SUCCESS/FAILED。CC 真跑含模型往返，
    // 给足 360s（工具自身 300s 超时兜底）。
    let reply = ws_chat_until(&mut stream, "<CC_DELEGATE>", "CC_DELEGATION_", 360).await?;
    assert!(
        reply.contains("CC_DELEGATION_SUCCESS"),
        "claude_code delegation must succeed end-to-end, got: {reply}"
    );

    // ① 真跑的物证：CC 真创建了文件且内容精确。
    let probe = find_file_recursive(ws.path(), "cc_probe.txt").context(
        "cc_probe.txt not found anywhere under the test workspace — CC did not really execute",
    )?;
    let content = std::fs::read_to_string(&probe)?;
    assert_eq!(
        content.trim(),
        "CC_SUBTASK_OK",
        "cc_probe.txt content must be exactly CC_SUBTASK_OK (at {})",
        probe.display()
    );

    // ② T5 权限档真形态的差分证明：默认档 accept_edits 下 CC 能在
    //    非交互 print 模式里完成文件写入（edits 自动接受）。若
    //    --permission-mode 没真传给 CLI，claude 的默认档在 print 模式
    //    会因权限提示非交互拒绝而建不出文件——①的文件成立即证明该
    //    参数真的到达了子进程。参数向量的构造另有 mock 单测对齐。
    // ③ 回灌主对话：收尾轮的 AI.Request.md 含 claude_code 工具调用
    //    （messages 里的 tool_call 段）。
    let files = collect_ai_request_files(&ws.home());
    assert!(
        files.iter().any(|(_, c)| c.contains("claude_code")),
        "no AI.Request.md contains the claude_code tool call — result not fed back into the conversation?"
    );

    Ok(())
}

// ===========================================================================
// V5 (B5) inbox/steer 端到端（web/WS 通道；真 IM 无凭据，挂账不变）
// ===========================================================================
//
// 前置架构事实（V5 的真 bug 与修复）：生产 pump（run_bus_arc，由
// AgentLoopServiceAdapter 拉起）是串行消费者——process_inbound_message
// 阻塞到整个轮次结束，消息 2 在 mpsc 里等到 session 已释放才被处理，
// U7 inbox 的 busy 分支对用户消息【不可达】（Go 1:1 原版同样串行，非
// 移植回归；只有单测手动占 session 才走到过）。修复 = gate-in-pump +
// spawn 轮次任务（仅 Queue/Steer 模式；Reject 默认路径字节不变）：
// gate 同步跑 routing+busy 判定，admitted 轮次起独立 task，busy 的追发
// 真正进 inbox 排队。这两个测试就是该修复的真机验收。
//
// 模型 testai-9.2：`<B5_BUSY>http://host:port</B5_BUSY>` → 单 web_fetch
// /slow?secs=8（8 秒工具轮制造 busy）；工具结果后的 LLM 调用按最新
// user 消息报 B5_TURN1_DONE / B5_STEER_WITNESSED / B5_QUEUED_TURN2_OK。

/// V5 启动前钩子：`agents.defaults.concurrent_request_mode = <mode>` +
/// 关 SSRF 层（queue / steer 两个用例共用）。
/// SSRF 必须关（V3 同款教训）：busy 轮靠 web_fetch 打
/// `127.0.0.1:<AI>/slow?secs=8` 制造 8 秒工具轮，SSRF guard 的 loopback
/// 拦截会让工具秒失败 → turn 1 毫秒级结束 → session 从未 busy →
/// 追发直接当新轮处理，busy 分支永远不触发（真机已实证一次）。
fn v5_pre_gateway(mode: &'static str) -> impl FnOnce(&TestWorkspace) -> Result<()> {
    move |ws: &TestWorkspace| {
        let cfg_path = ws.config_path();
        let mut cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path)?)?;
        cfg["agents"]["defaults"]["concurrent_request_mode"] = serde_json::json!(mode);
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)?)?;

        let sec_path = ws
            .home()
            .join("workspace")
            .join("config")
            .join("config.security.json");
        let mut sec: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&sec_path)?)?;
        sec["layers"]["ssrf"] = serde_json::json!({ "enabled": false });
        std::fs::write(&sec_path, serde_json::to_string_pretty(&sec)?)?;
        Ok(())
    }
}

/// 启动 busy 轮（8s 工具轮），等 2s 确保 turn 1 在跑（session 被占），
/// 返回 busy 开始时刻（用于断言回执在 turn 1 完成前到达）。
async fn v5_kick_busy_turn(
    stream: &mut test_harness::WsStream,
    ai_port: u16,
) -> Result<std::time::Instant> {
    let content = format!("<B5_BUSY>http://127.0.0.1:{ai_port}</B5_BUSY>");
    let kick = std::time::Instant::now();
    ws_send_chat(stream, &content).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(kick)
}

#[tokio::test]
#[ignore = "V5a 真机：queue 模式 busy+追发。需重编后的 testaiserver.exe（含 testai-9.2 + /slow）+ target/release/nemesisbot.exe（含 V5 gate-in-pump 修复）"]
async fn v5_inbox_queue_mode_e2e() -> Result<()> {
    let (ws, mut ai, mut gw, cfg) = setup_test_home_ports(
        "test/testai-9.2",
        V5_AI_PORT,
        V5_WEB_PORT,
        V5_WS_CHANNEL_PORT,
        V5_HEALTH_PORT,
        v5_pre_gateway("queue"),
    )
    .await?;
    let result = run_v5a(&ws, &cfg, V5_AI_PORT, V5_WEB_PORT).await;
    let _ = gw.kill().await;
    let _ = ai.kill().await;
    result
}

/// queue 模式：busy 中追发普通消息 → ⏳ 排队回执（turn 1 完成前到），
/// 本轮不注入；turn 1 结束（B5_TURN1_DONE）后排队的消息起独立新轮
/// （B5_QUEUED_TURN2_OK）。provider 请求双端断言：turn 1 的全部请求
/// 不含 QUEUE_TURN2，产出 TURN2 回复的请求含 QUEUE_TURN2。
async fn run_v5a(
    ws: &TestWorkspace,
    cfg: &serde_json::Value,
    ai_port: u16,
    web_port: u16,
) -> Result<()> {
    let token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut stream = ws_connect(web_port, &token)
        .await
        .context("WS connect to test gateway failed")?;

    // —— ① busy 轮：8s 工具轮 ——
    let kick = v5_kick_busy_turn(&mut stream, ai_port).await?;

    // —— ② 追发普通消息（不带 !）→ ⏳ 排队回执 ——
    let follow_up = "QUEUE_TURN2 等等别删，暂停删除操作";
    ws_send_chat(&mut stream, follow_up).await?;
    //    回执必须在 turn 1 结束（≥8s）前到达，证明它走的是 gate 的
    //    busy 分支（inbox 排队），而不是等 turn 1 完后当新消息处理。
    let receipt = ws_wait_reply_containing(&mut stream, "已排队", 15).await?;
    assert!(receipt.contains("排队"), "queue receipt: {receipt}");
    assert!(
        kick.elapsed() < Duration::from_secs(8),
        "queue receipt must arrive while turn 1 is still busy (elapsed {:?})",
        kick.elapsed()
    );

    // —— ③ turn 1 正常收尾 ——
    let done = ws_wait_reply_containing(&mut stream, "B5_TURN1_DONE", 60).await?;
    assert!(done.contains("B5_TURN1_DONE"), "got: {done}");

    // —— ④ 排队消息起独立新轮 ——
    let queued = ws_wait_reply_containing(&mut stream, "B5_QUEUED_TURN2_OK", 60)
        .await
        .context("queued message must start its own turn after turn 1 ends")?;
    assert!(queued.contains("B5_QUEUED_TURN2_OK"), "got: {queued}");

    // —— ⑤ provider 请求真相源 ——
    let files = collect_ai_request_files(&ws.home());
    // turn 1 的首轮请求：含 B5_BUSY 触发串，不含追发内容。
    let first_req = files
        .iter()
        .find(|(_, c)| c.contains("<B5_BUSY>"))
        .context("no AI.Request.md contains the <B5_BUSY> trigger — turn 1 never ran?")?;
    assert!(
        !first_req.1.contains("QUEUE_TURN2"),
        "{}: queue follow-up must NOT be visible in turn-1's initial request (queue = turn boundary, not mid-turn injection)",
        first_req.0.display()
    );
    // turn 1 目录内的工具后请求（含 /slow 结果）：同样不含追发内容 ——
    // queue 模式在工具边界【不】注入（那是 steer 的语义）。
    // 注意：不能按 "slept 8 seconds" 全局过滤 —— request_logs 每轮消息一
    // 个目录，turn 2 的请求携带全量历史、同样含 turn 1 的工具结果，全局
    // 过滤会把它误判成“轮中注入”（首次真机跑的假阳性即此）。
    let turn1_dir = first_req
        .0
        .parent()
        .context("turn-1 request file has no parent dir")?;
    let post_tool: Vec<_> = files
        .iter()
        .filter(|(p, c)| p.parent() == Some(turn1_dir) && c.contains("slept 8 seconds"))
        .collect();
    assert!(
        !post_tool.is_empty(),
        "no AI.Request.md in turn-1's dir contains the /slow?secs=8 result — tool round not fed back?"
    );
    for (p, c) in &post_tool {
        assert!(
            !c.contains("QUEUE_TURN2"),
            "{}: queue mode must NOT inject at the tool boundary (steer semantics), got the follow-up mid-turn",
            p.display()
        );
    }
    // turn 2 的请求：追发内容真正到达模型。
    let turn2_req = files
        .iter()
        .find(|(_, c)| c.contains("QUEUE_TURN2"))
        .context(
            "no AI.Request.md contains QUEUE_TURN2 — the queued message never reached the model",
        )?;
    assert!(
        turn2_req.1.contains("<B5_BUSY>") || turn2_req.1.contains("slept 8 seconds"),
        "{}: turn-2 request must carry the session history (turn-1 trigger or tool result)",
        turn2_req.0.display()
    );

    Ok(())
}

#[tokio::test]
#[ignore = "V5b 真机：steer 模式 busy+!追发插队。需重编后的 testaiserver.exe（含 testai-9.2 + /slow）+ target/release/nemesisbot.exe（含 V5 gate-in-pump 修复）"]
async fn v5_inbox_steer_mode_e2e() -> Result<()> {
    let (ws, mut ai, mut gw, cfg) = setup_test_home_ports(
        "test/testai-9.2",
        V5S_AI_PORT,
        V5S_WEB_PORT,
        V5S_WS_CHANNEL_PORT,
        V5S_HEALTH_PORT,
        v5_pre_gateway("steer"),
    )
    .await?;
    let result = run_v5b(&ws, &cfg, V5S_AI_PORT, V5S_WEB_PORT).await;
    let _ = gw.kill().await;
    let _ = ai.kill().await;
    result
}

/// steer 模式：busy 中追发 `!` 前缀消息 → ⚡ 插话回执；在 turn 1 的
/// 工具边界（/slow 结果之后的 LLM 调用）以最新 user 消息注入（marker
/// 已剥）→ testai-9.2 报 B5_STEER_WITNESSED（而非 B5_TURN1_DONE）。
async fn run_v5b(
    ws: &TestWorkspace,
    cfg: &serde_json::Value,
    ai_port: u16,
    web_port: u16,
) -> Result<()> {
    let token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut stream = ws_connect(web_port, &token)
        .await
        .context("WS connect to test gateway failed")?;

    // —— ① busy 轮 + ② `!` 前缀追发 → ⚡ 插话回执 ——
    let kick = v5_kick_busy_turn(&mut stream, ai_port).await?;
    // 消息体不含内层 !/！（内层 marker 不会被剥，断言会混淆）。
    let steer_content = "!等等别删 STEER_INJECT";
    ws_send_chat(&mut stream, steer_content).await?;
    let receipt = ws_wait_reply_containing(&mut stream, "紧急插话", 15).await?;
    assert!(receipt.contains("插话"), "steer receipt: {receipt}");
    assert!(
        kick.elapsed() < Duration::from_secs(8),
        "steer receipt must arrive while turn 1 is still busy (elapsed {:?})",
        kick.elapsed()
    );

    // —— ③ 工具边界注入 → 模型在工具结果后的 LLM 调用里看到插话 ——
    let reply = ws_wait_reply_containing(&mut stream, "B5_", 60).await?;
    assert!(
        reply.contains("B5_STEER_WITNESSED"),
        "steer must be injected at the tool boundary and witnessed by the model, got: {reply}"
    );

    // —— ④ provider 请求真相源 ——
    let files = collect_ai_request_files(&ws.home());
    let post_tool = files
        .iter()
        .find(|(_, c)| c.contains("slept 8 seconds"))
        .context("no AI.Request.md contains the /slow?secs=8 result — tool round not fed back?")?;
    // 同一条请求里既有工具结果又有（剥掉 marker 的）插话内容 —— 注入
    // 发生在工具边界，而不是等轮次结束后另起一轮。
    assert!(
        post_tool.1.contains("等等别删 STEER_INJECT"),
        "{}: the post-tool request must contain the steer content (marker-stripped)",
        post_tool.0.display()
    );
    // marker 已剥：`!` 前缀是路由信号，不进模型上下文。
    assert!(
        !post_tool.1.contains("!等等别删"),
        "{}: the ! steer marker must be stripped before injection",
        post_tool.0.display()
    );

    Ok(())
}

// ===========================================================================
// Z1 (Phase4-d): session fork e2e —— 真分支 + 运行中网关 + 投影真相源
// ===========================================================================
//
// 模型 testai-9.3（Go 侧 TestAI93）回复 Z1_USERS_<n>（请求里 user 消息
// 数）。流程：源会话 z1src 三轮 → CLI `session fork --at 2`（gateway 保持
// 运行——fork 文件落盘后，网关靠 SessionStore 内存未命中→磁盘回退载入，
// 这是 Z1 的关键路径）→ 新会话首聊必须 Z1_USERS_3（拷贝的 2 轮 + 新 user
// = 投影完整）→ 源会话再聊 Z1_USERS_4（源未受影响，第 3 轮还在）。request
// log AI.Request.md 是 provider 实收真相源：fork 首请求含 T1/T2/FORK、不含
// T3/T4；源会话 post-fork 请求含 T1-T4、不含 FORK。

const Z1_AI_PORT: u16 = 18100;
const Z1_WEB_PORT: u16 = 49020;
const Z1_WS_CHANNEL_PORT: u16 = 49021;
const Z1_HEALTH_PORT: u16 = 49022;

/// 带 session_id 的聊天发送（多会话协议：data.session_id →
/// `agent:main:session:{sid}`，server.rs 的唯一 web 入站咽喉）。
async fn ws_send_chat_with_session(
    stream: &mut test_harness::WsStream,
    session_id: &str,
    content: &str,
) -> Result<()> {
    let msg = serde_json::json!({
        "type": "message",
        "module": "chat",
        "cmd": "send",
        "data": { "content": content, "session_id": session_id },
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string()
    });
    stream.send(Message::Text(msg.to_string().into())).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "Z1 真机：需重编后的 testaiserver.exe（含 testai-9.3）+ target/release/nemesisbot.exe（含 session fork 命令）"]
async fn z1_session_fork_e2e() -> Result<()> {
    let (ws, mut ai, mut gw, cfg) = setup_test_home_ports(
        "test/testai-9.3",
        Z1_AI_PORT,
        Z1_WEB_PORT,
        Z1_WS_CHANNEL_PORT,
        Z1_HEALTH_PORT,
        |_| Ok(()),
    )
    .await?;
    let result = run_z1(&ws, &cfg).await;
    let _ = gw.kill().await;
    let _ = ai.kill().await;
    result
}

async fn run_z1(ws: &TestWorkspace, cfg: &serde_json::Value) -> Result<()> {
    let bin = resolve_nemesisbot_bin()?;
    let token = cfg["channels"]["web"]["auth_token"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut stream = ws_connect(Z1_WEB_PORT, &token)
        .await
        .context("WS connect to test gateway failed")?;

    // —— ① 源会话 z1src 三轮（每轮回复即 user 消息数，自校验递增）——
    let sid = "z1src";
    ws_send_chat_with_session(&mut stream, sid, "Z1_T1_ALPHA").await?;
    ws_wait_reply_containing(&mut stream, "Z1_USERS_1", 60).await?;
    ws_send_chat_with_session(&mut stream, sid, "Z1_T2_BETA").await?;
    ws_wait_reply_containing(&mut stream, "Z1_USERS_2", 60).await?;
    ws_send_chat_with_session(&mut stream, sid, "Z1_T3_GAMMA").await?;
    ws_wait_reply_containing(&mut stream, "Z1_USERS_3", 60).await?;

    // —— ② CLI fork --at 2：gateway 保持运行（fork 文件由 CLI 进程落盘）——
    let out = ws
        .run_cli(
            &bin,
            &["session", "fork", "agent:main:session:z1src", "--at", "2"],
        )
        .await;
    anyhow::ensure!(
        out.success(),
        "session fork failed\nstdout: {}\nstderr: {}",
        out.stdout,
        out.stderr
    );
    anyhow::ensure!(
        out.stdout.contains("agent:main:session:z1src__fork"),
        "fork stdout must name the new session key, got: {}",
        out.stdout
    );

    // —— ③ 新会话首聊：投影 = 拷贝的前 2 轮 + 新 user ⟹ Z1_USERS_3 ——
    // （此时网关 SessionStore 从未见过 z1src__fork——本步走的正是
    // 内存未命中→磁盘回退载入 fork 文件的路径。）
    ws_send_chat_with_session(&mut stream, "z1src__fork", "Z1_FORK_DELTA").await?;
    let fork_reply = ws_wait_reply_containing(&mut stream, "Z1_USERS_", 60).await?;
    assert!(
        fork_reply.contains("Z1_USERS_3"),
        "fork first turn must see 3 user messages (2 copied + 1 new), got: {fork_reply}"
    );

    // —— ④ 源会话继续聊：第 4 轮 ⟹ Z1_USERS_4（源会话历史含全部 4 轮，
    // fork 没有动它）——
    ws_send_chat_with_session(&mut stream, sid, "Z1_T4_EPS").await?;
    let src_reply = ws_wait_reply_containing(&mut stream, "Z1_USERS_", 60).await?;
    assert!(
        src_reply.contains("Z1_USERS_4"),
        "source session must keep its full history (4 user turns), got: {src_reply}"
    );

    // —— ⑤ request log 真相源：provider 实收 messages ——
    let files = collect_ai_request_files(&ws.home());
    // fork 首请求：唯一含 FORK_DELTA 的请求。
    let fork_req = files
        .iter()
        .find(|(_, c)| c.contains("Z1_FORK_DELTA"))
        .context("no AI.Request.md contains Z1_FORK_DELTA — fork turn not logged?")?;
    for present in ["Z1_T1_ALPHA", "Z1_T2_BETA", "Z1_FORK_DELTA"] {
        assert!(
            fork_req.1.contains(present),
            "{}: fork first request must contain {present}",
            fork_req.0.display()
        );
    }
    for absent in ["Z1_T3_GAMMA", "Z1_T4_EPS"] {
        assert!(
            !fork_req.1.contains(absent),
            "{}: fork first request must NOT contain {absent} (cut at turn 2)",
            fork_req.0.display()
        );
    }
    // 源会话 post-fork 请求：唯一含 T4 的请求——含全部 4 轮、不含 fork 内容。
    let src_req = files
        .iter()
        .find(|(_, c)| c.contains("Z1_T4_EPS"))
        .context("no AI.Request.md contains Z1_T4_EPS — source turn 4 not logged?")?;
    for present in ["Z1_T1_ALPHA", "Z1_T2_BETA", "Z1_T3_GAMMA", "Z1_T4_EPS"] {
        assert!(
            src_req.1.contains(present),
            "{}: source post-fork request must contain {present}",
            src_req.0.display()
        );
    }
    assert!(
        !src_req.1.contains("Z1_FORK_DELTA"),
        "{}: source post-fork request must NOT contain the fork's content",
        src_req.0.display()
    );

    // —— ⑥ 盘上三层产物：store 文件 / chat_log 前缀 / boundary 事件 ——
    // SessionStore 文件（sanitize：`:`→`_`）。
    let fork_store = ws
        .home()
        .join("workspace")
        .join("sessions")
        .join("agent_main_session_z1src__fork.json");
    let store_data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fork_store)?)?;
    let msgs = store_data["messages"]
        .as_array()
        .context("fork store: messages array")?;
    let joined = msgs
        .iter()
        .map(|m| m["content"].as_str().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("Z1_T1_ALPHA") && joined.contains("Z1_T2_BETA"));
    // Z1_FORK_DELTA legitimately IS here by now: the gateway loaded the
    // fork (disk fallback) and persisted the fork's first own turn. The
    // inviolable invariants are the CUT (T3) and no cross-contamination
    // from the source's post-fork turns (T4).
    assert!(
        !joined.contains("Z1_T3_GAMMA") && !joined.contains("Z1_T4_EPS"),
        "fork store must stop at the fork point: no turn-3+ source content, no post-fork source turns — got len {}",
        msgs.len()
    );
    // chat_log 前缀（Dashboard 会话浏览器可见）。
    let fork_log = std::fs::read_to_string(
        ws.home()
            .join("workspace")
            .join("logs")
            .join("session_logs")
            .join("agent_main_session_z1src__fork.jsonl"),
    )
    .context("fork chat_log jsonl must exist (Dashboard session browser reads it)")?;
    assert!(fork_log.contains("Z1_T1_ALPHA") && fork_log.contains("Z1_T2_BETA"));
    assert!(!fork_log.contains("Z1_T3_GAMMA"));
    // boundary 事件双侧落盘（U9 sidecar）。
    for (key, ev) in [
        ("agent_main_session_z1src", "session_fork_out"),
        ("agent_main_session_z1src__fork", "session_fork_in"),
    ] {
        let sidecar = std::fs::read_to_string(
            ws.home()
                .join("workspace")
                .join("logs")
                .join("boundary")
                .join(format!("{key}.jsonl")),
        )
        .with_context(|| format!("boundary sidecar for {key} must exist"))?;
        assert!(sidecar.contains(ev), "sidecar for {key} must contain {ev}");
    }

    println!("Z1 session-fork e2e: 4 source turns + fork-at-2 projection all verified");
    Ok(())
}
