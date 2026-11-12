use super::*;
use crate::registry::PolicyMetaSnapshot;
use std::path::PathBuf;

const DECLARATIVE_PANEL_TOML: &str = r#"api-version = 1
slug = "decl-panel"
kind = "panel"
name = "DeclPanel"
version = "0.1.0"

[[ui.page]]
route = "home"
title = "首页"
"#;

const TOOL_TOML_BODY: &str = r#"api-version = 1
slug = "textstat"
kind = "tool"
name = "TextStat"
version = "0.1.0"

[limits]
fuel = 20_000_000
timeout-ms = 10_000

[config-schema]
include_spaces = "是否含空格"
"#;

/// 拒绝审批卡（附拒绝理由）的审批面。
struct DenyApprove {
    reason: String,
}

impl InstallApprover for DenyApprove {
    fn approve<'a>(
        &'a self,
        _review: &'a InstallReview,
    ) -> futures::future::BoxFuture<'a, Result<bool, String>> {
        let reason = self.reason.clone();
        Box::pin(async move { Err(reason) })
    }

    fn can_approve_unsigned(&self) -> bool {
        true
    }
}

/// 具备人批卡能力的审批面（手工拷入无签名容忍判定用）。
struct CardApprove;

impl InstallApprover for CardApprove {
    fn approve<'a>(
        &'a self,
        _review: &'a InstallReview,
    ) -> futures::future::BoxFuture<'a, Result<bool, String>> {
        Box::pin(async { Ok(true) })
    }

    fn can_approve_unsigned(&self) -> bool {
        true
    }
}

fn fresh() -> (std::path::PathBuf, Arc<PluginManager>) {
    crate::test_support::ensure_host_dll();
    crate::test_support::default_manager()
}

fn installer(mgr: &Arc<PluginManager>) -> PluginInstaller {
    PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None)
}

fn stage(root: &Path, toml: &str, wasm: Option<&[u8]>) -> PathBuf {
    crate::test_support::stage_plugin(root, toml, wasm)
}

async fn install_ok(inst: &PluginInstaller, src: &Path, allow_unsigned: bool) -> InstallOutcome {
    inst.install(src, allow_unsigned).await.expect("install ok")
}

// -- AutoApprove / 审批面 ---------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn auto_approve_returns_true() {
    let review = InstallReview {
        slug: "s".into(),
        name: "n".into(),
        version: "0.1.0".into(),
        kind: "tool".into(),
        trust_state: "trusted".into(),
        trust_level: String::new(),
        signer_key: String::new(),
        egress: vec![],
        x_secret: vec![],
        wasm_bytes: 0,
        wasm_sha256: String::new(),
        ui_pages: 0,
        flow_templates: 0,
        connectors: vec![],
        limits_ignored: vec![],
        source_dir: String::new(),
    };
    assert!(AutoApprove.approve(&review).await.expect("approve"));
    assert!(!AutoApprove.can_approve_unsigned(), "缺省无批卡能力");
}

// -- 漏斗失败面（早拒早干净，零突变）------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn install_missing_source_dir_fails() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    let err = inst
        .install(Path::new("Z:/definitely/absent/src"), true)
        .await
        .expect_err("missing source");
    assert!(err.to_string().contains("source dir"), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn install_missing_manifest_fails() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    let dir = tempfile::tempdir().expect("tmp");
    let err = inst
        .install(dir.path(), true)
        .await
        .expect_err("no plugin.toml");
    assert!(err.to_string().contains("read plugin.toml"));
}

#[tokio::test(flavor = "multi_thread")]
async fn install_unsigned_strict_rejected() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    let dir = tempfile::tempdir().expect("tmp");
    let src = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    let err = inst
        .install(&src, false)
        .await
        .expect_err("unsigned + strict");
    assert!(err.to_string().contains("trust check failed"));
    // 零突变：载荷与 lockfile 均未落。
    assert!(!mgr.plugin_dir("decl-panel").join("plugin.toml").exists());
    assert!(!mgr.plugins_dir().join("lockfile.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn install_approver_rejection_carries_note() {
    let (_ws, mgr) = fresh();
    let inst = PluginInstaller::new(
        mgr.clone(),
        Arc::new(DenyApprove {
            reason: "sha 不明".into(),
        }),
        None,
    );
    let dir = tempfile::tempdir().expect("tmp");
    let src = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    let err = inst
        .install(&src, true)
        .await
        .err()
        .unwrap_or_else(|| panic!("rejected"));
    let text = err.to_string();
    assert!(text.contains("install rejected"), "{text}");
    assert!(text.contains("sha 不明"), "拒绝附言透传: {text}");
    assert!(!mgr.plugin_dir("decl-panel").join("plugin.toml").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn install_bad_connector_toml_fails_closed() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    let dir = tempfile::tempdir().expect("tmp");
    let src = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    std::fs::write(src.join("connector.toml"), "this is [not a connector").expect("write");
    let err = inst
        .install(&src, true)
        .await
        .err()
        .unwrap_or_else(|| panic!("connector"));
    assert!(err.to_string().contains("connector.toml 解析失败"));
    assert!(!mgr.plugin_dir("decl-panel").join("plugin.toml").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn install_sha_mismatch_rejected() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    // 载荷在盘但 manifest sha 与内容不符（stage 写的 sha 是对的——手工改坏）。
    let bytes = b"garbage wasm";
    let dir2 = tempfile::tempdir().expect("tmp2");
    let src = stage(dir2.path(), TOOL_TOML_BODY, Some(bytes));
    let raw = std::fs::read_to_string(src.join("plugin.toml")).expect("read");
    // 记账 sha 首个 hex 位改写（长度不变，64 位形态仍合法，值必不同）。
    let key = "wasm-sha256 = \"";
    let start = raw.find(key).expect("sha line") + key.len();
    let mut wrong = raw.clone();
    let swap = if raw.as_bytes()[start] == b'e' {
        "0"
    } else {
        "e"
    };
    wrong.replace_range(start..start + 1, swap);
    assert_ne!(raw, wrong, "sha 行必须存在且被改写");
    std::fs::write(src.join("plugin.toml"), wrong).expect("rewrite");
    let err = inst
        .install(&src, true)
        .await
        .err()
        .unwrap_or_else(|| panic!("sha mismatch"));
    assert!(err.to_string().contains("sha256 mismatch"), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn install_wasm_filename_escape_rejected() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    // 手写源目录（manifest 自带 sha，不经 stage 补写以免重复键）。
    let sha = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(b"garbage wasm"))
    };
    let dir = tempfile::tempdir().expect("tmp");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(
            src.join("plugin.toml"),
            format!("api-version = 1\nwasm-sha256 = \"{sha}\"\nwasm = \"../evil.wasm\"\nslug = \"escape\"\nkind = \"tool\"\nname = \"N\"\nversion = \"0.1.0\"\n"),
        )
        .expect("write");
    std::fs::write(dir.path().join("evil.wasm"), b"garbage wasm").expect("write");
    // 文件名纯度闸先于读盘触发（join 逃逸面）。
    let err = inst.install(&src, true).await.expect_err("escape rejected");
    assert!(err.to_string().contains("纯文件名"), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn install_oversize_wasm_rejected() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    let dir = tempfile::tempdir().expect("tmp");
    // 64MiB+1 零字节文件（稀疏写不为零时也是超限——metadata.len 判定）。
    let big = vec![0u8; (WASM_MAX_BYTES + 1) as usize];
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(
            src.join("plugin.toml"),
            format!("api-version = 1\nwasm-sha256 = \"{}\"\nwasm = \"big.wasm\"\nslug = \"big\"\nkind = \"tool\"\nname = \"N\"\nversion = \"0.1.0\"\n", "a".repeat(64)),
        )
        .expect("write");
    std::fs::write(src.join("big.wasm"), &big).expect("write big");
    let err = inst
        .install(&src, true)
        .await
        .err()
        .unwrap_or_else(|| panic!("oversize"));
    assert!(err.to_string().contains("exceeds"), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn install_invalid_wasm_fails_at_compile() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    let dir = tempfile::tempdir().expect("tmp");
    // 垃圾字节 + 正确 sha：过 ③，死在 ⑥ 编译（manifest 自带 sha，不经
    // stage 补写以免重复键）。
    let sha = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(b"garbage wasm"))
    };
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(
            src.join("plugin.toml"),
            format!("api-version = 1\nslug = \"badwasm\"\nkind = \"tool\"\nname = \"N\"\nversion = \"0.1.0\"\nwasm-sha256 = \"{sha}\"\n"),
        )
        .expect("write");
    std::fs::write(src.join("plugin.wasm"), b"garbage wasm").expect("write");
    let err = inst
        .install(&src, true)
        .await
        .err()
        .unwrap_or_else(|| panic!("compile"));
    assert!(matches!(err, PluginError::Compile(_)), "{err:?}");
    // lockfile 未写（编译失败先于 ⑦）。
    assert!(!mgr.plugins_dir().join("lockfile.json").exists());
}

// -- 声明式面板全链（①②⑤⑦⑧ + 开关 + 装载 + 卸载）----------------------------

#[tokio::test(flavor = "multi_thread")]
async fn declarative_panel_full_funnel_roundtrip() {
    let (ws, mgr) = fresh();
    let inst = installer(&mgr);
    let dir = tempfile::tempdir().expect("tmp");
    let src = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    let outcome = install_ok(&inst, &src, true).await;
    assert_eq!(outcome.slug, "decl-panel");
    assert_eq!(outcome.trust, PluginTrustState::ReviewRequired);
    assert!(outcome.registered.is_none(), "新装默认不加载");

    // ⑦ lockfile（trust/签名字段记账）。
    let lf = std::fs::read_to_string(mgr.plugins_dir().join("lockfile.json")).expect("lf");
    assert!(lf.contains("\"decl-panel\""));
    assert!(lf.contains("review-required"));
    // ⑧ 落位（manifest 原文入库）。
    let landed =
        std::fs::read_to_string(mgr.plugin_dir("decl-panel").join("plugin.toml")).expect("landed");
    assert!(landed.contains("decl-panel"));

    // 装载（声明式：component=0 哨兵注册）。
    let reg = inst
        .set_enabled("decl-panel", true)
        .await
        .expect("enable")
        .expect("loaded");
    assert_eq!(reg.component, 0);
    assert!(mgr.get("decl-panel").is_some());
    // 幂等 enable（已注册只写开关）。
    let reg2 = inst
        .set_enabled("decl-panel", true)
        .await
        .expect("enable again")
        .expect("still loaded");
    assert_eq!(reg2.component, 0);

    // 热升级路径：开关开着再装 = 摘旧重注册。
    let src2 = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    let outcome2 = install_ok(&inst, &src2, true).await;
    assert!(outcome2.registered.is_some(), "开关开着 = 重注册（热升级）");

    // 关闭升级：开关翻关再装 = 只换载荷不装载。
    inst.set_enabled("decl-panel", false)
        .await
        .expect("disable");
    assert!(mgr.get("decl-panel").is_none(), "off = 注册摘除");
    let src3 = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    let outcome3 = install_ok(&inst, &src3, true).await;
    assert!(outcome3.registered.is_none());
    assert!(!mgr.get_switch("decl-panel").enabled, "开关记录保持关");

    // 卸载全链（lockfile 摘除 → 注销 → 删载荷 → 开关摘除）。
    let removed = crate::install::uninstall(&mgr, "decl-panel")
        .await
        .expect("uninstall");
    assert!(removed);
    assert!(mgr.get("decl-panel").is_none());
    assert!(!mgr.plugin_dir("decl-panel").exists());
    let lf2 = std::fs::read_to_string(mgr.plugins_dir().join("lockfile.json")).expect("lf2");
    assert!(!lf2.contains("decl-panel"), "lockfile 条目已摘: {lf2}");
    assert!(
        !mgr.get_switch("decl-panel").enabled && mgr.get_switch("decl-panel").config.is_empty(),
        "开关记录摘除"
    );
    let _ = ws;
}

#[tokio::test(flavor = "multi_thread")]
async fn uninstall_semantics_matrix() {
    let (_ws, mgr) = fresh();
    // 非法 slug。
    let err = crate::install::uninstall(&mgr, "../x")
        .await
        .expect_err("invalid slug");
    assert!(err.to_string().contains("invalid slug"));
    let err = crate::install::uninstall_blocking(&mgr, "../x")
        .err()
        .unwrap_or_else(|| panic!("invalid slug"));
    assert!(err.to_string().contains("invalid slug"));
    // 全缺席 = false。
    assert!(
        !crate::install::uninstall(&mgr, "ghost")
            .await
            .expect("absent")
    );
    assert!(!crate::install::uninstall_blocking(&mgr, "ghost").expect("absent"));
}

// -- 手工拷入（无 lockfile）补跑漏斗 + 无签名容忍判定 ---------------------------

#[tokio::test(flavor = "multi_thread")]
async fn hand_copied_payload_refunnels_on_enable() {
    let (ws, mgr) = fresh();
    // 手工把载荷目录放进 plugins_wasm（无 lockfile 记录）。
    let pdir = mgr.plugin_dir("decl-panel");
    std::fs::create_dir_all(&pdir).expect("mkdir");
    std::fs::write(pdir.join("plugin.toml"), DECLARATIVE_PANEL_TOML).expect("write");
    // CLI 面（AutoApprove 无批卡能力）：无签名 = 从紧拒绝。
    let inst = installer(&mgr);
    let err = inst
        .set_enabled("decl-panel", true)
        .await
        .expect_err("strict refuse");
    assert!(err.to_string().contains("trust check failed"), "{err:?}");
    // 开关已先落盘（诚实可见态）。
    assert!(mgr.get_switch("decl-panel").enabled);

    // 换具备批卡能力的审批面：无签名转卡裁决 → 放行。
    let inst2 = PluginInstaller::new(mgr.clone(), Arc::new(CardApprove), None);
    let reg = inst2
        .set_enabled("decl-panel", true)
        .await
        .expect("enable")
        .expect("loaded");
    assert_eq!(reg.component, 0);
    assert!(mgr.get("decl-panel").is_some());
    // 补跑漏斗后 lockfile 补记（allow_unsigned 容差沿袭）。
    let lf = std::fs::read_to_string(mgr.plugins_dir().join("lockfile.json")).expect("lf");
    assert!(lf.contains("review-required"));
    let _ = ws;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_enabled_missing_payload_fails() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    let err = inst
        .set_enabled("ghost", true)
        .await
        .expect_err("not found");
    assert!(err.to_string().contains("not found"));
    let err = inst
        .set_enabled("../x", true)
        .await
        .err()
        .unwrap_or_else(|| panic!("bad slug"));
    assert!(err.to_string().contains("invalid slug"));
}

#[tokio::test(flavor = "multi_thread")]
async fn set_enabled_off_unregisters_and_writes_switch() {
    let (ws, mgr) = fresh();
    let inst = installer(&mgr);
    let dir = tempfile::tempdir().expect("tmp");
    let src = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    install_ok(&inst, &src, true).await;
    inst.set_enabled("decl-panel", true).await.expect("enable");
    assert!(mgr.get("decl-panel").is_some());
    // off：注册摘除 + 开关落盘。
    assert!(
        inst.set_enabled("decl-panel", false)
            .await
            .expect("disable")
            .is_none()
    );
    assert!(mgr.get("decl-panel").is_none());
    assert!(!mgr.get_switch("decl-panel").enabled);
    let _ = ws;
}

// -- 启动装载 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn load_all_respects_switches() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    // 空目录 = 0。
    assert_eq!(inst.load_all().await, 0);

    // 装一个声明式面板：开关关 → 不装载。
    let dir = tempfile::tempdir().expect("tmp");
    let src = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    install_ok(&inst, &src, true).await;
    assert_eq!(inst.load_all().await, 0, "开关关 = 不装载");
    assert!(mgr.get("decl-panel").is_none());
    // 开关开 → 装载。
    nemesis_config::plugin_wasm::set_enabled(
        &mgr.workspace_root().join("config"),
        "decl-panel",
        true,
    )
    .expect("switch on");
    assert_eq!(inst.load_all().await, 1);
    assert!(mgr.get("decl-panel").is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn load_all_tolerates_broken_payload() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    // 坏载荷目录（manifest 解析失败）+ 开关开：load_one 失败但计数诚实。
    let pdir = mgr.plugin_dir("broken");
    std::fs::create_dir_all(&pdir).expect("mkdir");
    std::fs::write(pdir.join("plugin.toml"), "not a manifest [(((").expect("write");
    nemesis_config::plugin_wasm::set_enabled(&mgr.workspace_root().join("config"), "broken", true)
        .expect("switch on");
    assert_eq!(inst.load_all().await, 0, "装载失败不计数");
    assert!(mgr.get("broken").is_none());
}

// -- connector 装配自动注册 -----------------------------------------------------

const CONNECTOR_TOML: &str = r#"[connector]
id = "demo-conn"
name = "演示连接器"
version = "0.1.0"
kind = "rest"

[connector.http]
base = "https://example.com"

[[op]]
id = "get_quote"
intent = ["查询行情"]
risk = "low"
class = "read"
http = { method = "GET", path = "/quote" }
"#;

#[tokio::test(flavor = "multi_thread")]
async fn bundled_connector_registered_and_recorded() {
    let (ws, mgr) = fresh();
    let inst = installer(&mgr);
    let dir = tempfile::tempdir().expect("tmp");
    let src = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    std::fs::write(src.join("connector.toml"), CONNECTOR_TOML).expect("write");
    install_ok(&inst, &src, true).await;
    // 落文件即注册：connectors/<id>/connector.toml。
    let target = ws
        .join("connectors")
        .join("demo-conn")
        .join("connector.toml");
    assert!(target.is_file(), "connector 已注册");
    // lockfile 记账。
    let lf = std::fs::read_to_string(mgr.plugins_dir().join("lockfile.json")).expect("lf");
    assert!(lf.contains("demo-conn"), "{lf}");

    // 冲突不覆盖（内容不同 = 留守用户自定义）。
    let customized = CONNECTOR_TOML.replace("演示连接器", "用户改过的名字");
    std::fs::write(&target, &customized).expect("customize");
    let src2 = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    std::fs::write(src2.join("connector.toml"), CONNECTOR_TOML).expect("write");
    install_ok(&inst, &src2, true).await;
    assert_eq!(
        std::fs::read_to_string(&target).expect("read"),
        customized,
        "用户自定义未被覆盖"
    );

    // 内容一致 = no-op 且记账。
    std::fs::write(&target, CONNECTOR_TOML).expect("restore");
    let src3 = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    std::fs::write(src3.join("connector.toml"), CONNECTOR_TOML).expect("write");
    install_ok(&inst, &src3, true).await;
    // 记账保持单条（no-op 分支不改账）。
    let lf3 = std::fs::read_to_string(mgr.plugins_dir().join("lockfile.json")).expect("lf3");
    assert!(lf3.contains("demo-conn"));
    let _ = ws;
}

// -- tool 载荷全链（探针对账 + 注册快照）----------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn tool_payload_funnel_and_metadata_reconcile() {
    let (ws, mgr) = fresh();
    let inst = installer(&mgr);
    let Some(wasm) = crate::test_support::fixture_wasm("textstat", "textstat_plugin") else {
        eprintln!("SKIP：textstat fixture 缺席");
        return;
    };
    let bytes = std::fs::read(&wasm).expect("read");
    let dir = tempfile::tempdir().expect("tmp");
    let src = stage(dir.path(), TOOL_TOML_BODY, Some(&bytes));
    let outcome = install_ok(&inst, &src, true).await;
    assert_eq!(outcome.trust, PluginTrustState::ReviewRequired);
    // 开关开 = 装载注册（探针在 enable 的 load_one 里跑全链）。
    let reg = inst
        .set_enabled("textstat", true)
        .await
        .expect("enable")
        .expect("loaded");
    let meta = reg.tool_meta.as_ref().expect("tool meta");
    assert_eq!(meta.name, "plugin.textstat.textstat");
    assert_eq!(meta.base, "textstat");
    assert_eq!(meta.operation_type, "read");
    assert_eq!(meta.min_tier, "big");
    assert!(
        serde_json::from_str::<serde_json::Value>(&meta.parameters_json)
            .expect("schema json")
            .is_object()
    );
    // 数据目录已被探针帧就位。
    assert!(mgr.plugin_data_dir("textstat").is_dir());

    // 升级路径：载荷变了 sha = 拒载（目录名一致但内容漂移）。
    std::fs::write(mgr.plugin_dir("textstat").join("plugin.wasm"), b"tampered").expect("tamper");
    inst.set_enabled("textstat", false).await.expect("off");
    let err = inst
        .set_enabled("textstat", true)
        .await
        .expect_err("sha drift");
    assert!(err.to_string().contains("sha256 mismatch"), "{err:?}");
    // 目录名与 slug 不一致 = 孤儿载荷拒载。
    let orphan = mgr.plugins_dir().join("mislabeled");
    std::fs::create_dir_all(&orphan).expect("mkdir");
    std::fs::write(
        orphan.join("plugin.toml"),
        format!("{}\n", DECLARATIVE_PANEL_TOML).replace("decl-panel", "textstat2"),
    )
    .expect("write");
    let err = inst
        .set_enabled("textstat2", true)
        .await
        .expect_err("orphan");
    assert!(err.to_string().contains("不一致"), "{err:?}");
    let _ = ws;
}

#[tokio::test(flavor = "multi_thread")]
async fn observer_payload_funnel_skips_probe() {
    let (ws, mgr) = fresh();
    let inst = installer(&mgr);
    let Some(wasm) = crate::test_support::fixture_wasm("activity-log", "activity_log_plugin")
    else {
        eprintln!("SKIP：activity-log fixture 缺席");
        return;
    };
    let bytes = std::fs::read(&wasm).expect("read");
    let dir = tempfile::tempdir().expect("tmp");
    let toml = "api-version = 1\nslug = \"activity-log\"\nkind = \"observer\"\nname = \"A\"\nversion = \"0.1.0\"\n";
    let src = stage(dir.path(), toml, Some(&bytes));
    install_ok(&inst, &src, true).await;
    let reg = inst
        .set_enabled("activity-log", true)
        .await
        .expect("enable")
        .expect("loaded");
    // observer 跳过对账：三类快照全空，观察者 worker 已拉起。
    assert!(reg.tool_meta.is_none());
    assert!(reg.policy_meta.is_none());
    assert!(reg.step_meta.is_none());
    mgr.enqueue_observer_event(r#"{"type":"tool_start"}"#.into());
    let _ = ws;
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_payload_funnel_reconciles_points() {
    let (ws, mgr) = fresh();
    let inst = installer(&mgr);
    let Some(wasm) = crate::test_support::fixture_wasm("policy-sample", "policy_sample_plugin")
    else {
        eprintln!("SKIP：policy-sample fixture 缺席");
        return;
    };
    let bytes = std::fs::read(&wasm).expect("read");
    let dir = tempfile::tempdir().expect("tmp");
    let toml = "api-version = 1\nslug = \"policy-sample\"\nkind = \"policy\"\nname = \"P\"\nversion = \"0.1.0\"\n";
    let src = stage(dir.path(), toml, Some(&bytes));
    install_ok(&inst, &src, true).await;
    let reg = inst
        .set_enabled("policy-sample", true)
        .await
        .expect("enable")
        .expect("loaded");
    let pm: PolicyMetaSnapshot = reg.policy_meta.expect("policy meta");
    assert!(!pm.points().is_empty(), "三点自报非空");
    assert!(
        mgr.policy_owner(crate::limits::PolicyPoint::Validation)
            .is_some(),
        "接管者在册"
    );
    let _ = ws;
}

// -- 杂项小面 -----------------------------------------------------------------

#[test]
fn eight_short_name_detection() {
    for yes in ["ABCDEF~1", "AB~2.HTM", "a~0"] {
        assert!(is_eight_short_name(yes), "{yes}");
    }
    for no in ["plain.txt", "", "~", "a~", "a~x", "AB~C1"] {
        assert!(!is_eight_short_name(no), "{no}");
    }
}

#[test]
fn resilient_remove_dir_all_semantics() {
    let dir = tempfile::tempdir().expect("tmp");
    // 不存在 = Ok(false)（首装/已删）。
    assert!(
        !resilient_remove_dir_all(&dir.path().join("absent")).expect("absent"),
        "不存在应返回 Ok(false)"
    );
    // 在场 = Ok(true)。
    let target = dir.path().join("target");
    std::fs::create_dir_all(&target).expect("mkdir");
    std::fs::write(target.join("f.txt"), b"x").expect("write");
    assert!(
        resilient_remove_dir_all(&target).expect("removed"),
        "在场应返回 Ok(true)"
    );
    assert!(!target.exists());
}

#[test]
fn no_secrets_resolver_returns_none() {
    assert!(NoSecrets.resolve("vault:plugin/x/y").is_none());
}

#[test]
fn validate_step_kinds_matrix() {
    assert!(validate_step_kinds(&["echo".to_string()]).is_ok());
    assert!(validate_step_kinds(&["jira:fetch".to_string(), "a".repeat(64)]).is_ok());
    assert!(validate_step_kinds(&[]).is_err(), "空词表拒");
    assert!(validate_step_kinds(&["has space".to_string()]).is_err());
    assert!(validate_step_kinds(&["ctrl\u{7}".to_string()]).is_err());
}

// -- lockfile 读面 -------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn corrupted_lockfile_fails_install_late() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    // 预置损坏 lockfile → update_lockfile 诚实报错（静默重置会丢安装历史）。
    std::fs::write(mgr.plugins_dir().join("lockfile.json"), "{corrupted json")
        .expect("write corrupted");
    let dir = tempfile::tempdir().expect("tmp");
    let src = stage(dir.path(), DECLARATIVE_PANEL_TOML, None);
    let err = inst
        .install(&src, true)
        .await
        .err()
        .unwrap_or_else(|| panic!("lockfile"));
    assert!(err.to_string().contains("lockfile 解析失败"), "{err:?}");
    // 载荷未落位（lockfile-first 零突变）。
    assert!(!mgr.plugin_dir("decl-panel").join("plugin.toml").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn recorded_allow_unsigned_follows_lockfile_trust() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    // lockfile 无记录 = 从紧。
    assert!(!inst.recorded_allow_unsigned("ghost"));
    // 记录 review-required = 容差 true。
    let lf = PluginLockfile {
        version: 1,
        plugins: [(
            "tolerated".to_string(),
            LockfileEntry {
                version: "0.1.0".into(),
                wasm_sha256: String::new(),
                trust: PluginTrustState::ReviewRequired.as_str().into(),
                installed_at_ms: 0,
                signed_by: String::new(),
                connectors: vec![],
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::write(
        mgr.plugins_dir().join("lockfile.json"),
        serde_json::to_string_pretty(&lf).expect("serialize"),
    )
    .expect("write");
    assert!(inst.recorded_allow_unsigned("tolerated"));
    // 记录 trusted = 容差 false（安装期未豁免）。
    let mut lf2 = lf;
    if let Some(e) = lf2.plugins.get_mut("tolerated") {
        e.trust = PluginTrustState::Trusted.as_str().into();
    }
    std::fs::write(
        mgr.plugins_dir().join("lockfile.json"),
        serde_json::to_string_pretty(&lf2).expect("serialize"),
    )
    .expect("write");
    assert!(!inst.recorded_allow_unsigned("tolerated"));
}

// -- 病毒扫描跳过注记 ------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn scan_payload_skips_without_engine() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    // 无引擎 = 诚实跳过注记。
    let note = inst
        .scan_payload(b"bytes", "slug")
        .await
        .expect("skip note");
    assert!(note.expect("note").contains("病毒扫描跳过"));
}

// -- ensure_plain_filename（经 wasm 文件名面）-----------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn wasm_filename_forms() {
    let (_ws, mgr) = fresh();
    let inst = installer(&mgr);
    let mk = |dir: &std::path::Path, name: &str, body: &[u8]| {
        let sha = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(body))
        };
        let src = dir.join("src");
        std::fs::create_dir_all(&src).expect("mkdir");
        std::fs::write(
                src.join("plugin.toml"),
                format!(
                    "api-version = 1\nslug = \"fncheck\"\nkind = \"tool\"\nname = \"N\"\nversion = \"0.1.0\"\nwasm = \"{name}\"\nwasm-sha256 = \"{sha}\"\n"
                ),
            )
            .expect("write");
        std::fs::write(src.join(name), body).expect("write wasm");
        src
    };
    // 合法纯文件名过闸（sha 记账正确）——死在编译（垃圾字节）。
    let dir = tempfile::tempdir().expect("tmp");
    let src = mk(dir.path(), "custom.wasm", b"garbage wasm");
    let err = inst
        .install(&src, true)
        .await
        .err()
        .unwrap_or_else(|| panic!("compile"));
    assert!(matches!(err, PluginError::Compile(_)), "{err:?}");
    // C: 盘符形态的平台语义分叉（文件名纯度闸先于读盘）：Windows 下
    // `C:x.wasm` 是盘符相对 → 纯文件名闸拒；POSIX 下只是含冒号的合法
    // 文件名 → 直通读盘，死于编译（与 custom.wasm 同臂）。
    let dir2 = tempfile::tempdir().expect("tmp2");
    let src2 = mk(dir2.path(), "C:evil.wasm", b"garbage wasm");
    let err2 = inst
        .install(&src2, true)
        .await
        .err()
        .unwrap_or_else(|| panic!("drive-relative"));
    if cfg!(windows) {
        assert!(err2.to_string().contains("纯文件名"), "{err2:?}");
    } else {
        assert!(matches!(err2, PluginError::Compile(_)), "{err2:?}");
    }
}
