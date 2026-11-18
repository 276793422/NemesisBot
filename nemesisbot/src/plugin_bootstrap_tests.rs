//! agent 自举插件（项6）单测：源清单解析 / 配额持久化 / 搜索扫描 /
//! auto_approve 资格矩阵 / 审批面裁决回传 / 政策闸矩阵（estop、总闸、
//! 白名单、未找到、已装、配额）/ is_installed 判定。
//! 全程无网络；被测面是工具的闸与纯函数层，漏斗全程归
//! nemesis-plugins-wasm 套件与 e2e。但 manager 构造期
//! `PluginRuntime::new` 无条件开壳（`HostDll::open`），CI 无预构建壳
//! 产物、exe-相邻候选全摸空——凡构造 manager 的用例先过
//! [`ensure_host_dll`]（plugin_bridge/tests 同纪律：按需构建 +
//! `NEMESIS_WASM_HOST_DLL` 权威注入）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use futures::future::BoxFuture;

use crate::plugin_bootstrap::{
    BootstrapQuotaState, PluginSourceConfig, SelfBootstrapApprover, SelfBootstrapConfig,
    auto_approve_eligible, bump_quota, load_quota_state, load_sources_file, quota_file_path,
    save_quota_state, search_sources, sources_file_path,
};
use nemesis_agent::r#loop::Tool as _;
use nemesis_plugins_wasm::install::{AutoApprove, InstallApprover, InstallReview, PluginInstaller};

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 临时 workspace（唯一前缀防并行串目录）。
fn temp_ws(tag: &str) -> PathBuf {
    let ws = std::env::temp_dir().join(format!(
        "nb_bootstrap_{}_{tag}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::create_dir_all(ws.join("config").join("plugin_bootstrap")).expect("mkdir ws");
    ws
}

/// 宿主壳 DLL 就绪保障（plugin_bridge/tests 同纪律）：产物缺席按需构建，
/// 路径经 `NEMESIS_WASM_HOST_DLL` 注入 loader——测试二进制在
/// target/debug/deps/，exe-相邻候选摸不到壳产物。
fn ensure_host_dll() {
    static HOST_DLL: OnceLock<Option<PathBuf>> = OnceLock::new();
    let path = HOST_DLL.get_or_init(|| {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../plugins/plugin-wasm");
        let out = crate_dir
            .join("target/release")
            .join(if cfg!(target_os = "windows") {
                "plugin_wasm.dll"
            } else if cfg!(target_os = "macos") {
                "libplugin_wasm.dylib"
            } else {
                "libplugin_wasm.so"
            });
        if !out.exists() {
            let cargo = std::env::var_os("CARGO")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("cargo"));
            let mut cmd = std::process::Command::new(cargo);
            cmd.args(["build", "--release"]).current_dir(&crate_dir);
            for k in [
                "CARGO_TARGET_DIR",
                "CARGO_ENCODED_RUSTFLAGS",
                "CARGO_BUILD_TARGET",
            ] {
                cmd.env_remove(k);
            }
            match cmd.status() {
                Ok(s) if s.success() => {}
                s => {
                    eprintln!("宿主壳 DLL 构建失败：{s:?}");
                    return None;
                }
            }
        }
        out.exists().then_some(out)
    });
    let Some(p) = path else {
        panic!("宿主壳 DLL 构建失败（环境错误）：需 cargo 可用 + 壳 crate 可编译");
    };
    // SAFETY: 单写点（OnceLock 串行化）；读者 = 同进程 loader，均在本函数
    // 返回后触达。
    unsafe { std::env::set_var("NEMESIS_WASM_HOST_DLL", p) };
}

/// InstallReview 全字段字面量（资格矩阵翻转单字段用）。
fn review(egress: &[&str], x_secret: &[&str], takeover: &[&str], kind: &str) -> InstallReview {
    InstallReview {
        slug: "alpha-tool".into(),
        name: "Alpha".into(),
        version: "0.1.0".into(),
        kind: kind.into(),
        trust_state: nemesis_plugins_wasm::PluginTrustState::Trusted
            .as_str()
            .into(),
        trust_level: "verified".into(),
        signer_key: "ab12".into(),
        egress: egress.iter().map(|s| s.to_string()).collect(),
        x_secret: x_secret.iter().map(|s| s.to_string()).collect(),
        wasm_bytes: 100,
        wasm_sha256: "cd34".into(),
        ui_pages: 0,
        flow_templates: 0,
        connectors: vec![],
        limits_ignored: vec![],
        takeover: takeover.iter().map(|s| s.to_string()).collect(),
        proposer_reason: Some("需要 alpha 能力".into()),
        source_dir: "/tmp/src/alpha-tool".into(),
    }
}

/// 记录调用的测试审批面（委托路径断言用）。
struct RecordingApprover {
    called: AtomicBool,
    verdict: bool,
}

impl InstallApprover for RecordingApprover {
    fn approve<'a>(&'a self, _review: &'a InstallReview) -> BoxFuture<'a, Result<bool, String>> {
        self.called.store(true, Ordering::SeqCst);
        Box::pin(async move { Ok(self.verdict) })
    }
}

/// 源目录 fixture：alpha-tool（tool + 载荷 + egress/secret 声明）+
/// beta-tool（tool + 载荷，零声明）+ broken（坏 toml）+ not-a-plugin
///（无清单）。返回源目录路径。注：panel 纯声明式必须有 [[ui.page]]，
/// 搜索夹具统一用带载荷 tool 形态避开面板段校验。
fn stage_source_dir(ws: &Path) -> PathBuf {
    let src = ws.join("srclib");
    let wasm = b"\0asm-fake-payload";
    let sha = {
        use sha2::{Digest, Sha256};
        hex_lower(&Sha256::digest(wasm))
    };
    // alpha-tool：带 egress/secret 声明。
    let alpha = src.join("alpha-tool");
    std::fs::create_dir_all(&alpha).expect("mkdir alpha");
    std::fs::write(alpha.join("plugin.wasm"), wasm).expect("write wasm");
    std::fs::write(
        alpha.join("plugin.toml"),
        format!(
            "api-version = 1\nslug = \"alpha-tool\"\nkind = \"tool\"\n\
             name = \"Alpha Tool\"\nversion = \"0.1.0\"\n\
             description = \"does alpha things\"\nwasm = \"plugin.wasm\"\n\
             wasm-sha256 = \"{sha}\"\n\n[permissions]\n\
             egress = [\"api.example.com\"]\nx-secret = [\"api_key\"]\n"
        ),
    )
    .expect("write alpha toml");
    // beta-tool：零声明（资格判据「干净」形态）。
    let beta = src.join("beta-tool");
    std::fs::create_dir_all(&beta).expect("mkdir beta");
    std::fs::write(beta.join("plugin.wasm"), wasm).expect("write beta wasm");
    std::fs::write(
        beta.join("plugin.toml"),
        format!(
            "api-version = 1\nslug = \"beta-tool\"\nkind = \"tool\"\n\
             name = \"Beta Tool\"\nversion = \"0.2.0\"\n\
             description = \"beta helper tool\"\nwasm = \"plugin.wasm\"\n\
             wasm-sha256 = \"{sha}\"\n"
        ),
    )
    .expect("write beta toml");
    // broken：坏 toml（搜索跳过）。
    let broken = src.join("broken");
    std::fs::create_dir_all(&broken).expect("mkdir broken");
    std::fs::write(broken.join("plugin.toml"), "not = [valid\n").expect("write broken");
    // not-a-plugin：无 plugin.toml。
    std::fs::create_dir_all(src.join("not-a-plugin")).expect("mkdir nap");
    src
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 默认源清单（enabled 总闸按参数）。
fn write_sources(ws: &Path, self_enabled: bool, allowed: &[&str]) {
    let allowed_json = allowed
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(
        sources_file_path(ws),
        format!(
            r#"{{"sources":[{{"name":"local","kind":"local","path":"{}","enabled":true}}],
               "self_bootstrap":{{"enabled":{self_enabled},"allowed_sources":[{allowed_json}],
               "auto_approve":false,"max_installs_per_day":3}}}}"#,
            ws.join("srclib")
                .display()
                .to_string()
                .replace('\\', "\\\\")
        ),
    )
    .expect("write sources.json");
}

// ---------------------------------------------------------------------------
// 源清单解析
// ---------------------------------------------------------------------------

#[test]
fn sources_file_missing_is_honest_setup_error() {
    let ws = temp_ws("missing");
    let err = load_sources_file(&ws).unwrap_err();
    assert!(err.contains("不存在"), "错误应指明文件缺席：{err}");
    assert!(
        err.contains("self_bootstrap"),
        "错误应内联 JSON 形态说明：{err}"
    );
    assert!(
        !sources_file_path(&ws).exists(),
        "缺席诚实报错，不静默建模板文件"
    );
}

#[test]
fn sources_file_defaults_parse() {
    let ws = temp_ws("defaults");
    std::fs::write(
        sources_file_path(&ws),
        r#"{"sources":[{"name":"local","path":"C:/x"}]}"#,
    )
    .expect("write");
    let f = load_sources_file(&ws).expect("parse");
    assert_eq!(f.sources.len(), 1);
    assert_eq!(f.sources[0].kind, "local", "kind 缺省 local");
    assert!(f.sources[0].enabled, "enabled 缺省 true");
    assert!(!f.self_bootstrap.enabled, "总闸缺省 false");
    assert!(f.self_bootstrap.allowed_sources.is_empty());
    assert!(!f.self_bootstrap.auto_approve);
    assert_eq!(f.self_bootstrap.max_installs_per_day, 3, "保险丝缺省 3");
}

// ---------------------------------------------------------------------------
// 配额持久化
// ---------------------------------------------------------------------------

#[test]
fn quota_persists_and_rolls_by_utc_date() {
    let ws = temp_ws("quota");
    // 缺席 = 0。
    assert_eq!(load_quota_state(&ws).installs_today, 0);
    // 写入 + 回读。
    save_quota_state(
        &ws,
        &BootstrapQuotaState {
            date: chrono::Utc::now().date_naive().to_string(),
            installs_today: 2,
        },
    )
    .expect("save");
    assert_eq!(load_quota_state(&ws).installs_today, 2);
    // bump +1。
    bump_quota(&ws);
    assert_eq!(load_quota_state(&ws).installs_today, 3);
    // 损坏 = 从紧归零。
    std::fs::write(quota_file_path(&ws), "not json").expect("write junk");
    assert_eq!(load_quota_state(&ws).installs_today, 0);
    // 过期日期（昨日）= 归零。
    let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
        .date_naive()
        .to_string();
    save_quota_state(
        &ws,
        &BootstrapQuotaState {
            date: yesterday,
            installs_today: 99,
        },
    )
    .expect("save stale");
    assert_eq!(load_quota_state(&ws).installs_today, 0, "UTC 日界翻新");
}

// ---------------------------------------------------------------------------
// 搜索
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_scans_sorted_skips_broken_and_reports_errors() {
    let ws = temp_ws("search");
    let src = stage_source_dir(&ws);
    let sources = vec![PluginSourceConfig {
        name: "local".into(),
        kind: "local".into(),
        path: src.display().to_string(),
        enabled: true,
    }];
    // 空 query = 列出全部有效插件（排序、破损/非插件跳过）。
    let out = search_sources(&sources, "", 10).await;
    assert_eq!(
        out.results
            .iter()
            .map(|r| r.slug.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha-tool", "beta-tool"],
        "破损 manifest 与非插件目录不进结果，slug 排序"
    );
    let alpha = &out.results[0];
    assert_eq!(alpha.kind, "tool");
    assert_eq!(alpha.egress_domains, 1);
    assert_eq!(alpha.secrets, 1);
    assert!(alpha.takeover.is_empty());
    assert_eq!(alpha.source, "local");
    assert!(alpha.path.ends_with("alpha-tool"));
    let beta = &out.results[1];
    assert_eq!(beta.egress_domains, 0, "零声明行如实展示");
    assert_eq!(beta.secrets, 0);
    // 子串匹配（name 命中）。
    let out = search_sources(&sources, "ALPHA", 10).await;
    assert_eq!(out.results.len(), 1, "大小写不敏感 slug/name 匹配");
    // limit 截断。
    let out = search_sources(&sources, "", 1).await;
    assert_eq!(out.results.len(), 1);
    // 源目录不可读 = 错误注记不 panic。
    let dead = vec![PluginSourceConfig {
        name: "gone".into(),
        kind: "local".into(),
        path: ws.join("nope").display().to_string(),
        enabled: true,
    }];
    let out = search_sources(&dead, "", 10).await;
    assert!(out.results.is_empty());
    assert_eq!(out.source_errors.len(), 1, "破损源注记不静默");
    assert!(out.source_errors[0].contains("gone"));
    // 禁用源不扫描。
    let mixed = vec![
        PluginSourceConfig {
            name: "off".into(),
            kind: "local".into(),
            path: ws.join("nope2").display().to_string(),
            enabled: false,
        },
        sources[0].clone(),
    ];
    let out = search_sources(&mixed, "", 10).await;
    assert_eq!(out.results.len(), 2, "disabled 源不参与");
    assert!(out.source_errors.is_empty());
}

// ---------------------------------------------------------------------------
// auto_approve 资格矩阵
// ---------------------------------------------------------------------------

#[test]
fn eligibility_matrix_conservative_readonly_approximation() {
    // 基线：trusted/verified + tool + 零出站 + 零秘密 + 零接管 + 零放宽。
    assert!(auto_approve_eligible(&review(&[], &[], &[], "tool")));
    // 信任四态 / 信任级翻转。
    let mut r = review(&[], &[], &[], "tool");
    r.trust_level = "community".into();
    assert!(!auto_approve_eligible(&r), "community 信任级不入白名单");
    let mut r = review(&[], &[], &[], "tool");
    r.trust_state = nemesis_plugins_wasm::PluginTrustState::ReviewRequired
        .as_str()
        .into();
    assert!(!auto_approve_eligible(&r));
    // kind 翻转（observer/policy/panel 不自动批）。
    assert!(!auto_approve_eligible(&review(&[], &[], &[], "observer")));
    assert!(!auto_approve_eligible(&review(&[], &[], &[], "panel")));
    // 出站 / 秘密 / 接管 / limits 放宽，任一命中即出局。
    assert!(!auto_approve_eligible(&review(
        &["api.example.com"],
        &[],
        &[],
        "tool"
    )));
    assert!(!auto_approve_eligible(&review(&[], &["k"], &[], "tool")));
    assert!(!auto_approve_eligible(&review(&[], &[], &["exec"], "tool")));
    let mut r = review(&[], &[], &[], "tool");
    r.limits_ignored = vec!["fuel".into()];
    assert!(!auto_approve_eligible(&r));
}

// ---------------------------------------------------------------------------
// SelfBootstrapApprover 裁决
// ---------------------------------------------------------------------------

fn approver(
    ws: &Path,
    policy: SelfBootstrapConfig,
    inner: Arc<dyn InstallApprover>,
) -> (SelfBootstrapApprover, Arc<parking_lot::Mutex<Option<bool>>>) {
    let slot = Arc::new(parking_lot::Mutex::new(None));
    let a = SelfBootstrapApprover::new(
        inner,
        ws.to_path_buf(),
        policy,
        None,
        "local".to_string(),
        slot.clone(),
    );
    (a, slot)
}

#[tokio::test]
async fn approver_auto_path_short_circuits_with_slot_true() {
    let ws = temp_ws("auto");
    let rec = Arc::new(RecordingApprover {
        called: AtomicBool::new(false),
        verdict: false,
    });
    let policy = SelfBootstrapConfig {
        enabled: true,
        allowed_sources: vec![],
        auto_approve: true,
        max_installs_per_day: 3,
    };
    let (a, slot) = approver(&ws, policy, rec.clone());
    let ok = a
        .approve(&review(&[], &[], &[], "tool"))
        .await
        .expect("approve");
    assert!(ok, "资格命中 = 自动批准");
    assert_eq!(*slot.lock(), Some(true), "裁决槽回传 auto 事实");
    assert!(!rec.called.load(Ordering::SeqCst), "自动路径不委托人工卡");
}

#[tokio::test]
async fn approver_ineligible_delegates_to_inner() {
    let ws = temp_ws("deleg");
    let rec = Arc::new(RecordingApprover {
        called: AtomicBool::new(false),
        verdict: true,
    });
    let policy = SelfBootstrapConfig {
        enabled: true,
        allowed_sources: vec![],
        auto_approve: true,
        max_installs_per_day: 3,
    };
    let (a, slot) = approver(&ws, policy, rec.clone());
    // 带出站声明 = 资格出局 → 委托人工面。
    let ok = a
        .approve(&review(&["api.example.com"], &[], &[], "tool"))
        .await
        .expect("approve");
    assert!(ok, "内层裁决生效");
    assert!(rec.called.load(Ordering::SeqCst), "资格不足必须委托内层");
    assert_eq!(*slot.lock(), Some(false), "委托路径 auto=false");
}

#[tokio::test]
async fn approver_quota_exhausted_short_circuits_false() {
    let ws = temp_ws("quota_out");
    save_quota_state(
        &ws,
        &BootstrapQuotaState {
            date: chrono::Utc::now().date_naive().to_string(),
            installs_today: 3,
        },
    )
    .expect("prefill quota");
    let rec = Arc::new(RecordingApprover {
        called: AtomicBool::new(false),
        verdict: true,
    });
    let policy = SelfBootstrapConfig {
        enabled: true,
        allowed_sources: vec![],
        auto_approve: true,
        max_installs_per_day: 3,
    };
    let (a, slot) = approver(&ws, policy, rec.clone());
    let ok = a
        .approve(&review(&[], &[], &[], "tool"))
        .await
        .expect("approve");
    assert!(!ok, "配额耗尽 = 拒绝（不转卡）");
    assert!(!rec.called.load(Ordering::SeqCst), "配额耗尽连人工卡都不开");
    assert_eq!(*slot.lock(), Some(false));
}

// ---------------------------------------------------------------------------
// plugin_propose 政策闸矩阵
// ---------------------------------------------------------------------------

use crate::plugin_bootstrap::PluginProposeTool;
use nemesis_plugins_wasm::limits::PluginLimits;
use nemesis_plugins_wasm::registry::PluginManager;

struct NoSecrets;
impl nemesis_plugins_wasm::host_impl::SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

/// 最小 propose 工具（真 manager 假 plugins 目录；漏斗不走——闸全在漏斗前）。
fn propose_tool(ws: &Path) -> PluginProposeTool {
    // env 注入 + 壳解析窗口持 crate 级锁（native_plugins/plugin_bridge
    // 同锁，防 cargo test 单进程并行 set_var 互踩）。
    let manager = {
        let _dll = crate::WASM_HOST_DLL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        ensure_host_dll();
        PluginManager::new_with_dirs(
            ws,
            &ws.join("plugins"),
            PluginLimits::default(),
            Arc::new(NoSecrets),
        )
        .expect("manager")
    };
    let installer = Arc::new(PluginInstaller::new(
        Arc::new(manager),
        Arc::new(AutoApprove),
        None,
    ));
    PluginProposeTool::new(
        ws.to_path_buf(),
        installer,
        Arc::new(crate::plugin_bridge::LateInstallApprover::new(300)),
        Arc::new(nemesis_agent::estop::EstopState::new()),
        None,
    )
}

async fn propose(ws: &Path, args: serde_json::Value) -> Result<String, String> {
    propose_tool(ws)
        .execute(&args.to_string(), &test_ctx())
        .await
}

/// RequestContext 无 Default——测试空上下文手造。
fn test_ctx() -> nemesis_agent::context::RequestContext {
    nemesis_agent::context::RequestContext {
        channel: String::new(),
        chat_id: String::new(),
        user: String::new(),
        session_key: String::new(),
        correlation_id: None,
        async_callback: None,
        tool_path_base: None,
        approval_ref: None,
        secrets: Arc::new(Vec::new()),
        tool_images: Arc::new(std::sync::Mutex::new(Vec::new())),
    }
}

#[tokio::test]
async fn propose_requires_slug_and_reason() {
    let ws = temp_ws("args");
    write_sources(&ws, true, &[]);
    stage_source_dir(&ws);
    let err = propose(&ws, serde_json::json!({"slug": "", "reason": "x"}))
        .await
        .unwrap_err();
    assert!(err.contains("slug"), "空 slug 拒绝：{err}");
    let err = propose(
        &ws,
        serde_json::json!({"slug": "alpha-tool", "reason": "  "}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("reason"), "空 reason（论证）拒绝：{err}");
}

#[tokio::test]
async fn propose_estop_frozen() {
    let ws = temp_ws("estop");
    write_sources(&ws, true, &[]);
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    // 锁窗口只护 env 注入 + 壳解析（async fn——锁不得跨 await，
    // await_holding_lock）；load-once 缓存后 env 翻转无影响。
    let manager = {
        let _dll = crate::WASM_HOST_DLL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        ensure_host_dll();
        PluginManager::new_with_dirs(
            &ws,
            &ws.join("plugins"),
            PluginLimits::default(),
            Arc::new(NoSecrets),
        )
        .expect("manager")
    };
    let tool = PluginProposeTool::new(
        ws.to_path_buf(),
        Arc::new(PluginInstaller::new(
            Arc::new(manager),
            Arc::new(AutoApprove),
            None,
        )),
        Arc::new(crate::plugin_bridge::LateInstallApprover::new(300)),
        estop,
        None,
    );
    let err = tool
        .execute(
            &serde_json::json!({"slug": "alpha-tool", "reason": "需要"}).to_string(),
            &test_ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("急停"), "estop 冻结提案：{err}");
}

#[tokio::test]
async fn propose_bootstrap_disabled_honest_refusal() {
    let ws = temp_ws("disabled");
    write_sources(&ws, false, &[]);
    let err = propose(
        &ws,
        serde_json::json!({"slug": "alpha-tool", "reason": "需要"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("自举未启用"), "总闸关 = 诚实拒绝：{err}");
    assert!(err.contains("enabled=false"));
}

#[tokio::test]
async fn propose_whitelist_rejection_is_structured() {
    let ws = temp_ws("wl");
    write_sources(&ws, true, &["other-src"]);
    stage_source_dir(&ws);
    let err = propose(
        &ws,
        serde_json::json!({"slug": "alpha-tool", "reason": "需要"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("白名单"), "白名单外结构化拒绝：{err}");
    assert!(err.contains("other-src"), "拒绝原因回带允许清单：{err}");
    assert!(err.contains("local"), "拒绝原因回带实际来源：{err}");
}

#[tokio::test]
async fn propose_not_found_is_structured() {
    let ws = temp_ws("nf");
    write_sources(&ws, true, &[]);
    stage_source_dir(&ws);
    let err = propose(&ws, serde_json::json!({"slug": "ghost", "reason": "需要"}))
        .await
        .unwrap_err();
    assert!(err.contains("未找到"), "结构化未找到：{err}");
    assert!(err.contains("plugin_search"), "拒绝原因引导自纠：{err}");
}

#[tokio::test]
async fn propose_already_installed_refusal() {
    let ws = temp_ws("installed");
    write_sources(&ws, true, &[]);
    stage_source_dir(&ws);
    // lockfile 有记录（含停用形态——注册表为空）。
    let pdir = ws.join("plugins");
    std::fs::create_dir_all(&pdir).expect("mkdir plugins");
    std::fs::write(
        pdir.join("lockfile.json"),
        r#"{"version":1,"plugins":{"alpha-tool":{"version":"0.1.0","wasm_sha256":"","trust":"trusted","installed_at_ms":0,"signed_by":"","connectors":[]}}}"#,
    )
    .expect("write lockfile");
    let err = propose(
        &ws,
        serde_json::json!({"slug": "alpha-tool", "reason": "需要"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("已安装"), "已装拒绝（lockfile 命中）：{err}");
    assert!(err.contains("upgrade"), "拒绝原因引导升级路径：{err}");
}

#[tokio::test]
async fn propose_quota_fuse_fail_fast() {
    let ws = temp_ws("fuse");
    write_sources(&ws, true, &[]);
    stage_source_dir(&ws);
    save_quota_state(
        &ws,
        &BootstrapQuotaState {
            date: chrono::Utc::now().date_naive().to_string(),
            installs_today: 3,
        },
    )
    .expect("prefill");
    let err = propose(
        &ws,
        serde_json::json!({"slug": "alpha-tool", "reason": "需要"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("保险丝"), "配额耗尽 fail fast：{err}");
    assert!(err.contains("人工安装"), "拒绝原因带出口：{err}");
}
