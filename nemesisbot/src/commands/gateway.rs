//! Gateway command - start the NemesisBot gateway server.
//!
//! Mirrors Go CmdGateway:
//! 1. Check config file exists
//! 2. Check home directory exists
//! 3. Load configuration
//! 4. Initialize logger from config
//! 5. Write PID file
//! 6. Create MessageBus
//! 7. Create LLM Provider via factory
//! 8. Create AgentLoop with bus integration
//! 9. Create WebServer with bus
//! 10. Create HealthServer
//! 11. Create HeartbeatService
//! 12. Start all services
//! 13. Print gateway banner
//! 14. Wait for shutdown signal
//! 15. Graceful shutdown

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use nemesis_services::LifecycleService;
use tracing::{error, info, warn};

use crate::adapters;
use crate::common;

// ---------------------------------------------------------------------------
// W2 P4: board autopilot cron 触发 + 派发超时 sweep
// ---------------------------------------------------------------------------

/// 解析 `board-ap:{id}` → (store, autopilot)；store 不可用或 id 非法时报错。
#[cfg(feature = "board")]
fn resolve_autopilot_job<'a>(
    job_name: &str,
    board_store: Option<&'a std::sync::Arc<nemesis_board::BoardStore>>,
) -> Result<
    (
        &'a std::sync::Arc<nemesis_board::BoardStore>,
        nemesis_board::Autopilot,
    ),
    String,
> {
    let ap_id: i64 = job_name
        .strip_prefix("board-ap:")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("autopilot job 名无法解析: {job_name}"))?;
    let store = board_store.ok_or("board service not available (autopilot)")?;
    let ap = store
        .get_autopilot(ap_id)
        .map_err(|e| format!("autopilot #{ap_id} 加载失败: {e}"))?;
    Ok((store, ap))
}

/// cron on_job 的 board autopilot 分支（job 名 `board-ap:{id}`）：按规则
/// 模板建单（target 非空时派发）并落 last_run_at；返回值进 job run 历史。
/// disabled 规则到点跳过（启停是用户意图，不算故障）。
/// 全自动流转 D2：auto_plan 规则经 moderator 槽自动拆解（槽晚填 OnceLock；
/// hub 传 None——cron 装配早于 web server，诚实降级无 SSE 推送）。
#[cfg(all(feature = "board", feature = "cluster"))]
fn fire_board_autopilot(
    job_name: &str,
    board_store: Option<&std::sync::Arc<nemesis_board::BoardStore>>,
    cluster: Option<&std::sync::Arc<nemesis_cluster::cluster::Cluster>>,
    auto_plan: Option<&nemesis_web::handlers::board::AutoPlanContext>,
) -> Result<String, String> {
    let (store, ap) = resolve_autopilot_job(job_name, board_store)?;
    if !ap.enabled {
        return Ok(format!("autopilot「{}」已停用，跳过", ap.name));
    }
    let out = nemesis_web::handlers::board::fire_autopilot(
        store,
        cluster,
        &ap,
        &nemesis_board::Actor::system("autopilot"),
        auto_plan,
    )
    .map_err(|e| format!("autopilot「{}」触发失败: {e}", ap.name))?;
    Ok(format!("autopilot「{}」已触发: {out}", ap.name))
}

/// 非 cluster 编译变体：只有本地建单能力（target 非空的规则由 fire_autopilot
/// 建单前拒绝，语义与 cluster 版一致）。
#[cfg(all(feature = "board", not(feature = "cluster")))]
fn fire_board_autopilot(
    job_name: &str,
    board_store: Option<&std::sync::Arc<nemesis_board::BoardStore>>,
) -> Result<String, String> {
    let (store, ap) = resolve_autopilot_job(job_name, board_store)?;
    if !ap.enabled {
        return Ok(format!("autopilot「{}」已停用，跳过", ap.name));
    }
    let out = nemesis_web::handlers::board::fire_autopilot(
        store,
        &ap,
        &nemesis_board::Actor::system("autopilot"),
    )
    .map_err(|e| format!("autopilot「{}」触发失败: {e}", ap.name))?;
    Ok(format!("autopilot「{}」已触发: {out}", ap.name))
}

/// 一轮派发超时清扫（W2 P4-①②，board 派发无人回报时的兜底）。逐条
/// dispatched 记录判定：
///   ① 派发超过 `timeout_secs` 未回报 → 超时失败；
///   ② worker 在注册表且明确离线、且距派发 ≥ 离线宽限期（600s，容忍抖动）
///      → 离线失败。peer 缺失不判——未发现的 worker 可能只是还没上线，
///      保守等超时。
/// `fail_dispatch` 是竞态闸（只认 dispatched 态）：赢者补 ⛔ 系统评论 +
/// dispatch_failed 站内通知；输者（worker 恰好回报）不动，下一轮自然不再
/// 列出。MVP 策略 = abort + notify + 手动重派，不自动 retry/reassign——
/// 同一 issue 双 worker 并发执行的风险大于自动化的收益（开发日志有记）。
#[cfg(all(feature = "board", feature = "cluster"))]
fn sweep_dispatch_timeouts(
    store: &std::sync::Arc<nemesis_board::BoardStore>,
    cluster: &std::sync::Arc<nemesis_cluster::cluster::Cluster>,
    timeout_secs: u64,
) {
    const OFFLINE_GRACE_SECS: u64 = 600;
    let records = match store.list_active_dispatches() {
        Ok(r) => r,
        Err(e) => {
            warn!("[Board][Sweep] list active dispatches failed: {e}");
            return;
        }
    };
    if records.is_empty() {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for record in records {
        let age = now.saturating_sub(record.dispatched_at.max(0) as u64);
        let offline = cluster
            .get_peer(&record.worker_id)
            .map(|p| !p.is_online())
            .unwrap_or(false);
        let timed_out = age >= timeout_secs;
        let offline_expired = offline && age >= OFFLINE_GRACE_SECS;
        if !timed_out && !offline_expired {
            continue;
        }
        let reason = if timed_out {
            format!("派发超时（{timeout_secs}s 无回报）")
        } else {
            format!("worker 离线（{age}s 无回报且节点已离线）")
        };
        match store.fail_dispatch(&record.task_id, &reason) {
            Ok(Some(rec)) => {
                let _ = store.add_comment(nemesis_board::NewComment {
                    issue_id: rec.issue_id,
                    author: nemesis_board::Actor::system("board"),
                    content: format!(
                        "⛔ {reason}，派发已标记失败（task {}）。可重新派发或取消任务。",
                        rec.task_id
                    ),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
                if let Err(e) = store.notify_dispatch_event(
                    rec.issue_id,
                    nemesis_board::notification_kind::DISPATCH_FAILED,
                    &reason,
                ) {
                    warn!(
                        "[Board][Sweep] notify dispatch_failed (task {}): {e}",
                        rec.task_id
                    );
                }
                warn!(
                    "[Board][Sweep] dispatch failed: task={} issue={} ({reason})",
                    rec.task_id, rec.issue_id
                );
            }
            Ok(None) => { /* 输竞态（worker 恰好回报），下轮不再列出 */ }
            Err(e) => warn!(
                "[Board][Sweep] fail_dispatch (task {}): {e}",
                record.task_id
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Global shutdown state
// ---------------------------------------------------------------------------

/// Global shutdown flag (replaces Go's globalShutdownChan).
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request global shutdown from any component.
#[cfg(not(target_os = "android"))]
#[allow(dead_code)] // only called from the tray's on_quit (desktop feature); kept as a general API.
pub fn trigger_global_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Check if global shutdown has been requested.
#[allow(dead_code)]
pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Plugin window management via ProcessManager
// ---------------------------------------------------------------------------

/// Check if plugin-ui library exists in the `plugins/` directory next to the executable.
///
/// M7（devtool-upgrade 阶段 5）：审批弹窗被 WebApprovalManager（Dashboard 审批卡）
/// 同构替换，本函数仅剩 ApprovalPopupAdapter（同样停用）消费——非测试构建下
/// dead。恢复弹窗：gateway 装配块里把 `web_mgr` 换回
/// `Arc::new(ApprovalPopupAdapter::new(process_manager.clone()))` 即可。
#[cfg(all(feature = "desktop", feature = "security"))]
#[allow(dead_code)]
fn plugin_ui_library_exists() -> bool {
    nemesis_utils::find_plugin_library("plugin_ui").is_some()
}

/// Adapter bridging the agent's memory write/forget gate (P2) to the interactive
/// approval popup. When attached, agent `memory_store`/`memory_forget` calls pop
/// up an approval dialog; approval is never bypassed by YOLO/auto. Denies on
/// timeout/error so a memory write never silently succeeds unapproved.
#[cfg(all(feature = "desktop", feature = "memory", feature = "security"))]
struct GatewayMemoryGate {
    approval: Arc<dyn nemesis_security::auditor::ApprovalManager>,
}

#[cfg(all(feature = "desktop", feature = "memory", feature = "security"))]
impl GatewayMemoryGate {
    fn new(approval: Arc<dyn nemesis_security::auditor::ApprovalManager>) -> Self {
        Self { approval }
    }

    /// Request interactive approval. Returns true only if explicitly approved.
    /// Runs the blocking `request_approval_sync` on a spawn_blocking thread (it
    /// spawns a popup child process and waits for the user).
    async fn request(&self, operation: &str, preview: &str) -> bool {
        let am = self.approval.clone();
        let operation = operation.to_string();
        let preview = preview.to_string();
        let req_id = uuid::Uuid::new_v4().to_string();
        match tokio::task::spawn_blocking(move || {
            am.request_approval_sync(&req_id, &operation, "memory", "MEDIUM", &preview, 30)
        })
        .await
        {
            Ok(Ok(v)) => v.approved,
            _ => false, // denied / expired / errored → treat as not approved
        }
    }
}

#[cfg(all(feature = "desktop", feature = "memory", feature = "security"))]
#[async_trait::async_trait]
impl nemesis_memory::memory_tools::MemoryApprovalGate for GatewayMemoryGate {
    async fn approve_store(&self, preview: &str) -> bool {
        self.request("memory_store", preview).await
    }
    async fn approve_forget(&self, preview: &str) -> bool {
        self.request("memory_forget", preview).await
    }
}

/// LLM safety judge (guardian) backed by the gateway's LLM provider. Calls the
/// model with GUARDIAN_PROMPT + the action as evidence, parses the JSON verdict.
/// Used only for CRITICAL operations (cost-bounded by the agent loop).
#[cfg(feature = "security")]
struct GatewayLlmJudge {
    provider: Arc<dyn nemesis_providers::router::LLMProvider>,
    model: String,
}

#[cfg(feature = "security")]
#[async_trait::async_trait]
impl nemesis_security::guardian::LlmJudge for GatewayLlmJudge {
    async fn judge(
        &self,
        req: &nemesis_security::guardian::JudgeRequest,
    ) -> Result<nemesis_security::guardian::JudgeVerdict, String> {
        let user = format!(
            "Proposed action: {}\nAssigned risk level: {}\n\nTranscript (untrusted evidence):\n{}\n\nOutput the JSON verdict now.",
            req.action, req.risk_level, req.transcript
        );
        let messages = vec![
            nemesis_providers::types::Message {
                role: "system".into(),
                content: nemesis_security::guardian::GUARDIAN_PROMPT
                    .to_string()
                    .into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
            },
            nemesis_providers::types::Message {
                role: "user".into(),
                content: user.into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
            },
        ];
        let opts = nemesis_providers::types::ChatOptions {
            temperature: Some(0.0),
            max_tokens: Some(256),
            top_p: None,
            stop: None,
            reasoning_effort: None,
            extra: std::collections::HashMap::new(),
        };
        let resp = self
            .provider
            .chat(&messages, &[], &self.model, &opts)
            .await
            .map_err(|e| format!("guardian LLM call failed: {}", e))?;
        nemesis_security::guardian::parse_verdict(&resp.content)
    }
}

/// Adapter connecting ProcessManager to the security auditor's ApprovalManager trait.
///
/// When a tool call triggers an "ask" security rule, the auditor calls
/// `request_approval_sync()` which spawns an approval popup child process
/// via ProcessManager and blocks until the user responds.
///
/// **M7（devtool-upgrade 阶段 5）已停用**：审批交互同构替换为
/// `crate::web_approval::WebApprovalManager`（Dashboard 审批卡，SSE +
/// WSAPI approval.respond），全平台可用。恢复方法：gateway 装配块（"Wire up
/// ApprovalManager" 注释处）把 web_mgr 换回
/// `Arc::new(ApprovalPopupAdapter::new(process_manager.clone()))`；本结构体
/// 与 `plugin_ui_library_exists` 一并恢复使用。测试（gateway/tests.rs、
/// tests_r9_live.rs）仍直接构造它，保留编译。
#[cfg(all(feature = "desktop", feature = "security"))]
#[allow(dead_code)]
struct ApprovalPopupAdapter {
    process_manager: Arc<nemesis_desktop::process::ProcessManager>,
}

#[cfg(all(feature = "desktop", feature = "security"))]
#[allow(dead_code)]
impl ApprovalPopupAdapter {
    fn new(pm: Arc<nemesis_desktop::process::ProcessManager>) -> Self {
        Self {
            process_manager: pm,
        }
    }
}

#[cfg(all(feature = "desktop", feature = "security"))]
impl nemesis_security::auditor::ApprovalManager for ApprovalPopupAdapter {
    fn is_running(&self) -> bool {
        true
    }

    fn request_approval_sync(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        use nemesis_security::auditor::ApprovalVerdict;
        // Check if plugin-ui library exists. If not, reject immediately —
        // we cannot show an approval popup without the UI plugin, and
        // allowing the operation without user confirmation is unsafe.
        if !plugin_ui_library_exists() {
            let label = nemesis_utils::plugin_library_label();
            warn!(
                "[Gateway] Approval rejected: plugin-ui {} not found (operation={}, target={}, risk={}). \
                 Cannot show approval popup — denying by default.",
                label, operation, target, risk_level
            );
            return Ok(ApprovalVerdict::denied());
        }

        let data = serde_json::json!({
            "request_id": request_id,
            "operation": operation,
            "operation_name": operation,
            "target": target,
            "risk_level": risk_level,
            "reason": reason,
            "timeout_seconds": timeout_secs.max(30),
            "context": {},
            "timestamp": chrono::Local::now().timestamp(),
        });

        info!(
            "[Gateway] Requesting approval popup: operation={}, target={}, risk={}",
            operation, target, risk_level
        );

        let (_child_id, result_rx) = self
            .process_manager
            .spawn_child("approval", &data)
            .map_err(|e| format!("spawn_child failed: {}", e))?;

        let result_rx = result_rx.ok_or("no result channel")?;

        // The oneshot receiver is async but we're in a sync context.
        // Use a dedicated thread with its own tokio runtime to wait for the result.
        let (tx, rx) = std::sync::mpsc::channel::<Result<serde_json::Value, String>>();
        let wait_secs = timeout_secs + 10;
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(wait_secs), result_rx).await
            });
            match result {
                Ok(Ok(value)) => {
                    let _ = tx.send(Ok(value));
                }
                Ok(Err(_)) => {
                    let _ = tx.send(Err("channel closed".to_string()));
                }
                Err(_) => {
                    let _ = tx.send(Err("timeout".to_string()));
                }
            }
        });

        // Block until the user responds or timeout
        match rx.recv_timeout(std::time::Duration::from_secs(timeout_secs + 15)) {
            Ok(Ok(value)) => {
                let action = value
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("rejected");
                info!(
                    "[Gateway] Approval result: action={} for request_id={}",
                    action, request_id
                );
                if action == "approved" {
                    Ok(ApprovalVerdict::approved())
                } else {
                    Ok(ApprovalVerdict::denied())
                }
            }
            Ok(Err(e)) => {
                warn!("[Gateway] Approval channel error: {}", e);
                Ok(ApprovalVerdict::denied())
            }
            Err(_) => {
                warn!("[Gateway] Approval timeout after {}s", timeout_secs);
                Ok(ApprovalVerdict::denied()) // timeout = rejected
            }
        }
    }
}

/// Bridge adapter connecting Cluster to Forge's ClusterForgeBridge trait.
///
/// Enables Forge to share reflections with and receive reflections from
/// cluster peers. Mirrors Go's `forge.NewClusterForgeBridge(cluster)`.
#[cfg(all(feature = "cluster", feature = "forge"))]
struct ClusterForgeBridgeAdapter {
    node_id: String,
}

#[cfg(all(feature = "cluster", feature = "forge"))]
impl ClusterForgeBridgeAdapter {
    fn new(node_id: String) -> Self {
        Self { node_id }
    }
}

#[cfg(all(feature = "cluster", feature = "forge"))]
#[async_trait::async_trait]
impl nemesis_forge::bridge::ClusterForgeBridge for ClusterForgeBridgeAdapter {
    async fn share_reflection(&self, report_json: serde_json::Value) -> Result<usize, String> {
        // TODO: When cluster has a share_reflection method, call it here.
        // For now, store locally only (matches Go's early implementation).
        let _ = report_json;
        Ok(0)
    }

    async fn get_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String> {
        // TODO: When cluster has get_reflection_reports, call it here.
        Ok(Vec::new())
    }

    async fn get_online_peers(&self) -> Result<Vec<String>, String> {
        // TODO: When cluster has get_online_peers with node IDs, call it here.
        Ok(Vec::new())
    }

    fn local_node_id(&self) -> &str {
        &self.node_id
    }

    fn is_cluster_enabled(&self) -> bool {
        true
    }
}

// K1（devtool-upgrade 阶段 4）：apply_security_layer_switches /
// load_security_rules / load_scanner_full_config 与 Step 9b 装配块整体迁往
// `crate::security_setup`（单一真相源，headless `run` 共用）。

/// Open a URL in the default browser.
#[cfg(all(feature = "desktop", not(target_os = "android")))]
fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd")
            .raw_arg(format!("/c start {}", url))
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = url;
        Err("unsupported platform".to_string())
    }
}

/// Open a plugin window using ProcessManager for lifecycle and deduplication.
///
/// **Single-instance**: Only one window per type is allowed. If a window of
/// the same type already exists, a `window.bring_to_front` notification is
/// sent via WebSocket. If that fails (child dead or unresponsive), the stale
/// child is terminated and a new one is spawned.
///
/// Falls back to browser if the plugin-ui library is not found.
#[cfg(all(feature = "desktop", not(target_os = "android")))]
fn open_plugin_window(
    process_manager: &Arc<nemesis_desktop::process::ProcessManager>,
    window_type: &str,
    backend_url: &str,
    auth_token: &str,
) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("get exe path: {}", e))?;
    let exe_dir = exe.parent().ok_or("no parent dir")?;

    // Check if plugin-ui library exists
    if nemesis_utils::find_plugin_library_in(exe_dir, "plugin_ui").is_none() {
        warn!("[Gateway] plugin-ui library not found, falling back to browser");
        return open_browser(backend_url);
    }

    // --- Dedup: check if a child of this type already exists ---
    if let Some(child_id) = process_manager.get_child_by_type(window_type) {
        info!(
            "[Gateway] Plugin window '{}' already running (child_id: {}), sending bring_to_front",
            window_type, child_id
        );
        // Try to notify the existing child to bring its window to front
        match process_manager.notify_child(
            &child_id,
            "window.bring_to_front",
            serde_json::json!({}),
        ) {
            Ok(()) => {
                info!(
                    "[Gateway] Sent bring_to_front notification to child {}",
                    child_id
                );
                return Ok(());
            }
            Err(e) => {
                // Notification failed — child may be dead. Clean up and respawn.
                warn!(
                    "[Gateway] Failed to notify child {} ({}), cleaning up and respawning",
                    child_id, e
                );
                let _ = process_manager.terminate_child(&child_id);
                process_manager.cleanup_stale();
            }
        }
    }

    // Build window data
    let window_data = match window_type {
        "dashboard" => serde_json::json!({
            "token": auth_token,
            "web_port": backend_url.split(':').next_back().and_then(|p| p.parse::<u16>().ok()).unwrap_or(49000),
            "web_host": backend_url.split("://").nth(1).and_then(|s| s.split(':').next()).unwrap_or("127.0.0.1"),
        }),
        "approval" => serde_json::json!({}),
        _ => serde_json::json!({}),
    };

    // Spawn new child via ProcessManager (handles pipe handshake + WS key + window data)
    match process_manager.spawn_child(window_type, &window_data) {
        Ok((child_id, _result_rx)) => {
            info!(
                "[Gateway] Plugin window '{}' spawned (child_id: {})",
                window_type, child_id
            );
            Ok(())
        }
        Err(e) => {
            warn!(
                "[Gateway] Failed to spawn plugin window '{}': {}",
                window_type, e
            );
            Err(format!("spawn failed: {}", e))
        }
    }
}

// ---------------------------------------------------------------------------
// Gateway banner
// ---------------------------------------------------------------------------

/// Print the gateway startup banner.
fn print_gateway_banner(
    web_host: &str,
    web_port: i64,
    auth_token: &str,
    channels_enabled: usize,
    gateway_host: &str,
    gateway_port: i64,
) {
    println!();
    println!("{}", "=".repeat(50));
    println!("NemesisBot Gateway");
    println!("{}", "=".repeat(50));
    println!("  Web Interface: http://{}:{}", web_host, web_port);
    println!("  Auth Token: {}", common::format_token(auth_token));

    if channels_enabled > 0 {
        println!("  OK {} channel(s) enabled", channels_enabled);
    } else {
        println!("  WARNING: No channels enabled");
    }

    println!("  OK Gateway started on {}:{}", gateway_host, gateway_port);
    println!();
    println!("  Press Ctrl+C to stop");
    println!("{}", "=".repeat(50));
    println!();
}

/// G9（2026-09-09 结构修复）：web host 解析——绑定地址与展示地址分离。
///
/// 配置 host `"0.0.0.0"`/空 = 绑定所有网卡。集群场景必须**如实绑定**：
/// worker 跨机拉资产走 HTTP，绑定回环则 bundle 广告出去的 LAN IP 全是
/// 空头支票（旧逻辑无条件翻译成 127.0.0.1，逼用户手工把 host 配成
/// LAN IP 才能跑通 G9 场景）。单机场景维持保守回环绑定，dashboard 不
/// 无谓暴露局域网。展示地址（gateway state / banner / 浏览器 URL）永远
/// 用可进地址栏的地址——0.0.0.0 不是合法浏览器地址。
/// 返回 (绑定 host, 展示 host)。
pub fn web_bind_and_display_hosts(configured: &str, cluster_starts: bool) -> (String, String) {
    let h = configured.trim();
    if h == "0.0.0.0" || h.is_empty() {
        if cluster_starts {
            ("0.0.0.0".to_string(), "127.0.0.1".to_string())
        } else {
            ("127.0.0.1".to_string(), "127.0.0.1".to_string())
        }
    } else {
        (h.to_string(), h.to_string())
    }
}

/// G9：对外资产基址 host 选择——只在 socket **真实监听所有网卡**（绑定
/// unspecified 地址）时才广告 LAN IP（此时跨机可达为真，与 cluster 节点
/// 注册挑 announce 地址同策略）；绑定回环或具体网卡时如实广告实际绑定
/// 地址（回环绑定 + 广告 LAN IP = 承诺不可达 URL，正是 G9 病灶）。
#[cfg_attr(
    not(all(feature = "board", feature = "cluster")),
    allow(dead_code) // 唯一调用点在 board+cluster 资产基址块；裁剪构建下不参与
)]
pub fn advertise_host_for(bound_ip: std::net::IpAddr, lan_ip: Option<String>) -> String {
    if bound_ip.is_unspecified() {
        lan_ip.unwrap_or_else(|| "127.0.0.1".to_string())
    } else {
        bound_ip.to_string()
    }
}

/// G9：从本机 IP 列表与集群注册表已知 peer 地址推导**对外 NIC**——与任一
/// peer 同网段的本机 IP 优先（peer 已证明在该网段内活动，本机同网段地址
/// 即为跨机可达的最优候选），无信号回落首个非回环 IP。多重网卡机器
/// （ICS/VPN/Hyper-V 虚拟适配器并存）靠这个选中真正连集群的网卡，而不是
/// `get_all_local_ips` 的首猜。纯函数便于单测。
#[cfg_attr(
    not(all(feature = "board", feature = "cluster")),
    allow(dead_code) // 同上
)]
pub fn select_advertised_lan_ip(local_ips: &[String], peer_addrs: &[String]) -> Option<String> {
    let peer_hosts: Vec<&str> = peer_addrs
        .iter()
        .filter_map(|a| a.rsplit_once(':').map(|(h, _)| h))
        .collect();
    // 同 /24 网段的 IPv4 优先（前三个八位组一致）。
    for ip in local_ips {
        if ip.starts_with("127.") {
            continue;
        }
        let Some((a, b, c, _)) = parse_ipv4_octets(ip) else {
            continue;
        };
        for ph in &peer_hosts {
            if let Some((x, y, z, _)) = parse_ipv4_octets(ph)
                && (a, b, c) == (x, y, z)
            {
                return Some(ip.clone());
            }
        }
    }
    local_ips.iter().find(|ip| !ip.starts_with("127.")).cloned()
}

/// 解析点分 IPv4 的四个八位组；非 v4 形态返回 None。
fn parse_ipv4_octets(s: &str) -> Option<(u8, u8, u8, u8)> {
    let mut it = s.trim().split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c, d))
}

/// G9 装配侧取数：本机 IP 列表 + 集群注册表已知 peer 地址（**排除本节点
/// 自己**——本节点注册地址是 get_all_local_ips 首猜，可能恰是要纠正的错
/// NIC），交给纯函数选对外 NIC。
#[cfg_attr(
    not(all(feature = "board", feature = "cluster")),
    allow(dead_code) // 同上
)]
fn select_lan_ip_for_advertisement(cluster: &nemesis_cluster::cluster::Cluster) -> Option<String> {
    let local_ips = nemesis_cluster::network::get_all_local_ips();
    let self_id = cluster.node_id();
    let peer_addrs: Vec<String> = cluster
        .list_nodes()
        .into_iter()
        .filter(|n| n.base.id != self_id)
        .map(|n| n.base.address)
        .collect();
    select_advertised_lan_ip(&local_ips, &peer_addrs)
}

#[cfg(test)]
mod tests;

/// R9 补测批：gateway 活动场景组（live 双节点/心跳/审批/工作流，见模块头注释）。
/// 整文件 Windows 形态（11/11 live 场景走 Windows CLI 进程边界），随测试一并门控。
#[cfg(all(test, windows))]
mod tests_r9_live;

/// E1 二期 token 回传（全自动流转 P5）：把 worker 回调携带的 `usage` 记入
/// master 用量账本（DataStore request_logs）。记账键 = `cluster_rpc:
/// {worker}/{task_id}`（与 worker 侧 `cluster_rpc:{A}/{chat}` 会话键同前缀
/// 家族；per-task 粒度让 token 预算闸能按派发行精确聚合，`{worker}%` LIKE
/// 前缀聚合同时可用）。诚实边界：无 usage（旧 worker / 错误回调）/ 无
/// DataStore / 落库失败 → 静默跳过（warn），绝不影响回调路由。
#[cfg(feature = "cluster")] // 唯一消费点在 peer_chat_callback（集群回调闭包内）
fn record_cluster_usage(
    ds: Option<&std::sync::Arc<nemesis_data::DataStore>>,
    source_node: &str,
    task_id: &str,
    usage: Option<&serde_json::Value>,
) {
    let Some(ds) = ds else { return };
    let Some(usage) = usage else { return };
    if task_id.is_empty() {
        return;
    }
    let get_num = |key: &str| -> i64 {
        usage
            .get(key)
            .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)))
            .unwrap_or(0)
    };
    let input = get_num("input_tokens");
    let output = get_num("output_tokens");
    let cost = usage
        .get("cost_usd")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let log = nemesis_data::RequestLog {
        id: 0,
        trace_id: format!("cluster-callback:{task_id}"),
        model: format!("cluster_delegate:{source_node}"),
        provider_type: "cluster".to_string(),
        input_tokens: input,
        output_tokens: output,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_cost_usd: cost,
        latency_ms: 0,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: chrono::Local::now().timestamp(),
        pricing_model: String::new(),
        input_cost_usd: 0.0,
        output_cost_usd: 0.0,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        first_token_ms: None,
        session_key: format!("cluster_rpc:{source_node}/{task_id}"),
    };
    if let Err(e) = ds.insert_request_log(&log) {
        tracing::warn!("[Gateway] Failed to record cluster callback usage: {e}");
    }
}

/// Parse "host:port" string into (host, port).
#[cfg(any(feature = "cluster", test))]
fn parse_host_port(addr: &str) -> (String, u16) {
    if let Some(idx) = addr.rfind(':') {
        let host = &addr[..idx];
        let port: u16 = addr[idx + 1..].parse().unwrap_or(0);
        (host.to_string(), port)
    } else {
        (addr.to_string(), 0)
    }
}

/// Count enabled channels.
fn count_enabled_channels(cfg: &nemesis_config::Config) -> usize {
    let mut count = 0;
    if cfg.channels.web.enabled {
        count += 1;
    }
    if cfg.channels.websocket.enabled {
        count += 1;
    }
    if cfg.channels.telegram.enabled {
        count += 1;
    }
    if cfg.channels.discord.enabled {
        count += 1;
    }
    if cfg.channels.feishu.enabled {
        count += 1;
    }
    if cfg.channels.slack.enabled {
        count += 1;
    }
    if cfg.channels.external.enabled {
        count += 1;
    }
    if cfg.channels.whatsapp.enabled {
        count += 1;
    }
    if cfg.channels.dingtalk.enabled {
        count += 1;
    }
    if cfg.channels.qq.enabled {
        count += 1;
    }
    if cfg.channels.line.enabled {
        count += 1;
    }
    if cfg.channels.onebot.enabled {
        count += 1;
    }
    if cfg.channels.maixcam.enabled {
        count += 1;
    }
    count
}

/// Print agent startup information.
fn print_agent_startup_info(home: &std::path::Path, total_tools: usize) {
    // Use register_default_tools just for counting display purposes
    let tools = nemesis_agent::register_default_tools();
    let default_count = tools.len();

    let skills_dir = home.join("workspace").join("skills");
    let skill_count = std::fs::read_dir(&skills_dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .count()
        })
        .unwrap_or(0);

    println!();
    println!("  Agent Status:");
    // saturating：注册 skew（total < default）是显示问题不是 panic 理由
    // （回归锁：test_print_agent_startup_info_no_panic 传小总数）。
    println!(
        "    Tools: {} loaded ({} default + {} extended)",
        total_tools,
        default_count,
        total_tools.saturating_sub(default_count)
    );
    println!("    Skills: {} available", skill_count);
    info!(
        "[Gateway] Agent initialized ({} tools, {} skills)",
        total_tools, skill_count
    );
}

// ---------------------------------------------------------------------------
// Cluster adapter types
// ---------------------------------------------------------------------------

/// Adapter: Cluster result store → TaskResultPersister trait.
///
// Bridges the cluster's TaskResultStore to PeerChatHandler's
/// TaskResultPersister interface.
#[cfg(feature = "cluster")]
struct ClusterResultPersisterAdapter {
    result_store: Arc<nemesis_cluster::task_result_store::TaskResultStore>,
    node_id: String,
}

#[cfg(feature = "cluster")]
impl nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister
    for ClusterResultPersisterAdapter
{
    fn set_running(&self, task_id: &str, _source_node: &str) {
        // Mark as running with a placeholder result
        self.result_store.store_success(
            task_id,
            "peer_chat",
            serde_json::json!({
                "status": "running",
                "from": self.node_id,
            }),
        );
    }

    fn set_result(
        &self,
        task_id: &str,
        status: &str,
        response: &str,
        error: &str,
        _source_node: &str,
    ) -> Result<(), String> {
        if status == "error" {
            self.result_store.store_failure(task_id, "peer_chat", error);
        } else {
            // G5 键归一（2026-09-01）：恢复轮询的 query_task_result handler
            // 读 "response"（与 peer_chat_callback 信封同键）；旧值 "content"
            // 曾导致查询恢复拿到空回复。
            self.result_store.store_success(
                task_id,
                "peer_chat",
                serde_json::json!({
                    "response": response,
                    "from": self.node_id,
                }),
            );
        }
        Ok(())
    }

    fn delete(&self, task_id: &str) -> Result<(), String> {
        // 2026-09-08 修复：旧实现是 no-op（注释谎称 "TaskResultStore doesn't
        // have a delete method"——remove() 明明存在）。后果：回调成功后
        // set_running 占位文件永留 rpc_cache/results/，7 天 TTL 才清扫。
        self.result_store.remove(task_id);
        Ok(())
    }
}

/// Adapter: nemesis_bus::MessageBus → Cluster's MessageBus trait.
///
/// Translates Cluster's BusInboundMessage to nemesis_types::InboundMessage
/// and publishes on the real message bus.
#[cfg(feature = "cluster")]
struct BusToClusterAdapter {
    bus: Arc<nemesis_bus::MessageBus>,
}

#[cfg(feature = "cluster")]
impl nemesis_cluster::cluster::MessageBus for BusToClusterAdapter {
    fn publish_inbound(&self, msg: nemesis_cluster::cluster::BusInboundMessage) {
        let inbound = nemesis_types::channel::InboundMessage {
            channel: msg.channel,
            sender_id: msg.sender_id,
            chat_id: msg.chat_id,
            content: msg.content,
            media: vec![],
            session_key: String::new(),
            correlation_id: String::new(),
            // G5: 透传结构化元数据（cluster_continuation 的 status /
            // source_node / error）——AgentLoop 续行拦截依赖这些字段。
            metadata: msg.metadata,
            voice_playback: None,
        };
        self.bus.publish_inbound(inbound);
    }
}

// ---------------------------------------------------------------------------
// Gateway command
// ---------------------------------------------------------------------------

/// One-shot migration: move pre-refactor workflow files from
/// `{home}/workflow/` (the legacy flat layout) into the new four-subdir
/// layout under `{home}/workspace/workflow/`.
///
/// Legacy layout (pre-refactor):
///   {home}/workflow/{wf}_{exec}.jsonl         -> executions/
///   {home}/workflow/checkpoints/{exec}/{cp}.json -> checkpoints/
///
/// New layout (post-refactor):
///   {home}/workspace/workflow/executions/{wf}_{exec}.jsonl
///   {home}/workspace/workflow/checkpoints/{exec}/{cp}.json
///
/// Runs only if the legacy dir exists. Skips files that already exist at
/// the destination (idempotent across re-runs). Removes the legacy dir if
/// it ends up empty. Errors are logged at warn level — gateway startup
/// proceeds regardless, since stale data shouldn't block the service.
#[cfg(feature = "workflow")]
fn migrate_legacy_workflow_dir(
    home: &std::path::Path,
    new_executions_dir: &std::path::Path,
    new_checkpoints_dir: &std::path::Path,
) {
    let legacy_root = home.join("workflow");
    if !legacy_root.exists() {
        return;
    }
    info!(
        "[Gateway] Migrating legacy workflow dir: {} -> {}",
        legacy_root.display(),
        new_executions_dir
            .parent()
            .unwrap_or(new_executions_dir)
            .display()
    );

    // Move *.jsonl execution logs.
    if let Ok(entries) = std::fs::read_dir(&legacy_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|ext| ext == "jsonl")
                    .unwrap_or(false)
            {
                let dest = new_executions_dir.join(path.file_name().unwrap_or_default());
                if dest.exists() {
                    continue;
                }
                if let Err(e) = std::fs::rename(&path, &dest) {
                    warn!(
                        "[Gateway] migration: failed to move {}: {}",
                        path.display(),
                        e
                    );
                }
            }
        }
    }

    // Move checkpoints/ subdir contents.
    let legacy_checkpoints = legacy_root.join("checkpoints");
    if legacy_checkpoints.exists()
        && let Ok(entries) = std::fs::read_dir(&legacy_checkpoints)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let dest = new_checkpoints_dir.join(&name);
            if dest.exists() {
                continue;
            }
            if let Err(e) = std::fs::rename(&path, &dest) {
                warn!(
                    "[Gateway] migration: failed to move checkpoint {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    // Best-effort cleanup: remove legacy dir if empty (ignoring the
    // now-empty checkpoints/ subdir). Don't touch non-empty dir — user may
    // have files we don't recognise.
    let _ = std::fs::remove_dir(legacy_root.join("checkpoints"));
    if std::fs::read_dir(&legacy_root)
        .map(|mut e| e.next().is_none())
        .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(&legacy_root);
        info!("[Gateway] Migration complete; removed empty legacy workflow dir");
    } else {
        warn!(
            "[Gateway] Migration partial: legacy dir {} not empty, left in place",
            legacy_root.display()
        );
    }
}

/// Swarm M4 批作业（§6）：写回把 issue 推进到 in_review 时携带评审触发
/// 目标——回调闭包据此 spawn 验收 agent（`board.auto_review` 闸在评审
/// 任务内读配置判定）。
#[cfg(all(feature = "board", feature = "cluster"))]
struct BoardWritebackOutcome {
    is_board_task: bool,
    issue_for_review: Option<i64>,
}

/// W2 P2 派发写回：board 派发（`issue.dispatch`）的 task_id 命中
/// issue_dispatch 表 → 终结派发（幂等）+ worker 结果评论 + 状态推进
/// （成功 → in_review 等 coordinator 验收；失败留在 in_progress）。
/// 返回是否为 board 派发任务——是则 peer_chat_callback 跳过 agent 续行
/// 路由（board 派发无续行快照，进 bus 只会产生加载失败噪音）。
/// board store 未注入（打开失败）→ 恒 false，写回静默关闭。
/// 唯一调用点在 peer_chat_callback（cluster 编译时）——board-only 构建下
/// 该函数不可达，cfg 需同时含 cluster 以免 dead_code 告警。
///
/// Swarm M3 交付线程（§5.6/G5）：成功且 worker 按四段汇报格式回流 →
/// ctype='delivery' 首评（原样保留，M4 验收 agent 同源解析）；没按格式 →
/// 全文当普通评论（诚实降级）。汇报超 64KB → 全文落资产 + 截断内联 +
/// 引用注记（全文走层 2 HTTP 资产拉取）。
#[cfg(all(feature = "board", feature = "cluster"))]
fn write_back_board_dispatch(
    board_store: &Option<std::sync::Arc<nemesis_board::BoardStore>>,
    workspace: &std::path::Path,
    task_id: &str,
    status: &str,
    response: &str,
) -> BoardWritebackOutcome {
    use nemesis_board::models::dispatch_state;

    let Some(bstore) = board_store.as_ref() else {
        return BoardWritebackOutcome {
            is_board_task: false,
            issue_for_review: None,
        };
    };
    if task_id.is_empty() {
        return BoardWritebackOutcome {
            is_board_task: false,
            issue_for_review: None,
        };
    }
    let disp = match bstore.get_dispatch(task_id) {
        Ok(Some(d)) => d,
        Ok(None) => {
            return BoardWritebackOutcome {
                is_board_task: false,
                issue_for_review: None,
            };
        }
        Err(e) => {
            // 读失败≠非 board 任务，但也不能误吞 agent 续行——按既有路由
            // 处理（false），告警留痕。
            warn!("[Gateway] board dispatch lookup failed (task_id={task_id}): {e}");
            return BoardWritebackOutcome {
                is_board_task: false,
                issue_for_review: None,
            };
        }
    };
    if disp.state != dispatch_state::DISPATCHED {
        info!(
            "[Gateway] board dispatch {task_id} already terminal ({}), skip",
            disp.state
        );
        return BoardWritebackOutcome {
            is_board_task: true,
            issue_for_review: None,
        };
    }

    let terminal = if status == "error" {
        dispatch_state::FAILED
    } else {
        dispatch_state::DONE
    };
    match bstore.finish_dispatch(task_id, terminal) {
        Ok(true) => {
            let worker_actor = nemesis_board::Actor::agent(&disp.worker_id);
            let (ctype, content) = if status == "error" {
                // 失败汇报不做格式判定——错误输出原样留痕。
                (
                    nemesis_board::CommentType::Comment,
                    format!("⛔ worker 汇报失败：\n\n{response}"),
                )
            } else if nemesis_board::parse_delivery_report(response).is_some() {
                // 结构化汇报 → 交付线程首评（G5）。超限先做截断+资产注记。
                (
                    nemesis_board::CommentType::Delivery,
                    delivery_inline_or_asset(bstore, workspace, response),
                )
            } else {
                // 无格式 → 诚实降级为普通评论。
                (
                    nemesis_board::CommentType::Comment,
                    format!("✅ worker 汇报完成：\n\n{response}"),
                )
            };
            if let Err(e) = bstore.add_comment(nemesis_board::NewComment {
                issue_id: disp.issue_id,
                author: worker_actor.clone(),
                content,
                parent_id: None,
                ctype,
            }) {
                warn!("[Gateway] board writeback comment failed (task_id={task_id}): {e}");
            }
            // 成功 → in_review（coordinator/验收 agent 处置）；失败留在
            // in_progress，失败评论已留痕（重派走 issue.dispatch）。推进
            // 成功即携带 M4 评审触发目标。
            let mut issue_for_review = None;
            if status != "error"
                && let Ok(issue) = bstore.get_issue(disp.issue_id)
                && issue.status == nemesis_board::IssueStatus::InProgress
            {
                match bstore.transition_issue(
                    disp.issue_id,
                    nemesis_board::IssueStatus::InReview,
                    &worker_actor,
                ) {
                    Ok(_) => issue_for_review = Some(disp.issue_id),
                    Err(e) => {
                        warn!(
                            "[Gateway] board writeback transition failed (task_id={task_id}): {e}"
                        );
                    }
                }
            }
            info!(
                "[Gateway] board dispatch writeback done (task_id={task_id}, issue_id={}, state={terminal})",
                disp.issue_id
            );
            BoardWritebackOutcome {
                is_board_task: true,
                issue_for_review,
            }
        }
        Ok(false) => {
            info!("[Gateway] board dispatch {task_id} finished concurrently, skip writeback");
            BoardWritebackOutcome {
                is_board_task: true,
                issue_for_review: None,
            }
        }
        Err(e) => {
            warn!("[Gateway] board dispatch finish failed (task_id={task_id}): {e}");
            BoardWritebackOutcome {
                is_board_task: true,
                issue_for_review: None,
            }
        }
    }
}

/// 汇报内联策略（交付线程首评）：≤64KB 原样内联；超限 → 全文落资产
/// （`delivery-<sha8>`）+ 截断内联 + 引用束注记（全文走层 2 HTTP）。
/// 资产写盘/登记/签发失败时诚实给无束注记，不丢截断内容也不炸写回。
#[cfg(all(feature = "board", feature = "cluster"))]
fn delivery_inline_or_asset(
    bstore: &nemesis_board::BoardStore,
    workspace: &std::path::Path,
    response: &str,
) -> String {
    use nemesis_board::report::MAX_INLINE_BYTES;

    if response.len() <= MAX_INLINE_BYTES {
        return response.to_string();
    }

    let head = floor_char_boundary(response, MAX_INLINE_BYTES);
    let sha = nemesis_board::sha256_bytes(response.as_bytes());
    let ref_name = format!("delivery-{}", &sha[..8]);

    // 签发引用束（与 board_asset 工具 publish 同源：同一 secret 文件 + 同一
    // node url 文件，token 在资产端点验证一致）。任一步失败 → 诚实注记，
    // 不丢截断内容也不炸写回。
    let bundle_json = (|| -> Option<String> {
        let assets_dir = nemesis_path::resolve_board_assets_dir_in_workspace(workspace);
        std::fs::create_dir_all(&assets_dir).ok()?;
        std::fs::write(assets_dir.join(&ref_name), response).ok()?;
        bstore
            .register_asset(nemesis_board::NewAsset {
                ref_name: ref_name.clone(),
                origin_issue: None,
                sha256: sha.clone(),
                size: response.len() as i64,
            })
            .ok()?;
        let secret = nemesis_board::load_or_create_secret(
            &nemesis_path::resolve_asset_secret_path_in_workspace(workspace),
        )
        .ok()?;
        let node_url = std::fs::read_to_string(
            nemesis_path::resolve_asset_node_url_path_in_workspace(workspace),
        )
        .ok()?;
        let node_url = node_url.trim().to_string();
        if node_url.is_empty() {
            return None;
        }
        let bundle = nemesis_board::issue_asset_bundle(
            &secret,
            &ref_name,
            &sha,
            response.len() as i64,
            &node_url,
            nemesis_board::DEFAULT_TOKEN_TTL_SECS,
        );
        serde_json::to_string(&bundle).ok()
    })();

    match bundle_json {
        Some(json) => format!(
            "{head}\n\n[汇报全文 {len} 字节，超过 64KB 内联上限已截断——全文下载引用：\n```json\n{json}\n```]",
            len = response.len()
        ),
        None => format!(
            "{head}\n\n[汇报全文 {len} 字节，超过 64KB 内联上限已截断；资产存档/签发失败，全文未能存档]",
            len = response.len()
        ),
    }
}

/// 字节上限的安全切片（多字节字符向下取整到 char boundary）。
#[cfg(all(feature = "board", feature = "cluster"))]
fn floor_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Run the gateway command.
pub async fn run(local: bool, extra_args: &[String]) -> Result<()> {
    // macOS: acquire the tray-handoff channel guard first, so that ANY return
    // path from this function (early `?` errors, normal completion) closes the
    // channel and unblocks the main thread waiting for the tray. See
    // nemesis_desktop::main_thread_handoff. (process::exit paths terminate the
    // whole process, so they need no special handling here.)
    #[cfg(target_os = "macos")]
    let _tray_channel_guard = nemesis_desktop::main_thread_handoff::channel_guard();

    // Step 1: Resolve home directory
    let home = common::resolve_home(local);

    // Step 2: Check configuration file exists
    let config_path = common::config_path(&home);
    if !config_path.exists() {
        eprintln!(
            "Error: Configuration file not found: {}",
            config_path.display()
        );
        eprintln!();
        eprintln!("  Gateway mode requires a configuration file.");
        eprintln!("  Run 'nemesisbot onboard default' to create one.");
        std::process::exit(1);
    }

    // Step 3: Check home directory exists
    if !home.exists() {
        eprintln!(
            "Error: Configuration directory not found: {}",
            home.display()
        );
        eprintln!("  Run 'nemesisbot onboard default' to create configuration.");
        std::process::exit(1);
    }

    // Step 3a: Ensure exe directory is in PATH so LLM shell tools can find nemesisbot
    if common::ensure_exe_in_path() {
        tracing::info!("[Gateway] Added exe directory to PATH for LLM shell access");
    }

    // Step 4: Load configuration into the runtime cache (single source of
    // truth). `cfg` is a startup snapshot for one-time reads below; live
    // consumers (executor.sandbox, …) read `config_store.handle()` so toggles
    // flip without a gateway restart.
    let config_store = std::sync::Arc::new(
        nemesis_config::ConfigStore::load(&config_path)
            .map_err(|e| anyhow::anyhow!("Error loading config: {}", e))?,
    );
    // Install the process-wide singleton so WSAPI handlers (sandbox/config/
    // channels…) reach the same live config without AppState wiring. A
    // dashboard write through the store is visible to every consumer —
    // including the executor's sandbox probe — on the next read, no restart.
    nemesis_config::set_global(config_store.clone());
    let cfg = config_store.handle().read().clone();

    // Step 4b: Ensure the Sandboxie engine is ready (Route A) — driver and
    // service are judged SEPARATELY, so this never triggers the per-run
    // driver-install UAC. Steady-state (both resident) is a reuse no-op; only
    // the lightweight "start service if stopped" path may run.
    #[cfg(feature = "sandbox")]
    {
        let sandbox_enabled = cfg.executor.as_ref().is_some_and(|ec| ec.sandbox);
        crate::commands::sandbox::ensure_sandbox_ready(&home, sandbox_enabled);
    }

    // [capture] Initialize the diagnostic capture sink — failure-triggered
    // only (zero happy-path overhead). Reads `debug.capture.enabled`
    // (defaults to true when unset). Evidence lands in
    // `{workspace}/logs/capture/{session_key}/{ts}_{signal}/` only when a
    // failure signal fires (LLM retry exhausted / context overflow / session
    // overwrite / agent error funnel). Diagnostic only — does not change any
    // control flow or business logic.
    {
        let capture_enabled = cfg.debug.as_ref().is_none_or(|d| d.capture.enabled);
        nemesis_agent::capture_sink::CaptureSink::init(home.join("workspace"), capture_enabled);
        if capture_enabled {
            info!("[Gateway] Diagnostic capture armed (failure-triggered → logs/capture/)");
        }
    }

    // Step 5: Initialize logger from config
    let mut args: Vec<String> = std::env::args().skip(2).collect();
    args.extend(extra_args.iter().cloned());
    let _log_flags = common::init_logger_from_config(&config_path, &args);

    // Step 6: Write gateway state file (PID only; web_port updated after bind)
    let pid = std::process::id();
    {
        let state_dir =
            nemesis_path::resolve_state_dir_in_workspace(&common::workspace_path(&home));
        if let Err(e) = std::fs::create_dir_all(&state_dir) {
            warn!("[Gateway] Failed to create state dir: {}", e);
        }
        let state_path = state_dir.join("gateway.json");
        let state_json = serde_json::json!({
            "pid": pid,
            "web_host": "",
            "web_port": 0,
        });
        if let Err(e) = std::fs::write(&state_path, state_json.to_string()) {
            warn!("[Gateway] Failed to write gateway state: {}", e);
        } else {
            info!(
                "[Gateway] Gateway state written: {} (PID: {})",
                state_path.display(),
                pid
            );
        }
    }

    // Step 7: Resolve the default LLM model and create provider
    let llm_ref = nemesis_config::get_effective_llm(Some(&cfg));
    let resolution = nemesis_config::resolve_model_config(&cfg, &llm_ref)
        .map_err(|e| anyhow::anyhow!("Failed to resolve model '{}': {}", llm_ref, e))?;

    // Build the LLM provider once. The same Arc<dyn LLMProvider> is reused by
    // the workflow engine (milestone 1a-E1, so workflow `llm` nodes route to
    // the same model) and the security guardian judge. The main agent loop
    // builds its own provider, so this is only needed when workflow or security
    // is enabled.
    #[cfg(any(feature = "workflow", feature = "security"))]
    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key.clone(),
        api_base: resolution.api_base.clone(),
        workspace: home.join("workspace").to_string_lossy().to_string(),
        connect_mode: resolution.connect_mode.clone(),
        protocol: resolution.protocol.clone(),
        account_id: String::new(),
        headers: std::collections::HashMap::new(),
    };
    #[cfg(any(feature = "workflow", feature = "security"))]
    let llm_provider: Arc<dyn nemesis_providers::router::LLMProvider> =
        nemesis_providers::factory::create_provider(&factory_cfg)
            .map_err(|e| anyhow::anyhow!("Failed to create provider: {}", e))?;
    #[cfg(any(feature = "workflow", feature = "security"))]
    {
        info!("[Gateway] Provider config validated for {}", llm_ref);
    }

    let model_name = resolution.model_name.clone();

    // --- Workflow Engine (milestone 1a-E1) ---
    // Build an integrated engine that wires RealLLMNodeExecutor (so llm nodes
    // invoke the same provider as the agent) and RealToolNodeExecutor (so tool
    // nodes can dispatch to any later-registered tools). All workflow files
    // live under {home}/workspace/workflow/ with four subdirs:
    //   definitions/  - YAML workflow definitions (loaded at startup)
    //   templates/    - starter templates for the CLI workflow command
    //   checkpoints/  - resume snapshots for in-flight recovery
    //   executions/   - JSONL execution logs
    // Migrate any pre-refactor data from {home}/workflow/ first.
    #[cfg(feature = "workflow")]
    let workflow_engine: std::sync::Arc<nemesis_workflow::engine::WorkflowEngine>;
    #[cfg(feature = "workflow")]
    let chat_secret_store: std::sync::Arc<nemesis_workflow::chat_secrets::ChatSecretStore>;
    // Tool registry shared with the workflow engine. Declared in the outer
    // scope (not inside the workflow-init block below) so we can populate it
    // *after* the agent loop is built — the agent's tools must be bridged in
    // via `AgentToolAdapter` (the two `Tool` traits are incompatible, so the
    // workflow registry cannot share the agent's map directly).
    #[cfg(feature = "workflow")]
    let workflow_tool_registry: std::sync::Arc<nemesis_tools::registry::ToolRegistry>;
    #[cfg(feature = "workflow")]
    {
        let workflow_root = home.join("workspace").join("workflow");
        let workflow_executions_dir = workflow_root.join("executions");
        let workflow_checkpoints_dir = workflow_root.join("checkpoints");
        let workflow_defs_dir = workflow_root.join("definitions");
        for d in [
            &workflow_executions_dir,
            &workflow_checkpoints_dir,
            &workflow_defs_dir,
        ] {
            if let Err(e) = std::fs::create_dir_all(d) {
                warn!(
                    "[Gateway] Failed to create workflow subdir {}: {}",
                    d.display(),
                    e
                );
            }
        }
        migrate_legacy_workflow_dir(&home, &workflow_executions_dir, &workflow_checkpoints_dir);

        workflow_tool_registry = Arc::new(nemesis_tools::registry::ToolRegistry::new());
        let engine = nemesis_workflow::engine::WorkflowEngine::new_integrated_with_dirs(
            llm_provider.clone(),
            workflow_tool_registry.clone(),
            Some(workflow_executions_dir.clone()),
            Some(workflow_checkpoints_dir.clone()),
        );

        // Load all workflow definitions from {home}/workspace/workflow/definitions/.
        engine.set_workflow_defs_dir(workflow_defs_dir.clone());

        // U10 统一执行世界：workflow script 节点（无 registry 的装配路径）、
        // per-node `sandbox: false` 的受守卫直跑、引擎控制面写盘（persist/
        // delete 根外拒绝）都经同一个 ExecutionWorld。executor 分离开（默认）
        // → 无 world（行为不变）；开 → 与 agent 工具层同一条开关链
        // （executor.enabled / executor.sandbox，live probe）。
        // 注意：gateway 的 script 节点主路径仍走注册表车道（AgentToolAdapter
        // 桥接的 agent 工具 —— 已是 RemoteExecutorTool 包装，Layer 1/2 生效）；
        // world 提供的是 CLI 侧同能力 + 引擎 IO 守卫 + spawn 车道。
        #[cfg(feature = "sandbox")]
        {
            let workspace_dir = home.join("workspace");
            let spawn_roots = vec![workspace_dir.clone()];
            match crate::exec_world::build_workflow_world(
                &home,
                &workspace_dir,
                vec![
                    workflow_defs_dir.clone(),
                    workflow_checkpoints_dir.clone(),
                    workflow_executions_dir.clone(),
                ],
                spawn_roots,
                config_store.handle(),
            ) {
                Ok(Some(world)) => {
                    engine.set_execution_world(world);
                }
                Ok(None) => {
                    info!(
                        "[Gateway] executor separation off — workflow engine runs without an \
                         execution world (script nodes via tool registry, engine IO unguarded; \
                         pre-U10 behaviour)"
                    );
                }
                Err(e) => {
                    warn!(
                        "[Gateway] execution world build failed (engine continues without it): {}",
                        e
                    );
                }
            }
        }

        match engine.load_workflows_from_dir(&workflow_defs_dir) {
            Ok(n) => {
                info!(
                    "[Gateway] Workflow engine loaded {} definition(s) from {}",
                    n,
                    workflow_defs_dir.display()
                );
            }
            Err(e) => {
                warn!(
                    "[Gateway] Workflow engine load failed: {} (dir={})",
                    e,
                    workflow_defs_dir.display()
                );
            }
        }

        // Spawn cron-triggered workflows (milestone 1a-E2).
        let _workflow_cron_handles = engine.spawn_cron_triggers();
        let cron_wf_count = _workflow_cron_handles.len();
        if cron_wf_count > 0 {
            info!(
                "[Gateway] Workflow cron triggers registered: {}",
                cron_wf_count
            );
        }

        // Restore any in-flight executions paused at human_review nodes or
        // interrupted by a previous crash (milestone 1b-A1 step 7). The checkpoint
        // store lives under {home}/workspace/workflow/checkpoints/.
        match engine.restore_incomplete_executions().await {
            Ok(n) if n > 0 => {
                info!(
                    "[Gateway] Workflow engine restored {} in-flight execution(s) from checkpoints",
                    n
                );
            }
            Ok(_) => {}
            Err(e) => {
                warn!(
                    "[Gateway] Workflow checkpoint restore failed: {} (continuing with fresh state)",
                    e
                );
            }
        }
        workflow_engine = engine;

        // Per-workflow chat password store for the standalone workflow-chat page.
        // Loaded from {home}/workspace/workflow/chat_secrets.json — created on
        // first set_password call. Lives outside the workflow engine because
        // secrets shouldn't ride along with workflow YAML (which is shareable).
        let chat_secrets_path = workflow_root.join("chat_secrets.json");
        chat_secret_store = Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::open(
            chat_secrets_path,
        ));
    }

    // Step 8: Create MessageBus
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    info!("[Gateway] Message bus created");

    // Step 9: Create AgentLoop with mpsc channels (bridge to broadcast bus)
    // The AgentLoop uses mpsc channels, while the bus uses broadcast.
    // We bridge: bus inbound (broadcast) → mpsc inbound → AgentLoop
    //            AgentLoop → mpsc outbound → bus outbound (broadcast)
    //
    // Capacity is 1024 (up from 256) to reduce message loss under load.
    // The inbound bridge is created inside AgentLoopServiceAdapter::start().
    let (agent_outbound_tx, mut agent_outbound_rx) =
        tokio::sync::mpsc::channel::<nemesis_types::channel::OutboundMessage>(1024);

    // Bridge: agent outbound mpsc → bus outbound broadcast
    let bus_out = bus.clone();
    let bridge_outbound_handle = tokio::spawn(async move {
        while let Some(msg) = agent_outbound_rx.recv().await {
            bus_out.publish_outbound(msg);
        }
    });

    // The AgentLoop is now created by the factory function (agent_factory.rs).
    // provider, system prompt, AgentConfig, AgentLoop::new_bus, session store,
    // state manager, SharedToolConfig, tool registration, MCP, cluster_rpc,
    // continuation manager — all handled inside build_agent_loop().
    // agent_outbound_tx will be stored in SharedResources later.

    // agent_outbound_tx is moved into SharedResources below.
    // For now, keep it as a local variable.
    // State manager injection into agent_loop is now handled by the factory function.

    // Register all tools (mirrors Go's bot_service.go initComponents):
    //   default tools + web + cluster + spawn + memory + skills + hardware + exec + cron
    let cron_store_path = common::cron_store_path(&home);
    let cron_service = std::sync::Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(&cron_store_path.to_string_lossy()),
    ));

    // Swarm M3（§5.4）：本节点对外 web 基址槽（bind 后 set；G9 起可由
    // 自愈任务随集群注册表知识更新——多重网卡选对 NIC、DHCP 换 IP 自动
    // 跟随。dispatch 签发资产 bundle 时经 store 的 AssetSignContext 读取；
    // 未 set = 基址未解析，签发诚实跳过）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_asset_url_slot: nemesis_board::AdvertisedUrl =
        nemesis_board::AdvertisedUrl::default();

    // W2 P1: Managed-agent board store — open (or create) {workspace}/board/board.db.
    // Injected into the web server below; failure logs a warning and leaves the
    // board unavailable (board.* WSAPI commands return "board service not
    // available") instead of blocking gateway startup.
    #[cfg(feature = "board")]
    let board_store = {
        let board_db = home.join("workspace").join("board").join("board.db");
        match nemesis_board::BoardStore::open(&board_db, "NB") {
            Ok(store) => {
                // Swarm M2: 默认频道（#dev/#qa/#general）幂等种子——首启建，
                // 已有则 no-op。
                if let Err(e) = store.ensure_default_channels() {
                    warn!("[Gateway] Board default channels seed failed: {}", e);
                }
                info!("[Gateway] Board store opened at {}", board_db.display());
                Some(std::sync::Arc::new(store))
            }
            Err(e) => {
                warn!(
                    "[Gateway] Board store open failed: {} (board.* disabled)",
                    e
                );
                None
            }
        }
    };

    // Swarm M2: board 维护循环 —— 启动即清扫一次频道消息 + 备份一次
    // board.db，之后每 24h 重复。频道消息按 `board.discussion.retention_days`
    // 清扫（0=永久）；备份经 VACUUM INTO 快照到 workspace/backups/
    // （`board.backup.keep`=0 关闭备份）。
    #[cfg(feature = "board")]
    {
        let maint_cfg = cfg.board.clone().unwrap_or_default();
        let store_for_maint = board_store.clone();
        let workspace_dir = home.join("workspace");
        let db_path_for_maint = workspace_dir.join("board").join("board.db");
        let backups_dir = nemesis_path::resolve_board_backups_dir_in_workspace(&workspace_dir);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(86_400));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await; // 首 tick 立即返回 = 启动即维护
                let Some(store) = store_for_maint.as_ref() else {
                    continue;
                };
                let retention = maint_cfg.discussion.retention_days;
                match store.sweep_channel_messages(retention) {
                    Ok(0) => {}
                    Ok(n) => info!(
                        "[Gateway] Board channel sweep removed {} messages (retention={}d)",
                        n, retention
                    ),
                    Err(e) => warn!("[Gateway] Board channel sweep failed: {}", e),
                }
                match nemesis_board::backup::backup_database(
                    &db_path_for_maint,
                    &backups_dir,
                    maint_cfg.backup.keep as usize,
                ) {
                    Ok(Some(p)) => info!("[Gateway] Board backup written: {}", p.display()),
                    Ok(None) => {}
                    Err(e) => warn!("[Gateway] Board backup failed: {}", e),
                }
            }
        });
        info!(
            "[Gateway] Board maintenance loop armed (retention={}d, backup keep={})",
            maint_cfg.discussion.retention_days, maint_cfg.backup.keep
        );
    }

    // Swarm M3: master 侧讨论额度台账（进程内存态，重启清零=诚实边界）。
    // 配置三闸来自 board.discussion（0=该闸关闭）；经 live 句柄每次判定
    // 现读——config.json 热改额度即时生效（与 tier/hidden_tools 同语义）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_quota: std::sync::Arc<nemesis_board::quota::QuotaLedger> = {
        let config_handle = config_store.handle();
        std::sync::Arc::new(nemesis_board::quota::QuotaLedger::with_provider(
            move || {
                let guard = config_handle.read();
                let disc = guard.board.clone().unwrap_or_default().discussion;
                nemesis_board::quota::QuotaConfig {
                    max_agent_turns_per_thread: disc.max_agent_turns_per_thread,
                    hourly_budget_per_node: disc.hourly_budget_per_node,
                    rate_limit_per_min: disc.rate_limit_per_min,
                }
            },
        ))
    };

    // Swarm M3: 主持人裁决用主 AgentLoop 后置装配桥（nb_bus 注册早于
    // agent_loop 构建；build 完成后 set()）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_moderator_loop: std::sync::Arc<
        std::sync::OnceLock<std::sync::Arc<nemesis_agent::r#loop::AgentLoop>>,
    > = std::sync::Arc::new(std::sync::OnceLock::new());

    // Swarm M3（G4/G8）：worker 侧讨论事件入站箱（本节点不是 board 权威
    // 时创建；board feature 裁剪下恒 None——adapter 不接讨论通道）。
    #[cfg(feature = "cluster")]
    #[allow(unused_mut)]
    let mut board_worker_inbox: Option<std::sync::Arc<crate::cluster_agent::DiscussionInbox>> =
        None;

    // Opt 2: conversation→WS router, shared between the cron fire handler
    // (lookup, here) and process_messages (bind, in the web server). Built
    // early so the cron closure can capture a clone before set_on_job; the
    // original Arc is later moved into the web server via set_conv_router.
    let conv_router: nemesis_web::SharedConvRouter =
        std::sync::Arc::new(nemesis_web::ConvRouter::new());

    // W2 P4: board autopilot 的集群槽位。on_job 闭包在 cluster 创建之前
    // 装配（cron 服务先于 cluster 就绪），用 OnceLock 让闭包在 cluster 建
    // 好后取用；未启用集群时保持 None（target 为空的 autopilot 规则仍可
    // 纯本地建单）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    let autopilot_cluster_slot: Arc<
        std::sync::OnceLock<Arc<nemesis_cluster::cluster::Cluster>>,
    > = Arc::new(std::sync::OnceLock::new());

    // C3: Wire CronService — set_on_job handler + start.
    // Mirrors Go's bot_service.go:392-399, 571-579.
    {
        let bus_for_cron = bus.clone();
        let router_for_cron = conv_router.clone();
        // W2 P4: autopilot 分支捕获（board store + 集群槽位）。
        #[cfg(feature = "board")]
        let store_for_ap = board_store.clone();
        #[cfg(all(feature = "board", feature = "cluster"))]
        let slot_for_ap = autopilot_cluster_slot.clone();
        // 全自动流转 D2：auto_plan 的 moderator 槽（board_moderator_loop 在
        // 本闭包装配前创建、agent_loop 建成后 set——同 OnceLock 模式）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        let mod_slot_for_ap = board_moderator_loop.clone();
        #[cfg(all(feature = "board", feature = "cluster"))]
        let home_for_ap = home.clone();
        cron_service
            .lock()
            .unwrap()
            .set_on_job(move |job: &nemesis_cron::service::CronJob| {
                // W2 P4: board autopilot job（名 `board-ap:{id}`）→ 模板建单
                // +（可选）派发，不走消息总线。必须放在 message 判空前——
                // autopilot job 的 message 恒为空，否则落 "No message to
                // deliver"。
                #[cfg(feature = "board")]
                if job.name.starts_with("board-ap:") {
                    #[cfg(feature = "cluster")]
                    {
                        // auto_plan 上下文在触发时现构（槽引用 + home + 集群；
                        // hub 传 None——闭包装配早于 web server，SSE 诚实降级）。
                        let ap_ctx = nemesis_web::handlers::board::AutoPlanContext {
                            moderator_slot: mod_slot_for_ap.clone(),
                            home: home_for_ap.clone(),
                            hub: None,
                            cluster: slot_for_ap.get().cloned(),
                        };
                        return fire_board_autopilot(
                            &job.name,
                            store_for_ap.as_ref(),
                            slot_for_ap.get(),
                            Some(&ap_ctx),
                        );
                    }
                    #[cfg(not(feature = "cluster"))]
                    return fire_board_autopilot(&job.name, store_for_ap.as_ref());
                }
                if !job.payload.message.is_empty() {
                    let channel = job
                        .payload
                        .channel
                        .clone()
                        .unwrap_or_else(|| "web".to_string());
                    // Phase 2: target the named conversation so the exchange is
                    // persisted into its history (loop.rs adopts `agent:`-prefixed
                    // session_key verbatim). Opt 2: if a live WS tab is bound for
                    // this conversation, set chat_id = web:<ws_id> so the reply
                    // also live-pushes; otherwise chat_id falls back to `to`
                    // (delivery may fail-soft, but history is still saved).
                    let session_key = job.payload.session_key.clone().unwrap_or_default();
                    let chat_id = if !session_key.is_empty() {
                        router_for_cron
                            .target(&session_key)
                            .unwrap_or_else(|| job.payload.to.clone().unwrap_or_default())
                    } else {
                        job.payload.to.clone().unwrap_or_default()
                    };
                    let inbound = nemesis_types::channel::InboundMessage {
                        channel,
                        sender_id: format!("cron:{}", job.id),
                        chat_id,
                        content: job.payload.message.clone(),
                        media: vec![],
                        session_key,
                        correlation_id: String::new(),
                        metadata: {
                            let mut m = std::collections::HashMap::new();
                            m.insert("cron_job_id".to_string(), job.id.clone());
                            m.insert("cron_job_name".to_string(), job.name.clone());
                            // T3 (U12): per-fire tool-round budget — the agent
                            // loop reads this and caps the continuation turn's
                            // tool iterations at this value (graceful stop via
                            // the grace-round path; job survives).
                            if let Some(mr) = job.payload.max_rounds {
                                m.insert("cron_max_rounds".to_string(), mr.to_string());
                            }
                            m
                        },
                        voice_playback: None,
                    };
                    bus_for_cron.publish_inbound(inbound);
                    Ok(format!("Cron job '{}' triggered", job.name))
                } else {
                    Ok("No message to deliver".to_string())
                }
            });
        info!("[Gateway] Cron service handler wired (publishes to bus; Opt2 conv_router attached)");
    }

    // Create Forge executor (always create instance for runtime toggle support).
    // M2 + M3 + L1 + L2 + M4 all wired here.
    #[cfg(feature = "forge")]
    let forge_enabled = cfg.forge.as_ref().map(|f| f.enabled).unwrap_or(false);
    #[cfg(feature = "forge")]
    let forge_for_web: Option<std::sync::Arc<nemesis_forge::forge::Forge>>;
    #[cfg(feature = "forge")]
    let forge_executor_for_tools: Option<
        std::sync::Arc<nemesis_forge::forge_tools::ForgeToolExecutor>,
    >;
    #[cfg(feature = "forge")]
    {
        // Load forge config from file, fall back to defaults if missing.
        // 委托 nemesis-path 唯一拼接点（与 web forge handler / CLI 同源）。
        let forge_config_path =
            nemesis_path::resolve_forge_config_path_in_workspace(&common::workspace_path(&home));
        let mut forge_config = if forge_config_path.exists() {
            nemesis_forge::config::load_forge_config(&forge_config_path)
        } else {
            nemesis_forge::config::ForgeConfig::default()
        };
        // F-P1 truth-source fix: the master switch is config.json's `forge.enabled`
        // (-> `forge_enabled`, which gates the background tasks). Mirror it into
        // forge_config so `Forge::is_enabled()` (the per-tool-call recording
        // gate) reflects the real runtime state. config.forge.json is often
        // absent (default enabled=false) — without this, recording would be
        // silently blocked even when forge is on.
        forge_config.enabled = forge_enabled;
        let forge_workspace = home.join("workspace");
        let forge_dir = forge_workspace.join("forge");
        let mut forge = nemesis_forge::forge::Forge::new(forge_config.clone(), forge_workspace);

        // Initialize Reflector (statistical analysis + report writing).
        forge.init_reflector(nemesis_forge::reflector::Reflector::with_reflections_dir(
            forge_dir.join("reflections"),
        ));
        info!("[Gateway] Forge reflector initialized");

        // ONE shared registry for the Phase 6 closed loop (pipeline + monitor +
        // learning engine). Previously each got its own empty Registry (default
        // relative index_path), so deploy/monitor/feedback operated on disjoint
        // stores. (F-C3) Dedicated path so it persists + reloads; separate from
        // Forge's manual-create registry.json to avoid a two-instance collision.
        let forge_shared_registry = std::sync::Arc::new(nemesis_forge::registry::Registry::new(
            nemesis_forge::types::RegistryConfig {
                index_path: forge_dir
                    .join("learning_registry.json")
                    .to_string_lossy()
                    .to_string(),
            },
        ));
        // F-D3: reload prior learned artifacts so they survive restart.
        let _ = forge_shared_registry.load().await;

        // Initialize Pipeline (3-stage validation). Built as Arc + sharing the
        // closed-loop registry so it can be injected into the LearningEngine (F-C1).
        let forge_pipeline = std::sync::Arc::new(nemesis_forge::pipeline::Pipeline::new(
            forge_config.clone(),
            forge_shared_registry.clone(),
        ));
        forge.init_pipeline(forge_pipeline.clone());
        info!("[Gateway] Forge pipeline initialized");

        // Initialize trace collection (TraceCollector + TraceStore).
        {
            let trace_collector = nemesis_forge::trace::TraceCollector::new();
            let trace_store = nemesis_forge::trace_store::TraceStore::new(forge_dir.join("traces"));
            forge.init_trace(trace_collector, trace_store);
            info!("[Gateway] Forge trace collection initialized");
        }

        // Initialize learning engine (Phase 6 closed-loop). Shares the same
        // registry as pipeline + monitor (F-C3); init_learning injects the
        // pipeline + monitor into the engine so the loop runs (F-C1).
        let cycle_store = nemesis_forge::cycle_store::CycleStore::new(&forge_dir);
        let learning_engine = nemesis_forge::learning_engine::LearningEngine::with_forge_dir(
            forge_config.clone(),
            forge_dir.clone(),
            forge_shared_registry.clone(),
            cycle_store,
        );
        let cycle_store_for_init = nemesis_forge::cycle_store::CycleStore::new(&forge_dir);
        let forge_monitor = std::sync::Arc::new(nemesis_forge::monitor::DeploymentMonitor::new(
            forge_config.clone(),
            forge_shared_registry.clone(),
        ));
        forge.init_learning(learning_engine, forge_monitor, cycle_store_for_init);
        info!("[Gateway] Forge learning engine initialized (Phase 6; pipeline+monitor injected)");

        // Set bridge → init syncer.
        forge.set_bridge(std::sync::Arc::new(nemesis_forge::bridge::NoOpBridge::new(
            "local".to_string(),
        )));
        forge.init_syncer();
        info!("[Gateway] Forge syncer initialized");

        // Set LLM provider — now handled by the factory function (agent_factory.rs).

        let forge = std::sync::Arc::new(forge);

        // F-D3: reload Forge's manual-create registry so prior artifacts survive restart.
        let _ = forge.registry().load().await;
        // F-C1: inject skill_creator into the learning engine (needs Arc<Forge>,
        // which implements SkillCreator). pipeline+monitor were injected in
        // init_learning; this completes the Phase 6 wiring.
        if let Some(le) = forge.learning_engine() {
            le.set_skill_creator(forge.clone());
            info!("[Gateway] Forge learning engine wired: skill_creator injected");
        }

        // LearningEngine dependency injection is now handled by the factory function.

        let executor = std::sync::Arc::new(nemesis_forge::forge_tools::ForgeToolExecutor::new(
            forge.clone(),
        ));
        info!("[Gateway] Forge executor created (8 tools will be registered)");

        // Forge injection into agent_loop is now handled by the factory function.

        // Start background tasks only if enabled in config.
        if forge_enabled {
            let forge_for_start = forge.clone();
            tokio::spawn(async move {
                forge_for_start.start().await;
            });
            info!("[Gateway] Forge started (background tasks running)");
        } else {
            info!("[Gateway] Forge created but not started (enabled=false in config)");
        }

        // Store for web server injection.
        forge_for_web = Some(forge);
        forge_executor_for_tools = Some(executor);
    }

    let mcp_enabled = cfg.mcp.as_ref().map(|m| m.enabled).unwrap_or(false);

    #[cfg(feature = "memory")]
    let mut memory_manager_for_web: Option<
        std::sync::Arc<nemesis_memory::manager::MemoryManager>,
    > = None;

    let skills_loader_arc: Option<std::sync::Arc<nemesis_skills::loader::SkillsLoader>> = {
        let workspace_str = home.join("workspace").to_string_lossy().to_string();
        let global_skills_str = home
            .join("workspace")
            .join("skills")
            .to_string_lossy()
            .to_string();
        let loader =
            nemesis_skills::loader::SkillsLoader::new(&workspace_str, &global_skills_str, "");
        info!(
            "[Gateway] Skills loader created (workspace={}, global_skills={})",
            workspace_str, global_skills_str
        );
        Some(std::sync::Arc::new(loader))
    };

    let skills_registry_arc: Option<std::sync::Arc<nemesis_skills::registry::RegistryManager>> = {
        // 委托 nemesis-path 唯一拼接点。
        let skills_config_path =
            nemesis_path::resolve_skills_config_path_in_workspace(&common::workspace_path(&home));
        if skills_config_path.exists() {
            match std::fs::read_to_string(&skills_config_path) {
                Ok(content) => {
                    match serde_json::from_str::<nemesis_skills::types::RegistryConfig>(&content) {
                        Ok(reg_config) => {
                            let rm =
                                nemesis_skills::registry::RegistryManager::from_config(reg_config);
                            info!(
                                "[Gateway] Skills registry loaded from {}",
                                skills_config_path.display()
                            );
                            Some(std::sync::Arc::new(rm))
                        }
                        Err(e) => {
                            warn!(
                                "[Gateway] Failed to parse skills config: {} — skills search/install disabled",
                                e
                            );
                            None
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "[Gateway] Failed to read skills config: {} — skills search/install disabled",
                        e
                    );
                    None
                }
            }
        } else {
            info!(
                "[Gateway] No skills config found at {} — skills search/install disabled",
                skills_config_path.display()
            );
            None
        }
    };

    // Create MemoryManager (still needed for web server injection).
    // Memory tool executor creation is now handled by the factory function.
    #[cfg(feature = "memory")]
    {
        if cfg.memory.as_ref().map(|m| m.enabled).unwrap_or(false) {
            let memory_data_dir = home.join("workspace").join("memory_vector");
            let config_dir = home.join("workspace").join("config");
            let mgr = std::sync::Arc::new(nemesis_memory::manager::MemoryManager::with_config_dir(
                &memory_data_dir,
                &config_dir,
            ));
            info!(
                "[Gateway] Memory manager created (data_dir={})",
                memory_data_dir.display()
            );
            memory_manager_for_web = Some(mgr);
        } else {
            info!("[Gateway] Enhanced memory disabled (config.json: memory.enabled = false)");
        }
    }

    // Web search config: compute for reference, but tool registration is handled by factory.
    {
        let web = &cfg.tools.web;
        let any_enabled = web.brave.enabled || web.duckduckgo.enabled || web.perplexity.enabled;
        if any_enabled {
            info!(
                "[Gateway] Web search enabled (brave={}, duckduckgo={}, perplexity={})",
                web.brave.enabled, web.duckduckgo.enabled, web.perplexity.enabled
            );
        } else {
            info!("[Gateway] Web search disabled (no provider enabled in config.json: tools.web)");
        }
    }

    // SharedToolConfig construction, tool registration, and MCP reload are now handled
    // by the factory function (agent_factory.rs build_agent_loop()).

    if !mcp_enabled {
        info!("[Gateway] MCP disabled in config.json (mcp.enabled = false), skipping");
    }
    info!(
        "[Gateway] Agent loop tools configured (default + memory + skills + hardware + exec + cron{})",
        if mcp_enabled { " + MCP" } else { "" }
    );

    // Step 9b: Create DataStore for usage statistics（E1 二期：前移到集群回调
    // 装配点之前——peer_chat_callback 闭包要捕获它，把 worker 回传的 usage
    // 记入 master 用量账本）
    let data_store = {
        let data_dir = nemesis_path::workspace_data_dir(&home);
        let db_path = data_dir.join("nemesisbot_data.db");
        match nemesis_data::DataStore::open(&db_path) {
            Ok(store) => {
                info!("[Gateway] DataStore opened at {}", db_path.display());
                Some(Arc::new(store))
            }
            Err(e) => {
                warn!("[Gateway] Failed to open DataStore: {e}, usage statistics disabled");
                None
            }
        }
    };

    // Step 9a: Set up cluster.
    // Mirrors Go's bot_service.go initComponents → startCluster.
    // The Cluster object and adapter are always created for dynamic start/stop support.
    // Network components (RPC server, discovery) only start when both config flags are enabled.
    #[cfg(feature = "cluster")]
    let cluster_master_enabled = cfg.cluster.as_ref().map(|c| c.enabled).unwrap_or(false);
    #[cfg(feature = "cluster")]
    let cluster_app_cfg = nemesis_cluster::config_loader::load_app_config(&home.join("workspace"));
    #[cfg(feature = "cluster")]
    let cluster_should_start = cluster_master_enabled && cluster_app_cfg.enabled;

    // Cluster RPC resources — filled inside the cluster block below, consumed by SharedResources.
    #[allow(unused_mut)] // mut only needed when feature="cluster" assigns these in the init block
    let mut cluster_rpc_call_fn: Option<
        Arc<
            dyn Fn(
                    &str,
                    &str,
                    serde_json::Value,
                ) -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
                > + Send
                + Sync,
        >,
    > = None;
    #[allow(unused_mut)]
    let mut cluster_rpc_config: Option<nemesis_agent::ClusterRpcConfig> = None;
    #[allow(unused_mut)]
    let mut cluster_peers_fn: Option<
        Arc<dyn Fn() -> Vec<(String, String, Vec<String>)> + Send + Sync>,
    > = None;
    // Cluster adapter — manages dynamic start/stop of all cluster components.
    #[cfg(feature = "cluster")]
    let mut cluster_adapter: Option<Arc<crate::cluster_service::ClusterServiceAdapter>> = None;
    // Cluster refs saved during init, used to create adapter after SharedResources is built.
    #[cfg(feature = "cluster")]
    #[allow(unused_assignments)] // always overwritten by the cluster init block below
    let mut cluster_adapter_refs: Option<(
        Arc<nemesis_cluster::cluster::Cluster>,
        Arc<nemesis_cluster::ClusterTaskList>,
        Arc<nemesis_cluster::ClusterWorkQueue>,
        Arc<dyn nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister>,
    )> = None;
    // Always create cluster infrastructure (Cluster object, handlers, adapter refs).
    // Network components are started below only when cluster_should_start is true.
    // P1/T1-6 estop 保险丝：句柄在集群装配前创建——peer_chat_callback 里的
    // board 评审依赖集与下方 SharedResources 共享同一 Arc（跨 agent 重启存活）。
    let estop = std::sync::Arc::new(nemesis_agent::estop::EstopState::new());
    info!("[Gateway] Global e-stop (kill switch) initialized (released)");
    #[cfg_attr(
        not(all(feature = "board", feature = "cluster")),
        allow(dead_code, unused_variables)
    )]
    let board_estop_parked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(
        crate::board_review::ParkedKind,
        i64,
    )>::new()));
    // P4/B2b 自检取证路由表：selfcheck 派发不写 issue_dispatch，callback
    // 凭本表识别取证任务并路由到二段验收（评审任务与回调闭包共享）。
    #[cfg_attr(
        not(all(feature = "board", feature = "cluster")),
        allow(dead_code, unused_variables)
    )]
    let board_selfcheck_registry = crate::board_review::SelfcheckRegistry::new();
    #[cfg(feature = "cluster")]
    {
        // Build ClusterConfig — node_id 留空，with_workspace() 会从 peers.toml [node] 段加载真实身份
        let cluster_config = nemesis_cluster::types::ClusterConfig {
            node_id: String::new(),
            bind_address: format!("0.0.0.0:{}", cluster_app_cfg.rpc_port),
            peers: vec![],
        };

        let mut cluster = nemesis_cluster::cluster::Cluster::with_workspace(
            cluster_config,
            home.join("workspace"),
        );

        // Set ports and node info from app config
        cluster.set_ports(cluster_app_cfg.port, cluster_app_cfg.rpc_port);
        cluster.set_broadcast_interval(std::time::Duration::from_secs(
            cluster_app_cfg.broadcast_interval.max(1),
        ));
        cluster.set_node_type("agent");

        // Swarm M2: 节点发现 → 自动收编进看板频道（first-join 语义：
        // 成员在 channel_member 全表零行 = 全新节点才自动入；管理员手动
        // 调整过的不被 announce 拉回）。role=worker 且 category 含 qa →
        // #qa，其余 worker → #dev；coordinator / 本节点不入。注册在静态
        // peers 装载之前 —— 启动时静态对端同样收编。
        // A2 停车场 sweep：cluster 句柄经 OnceLock 回填（回调注册早于
        // Arc::new(cluster)，同 autopilot 槽位模式）；10s 节流抗 announce
        // 风暴；estop 挂起时不派发（急停冻结一切自动 agent 活动）。
        // None = 从未跑过（Option 防开机窗口 Instant 下溢）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        let (sweep_cluster_slot, sweep_last, estop_for_sweep) = {
            let slot: Arc<std::sync::OnceLock<Arc<nemesis_cluster::cluster::Cluster>>> =
                Arc::new(std::sync::OnceLock::new());
            let last: Arc<std::sync::Mutex<Option<std::time::Instant>>> =
                Arc::new(std::sync::Mutex::new(None));
            (slot, last, estop.clone())
        };

        #[cfg(all(feature = "board", feature = "cluster"))]
        {
            let store_for_hook = board_store.clone();
            let self_node_id = cluster.node_id().to_string();
            let sweep_cluster_slot = sweep_cluster_slot.clone();
            let sweep_last = sweep_last.clone();
            cluster.set_on_node_discovered(Arc::new(move |node_id, role, category| {
                // sweep 先于 auto-join 的 self/非 worker 早退：任何非本节点
                // announce（含身份/tags 变更刷新）都是重试信号；本节点自身
                // 的 announce 不是（派发匹配器本就排除本机）。
                // 触发闸（estop 短路 + 10s 节流）抽为 board_review::
                // park_sweep_gate（可测）：estop 挂起 → 拒且不消耗节流窗口。
                if node_id != self_node_id && sweep_cluster_slot.get().is_some() {
                    let throttle_ok = {
                        let mut last = sweep_last
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        crate::board_review::park_sweep_gate(
                            estop_for_sweep.is_engaged(),
                            &mut last,
                            std::time::Instant::now(),
                            std::time::Duration::from_secs(10),
                        )
                    };
                    if throttle_ok {
                        let store = store_for_hook.clone();
                        let cluster = sweep_cluster_slot.get().unwrap().clone();
                        let node_id = node_id.to_string();
                        tokio::spawn(async move {
                            if let Some(store) = store.as_ref() {
                                let actor = nemesis_board::Actor::system("board");
                                let (cands, dispatched, failed) =
                                    nemesis_web::handlers::board::sweep_parked_dispatches(store, &cluster, &actor);
                                if dispatched > 0 || failed > 0 {
                                    info!(
                                        "[Gateway] 停车场 sweep：候选 {cands} 派出 {dispatched} 失败 {failed}（节点 {} 上线/刷新触发）",
                                        node_id
                                    );
                                }
                            }
                        });
                    }
                }
                let Some(store) = store_for_hook.as_ref() else {
                    return;
                };
                if node_id == self_node_id || !role.eq_ignore_ascii_case("worker") {
                    return;
                }
                let channel_name = if category.to_lowercase().contains("qa") {
                    "#qa"
                } else {
                    "#dev"
                };
                let member = nemesis_board::Actor::agent(node_id);
                match store.has_any_channel_membership(&member) {
                    Ok(true) => {} // 已见过的成员：手动调整不被撤销
                    Ok(false) => match store.get_channel_by_name(channel_name) {
                        Ok(Some(ch)) => {
                            if let Err(e) = store.join_channel(ch.id, member) {
                                warn!("[Gateway] Board auto-join failed: {}", e);
                            } else {
                                info!(
                                    "[Gateway] Board: node {} auto-joined {}",
                                    node_id, channel_name
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(e) => warn!("[Gateway] Board auto-join lookup failed: {}", e),
                    },
                    Err(e) => warn!("[Gateway] Board auto-join probe failed: {}", e),
                }
            }));
        }

        // Load static peers from peers.toml into the registry
        // The peers.toml uses [peers.Key] table format (not [[peers]] array),
        // so we parse it manually.
        let peers_toml_path = common::cluster_dir(&home).join("peers.toml");
        if peers_toml_path.exists()
            && let Ok(content) = std::fs::read_to_string(&peers_toml_path)
            && let Ok(doc) = content.parse::<toml::Value>()
            && let Some(peers_table) = doc.get("peers").and_then(|v| v.as_table())
        {
            for (key, val) in peers_table {
                // sanitize_peer_key only replaces `.` and `:` now (both are valid TOML
                // bare key chars but ambiguous in key names). `-` and `_` are preserved
                // as-is per TOML v1.0.0 spec, so the key IS the peer_id — no reverse
                // mapping needed.
                let peer_id = key.clone();
                let addr = val.get("address").and_then(|v| v.as_str()).unwrap_or("");
                let name = val.get("name").and_then(|v| v.as_str()).unwrap_or(&peer_id);
                let role = val.get("role").and_then(|v| v.as_str()).unwrap_or("worker");
                let cat = val
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("general");
                let tags: Vec<String> = val
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                if addr.is_empty() {
                    continue;
                }
                // The address field contains UDP host:port (e.g., "127.0.0.1:11950").
                // Derive RPC port by convention: UDP port + 10000 (11949→21949).
                let (host, udp_port) = parse_host_port(addr);
                let rpc_port = if udp_port > 0 { udp_port + 10000 } else { 0 };
                let addresses = if host.is_empty() { vec![] } else { vec![host] };
                info!(
                    "[Gateway] Loading static peer: {} ({}) addr={} rpc_port={}",
                    name, peer_id, addr, rpc_port
                );
                cluster.handle_discovered_node(
                    &peer_id,
                    name,
                    addresses,
                    rpc_port,
                    role,
                    cat,
                    tags,
                    vec![],
                    "unknown",
                );
                // Mark as static/configured so flaky UDP discovery
                // (e.g. multi-node on one device) can't take it
                // offline via check_health staleness — the address
                // is known, only explicit removal/RPC-failure takes
                // it down.
                cluster.mark_peer_static(&peer_id);
            }
        }

        // --- Create and set RPC Server (before start, needs &mut self) ---
        let rpc_server_config = nemesis_cluster::rpc::server::RpcServerConfig {
            bind_address: format!("0.0.0.0:{}", cluster_app_cfg.rpc_port),
            ..Default::default()
        };
        cluster.set_rpc_server(Arc::new(nemesis_cluster::rpc::server::RpcServer::new(
            rpc_server_config,
        )));

        // Start cluster (registers local node, creates RPC client, starts sync/recovery loops)
        cluster.start();

        // 节点身份（node_id / node_name）由 with_workspace() 从 peers.toml [node] 段加载，
        // 这里直接从 cluster 拿真值供下游消费（ClusterRpcConfig.local_node_id、Forge 桥、日志）。
        let node_id = cluster.node_id().to_string();
        let node_name = cluster.node_name();
        info!(
            "[Gateway] Cluster started (node_id: {}, name: {}, udp: {}, rpc: {})",
            node_id, node_name, cluster_app_cfg.port, cluster_app_cfg.rpc_port
        );

        // Diagnostic: list registry contents after start
        {
            let all_nodes = cluster.list_nodes();
            for n in &all_nodes {
                info!(
                    "[Gateway] Registry node: {} (id={}) status={:?} addr={}",
                    n.base.name, n.base.id, n.status, n.base.address
                );
            }
        }

        // Register RPC handlers on the server
        if let Err(e) = cluster.register_basic_handlers() {
            warn!("[Gateway] Failed to register basic RPC handlers: {}", e);
        }

        // Start RPC server (network operation — only when cluster is fully enabled).
        if cluster_should_start {
            let rpc_server_ref = cluster.rpc_server().expect("rpc_server just set").clone();
            info!(
                "[Gateway] Starting RPC server on 0.0.0.0:{}",
                cluster_app_cfg.rpc_port
            );
            // Await start() synchronously — it binds the TCP listener and spawns the
            // accept loop, then returns. This ensures default handlers are registered
            // before we overwrite them below.
            if let Err(e) = rpc_server_ref.start().await {
                error!(
                    "[Gateway] RPC server error on port {}: {}",
                    cluster_app_cfg.rpc_port, e
                );
            }
            info!(
                "[Gateway] RPC server started on port {}",
                cluster_app_cfg.rpc_port
            );
        }

        // Now register custom peer_chat handler using PeerChatHandler.
        // NOTE: We create the handler here but register it AFTER Arc::new(cluster)
        // so the closure can capture the Arc and register the remote node in the registry.
        let result_store = cluster.result_store().clone();
        let node_id_for_handler = node_id.clone();
        let _node_name_for_handler = node_name.clone();

        let mut handler = nemesis_cluster::rpc::peer_chat_handler::PeerChatHandler::new(
            node_id_for_handler.clone(),
        );
        let llm_timeout = nemesis_cluster::rpc::peer_chat_handler::llm_timeout_from_config_secs(
            cluster_app_cfg.llm_timeout_secs,
        );
        handler.set_timeout(llm_timeout);

        // Create cluster agent work queue and task list.
        let cluster_data_dir = nemesis_path::workspace_data_dir(&home);
        let cluster_task_list = Arc::new(nemesis_cluster::ClusterTaskList::new(&cluster_data_dir));
        let cluster_work_queue = Arc::new(nemesis_cluster::ClusterWorkQueue::new(64));
        handler.set_cluster_queue(cluster_task_list.clone(), cluster_work_queue.clone());

        // Set RPC client for callbacks (after cluster.start() creates the client).
        let rpc_client = cluster.rpc_client_arc();
        if let Some(client) = rpc_client.clone() {
            handler.set_rpc_client(client);
        }

        // Set result persister for fallback when callback fails.
        // 2026-09-08 G1 收口：同一份 persister 同时交给 peer_chat_handler
        // （legacy 路径）与 cluster agent work-queue 路径（经
        // cluster_adapter_refs → ClusterServiceAdapter）。此前 work-queue
        // 路径（生产唯一路径）不接 persister：回调失败真结果不落盘 → G5
        // 恢复轮询只能拿到 running 占位；回调成功占位也不清理。
        let persister: Arc<dyn nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister> =
            Arc::new(ClusterResultPersisterAdapter {
                result_store: result_store.clone(),
                node_id: node_id_for_handler.clone(),
            });
        handler.set_result_persister(persister.clone());

        // We'll register the handler after Arc::new(cluster) below.
        let handler_arc = Arc::new(handler);
        // Register callback handler (placeholder — will be replaced after Arc::new below).
        // This placeholder just acknowledges receipt.
        {
            let _ = cluster.register_rpc_handler(
                "peer_chat_callback",
                Box::new(move |payload| {
                    let task_id = payload
                        .get("task_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    Ok(serde_json::json!({"status": "placeholder", "task_id": task_id}))
                }),
            );
        }

        let cluster = Arc::new(cluster);

        // W2 P4: 回填 autopilot 集群槽位（on_job 闭包经 OnceLock 取用）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        {
            let _ = autopilot_cluster_slot.set(cluster.clone());
            // A2 停车场 sweep 槽位回填（节点发现闭包经 OnceLock 取用）。
            // 启动期 announce 早于回填也不丢：下一个 announce 周期兜底。
            let _ = sweep_cluster_slot.set(cluster.clone());
        }

        // --- Inject cluster task queue into cluster for callback routing ---
        cluster.set_cluster_task_queue(cluster_task_list.clone(), cluster_work_queue.clone());

        // --- Swarm M3: nb_bus handler（board 信封协议）---
        // master 与 worker 同名注册 `nb_bus`，按信封 ns/op 路由；本节点是
        // master（coordinator 角色 + board_store 在手）才注册上行 op，否则装
        // worker 侧唤醒通道 + 周期 board.sync 补拉。master 判据必须用集群
        // 角色：board_store 是全员 open 的（每个节点都有本地 board.db 作
        // dashboard 视图），拿它当 master 判据会让 worker 注册错 handler、
        // 唤醒通道成死代码（2026-09-09 UAT T23 根因）。cluster 未启动时
        // 注册失败静默（register_rpc_handler 要求 running——与 peer_chat
        // 同款忽略策略），board.sync 兜底语义不受影响。board feature 裁剪
        // 形态不参与讨论。（dashboard 人工发言桥在下文 board_service 装配
        // 处接线。）
        #[cfg(all(feature = "board", feature = "cluster"))]
        let board_master_armed = board_store.is_some()
            && matches!(
                cluster.role().as_str(),
                "coordinator" | "master" | "manager"
            );
        #[cfg(all(feature = "board", feature = "cluster"))]
        if board_master_armed {
            let deps = crate::board_bus::MasterBusDeps {
                store: board_store.clone().expect("board_store checked above"),
                quota: board_quota.clone(),
                cluster: cluster.clone(),
                moderator_loop: board_moderator_loop.clone(),
            };
            match cluster.register_rpc_handler(
                nemesis_cluster::envelope::NB_BUS_ACTION,
                crate::board_bus::build_master_nb_bus_handler(deps),
            ) {
                Ok(()) => info!("[Gateway] Registered nb_bus handler (board envelope protocol)"),
                Err(e) => warn!("[Gateway] nb_bus handler registration skipped: {}", e),
            }
        } else {
            let inbox = Arc::new(crate::cluster_agent::DiscussionInbox::new());
            let deps = crate::board_bus::WorkerBusDeps {
                self_node_id: cluster.node_id().to_string(),
                node_name: cluster.node_name(),
                node_role: cluster.role(),
                node_category: cluster.category(),
                inbox: inbox.clone(),
                wake_state: Arc::new(crate::board_bus::WorkerWakeState::load_or_create(
                    nemesis_path::board_wake_state_path(&home),
                )),
            };
            match cluster.register_rpc_handler(
                nemesis_cluster::envelope::NB_BUS_ACTION,
                crate::board_bus::build_worker_nb_bus_handler(deps.clone()),
            ) {
                Ok(()) => info!("[Gateway] Registered worker nb_bus handler (board wake channel)"),
                Err(e) => warn!("[Gateway] worker nb_bus registration skipped: {}", e),
            }
            crate::board_bus::spawn_worker_sync_loop(deps, cluster.clone());
            board_worker_inbox = Some(inbox);
        }

        // --- Register peer_chat handler (needs Arc<Cluster> to register remote nodes) ---
        {
            let handler_ref = handler_arc.clone();
            let cluster_ref = cluster.clone();
            let _ = cluster.register_rpc_handler(
                "peer_chat",
                Box::new(move |payload| {
                    // Extract source node ID from RPC metadata injected by the server.
                    let source_node_id = payload
                        .get("_rpc")
                        .and_then(|r| r.get("from"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !source_node_id.is_empty() {
                        // Register the remote node in our registry so we can callback later.
                        // The remote node may not be known via UDP discovery yet (static peers
                        // use peer names, not node_ids). We use the RPC port from the payload
                        // (sent by the remote node's ClusterRpcTool).
                        if cluster_ref.get_peer(&source_node_id).is_none() {
                            let remote_rpc_port = payload
                                .get("_source_rpc_port")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(21949)
                                as u16;

                            cluster_ref.handle_discovered_node(
                                &source_node_id,
                                &source_node_id,
                                vec!["127.0.0.1".to_string()],
                                remote_rpc_port,
                                "worker",
                                "general",
                                vec![],
                                vec![],
                                "unknown",
                            );
                        }
                    }

                    // Pass RpcMeta to PeerChatHandler so it can read source_node_id from
                    // `rpc_meta.from` (authoritative wire-level sender ID) and chat_id from
                    // `payload._source.chat_id` (filled by the originating node's tasks_submit).
                    // Together these form the composite session_key `cluster_rpc:{node_id}/{chat_id}`
                    // for LLM conversation isolation.
                    let rpc_meta = nemesis_cluster::rpc::peer_chat_handler::RpcMeta {
                        from: if source_node_id.is_empty() {
                            None
                        } else {
                            Some(source_node_id.clone())
                        },
                    };
                    let h = handler_ref.clone();
                    let ack = h.handle(payload, Some(rpc_meta));
                    Ok(serde_json::to_value(&ack)
                        .unwrap_or_else(|_| serde_json::json!({"status": "error"})))
                }),
            );
            info!("[Gateway] Registered PeerChatHandler (async LLM + callback) for peer_chat");
        }

        // --- H4: 任务恢复 handler（query_task_result / confirm_task_delivery）---
        // gateway 装配不走 set_rpc_channel（该入口无人触发 →
        // register_peer_chat_handlers 不会执行），这里显式补注册，与
        // peer_chat/callback/task_cancel 并列。缺失时 A 侧
        // poll_stale_pending_tasks（120s 周期）永远收到 "no handler"，
        // B 重启丢结果后的 stale 恢复链路断裂（2026-09-01 跨机 E2E 实测）。
        cluster.register_task_recovery_handlers();
        info!(
            "[Gateway] Registered task recovery handlers (query_task_result/confirm_task_delivery)"
        );

        // --- W2 P4: task_cancel handler — per-task cancel ---
        // 取消指定 peer_chat 任务的执行：排队中的直接出队丢弃，运行中的经
        // running_tokens 广播取消（LLM 迭代间隙/工具派发前检查）。这是与
        // estop 正交的细粒度取消（estop 冻结全部 agent 活动，此处只停一个
        // 任务）；board issue.cancel 经此送达 worker。
        {
            let task_list_for_cancel = cluster_task_list.clone();
            let _ = cluster.register_rpc_handler(
                "task_cancel",
                Box::new(move |payload| {
                    let task_id = payload
                        .get("task_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if task_id.is_empty() {
                        return Err("missing field: task_id".to_string());
                    }
                    let outcome = task_list_for_cancel.cancel_task(task_id);
                    info!(
                        "[Gateway] task_cancel: task={} outcome={:?}",
                        task_id, outcome
                    );
                    Ok(serde_json::to_value(&outcome)
                        .unwrap_or_else(|_| serde_json::json!({"outcome": "error"})))
                }),
            );
            info!("[Gateway] Registered task_cancel handler (per-task cancel)");
        }

        // --- W2 P4: 派发超时 sweep（board 派发无人回报的兜底）---
        // board.dispatch_timeout_secs = 0 关闭；扫描间隔
        // dispatch_sweep_interval_secs（下限 1s）。失败处置（⛔ 评论 + 通知）
        // 在 sweep_dispatch_timeouts 内完成，赢 fail_dispatch 竞态才动账。
        #[cfg(all(feature = "board", feature = "cluster"))]
        {
            let sweep_cfg = cfg.board.clone().unwrap_or_default();
            if sweep_cfg.dispatch_timeout_secs > 0 {
                let store_for_sweep = board_store.clone();
                let cluster_for_sweep = cluster.clone();
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                        sweep_cfg.dispatch_sweep_interval_secs.max(1),
                    ));
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    loop {
                        ticker.tick().await;
                        let Some(store) = store_for_sweep.as_ref() else {
                            continue;
                        };
                        sweep_dispatch_timeouts(
                            store,
                            &cluster_for_sweep,
                            sweep_cfg.dispatch_timeout_secs,
                        );
                    }
                });
                info!(
                    "[Gateway] Board dispatch sweep armed (timeout={}s, interval={}s)",
                    sweep_cfg.dispatch_timeout_secs,
                    sweep_cfg.dispatch_sweep_interval_secs.max(1)
                );
            }
        }

        // --- Now that Cluster is Arc-wrapped, wire up the real callback handler ---
        // Routes callbacks to the correct destination:
        // 1. If the callback matches a ClusterAgent child task (nested cluster_rpc),
        //    inject it back into the ClusterAgent's work queue.
        // 2. Otherwise, publish to bus as cluster_continuation for the main AgentLoop.
        // 3. Update TaskManager task status (for dashboard-initiated peer_chat).
        {
            let bus_for_cb = bus.clone();
            let task_list_for_cb = cluster_task_list.clone();
            let work_queue_for_cb = cluster_work_queue.clone();
            let cluster_for_cb = cluster.clone();
            // E1 二期 token 回传：worker 回传的 usage 记入 master 用量账本
            //（session_key=cluster_rpc:{worker}/{task_id}，E1 token 预算闸
            // 按派发行精确聚合）。未装配 DataStore = 记账静默跳过。
            let ds_for_usage_cb = data_store.clone();
            // W2 P2 派发写回：board 派发的 task_id 命中 issue_dispatch →
            // 写回看板。board feature 未编译时占位（拦截整体被 cfg 掉）。
            #[cfg(feature = "board")]
            let board_store_for_cb = board_store.clone();
            // 交付线程超限汇报落资产需要 workspace 根（层 2 HTTP 资产目录）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let workspace_for_cb = home.join("workspace");
            // Swarm M4 验收 agent 读 config.json 旗标需要 home 根。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let home_for_cb = home.clone();
            // M4 评审用主 loop 桥槽——闭包外再 clone 一份（原 Arc 稍后
            // set(agent_loop) 还要用）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let moderator_loop_for_cb = board_moderator_loop.clone();
            // P1/T1-6：estop 保险丝随评审依赖集进回调闭包（冻结停车用）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let estop_for_cb = estop.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let estop_parked_for_cb = board_estop_parked.clone();
            // P4/B2b：取证路由表随回调闭包（selfcheck 命中判定）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let selfcheck_for_cb = board_selfcheck_registry.clone();
            #[cfg(not(feature = "board"))]
            #[allow(unused_variables)]
            let board_store_for_cb: Option<()> = None;
            let _ = cluster.register_rpc_handler("peer_chat_callback", Box::new(move |payload| {
                let task_id = payload
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let status = payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("success");
                let response = payload
                    .get("response")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // worker 身份走传输层：RPC server 派发前注入 `_rpc.from`
                //（server.rs enhancePayload 同款），payload 本体没有
                // source_node 字段——T37 真机实证空段。优先 _rpc.from，
                // 显式字段保留为前向兼容 fallback。
                let source_node = payload
                    .get("_rpc")
                    .and_then(|m| m.get("from"))
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        payload
                            .get("source_node")
                            .and_then(|v| v.as_str())
                    })
                    .unwrap_or("");
                // E1 二期：usage 可选字段（serde 兼容——旧 worker 无此字段
                // 不炸；只认 object 形态）。
                let usage = payload.get("usage").filter(|v| v.is_object());
                record_cluster_usage(
                    ds_for_usage_cb.as_ref(),
                    source_node,
                    task_id,
                    usage,
                );

                info!("[Gateway] peer_chat_callback received: task_id={}, status={}, from={}", task_id, status, source_node);

                // P4/B2b 自检取证拦截（先于一切路由）：命中 SelfcheckRegistry
                // 的 task_id 是取证任务（不写 issue_dispatch——Route 0 的
                // 写回不适用；不进 Route 2 续行 bus——无续行快照只会告警），
                // 路由到二段验收。TaskManager 收口（Route 3）照常——取证
                // 任务也是 submit_peer_chat 登记的，不收口留 ghost pending。
                #[cfg(all(feature = "board", feature = "cluster"))]
                let selfcheck_issue_id = if task_id.is_empty() {
                    None
                } else {
                    selfcheck_for_cb.take(task_id)
                };
                #[cfg(any(not(feature = "board"), not(feature = "cluster")))]
                let selfcheck_issue_id: Option<i64> = None;
                #[cfg(all(feature = "board", feature = "cluster"))]
                if let Some(sc_issue_id) = selfcheck_issue_id {
                    info!(
                        "[Gateway] peer_chat_callback selfcheck branch: task_id={}, issue={}",
                        task_id, sc_issue_id
                    );
                    crate::board_review::spawn_selfcheck_second_stage(
                        crate::board_review::BoardReviewDeps {
                            store: board_store_for_cb.clone().expect(
                                "selfcheck armed implies store present",
                            ),
                            workspace: workspace_for_cb.clone(),
                            home: home_for_cb.clone(),
                            moderator_loop: moderator_loop_for_cb.clone(),
                            cluster: cluster_for_cb.clone(),
                            estop: estop_for_cb.clone(),
                            estop_parked: estop_parked_for_cb.clone(),
                            selfcheck: selfcheck_for_cb.clone(),
                        },
                        sc_issue_id,
                        status.to_string(),
                        response.to_string(),
                    );
                    // TaskManager 状态收口（同 Route 3 语义）。
                    let result_value = serde_json::json!({
                        "status": status,
                        "response": response,
                        "source_node": source_node,
                    });
                    if status == "error" {
                        cluster_for_cb.fail_task(task_id, response);
                    } else {
                        cluster_for_cb.complete_task(task_id, result_value);
                    }
                    return Ok(serde_json::json!({"status": "received", "task_id": task_id}));
                }

                // Route 0: Board 派发写回（W2 P2）——命中 issue_dispatch 的
                // task_id 直接写回看板（结果评论 + 状态推进），且跳过 Route 2
                // 的 agent 续行（board 派发无续行快照）；TaskManager 状态更新
                // （Route 3）照常，board 派发也登记了本地 task。
                // Swarm M4：推进到 in_review 的写回携带评审触发目标，下方
                // spawn 验收 agent。
                #[cfg(all(feature = "board", feature = "cluster"))]
                let board_writeback = write_back_board_dispatch(
                    &board_store_for_cb,
                    &workspace_for_cb,
                    task_id,
                    status,
                    response,
                );
                #[cfg(all(feature = "board", feature = "cluster"))]
                let is_board_task = board_writeback.is_board_task;
                #[cfg(any(not(feature = "board"), not(feature = "cluster")))]
                let is_board_task = false;

                // Swarm M4 批作业：in_review 自动验收（三态处置 + FAIL 重派
                // 保险丝）。依赖集在此快照——moderator 桥槽/board store/
                // cluster 均为 Arc，评审任务运行时再解引用。
                #[cfg(all(feature = "board", feature = "cluster"))]
                if let Some(review_issue_id) = board_writeback.issue_for_review {
                    crate::board_review::spawn_board_review(
                        crate::board_review::BoardReviewDeps {
                            store: board_store_for_cb.clone().expect(
                                "board writeback armed implies store present",
                            ),
                            workspace: workspace_for_cb.clone(),
                            home: home_for_cb.clone(),
                            moderator_loop: moderator_loop_for_cb.clone(),
                            cluster: cluster_for_cb.clone(),
                            estop: estop_for_cb.clone(),
                            estop_parked: estop_parked_for_cb.clone(),
                            selfcheck: selfcheck_for_cb.clone(),
                        },
                        review_issue_id,
                    );
                }

                // Route 1: Check if this callback belongs to a ClusterAgent child task.
                // When the ClusterAgent's LLM generates a nested cluster_rpc, the child
                // task's callback must be routed back to the ClusterAgent work queue,
                // not to the main AgentLoop's continuation system.
                if !task_id.is_empty()
                    && let Some(parent_task_id) = task_list_for_cb.find_by_child_task_id(task_id) {
                        info!(
                            "[Gateway] Callback for child task {} matched ClusterAgent parent task {}, injecting result",
                            task_id, parent_task_id
                        );
                        task_list_for_cb.inject_callback(&parent_task_id, response);
                        if let Err(e) = work_queue_for_cb.submit(parent_task_id) {
                            warn!("[Gateway] Failed to submit resumed task to work queue: {}", e);
                        }
                        return Ok(serde_json::json!({"status": "received", "task_id": task_id}));
                    }

                // Route 2: Main AgentLoop continuation — publish to bus.
                // （board 派发任务跳过：无续行快照，进 bus 只会告警。）
                if !task_id.is_empty() && !is_board_task {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("status".to_string(), status.to_string());
                    metadata.insert("source_node".to_string(), source_node.to_string());
                    if status == "error" {
                        metadata.insert("error".to_string(), response.to_string());
                    }

                    let inbound = nemesis_types::channel::InboundMessage {
                        channel: "system".to_string(),
                        sender_id: format!("cluster_continuation:{}", task_id),
                        chat_id: String::new(),
                        content: response.to_string(),
                        media: vec![],
                        session_key: String::new(),
                        correlation_id: String::new(),
                        metadata,
                        voice_playback: None,
                    };
                    bus_for_cb.publish_inbound(inbound);
                    info!("[Gateway] Published cluster_continuation for task_id={}", task_id);
                }

                // Route 3: Update TaskManager task status (for dashboard-initiated peer_chat).
                if !task_id.is_empty() {
                    let result_value = serde_json::json!({
                        "status": status,
                        "response": response,
                        "source_node": source_node,
                    });
                    if status == "error" {
                        cluster_for_cb.fail_task(task_id, response);
                    } else {
                        cluster_for_cb.complete_task(task_id, result_value);
                    }
                }

                Ok(serde_json::json!({"status": "received", "task_id": task_id}))
            }));
        }

        // --- Inject MessageBus into Cluster for continuation flow ---
        // Cluster.handle_task_complete() publishes cluster_continuation messages
        // on the bus, which AgentLoop intercepts to resume from snapshots.
        {
            let bus_adapter = Arc::new(BusToClusterAdapter { bus: bus.clone() });
            cluster.set_message_bus(bus_adapter);
            info!("[Gateway] Cluster: message bus injected for continuation flow");
        }

        // --- Network components: only start when cluster is fully enabled ---
        if cluster_should_start {
            // Wire Forge-Cluster bridge
            #[cfg(feature = "forge")]
            {
                if let Some(ref forge_arc) = forge_for_web {
                    let cluster_bridge = ClusterForgeBridgeAdapter::new(node_id.clone());
                    forge_arc.set_bridge(Arc::new(cluster_bridge));
                    info!("[Gateway] Forge-Cluster bridge wired (node_id={})", node_id);
                }
            }

            // Start UDP Discovery Service (managed by Cluster)
            cluster.start_discovery(cluster.clone());
            info!(
                "[Gateway] UDP discovery started on port {}",
                cluster_app_cfg.port
            );

            // RPC server was already created and set above before start().
            // RPC client was already created by Cluster::start().

            // Create cluster_rpc config + RPC call function for SharedResources.
            // The factory function will create the ClusterRpcTool and register it.
            let rpc_cfg = nemesis_agent::ClusterRpcConfig {
                local_node_id: node_id.clone(),
                timeout_secs: 3600,
                local_rpc_port: cluster_app_cfg.rpc_port,
            };

            // Wire the RPC call function to use cluster.call_with_context_async
            let cluster_weak_for_rpc = Arc::downgrade(&cluster);
            let call_fn = std::sync::Arc::new(
                move |target: &str, action: &str, payload: serde_json::Value| {
                    let c = match cluster_weak_for_rpc.upgrade() {
                        Some(arc) => arc,
                        None => {
                            return Box::pin(async move {
                                Err("Cluster已关闭，RPC调用不可用".to_string())
                            })
                                as std::pin::Pin<
                                    Box<
                                        dyn std::future::Future<
                                                Output = Result<serde_json::Value, String>,
                                            > + Send,
                                    >,
                                >;
                        }
                    };
                    let t = target.to_string();
                    let a = action.to_string();
                    Box::pin(async move {
                        let bytes = c
                            .call_with_context_async(
                                &t,
                                &a,
                                payload,
                                std::time::Duration::from_secs(3600),
                            )
                            .await
                            .map_err(|e| e.to_string())?;
                        // Deserialize the response bytes to JSON Value
                        serde_json::from_slice::<serde_json::Value>(&bytes)
                            .map_err(|e| format!("Failed to parse RPC response: {}", e))
                    })
                        as std::pin::Pin<
                            Box<
                                dyn std::future::Future<Output = Result<serde_json::Value, String>>
                                    + Send,
                            >,
                        >
                },
            );

            // Store for SharedResources (factory function will register the tool).
            cluster_rpc_call_fn = Some(call_fn);
            cluster_rpc_config = Some(rpc_cfg);

            // cluster_rpc tool registration is now handled by the factory function.
            // The rpc_call_fn is stored here for SharedResources consumption.
            info!(
                "[Gateway] cluster_rpc tool created (node: {}, peers loaded from peers.toml)",
                node_name
            );

            // Build peers_fn: closure that returns online peers with capabilities
            // from the Cluster registry, EXCLUDING the local node. Used by
            // ClusterRpcTool's dynamic tool description so the LLM never sees
            // itself as a valid cluster_rpc target (prevents self-invocation loops).
            {
                let cluster_weak_for_peers = Arc::downgrade(&cluster);
                cluster_peers_fn = Some(Arc::new(move || match cluster_weak_for_peers.upgrade() {
                    Some(c) => c
                        .get_online_peers_excluding_self()
                        .into_iter()
                        .map(|p| (p.base.id, p.base.name, p.capabilities))
                        .collect(),
                    None => Vec::new(),
                }));
            }
        } else {
            info!(
                "[Gateway] Cluster initialized (inactive) — start via Dashboard or enable in config"
            );
        }

        // ContinuationManager injection into agent_loop is now handled by the factory function.

        // Save references for ClusterServiceAdapter creation (after SharedResources is built).
        // The adapter will be created later in the code where SharedResources is available.
        // For now, save the cluster-related Arc refs needed.
        cluster_adapter_refs = Some((
            cluster.clone(),
            cluster_task_list.clone(),
            cluster_work_queue.clone(),
            persister.clone(),
        ));
    }

    // C1: Create ChannelManager and wire it.
    // Mirrors Go's bot_service.go:333-344: create ChannelManager, register channels,
    // start dispatch loop, call agentLoop.SetChannelManager().

    // Create WebServer early so we can inject SessionManager into WebChannel.
    // G9：绑定地址与展示地址分离（见 web_bind_and_display_hosts）——集群
    // 场景 0.0.0.0 如实绑定所有网卡，资产 bundle 广告的 LAN IP 才为真。
    #[cfg(feature = "cluster")]
    let web_cluster_starts = cluster_should_start;
    #[cfg(not(feature = "cluster"))]
    let web_cluster_starts = false;
    let (web_bind_host, web_display_host) =
        web_bind_and_display_hosts(&cfg.channels.web.host, web_cluster_starts);
    let web_port = cfg.channels.web.port;
    let cors_origins = {
        let cors_path = common::cors_config_path(&home);
        if cors_path.exists() {
            match nemesis_web::cors::CORSManager::new(&cors_path) {
                Ok(mgr) => {
                    let mgr_cfg = mgr.config();
                    if mgr_cfg.development_mode {
                        info!("[Gateway] CORS: development_mode enabled, allowing all origins");
                        vec![]
                    } else {
                        let origins = mgr.list_origins();
                        info!(
                            "[Gateway] CORS: loaded {} allowed origins from {}",
                            origins.len(),
                            cors_path.display()
                        );
                        origins
                    }
                }
                Err(e) => {
                    warn!(
                        "[Gateway] Failed to load CORS config: {}, using permissive defaults",
                        e
                    );
                    vec![]
                }
            }
        } else {
            vec![]
        }
    };
    let static_files = crate::embedded::resolve_static_files();
    let web_config = nemesis_web::server::WebServerConfig {
        listen_addr: format!("{}:{}", web_bind_host, web_port),
        auth_token: cfg.channels.web.auth_token.clone(),
        cors_origins,
        ws_path: "/ws".to_string(),
        workspace: Some(home.join("workspace").to_string_lossy().to_string()),
        home: Some(home.to_string_lossy().to_string()),
        version: crate::common::VERSION_INFO.version.to_string(),
        static_dir: None,
        static_files: Some(static_files),
        index_file: "index.html".to_string(),
    };
    let mut web_server = nemesis_web::server::WebServer::new(web_config);
    let web_server_ops = std::sync::Arc::new(crate::adapters::WebServerOpsAdapter::new(
        web_server.session_manager().clone(),
    ));

    // Build list of enabled channels from config (needed by SharedResources + ChannelManager).
    let mut enabled_channels = Vec::new();
    if cfg.channels.web.enabled {
        enabled_channels.push("web".to_string());
    }
    if cfg.channels.websocket.enabled {
        enabled_channels.push("websocket".to_string());
    }
    if cfg.channels.telegram.enabled {
        enabled_channels.push("telegram".to_string());
    }
    if cfg.channels.discord.enabled {
        enabled_channels.push("discord".to_string());
    }
    if cfg.channels.feishu.enabled {
        enabled_channels.push("feishu".to_string());
    }
    if cfg.channels.slack.enabled {
        enabled_channels.push("slack".to_string());
    }
    if cfg.channels.whatsapp.enabled {
        enabled_channels.push("whatsapp".to_string());
    }
    if cfg.channels.dingtalk.enabled {
        enabled_channels.push("dingtalk".to_string());
    }
    if cfg.channels.qq.enabled {
        enabled_channels.push("qq".to_string());
    }
    if cfg.channels.line.enabled {
        enabled_channels.push("line".to_string());
    }
    if cfg.channels.onebot.enabled {
        enabled_channels.push("onebot".to_string());
    }
    if cfg.channels.maixcam.enabled {
        enabled_channels.push("maixcam".to_string());
    }
    if cfg.channels.external.enabled {
        enabled_channels.push("external".to_string());
    }

    {
        let channel_manager = Arc::new(
            nemesis_channels::manager::ChannelManager::with_allowed_channels(
                enabled_channels.clone(),
            ),
        );

        // Build ChannelInitConfig from gateway config (web channel is always available).
        // Feature-gated channel fields only exist under some cfg combos, so the
        // `..Default::default()` base is required (not dead code); the lint can't see that.
        #[allow(clippy::needless_update)]
        let init_config = nemesis_channels::manager::ChannelInitConfig {
            web: if cfg.channels.web.enabled {
                Some(nemesis_channels::web::WebChannelConfig {
                    host: cfg.channels.web.host.clone(),
                    port: cfg.channels.web.port as u16,
                    ws_path: cfg.channels.web.path.clone(),
                    auth_token: cfg.channels.web.auth_token.clone(),
                    session_timeout_secs: cfg.channels.web.session_timeout as u64,
                    allow_from: cfg.channels.web.allow_from.clone(),
                })
            } else {
                None
            },
            web_server_ops: Some(web_server_ops),
            external: if cfg.channels.external.enabled {
                Some(nemesis_channels::external::ExternalConfig {
                    input_exe: cfg.channels.external.input_exe.clone(),
                    output_exe: cfg.channels.external.output_exe.clone(),
                    chat_id: cfg.channels.external.chat_id.clone(),
                    sync_to: cfg.channels.external.sync_to.clone(),
                    allow_from: cfg.channels.external.allow_from.clone(),
                })
            } else {
                None
            },
            maixcam: if cfg.channels.maixcam.enabled {
                Some(nemesis_channels::maixcam::MaixCamConfig {
                    host: cfg.channels.maixcam.host.clone(),
                    port: cfg.channels.maixcam.port as u16,
                    allow_from: cfg.channels.maixcam.allow_from.clone(),
                })
            } else {
                None
            },
            line: if cfg.channels.line.enabled {
                Some(nemesis_channels::line::LineConfig {
                    channel_access_token: cfg.channels.line.channel_access_token.clone(),
                    channel_secret: cfg.channels.line.channel_secret.clone(),
                    webhook_port: cfg.channels.line.webhook_port as u16,
                    allow_from: cfg.channels.line.allow_from.clone(),
                })
            } else {
                None
            },
            websocket: if cfg.channels.websocket.enabled {
                Some(nemesis_channels::websocket::WebSocketChannelConfig {
                    host: cfg.channels.websocket.host.clone(),
                    port: cfg.channels.websocket.port as u16,
                    path: cfg.channels.websocket.path.clone(),
                    auth_token: cfg.channels.websocket.auth_token.clone(),
                    allow_from: cfg.channels.websocket.allow_from.clone(),
                    sync_to: cfg.channels.websocket.sync_to.clone(),
                })
            } else {
                None
            },
            // Feature-gated channels (telegram/discord/feishu/slack/etc.) are mapped
            // when the corresponding feature is enabled in nemesisbot's Cargo.toml:
            //   nemesis-channels = { workspace = true, features = ["telegram"] }
            ..Default::default()
        };

        // Initialize channels from config (registers them in the manager).
        let bus_inbound_sender = bus.inbound_sender();
        if let Err(e) = channel_manager
            .init_channels(&init_config, bus_inbound_sender)
            .await
        {
            warn!(
                "[Gateway] ChannelManager init_channels note: {} (non-fatal)",
                e
            );
        }

        // Setup sync targets — reads each channel's sync_to config and calls add_sync_target().
        // Mirrors Go's manager.go: m.setupSyncTargets() called after initChannels().
        {
            let mut sync_map = std::collections::HashMap::new();
            // Collect sync_to from all channel configs that are enabled
            macro_rules! add_sync {
                ($cfg:expr, $name:expr) => {
                    if $cfg.enabled && !$cfg.sync_to.is_empty() {
                        sync_map.insert($name.to_string(), $cfg.sync_to.clone());
                    }
                };
            }
            add_sync!(cfg.channels.websocket, "websocket");
            add_sync!(cfg.channels.external, "external");
            add_sync!(cfg.channels.web, "web");
            add_sync!(cfg.channels.telegram, "telegram");
            add_sync!(cfg.channels.discord, "discord");
            add_sync!(cfg.channels.feishu, "feishu");
            add_sync!(cfg.channels.dingtalk, "dingtalk");
            add_sync!(cfg.channels.slack, "slack");
            add_sync!(cfg.channels.whatsapp, "whatsapp");
            add_sync!(cfg.channels.qq, "qq");
            add_sync!(cfg.channels.line, "line");
            add_sync!(cfg.channels.maixcam, "maixcam");
            add_sync!(cfg.channels.onebot, "onebot");
            let sync_config = nemesis_channels::manager::ChannelSyncConfig { targets: sync_map };
            channel_manager.setup_sync_targets(&sync_config).await;
        }

        // Bridge: bus outbound broadcast → ChannelManager mpsc.
        // Mirrors Go's manager.go: dispatchOutbound reading from bus.OutboundChannel().
        // Without this, non-web channel outbound is silently dropped.
        let bus_for_cm = bus.clone();
        let cm_outbound_tx = channel_manager.outbound_sender();
        let _cm_bridge_handle = tokio::spawn(async move {
            let mut rx = bus_for_cm.subscribe_outbound();
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        if cm_outbound_tx.send(msg).await.is_err() {
                            break; // ChannelManager dispatch loop stopped
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            "[Gateway] ChannelManager outbound bridge lagged {} messages",
                            n
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        });
        info!("[Gateway] Bus outbound → ChannelManager bridge connected");

        // Start the outbound dispatch loop (reads from internal mpsc, dispatches to channels).
        if let Err(e) = channel_manager.start_dispatch_loop() {
            warn!(
                "[Gateway] ChannelManager start_dispatch_loop note: {} (non-fatal)",
                e
            );
        }

        // Start all registered channels.
        if let Err(e) = channel_manager.start_all().await {
            warn!("[Gateway] ChannelManager start_all note: {} (non-fatal)", e);
        }

        // Keep the ChannelManager alive.
        std::mem::forget(channel_manager);
        info!(
            "[Gateway] ChannelManager created with {} enabled channel(s)",
            enabled_channels.len()
        );
        // Channel manager injection into agent_loop is now handled by the factory function.
    }

    // Step 9b: Create and inject SecurityPlugin if enabled.
    // Mirrors Go's SecurityPlugin registered via PluginManager in instance.go.
    // Keep a reference to the auditor so we can wire up the approval manager later.
    // K1（devtool-upgrade 阶段 4）：装配逻辑原样迁往 `crate::security_setup`
    // （layer 开关 + DLP + 规则 + 审计日志 + scanner 链）——headless `run`
    // 与 gateway 共用同一构造，安全 9 层在无端口形态不降级。
    let security_plugin = crate::security_setup::build_security_plugin(
        &home,
        cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true),
    )
    .await;

    // Step 9d: Setup Observer Manager for conversation lifecycle events.
    // Mirrors Go's bot_service.go Phase 5: observerMgr creation + RequestLogger registration.
    let observer_manager: Option<Arc<nemesis_observer::Manager>> = {
        let observer_mgr = Arc::new(nemesis_observer::Manager::new());

        // Register RequestLogger as Observer (if logging.llm.enabled)
        if let Some(ref logging_cfg) = cfg.logging
            && let Some(llm_cfg) = &logging_cfg.llm
            && llm_cfg.enabled
        {
            let rl_logging_config = nemesis_agent::request_logger::LoggingConfig {
                enabled: true,
                detail_level: match llm_cfg.detail_level.as_str() {
                    "truncated" => nemesis_agent::request_logger::DetailLevel::Truncated,
                    _ => nemesis_agent::request_logger::DetailLevel::Full,
                },
                log_dir: if llm_cfg.log_dir.is_empty() {
                    "logs/request_logs".to_string()
                } else {
                    llm_cfg.log_dir.clone()
                },
                save_raw: llm_cfg.save_raw,
            };
            let workspace_path = home.join("workspace");
            let rl_observer = Arc::new(
                nemesis_agent::request_logger_observer::RequestLoggerObserver::new(
                    rl_logging_config,
                    &workspace_path,
                ),
            );
            // We need an async context to register, but observer_mgr.register is async
            // and we're not in an async block here. Use tokio runtime handle directly.
            let mgr = observer_mgr.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    mgr.register(rl_observer).await;
                })
            });
            info!("[Gateway] RequestLoggerObserver registered (logging.llm.enabled = true)");
        }

        // Check if any observers were registered.
        let mgr_check = observer_mgr.clone();
        let has_observers = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { mgr_check.has_observers().await })
        });
        if has_observers {
            info!("[Gateway] Observer manager initialized (injection handled by factory)");
            Some(observer_mgr)
        } else {
            None
        }
    };

    // Note: DataStore injection into agent_loop is now handled by the factory function.

    // Note: Forge injection into agent_loop is now handled at creation time above.

    // Build SharedResources and use the factory to create the AgentLoop.
    // （estop 句柄已在集群装配块前创建——board 评审依赖集共用同一 Arc。）
    // C5 (2026-09-04): ONE LspManager for the whole gateway — the LspTool
    // registers with it (via SharedToolConfig.lsp_manager), the web server
    // holds the same Arc, and Step-24 teardown calls shutdown_all() so
    // language-server child processes don't outlive the gateway.
    let lsp_manager = std::sync::Arc::new(nemesis_lsp::LspManager::new(
        cfg.agents
            .lsp_tool
            .timeout_secs
            .map(std::time::Duration::from_secs),
        cfg.agents
            .lsp_tool
            .idle_secs
            .map(std::time::Duration::from_secs),
    ));
    // C6（devtool-upgrade 阶段 6）：agents.lsp_tool.auto_install=true → 网关
    // 启动期静默自举缺失的语言服务器（官方安装通道白名单目录，非交互命令
    // 串行后台执行，结果进日志）。信任级=用户显式配置的 standing consent
    // （同 A6 formatter / LSP spawn——基础设施装配不走 8 层管线；dashboard
    // 一键安装按钮那条路才走）。装完需重启 Agent 重新探测注册。
    if cfg.agents.lsp_tool.auto_install {
        tokio::spawn(nemesis_lsp::install::auto_install_missing(
            std::time::Duration::from_secs(600),
        ));
    }
    // M1a (2026-09-05): ONE tool-event broadcast channel for the gateway —
    // sender side goes into SharedResources (each AgentLoop gets a
    // ToolEventHook), receiving side goes to the web server (pump routes
    // events to Dashboard WS push + EventHub).
    let (agent_event_tx, agent_event_rx) =
        tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(256);
    // B4 (2026-09-05): gateway-level background-process registry singleton —
    // construction is process-free (children spawn lazily via
    // background_start); Drop raises kill flags so no spawned job outlives
    // the gateway.
    let background_registry = std::sync::Arc::new(nemesis_agent::BackgroundProcessRegistry::new());
    let shared_resources = crate::agent_factory::SharedResources {
        home: home.clone(),
        // K1：gateway 固定 canonical 布局（显式写出，不依赖 fallback）。
        workspace: home.join("workspace"),
        bus: bus.clone(),
        agent_outbound_tx,
        #[cfg(feature = "forge")]
        forge: forge_for_web.clone(),
        #[cfg(not(feature = "forge"))]
        forge: None,
        #[cfg(feature = "forge")]
        forge_executor: forge_executor_for_tools.clone(),
        #[cfg(not(feature = "forge"))]
        forge_executor: None,
        cron_service: cron_service.clone(),
        security_plugin: security_plugin.clone(),
        observer_manager: observer_manager.clone(),
        data_store: data_store.clone(),
        skills_loader: skills_loader_arc.clone(),
        skills_registry: skills_registry_arc.clone(),
        #[cfg(feature = "memory")]
        memory_manager: memory_manager_for_web.clone(),
        #[cfg(not(feature = "memory"))]
        memory_manager: None,
        enabled_channels: enabled_channels.clone(),
        #[cfg(feature = "workflow")]
        workflow_engine: Some(workflow_engine.clone()),
        #[cfg(not(feature = "workflow"))]
        workflow_engine: None,
        cluster_rpc_call_fn,
        cluster_rpc_config,
        cluster_peers_fn,
        cluster_rpc_enabled: parking_lot::RwLock::new(None::<Arc<std::sync::atomic::AtomicBool>>),
        mcp_config_path: common::mcp_config_path(&home),
        mcp_enabled,
        estop,
        config_store: config_store.clone(),
        lsp_manager,
        agent_event_tx: Some(agent_event_tx),
        background_registry,
        // 全自动流转 P3/D1：board_issue 工具依赖（注册点在 build_agent_loop
        // 主 agent；store=None 时工具不注册）。moderator 槽此刻还空，agent
        // 建成后 :board_moderator_loop.set 填充——工具调用时读槽即得。
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_store: board_store.clone(),
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_cluster: cluster_adapter_refs.as_ref().map(|(c, _, _, _)| c.clone()),
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_moderator_slot: board_moderator_loop.clone(),
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_home: home.clone(),
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_event_hub: Some(web_server.event_hub().clone()),
        #[cfg(feature = "security")]
        approval_slot: std::sync::Arc::new(parking_lot::RwLock::new(
            None::<Arc<dyn nemesis_security::auditor::ApprovalManager>>,
        )),
        #[cfg(not(feature = "security"))]
        approval_slot: (),
        // F7（2026-09-06）：question 工具 broker 槽（先建空槽，装配块晚填
        // WebQuestionBroker；重启重建的 AgentLoop 共享同一槽 Arc）。
        question_slot: std::sync::Arc::new(parking_lot::RwLock::new(
            None::<Arc<dyn nemesis_types::agent::QuestionAsker>>,
        )),
    };

    let shared_resources = Arc::new(shared_resources);
    let agent_loop = crate::agent_factory::build_agent_loop(&shared_resources)
        .map_err(|e| anyhow::anyhow!("Failed to build agent loop: {}", e))?;
    let initial_tool_count = agent_loop.tool_count();
    info!(
        "[Gateway] AgentLoop built via factory ({} tools)",
        initial_tool_count
    );

    // Bridge the agent's tools into the workflow engine's tool registry so the
    // workflow `tool` node can invoke them. The registry was created empty
    // during workflow init above; here we wrap each agent tool in an
    // `AgentToolAdapter` (the two `Tool` traits are incompatible) and register
    // it. Each adapted tool runs the 8-layer security rule pipeline per call —
    // no interactive approval popup, no guardian LLM judge — so batch
    // workflows run unattended while still respecting workspace isolation and
    // the other rule layers.
    #[cfg(feature = "workflow")]
    {
        let tools_guard = agent_loop.tools();
        let mut bridged = 0usize;
        for (name, tool) in tools_guard.iter() {
            #[cfg(feature = "security")]
            let adapted = nemesis_agent::tool_adapter::AgentToolAdapter::new(
                name.clone(),
                Arc::clone(tool),
                security_plugin.clone(),
            );
            #[cfg(not(feature = "security"))]
            let adapted =
                nemesis_agent::tool_adapter::AgentToolAdapter::new(name.clone(), Arc::clone(tool));
            workflow_tool_registry.register(adapted);
            bridged += 1;
        }
        info!(
            "[Gateway] Bridged {} agent tools into the workflow tool registry",
            bridged
        );
    }

    // Wire up `agent` workflow nodes (milestone 1b-D2). Each workflow run
    // that hits an `agent` node will route through this runner, which
    // namespaces session keys under `workflow:{agent_id}` so workflow
    // sessions don't collide with human user sessions.
    #[cfg(feature = "workflow")]
    {
        workflow_engine
            .register_agent_runner(Arc::new(GatewayAgentRunner::new(agent_loop.clone())));
        info!("[Gateway] Workflow agent runner registered");

        // Wire DataStore into workflow engine so llm/question_classifier/parameter_extractor
        // node executors record a RequestLog per LLM call. Agent nodes are already
        // tracked via the agent_loop's own data_store wiring.
        if let Some(ref ds) = data_store {
            workflow_engine.set_usage_store(ds.clone());
            info!("[Gateway] Workflow usage store wired");
        }
    }

    // --- Swarm M3: 主持人裁决桥填装（nb_bus 注册早于 agent_loop 构建）---
    #[cfg(all(feature = "board", feature = "cluster"))]
    {
        if board_moderator_loop.set(agent_loop.clone()).is_err() {
            warn!("[Gateway] Board moderator loop already set");
        }
    }

    // --- 全自动流转 P1（A3）：父单收口验收钩子注册 ---
    // sync_parent_status 子单全 done 路径 → 本钩子（读 auto_close_parent
    // 旗标 + master 角色闸）→ spawn_parent_review。旗标每次现读：config
    // 热改即时生效，与 load_board_flags 同语义。worker 节点本地 board.db
    // 是 dashboard 视图，非权威——角色闸保持与 write_back 触发链同级。
    #[cfg(all(feature = "board", feature = "cluster"))]
    {
        let cluster_ok = cluster_adapter_refs
            .as_ref()
            .map(|(c, _, _, _)| matches!(c.role().as_str(), "coordinator" | "master" | "manager"))
            .unwrap_or(false);
        if cluster_ok
            && let (Some(store), Some((cluster, _, _, _))) =
                (board_store.clone(), cluster_adapter_refs.as_ref())
        {
            let deps = crate::board_review::BoardReviewDeps {
                store,
                workspace: home.join("workspace"),
                home: home.clone(),
                moderator_loop: board_moderator_loop.clone(),
                cluster: cluster.clone(),
                estop: shared_resources.estop.clone(),
                estop_parked: board_estop_parked.clone(),
                selfcheck: board_selfcheck_registry.clone(),
            };
            let hook_deps = std::sync::Arc::new(deps);
            // P1/T1-6：estop 释放 watcher——把冻结停车的评审逐条复评恢复。
            crate::board_review::spawn_estop_resume_watcher((*hook_deps).clone());
            let hook_home = home.clone();
            let hook_cluster = cluster.clone();
            if let Err(e) = nemesis_web::handlers::board::set_parent_review_hook(
                std::sync::Arc::new(move |parent_id: i64| {
                    // 旗标现读（fail-closed：读失败不收口）。master 判定
                    // 与 nb_bus handler 注册同款（board_store 全员 open
                    // ≠ master 身份）。
                    let is_master = matches!(
                        hook_cluster.role().as_str(),
                        "coordinator" | "master" | "manager"
                    );
                    if !is_master {
                        return;
                    }
                    let flags = nemesis_config::load_config(&hook_home.join("config.json"))
                        .map(|c| c.board.unwrap_or_default());
                    match flags {
                        Ok(f) if f.auto_close_parent => {
                            crate::board_review::spawn_parent_review(
                                (*hook_deps).clone(),
                                parent_id,
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                "[Gateway] 父单 {parent_id} 收口旗标读取失败（fail-closed 转人工）：{e}"
                            );
                        }
                    }
                }),
            ) {
                warn!("[Gateway] Board parent review hook register failed: {e}");
            } else {
                info!("[Gateway] Board parent review hook armed (auto_close_parent)");
            }

            // --- 全自动流转 P4（F3）：项目收口验收钩子注册 ---
            // 全部顶层父单 done → notify 聚合预检过了才 fire；旗标
            // `board.review.auto_close_project` 每次现读（同父单钩子语义）。
            let proj_deps = std::sync::Arc::new(crate::board_review::BoardReviewDeps {
                store: board_store
                    .clone()
                    .expect("cluster_ok arm guarantees board store present"),
                workspace: home.join("workspace"),
                home: home.clone(),
                moderator_loop: board_moderator_loop.clone(),
                cluster: cluster.clone(),
                estop: shared_resources.estop.clone(),
                estop_parked: board_estop_parked.clone(),
                selfcheck: board_selfcheck_registry.clone(),
            });
            let proj_home = home.clone();
            let proj_cluster = cluster.clone();
            if let Err(e) = nemesis_web::handlers::board::set_project_review_hook(
                std::sync::Arc::new(move |project_id: i64| {
                    let is_master = matches!(
                        proj_cluster.role().as_str(),
                        "coordinator" | "master" | "manager"
                    );
                    if !is_master {
                        return;
                    }
                    let flags = nemesis_config::load_config(&proj_home.join("config.json"))
                        .map(|c| c.board.unwrap_or_default());
                    match flags {
                        Ok(f) if f.auto_review && f.review.auto_close_project => {
                            crate::board_review::spawn_project_review(
                                (*proj_deps).clone(),
                                project_id,
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                "[Gateway] 项目 {project_id} 收口旗标读取失败（fail-closed 转人工）：{e}"
                            );
                        }
                    }
                }),
            ) {
                warn!("[Gateway] Board project review hook register failed: {e}");
            } else {
                info!("[Gateway] Board project review hook armed (review.auto_close_project)");
            }
        }
    }

    // --- Inject tool capabilities into cluster for discovery broadcast ---
    #[cfg(feature = "cluster")]
    {
        if let Some((ref cluster, _, _, _)) = cluster_adapter_refs {
            let tool_names = agent_loop.tool_names();
            cluster.set_capabilities(tool_names);
            info!(
                "[Gateway] Cluster capabilities injected ({} tools)",
                initial_tool_count
            );
        }
    }

    // --- Create ClusterServiceAdapter (always, for dynamic start/stop from Dashboard) ---
    // The adapter manages: cluster.start(), RPC server start, discovery start,
    // task recovery from disk, cluster agent loop spawn, ClusterRpcTool enable.
    // 批次 E：dashboard 发言桥要用的 cluster 引用（下方 take() 会把 Arc 消耗
    // 进 adapter，先抢一份）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_discussion_cluster: Option<std::sync::Arc<nemesis_cluster::cluster::Cluster>> =
        cluster_adapter_refs.as_ref().map(|(c, _, _, _)| c.clone());
    #[cfg(feature = "cluster")]
    {
        if let Some((cluster, task_list, work_queue, result_persister)) =
            cluster_adapter_refs.take()
        {
            let adapter = Arc::new(crate::cluster_service::ClusterServiceAdapter::new(
                cluster,
                shared_resources.clone(),
                tokio::runtime::Handle::current(),
                home.clone(),
                task_list,
                work_queue,
                result_persister,
                board_worker_inbox.take(),
            ));
            // Only perform first start when both config flags are enabled.
            // Otherwise the adapter is created but idle — can be started from Dashboard.
            if cluster_should_start && let Err(e) = adapter.first_start() {
                warn!("[Gateway] Cluster adapter first start failed: {}", e);
            }
            cluster_adapter = Some(adapter);
        }
    }

    // Create shared reference for WebServer model switching
    let agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>> =
        Arc::new(parking_lot::RwLock::new(None));

    // Create AgentLoopServiceAdapter for tray start/stop control.
    // Passes the initial AgentLoop directly — no double construction.
    // The adapter manages the inbound bridge + agent spawn internally.
    let agent_adapter = Arc::new(adapters::AgentLoopServiceAdapter::new(
        agent_loop.clone(),
        shared_resources.clone(),
        bus.clone(),
        agent_loop_ref.clone(),
    ));

    // --- L6++（2026-09-08）：项目常驻 loop 启动（对话/项目双分组）---
    // 共享主 loop 内建的同一 SessionStore Arc（存储全局集中、会话隔离靠
    // session_key）；注册表缺失/损坏走 lenient 空表，目录消失的项目
    // warn + skip 不炸 gateway。G3 起项目调度器经 manager 把项目会话消息
    // 转发进对应项目 loop（主桥 skip 谓词同步接线）。
    let projects_manager = {
        let main_store = agent_loop
            .session_store()
            .cloned()
            .expect("main agent loop must carry a session store");
        let mgr = Arc::new(crate::projects::manager::ProjectLoopManager::new(
            shared_resources.clone(),
            main_store,
            bus.clone(),
        ));
        mgr.start_all();
        mgr
    };
    // G3（2026-09-08）：主桥 skip 谓词（项目会话消息不进主 loop，由项目
    // 调度器转发）+ 全进程唯一 1 个项目调度订阅。时序在 web bind 之前，
    // 满足「loop 订阅 bus → web bind」不变量。
    {
        let mgr_for_pred = projects_manager.clone();
        agent_adapter.set_skip_predicate(Arc::new(move |msg| mgr_for_pred.bridge_should_skip(msg)));
        projects_manager.start_routing();
        // G4（2026-09-08）：ProjectsBridge 接线——projects.* WSAPI 与
        // resolve_session_loop（chat/tools/approval/question/agent/fs 各
        // handler 的归属解析）经此 trait 触达 manager（trait 在
        // ProjectLoopManager 上直接实现，Arc 协同转换装槽）。
        let projects_bridge: std::sync::Arc<dyn nemesis_web::handlers::projects::ProjectsBridge> =
            projects_manager.clone();
        nemesis_web::handlers::projects::install_projects_bridge(projects_bridge);
    }

    // Step 10: Wire up WebServer (created early for WebChannel injection)
    web_server.set_message_bus(bus.clone());
    web_server.set_model_info(
        &model_name,
        &resolution.api_base,
        !resolution.api_key.is_empty(),
    );

    // Wire streaming provider for SSE chat endpoint.
    // Create an HttpProvider from the same config used for the main provider.
    // This enables /api/chat/stream for token-by-token streaming.
    {
        let streaming_cfg = nemesis_providers::http_provider::HttpProviderConfig {
            name: "streaming".to_string(),
            base_url: resolution.api_base.clone(),
            api_key: resolution.api_key.clone(),
            default_model: resolution.model_name.clone(),
            timeout_secs: 120,
            headers: std::collections::HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        web_server.set_streaming_provider(Arc::new(
            nemesis_providers::http_provider::HttpProvider::new(streaming_cfg),
        ));
        info!("[Gateway] SSE streaming provider configured for /api/chat/stream");
    }

    info!(
        "[Gateway] Web server created for {}:{}",
        web_bind_host, web_port
    );

    // Inject agent service into web server for start/stop control
    web_server.set_agent_service(agent_adapter.clone());
    info!("[Gateway] Agent service injected into web server");

    // Inject global e-stop state so /api/internal can trigger/release/query it
    // (EstopState is a thread-safe Arc<AtomicBool+watch>; the web handler
    // mutates it directly — no mpsc round-trip needed, and status returns live).
    web_server.set_estop(shared_resources.estop.clone());
    info!("[Gateway] E-stop state injected into web server");

    // C5: inject the shared LSP manager (same Arc the LspTool registered
    // with) — Phase-2 diagnostics loop and future dashboard LSP ops read it.
    web_server.set_lsp_manager(shared_resources.lsp_manager.clone());
    info!("[Gateway] LSP manager injected into web server");

    // M1a: hand the tool-event receiver to the web server — its pump (spawned
    // in WebServer::start) routes events to Dashboard WS push + EventHub.
    web_server.set_agent_event_rx(agent_event_rx);
    info!("[Gateway] Agent tool-event receiver injected into web server");

    // Inject the runtime CronService (so tasks.cron.* calls the live scheduler)
    // and the ConvRouter (shared with the cron fire handler for Opt 2 live
    // delivery). CronService is cloned because gateway still owns a handle to
    // start() it later; conv_router is moved (its only other reference is the
    // clone already captured in the cron fire closure).
    web_server.set_cron(cron_service.clone());
    web_server.set_conv_router(conv_router);
    info!("[Gateway] CronService + ConvRouter injected into web server");

    // Inject the managed-agent board store (board feature only; None →
    // board.* WSAPI commands report "board service not available").
    #[cfg(feature = "board")]
    if let Some(ref store) = board_store {
        // 角色解析（goal 硬约束①：复用 NodeRole，无平行 role 字段）：
        // - cluster 启用 → 取 cluster.role()（peers.toml [node].role；
        //   from_role_str 兼容旧值 master/manager）；
        // - cluster 关闭 / cluster feature 未编译 → Coordinator。
        // role 2026-08-31 起仅为元数据（日志/诊断展示），不门控 board 写——
        // board.db 是节点本地数据，写权限与 role 无关（见 BoardService 文档）。
        #[cfg(feature = "cluster")]
        let board_role = if cluster_should_start {
            cluster_adapter_refs
                .as_ref()
                .map(|(c, _, _, _)| nemesis_types::cluster::NodeRole::from_role_str(&c.role()))
                .unwrap_or(nemesis_types::cluster::NodeRole::Coordinator)
        } else {
            nemesis_types::cluster::NodeRole::Coordinator
        };
        #[cfg(not(feature = "cluster"))]
        let board_role = nemesis_types::cluster::NodeRole::Coordinator;
        // Swarm M3（§5.4/D6）：资产服务装配——密钥 load-or-create
        // （<workspace>/config/asset_secret.key；损坏 loud 报错不重置，
        // 重置=作废全部已签发 token）+ 资产目录 <workspace>/board/assets。
        // 齐备后本节点就是资产提供方（公开端点 /api/board/asset/{ref} 验
        // 自己的 secret），worker 产物反走同一条路。签发上下文挂 store
        // （dispatch 链全部函数持有 store，零参数蔓延）；对外基址槽 bind
        // 后 set（见下方 real_port 解析处）。
        let mut board_service = nemesis_board::BoardService::new(store.clone(), board_role);
        #[cfg(feature = "cluster")]
        match nemesis_board::asset_token::load_or_create_secret(
            &nemesis_path::resolve_asset_secret_path_in_workspace(&home.join("workspace")),
        ) {
            Ok(secret) => {
                store.set_asset_signing(nemesis_board::AssetSignContext {
                    secret: secret.clone(),
                    node_url: board_asset_url_slot.clone(),
                });
                board_service = board_service.with_asset_secret(secret).with_assets_dir(
                    nemesis_path::resolve_board_assets_dir_in_workspace(&home.join("workspace")),
                );
                info!("[Gateway] Board asset serving armed (HMAC token endpoint)");
            }
            Err(e) => warn!("[Gateway] Board asset serving disabled: {}", e),
        }
        #[cfg(not(feature = "cluster"))]
        let _ = &mut board_service;
        // Swarm M3 批次 E：dashboard 人工发言桥（board.channel.post → 讨论管
        // 线）。board+cluster 齐备且本节点持有 board_store（master 形态）时
        // 注入——幂等/额度/裁决与 worker 上行同一条管线（单一真相）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        if let (Some(store), Some(discussion_cluster)) =
            (board_store.as_ref(), board_discussion_cluster.clone())
        {
            board_service =
                board_service.with_discussion(Arc::new(crate::board_bus::LocalDiscussionIngress {
                    deps: crate::board_bus::MasterBusDeps {
                        store: store.clone(),
                        quota: board_quota.clone(),
                        cluster: discussion_cluster,
                        moderator_loop: board_moderator_loop.clone(),
                    },
                }));
            info!(
                "[Gateway] Board discussion ingress armed (dashboard channel.post → nb_bus pipeline)"
            );
        }
        web_server.set_board(board_service);
        info!(
            "[Gateway] Board service injected into web server (role={})",
            board_role.as_role_str()
        );

        // --- W2.5: board 数据变化 watcher → SSE 广播 ---
        // 独立零写入连接轮询 `PRAGMA data_version`（只对其他连接的写敏感）：
        // WSAPI / 集群写回 / autopilot / sweep（BoardStore 自己的连接）与
        // CLI 子命令（跨进程）的每次落库都会被看见——写路径零埋点覆盖全部
        // 写入方。变化 → SSE `board-changed` → 前端各面板 200ms 防抖刷新。
        // 无 SSE 订阅者（dashboard 未开）时跳过轮询读数，空闲零成本。
        const BOARD_CHANGE_POLL_SECS: u64 = 2;
        let board_db_for_watch = home.join("workspace").join("board").join("board.db");
        let event_hub_for_watch = web_server.event_hub().clone();
        match nemesis_board::watcher::open_conn(&board_db_for_watch) {
            Ok(watch_conn) => {
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                        BOARD_CHANGE_POLL_SECS,
                    ));
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    let mut last = nemesis_board::watcher::data_version(&watch_conn).ok();
                    loop {
                        ticker.tick().await;
                        if event_hub_for_watch.subscriber_count() == 0 {
                            continue;
                        }
                        let Ok(v) = nemesis_board::watcher::data_version(&watch_conn) else {
                            continue;
                        };
                        if last != Some(v) {
                            last = Some(v);
                            event_hub_for_watch.publish(
                                "board-changed",
                                serde_json::json!({ "ts": chrono::Utc::now().to_rfc3339() }),
                            );
                            tracing::debug!(
                                "[Board] change detected (data_version={v}) → board-changed"
                            );
                        }
                    }
                });
            }
            Err(e) => {
                tracing::warn!("[Gateway] board change watcher disabled: {e}");
            }
        }
    }

    // Inject DataStore into web server for usage statistics API
    if let Some(ref ds) = data_store {
        web_server.set_data_store(ds.clone());
        info!("[Gateway] DataStore injected into web server");

        // --- A3: usage 明细变化 watcher → SSE `usage-changed` ---
        // 独立零写入连接轮询 `PRAGMA data_version`（board 同款原语）：
        // AgentLoop / workflow LLM 节点经 DataStore 自己连接的每次落库都
        // 会被看见，写路径零埋点。变化 → SSE `usage-changed` → 前端请求
        // 明细 tab 200ms 防抖静默刷新。无 SSE 订阅者（dashboard 未开）
        // 时跳过轮询读数，空闲零成本。
        const USAGE_CHANGE_POLL_SECS: u64 = 2;
        let usage_db_for_watch = nemesis_path::workspace_data_dir(&home).join("nemesisbot_data.db");
        let event_hub_for_usage = web_server.event_hub().clone();
        match nemesis_data::watcher::open_conn(&usage_db_for_watch) {
            Ok(watch_conn) => {
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                        USAGE_CHANGE_POLL_SECS,
                    ));
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    let mut last = nemesis_data::watcher::data_version(&watch_conn).ok();
                    loop {
                        ticker.tick().await;
                        if event_hub_for_usage.subscriber_count() == 0 {
                            continue;
                        }
                        let Ok(v) = nemesis_data::watcher::data_version(&watch_conn) else {
                            continue;
                        };
                        if last != Some(v) {
                            last = Some(v);
                            event_hub_for_usage.publish(
                                "usage-changed",
                                serde_json::json!({ "ts": chrono::Utc::now().to_rfc3339() }),
                            );
                            tracing::debug!(
                                "[Gateway] usage change detected (data_version={v}) → usage-changed"
                            );
                        }
                    }
                });
                info!("[Gateway] usage change watcher armed (poll={USAGE_CHANGE_POLL_SECS}s)");
            }
            Err(e) => {
                warn!("[Gateway] usage change watcher disabled: {e}");
            }
        }

        // --- A3: 保留策略 sweep（启动时 + 每 6h）---
        // config `usage` 段：retention_days=0 关闭按天清理（明细只增到
        // max_rows 上限为止）；max_rows=0 无上限。此前 rollup 逻辑从未
        // 被生产调用（只有测试），本次一并接上。
        let usage_cfg = cfg.usage.clone().unwrap_or_default();
        let ds_for_sweep = ds.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // interval 首个 tick 立即返回 = 启动时先跑一次。
            loop {
                ticker.tick().await;
                let max_rows = (usage_cfg.max_rows > 0).then_some(usage_cfg.max_rows);
                if let Err(e) = ds_for_sweep.retention_sweep(
                    (usage_cfg.retention_days > 0).then_some(usage_cfg.retention_days),
                    max_rows,
                ) {
                    warn!("[Gateway] usage retention sweep failed: {e}");
                }
            }
        });
        info!(
            "[Gateway] usage retention sweep armed (retention_days={}, max_rows={})",
            usage_cfg.retention_days, usage_cfg.max_rows
        );
    }

    // Inject MemoryManager into web server for runtime vector store control
    #[cfg(feature = "memory")]
    {
        if let Some(mgr) = memory_manager_for_web {
            web_server.set_memory_manager(mgr);
            info!("[Gateway] MemoryManager injected into web server");
        }
    }

    // Inject Forge into web server for runtime start/stop control
    #[cfg(feature = "forge")]
    {
        if let Some(forge) = forge_for_web.as_ref() {
            web_server.set_forge(forge.clone());
            info!("[Gateway] Forge instance injected into web server");
        }
    }

    // Inject Cluster into web server for dashboard data queries
    #[cfg(feature = "cluster")]
    {
        if let Some(ref adapter) = cluster_adapter {
            web_server.set_cluster(adapter.cluster().clone());
            info!("[Gateway] Cluster instance injected into web server");

            // Initialize cluster log writer for structured JSONL logging.
            // try_ 幂等变体：同一进程内第二次 run()（in-process 测试复跑网关）
            // 不再 panic "called more than once"；生产单 gateway 每进程只走一次，
            // 行为不变。
            let cluster_log_dir = home.join("workspace/logs/cluster_logs");
            if nemesis_cluster::cluster_log::try_init_cluster_log(&cluster_log_dir) {
                info!(
                    dir = %cluster_log_dir.display(),
                    "[ClusterLog] Initialized"
                );
            } else {
                info!("[ClusterLog] Already initialized in this process — reusing existing writer");
            }

            // Inject cluster lifecycle service for start/stop control
            web_server.set_cluster_service(
                adapter.clone() as Arc<dyn nemesis_services::bot_service::LifecycleService>
            );
            // Inject cluster log directory for JSONL log reader
            web_server.set_cluster_log_dir(cluster_log_dir.to_string_lossy().to_string());
            info!("[Gateway] Cluster service and log dir injected into web server");

            // Phase 4: Bridge cluster log events → SSE EventHub for real-time Dashboard updates.
            // Every cluster log entry (task_submitted, rpc_call, node_online, etc.) is forwarded
            // to connected SSE clients via the EVENT_CLUSTER_EVENT channel.
            let event_hub = web_server.event_hub().clone();
            nemesis_cluster::cluster_log::set_cluster_log_hook(Arc::new(move |event, data| {
                event_hub.publish(
                    nemesis_web::events::EVENT_CLUSTER_EVENT,
                    serde_json::json!({
                        "event": event,
                        "data": data,
                    }),
                );
            }));
            info!("[Gateway] Cluster log → SSE EventHub bridge connected");
        }
    }

    // Inject AgentLoop ref into web server for runtime model switching
    web_server.set_agent_loop(agent_loop_ref.clone());
    info!("[Gateway] AgentLoop ref injected into web server for model switching");

    // Inject WorkflowEngine into web server for /api/workflow/* endpoints
    #[cfg(feature = "workflow")]
    {
        web_server.set_workflow_engine(workflow_engine.clone());
        web_server.set_chat_secret_store(chat_secret_store.clone());
        info!("[Gateway] Workflow engine injected into web server");
    }

    #[cfg(feature = "workflow")]
    {
        // --- Workflow trigger drivers (event + message) ---
        // Two subscription tasks wire trigger configs to their data sources:
        //
        // 1. Inbound bus → message triggers:
        //    Every InboundMessage published by any channel (web, telegram, discord,
        //    etc.) is matched against each workflow's `message` trigger configs
        //    (channel/content/sender_id/chat_id glob match). Matches start a
        //    background execution.
        //
        // 2. EventDispatcher → event triggers:
        //    TriggerEvents (workflow.completed/failed, forge.pattern_created, or
        //    manual fire_event via WSAPI) match each workflow's `event` trigger
        //    configs (event_type glob + data field matchers). Matches start a
        //    background execution.
        //
        // Without these, `message` and `event` triggers never fire — the warning
        // "trigger type X has no runtime driver" no longer applies as of P3.
        let msg_engine = workflow_engine.clone();
        let mut inbound_rx_for_wf = bus.subscribe_inbound();
        let _inbound_wf_handle = tokio::spawn(async move {
            loop {
                match inbound_rx_for_wf.recv().await {
                    Ok(msg) => {
                        let channel = msg.channel.clone();
                        let sender = msg.sender_id.clone();
                        let chat = msg.chat_id.clone();
                        let content = msg.content.clone();
                        let session_key = msg.session_key.clone();
                        let matched = msg_engine
                            .workflows_matching_message(&channel, &sender, &chat, &content);
                        if matched.is_empty() {
                            continue;
                        }
                        for wf_name in matched {
                            let engine = msg_engine.clone();
                            let ch = channel.clone();
                            let sd = sender.clone();
                            let ct = chat.clone();
                            let cn = content.clone();
                            let sk = session_key.clone();
                            tokio::spawn(async move {
                                let trigger = nemesis_workflow::types::TriggerSource::Message {
                                    channel: ch.clone(),
                                    chat_id: ct.clone(),
                                    sender_id: sd.clone(),
                                    content: cn.clone(),
                                };
                                let mut input = std::collections::HashMap::new();
                                input.insert("channel".to_string(), serde_json::json!(ch));
                                input.insert("sender_id".to_string(), serde_json::json!(sd));
                                input.insert("chat_id".to_string(), serde_json::json!(ct));
                                input.insert("content".to_string(), serde_json::json!(cn));
                                // Unified `input` field: the message content is
                                // the natural main input for `message` triggers.
                                input.insert("input".to_string(), serde_json::json!(cn));
                                input.insert("session_key".to_string(), serde_json::json!(sk));
                                match engine.start_async(&wf_name, input, Some(trigger)).await {
                                    Ok(id) => {
                                        info!(
                                            workflow = %wf_name,
                                            execution_id = %id,
                                            channel = %ch,
                                            "[Workflow] message-triggered execution started"
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            workflow = %wf_name,
                                            error = %e,
                                            "[Workflow] message-triggered execution failed to start"
                                        );
                                    }
                                }
                            });
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            n,
                            "[Workflow] message-trigger subscriber lagged (some triggerable messages dropped)"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("[Workflow] inbound bus closed, message-trigger task exiting");
                        break;
                    }
                }
            }
        });

        let evt_engine = workflow_engine.clone();
        let mut event_rx = evt_engine.event_dispatcher().subscribe();
        let _event_wf_handle = tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        let matched = evt_engine.workflows_matching_event(&event);
                        if matched.is_empty() {
                            continue;
                        }
                        for wf_name in matched {
                            let engine = evt_engine.clone();
                            let ev = event.clone();
                            tokio::spawn(async move {
                                let trigger = nemesis_workflow::types::TriggerSource::Event {
                                    event_type: ev.event_type.clone(),
                                    data: serde_json::Value::Object(
                                        ev.data.clone().into_iter().collect(),
                                    ),
                                };
                                let mut input = std::collections::HashMap::new();
                                input.insert(
                                    "event_type".to_string(),
                                    serde_json::json!(ev.event_type),
                                );
                                for (k, v) in &ev.data {
                                    input.insert(k.clone(), v.clone());
                                }
                                // Unified `input` field: prefer `ev.data.input`
                                // if present (caller can set it explicitly);
                                // otherwise JSON-serialise the whole data object.
                                if !input.contains_key("input") {
                                    let serialized = serde_json::Value::Object(
                                        ev.data.clone().into_iter().collect(),
                                    )
                                    .to_string();
                                    input
                                        .insert("input".to_string(), serde_json::json!(serialized));
                                }
                                if let Some(src) = &ev.source_execution_id {
                                    input.insert(
                                        "source_execution_id".to_string(),
                                        serde_json::json!(src),
                                    );
                                }
                                match engine.start_async(&wf_name, input, Some(trigger)).await {
                                    Ok(id) => {
                                        info!(
                                            workflow = %wf_name,
                                            execution_id = %id,
                                            event_type = %ev.event_type,
                                            "[Workflow] event-triggered execution started"
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            workflow = %wf_name,
                                            event_type = %ev.event_type,
                                            error = %e,
                                            "[Workflow] event-triggered execution failed to start"
                                        );
                                    }
                                }
                            });
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            n,
                            "[Workflow] event-trigger subscriber lagged (some triggerable events dropped)"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("[Workflow] event dispatcher closed, event-trigger task exiting");
                        break;
                    }
                }
            }
        });

        info!("[Gateway] Workflow trigger drivers spawned (event + message)");

        // Register the WorkflowChatReplyObserver so `/workflow/chat/<index>`
        // pages get a reply broadcast when their execution finishes. The observer
        // filters by `TriggerSource::WorkflowChat` so non-chat executions are
        // ignored. Per-workflow serialization guards are released here too.
        {
            let observer = Arc::new(
                nemesis_web::workflow_chat_reply_observer::WorkflowChatReplyObserver::new(
                    web_server.session_manager().clone(),
                    workflow_engine.clone(),
                ),
            );
            workflow_engine
                .event_manager()
                .register(observer as Arc<dyn nemesis_workflow::events::WorkflowObserver>)
                .await;
            info!("[Gateway] WorkflowChatReplyObserver registered");
        }
    }

    info!("[Gateway] Web server components injected");

    // Step 11: Create HealthServer
    #[cfg(feature = "health")]
    let health_server = {
        let health_port = cfg.gateway.port;
        let health_config = nemesis_health::server::HealthServerConfig {
            listen_addr: format!("{}:{}", &cfg.gateway.host, health_port),
            version: Some(crate::common::VERSION_INFO.version.to_string()),
        };
        Arc::new(nemesis_health::server::HealthServer::new(health_config))
    };
    #[cfg(feature = "health")]
    info!(
        "[Gateway] Health server created for {}:{}",
        &cfg.gateway.host, cfg.gateway.port
    );

    // Step 12: Create HeartbeatService
    let heartbeat_interval_secs = if cfg.heartbeat.interval > 0 {
        (cfg.heartbeat.interval * 60) as u64
    } else {
        300
    };
    #[cfg(feature = "heartbeat")]
    let heartbeat_service = {
        let heartbeat_config = nemesis_heartbeat::service::HeartbeatConfig {
            interval: std::time::Duration::from_secs(heartbeat_interval_secs),
            enabled: cfg.heartbeat.enabled,
            workspace: Some(common::workspace_path(&home).to_string_lossy().to_string()),
            min_interval_minutes: 5,
            default_interval_minutes: 30,
        };
        Arc::new(nemesis_heartbeat::service::HeartbeatService::new(
            heartbeat_config,
        ))
    };
    #[cfg(feature = "heartbeat")]
    info!(
        "[Gateway] Heartbeat service created (enabled: {})",
        cfg.heartbeat.enabled
    );

    // C2: Wire HeartbeatService — bus + handler + skip file.
    // Mirrors Go's bot_service.go:403-406:
    //   heartbeatSvc.SetBus(msgBus)
    //   heartbeatSvc.SetHandler(createHeartbeatHandler(agentLoop))
    #[cfg(feature = "heartbeat")]
    {
        // Adapter: nemesis_bus::MessageBus → heartbeat::MessageBus
        struct HeartbeatBusAdapter {
            bus: Arc<nemesis_bus::MessageBus>,
        }
        impl nemesis_heartbeat::service::MessageBus for HeartbeatBusAdapter {
            fn publish_outbound(&self, channel: String, chat_id: String, content: String) {
                let msg = nemesis_types::channel::OutboundMessage {
                    channel,
                    chat_id,
                    content,
                    message_type: String::new(),
                    meta: Default::default(),
                };
                self.bus.publish_outbound(msg);
            }
        }
        heartbeat_service.set_bus(Arc::new(HeartbeatBusAdapter { bus: bus.clone() }));

        // Handler: calls agent_loop.process_heartbeat() synchronously via block_in_place.
        // Mirrors Go's `createHeartbeatHandler()` in bot_service.go:
        //   1. Check BOOTSTRAP.md → skip heartbeat
        //   2. Fallback channel = "cli", chat_id = "direct"
        //   3. Call ProcessHeartbeat(prompt, channel, chatID)
        //   4. Always return SilentResult (agent sends messages via tools, not via handler)
        let bootstrap_path = common::workspace_path(&home).join("BOOTSTRAP.md");
        let adapter_for_hb = agent_adapter.clone();
        heartbeat_service.set_handler(Box::new(
            move |prompt: String, mut channel: String, mut chat_id: String| {
                // Check BOOTSTRAP.md — if exists, skip heartbeat entirely.
                if bootstrap_path.exists() {
                    tracing::info!("[Gateway] BOOTSTRAP.md exists, skipping heartbeat LLM call");
                    return Some(nemesis_heartbeat::service::HeartbeatResult {
                        is_error: false,
                        is_async: false,
                        silent: true,
                        for_user: String::new(),
                        for_llm: "HEARTBEAT_OK".to_string(),
                    });
                }

                // Get the current AgentLoop via adapter (may be None if stopped).
                let agent_loop_for_hb = match adapter_for_hb.current() {
                    Some(al) => al,
                    None => {
                        tracing::debug!("[Gateway] Agent not running, skipping heartbeat");
                        return Some(nemesis_heartbeat::service::HeartbeatResult {
                            is_error: false,
                            is_async: false,
                            silent: true,
                            for_user: String::new(),
                            for_llm: "HEARTBEAT_OK".to_string(),
                        });
                    }
                };

                // Use cli:direct as fallback (matching Go).
                if channel.is_empty() || chat_id.is_empty() {
                    channel = "cli".to_string();
                    chat_id = "direct".to_string();
                }

                tokio::task::block_in_place(|| {
                    let rt = tokio::runtime::Handle::current();
                    match rt
                        .block_on(agent_loop_for_hb.process_heartbeat(&prompt, &channel, &chat_id))
                    {
                        Ok(response) if response.is_empty() => None,
                        Ok(response) => {
                            let is_heartbeat_ok = response.trim() == "HEARTBEAT_OK";
                            Some(nemesis_heartbeat::service::HeartbeatResult {
                                is_error: false,
                                is_async: false,
                                silent: true, // Go always returns SilentResult
                                for_user: String::new(),
                                for_llm: if is_heartbeat_ok {
                                    "HEARTBEAT_OK".to_string()
                                } else {
                                    response
                                },
                            })
                        }
                        Err(e) => Some(nemesis_heartbeat::service::HeartbeatResult {
                            is_error: true,
                            is_async: false,
                            silent: false,
                            for_user: String::new(),
                            for_llm: format!("Heartbeat error: {}", e),
                        }),
                    }
                })
            },
        ));

        // Set skip file (BOOTSTRAP.md) — if present, heartbeat is deferred.
        let skip_file = common::workspace_path(&home).join("BOOTSTRAP.md");
        if skip_file.exists() {
            heartbeat_service.set_skip_file(skip_file.to_string_lossy().to_string());
        }

        info!("[Gateway] Heartbeat service wired (bus + handler + skip_file)");
    }

    // M1: Create and wire DeviceService.
    // Mirrors Go's bot_service.go:409-413: devices.NewService(Config{Enabled, MonitorUSB}).
    #[cfg(feature = "devices")]
    {
        if cfg.devices.enabled {
            let device_config = nemesis_devices::service::DeviceServiceConfig {
                enabled: true,
                poll_interval_secs: 30,
                monitor_usb: cfg.devices.monitor_usb,
            };
            let device_service =
                nemesis_devices::service::DeviceService::with_config(device_config);
            // Wire bus sender: device events → outbound messages via bus
            let bus_for_devices = bus.clone();
            device_service.set_bus_sender(Box::new(
                move |channel: &str, chat_id: &str, content: &str| {
                    let msg = nemesis_types::channel::OutboundMessage {
                        channel: channel.to_string(),
                        chat_id: chat_id.to_string(),
                        content: content.to_string(),
                        message_type: String::new(),
                        meta: Default::default(),
                    };
                    bus_for_devices.publish_outbound(msg);
                },
            ));
            // Start monitoring (USB hotplug, etc.) — async, fire-and-forget
            if let Err(e) = device_service.start().await {
                warn!("[Gateway] Device service start note: {} (non-fatal)", e);
            } else {
                info!("[Gateway] Device service started (USB hotplug monitoring)");
            }
        } else {
            info!("[Gateway] Device service disabled (config.json: devices.enabled = false)");
        }
    } // #[cfg(feature = "devices")]

    // Step 13: Create ServiceManager with config
    let bot_config = nemesis_services::BotServiceConfig {
        security_enabled: cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true),
        config_path: config_path.clone(),
        workspace: home.join("workspace"),
        heartbeat_interval_secs,
        heartbeat_enabled: cfg.heartbeat.enabled,
        gateway_host: cfg.gateway.host.clone(),
        gateway_port: cfg.gateway.port as u16,
        llm_logging_enabled: cfg
            .logging
            .as_ref()
            .and_then(|l| l.llm.as_ref())
            .map(|l| l.enabled)
            .unwrap_or(false),
        ..Default::default()
    };
    let svc_mgr = Arc::new(nemesis_services::ServiceManager::with_config(bot_config));

    // Inject adapted services into BotService
    {
        let bot = svc_mgr.get_bot_service();
        #[cfg(feature = "health")]
        {
            bot.inject_health(Arc::new(adapters::HealthServerAdapter::new(
                health_server.clone(),
            )));
        }
        #[cfg(feature = "heartbeat")]
        {
            bot.inject_heartbeat(Arc::new(adapters::HeartbeatServiceAdapter::new(
                heartbeat_service.clone(),
            )));
        }
        #[cfg(not(any(feature = "health", feature = "heartbeat")))]
        let _ = bot;
        // Agent is NOT injected into BotService — its lifecycle is managed directly
        // by AgentLoopServiceAdapter (tray start/stop, gateway shutdown).
    }

    // Step 14: Start basic services
    svc_mgr
        .start_basic_services()
        .map_err(|e| anyhow::anyhow!("Error starting basic services: {}", e))?;

    // W2 P4: board autopilot 启动同步——store 为真相源，删孤儿/补登记/跟随
    // （必须在 cron.start 之前完成，防同步窗口内 job 已开始调度）。
    #[cfg(feature = "board")]
    if let Some(store) = board_store.as_ref() {
        match nemesis_web::handlers::board::sync_autopilot_jobs(&cron_service, store) {
            Ok(n) if n > 0 => {
                info!("[Gateway] Board autopilot sync: {n} rule(s) re-armed from store")
            }
            Ok(_) => {}
            Err(e) => warn!("[Gateway] Board autopilot sync failed: {}", e),
        }
    }

    // Start cron scheduler (after on_job handler is wired).
    // Mirrors Go's bot_service.go:571-579 cronSvc.Start().
    // 提取为嵌套 fn：await_holding_lock 是数据流型 lint，只认函数级 allow，
    // 不认语句级 attribute——函数级 allow 必须挂在这个小 fn 上而非几千行的
    // run() 上。
    #[allow(clippy::await_holding_lock)]
    async fn start_cron_scheduler(
        cron_service: &std::sync::Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    ) {
        // 启动序列唯一持有者：此时 cron handler 未运行、无并发 lock 竞争者，
        // std guard 跨这一次性 start().await 无实际死锁风险（start(&self) 的
        // future 借用锁内数据，结构性无法先放锁；彻底解 = Arc 化 CronService
        // 去掉外层 std::Mutex，见 goal 文档债务记录）。
        let cron = cron_service.lock().unwrap();
        if let Err(e) = cron.start().await {
            warn!("[Gateway] Cron service start note: {}", e);
        } else {
            info!("[Gateway] Cron scheduler started");
        }
    }
    start_cron_scheduler(&cron_service).await;
    // H1 (U12) armed gate：这里只 start 不 arm——arm() 被移到 Step 17 之后
    // （agent 已订阅 + web 已 bind）。此前 arm 挂在本处是 BUG #49（2026-08-28）
    // 的根因：boot 顺序是 arm(Step14) → web bind/state 写盘(Step17) →
    // agent_adapter.start() 才订阅 bus inbound(旧 Step18)，中间没有任何
    // 订阅者。overdue 的持久化 job 在 arm 后第一个 1s tick 即 fire，消息
    // publish 进 tokio broadcast 时零订阅者 = 静默丢弃（cron fire-and-forget
    // 照记 last_status=ok，agent 的 LLM 一次不发）。空载时间隙 <1s 订阅赢，
    // 负载下间隙拉开 >1s 必丢——测试全部通过/失败随负载轮换的根源。
    // tick 调度器在 disarm 状态下空转（见 service.rs H1 gate），晚 arm 无
    // 副作用，只是把 fire 时机推迟到"订阅者就位"之后。

    // Step 14b: Start AgentLoop's bus processing（原 Step 18 上移，BUG #49）
    // 订阅必须在所有 inbound 生产者上线之前完成：
    //   - web server（Step 17 起 accept WS 消息 → bus.publish_inbound）；
    //   - cron.arm()（下移到 Step 17 之后，armed 后第一个 tick 即 fire）。
    // tokio broadcast 零订阅者时 publish 即丢，所以顺序不变量是：
    // agent 订阅 → web 上线 → cron arm。
    if let Err(e) = agent_adapter.start() {
        warn!("[Gateway] Agent adapter start note: {}", e);
    }
    info!("[Gateway] Agent loop started via adapter, listening on bus");

    // Step 15: Print agent startup info
    print_agent_startup_info(&home, initial_tool_count);

    // L3: Bridge logger → SSE EventHub for real-time log streaming to Dashboard.
    // Mirrors Go's bot_service.go:674-688: logger.SetLogHook() → eventHub.
    //
    // Two paths:
    //   1. GlobalSseLogLayer installed in `init_logger_from_config` intercepts every
    //      `tracing::info!` / `tracing::warn!` / etc. across the codebase (~680 sites).
    //      This is the main path — most production logging goes through tracing macros.
    //   2. Legacy `NemesisLogger::set_hook` captures the rare `logger.log()` call (mostly tests
    //      these days). Kept for backwards compatibility.
    {
        let event_hub = web_server.event_hub().clone();
        nemesis_logger::set_global_log_callback(move |ev: nemesis_logger::SseLogEvent| {
            let data = serde_json::json!({
                "seq": ev.seq,
                "level": ev.level,
                "timestamp": ev.timestamp,
                "component": ev.component,
                "target": ev.target,
                "source": ev.source,
                "message": ev.message,
                "fields": ev.fields,
                "file": ev.file,
                "line": ev.line,
            });
            event_hub.publish(nemesis_web::events::EVENT_LOG, data);
        });
        info!("[Gateway] Tracing → SSE EventHub bridge connected (GlobalSseLogLayer)");
    }
    if let Some(logger) = nemesis_logger::global() {
        let event_hub = web_server.event_hub().clone();
        logger.set_hook(Box::new(move |entry: nemesis_logger::logger::LogEntry| {
            let data = serde_json::json!({
                "level": entry.level,
                "timestamp": entry.timestamp,
                "component": entry.component,
                "message": entry.message,
            });
            event_hub.publish(nemesis_web::events::EVENT_LOG, data);
        }));
        info!("[Gateway] Logger → SSE EventHub bridge connected");
    }

    // Step 16: Start outbound dispatch (bus outbound → WebSocket sessions)
    //  MSG: 目前这里暂不需要了，因为我们通过主通道直接来收发 web 消息了
    //let dispatch_bus = bus.clone();
    //let dispatch_session_mgr = web_server.session_manager().clone();
    //let dispatch_handle = tokio::spawn(async move {
    //    nemesis_web::server::dispatch_outbound(dispatch_bus, dispatch_session_mgr).await;
    //});
    //info!("[Gateway] Outbound dispatch started");

    // Step 17: Start WebServer in background
    let web_shutdown_rx = svc_mgr.subscribe_shutdown();
    let (bound_tx, bound_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    // Create internal command channel (web handler → gateway logic)
    let (internal_cmd_tx, internal_cmd_rx) =
        tokio::sync::mpsc::channel::<nemesis_web::internal::InternalCommand>(16);
    web_server.set_internal_cmd_tx(internal_cmd_tx);

    // 在 web server 接受请求之前，按 config.voice.json 自动初始化已启用的语音引擎
    // （STT/TTS/speaker）。这样 dashboard 查 engine_status 拿到的就是真值，前端 chat
    // 页一次询问即可正确反映按钮可用状态，无需轮询。
    #[cfg(feature = "voice")]
    {
        nemesis_web::handlers::voice::init_engines_from_config(&home.join("workspace")).await;
    }

    let web_handle = tokio::spawn(async move {
        if let Err(e) = web_server
            .start_with_shutdown(web_shutdown_rx, Some(bound_tx))
            .await
        {
            error!("[Gateway] Web server error: {}", e);
        }
    });
    info!(
        "[Gateway] Web server starting on {}:{}",
        web_bind_host, web_port
    );

    // Wait for the actual bound address (sent immediately after TcpListener::bind)
    let real_port: i64 = match bound_rx.await {
        Ok(addr) => {
            info!("[Gateway] Web server bound to {}", addr);
            // Swarm M3（§5.4）：对外资产基址落槽。G9（2026-09-09 结构修复）：
            // ① 只在 socket 真实监听所有网卡（unspecified 绑定）时广告 LAN
            //   IP；回环绑定如实广告 127.0.0.1——回环绑定 + 广告 LAN IP =
            //   承诺不可达 URL（G9 病灶）。
            // ② LAN IP 的选择走集群注册表网段匹配（select_advertised_lan_ip
            //   ，static peers 此时已入表），并挂 30s 自愈任务——UDP 发现的
            //   peer 稍后入表、多重网卡/DHCP 换 IP 都自动跟随，bundle 重签
            //   即走 HTTP 老路。
            #[cfg(all(feature = "board", feature = "cluster"))]
            {
                let lan_ip = cluster_adapter
                    .as_ref()
                    .and_then(|ca| select_lan_ip_for_advertisement(ca.cluster()));
                let host = advertise_host_for(addr.ip(), lan_ip);
                let base_url = format!("http://{host}:{}", addr.port());
                board_asset_url_slot.set(base_url.clone());
                // 落盘一份：board_asset 工具 publish 时读取（跨进程一致）。
                let url_path =
                    nemesis_path::resolve_asset_node_url_path_in_workspace(&home.join("workspace"));
                if let Some(parent) = url_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&url_path, &base_url) {
                    warn!("[Gateway] asset node url persist failed: {}", e);
                }

                // G9 自愈：每 30s 按最新注册表重算对外基址，变化才更新槽与
                // 落盘（UDP 发现的 peer 入表 / 本机 IP 变化后 bundle 广告
                // 自动修正）。槽是可更新句柄（AdvertisedUrl），刷新无碍
                // dispatch 签发链。
                let heal_slot = board_asset_url_slot.clone();
                let heal_home = home.clone();
                let heal_port = addr.port();
                let heal_adapter = cluster_adapter.clone();
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    ticker.tick().await; // 首个 tick 立即返回，跳过（bind 刚算过）
                    loop {
                        ticker.tick().await;
                        let Some(ref ca) = heal_adapter else {
                            continue;
                        };
                        let cluster = ca.cluster();
                        if !cluster.is_running() {
                            continue;
                        }
                        let host = select_lan_ip_for_advertisement(cluster)
                            .unwrap_or_else(|| "127.0.0.1".to_string());
                        let base_url = format!("http://{host}:{heal_port}");
                        if heal_slot.get().as_deref() == Some(base_url.as_str()) {
                            continue;
                        }
                        heal_slot.set(base_url.clone());
                        let url_path = nemesis_path::resolve_asset_node_url_path_in_workspace(
                            &heal_home.join("workspace"),
                        );
                        if let Some(parent) = url_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(&url_path, &base_url);
                        info!(
                            "[Gateway] Asset base url healed: {base_url} (cluster registry changed)"
                        );
                    }
                });
            }
            addr.port() as i64
        }
        Err(_) => {
            // oneshot 发送端被 drop = web 任务在报告绑定地址前就退出（典型：
            // build_router panic——panic 只落 stderr，error! 臂都到不了）。
            // 此时 web 完全不可用，绝不是"退回 config 端口还能服务"，
            // 必须 error 级别如实告警（2026-08-31 A3 路由 panic 事故教训）。
            error!(
                "[Gateway] Web server task exited without reporting a bound address; \
                 web UI/API are NOT serving (check stderr for a task panic)"
            );
            web_port
        }
    };

    // Update gateway state with actual web port
    {
        let state_path =
            nemesis_path::resolve_gateway_state_path_in_workspace(&common::workspace_path(&home));
        let state_json = serde_json::json!({
            "pid": std::process::id(),
            "web_host": web_display_host,
            "web_port": real_port,
        });
        if let Err(e) = std::fs::write(&state_path, state_json.to_string()) {
            warn!("[Gateway] Failed to update gateway state: {}", e);
        } else {
            info!("[Gateway] Gateway state updated: port={}", real_port);
        }
    }

    // Step 17: HealthServer is started by BotService (svc_mgr.start_bot() below)
    // via start_services() → services.health.start(). No separate spawn needed here.
    info!(
        "[Gateway] Health server will be started by bot service on {}:{}",
        &cfg.gateway.host, cfg.gateway.port
    );

    // Step 18: Arm cron（原 agent_adapter.start() 位置，BUG #49 调序后）
    // 此时序不变量全部就位：agent 已订阅 bus（Step 14b）+ web 已 bind 且
    // gateway state 已写盘（上方 Step 17 尾部）——armed 后第一个 tick fire
    // 的 overdue job，其消息有订阅者接、deliver=true 的回复有 web channel 投。
    // fresh-process 保护语义不变：arm 之前的一切启动流程仍在 disarm 下跑。
    {
        let cron = cron_service.lock().unwrap();
        cron.arm();
    }

    // Step 19: Start bot service (for state tracking)
    if let Err(e) = svc_mgr.start_bot() {
        warn!("[Gateway] Bot service start note: {}", e);
        // Non-fatal: the real services are already started above
    }

    // Step 20: Compute display URLs (real_port already resolved via oneshot in Step 17)
    let _web_url = format!("http://{}:{}", web_display_host, real_port);
    let _chat_url = format!("http://{}:{}/chat/", web_display_host, real_port);

    // Step 21: Print startup banner
    let enabled_channels = count_enabled_channels(&cfg);
    print_gateway_banner(
        &web_display_host,
        real_port,
        &cfg.channels.web.auth_token,
        enabled_channels,
        &cfg.gateway.host,
        cfg.gateway.port,
    );

    // Verify web server is listening
    let listen_addr = format!("{}:{}", web_display_host, real_port);
    println!("  Checking web server on {}...", listen_addr);
    match tokio::net::TcpStream::connect(&listen_addr).await {
        Ok(_) => println!("  OK Web server is listening"),
        Err(e) => println!("  WARNING: Web server not yet listening: {}", e),
    }

    // Mark as ready (mirrors Go's automatic readiness after HTTP server starts)
    #[cfg(feature = "health")]
    {
        health_server.set_ready(true);
    }

    // Create and start ProcessManager for plugin window lifecycle + dedup
    #[cfg(feature = "desktop")]
    let process_manager = Arc::new(nemesis_desktop::process::ProcessManager::new());
    #[cfg(feature = "desktop")]
    {
        if let Err(e) = process_manager.start().await {
            warn!(
                "[Gateway] ProcessManager start note: {} (non-fatal, plugin windows will use fallback)",
                e
            );
        } else {
            info!(
                "[Gateway] ProcessManager started (WS server on port {})",
                process_manager.ws_port()
            );
        }
    }

    // Wire up ApprovalManager: WebApprovalManager → SecurityPlugin auditor
    // M7（devtool-upgrade 阶段 5）：审批交互同构替换为 Dashboard 审批卡——
    // auditor "ask" 规则触发时广播 SSE `approval-requested`，用户在
    // Dashboard 点批准/拒绝 → WSAPI `approval.respond` → mpsc 解除阻塞。
    // 全平台可用（desktop WebView 内嵌同一 Dashboard，天然生效），因此
    // 装配移出 desktop cfg 门。恢复弹窗方案：ApprovalPopupAdapter 保留
    // （#[allow(dead_code)]，见其头注释）。
    #[cfg(feature = "security")]
    {
        if let Some(ref plugin) = security_plugin {
            let auditor = plugin.auditor();
            // F3: 审批记忆规则表热载器（auditor 查询侧 + 审批卡写入侧共用
            // 同一磁盘文件 `<workspace>/config/approval_rules.json`）。
            let approval_rules_path = nemesis_path::resolve_approval_rules_path_in_workspace(
                &shared_resources.workspace_dir(),
            );
            let approval_rules_hot = Arc::new(nemesis_config::HotReloader::new(
                approval_rules_path.clone(),
                nemesis_security::approval_rules::load_rules,
            ));
            auditor.set_approval_rules(approval_rules_hot);
            let web_mgr = Arc::new(crate::web_approval::WebApprovalManager::new(
                shared_resources.agent_event_tx.clone(),
                Some(approval_rules_path),
            ));
            // K4 (b)（devtool-upgrade 阶段 7）：IM 通道审批卡 + 组合分流——
            // web/无上下文 → web_mgr（Dashboard 卡片，现状不变）；IM 通道
            // → ChannelApprovalManager（审批卡回发起对话 + /approve|/deny
            // 回执）。skill_manage / memory gate / responder 桥仍指 web_mgr
            // （那三处是 dashboard 语义），只有 auditor 的 manager 换组合。
            let channel_mgr = Arc::new(crate::channel_approval::ChannelApprovalManager::new(
                bus.clone(),
            ));
            // watcher 装配走 spawn_watcher（订阅先于 spawn）：任务内订阅
            // 存在回执丢失窗口——窗口内的 /approve 被 broadcast 静默丢弃，
            // 用户批复等满超时被误拒（2026-09-08 全量实证根因）。
            channel_mgr.spawn_watcher();
            let adapter: Arc<dyn nemesis_security::auditor::ApprovalManager> =
                Arc::new(crate::channel_approval::CompositeApprovalManager::new(
                    web_mgr.clone(),
                    channel_mgr,
                ));
            let responder: Arc<dyn nemesis_types::agent::ApprovalResponder> = web_mgr.clone();
            auditor.set_approval_manager(adapter.clone());
            // Bridge the same approval manager to `skill_manage` write approval.
            *shared_resources.approval_slot.write() = Some(adapter.clone());
            // P2: bridge the same approval manager to the agent's memory write/forget
            // gate — agent memory_store/forget now pop up for approval, never
            // bypassed by YOLO/auto. No-op if no memory executor was stashed.
            #[cfg(feature = "memory")]
            {
                agent_loop.set_memory_approval_gate(Arc::new(GatewayMemoryGate::new(adapter)));
            }
            // M7: dashboard 审批卡的响应端点经 AgentLoop 的 responder 槽触达
            // （nemesis-web approval handler 读 agent_loop.approval_responder()）。
            agent_loop.set_approval_responder(responder);
            // X2 (U8 refinement): reflect interactive-approval reachability
            // in the merged context snapshot's `# Runtime Policy` section.
            agent_loop.set_interactive_approval(true);
            info!("[Gateway] Approval manager wired (dashboard web approval, M7)");
            // P5: attach the LLM guardian judge for CRITICAL-op semantic review.
            plugin.set_judge(Arc::new(GatewayLlmJudge {
                provider: llm_provider.clone(),
                model: model_name.clone(),
            }));
            info!("[Gateway] Guardian LLM judge attached (CRITICAL-op review)");
        }
    }

    // F7（devtool-upgrade 阶段 5）：question 工具的 Dashboard 提问 broker。
    // 与审批不同，提问是交互动作不是安全动作——不依赖 security feature，
    // 无 cfg 门（gateway 跑起来就有 Dashboard，提问天然可答）。broker 同时
    // 扮演两个角色：question 工具的阻塞端（SharedResources.question_slot，
    // SharedToolConfig 建槽时已克隆同一 Arc，此处晚填即生效）+ WSAPI
    // question.respond/pending 的响应端（AgentLoop responder 槽）。
    // J5（devtool-upgrade 阶段 6）：同一 Arc 再挂 AgentLoop asker 槽——
    // doom-loop escalation 审批卡（agents.doom_loop_approval，默认关）与
    // question 工具共用同一提问通路与作答 UI，不新造审批协议。
    {
        let broker = Arc::new(crate::question_broker::WebQuestionBroker::new(
            shared_resources.agent_event_tx.clone(),
        ));
        *shared_resources.question_slot.write() = Some(broker.clone());
        agent_loop.set_question_responder(broker.clone());
        agent_loop.set_question_asker(broker);
        info!(
            "[Gateway] Question broker wired (dashboard question card, F7 + doom-loop approval, J5)"
        );
    }

    // Internal command loop: /api/internal → open_plugin_window / open_browser
    {
        #[cfg(all(feature = "desktop", not(target_os = "android")))]
        let pm = Arc::clone(&process_manager);
        let url = format!("http://{}:{}", web_display_host, real_port);
        let token = cfg.channels.web.auth_token.clone();
        let mut rx = internal_cmd_rx;
        // (BUG #31, quality-hardening goal 冲刺 S11e) `nemesisbot shutdown`
        // 的 HTTP 臂经 POST /api/internal {"cmd":"shutdown"} 落到这里的
        // Shutdown 变体：与托盘 Quit 同源的优雅停机——置全局标志后调用
        // ServiceManager.shutdown()，其 broadcast 让 Step 23 的
        // wait_for_shutdown 返回，进入 Step 24 统一善 teardown。
        let shutdown_svc_internal = Arc::clone(&svc_mgr);
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    nemesis_web::internal::InternalCommand::OpenDashboard => {
                        #[cfg(all(feature = "desktop", not(target_os = "android")))]
                        {
                            info!("[Gateway] Internal command: open_dashboard");
                            let _ = open_plugin_window(&pm, "dashboard", &url, &token);
                        }
                        #[cfg(not(all(feature = "desktop", not(target_os = "android"))))]
                        {
                            let _ = (&url, &token);
                            info!(
                                "[Gateway] Internal command: open_dashboard (no desktop / android)"
                            );
                        }
                    }
                    nemesis_web::internal::InternalCommand::Shutdown => {
                        info!("[Gateway] Internal command: shutdown via /api/internal (BUG #31)");
                        #[cfg(not(target_os = "android"))]
                        trigger_global_shutdown();
                        shutdown_svc_internal.shutdown();
                    }
                }
            }
        });
        info!("[Gateway] Internal command listener started");
    }

    // T8（多模态 goal 2026-09-03）：uploads 暂存目录 TTL 清扫（启动扫一次 +
    // 每 6 小时一次，7 天 TTL）。uploads 是 temp 语义；被清扫文件的历史引用
    // 由 T6 水合层诚实降级 `[图片已失效]`，不依赖此处。路径唯一真相源
    // nemesis-path resolve_uploads_dir_in_workspace。
    nemesis_web::handlers::upload::spawn_uploads_sweeper();
    info!("[Gateway] uploads TTL sweeper started (7d TTL, 6h interval)");

    // Ctrl+C / SIGINT 优雅停机臂（2026-08-31 web-dead 停机事故教训）：
    // 此前 gateway 停机只有两条路——/api/internal（骑 web，web 任务一死即全断）
    // 和托盘 Quit（要求桌面托盘存活）。控制台 Ctrl+C 落 std 默认处置=硬终止，
    // Step 24 善后全部跳过。补一条与托盘 Quit 同源的信号臂。
    // 注意：msys/bash `&` 起的后台子进程继承 SIG_IGN(SIGINT)，该启动方式下
    // 本臂不会触发（Start-Process / 前台控制台启动则有效）。
    #[cfg(not(target_os = "android"))]
    {
        let svc_mgr_signal = Arc::clone(&svc_mgr);
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("[Gateway] Ctrl+C received, initiating graceful shutdown");
                trigger_global_shutdown();
                svc_mgr_signal.shutdown();
            }
        });
    }

    // Step 22: Configure system tray (desktop only)
    #[cfg(all(feature = "desktop", not(target_os = "android")))]
    {
        use nemesis_desktop::PlatformTray;

        let mut tray = PlatformTray::new();

        // Set cluster callbacks — tray controls both config files + runtime
        #[cfg(feature = "cluster")]
        {
            if let Some(ref ca) = cluster_adapter {
                let home_for_start = home.clone();
                let ca_start = ca.clone();
                tray.set_on_cluster_start(Box::new(move || {
                    // Write config.json cluster.enabled = true
                    let cfg_path = home_for_start.join("config.json");
                    if let Ok(content) = std::fs::read_to_string(&cfg_path)
                        && let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&content)
                    {
                        if cfg.get("cluster").is_none() {
                            cfg["cluster"] = serde_json::json!({});
                        }
                        if let Some(obj) = cfg.get_mut("cluster").and_then(|c| c.as_object_mut()) {
                            obj.insert("enabled".to_string(), serde_json::json!(true));
                            if let Ok(updated) = serde_json::to_string_pretty(&cfg) {
                                let _ = std::fs::write(&cfg_path, updated);
                            }
                        }
                    }
                    // Write config.cluster.json enabled = true
                    let cluster_cfg_path = nemesis_path::resolve_cluster_config_path_in_workspace(
                        &common::workspace_path(&home_for_start),
                    );
                    if let Ok(content) = std::fs::read_to_string(&cluster_cfg_path)
                        && let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&content)
                        && let Some(obj) = cfg.as_object_mut()
                    {
                        obj.insert("enabled".to_string(), serde_json::json!(true));
                        if let Ok(updated) = serde_json::to_string_pretty(&cfg) {
                            let _ = std::fs::write(&cluster_cfg_path, updated);
                        }
                    }
                    if let Err(e) = ca_start.start() {
                        tracing::warn!("[Gateway] Tray: failed to start cluster: {}", e);
                    }
                }));

                let home_for_stop = home.clone();
                let ca_stop = ca.clone();
                tray.set_on_cluster_stop(Box::new(move || {
                    if let Err(e) = ca_stop.stop() {
                        tracing::warn!("[Gateway] Tray: failed to stop cluster: {}", e);
                    }
                    // Write config.cluster.json enabled = false
                    let cluster_cfg_path = nemesis_path::resolve_cluster_config_path_in_workspace(
                        &common::workspace_path(&home_for_stop),
                    );
                    if let Ok(content) = std::fs::read_to_string(&cluster_cfg_path)
                        && let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&content)
                        && let Some(obj) = cfg.as_object_mut()
                    {
                        obj.insert("enabled".to_string(), serde_json::json!(false));
                        if let Ok(updated) = serde_json::to_string_pretty(&cfg) {
                            let _ = std::fs::write(&cluster_cfg_path, updated);
                        }
                    }
                    // Write config.json cluster.enabled = false
                    let cfg_path = home_for_stop.join("config.json");
                    if let Ok(content) = std::fs::read_to_string(&cfg_path)
                        && let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&content)
                        && let Some(obj) = cfg.get_mut("cluster").and_then(|c| c.as_object_mut())
                    {
                        obj.insert("enabled".to_string(), serde_json::json!(false));
                        if let Ok(updated) = serde_json::to_string_pretty(&cfg) {
                            let _ = std::fs::write(&cfg_path, updated);
                        }
                    }
                }));
            }
        }

        let start_adapter = Arc::clone(&agent_adapter);
        tray.set_on_start(Box::new(move || {
            if let Err(e) = start_adapter.start() {
                tracing::warn!("[Gateway] Tray: failed to start agent: {}", e);
            }
        }));

        let stop_adapter = Arc::clone(&agent_adapter);
        tray.set_on_stop(Box::new(move || {
            if let Err(e) = stop_adapter.stop() {
                tracing::warn!("[Gateway] Tray: failed to stop agent: {}", e);
            }
        }));

        // E-stop / release: tray is in-process, capture the same EstopState Arc
        // the agent loop reads (shared_resources.estop). trigger()/release() are
        // &self on a thread-safe AtomicBool+watch, safe to call from the tray thread.
        let estop_engage = Arc::clone(&shared_resources.estop);
        tray.set_on_estop(Box::new(move || {
            estop_engage.trigger();
            tracing::info!("[Gateway] Tray: e-stop engaged");
        }));

        let estop_release = Arc::clone(&shared_resources.estop);
        tray.set_on_release(Box::new(move || {
            estop_release.release();
            tracing::info!("[Gateway] Tray: e-stop released");
        }));

        let pm = Arc::clone(&process_manager);
        let dashboard_url = _web_url.clone();
        let dashboard_token = cfg.channels.web.auth_token.clone();
        tray.set_on_open_dashboard(Box::new(move || {
            let _ = open_plugin_window(&pm, "dashboard", &dashboard_url, &dashboard_token);
        }));

        let chat_url = _chat_url.clone();
        tray.set_on_open_chat(Box::new(move || {
            let _ = open_browser(&chat_url);
        }));

        let shutdown_svc = Arc::clone(&svc_mgr);
        tray.set_on_quit(Box::new(move || {
            trigger_global_shutdown();
            shutdown_svc.shutdown();
        }));

        // Start the tray.
        //
        // Windows: runs on a dedicated thread (winit allows off-main-thread via
        //          with_any_thread). macOS: winit's EventLoop MUST run on the
        //          main thread, so hand the configured tray to the main thread
        //          (see nemesis_desktop::main_thread_handoff) which runs the
        //          event loop there. The gateway itself continues on this worker.
        #[cfg(target_os = "macos")]
        {
            nemesis_desktop::main_thread_handoff::deliver(tray);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _tray_handle = tray.run();
        }
        info!("[Gateway] System tray started");
        println!("  OK System tray started");
    }

    // Step 23: Wait for shutdown signal
    svc_mgr.wait_for_shutdown().await;

    // Step 24: Graceful shutdown
    println!();
    println!("Shutting down...");
    svc_mgr.shutdown();

    // F-L2: graceful Forge shutdown — flush buffered aggregation + stop the
    // background loops cleanly. Previously the spawn handle was dropped and the
    // loops were killed by runtime teardown (losing unflushed data / mid-write).
    #[cfg(feature = "forge")]
    {
        if let Some(ref forge) = forge_for_web {
            forge.stop().await;
            info!("[Gateway] Forge stopped cleanly");
        }
    }

    // Cancel active voice sessions and release ONNX engines
    // so spawn_blocking tasks exit before Runtime drop.
    #[cfg(feature = "voice")]
    {
        nemesis_web::handlers::voice::voice_shutdown().await;
    }

    // Stop ProcessManager (terminates all child processes)
    #[cfg(feature = "desktop")]
    {
        if let Err(e) = process_manager.stop() {
            warn!("[Gateway] ProcessManager stop note: {}", e);
        }
    }

    // Stop scanner chain — kills clamd so it doesn't orphan (holds port 3310,
    // breaks next gateway start). SecurityService trait has no stop hook, so we
    // call SecurityPlugin.stop_scanner directly here.
    #[cfg(feature = "security")]
    {
        if let Some(ref plugin) = security_plugin {
            plugin.stop_scanner().await;
            info!("[Gateway] Scanner chain stopped (clamd killed)");
        }
    }

    // Sandbox: leave SbieSvc RESIDENT on exit — do NOT stop it. The gateway
    // runs non-elevated; stopping the privileged SbieSvc shells out to
    // `KmdUtil.exe stop SbieSvc`, which is denied (needs admin) and pops a GUI
    // "no permission" dialog that blocks shutdown. A running SbieSvc is
    // harmless: `ensure_sandbox_ready` reuses it on next start (see
    // commands/sandbox.rs), and the kernel driver is resident-by-design. Use
    // the elevated `sandbox stop` CLI / dashboard button to fully uninstall.
    //
    // DISABLED — original per-run stop call kept here for reference. To
    // re-enable, the stop MUST go through an ELEVATED path (elevation.rs /
    // runas); a non-elevated stop re-introduces the KmdUtil permission popup.
    // #[cfg(feature = "sandbox")]
    // {
    //     crate::commands::sandbox::stop_service_if_ours(&home);
    // }

    // Close the message bus
    bus.close();

    // C5: gracefully close every LSP session (shutdown → exit → kill) so
    // language-server child processes never outlive the gateway. Previously
    // the tool's sessions were only reaped lazily (idle timeout) or via
    // kill_on_drop at process exit — an abrupt teardown could orphan them.
    let lsp_closed = shared_resources.lsp_manager.shutdown_all().await;
    info!(
        "[Gateway] LSP shutdown: {} language-server session(s) closed",
        lsp_closed
    );

    // Abort background tasks
    web_handle.abort();
    agent_adapter.stop().ok();
    // L6++：项目常驻 loop 收尾（镜像主 agent stop：摘表 + stop + abort 任务）。
    projects_manager.stop_all();
    bridge_outbound_handle.abort();
    //  MSG: 同 step 16 ，目前暂时不用，所以注释掉了
    //dispatch_handle.abort();

    // Stop cluster (adapter handles: agent abort, RPC server, discovery, recovery/sync loops)
    #[cfg(feature = "cluster")]
    {
        if let Some(adapter) = cluster_adapter.take() {
            let _ = adapter.stop();
        }
    }

    // Clean up gateway state file
    let _ = std::fs::remove_file(nemesis_path::resolve_gateway_state_path_in_workspace(
        &common::workspace_path(&home),
    ));

    println!("  OK Gateway stopped");

    // macOS: tell the main-thread tray loop to exit now that cleanup is done.
    // Covers shutdown paths that don't go through the tray "Quit" menu item
    // (e.g. Ctrl+C) — without this the main thread would block in the tray
    // event loop forever while the gateway worker had already finished.
    #[cfg(target_os = "macos")]
    nemesis_desktop::main_thread_handoff::request_exit();

    Ok(())
}

// ---------------------------------------------------------------------------
// GatewayAgentRunner — bridges workflow `agent` nodes to the live AgentLoop
// ---------------------------------------------------------------------------

/// Adapter that lets workflow `agent` nodes drive the gateway's AgentLoop
/// without `nemesis-workflow` needing to depend on `nemesis-agent` (which
/// would pull in a huge transitive dependency closure).
///
/// Session keys are namespaced as `workflow:{agent_id}` so that workflow
/// sessions stay isolated from human user sessions in the SessionStore.
///
/// `tools_used` is currently always empty: the public AgentLoop entry point
/// only returns the final response string. If/when we surface tool-call
/// events, this struct can be extended to capture them without breaking the
/// trait contract.
#[cfg(feature = "workflow")]
struct GatewayAgentRunner {
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
}

#[cfg(feature = "workflow")]
impl GatewayAgentRunner {
    fn new(agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>) -> Self {
        Self { agent_loop }
    }
}

#[cfg(feature = "workflow")]
#[async_trait::async_trait]
impl nemesis_workflow::nodes::AgentRunner for GatewayAgentRunner {
    async fn run_direct(
        &self,
        prompt: &str,
        agent_id: &str,
        max_turns: u32,
        model: Option<&str>,
    ) -> Result<nemesis_workflow::nodes::AgentRunResult, String> {
        // Note: max_turns is currently applied via AgentLoop's own config at
        // construction time. Once we add a per-call override on AgentLoop
        // (e.g. `process_direct_with_options`), this runner should respect
        // it explicitly to prevent runaway workflow agent loops.
        let _ = max_turns;
        // Same applies to model: AgentLoop currently uses its configured
        // default. Log when a override is requested but cannot be honored so
        // the silent-drop pattern from BUG #8 (form collecting model that
        // backend ignored) stays visible.
        if let Some(m) = model {
            tracing::warn!(
                model = %m,
                agent_id = %agent_id,
                "workflow `agent` node requested model override but AgentLoop does not yet support per-call model switching; using default"
            );
        }

        let session_key = format!("workflow:{}", agent_id);
        let response = self.agent_loop.process_direct(prompt, &session_key).await?;

        Ok(nemesis_workflow::nodes::AgentRunResult {
            response,
            tools_used: Vec::new(),
        })
    }
}
