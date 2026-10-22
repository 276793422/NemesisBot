//! W7 对抗套件（A2 全案例 + A9 observer 对抗）。
//!
//! 方法论：fixtures = 真实 cdylib 组件 crate
//! （`plugins/wasm/{translate,activity-log}`），产物缺席时在测试内
//! 按需 `cargo build --target wasm32-wasip2 --release`（fixture crate 自有
//! workspace，target 目录独立于宿主）；构建失败 = 环境不满足，诚实 SKIP。
//! 所有用例走 [`PluginInstaller`] 正式 admit 漏斗（manifest 校验→验签→
//! hash→编译→对账→落位→注册），不手搓注册旁路。
//!
//! ```text
//! cargo test -p nemesis-plugins-wasm --test adversarial
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nemesis_plugins_wasm::host_impl::SecretResolver;
use nemesis_plugins_wasm::install::{AutoApprove, PluginInstaller};
use nemesis_plugins_wasm::limits::PluginLimits;
use nemesis_plugins_wasm::observer::project_event;
use nemesis_plugins_wasm::registry::PluginManager;
use nemesis_plugins_wasm::runtime::ExecContext;
use nemesis_plugins_wasm::{PluginError, PluginTrustState};
use nemesis_types::agent::AgentEvent;

mod common;

struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

/// 只解析 api_key 别名（其余声明名 → NotFound 路径）。
struct OneSecret;

impl SecretResolver for OneSecret {
    fn resolve(&self, alias: &str) -> Option<String> {
        alias
            .ends_with("/api_key")
            .then(|| "s3cr3t-value".to_string())
    }
}

// ---------------------------------------------------------------------------
// fixture 按需构建（产物缓存 OnceLock；并发用例共享同一次构建）
// ---------------------------------------------------------------------------

fn cargo_bin() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

fn fixture_wasm(slug_dir: &'static str, artifact: &'static str) -> Option<PathBuf> {
    static CACHE: std::sync::OnceLock<
        std::collections::HashMap<(&'static str, &'static str), Option<PathBuf>>,
    > = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let mut m = std::collections::HashMap::new();
            m.insert((slug_dir, artifact), build_fixture(slug_dir, artifact));
            m
        })
        .get(&(slug_dir, artifact))
        .cloned()
        .flatten()
}

/// 先找既有产物；没有就现场构建（fixture 自有 workspace → 独立 target-dir）。
fn build_fixture(slug_dir: &str, artifact: &str) -> Option<PathBuf> {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/wasm")
        .join(slug_dir);
    let out = crate_dir
        .join("target/wasm32-wasip2/release")
        .join(format!("{artifact}.wasm"));
    if !out.exists() {
        let status = std::process::Command::new(cargo_bin())
            .args(["build", "--target", "wasm32-wasip2", "--release"])
            .current_dir(&crate_dir)
            .status();
        match status {
            Ok(s) if s.success() => {}
            status => {
                eprintln!(
                    "SKIP adversarial：fixture 构建失败（{slug_dir}: {status:?}）。\
环境要求：rustup target add wasm32-wasip2 + 网络可取 wit-bindgen 依赖"
                );
                return None;
            }
        }
    }
    out.exists().then_some(out)
}

fn skip_reason(what: &str) -> String {
    format!("SKIP adversarial：fixture 缺席（{what}）——见构建失败时的环境说明")
}

// ---------------------------------------------------------------------------
// 装配（正式 admit 漏斗，AutoApprove 直通）
// ---------------------------------------------------------------------------

/// 写插件源目录（plugin.toml + wasm），返回目录路径。
fn stage_plugin(
    root: &Path,
    toml_body: String,
    wasm_src: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let wasm_bytes = std::fs::read(wasm_src)?;
    let sha = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(&wasm_bytes))
    };
    let dir = root.join("src");
    std::fs::create_dir_all(&dir)?;
    // sha 放文件头（TOML 根键必须在任何 table 段之前）。
    let manifest = format!("wasm-sha256 = \"{sha}\"\n{toml_body}\n");
    std::fs::write(dir.join("plugin.toml"), manifest)?;
    std::fs::write(dir.join("plugin.wasm"), &wasm_bytes)?;
    Ok(dir)
}

/// 对抗夹具 manifest：全局上限内收紧 memory / 帧预算 / fuel，
/// 让 mem_bomb / host_flood 用例确定性触顶（而非等全局默认额度）。
const ADVERSARIAL_TOML: &str = r#"api-version = 1
slug = "adversarial"
kind = "tool"
name = "Adversarial"
version = "0.1.0"
description = "对抗测试夹具（复用 translate 行为面）"

[permissions]
x-secret = ["api_key", "extra_key"]

[limits]
fuel = 200_000_000
timeout-ms = 20_000
memory-bytes = 8_000_000
host-call-budget = 64

[config-schema]
greeting = "问候语"
"#;

/// 无 x-secret 的干净 manifest（秘密语义用例换声明集时复用）。
const ACTIVITY_TOML: &str = r#"api-version = 1
slug = "activity-log"
kind = "observer"
name = "ActivityLog"
version = "0.1.0"
description = "对抗夹具 observer（trap marker 行为）"
"#;

fn ctx() -> ExecContext {
    ExecContext {
        session_key: "sess-adv".into(),
        call_id: "call-adv".into(),
    }
}

fn manager_with(limits: PluginLimits, secrets: Arc<dyn SecretResolver>) -> Arc<PluginManager> {
    common::ensure_host_dll();
    let ws = tempfile::tempdir().expect("tempdir");
    let ws_path = ws.path().to_path_buf();
    std::mem::forget(ws); // 用例生命周期 = 进程生命周期；TempDir 泄漏免清理竞态
    // 测试形态 = 目录注入（生产 `new` 解析 exe 旁真实 plugins_wasm/，
    // 并行用例共享真实目录会互相删载荷/摘注册——串扰根因）。
    Arc::new(
        PluginManager::new_with_dirs(&ws_path, &ws_path.join("plugins_wasm"), limits, secrets)
            .expect("manager"),
    )
}

async fn install_adversarial(
    mgr: &Arc<PluginManager>,
    wasm: &Path,
) -> Result<uninstall_guard::Guard, PluginError> {
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), ADVERSARIAL_TOML.to_string(), wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    let outcome = installer.install(&src, true).await?;
    drop(tmp); // 安装已把载荷复制进 plugins_wasm/<slug>/，staging 目录可弃
    assert_eq!(
        outcome.trust,
        PluginTrustState::ReviewRequired,
        "无签名 = 待审"
    );
    assert!(
        outcome.registered.is_none(),
        "新装默认不加载（2026-10-22 语义）"
    );
    // 用例语义 =「用户已开开关」。
    installer
        .set_enabled("adversarial", true)
        .await?
        .expect("loaded");
    Ok(uninstall_guard::Guard {
        mgr: mgr.clone(),
        slug: "adversarial".into(),
    })
}

/// 用例收尾注销（防同进程用例间 slug 冲突；Drop 尽力清理）。
mod uninstall_guard {
    use super::*;
    pub struct Guard {
        pub mgr: Arc<PluginManager>,
        pub slug: String,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = nemesis_plugins_wasm::install::uninstall_blocking(&self.mgr, &self.slug);
        }
    }
}

// ---------------------------------------------------------------------------
// A2 案例集
// ---------------------------------------------------------------------------

/// 内存炸弹被 ResourceLimiter 拦截：guest 分配失败 → trap（非 Timeout、
/// 非进程 OOM、非宿主 panic）。
#[tokio::test(flavor = "multi_thread")]
async fn mem_bomb_trapped_by_limiter() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate_plugin.wasm"));
        return;
    };
    let mgr = manager_with(PluginLimits::default(), Arc::new(NoSecrets));
    let _guard = install_adversarial(&mgr, &wasm).await.expect("install");

    let started = Instant::now();
    let err = mgr
        .execute_tool(
            "plugin.adversarial.translate",
            r#"{"mode":"mem_bomb"}"#.into(),
            ctx(),
        )
        .await
        .expect_err("mem bomb must fail");
    assert!(
        matches!(err, PluginError::Trap(ref m) if {
            let m = m.to_lowercase();
            m.contains("memory") || m.contains("alloc") || m.contains("unreachable")
        }),
        "expected allocation trap, got: {err:?}"
    );
    // memory-bytes=8MB 收紧后应远快于 timeout-ms=20s 墙钟闸。
    assert!(started.elapsed() < Duration::from_secs(20));
}

/// 帧预算耗尽返回 budget-exceeded：guest 收 BudgetExceeded host-error，
/// 回报实际调用次数 ≤ 收紧后的预算 64。
#[tokio::test(flavor = "multi_thread")]
async fn host_call_budget_exhausted_reports_budget() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate_plugin.wasm"));
        return;
    };
    let mgr = manager_with(PluginLimits::default(), Arc::new(NoSecrets));
    let _guard = install_adversarial(&mgr, &wasm).await.expect("install");

    let out = mgr
        .execute_tool(
            "plugin.adversarial.translate",
            r#"{"mode":"host_flood"}"#.into(),
            ctx(),
        )
        .await
        .expect("host_flood completes (guest handles budget error)");
    assert!(
        !out.is_error,
        "guest 应把预算耗尽当正常语义上报: {}",
        out.content
    );
    let count: u32 = out
        .content
        .strip_prefix("budget-exhausted-after=")
        .unwrap_or_else(|| panic!("unexpected content: {}", out.content))
        .parse()
        .expect("call count");
    assert!(
        (60..=64).contains(&count),
        "预算 64：调用次数应贴顶（60-64），got {count}"
    );
}

/// 秘密语义全解：声明且 vault 命中 → 原文可达（只回形态）；声明但 vault
/// 缺失 → NotFound；未声明 → PolicyDenied。三条路都进审计且原文零泄漏由
/// e2e_guest::audit_events_recorded 覆盖，此处钉错误形态区分。
#[tokio::test(flavor = "multi_thread")]
async fn secret_semantics_resolved_notfound_denied() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate_plugin.wasm"));
        return;
    };
    let mgr = manager_with(PluginLimits::default(), Arc::new(OneSecret));
    let _guard = install_adversarial(&mgr, &wasm).await.expect("install");

    // 声明且命中（manifest x-secret 含 api_key）。
    let out = mgr
        .execute_tool(
            "plugin.adversarial.translate",
            r#"{"mode":"secret_probe","name":"api_key"}"#.into(),
            ctx(),
        )
        .await
        .expect("probe api_key");
    assert!(
        out.content.contains(":resolved(len="),
        "got: {}",
        out.content
    );

    // 声明但 vault 无值 → NotFound。
    let out = mgr
        .execute_tool(
            "plugin.adversarial.translate",
            r#"{"mode":"secret_probe","name":"extra_key"}"#.into(),
            ctx(),
        )
        .await
        .expect("probe extra_key");
    assert!(
        out.content.contains(":denied(HostError::NotFound"),
        "声明缺值应 NotFound, got: {}",
        out.content
    );

    // 未声明 → PolicyDenied。
    let out = mgr
        .execute_tool(
            "plugin.adversarial.translate",
            r#"{"mode":"secret_probe","name":"evil_key"}"#.into(),
            ctx(),
        )
        .await
        .expect("probe evil_key");
    assert!(
        out.content.contains(":denied(HostError::PolicyDenied"),
        "未声明应 PolicyDenied, got: {}",
        out.content
    );
}

// ---------------------------------------------------------------------------
// A9 observer 对抗
// ---------------------------------------------------------------------------

/// 安装 activity-log（trap marker 行为见其源码头注）。
async fn install_observer(mgr: &Arc<PluginManager>, wasm: &Path) {
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), ACTIVITY_TOML.to_string(), wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer
        .install(&src, true)
        .await
        .expect("install observer");
    // 测试语义 =「用户已开开关」（2026-10-22：安装 ≠ 加载）。
    installer
        .set_enabled("activity-log", true)
        .await
        .expect("enable observer")
        .expect("observer loaded");
    drop(tmp);
}

fn events_log(mgr: &PluginManager) -> PathBuf {
    mgr.plugin_data_dir("activity-log").join("events.log")
}

fn read_events(mgr: &PluginManager) -> String {
    std::fs::read_to_string(events_log(mgr)).unwrap_or_default()
}

/// 等到谓词成立（100ms 步进）；超时 panic 并带现场。
async fn wait_until(what: &str, secs: u64, mut pred: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if pred() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timeout waiting for {what}");
}

/// 事件洪泛：投递路径零阻塞（try_send 满即丢+计数），消费者仍存活。
#[tokio::test(flavor = "multi_thread")]
async fn observer_flood_drops_without_blocking() {
    let Some(wasm) = fixture_wasm("activity-log", "activity_log_plugin") else {
        eprintln!("{}", skip_reason("activity_log_plugin.wasm"));
        return;
    };
    // 队列深度收紧到 8 → 洪泛必丢（确定性触发丢弃计数）。
    let mut limits = PluginLimits::default();
    limits.observer_queue_depth = 8;
    let mgr = manager_with(limits, Arc::new(NoSecrets));
    install_observer(&mgr, &wasm).await;
    let reg = mgr.get("activity-log").expect("registered");

    // 洪泛 500 条：enqueue 恒不阻塞（有界队列 try_send）。
    let started = Instant::now();
    for i in 0..500 {
        mgr.enqueue_observer_event(format!(
            r#"{{"type":"tool_start","seq":{i},"tool":"exec"}}"#
        ));
    }
    let enqueue_elapsed = started.elapsed();
    assert!(
        enqueue_elapsed < Duration::from_secs(5),
        "enqueue 洪泛不得反压事件路径，took {enqueue_elapsed:?}"
    );

    // 丢弃计数见涨（消费速率 << 投递速率）。
    wait_until("dropped_events > 0", 15, || {
        reg.dropped_events
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
    })
    .await;

    // 消费者存活：队列排干后 events.log 收到事件。
    wait_until("events.log non-empty", 20, || !read_events(&mgr).is_empty()).await;
    assert!(mgr.is_enabled(), "洪泛不得影响子系统总开关（零阻塞语义）");
}

/// observer trap 隔离：带 trap marker 的事件让 guest panic → 该事件丢弃、
/// 泵存活、后续正常事件照常投递（agent 事件路径无感对照）。
#[tokio::test(flavor = "multi_thread")]
async fn observer_trap_isolated_and_pump_continues() {
    let Some(wasm) = fixture_wasm("activity-log", "activity_log_plugin") else {
        eprintln!("{}", skip_reason("activity_log_plugin.wasm"));
        return;
    };
    let mgr = manager_with(PluginLimits::default(), Arc::new(NoSecrets));
    install_observer(&mgr, &wasm).await;
    let reg = mgr.get("activity-log").expect("registered");

    // ① trap 事件：guest panic → observe 失败（warn 日志 + 事件丢弃），
    //   不得毒化 worker / 不得 panic 宿主。
    mgr.enqueue_observer_event(r#"{"type":"tool_start","trap":true}"#.into());
    // ② 后续正常事件照常投递（失败隔离的直接证据）。
    mgr.enqueue_observer_event(r#"{"type":"tool_end","tool":"exec"}"#.into());

    wait_until("post-trap event lands", 20, || {
        read_events(&mgr).contains("tool_end")
    })
    .await;
    let body = read_events(&mgr);
    assert!(
        !body.contains("\"trap\":true"),
        "trap 事件不得出现在消费面（已被丢弃）: {body}"
    );
    // dropped_events 计的是「队列满/满员丢帧」，trap 走消费失败路径——
    // 两者分流是投递语义的一部分。
    assert_eq!(
        reg.dropped_events
            .load(std::sync::atomic::Ordering::Relaxed),
        0,
        "trap 消费失败不得计入队列丢弃"
    );
    assert!(mgr.is_enabled());
}

/// v1 事件投影零内容体：args_preview / result_preview / chat_id 等
/// 内容体字段不出宿主（结构性断言：投影键集 == 白名单）。
#[test]
fn projected_event_has_no_content_body() {
    // serde_json Map 是字典序——键集断言用排序后向量比较。
    let sorted_keys = |json: &str| -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(json).expect("valid json");
        let mut keys: Vec<String> = v.as_object().expect("object").keys().cloned().collect();
        keys.sort_unstable();
        keys
    };

    let started = AgentEvent::ToolStarted {
        session_key: "sess-1".into(),
        chat_id: "chat-1".into(),
        call_id: "call-1".into(),
        tool: "exec".into(),
        args_preview: "CONTENT-BODY-ARGS-SENTINEL".into(),
    };
    let json = project_event(&started).expect("tool_start projects");
    assert_eq!(
        sorted_keys(&json),
        ["call_id", "session_key", "tool", "ts_ms", "type"],
        "投影键集必须等于白名单（A9：零内容体）"
    );
    assert!(
        !json.contains("CONTENT-BODY-ARGS-SENTINEL"),
        "args 泄漏: {json}"
    );
    assert!(!json.contains("chat-1"), "chat_id 不出宿主: {json}");

    let finished = AgentEvent::ToolFinished {
        session_key: "sess-1".into(),
        chat_id: "chat-1".into(),
        call_id: "call-1".into(),
        tool: "exec".into(),
        duration_ms: 5,
        ok: false,
        result_preview: "CONTENT-BODY-RESULT-SENTINEL".into(),
    };
    let json = project_event(&finished).expect("tool_end projects");
    assert!(
        !json.contains("CONTENT-BODY-RESULT-SENTINEL"),
        "result 泄漏: {json}"
    );
    assert_eq!(
        sorted_keys(&json),
        [
            "call_id",
            "duration_ms",
            "ok",
            "session_key",
            "tool",
            "ts_ms",
            "type"
        ],
        "tool_end 投影键集 = 白名单 + 形状字段"
    );
    assert!(json.contains("\"ok\":false"), "ok 位在投影内（形状非内容）");

    // 其余变体（含内容体 TodoUpdated / RoundText）v1 一律不投影。
    assert!(
        project_event(&AgentEvent::ModeChanged {
            session_key: "s".into(),
            chat_id: "c".into(),
            mode: "plan".into(),
        })
        .is_none()
    );
}
