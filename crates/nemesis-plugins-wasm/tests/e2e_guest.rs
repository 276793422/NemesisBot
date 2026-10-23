//! W1 spike e2e：真 guest 组件经宿主装配漏斗 + fresh-store 链路全跑通。
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

use nemesis_plugins_wasm::host_impl::SecretResolver;
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

/// 接管变体（2026-10-23 工具接管层）：根级 `takeover` 声明（必须在任何
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

    // 数据目录落盘（/data preopen → plugin-data/translate/）。
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
    // 工具接管层 e2e（2026-10-23）：正例 = 带接管声明的插件走完装配漏斗，
    // 声明原样落盘、装配注册后 owner 裁决命中（桥 wrap/翻开关位由
    // nemesisbot plugin_bridge tests 用桩内置工具钉过——这里钉宿主面全链
    // ：漏斗不吞声明、注册面消费声明）。反例 = deny-list 命中（exec）在
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

/// W3 审计链接线：secret-get / workspace-read / http-send 的 allowed 与
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
// 2026-10-23 开关补漏斗收口：手工拷入无签名载荷 + 卡面救援全链
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
