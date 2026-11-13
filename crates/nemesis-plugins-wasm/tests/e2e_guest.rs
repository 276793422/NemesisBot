//! spike e2e：真 guest 组件经宿主装配漏斗 + fresh-store 链路全跑通。
//!
//! fixture = `plugins/wasm/{translate,activity-log}` 编译产物
//! （wasm32-wasip2 release）。fixture 缺席时诚实 SKIP（打印原因后返回）——
//! 完整跑法见文件尾注释：
//!
//! ```text
//! cd plugins/wasm/translate && cargo build --target wasm32-wasip2 --release
//! cd plugins/wasm/activity-log && cargo build --target wasm32-wasip2 --release
//! cargo test -p nemesis-plugins-wasm --test e2e_guest
//! ```

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nemesis_plugins_wasm::host_impl::{ConfigWriter, LlmInvoker, SecretResolver, SecretWriter};
use nemesis_plugins_wasm::install::{
    AutoApprove, InstallApprover, InstallReview, PluginInstaller, WASM_MAX_BYTES,
};
use nemesis_plugins_wasm::limits::PluginLimits;
use nemesis_plugins_wasm::registry::PluginManager;
use nemesis_plugins_wasm::runtime::ExecContext;
use nemesis_plugins_wasm::{PluginError, PluginTrustState};

struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

/// 提供一个真实凭据（api_key 别名命中），验证 secret-get allowed 路径。
struct OneSecret;

impl SecretResolver for OneSecret {
    fn resolve(&self, alias: &str) -> Option<String> {
        alias
            .ends_with("/api_key")
            .then(|| "s3cr3t-value".to_string())
    }
}

/// 读审计目录里的当日审计文件全文（找不到 = 空）。
fn audit_body(audit_dir: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(audit_dir) else {
        return String::new();
    };
    let mut body = String::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("security_audit_") && name.ends_with(".log") {
            body.push_str(&std::fs::read_to_string(e.path()).unwrap_or_default());
        }
    }
    body
}

/// fixture wasm 路径（不存在 = None → 诚实 SKIP）。
fn fixture_wasm(slug_dir: &str, artifact: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../plugins/wasm/{slug_dir}/target/wasm32-wasip2/release/{artifact}.wasm"
    ));
    p.exists().then_some(p)
}

fn skip_reason(slug_dir: &str) -> String {
    format!(
        "SKIP e2e_guest：fixture 缺席（plugins/wasm/{slug_dir} 未编译）。\
完整跑法：cd plugins/wasm/{slug_dir} && cargo build --target wasm32-wasip2 --release，\
再 cargo test -p nemesis-plugins-wasm --test e2e_guest"
    )
}

/// 写插件源目录（plugin.toml + wasm），返回目录与 wasm 字节。
fn stage_plugin(
    root: &Path,
    toml_body: String,
    wasm_src: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let wasm_bytes = std::fs::read(wasm_src)?;
    if wasm_bytes.len() as u64 > WASM_MAX_BYTES {
        return Err("fixture exceeds 64MiB".into());
    }
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

const TRANSLATE_TOML: &str = r#"api-version = 1
slug = "translate"
kind = "tool"
name = "Translate"
version = "0.1.0"
description = "e2e fixture tool"

[permissions]
x-secret = ["api_key"]

[limits]
fuel = 20_000_000
timeout-ms = 10_000

[config-schema]
greeting = "问候语"
"#;

const ACTIVITY_TOML: &str = r#"api-version = 1
slug = "activity-log"
kind = "observer"
name = "ActivityLog"
version = "0.1.0"
description = "e2e fixture observer"
"#;

/// 接管变体（工具接管层）：根级 `takeover` 声明（必须在任何
/// table 段之前——TOML 根键纪律与 wasm-sha256 同款）。
const TRANSLATE_TAKEOVER_TOML: &str = r#"api-version = 1
slug = "translate"
kind = "tool"
name = "Translate"
version = "0.1.0"
description = "e2e fixture tool (takeover variant)"
takeover = ["web_fetch"]

[permissions]
x-secret = ["api_key"]

[limits]
fuel = 20_000_000
timeout-ms = 10_000

[config-schema]
greeting = "问候语"
"#;

fn ctx() -> ExecContext {
    ExecContext {
        session_key: "sess-e2e".into(),
        call_id: "call-e2e".into(),
    }
}

async fn fresh_manager_secrets(
    secrets: Arc<dyn SecretResolver>,
) -> Result<(tempfile::TempDir, Arc<PluginManager>), PluginError> {
    common::ensure_host_dll();
    let ws = tempfile::tempdir().map_err(|e| PluginError::Io(e.to_string()))?;
    // 测试形态 = 目录注入（生产 `new` 指向 exe 旁真实 plugins_wasm/，
    // 并行用例共享会互相串扰）。
    let mgr = Arc::new(PluginManager::new_with_dirs(
        ws.path(),
        &ws.path().join("plugins_wasm"),
        PluginLimits::default(),
        secrets,
    )?);
    Ok((ws, mgr))
}

async fn fresh_manager() -> Result<(tempfile::TempDir, Arc<PluginManager>), PluginError> {
    fresh_manager_secrets(Arc::new(NoSecrets)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_install_roundtrip_and_host_calls() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), TRANSLATE_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    let outcome = installer.install(&src, true).await.expect("install");
    assert_eq!(outcome.trust, PluginTrustState::ReviewRequired);
    assert!(
        outcome.registered.is_none(),
        "新装默认不加载（2026-10-22 语义）"
    );

    // 用户开开关 → 装配注册。
    let reg = installer
        .set_enabled("translate", true)
        .await
        .expect("enable")
        .expect("loaded");

    // 注册面：工具全名与元数据对账。
    let found = mgr
        .find_by_tool_name("plugin.translate.translate")
        .expect("tool registered");
    let meta = found.tool_meta.as_ref().expect("tool meta");
    assert_eq!(meta.name, "plugin.translate.translate");
    assert_eq!(meta.operation_type, "read");
    assert_eq!(meta.min_tier, "big");

    // 基线往返。
    let out = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"echo","text":"hello wasm"}"#.into(),
            ctx(),
        )
        .await
        .expect("echo");
    assert!(!out.is_error);
    assert_eq!(out.content, "hello wasm");

    // 实例配置 → config-get 通道。
    mgr.set_config_key("translate", "greeting", "hey")
        .expect("set config");
    let out = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"host_calls"}"#.into(),
            ctx(),
        )
        .await
        .expect("host_calls");
    assert!(out.content.contains("greeting=hey"), "got: {}", out.content);
    assert!(
        out.content.contains("data-write=ok"),
        "got: {}",
        out.content
    );
    assert!(
        out.content.contains("tool-invoke=unavailable"),
        "got: {}",
        out.content
    );
    assert!(out.content.contains("now="), "got: {}", out.content);

    // 数据目录落盘（/data preopen → plugin-data/translate）。
    let spill = mgr.plugin_data_dir("translate").join("spill.txt");
    assert!(spill.exists(), "spill missing at {}", spill.display());

    // 宿主日志环形缓冲（guest 三条：begin/warn/end）。
    let (lines, _dropped) = reg.logs.snapshot();
    assert!(lines.len() >= 3, "log lines: {}", lines.len());
    assert!(lines.iter().any(|l| l.message.contains("host_calls begin")));
}

#[tokio::test(flavor = "multi_thread")]
async fn egress_denied_by_default() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), TRANSLATE_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");
    installer
        .set_enabled("translate", true)
        .await
        .expect("enable");

    let out = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"egress_probe"}"#.into(),
            ctx(),
        )
        .await
        .expect("egress_probe executes");
    assert!(out.is_error);
    assert!(
        out.content.contains("NoPermission"),
        "deny-by-default expected, got: {}",
        out.content
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn takeover_declaration_survives_install_funnel() {
    // 工具接管层 e2e：正例 = 带接管声明的插件走完装配漏斗，
    // 声明原样落盘、装配注册后 owner 裁决命中（桥 wrap/翻开关位由
    // nemesisbot plugin_bridge tests 用桩内置工具钉过——这里钉宿主面全链
    // 漏斗不吞声明、注册面消费声明）。反例 = deny-list 命中（exec）在
    // 漏斗第①步（manifest 解析）诚实拒绝——安装即拦截，不等到注册面。
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), TRANSLATE_TAKEOVER_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");
    installer
        .set_enabled("translate", true)
        .await
        .expect("enable")
        .expect("loaded");
    let owner = mgr.takeover_owner("web_fetch").expect("takeover owner");
    assert_eq!(owner.manifest.slug, "translate");
    assert_eq!(owner.manifest.takeover, vec!["web_fetch".to_string()]);

    let deny_toml =
        TRANSLATE_TAKEOVER_TOML.replacen("takeover = [\"web_fetch\"]", "takeover = [\"exec\"]", 1);
    let (_ws2, mgr2) = fresh_manager().await.expect("manager2");
    let tmp2 = tempfile::tempdir().expect("stage tmp2");
    let src2 = stage_plugin(tmp2.path(), deny_toml, &wasm).expect("stage2");
    let installer2 = PluginInstaller::new(mgr2.clone(), Arc::new(AutoApprove), None);
    // InstallOutcome 无 Debug——expect_err 用不了，match 拿错误面。
    let err = match installer2.install(&src2, true).await {
        Ok(_) => panic!("deny-list takeover must be rejected at install"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("takeover"), "拒绝带原因: {err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn fuel_bomb_times_out() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), TRANSLATE_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");
    installer
        .set_enabled("translate", true)
        .await
        .expect("enable");

    let started = Instant::now();
    let err = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"fuel_bomb"}"#.into(),
            ctx(),
        )
        .await
        .expect_err("fuel bomb must fail");
    assert!(
        matches!(err, PluginError::Timeout { .. }),
        "expected Timeout, got: {err:?}"
    );
    // fuel=20M 应远快于 timeout-ms=10s 的墙钟闸。
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test(flavor = "multi_thread")]
async fn trap_classified() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), TRANSLATE_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");
    installer
        .set_enabled("translate", true)
        .await
        .expect("enable");

    let err = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"trap"}"#.into(),
            ctx(),
        )
        .await
        .expect_err("trap must fail");
    assert!(
        matches!(err, PluginError::Trap(_)),
        "expected Trap, got: {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn disable_blocks_then_reenable_restores() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), TRANSLATE_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");

    // 新语义：装完默认不加载——未开开关时调用必须诚实拒绝。
    let err = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"echo","text":"x"}"#.into(),
            ctx(),
        )
        .await
        .expect_err("not-yet-enabled must fail");
    assert!(matches!(err, PluginError::NotAvailable(_)), "got: {err:?}");

    // 关开关（幂等摘除）也不装载。
    installer
        .set_enabled("translate", false)
        .await
        .expect("disable");
    let err = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"echo","text":"x"}"#.into(),
            ctx(),
        )
        .await
        .expect_err("disabled must fail");
    assert!(matches!(err, PluginError::NotAvailable(_)), "got: {err:?}");

    // 开开关 → 真装配 → 可调用。
    installer
        .set_enabled("translate", true)
        .await
        .expect("enable");
    mgr.execute_tool(
        "plugin.translate.translate",
        r#"{"mode":"echo","text":"x"}"#.into(),
        ctx(),
    )
    .await
    .expect("re-enabled works");
}

#[tokio::test(flavor = "multi_thread")]
async fn observer_pipeline_receives_events() {
    let Some(wasm) = fixture_wasm("activity-log", "activity_log_plugin") else {
        eprintln!("{}", skip_reason("activity-log"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), ACTIVITY_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");
    let reg = installer
        .set_enabled("activity-log", true)
        .await
        .expect("enable")
        .expect("loaded");

    mgr.enqueue_observer_event(r#"{"type":"tool_start","tool":"exec"}"#.into());
    mgr.enqueue_observer_event(r#"{"type":"tool_end","tool":"exec"}"#.into());

    let events_log = mgr.plugin_data_dir("activity-log").join("events.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_body = String::new();
    loop {
        if events_log.exists() {
            let body = std::fs::read_to_string(&events_log).unwrap_or_default();
            last_body = body.clone();
            if body.lines().count() >= 2 {
                assert!(body.contains("tool_start"));
                assert!(body.contains("tool_end"));
                break;
            }
        }
        if Instant::now() >= deadline {
            let (lines, dropped_ring) = reg.logs.snapshot();
            panic!(
                "observer did not write events.log in time; file_exists={} body={last_body:?} \
dropped_events={} dropped_ring={dropped_ring} host_logs={lines:?}",
                events_log.exists(),
                reg.dropped_events
                    .load(std::sync::atomic::Ordering::Relaxed)
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (lines, _dropped) = reg.logs.snapshot();
    assert!(
        lines.iter().any(|l| l.message.contains("event received")),
        "observer host log missing: {:?}",
        lines
    );
}

/// 审计链接线：secret-get / workspace-read / http-send 的 allowed 与
/// denied 都落安全审计文件；凭据原文永不入审计。
#[tokio::test(flavor = "multi_thread")]
async fn audit_events_recorded() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate"));
        return;
    };
    let (ws, mgr) = fresh_manager_secrets(Arc::new(OneSecret))
        .await
        .expect("manager");
    let audit_dir = tempfile::tempdir().expect("audit tmp");
    let logger = nemesis_security::audit_log::AuditLogger::new(
        nemesis_security::audit_log::AuditLogConfig {
            audit_log_dir: audit_dir.path().to_path_buf(),
            enabled: true,
        },
    )
    .expect("audit logger");
    mgr.set_audit(Arc::new(std::sync::Mutex::new(logger)));

    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), TRANSLATE_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");
    installer
        .set_enabled("translate", true)
        .await
        .expect("enable")
        .expect("loaded");

    // ① secret allowed（声明内 + vault 命中）
    let out = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"secret_probe","name":"api_key"}"#.into(),
            ctx(),
        )
        .await
        .expect("secret_probe");
    assert!(
        out.content.contains("secret=api_key:resolved(len="),
        "got: {}",
        out.content
    );

    // ② secret denied（未在 manifest x-secret 声明）
    let out = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"secret_probe","name":"evil_key"}"#.into(),
            ctx(),
        )
        .await
        .expect("secret_probe undeclared");
    assert!(
        out.content.contains("secret=evil_key:denied("),
        "got: {}",
        out.content
    );

    // ③ workspace-read allowed（工作区内文件）
    std::fs::write(ws.path().join("hello.txt"), b"audit-ok").expect("ws file");
    let out = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"ws_read","path":"hello.txt"}"#.into(),
            ctx(),
        )
        .await
        .expect("ws_read");
    assert!(
        out.content.contains("ws-read=hello.txt:audit-ok"),
        "got: {}",
        out.content
    );

    // ④ workspace-read denied（越界路径）
    let out = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"ws_read","path":"../escape.txt"}"#.into(),
            ctx(),
        )
        .await
        .expect("ws_read escape");
    assert!(out.content.contains(":denied("), "got: {}", out.content);

    // ⑤ http-send denied（无 allowlist）
    let out = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"egress_probe"}"#.into(),
            ctx(),
        )
        .await
        .expect("egress_probe");
    assert!(out.content.contains("NoPermission"), "got: {}", out.content);

    // 审计文件对账：三类事件齐 + allowed/denied 两态齐 + 原文不泄漏。
    let body = audit_body(audit_dir.path());
    assert!(body.contains("plugin_secret_get"), "audit body: {body}");
    assert!(body.contains("plugin_workspace_read"), "audit body: {body}");
    assert!(body.contains("plugin_http_send"), "audit body: {body}");
    assert!(body.contains("| allowed |"), "audit body: {body}");
    assert!(body.contains("| denied |"), "audit body: {body}");
    assert!(
        !body.contains("s3cr3t-value"),
        "secret value must not leak into audit log: {body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn uninstall_removes_payload_keeps_data() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), TRANSLATE_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");

    // 制造数据目录内容（卸载应保留）。
    let data_dir = mgr.plugin_data_dir("translate");
    std::fs::write(data_dir.join("precious.txt"), b"keep").expect("data");

    assert!(
        nemesis_plugins_wasm::install::uninstall(&mgr, "translate")
            .await
            .expect("uninstall"),
        "uninstall reports presence"
    );
    assert!(mgr.get("translate").is_none(), "registry cleared");
    assert!(
        !mgr.plugin_dir("translate").exists(),
        "payload dir must be gone"
    );
    assert!(
        data_dir.join("precious.txt").exists(),
        "data dir must survive uninstall"
    );
    let err = mgr
        .execute_tool(
            "plugin.translate.translate",
            r#"{"mode":"echo","text":"x"}"#.into(),
            ctx(),
        )
        .await
        .expect_err("uninstalled must fail");
    assert!(matches!(err, PluginError::NotAvailable(_)));
}

// ---------------------------------------------------------------------------
// 开关补漏斗收口：手工拷入无签名载荷 + 卡面救援全链
//（set_enabled 补漏斗；从紧面（无卡 CLI）归 install_registry::test_ unsigned_without_card_stays_strict）
// ---------------------------------------------------------------------------

/// 审批卡替身（can_approve_unsigned=true，裁决可编程）——gateway
/// LateInstallApprover 的测试投影：具备人批卡能力、结果由用例钉死。
struct CardApprover {
    approve: bool,
}

impl InstallApprover for CardApprover {
    fn can_approve_unsigned(&self) -> bool {
        true
    }

    fn approve<'a>(
        &'a self,
        _review: &'a InstallReview,
    ) -> futures::future::BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            if self.approve {
                Ok(true)
            } else {
                Err("测试拒绝".into())
            }
        })
    }
}

/// 写手工拷入形态载荷（直落 plugins_wasm/<slug>/，无 lockfile 记录、无签名）。
fn stage_hand_copied(mgr: &PluginManager, slug: &str, wasm_src: &Path) -> std::path::PathBuf {
    let wasm = std::fs::read(wasm_src).expect("read fixture wasm");
    let sha = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(&wasm))
    };
    let pdir = mgr.plugin_dir(slug);
    std::fs::create_dir_all(&pdir).expect("payload dir");
    // sha 放文件头（TOML 根键必须在任何 table 段之前）；wasm 缺省名 plugin.wasm。
    let manifest = format!("wasm-sha256 = \"{sha}\"\n{TRANSLATE_TOML}");
    std::fs::write(pdir.join("plugin.toml"), manifest).expect("write manifest");
    std::fs::write(pdir.join("plugin.wasm"), wasm).expect("write wasm");
    pdir
}

/// 卡面批准：手拷无签名载荷开关 → 补漏斗放行（trust=review-required 进卡）
/// → 批准 = 装载注册 + lockfile 留档（重启装载沿用同一容差）。
#[tokio::test(flavor = "multi_thread")]
async fn hand_copied_unsigned_rescued_by_card_approval() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    stage_hand_copied(&mgr, "translate", &wasm);

    let installer =
        PluginInstaller::new(mgr.clone(), Arc::new(CardApprover { approve: true }), None);
    // Ok 侧类型无 Debug——match 拆。
    match installer.set_enabled("translate", true).await {
        Ok(reg) => assert!(reg.is_some(), "批准后应装载注册"),
        Err(e) => panic!("card rescue failed: {e:?}"),
    }
    assert!(mgr.get("translate").is_some(), "注册表在场");

    // lockfile 留档 = review-required（人批卡与 --allow-unsigned 同一留档语义，
    // recorded_allow_unsigned 由此放行重启装载）。
    let lf_raw =
        std::fs::read_to_string(mgr.plugins_dir().join("lockfile.json")).expect("lockfile written");
    assert!(
        lf_raw.contains("review-required"),
        "lockfile 应记 review-required: {lf_raw}"
    );
}

/// 卡面拒绝：漏斗走到审批步被否 → 带注记失败、注册表不收、开关保持 true
///（诚实重试态——用户换载荷后再开）。
#[tokio::test(flavor = "multi_thread")]
async fn hand_copied_unsigned_denied_by_card_stays_out() {
    let Some(wasm) = fixture_wasm("translate", "translate_plugin") else {
        eprintln!("{}", skip_reason("translate"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    stage_hand_copied(&mgr, "translate", &wasm);

    let installer =
        PluginInstaller::new(mgr.clone(), Arc::new(CardApprover { approve: false }), None);
    let err = match installer.set_enabled("translate", true).await {
        Ok(_) => panic!("denied install must fail"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("测试拒绝"),
        "拒绝注记透传: {err:?}"
    );
    assert!(mgr.get("translate").is_none(), "被拒插件不进注册表");
    assert!(
        mgr.get_config("translate").enabled,
        "开关先落盘（失败保持 true = 重试态）"
    );
}

// ---------------------------------------------------------------------------
// 面板扩展批：panel-echo 夹具全链（install → enable → invoke_ui
// 五行为面：echo 往返 / rows 投影 / fail 业务失败 / unknown 回执 / probe
// 能力探针 + guest 日志 + 审计链）
// ---------------------------------------------------------------------------

const PANEL_ECHO_TOML: &str = r#"api-version = 1
slug = "panel-echo"
kind = "panel"
name = "Panel Echo"
version = "0.1.0"
description = "e2e fixture panel"

[limits]
fuel = 20_000_000
timeout-ms = 10_000

[[ui.page]]
route = "echo"
title = "回显面板"

[[ui.page.views]]
kind = "table"
bind = "cmd://rows"

[[ui.page.views.columns]]
key = "id"
label = "编号"

[[ui.page.views]]
kind = "actions"
bind = ""

[[ui.page.views.buttons]]
label = "回显"
command = "echo"
"#;

#[tokio::test(flavor = "multi_thread")]
async fn panel_install_roundtrip_and_ui_invoke() {
    let Some(wasm) = fixture_wasm("panel-echo", "panel_echo_plugin") else {
        eprintln!("{}", skip_reason("panel-echo"));
        return;
    };
    let (ws, mgr) = fresh_manager().await.expect("manager");
    let audit_dir = tempfile::tempdir().expect("audit tmp");
    let logger = nemesis_security::audit_log::AuditLogger::new(
        nemesis_security::audit_log::AuditLogConfig {
            audit_log_dir: audit_dir.path().to_path_buf(),
            enabled: true,
        },
    )
    .expect("audit logger");
    mgr.set_audit(Arc::new(std::sync::Mutex::new(logger)));

    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), PANEL_ECHO_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    let outcome = installer.install(&src, true).await.expect("install");
    assert!(
        outcome.registered.is_none(),
        "新装默认不加载（2026-10-22 语义）"
    );
    let reg = installer
        .set_enabled("panel-echo", true)
        .await
        .expect("enable")
        .expect("loaded");

    // ① 基线往返：echo 带中文（CABI 串跨界）。
    let reply = mgr
        .invoke_ui(
            "panel-echo",
            r#"{"command":"echo","args":{"text":"面板你好"}}"#.into(),
        )
        .await
        .expect("echo");
    assert!(reply.contains("\"ok\":true"), "reply: {reply}");
    assert!(reply.contains("面板你好"), "reply: {reply}");

    // ② table 数据投影（声明面 cmd://rows 绑定的guest 回执形态）。
    let reply = mgr
        .invoke_ui("panel-echo", r#"{"command":"rows"}"#.into())
        .await
        .expect("rows");
    assert!(reply.contains("NB-1"), "reply: {reply}");
    assert!(reply.contains("样例行"), "reply: {reply}");

    // ③ 业务失败形态（{"error":...} 约定回执，非 trap——宿主照常返回）。
    let reply = mgr
        .invoke_ui("panel-echo", r#"{"command":"fail"}"#.into())
        .await
        .expect("fail passthrough");
    assert!(reply.contains("演示性业务失败"), "reply: {reply}");

    // ④ 未知命令 → guest 约定错误回执。
    let reply = mgr
        .invoke_ui("panel-echo", r#"{"command":"nope"}"#.into())
        .await
        .expect("unknown cmd");
    assert!(reply.contains("unknown command: nope"), "reply: {reply}");

    // ⑤ 能力面探针：log + now + tool-invoke（独立运行时无接线 = unavailable）。
    let reply = mgr
        .invoke_ui("panel-echo", r#"{"command":"probe"}"#.into())
        .await
        .expect("probe");
    assert!(reply.contains("tool-invoke=unavailable"), "reply: {reply}");
    assert!(reply.contains("\"now\":"), "reply: {reply}");
    let (lines, _dropped) = reg.logs.snapshot();
    assert!(
        lines
            .iter()
            .any(|l| l.message.contains("panel probe begin")),
        "log lines: {lines:?}"
    );

    // 审计链：每次 invoke 落 plugin_ui_invoke allow 事件。
    let body = audit_body(audit_dir.path());
    assert!(body.contains("plugin_ui_invoke"), "audit body: {body}");
    // 开关关 → 真摘除（纪律：disable = unregister，无「注册但拒
    // 调用」半态）→ invoke 诚实报 not loaded（动作面随开关冻结）。
    installer
        .set_enabled("panel-echo", false)
        .await
        .expect("disable");
    let err = mgr
        .invoke_ui("panel-echo", r#"{"command":"echo"}"#.into())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not loaded"), "{err}");
    let _ = ws; // tempdir 生命周期保持到断言结束
}

// ---------------------------------------------------------------------------
// 决策 C：面板 llm-complete 双闸（manifest `x-llm` 声明 ×
// `panel_llm.enabled` 总闸，缺省 false）。授予裁决在 registry::build_frame
// （私有，无公开帧访问器——crate 纪律 src 零内联测试），可观察面 = guest
// 夹具 `llm_probe` 命令消费帧内 llm 是否在场。宿主 invoker 用测试桩。
// ---------------------------------------------------------------------------

/// LLM 双闸变体：同面板 + `[permissions] x-llm = true`（单源派生防漂移）。
fn panel_echo_llm_toml() -> String {
    PANEL_ECHO_TOML.replace(
        "description = \"e2e fixture panel\"\n",
        "description = \"e2e fixture panel\"\n\n[permissions]\nx-llm = true\n",
    )
}

/// 测试桩 invoker：恒回固定标记文本。
struct StubLlm;

impl LlmInvoker for StubLlm {
    fn complete(
        &self,
        _prompt: &str,
        _max_tokens: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
    {
        Box::pin(async { Ok("stub-llm-ok".to_string()) })
    }
}

/// 装好 panel-echo（给定 manifest）并接好 llm 桩，返回 (workspace, manager)。
async fn staged_llm_panel(
    toml_body: String,
) -> Result<(tempfile::TempDir, Arc<PluginManager>, PluginInstaller), Box<dyn std::error::Error>> {
    let Some(wasm) = fixture_wasm("panel-echo", "panel_echo_plugin") else {
        return Err("fixture missing".into());
    };
    let (ws, mgr) = fresh_manager().await.expect("manager");
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), toml_body, &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");
    installer
        .set_enabled("panel-echo", true)
        .await
        .expect("enable")
        .expect("loaded");
    mgr.set_llm(Arc::new(StubLlm));
    Ok((ws, mgr, installer))
}

#[tokio::test(flavor = "multi_thread")]
async fn panel_llm_dual_gate_hot() {
    let Ok((ws, mgr, _installer)) = staged_llm_panel(panel_echo_llm_toml()).await else {
        eprintln!("{}", skip_reason("panel-echo"));
        return;
    };
    let config_dir = mgr.workspace_root().join("config");
    let probe = |mgr: &Arc<PluginManager>| {
        let mgr = mgr.clone();
        async move {
            mgr.invoke_ui("panel-echo", r#"{"command":"llm_probe"}"#.into())
                .await
                .expect("llm_probe")
        }
    };

    // ① 总闸缺省 false（文件缺席）→ 帧内 llm 缺席 → guest 诚实报 unavailable。
    let reply = probe(&mgr).await;
    assert!(reply.contains("\"llm\":\"unavailable\""), "reply: {reply}");

    // ② 开闸（写真相源）→ 下一次调用即生效（fresh-store-per-call 热语义）。
    nemesis_config::plugin_wasm::set_panel_llm(&config_dir, true).expect("set on");
    let reply = probe(&mgr).await;
    assert!(
        reply.contains("\"llm\":\"ok:stub-llm-ok\""),
        "双闸齐 → invoker 文本透传: {reply}"
    );

    // ③ 关闸 → 立刻回到 unavailable（无缓存，关也是热的）。
    nemesis_config::plugin_wasm::set_panel_llm(&config_dir, false).expect("set off");
    let reply = probe(&mgr).await;
    assert!(reply.contains("\"llm\":\"unavailable\""), "reply: {reply}");
    let _ = ws;
}

#[tokio::test(flavor = "multi_thread")]
async fn panel_llm_missing_declaration_stays_denied() {
    // 总闸开 + 无 `x-llm` 声明 → 仍然 denied（双闸是 AND，声明位不可省）。
    let Ok((_ws, mgr, _installer)) = staged_llm_panel(PANEL_ECHO_TOML.to_string()).await else {
        eprintln!("{}", skip_reason("panel-echo"));
        return;
    };
    let config_dir = mgr.workspace_root().join("config");
    nemesis_config::plugin_wasm::set_panel_llm(&config_dir, true).expect("set on");
    let reply = mgr
        .invoke_ui("panel-echo", r#"{"command":"llm_probe"}"#.into())
        .await
        .expect("llm_probe");
    assert!(
        reply.contains("\"llm\":\"unavailable\""),
        "未声明 x-llm 的面板即使总闸开也不得拿到 llm 面: {reply}"
    );
}

// ---------------------------------------------------------------------------
// guest step 批：stepdemo 夹具（kind=flow-step）全链
// install → enable → run_flow_step 八行为面：echo done（中文 CABI）/
// tally running×2→done（guest 数据目录记账幂等）/ chaos failed / chaos
// skipped 形态透传 / 越权 op 对账拒绝 / unknown slug / host-error → Trap /
// disable 真摘除 + 审计链。
// ---------------------------------------------------------------------------

const STEPDEMO_TOML: &str = r#"api-version = 1
slug = "stepdemo"
kind = "flow-step"
name = "StepDemo"
version = "0.1.0"
description = "e2e fixture flow step"

[limits]
fuel = 20_000_000
timeout-ms = 10_000
"#;

#[tokio::test(flavor = "multi_thread")]
async fn flow_step_install_roundtrip_and_run() {
    let Some(wasm) = fixture_wasm("stepdemo", "stepdemo_plugin") else {
        eprintln!("{}", skip_reason("stepdemo"));
        return;
    };
    let (ws, mgr) = fresh_manager().await.expect("manager");
    let audit_dir = tempfile::tempdir().expect("audit tmp");
    let logger = nemesis_security::audit_log::AuditLogger::new(
        nemesis_security::audit_log::AuditLogConfig {
            audit_log_dir: audit_dir.path().to_path_buf(),
            enabled: true,
        },
    )
    .expect("audit logger");
    mgr.set_audit(Arc::new(std::sync::Mutex::new(logger)));

    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), STEPDEMO_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    let outcome = installer.install(&src, true).await.expect("install");
    assert!(
        outcome.registered.is_none(),
        "新装默认不加载（2026-10-22 语义）"
    );
    let reg = installer
        .set_enabled("stepdemo", true)
        .await
        .expect("enable")
        .expect("loaded");

    // 安装期对账快照：kinds 词表自报（echo/tally/chaos）。
    let kinds = reg.step_meta.as_ref().expect("step meta").kinds.clone();
    assert_eq!(kinds, vec!["echo", "tally", "chaos"], "kinds: {kinds:?}");

    // ① echo：同步 done，args 回显带中文（CABI 串跨界）。
    let out = mgr
        .run_flow_step(
            "stepdemo",
            "echo",
            r#"{"op":"echo","args":{"text":"流程你好"},"session_key":"flow:t/i"}"#.into(),
        )
        .await
        .expect("echo");
    assert!(out.contains("流程你好"), "out: {out}");
    assert!(out.contains("\"status\":\"done\""), "out: {out}");

    // ② tally：前两次 running（guest 数据目录记账幂等），第三次 done。
    let tally_input = r#"{"op":"tally","instance":{"instance":"e2e-i"},"session_key":"flow:t/i"}"#;
    for n in 1..3 {
        let out = mgr
            .run_flow_step("stepdemo", "tally", tally_input.into())
            .await
            .expect("tally running");
        assert!(out.contains("\"status\":\"running\""), "poll {n}: {out}");
    }
    let out = mgr
        .run_flow_step("stepdemo", "tally", tally_input.into())
        .await
        .expect("tally done");
    assert!(out.contains("\"status\":\"done\""), "out: {out}");
    assert!(out.contains("\"polls\":3"), "out: {out}");

    // ③ failed：业务失败形态透传（四态解析归 flow 引擎，宿主只投递）。
    let out = mgr
        .run_flow_step(
            "stepdemo",
            "chaos",
            r#"{"op":"chaos","args":{"mode":"failed"},"session_key":"flow:t/i"}"#.into(),
        )
        .await
        .expect("failed passthrough");
    assert!(out.contains("注定失败"), "out: {out}");

    // ④ skipped 形态透传；malformed 字符串出参同样原样（畸形判定归 flow 引擎）。
    let out = mgr
        .run_flow_step(
            "stepdemo",
            "chaos",
            r#"{"op":"chaos","args":{"mode":"skipped"},"session_key":"flow:t/i"}"#.into(),
        )
        .await
        .expect("skipped passthrough");
    assert!(out.contains("\"status\":\"skipped\""), "out: {out}");
    let out = mgr
        .run_flow_step(
            "stepdemo",
            "chaos",
            r#"{"op":"chaos","args":{"mode":"malformed"},"session_key":"flow:t/i"}"#.into(),
        )
        .await
        .expect("malformed passthrough");
    assert_eq!(out, "not-json-at-all");

    // ⑤ 越权 op：词表外 = 安装期对账拒绝（MetadataMismatch）。
    let err = mgr
        .run_flow_step(
            "stepdemo",
            "not_in_kinds",
            r#"{"op":"not_in_kinds"}"#.into(),
        )
        .await
        .expect_err("unauthorized op");
    assert!(
        matches!(err, PluginError::MetadataMismatch(ref m) if m.contains("not in plugin 'stepdemo'")),
        "err: {err:?}"
    );

    // ⑥ 未知 slug：未装载 = NotAvailable。
    let err = mgr
        .run_flow_step("ghost", "echo", r#"{"op":"echo"}"#.into())
        .await
        .expect_err("unknown slug");
    assert!(
        matches!(err, PluginError::NotAvailable(ref m) if m.contains("not loaded: ghost")),
        "err: {err:?}"
    );

    // ⑦ host-error：guest Err 经 wire err 载荷 = Trap（策略点同款 decode
    // 前缀惯例 + host-error Debug 形态携带 guest 报错原文）。
    let err = mgr
        .run_flow_step(
            "stepdemo",
            "chaos",
            r#"{"op":"chaos","args":{"mode":"err"},"session_key":"flow:t/i"}"#.into(),
        )
        .await
        .expect_err("host error");
    assert!(
        matches!(err, PluginError::Trap(ref m) if m.contains("unavailable") && m.contains("guest 侧异常（mode=err）")),
        "err: {err:?}"
    );

    // 审计链：每次投递落 plugin_flow_step allow 事件。
    let body = audit_body(audit_dir.path());
    assert!(body.contains("plugin_flow_step"), "audit body: {body}");

    // ⑧ disable = 真摘除（纪律）→ 投递诚实报 not loaded。
    installer
        .set_enabled("stepdemo", false)
        .await
        .expect("disable");
    let err = mgr
        .run_flow_step("stepdemo", "echo", r#"{"op":"echo"}"#.into())
        .await
        .expect_err("after disable");
    assert!(
        matches!(err, PluginError::NotAvailable(ref m) if m.contains("not loaded")),
        "err: {err:?}"
    );
    let _ = ws;
}

// ---------------------------------------------------------------------------
// 组合 world 批：panel-step-demo 夹具（kind=panel 双导出
// flow-step，world=plugin-panel-flow-step）全链 install → enable →
// 面板写通道正向控制（secret-set/config-set 真落 writer）→ flow-step 帧
// 写通道活体证明（同一份 guest 代码的写调用被帧纪律拦下）→ 纯面板
// 负面（无 step 快照 = 步骤投递拒）→ 审计链。
// ---------------------------------------------------------------------------

const PANEL_STEP_TOML: &str = r#"api-version = 1
slug = "panel-step-demo"
kind = "panel"
name = "PanelStepDemo"
version = "0.1.0"
description = "e2e fixture combined panel+flow-step"

[permissions]
x-secret = ["probe_token"]

[limits]
fuel = 20_000_000
timeout-ms = 10_000

[config-schema]
probe_mode = "演示配置键"
"#;

/// 记账型 SecretWriter：物理证明 FlowStep 帧的写调用永远到不了 writer——
/// 面板帧正向控制恰好 1 次，步骤探针后计数不变 = grant 级剥离实锤
/// （若拦在 cb 层但 grant 未剥，探针会多出 1 条记账即红）。
#[derive(Default)]
struct RecordingSecretWriter {
    calls: std::sync::Mutex<Vec<(String, String)>>,
}

impl SecretWriter for RecordingSecretWriter {
    fn write(&self, alias: &str, value: &str) -> Result<(), String> {
        self.calls
            .lock()
            .expect("stub lock")
            .push((alias.to_string(), value.to_string()));
        Ok(())
    }
}

/// 生产同源形态（plugin_bridge.rs::PluginManagerConfigWriter 的 Weak 委派）：
/// e2e 的 config-set 走真 `set_config_key`（声明校验 + 落盘），面板
/// get_config 读回 = 真 roundtrip 而非 stub 回声。
struct ManagerConfigWriter(std::sync::Weak<PluginManager>);

impl ConfigWriter for ManagerConfigWriter {
    fn set_config(&self, slug: &str, key: &str, value: &str) -> Result<(), String> {
        self.0
            .upgrade()
            .ok_or_else(|| "config-set 失败：插件注册表已不可达".to_string())?
            .set_config_key(slug, key, value)
            .map_err(|e| e.to_string())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn combined_panel_step_roundtrip() {
    let Some(wasm) = fixture_wasm("panel-step-demo", "panel_step_demo_plugin") else {
        eprintln!("{}", skip_reason("panel-step-demo"));
        return;
    };
    let (ws, mgr) = fresh_manager().await.expect("manager");
    let audit_dir = tempfile::tempdir().expect("audit tmp");
    let logger = nemesis_security::audit_log::AuditLogger::new(
        nemesis_security::audit_log::AuditLogConfig {
            audit_log_dir: audit_dir.path().to_path_buf(),
            enabled: true,
        },
    )
    .expect("audit logger");
    mgr.set_audit(Arc::new(std::sync::Mutex::new(logger)));

    // 写通道接线（生产同源：secret = 记账 stub / config = Weak 委派 manager）。
    let rec = Arc::new(RecordingSecretWriter::default());
    mgr.set_secret_writer(rec.clone());
    mgr.set_config_writer(Arc::new(ManagerConfigWriter(Arc::downgrade(&mgr))));

    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), PANEL_STEP_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    let outcome = installer.install(&src, true).await.expect("install");
    assert!(
        outcome.registered.is_none(),
        "新装默认不加载（2026-10-22 语义）"
    );
    let reg = installer
        .set_enabled("panel-step-demo", true)
        .await
        .expect("enable")
        .expect("loaded");

    // 组合面安装期对账：manifest 仍是 kind=panel（声明面要求），步骤面
    // 以安装期自报快照进注册表。
    assert_eq!(reg.manifest.kind.as_str(), "panel");
    let kinds = reg
        .step_meta
        .as_ref()
        .expect("combined step meta snapshot")
        .kinds
        .clone();
    assert_eq!(kinds, vec!["probe_write_denial"], "kinds: {kinds:?}");

    // ① Panel 帧基线往返。
    let reply = mgr
        .invoke_ui("panel-step-demo", r#"{"command":"ping"}"#.into())
        .await
        .expect("ping");
    assert!(reply.contains("\"pong\":true"), "reply: {reply}");

    // ② 写通道正向控制（Panel 帧 = 用户意志入口）：secret 落 writer
    //    （vault 裸别名 plugin/<slug>/<name>），config 走真 set_config_key
    //    并被 get_config 读回（同源 roundtrip）。
    let reply = mgr
        .invoke_ui(
            "panel-step-demo",
            r#"{"command":"set_secret","args":{"value":"tok-1"}}"#.into(),
        )
        .await
        .expect("set_secret");
    assert!(reply.contains("\"ok\":true"), "reply: {reply}");
    {
        let calls = rec.calls.lock().expect("stub lock");
        assert_eq!(calls.len(), 1, "secret writer calls: {calls:?}");
        assert_eq!(calls[0].0, "plugin/panel-step-demo/probe_token");
        assert_eq!(calls[0].1, "tok-1");
    }
    let reply = mgr
        .invoke_ui(
            "panel-step-demo",
            r#"{"command":"set_config","args":{"value":"demo"}}"#.into(),
        )
        .await
        .expect("set_config");
    assert!(reply.contains("\"ok\":true"), "reply: {reply}");
    let reply = mgr
        .invoke_ui("panel-step-demo", r#"{"command":"get_config"}"#.into())
        .await
        .expect("get_config");
    assert!(reply.contains("\"value\":\"demo\""), "reply: {reply}");

    // ③ FlowStep 帧写通道活体证明：同一份 guest 代码的 secret-set /
    //    config-set 走 FlowStep 帧（构造期即无写面）→ 逐路
    //    denied_unavailable；writer 物理未触达（计数不变）。
    let out = mgr
        .run_flow_step(
            "panel-step-demo",
            "probe_write_denial",
            r#"{"op":"probe_write_denial","session_key":"flow:t/i"}"#.into(),
        )
        .await
        .expect("probe step");
    assert!(out.contains("\"status\":\"done\""), "out: {out}");
    assert_eq!(
        out.matches("denied_unavailable").count(),
        2,
        "两路写都必须被帧纪律拦下: {out}"
    );
    assert_eq!(
        rec.calls.lock().expect("stub lock").len(),
        1,
        "flow-step 帧的写调用不得触达 writer（grant 级剥离实锤）"
    );

    // 审计链：面板命令、面板写（secret+config）、步骤投递全部落账。
    let body = audit_body(audit_dir.path());
    assert!(body.contains("plugin_ui_invoke"), "audit body: {body}");
    assert!(body.contains("plugin_secret_set"), "audit body: {body}");
    assert!(body.contains("plugin_config_set"), "audit body: {body}");
    assert!(body.contains("plugin_flow_step"), "audit body: {body}");
    let _ = ws;
}

#[tokio::test(flavor = "multi_thread")]
async fn pure_panel_without_step_meta_rejects_flow_step() {
    // 纯面板（panel-echo，无双导出）：run_flow_step 的 kind 闸放行
    // （Panel ∈ 放宽集），op 对账闸必须拒（无 step 快照）——组合 world
    // 的步骤面只认安装期自报，不认「碰巧是 panel kind」。
    let Some(wasm) = fixture_wasm("panel-echo", "panel_echo_plugin") else {
        eprintln!("{}", skip_reason("panel-echo"));
        return;
    };
    let (_ws, mgr) = fresh_manager().await.expect("manager");
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = stage_plugin(tmp.path(), PANEL_ECHO_TOML.to_string(), &wasm).expect("stage");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");
    installer
        .set_enabled("panel-echo", true)
        .await
        .expect("enable")
        .expect("loaded");
    let err = mgr
        .run_flow_step("panel-echo", "echo", r#"{"op":"echo"}"#.into())
        .await
        .expect_err("pure panel must not be dispatchable as flow step");
    assert!(
        matches!(err, PluginError::MetadataMismatch(ref m) if m.contains("not in plugin 'panel-echo' reported step kinds")),
        "err: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// sc-bugfix shipped 真载荷 e2e（域插件实机形态，非夹具）：
// install → enable → 六 op 词表快照 → 面板写通道（save_settings 真 config
// 落盘 + secret 落 writer + 枚举先验拒绝）→ 无配置步骤诚实失败（不猜 URL）
// → 词表外 op 安装期对账拒绝。全程零真实网络（config 缺席先于 http-send
// 失败；egress deny-by-default 兜底）。
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn shipped_sc_bugfix_roundtrip() {
    let wasm_src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wasm/sc-bugfix/plugin.wasm");
    let toml_src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wasm/sc-bugfix/plugin.toml");
    if !wasm_src.exists() {
        eprintln!("SKIP: shipped sc-bugfix plugin.wasm 缺席（先构建 guest）");
        return;
    }
    let raw = std::fs::read_to_string(&toml_src).expect("read shipped plugin.toml");

    let (ws, mgr) = fresh_manager().await.expect("manager");
    let audit_dir = tempfile::tempdir().expect("audit tmp");
    let logger = nemesis_security::audit_log::AuditLogger::new(
        nemesis_security::audit_log::AuditLogConfig {
            audit_log_dir: audit_dir.path().to_path_buf(),
            enabled: true,
        },
    )
    .expect("audit logger");
    mgr.set_audit(Arc::new(std::sync::Mutex::new(logger)));
    let rec = Arc::new(RecordingSecretWriter::default());
    mgr.set_secret_writer(rec.clone());
    mgr.set_config_writer(Arc::new(ManagerConfigWriter(Arc::downgrade(&mgr))));

    // shipped 原文自带 wasm-sha256 根键——不走 stage_plugin（其会再前置一行
    // 哈希 = 重复根键 TOML parse error），手工 staging（零改写过漏斗）。
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).expect("stage dir");
    std::fs::write(src.join("plugin.toml"), &raw).expect("write plugin.toml");
    std::fs::copy(&wasm_src, src.join("plugin.wasm"))
        .expect("stage shipped plugin.wasm（两件套必须同在）");
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");
    let reg = installer
        .set_enabled("sc-bugfix", true)
        .await
        .expect("enable")
        .expect("loaded");

    // 安装期对账快照：五 op 词表自报（fetch/comment/report/create_mr/
    // mr_list）。
    let mut kinds = reg
        .step_meta
        .as_ref()
        .expect("combined step meta snapshot")
        .kinds
        .clone();
    kinds.sort();
    assert_eq!(
        kinds,
        vec!["comment", "create_mr", "fetch", "mr_list", "report"],
        "kinds: {kinds:?}"
    );

    // ① 枚举先验：code_host 值域外 = 整批拒绝（不落半截配置）。
    let reply = mgr
        .invoke_ui(
            "sc-bugfix",
            r#"{"command":"save_settings","args":{"code_host":"gitea","jira_base_url":"https://jira.example.com"}}"#.into(),
        )
        .await
        .expect("bad code_host");
    assert!(reply.contains("code_host 值无法识别"), "reply: {reply}");
    assert!(
        rec.calls.lock().expect("stub lock").is_empty(),
        "被拒批次不得触达 secret writer"
    );

    // ④a 零配置诚实失败：fetch 在 config_get 缺席处报「未设置」——检查序
    //    base_url → 认证 → http_send，网络面零触达。
    let out = mgr
        .run_flow_step(
            "sc-bugfix",
            "fetch",
            r#"{"op":"fetch","args":{"jql":"project = PROJ"},"session_key":"flow:t/i"}"#.into(),
        )
        .await
        .expect("fetch honest failure");
    assert!(out.contains("\"status\":\"failed\""), "out: {out}");
    assert!(
        out.contains("jira_base_url 未设置"),
        "诚实失败必须点名缺的配置键: {out}"
    );

    // ② 正向写通道：base_url 归一（尾斜杠剥除）+ secret 落 vault 别名。
    let reply = mgr
        .invoke_ui(
            "sc-bugfix",
            r#"{"command":"save_settings","args":{"jira_base_url":"https://jira.example.com/","gitlab_base_url":"https://gitlab.example.com","code_host":"gitlab","gitlab_owner":"zhangsan","pat":"gl-secret-1"}}"#.into(),
        )
        .await
        .expect("save_settings");
    assert!(reply.contains("\"ok\":true"), "reply: {reply}");
    assert!(
        mgr.get_config("sc-bugfix")
            .config
            .get("jira_base_url")
            .and_then(|v| v.as_str())
            == Some("https://jira.example.com"),
        "尾斜杠必须归一剥除"
    );
    assert!(
        mgr.get_config("sc-bugfix")
            .config
            .get("code_host")
            .and_then(|v| v.as_str())
            == Some("gitlab"),
        "code_host 真落盘（面板 get_config 同源读回）"
    );
    {
        let calls = rec.calls.lock().expect("stub lock");
        assert_eq!(calls.len(), 1, "calls: {calls:?}");
        assert_eq!(calls[0].0, "plugin/sc-bugfix/pat");
        assert_eq!(calls[0].1, "gl-secret-1");
    }

    // ③ 全空保存 = 探活语义：诚实无写入应答（writer 计数不变）。
    let reply = mgr
        .invoke_ui(
            "sc-bugfix",
            r#"{"command":"save_settings","args":{}}"#.into(),
        )
        .await
        .expect("empty save");
    assert!(reply.contains("未填写任何字段，未改动"), "reply: {reply}");
    assert_eq!(rec.calls.lock().expect("stub lock").len(), 1);

    // ④b 凭据缺席诚实失败：配置已存（④a 之后 save 了 jira_base_url）但
    //    jira_pat secret 未写——secret 面在 http_send 之前诚实点名（真实
    //    用户场景：填了基址忘了令牌），网络面零触达。
    let out = mgr
        .run_flow_step(
            "sc-bugfix",
            "fetch",
            r#"{"op":"fetch","args":{"jql":"project = PROJ"},"session_key":"flow:t/i"}"#.into(),
        )
        .await
        .expect("fetch credential-missing failure");
    assert!(out.contains("\"status\":\"failed\""), "out: {out}");
    assert!(
        out.contains("jira_pat"),
        "凭据缺席必须点名 secret 别名: {out}"
    );

    // ⑤ 词表外 op = 安装期对账拒绝（宿主闸，不进 guest）。
    let err = mgr
        .run_flow_step(
            "sc-bugfix",
            "not_in_kinds",
            r#"{"op":"not_in_kinds"}"#.into(),
        )
        .await
        .expect_err("unauthorized op");
    assert!(
        matches!(err, PluginError::MetadataMismatch(ref m) if m.contains("not in plugin 'sc-bugfix'")),
        "err: {err:?}"
    );

    // 审计链：面板命令、面板写（secret+config）、步骤投递全部落账。
    let body = audit_body(audit_dir.path());
    assert!(body.contains("plugin_ui_invoke"), "audit body: {body}");
    assert!(body.contains("plugin_secret_set"), "audit body: {body}");
    assert!(body.contains("plugin_config_set"), "audit body: {body}");
    assert!(body.contains("plugin_flow_step"), "audit body: {body}");
    let _ = ws;
}
