//! NemesisBot 自建基准套件（agent-bench，补齐差距 🔴3「可靠性公开叙事」）。
//!
//! 业界（OpenClaw 等）公开页都有可复跑的 benchmark/可靠性指标；本套件提供
//! **可复现的自证基线**：确定性 TestAI 脚本模型做评分 oracle，全程走真实
//! gateway WS 链路（chat.send → AgentLoop → LLM → 工具 → 回复），产出
//! JSON + Markdown 记分卡，并支持 baseline 保存/对比（pass_rate 回退即
//! exit 1，可挂 CI 门禁）。
//!
//! Usage:
//!   agent-bench                          # 全场景跑一轮，产出记分卡
//!   agent-bench --scenarios basic_chat   # 只跑指定场景（逗号分隔）
//!   agent-bench --save-baseline bench-baseline.json
//!   agent-bench --compare bench-baseline.json   # 回退即 exit 1
//!
//! 前置：`cargo build --release -p nemesisbot` + TestAIServer 已构建
//! （test-tools/TestAIServer/testaiserver.exe，需含 testai-2.1/9.3/vision）。

mod report;
mod scenarios;

use anyhow::{Result, bail};
use futures::StreamExt;
use report::Scorecard;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use test_harness::*;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

struct BenchConfig {
    ai_server_bin: PathBuf,
    gateway_bin: PathBuf,
    scenario_filter: Option<Vec<String>>,
    save_baseline: Option<PathBuf>,
    compare_baseline: Option<PathBuf>,
    out_dir: PathBuf,
}

impl BenchConfig {
    fn resolve() -> Result<Self> {
        let args: Vec<String> = std::env::args().collect();
        let mut ai_server_bin = None;
        let mut gateway_bin = None;
        let mut scenario_filter = None;
        let mut save_baseline = None;
        let mut compare_baseline = None;
        let mut out_dir = None;
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--ai-server" => {
                    i += 1;
                    ai_server_bin = Some(PathBuf::from(&args[i]));
                }
                "--gateway" => {
                    i += 1;
                    gateway_bin = Some(PathBuf::from(&args[i]));
                }
                "--scenarios" => {
                    i += 1;
                    scenario_filter =
                        Some(args[i].split(',').map(|s| s.trim().to_string()).collect());
                }
                "--save-baseline" => {
                    i += 1;
                    save_baseline = Some(PathBuf::from(&args[i]));
                }
                "--compare" => {
                    i += 1;
                    compare_baseline = Some(PathBuf::from(&args[i]));
                }
                "--out" => {
                    i += 1;
                    out_dir = Some(PathBuf::from(&args[i]));
                }
                other => bail!("未知参数: {other}"),
            }
            i += 1;
        }

        let ai_server_bin = ai_server_bin.unwrap_or_else(|| {
            resolve_ai_server_bin().unwrap_or_else(|_| {
                let root = resolve_project_root().unwrap_or_else(|_| PathBuf::from("."));
                root.join("test-tools/TestAIServer/testaiserver.exe")
            })
        });
        let gateway_bin = gateway_bin.unwrap_or_else(|| {
            resolve_nemesisbot_bin().unwrap_or_else(|_| {
                let root = resolve_project_root().unwrap_or_else(|_| PathBuf::from("."));
                root.join("target/release/nemesisbot.exe")
            })
        });
        let out_dir = out_dir.unwrap_or_else(|| {
            resolve_project_root()
                .map(|root| root.join("target/agent-bench"))
                .unwrap_or_else(|_| PathBuf::from("."))
        });

        Ok(Self {
            ai_server_bin,
            gateway_bin,
            scenario_filter,
            save_baseline,
            compare_baseline,
            out_dir,
        })
    }
}

// ---------------------------------------------------------------------------
// WSAPI 客户端（models.set_default 热切；reqId 关联，同 board_ws_tests 先例）
// ---------------------------------------------------------------------------

struct WsApi {
    stream: WsStream,
    next_id: u32,
}

impl WsApi {
    async fn connect() -> Result<Self> {
        Ok(Self {
            stream: ws_connect(WS_PORT, AUTH_TOKEN).await?,
            next_id: 0,
        })
    }

    async fn call(
        &mut self,
        module: &str,
        cmd: &str,
        data: Option<Value>,
    ) -> (Option<Value>, Option<String>) {
        self.next_id += 1;
        let req_id = format!("bench-{}", self.next_id);
        let msg = json!({
            "type": "request",
            "module": module,
            "cmd": cmd,
            "reqId": req_id,
            "data": data,
        });
        if let Err(e) = ws_send_json(&mut self.stream, &msg).await {
            return (None, Some(format!("ws send failed: {e}")));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            match tokio::time::timeout_at(deadline, self.stream.next()).await {
                Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => {
                    let Ok(v) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if v.get("type").and_then(|t| t.as_str()) == Some("response")
                        && v.get("reqId").and_then(|r| r.as_str()) == Some(req_id.as_str())
                    {
                        let err = v.get("error").and_then(|e| e.as_str()).map(String::from);
                        let dat = v.get("data").cloned().filter(|d| !d.is_null());
                        return (dat, err);
                    }
                    continue; // 别的 reqId / push
                }
                Ok(Some(Ok(_))) => continue, // ping/pong/binary
                Ok(Some(Err(e))) => return (None, Some(format!("ws error: {e}"))),
                Ok(None) => return (None, Some("ws closed".into())),
                Err(_) => return (None, Some("ws timeout (60s)".into())),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 环境装配
// ---------------------------------------------------------------------------

/// 基准专用 config.json：4 个确定性模型 + restrict_to_workspace（安全场景
/// 依赖边界硬围栏）。onboard 先行提取 workspace 模板，再整体覆写 config。
fn write_bench_config(ws: &TestWorkspace) -> Result<()> {
    let ai = ai_server_port();
    let model = |alias: &str| {
        json!({
            "model": format!("test/{alias}"),
            "model_name": alias,
            // ⚠ typed 字段名是 api_base——写作 base_url 会被 serde(flatten)
            // extra 静默吞掉 → provider base_url 空 → 调用瞬时失败
            // （2026-09-25 首跑实证；integration-test 恢复块的 base_url 键
            // 同病，其软断言吃下了错误回复才没红）。
            "api_base": format!("http://127.0.0.1:{ai}/v1"),
            "api_key": "test-key",
            "provider": "test",
            "enabled": true,
        })
    };
    let config = json!({
        "version": "1.0",
        "default_model": "test/testai-1.1",
        "model_list": [
            model("testai-1.1"),
            model("testai-9.3"),
            model("testai-2.1"),
            model("testai-5.0"),
            model("testai-vision-1.0"),
        ],
        "channels": {
            "web": {"enabled": true, "host": "127.0.0.1", "port": 49000, "auth_token": AUTH_TOKEN},
            // 独立 websocket 通道保持关（同 integration-test：SEC-001 拒裸绑定）。
            "websocket": {"enabled": false}
        },
        // health 端口钉死 harness 常量（18790 本机 ghost socket，见 HEALTH_PORT 注）。
        "gateway": {"host": "127.0.0.1", "port": HEALTH_PORT as i64},
        "agents": {
            "defaults": {
                "workspace": "",
                "restrict_to_workspace": true,
                "llm": "test/testai-1.1",
                "max_tokens": 8192,
                "temperature": 0.7,
                "max_tool_iterations": 20,
                "concurrent_request_mode": "reject",
                "queue_size": 8
            }
        },
        "security": {"enabled": true},
        "forge": {"enabled": false},
        "logging": {"llm": {"enabled": false}},
        "cluster": {"enabled": false}
    });
    std::fs::write(ws.config_path(), serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

/// 关 SSRF 层（loopback 目标会被拦——V3 先例 v3_pre_gateway 同款）。
/// tool_parallel_batch 的 web_fetch 指向 127.0.0.1:ai_port，必须放行。
fn disable_ssrf_layer(ws: &TestWorkspace) -> Result<()> {
    let path = ws
        .home()
        .join("workspace")
        .join("config")
        .join("config.security.json");
    let mut cfg: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    cfg["layers"]["ssrf"] = json!({ "enabled": false });
    std::fs::write(&path, serde_json::to_string_pretty(&cfg)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 场景调度
// ---------------------------------------------------------------------------

async fn run_scenario(
    name: &str,
    ctx: scenarios::BenchCtx,
    runs: u32,
    ws: &TestWorkspace,
    security_target: &std::path::Path,
) -> Vec<Result<(u64, String)>> {
    match name {
        "basic_chat" => scenarios::run_basic_chat(ctx, runs).await,
        "context_integrity" => scenarios::run_context_integrity(ctx, runs).await,
        "tool_parallel_batch" => scenarios::run_tool_parallel_batch(ctx, runs).await,
        "security_boundary_block" => {
            scenarios::run_security_boundary_block(ctx, runs, ws, security_target).await
        }
        "concurrent_sessions" => scenarios::run_concurrent_sessions(ctx, runs).await,
        "vision_roundtrip" => {
            scenarios::run_vision_roundtrip(ctx, runs, &ws.workspace().join("bench_image.png"))
                .await
        }
        other => {
            println!("  未知场景 {other}，跳过");
            Vec::new()
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("{}", "=".repeat(60));
    println!("  NemesisBot Agent Bench（自建基准套件 v1）");
    println!("{}", "=".repeat(60));

    let cfg = BenchConfig::resolve()?;
    if !cfg.ai_server_bin.exists() {
        bail!("AI server not found: {}", cfg.ai_server_bin.display());
    }
    if !cfg.gateway_bin.exists() {
        bail!("Gateway not found: {}", cfg.gateway_bin.display());
    }

    let catalog: Vec<_> = scenarios::default_catalog()
        .into_iter()
        .filter(|s| {
            cfg.scenario_filter
                .as_ref()
                .is_none_or(|f| f.iter().any(|n| n == s.name))
        })
        .collect();
    if catalog.is_empty() {
        bail!("场景过滤后为空（--scenarios 拼写检查）");
    }

    println!("\n  AI Server: {}", cfg.ai_server_bin.display());
    println!("  Gateway:   {}", cfg.gateway_bin.display());
    println!(
        "  场景:      {}",
        catalog
            .iter()
            .map(|s| s.name)
            .collect::<Vec<_>>()
            .join(", ")
    );

    let ws = TestWorkspace::new()?;
    println!("  Workspace: {}", ws.path().display());

    // ---- 环境清理 + 装配 ----
    cleanup_ports(&[ai_server_port(), WS_PORT, HEALTH_PORT]);

    println!("\n[1/4] Onboard（提取 workspace 模板）...");
    let onboard = ws.run_cli(&cfg.gateway_bin, &["onboard", "default"]).await;
    if !onboard.success() {
        bail!(
            "onboard default 失败: exit={} stderr={}",
            onboard.exit_code,
            &onboard.stderr[..onboard.stderr.len().min(400)]
        );
    }

    println!("[2/4] 写基准 config（4+1 模型 + 边界围栏）+ 关 SSRF 层...");
    write_bench_config(&ws)?;
    disable_ssrf_layer(&ws)?;

    println!("[3/4] 启动 AI Server（端口 {}）...", ai_server_port());
    let mut ai_server = ManagedProcess::spawn(
        "AI Server",
        &cfg.ai_server_bin,
        &["--port", &ai_server_port().to_string()],
        ws.path(),
    )?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    if !ai_server.is_running().await {
        ai_server.kill().await;
        bail!("AI Server 立即退出（端口 {} 可能被占）", ai_server_port());
    }
    wait_for_http(
        &format!("http://127.0.0.1:{}/health", ai_server_port()),
        Duration::from_secs(10),
    )
    .await?;

    println!("[4/4] 启动 Gateway（health 端口 {HEALTH_PORT}）...");
    let mut gateway = ManagedProcess::spawn(
        "Gateway",
        &cfg.gateway_bin,
        &["--local", "gateway"],
        ws.path(),
    )?;
    wait_for_http(
        &format!("http://127.0.0.1:{}/health", HEALTH_PORT),
        Duration::from_secs(15),
    )
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ---- 逐场景执行 ----
    let ctx = scenarios::BenchCtx {
        ai_port: ai_server_port(),
    };
    let security_target = std::env::temp_dir().join("agent-bench-pwned.txt");
    scenarios::drop_png_fixture(&ws)?;
    let mut card = Scorecard::new(gateway_version_stub(&cfg));
    let mut api = WsApi::connect().await?;
    let bench_start = Instant::now();

    for spec in &catalog {
        let model = report::scenario_model(spec.name);
        println!(
            "\n=== 场景 {}（模型 {model}，{} 轮）===",
            spec.name, spec.runs
        );

        // 逐场景热切默认模型（WSAPI，运行时生效——board_ws_tests 同款）。
        let (_, err) = api
            .call("models", "set_default", Some(json!({ "name": model })))
            .await;
        if let Some(e) = err {
            let score = card.score(spec.name);
            score.record_fail(format!("models.set_default({model}) 失败: {e}"));
            continue;
        }

        let results = run_scenario(spec.name, ctx, spec.runs, &ws, &security_target).await;
        let score = card.score(spec.name);
        for r in results {
            match r {
                Ok((ms, _)) => {
                    score.record_pass(ms);
                    println!("  PASS  {ms}ms");
                }
                Err(e) => {
                    score.record_fail(format!("{e:#}"));
                    println!("  FAIL  {e:#}");
                }
            }
        }
        println!(
            "  通过率 {:.1}%（{}/{}）",
            score.pass_rate() * 100.0,
            score.passed,
            score.total()
        );
    }
    card.bench_wall_secs = bench_start.elapsed().as_secs();

    // ---- 记分卡落盘 + baseline ----
    std::fs::create_dir_all(&cfg.out_dir)?;
    let json_path = cfg.out_dir.join("scorecard.json");
    let md_path = cfg.out_dir.join("scorecard.md");
    std::fs::write(&json_path, serde_json::to_string_pretty(&card.to_json())?)?;
    std::fs::write(&md_path, card.to_markdown())?;
    println!("\n记分卡：\n  {}", json_path.display());
    println!("  {}", md_path.display());

    if let Some(base) = &cfg.save_baseline {
        report::save_baseline(&card, base)?;
        println!("baseline 已保存：{}", base.display());
    }

    let mut exit_code = 0;
    if let Some(base) = &cfg.compare_baseline {
        match report::compare_baseline(&card, base) {
            Ok(regressions) if regressions.is_empty() => {
                println!("baseline 对比：无回退 ✓");
            }
            Ok(regressions) => {
                eprintln!("baseline 对比：{} 项回退：", regressions.len());
                for r in &regressions {
                    eprintln!("  - {r}");
                }
                exit_code = 1;
            }
            Err(e) => {
                eprintln!("baseline 对比失败: {e:#}");
                exit_code = 1;
            }
        }
    }

    // ---- 收尾（coverage-safe teardown 同款）----
    println!("\n停止服务...");
    match graceful_shutdown_gateway(WS_PORT, AUTH_TOKEN).await {
        Ok(()) => {
            if let Err(e) = gateway.wait_for_exit(Duration::from_secs(20)).await {
                println!("  warning: {e}；回退 kill");
                gateway.kill().await;
            }
        }
        Err(e) => {
            println!("  warning: graceful shutdown 失败（{e}）；kill gateway");
            gateway.kill().await;
        }
    }
    ai_server.kill().await;

    println!(
        "\n总通过率 {:.1}%（{}/{}，墙钟 {}s）",
        card.to_json()["totals"]["pass_rate"]
            .as_f64()
            .unwrap_or(0.0)
            * 100.0,
        card.to_json()["totals"]["passed"],
        card.to_json()["totals"]["runs"],
        card.bench_wall_secs
    );

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

/// 被测网关版本标注（记分卡元信息）。
fn gateway_version_stub(cfg: &BenchConfig) -> String {
    format!("{}", cfg.gateway_bin.display())
}
