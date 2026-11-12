use super::*;
use crate::install::{AutoApprove, PluginInstaller};
use crate::runtime::ExecContext;
use std::sync::atomic::Ordering;

fn sha64() -> String {
    "a".repeat(64)
}

fn manifest_raw(slug: &str, kind: &str, extra: &str) -> String {
    format!(
        "api-version = 1\nslug = \"{slug}\"\nkind = \"{kind}\"\nname = \"N\"\nversion = \"0.1.0\"\nwasm-sha256 = \"{}\"\n{extra}",
        sha64()
    )
}

fn parse_manifest(slug: &str, kind: &str, extra: &str) -> PluginManifest {
    PluginManifest::parse(&manifest_raw(slug, kind, extra))
        .expect("manifest")
        .0
}

fn tool_meta(slug: &str, base: &str) -> Option<ToolMetaSnapshot> {
    Some(ToolMetaSnapshot {
        name: format!("plugin.{slug}.{base}"),
        base: base.to_string(),
        title: "T".into(),
        description: "d".into(),
        parameters_json: "{}".into(),
        operation_type: "read".into(),
        min_tier: "big".into(),
    })
}

async fn install_fixture(
    mgr: &Arc<PluginManager>,
    ws_root: &Path,
    slug_dir: &str,
    artifact: &str,
    toml: &str,
    slug: &str,
) -> bool {
    let Some(wasm) = crate::test_support::fixture_wasm(slug_dir, artifact) else {
        eprintln!("SKIP：fixture {slug_dir} 缺席");
        return false;
    };
    let bytes = std::fs::read(&wasm).expect("read fixture");
    let stage = tempfile::tempdir().expect("stage");
    let src = crate::test_support::stage_plugin(stage.path(), toml, Some(&bytes));
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer.install(&src, true).await.expect("install");
    installer
        .set_enabled(slug, true)
        .await
        .expect("enable")
        .expect("loaded");
    let _ = ws_root;
    true
}

// -- 路径解析 ------------------------------------------------------------

#[test]
fn resolve_plugins_wasm_dir_env_authority() {
    let _guard = crate::test_support::env_lock();
    let dir = tempfile::tempdir().expect("tmp");
    let target = dir.path().join("custom-plugins");
    let saved = std::env::var("NEMESIS_WASM_PLUGINS_DIR").ok();
    // SAFETY: env_lock 串行化窗口；结束恢复。
    unsafe { std::env::set_var("NEMESIS_WASM_PLUGINS_DIR", &target) };
    let resolved = resolve_plugins_wasm_dir();
    unsafe {
        match saved {
            Some(v) => std::env::set_var("NEMESIS_WASM_PLUGINS_DIR", v),
            None => std::env::remove_var("NEMESIS_WASM_PLUGINS_DIR"),
        }
    }
    assert_eq!(resolved, target, "env 权威");
    assert!(target.is_dir(), "env 路径就地创建");
}

#[test]
fn resolve_plugins_wasm_dir_blank_env_falls_through() {
    let _guard = crate::test_support::env_lock();
    let saved = std::env::var("NEMESIS_WASM_PLUGINS_DIR").ok();
    // SAFETY: env_lock 串行化窗口；结束恢复。
    unsafe { std::env::set_var("NEMESIS_WASM_PLUGINS_DIR", "  ") };
    let resolved = resolve_plugins_wasm_dir();
    unsafe {
        match saved {
            Some(v) => std::env::set_var("NEMESIS_WASM_PLUGINS_DIR", v),
            None => std::env::remove_var("NEMESIS_WASM_PLUGINS_DIR"),
        }
    }
    // 空白 env 不生效：回落候选序（首个存在者或 exe 旁创建）。
    assert!(resolved.file_name().unwrap_or_default() == "plugins_wasm");
}

// -- 旧载荷迁移（经 build_dirs 构造器触发）---------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn legacy_payload_migration_moves_tree() {
    crate::test_support::ensure_host_dll();
    let ws = tempfile::tempdir().expect("tmp");
    crate::test_support::stage_legacy_payload(
        ws.path(),
        "old-plugin",
        &manifest_raw("old-plugin", "tool", ""),
    );
    let plugins_dir = ws.path().join("plugins_wasm");
    PluginManager::new_with_dirs(
        ws.path(),
        &plugins_dir,
        PluginLimits::default(),
        Arc::new(crate::install::NoSecrets),
    )
    .expect("manager");
    // 整树并入 + lockfile 迁入 + 旧目录清空。
    assert!(plugins_dir.join("old-plugin").join("plugin.toml").is_file());
    assert!(plugins_dir.join("lockfile.json").is_file());
    assert!(!ws.path().join("plugins").exists(), "全迁走 = 旧目录删除");
    // 幂等：旧 lockfile 不在场 = 空操作（再来一次构造器不炸）。
    PluginManager::new_with_dirs(
        ws.path(),
        &plugins_dir,
        PluginLimits::default(),
        Arc::new(crate::install::NoSecrets),
    )
    .expect("manager again");
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_migration_conflict_keeps_entry_in_place() {
    crate::test_support::ensure_host_dll();
    let ws = tempfile::tempdir().expect("tmp");
    crate::test_support::stage_legacy_payload(
        ws.path(),
        "clash",
        &manifest_raw("clash", "tool", ""),
    );
    // 目标 slug 目录已在 = 该条目留守。
    let plugins_dir = ws.path().join("plugins_wasm");
    std::fs::create_dir_all(plugins_dir.join("clash")).expect("mkdir target");
    std::fs::write(plugins_dir.join("clash").join("keep.txt"), b"x").expect("write");
    PluginManager::new_with_dirs(
        ws.path(),
        &plugins_dir,
        PluginLimits::default(),
        Arc::new(crate::install::NoSecrets),
    )
    .expect("manager");
    assert!(
        ws.path()
            .join("plugins")
            .join("clash")
            .join("plugin.toml")
            .is_file(),
        "冲突条目留守旧位"
    );
    assert!(
        plugins_dir.join("clash").join("keep.txt").is_file(),
        "目标未被覆盖"
    );
}

#[test]
fn new_with_dirs_rejects_missing_workspace_root() {
    let err = PluginManager::new_with_dirs(
        Path::new("Z:/definitely/absent/workspace"),
        Path::new("Z:/definitely/absent/plugins"),
        PluginLimits::default(),
        Arc::new(crate::install::NoSecrets),
    )
    .err()
    .unwrap_or_else(|| panic!("workspace canonicalize fails"));
    assert!(err.to_string().contains("workspace root"), "{err:?}");
}

// -- 快照小面 -------------------------------------------------------------

#[test]
fn policy_meta_points_and_takes() {
    let all = PolicyMetaSnapshot {
        compact: true,
        final_judge: false,
        validation: true,
    };
    let pts = all.points();
    assert_eq!(pts.len(), 2);
    assert!(all.takes(PolicyPoint::Compact));
    assert!(!all.takes(PolicyPoint::FinalJudge));
    assert!(all.takes(PolicyPoint::Validation));
    let none = PolicyMetaSnapshot {
        compact: false,
        final_judge: false,
        validation: false,
    };
    assert!(none.points().is_empty());
    assert!(!none.takes(PolicyPoint::Compact));
}

#[tokio::test(flavor = "multi_thread")]
async fn registered_plugin_tool_name() {
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    let manifest = parse_manifest("tool-demo", "tool", "");
    mgr.register(
        manifest,
        PluginTrustState::Trusted,
        0,
        tool_meta("tool-demo", "run"),
        None,
        None,
    )
    .expect("register");
    let reg = mgr.get("tool-demo").expect("present");
    assert_eq!(reg.tool_name(), Some("plugin.tool-demo.run"));
}

// -- register 对账矩阵 ------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn register_tool_meta_mismatches() {
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    // 非 tool kind 带 tool 元数据。
    let err = mgr
        .register(
            parse_manifest("w1", "observer", ""),
            PluginTrustState::Trusted,
            0,
            tool_meta("w1", "run"),
            None,
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("kind mismatch"));
    assert!(err.to_string().contains("non-tool plugin"), "{err:?}");
    // tool kind 缺元数据。
    let err = mgr
        .register(
            parse_manifest("w2", "tool", ""),
            PluginTrustState::Trusted,
            0,
            None,
            None,
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("missing meta"));
    assert!(err.to_string().contains("missing metadata snapshot"));
    // 全名不匹配。
    let wrong = ToolMetaSnapshot {
        name: "plugin.w3.other".into(),
        base: "run".into(),
        title: "T".into(),
        description: "d".into(),
        parameters_json: "{}".into(),
        operation_type: "read".into(),
        min_tier: "big".into(),
    };
    let err = mgr
        .register(
            parse_manifest("w3", "tool", ""),
            PluginTrustState::Trusted,
            0,
            Some(wrong),
            None,
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("name mismatch"));
    assert!(err.to_string().contains("registered name"), "{err:?}");
    assert!(
        mgr.get("w1").is_none() && mgr.get("w3").is_none(),
        "拒绝项不入注册表"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn register_policy_meta_matrix() {
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    // 非 policy kind 带 policy 元数据（observer kind 过 tool 检查）。
    let err = mgr
        .register(
            parse_manifest("p1", "observer", ""),
            PluginTrustState::Trusted,
            0,
            None,
            Some(PolicyMetaSnapshot {
                compact: true,
                final_judge: false,
                validation: false,
            }),
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("kind mismatch"));
    assert!(err.to_string().contains("non-policy plugin"), "{err:?}");
    // policy kind 缺元数据。
    let err = mgr
        .register(
            parse_manifest("p2", "policy", ""),
            PluginTrustState::Trusted,
            0,
            None,
            None,
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("missing meta"));
    assert!(err.to_string().contains("policy plugin missing metadata"));
    // 零接管点。
    let err = mgr
        .register(
            parse_manifest("p3", "policy", ""),
            PluginTrustState::Trusted,
            0,
            None,
            Some(PolicyMetaSnapshot {
                compact: false,
                final_judge: false,
                validation: false,
            }),
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("zero points"));
    assert!(err.to_string().contains("at least one policy point"));
    // 合法注册。
    mgr.register(
        parse_manifest("p4", "policy", ""),
        PluginTrustState::Trusted,
        0,
        None,
        Some(PolicyMetaSnapshot {
            compact: false,
            final_judge: true,
            validation: false,
        }),
        None,
    )
    .expect("policy ok");
    // 唯一接管者对账：FinalJudge 已被 p4 占用。
    let err = mgr
        .register(
            parse_manifest("p5", "policy", ""),
            PluginTrustState::Trusted,
            0,
            None,
            Some(PolicyMetaSnapshot {
                compact: false,
                final_judge: true,
                validation: false,
            }),
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("clash"));
    assert!(err.to_string().contains("already taken by plugin 'p4'"));
}

#[tokio::test(flavor = "multi_thread")]
async fn register_panel_and_step_meta_matrix() {
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    // panel 带 tool 元数据 = 拒（tool 元数据检查先于 panel 检查，
    // 错误文案按实际闸序）。
    let err = mgr
        .register(
            parse_manifest("panel1", "panel", ""),
            PluginTrustState::Trusted,
            0,
            tool_meta("panel1", "x"),
            None,
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("panel must not carry tool meta"));
    assert!(
        err.to_string()
            .contains("tool metadata provided for non-tool plugin"),
        "{err:?}"
    );
    // panel 带载荷必须给组件句柄。
    let err = mgr
        .register(
            parse_manifest("panel2", "panel", ""),
            PluginTrustState::Trusted,
            0,
            None,
            None,
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("payload panel needs component"));
    assert!(err.to_string().contains("requires a compiled component"));
    // 纯声明式 panel（无 sha）+ component=0 哨兵 = 合法。
    let declarative = PluginManifest::parse(
            "api-version = 1\nslug = \"panel3\"\nkind = \"panel\"\nname = \"N\"\nversion = \"0.1.0\"\n\n[[ui.page]]\nroute = \"home\"\ntitle = \"首页\"\n",
        )
        .expect("manifest")
        .0;
    mgr.register(declarative, PluginTrustState::Trusted, 0, None, None, None)
        .expect("declarative panel ok");
    // 路由跨插件唯一。
    let clash = PluginManifest::parse(
            "api-version = 1\nslug = \"panel4\"\nkind = \"panel\"\nname = \"N\"\nversion = \"0.1.0\"\n\n[[ui.page]]\nroute = \"home\"\ntitle = \"首页\"\n",
        )
        .expect("manifest")
        .0;
    let err = mgr
        .register(clash, PluginTrustState::Trusted, 0, None, None, None)
        .err()
        .unwrap_or_else(|| panic!("route clash"));
    assert!(err.to_string().contains("ui route 'home' already declared"));
    // 非 flow-step 带 step 元数据（observer kind 过 tool 检查）。
    let err = mgr
        .register(
            parse_manifest("s1", "observer", ""),
            PluginTrustState::Trusted,
            0,
            None,
            None,
            Some(StepMetaSnapshot {
                kinds: vec!["echo".into()],
            }),
        )
        .err()
        .unwrap_or_else(|| panic!("step meta on non-flow-step"));
    assert!(err.to_string().contains("non-flow-step plugin"), "{err:?}");
    // flow-step 缺元数据 / 空词表 / 非法词。
    let err = mgr
        .register(
            parse_manifest("s2", "flow-step", ""),
            PluginTrustState::Trusted,
            0,
            None,
            None,
            None,
        )
        .err()
        .unwrap_or_else(|| panic!("flow-step missing meta"));
    assert!(
        err.to_string()
            .contains("flow-step plugin missing metadata")
    );
    let err = mgr
        .register(
            parse_manifest("s3", "flow-step", ""),
            PluginTrustState::Trusted,
            0,
            None,
            None,
            Some(StepMetaSnapshot { kinds: vec![] }),
        )
        .err()
        .unwrap_or_else(|| panic!("empty kinds"));
    assert!(err.to_string().contains("at least one step kind"));
    let err = mgr
        .register(
            parse_manifest("s4", "flow-step", ""),
            PluginTrustState::Trusted,
            0,
            None,
            None,
            Some(StepMetaSnapshot {
                kinds: vec!["has space".into()],
            }),
        )
        .err()
        .unwrap_or_else(|| panic!("invalid kind"));
    assert!(err.to_string().contains("invalid step kind"));
    // 合法 flow-step（词表带冒号）。
    mgr.register(
        parse_manifest("s5", "flow-step", ""),
        PluginTrustState::Trusted,
        0,
        None,
        None,
        Some(StepMetaSnapshot {
            kinds: vec!["jira:fetch".into()],
        }),
    )
    .expect("flow-step ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn register_creates_data_dir_and_reports_limits_ignored() {
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    // manifest limits 放宽请求被忽略（只允许收紧）。
    let manifest = PluginManifest::parse(&manifest_raw(
        "lim",
        "tool",
        "\n[limits]\nfuel = 999_999_999_999\nbogus = 1\n",
    ))
    .expect("manifest")
    .0;
    mgr.register(
        manifest,
        PluginTrustState::Trusted,
        0,
        tool_meta("lim", "run"),
        None,
        None,
    )
    .expect("register");
    let reg = mgr.get("lim").expect("present");
    assert_eq!(reg.limits.fuel, mgr.limits().fuel, "放宽被忽略");
    assert!(reg.data_dir.is_dir(), "数据目录已创建");
    assert_eq!(reg.dir.file_name().unwrap_or_default(), "lim");
}

#[tokio::test(flavor = "multi_thread")]
async fn register_upgrade_replaces_and_observer_queue_recreated() {
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    mgr.register(
        parse_manifest("ob", "observer", ""),
        PluginTrustState::ReviewRequired,
        0,
        None,
        None,
        None,
    )
    .expect("register 1");
    assert!(
        mgr.get("ob")
            .expect("present")
            .dropped_events
            .load(Ordering::Relaxed)
            == 0
    );
    // 升级替换：先摘旧队列再拉新 worker——replace 路径不炸。
    mgr.register(
        parse_manifest("ob", "observer", ""),
        PluginTrustState::ReviewRequired,
        0,
        None,
        None,
        None,
    )
    .expect("register 2 (upgrade)");
    mgr.enqueue_observer_event(r#"{"type":"tool_start"}"#.into());
}

// -- 注销 / 查询 ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn unregister_and_lookup_surfaces() {
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    assert!(!mgr.unregister("ghost"));
    mgr.register(
        parse_manifest("t", "tool", ""),
        PluginTrustState::Trusted,
        0,
        tool_meta("t", "run"),
        None,
        None,
    )
    .expect("register");
    assert!(mgr.get("t").is_some());
    assert_eq!(mgr.list().len(), 1);
    // find_by_tool_name 各失败臂。
    assert!(mgr.find_by_tool_name("no-prefix").is_none());
    assert!(mgr.find_by_tool_name("plugin.").is_none());
    assert!(mgr.find_by_tool_name("plugin.ghost.run").is_none());
    assert!(mgr.find_by_tool_name("plugin.t.wrong").is_none());
    assert!(mgr.find_by_tool_name("plugin.t.run").is_some());
    // 注销。
    assert!(mgr.unregister("t"));
    assert!(mgr.get("t").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn path_helpers() {
    crate::test_support::ensure_host_dll();
    let (ws, mgr) = crate::test_support::default_manager();
    assert_eq!(mgr.plugin_dir("x").file_name().unwrap_or_default(), "x");
    assert_eq!(
        mgr.plugin_data_dir("x").file_name().unwrap_or_default(),
        "x"
    );
    assert!(mgr.workspace_root().is_dir());
    assert!(mgr.plugins_dir().is_dir());
    let _ = ws;
}

// -- 开关与实例配置 ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn config_key_discipline_and_roundtrip() {
    crate::test_support::ensure_host_dll();
    let (ws, mgr) = crate::test_support::default_manager();
    // 载荷目录（未加载也可设）。
    let dir = mgr.plugin_dir("cfg-demo");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("plugin.toml"),
        manifest_raw(
            "cfg-demo",
            "tool",
            "\n[permissions]\nx-secret = [\"vault_key\"]\n\n[config-schema]\nlang = \"语言\"\n",
        ),
    )
    .expect("write manifest");

    // 非法 slug 拒（join 越界防）。
    for bad in ["../x", "", "ALL", "-x"] {
        let err = mgr
            .set_config_key(bad, "lang", "v")
            .err()
            .unwrap_or_else(|| panic!("invalid slug"));
        assert!(err.to_string().contains("invalid slug"), "{bad}");
        let _ = mgr.unset_config_key(bad, "lang");
        mgr.disk_manifest(bad)
            .err()
            .unwrap_or_else(|| panic!("invalid slug"));
    }
    // x-secret 键走 vault。
    let err = mgr
        .set_config_key("cfg-demo", "vault_key", "v")
        .err()
        .unwrap_or_else(|| panic!("x-secret key"));
    assert!(err.to_string().contains("vault"), "{err}");
    // 未声明键。
    let err = mgr
        .set_config_key("cfg-demo", "ghost", "v")
        .err()
        .unwrap_or_else(|| panic!("undeclared"));
    assert!(err.to_string().contains("key not declared"));
    // 合法写入 + 读回。
    mgr.set_config_key("cfg-demo", "lang", "zh").expect("set");
    let sw = mgr.get_config("cfg-demo");
    assert!(!sw.enabled, "缺省关");
    assert_eq!(sw.config.get("lang").and_then(|v| v.as_str()), Some("zh"));
    // get_switch 同源。
    assert!(mgr.get_switch("cfg-demo").config.contains_key("lang"));
    // unset（在场删除 + 幂等）。
    mgr.unset_config_key("cfg-demo", "lang").expect("unset");
    assert!(!mgr.get_config("cfg-demo").config.contains_key("lang"));
    mgr.unset_config_key("cfg-demo", "lang")
        .expect("unset idempotent");
    let _ = ws;
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_instance_file_migrates_on_read() {
    crate::test_support::ensure_host_dll();
    let (ws, mgr) = crate::test_support::default_manager();
    crate::test_support::write_legacy_instance(
        &ws,
        "legacy-demo",
        r#"{"entries":{"old_key":"old_val"}}"#,
    );
    let sw = mgr.get_switch("legacy-demo");
    // enabled 强制 false（新纪律接管），entries 保留。
    assert!(!sw.enabled);
    assert_eq!(
        sw.config.get("old_key").and_then(|v| v.as_str()),
        Some("old_val")
    );
    // 旧文件已删（并入主文件）。
    assert!(
        !ws.join("config")
            .join("plugins")
            .join("legacy-demo.json")
            .exists(),
        "旧实例文件并入后删除"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn disk_manifest_reads_payload_dir() {
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    let dir = mgr.plugin_dir("disk-m");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("plugin.toml"), manifest_raw("disk-m", "tool", "")).expect("write");
    let m = mgr.disk_manifest("disk-m").expect("read");
    assert_eq!(m.slug, "disk-m");
    // 目录缺席 = Manifest 错。
    mgr.disk_manifest("ghost-slug")
        .err()
        .unwrap_or_else(|| panic!("missing dir"));
}

// -- 总开关 / 审计 ----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn master_switch_and_audit_surface() {
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    assert!(mgr.is_enabled());
    mgr.set_enabled(false);
    assert!(!mgr.is_enabled());
    mgr.set_enabled(true);
    // 审计器未装配 = 静默跳过。
    mgr.audit_event("e", "allow", "t", "LOW", "r", "sess");
    // 装配后落盘。
    let dir = tempfile::tempdir().expect("audit tmp");
    let logger = nemesis_security::audit_log::AuditLogger::new(
        nemesis_security::audit_log::AuditLogConfig {
            audit_log_dir: dir.path().to_path_buf(),
            enabled: true,
        },
    )
    .expect("logger");
    mgr.set_audit(Arc::new(std::sync::Mutex::new(logger)));
    mgr.audit_event(
        "plugin_ui_invoke",
        "allow",
        "slug",
        "LOW",
        "reason",
        "sess-1",
    );
    assert!(
        std::fs::read_dir(dir.path()).expect("dir").count() > 0,
        "审计记录落盘"
    );
}

// -- 执行闸链（tool 全链用 textstat fixture）---------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn execute_tool_gates_and_roundtrip() {
    crate::test_support::ensure_host_dll();
    let (ws, mgr) = crate::test_support::default_manager();
    let toml = "api-version = 1\nslug = \"textstat\"\nkind = \"tool\"\nname = \"TextStat\"\nversion = \"0.1.0\"\n\n[config-schema]\ninclude_spaces = \"是否含空格\"\n";
    if !install_fixture(&mgr, &ws, "textstat", "textstat_plugin", toml, "textstat").await {
        return;
    }

    // 未知工具名。
    let err = mgr
        .execute_tool("plugin.ghost.run", "{}".into(), ExecContext::default())
        .await
        .err()
        .unwrap_or_else(|| panic!("unknown tool"));
    assert!(err.to_string().contains("plugin tool not found"));

    // 子系统总开关关。
    mgr.set_enabled(false);
    let err = mgr
        .execute_tool(
            "plugin.textstat.textstat",
            "{}".into(),
            ExecContext::default(),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("disabled"));
    assert!(err.to_string().contains("disabled"));
    mgr.set_enabled(true);

    // 插件 enable 位关。
    let reg = mgr.get("textstat").expect("registered");
    reg.enabled.store(false, Ordering::SeqCst);
    let err = mgr
        .execute_tool(
            "plugin.textstat.textstat",
            "{}".into(),
            ExecContext::default(),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("plugin disabled"));
    assert!(err.to_string().contains("disabled"));
    reg.enabled.store(true, Ordering::SeqCst);

    // 运行期总闸关。
    crate::test_support::disable_runtime_gate(&ws);
    let err = mgr
        .execute_tool(
            "plugin.textstat.textstat",
            "{}".into(),
            ExecContext::default(),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("runtime gate"));
    assert!(err.to_string().contains("runtime master gate"));
    // 恢复闸（重写配置文件为启用态——native 闸文件删除 = 缺省启用）。
    std::fs::remove_file(ws.join("config").join("config.plugin.native.json"))
        .expect("restore gate");

    // 正常执行（fresh-store 全链 + 实例配置投影）。
    let out = mgr
        .execute_tool(
            "plugin.textstat.textstat",
            r#"{"text":"hello world"}"#.into(),
            ExecContext {
                session_key: "s1".into(),
                call_id: "c1".into(),
            },
        )
        .await
        .expect("execute");
    assert!(!out.is_error, "content: {}", out.content);
    assert!(
        out.content.contains("\"words\":2"),
        "content: {}",
        out.content
    );

    // 实例配置热生效（config.set → 下帧生效）。
    mgr.set_config_key("textstat", "include_spaces", "false")
        .expect("set");
    let out = mgr
        .execute_tool(
            "plugin.textstat.textstat",
            r#"{"text":"hello world"}"#.into(),
            ExecContext::default(),
        )
        .await
        .expect("execute 2");
    assert!(
        out.content.contains("\"chars\":11"),
        "content: {}",
        out.content
    );
}

// -- 面板 / 策略 / flow 步骤闸链 ---------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn invoke_ui_gates_and_roundtrip() {
    crate::test_support::ensure_host_dll();
    let (ws, mgr) = crate::test_support::default_manager();
    // 未加载。
    let err = mgr
        .invoke_ui("ghost", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("not loaded"));
    assert!(err.to_string().contains("not loaded"));
    // 非 panel kind。
    let manifest = parse_manifest("notpanel", "tool", "");
    mgr.register(
        manifest,
        PluginTrustState::Trusted,
        0,
        tool_meta("notpanel", "run"),
        None,
        None,
    )
    .expect("register");
    let err = mgr
        .invoke_ui("notpanel", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("wrong kind"));
    assert!(err.to_string().contains("is not a panel"));

    // 纯声明式 panel（component=0）= cmd:// 不可达。
    let declarative = PluginManifest::parse(
            "api-version = 1\nslug = \"decl-panel\"\nkind = \"panel\"\nname = \"N\"\nversion = \"0.1.0\"\n\n[[ui.page]]\nroute = \"home\"\ntitle = \"首页\"\n",
        )
        .expect("manifest")
        .0;
    mgr.register(declarative, PluginTrustState::Trusted, 0, None, None, None)
        .expect("register panel");
    let err = mgr
        .invoke_ui("decl-panel", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("declarative panel"));
    assert!(err.to_string().contains("declarative"));

    // panel-echo fixture 全链（fixture 缺席 = 跳过执行面）。
    let toml = "api-version = 1\nslug = \"panel-echo\"\nkind = \"panel\"\nname = \"Panel Echo\"\nversion = \"0.1.0\"\n\n[[ui.page]]\nroute = \"echo\"\ntitle = \"回显面板\"\n\n[[ui.page.views]]\nkind = \"actions\"\nbind = \"\"\n\n[[ui.page.views.buttons]]\nlabel = \"回显\"\ncommand = \"echo\"\n";
    if !install_fixture(
        &mgr,
        &ws,
        "panel-echo",
        "panel_echo_plugin",
        toml,
        "panel-echo",
    )
    .await
    {
        return;
    }
    // 总开关关。
    mgr.set_enabled(false);
    let err = mgr
        .invoke_ui("panel-echo", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("disabled"));
    assert!(err.to_string().contains("disabled"));
    mgr.set_enabled(true);
    // 运行期总闸关。
    crate::test_support::disable_runtime_gate(&ws);
    let err = mgr
        .invoke_ui("panel-echo", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("runtime gate"));
    assert!(err.to_string().contains("runtime master gate"));
    std::fs::remove_file(ws.join("config").join("config.plugin.native.json"))
        .expect("restore gate");
    // 正常转发（echo 往返，guest 回执 ok:true）。
    let reply = mgr
        .invoke_ui(
            "panel-echo",
            r#"{"command":"echo","args":{"text":"你好"}}"#.into(),
        )
        .await
        .expect("echo");
    assert!(reply.contains("\"ok\":true"), "reply: {reply}");
}

#[tokio::test(flavor = "multi_thread")]
async fn call_policy_point_gates_and_roundtrip() {
    crate::test_support::ensure_host_dll();
    let (ws, mgr) = crate::test_support::default_manager();
    // 无接管者。
    let err = mgr
        .call_policy_point(PolicyPoint::Validation, "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("no owner"));
    assert!(err.to_string().contains("no active policy plugin"));

    // policy-sample fixture 全链。
    let toml = "api-version = 1\nslug = \"policy-sample\"\nkind = \"policy\"\nname = \"P\"\nversion = \"0.1.0\"\n";
    if !install_fixture(
        &mgr,
        &ws,
        "policy-sample",
        "policy_sample_plugin",
        toml,
        "policy-sample",
    )
    .await
    {
        return;
    }
    assert!(mgr.policy_owner(PolicyPoint::Validation).is_some());
    assert!(mgr.policy_owner(PolicyPoint::Compact).is_some());
    // 子系统总开关关。
    mgr.set_enabled(false);
    let err = mgr
        .call_policy_point(PolicyPoint::Validation, "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("disabled"));
    assert!(err.to_string().contains("subsystem disabled"));
    mgr.set_enabled(true);
    // 运行期总闸关。
    crate::test_support::disable_runtime_gate(&ws);
    let err = mgr
        .call_policy_point(PolicyPoint::Validation, "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("runtime gate"));
    assert!(err.to_string().contains("runtime master gate"));
    std::fs::remove_file(ws.join("config").join("config.plugin.native.json"))
        .expect("restore gate");
    // 三点正常裁决（输入 JSON 投影透传）。
    for point in [
        PolicyPoint::Validation,
        PolicyPoint::Compact,
        PolicyPoint::FinalJudge,
    ] {
        let out = mgr
            .call_policy_point(
                point,
                r#"{"session_key":"s1","tool":"read_file","args":{"path":"a.txt"}}"#.into(),
            )
            .await
            .expect("verdict");
        assert!(out.contains("verdict"), "裁决 JSON: {out}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn run_flow_step_gates_and_roundtrip() {
    crate::test_support::ensure_host_dll();
    let (ws, mgr) = crate::test_support::default_manager();
    // 未加载 / kind 闸 / op 对账。
    let err = mgr
        .run_flow_step("ghost", "echo", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("not loaded"));
    assert!(err.to_string().contains("not loaded"));
    mgr.register(
        parse_manifest("notfs", "tool", ""),
        PluginTrustState::Trusted,
        0,
        tool_meta("notfs", "run"),
        None,
        None,
    )
    .expect("register");
    let err = mgr
        .run_flow_step("notfs", "echo", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("wrong kind"));
    assert!(err.to_string().contains("is not a flow-step plugin"));

    // stepdemo fixture（kinds = echo/tally/chaos）。
    let toml = "api-version = 1\nslug = \"stepdemo\"\nkind = \"flow-step\"\nname = \"S\"\nversion = \"0.1.0\"\n\n[limits]\nfuel = 20_000_000\ntimeout-ms = 10_000\n";
    if !install_fixture(&mgr, &ws, "stepdemo", "stepdemo_plugin", toml, "stepdemo").await {
        return;
    }
    let err = mgr
        .run_flow_step("stepdemo", "not_in_kinds", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("op not reported"));
    assert!(err.to_string().contains("not in plugin 'stepdemo'"));
    // 子系统总开关关。
    mgr.set_enabled(false);
    let err = mgr
        .run_flow_step("stepdemo", "echo", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("disabled"));
    assert!(err.to_string().contains("subsystem disabled"));
    mgr.set_enabled(true);
    // 插件位关。
    let reg = mgr.get("stepdemo").expect("registered");
    reg.enabled.store(false, Ordering::SeqCst);
    let err = mgr
        .run_flow_step("stepdemo", "echo", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("plugin disabled"));
    assert!(err.to_string().contains("disabled"));
    reg.enabled.store(true, Ordering::SeqCst);
    // 运行期总闸关。
    crate::test_support::disable_runtime_gate(&ws);
    let err = mgr
        .run_flow_step("stepdemo", "echo", "{}".into())
        .await
        .err()
        .unwrap_or_else(|| panic!("runtime gate"));
    assert!(err.to_string().contains("runtime master gate"));
    std::fs::remove_file(ws.join("config").join("config.plugin.native.json"))
        .expect("restore gate");
    // 正常投递（echo 同步 done + session_key 抽取进审计）。
    let out = mgr
        .run_flow_step(
            "stepdemo",
            "echo",
            r#"{"op":"echo","args":{"text":"流程"},"session_key":"flow:s/i"}"#.into(),
        )
        .await
        .expect("echo");
    assert!(out.contains("\"status\":\"done\""), "out: {out}");
}

// -- 接管仲裁 ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn takeover_owner_arbitration() {
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    // 无接管者。
    assert!(mgr.takeover_owner("weather_lookup").is_none());
    // 单接管者。
    let m_a = PluginManifest::parse(&manifest_raw(
        "aaa",
        "tool",
        "\ntakeover = [\"weather_lookup\"]\n",
    ))
    .expect("manifest")
    .0;
    mgr.register(
        m_a,
        PluginTrustState::Trusted,
        0,
        tool_meta("aaa", "run"),
        None,
        None,
    )
    .expect("register a");
    assert_eq!(
        mgr.takeover_owner("weather_lookup")
            .expect("owner")
            .manifest
            .slug,
        "aaa"
    );
    // 双声明 = slug 字典序最小者胜。
    let m_b = PluginManifest::parse(&manifest_raw(
        "bbb",
        "tool",
        "\ntakeover = [\"weather_lookup\"]\n",
    ))
    .expect("manifest")
    .0;
    mgr.register(
        m_b,
        PluginTrustState::Trusted,
        0,
        tool_meta("bbb", "run"),
        None,
        None,
    )
    .expect("register b");
    assert_eq!(
        mgr.takeover_owner("weather_lookup")
            .expect("owner")
            .manifest
            .slug,
        "aaa"
    );
    // 禁用组件不参选。
    let reg = mgr.get("aaa").expect("present");
    reg.enabled.store(false, Ordering::SeqCst);
    assert_eq!(
        mgr.takeover_owner("weather_lookup")
            .expect("owner")
            .manifest
            .slug,
        "bbb"
    );
    reg.enabled.store(true, Ordering::SeqCst);
}

// -- 观察者队列缺位计数 --------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn enqueue_without_queue_counts_drop_for_observer_kind() {
    // 注册后手工注销队列的窗口无法直接构造（observers 私有）；这里用
    // tool kind（本无队列）验证「无队列不补计数」的语义面。
    crate::test_support::ensure_host_dll();
    let (_ws, mgr) = crate::test_support::default_manager();
    mgr.register(
        parse_manifest("tool-noq", "tool", ""),
        PluginTrustState::Trusted,
        0,
        tool_meta("tool-noq", "run"),
        None,
        None,
    )
    .expect("register");
    mgr.enqueue_observer_event(r#"{"type":"tool_start"}"#.into());
    let reg = mgr.get("tool-noq").expect("present");
    assert_eq!(
        reg.dropped_events.load(Ordering::Relaxed),
        0,
        "tool 类无队列不补计数"
    );
}
