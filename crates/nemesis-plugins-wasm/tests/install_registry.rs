//! 2026-09-29 交付审查修复的回归钉（宿主 crate 公开面）：
//! - egress 私网闸边界（H1：v4-mapped / NAT64 归一 + v4 段表）；
//! - 开关记录真相源 `config.plugin.wasm.json`（3.2：幽灵/穿越拒绝 +
//!   config/enabled 独立读写 + 卸载摘除 + 损坏 fail-closed；2026-10-22
//!   「默认不加载」语义）；
//! - install 漏斗 wasm 载荷文件名穿越拒绝（L1）。
//!
//! ```text
//! cargo test -p nemesis-plugins-wasm --test install_registry
//! ```

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::Arc;

use nemesis_plugins_wasm::PluginError;
use nemesis_plugins_wasm::egress::is_private_ip;
use nemesis_plugins_wasm::host_impl::SecretResolver;
use nemesis_plugins_wasm::install::{AutoApprove, LockfileEntry, PluginInstaller, PluginLockfile};
use nemesis_plugins_wasm::limits::PluginLimits;
use nemesis_plugins_wasm::registry::PluginManager;

mod common;

struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

fn fresh_manager() -> (tempfile::TempDir, Arc<PluginManager>) {
    common::ensure_host_dll();
    let ws = tempfile::tempdir().expect("tempdir");
    // 测试形态 = 目录注入（生产 `new` 解析 exe 旁真实 plugins_wasm/，
    // 并行测试共享真实目录互相踩踏）。
    let mgr = Arc::new(
        PluginManager::new_with_dirs(
            ws.path(),
            &ws.path().join("plugins_wasm"),
            PluginLimits::default(),
            Arc::new(NoSecrets),
        )
        .expect("manager"),
    );
    (ws, mgr)
}

// ---------------------------------------------------------------------------
// H1：egress 私网闸边界（is_private_ip 段表逐项钉死）
// ---------------------------------------------------------------------------

#[test]
fn egress_private_ip_boundary() {
    let v4 = Ipv4Addr::new;
    let v6 = |a, b, c, d, e, f, g, h| IpAddr::V6(Ipv6Addr::new(a, b, c, d, e, f, g, h));

    // v4 常规段。
    for ip in [
        v4(127, 0, 0, 1),   // loopback
        v4(10, 0, 0, 5),    // 私网
        v4(192, 168, 1, 1), // 私网
        v4(172, 16, 3, 4),  // 私网
        v4(169, 254, 9, 9), // link-local
        v4(100, 64, 1, 1),  // CGNAT
        v4(0, 0, 0, 0),     // unspecified
        v4(0, 1, 2, 3),     // 0.0.0.0/8 「本网络」整段
        v4(192, 0, 2, 1),   // documentation
    ] {
        assert!(is_private_ip(IpAddr::V4(ip)), "{ip} must be private");
    }
    assert!(!is_private_ip(IpAddr::V4(v4(8, 8, 8, 8))));

    // H1 主案：IPv4-mapped 归一回 v4 判定（裸 v6 五项检查全不命中的绕过面）。
    let mapped = |o: [u8; 4]| {
        IpAddr::V6(Ipv6Addr::from_segments([
            0,
            0,
            0,
            0,
            0,
            0xffff,
            {
                let [a, b, _, _] = o;
                ((a as u16) << 8) | b as u16
            },
            {
                let [_, _, c, d] = o;
                ((c as u16) << 8) | d as u16
            },
        ]))
    };
    for o in [
        [127, 0, 0, 1],
        [10, 1, 2, 3],
        [169, 254, 1, 1],
        [0, 0, 0, 0],
    ] {
        let ip = mapped(o);
        assert!(is_private_ip(ip), "{ip} (v4-mapped) must be private");
    }
    // v4-mapped 公网地址不误伤。
    assert!(!is_private_ip(mapped([8, 8, 8, 8])));

    // NAT64 64:ff9b::/96 内嵌 IPv4 归一。
    let nat64 = |o: [u8; 4]| {
        let [a, b, c, d] = o;
        IpAddr::V6(Ipv6Addr::new(
            0x64,
            0xff9b,
            0,
            0,
            0,
            0,
            ((a as u16) << 8) | b as u16,
            ((c as u16) << 8) | d as u16,
        ))
    };
    assert!(is_private_ip(nat64([127, 0, 0, 1])), "NAT64 内嵌回环");
    assert!(is_private_ip(nat64([10, 0, 0, 1])), "NAT64 内嵌私网");
    assert!(!is_private_ip(nat64([93, 184, 216, 34])), "NAT64 内嵌公网");

    // 裸 v6 段表。
    assert!(is_private_ip(v6(0, 0, 0, 0, 0, 0, 0, 1)), "v6 loopback");
    assert!(is_private_ip(v6(0xfc00, 0, 0, 0, 0, 0, 0, 1)), "ULA");
    assert!(
        is_private_ip(v6(0xfd12, 0x3456, 0, 0, 0, 0, 0, 1)),
        "ULA fd"
    );
    assert!(is_private_ip(v6(0xfe80, 0, 0, 0, 0, 0, 0, 1)), "link-local");
    assert!(
        is_private_ip(v6(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1)),
        "documentation"
    );
    assert!(!is_private_ip(v6(
        0x2606, 0x4700, 0, 0, 0, 0, 0x6810, 0x80e4
    )));
}

// ---------------------------------------------------------------------------
// 3.2 / L6：开关记录写路径（config.plugin.wasm.json 单一真相源；2026-10-22
// 「默认不加载」语义——开关写经 installer.set_enabled，配置键经 manager）
// ---------------------------------------------------------------------------

fn seed_lockfile(mgr: &PluginManager, slug: &str) {
    let lf = PluginLockfile {
        version: 1,
        plugins: [(
            slug.to_string(),
            LockfileEntry {
                version: "0.1.0".into(),
                wasm_sha256: "a".repeat(64),
                trust: "review-required".into(),
                installed_at_ms: 0,
                signed_by: String::new(),
            },
        )]
        .into_iter()
        .collect(),
    };
    std::fs::create_dir_all(mgr.plugins_dir()).expect("plugins dir");
    std::fs::write(
        mgr.plugins_dir().join("lockfile.json"),
        serde_json::to_string_pretty(&lf).expect("serialize"),
    )
    .expect("write lockfile");
}

/// 最小合法载荷目录（plugin.toml 声明 config-schema.greeting；wasm 文件
/// 不需要——config 键校验只读 manifest 结构）。
fn seed_payload_dir(mgr: &PluginManager, slug: &str) {
    let pdir = mgr.plugin_dir(slug);
    std::fs::create_dir_all(&pdir).expect("plugin dir");
    let manifest = format!(
        "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"{slug}\"\nkind = \"tool\"\n\
         name = \"Demo\"\nversion = \"0.1.0\"\ndescription = \"demo\"\nwasm = \"{slug}.wasm\"\n\
         \n[config-schema]\ngreeting = \"string\"\n",
        "a".repeat(64)
    );
    std::fs::write(pdir.join("plugin.toml"), manifest).expect("write manifest");
}

#[tokio::test(flavor = "multi_thread")]
async fn set_enabled_ghost_and_traversal_rejected() {
    let (_ws, mgr) = fresh_manager();
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    let err = match installer.set_enabled("never-installed", true).await {
        Ok(_) => panic!("ghost slug must be rejected"),
        Err(e) => e,
    };
    assert!(matches!(err, PluginError::Manifest(_)), "got: {err:?}");

    let err = match installer.set_enabled("../escape", true).await {
        Ok(_) => panic!("invalid slug must be rejected"),
        Err(e) => e,
    };
    assert!(matches!(err, PluginError::Manifest(_)), "got: {err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn switch_records_config_and_enabled_independently() {
    let (_ws, mgr) = fresh_manager();
    seed_lockfile(&mgr, "demo");
    seed_payload_dir(&mgr, "demo");

    // 未加载也可设配置键（装载时生效——新语义核心）。
    mgr.set_config_key("demo", "greeting", "hi").expect("set");
    let sw = mgr.get_config("demo");
    assert!(!sw.enabled, "缺省关（新装/未开开关）");
    assert_eq!(
        sw.config.get("greeting"),
        Some(&serde_json::Value::String("hi".into())),
        "config 与 enabled 互不干扰"
    );

    // 开关记录（installer 路径会装配；此处只验真相源文件位——装配面归
    // e2e_guest）。直接走 config 层验证写读一致。
    let config_dir = mgr.workspace_root().join("config");
    nemesis_config::plugin_wasm::set_enabled(&config_dir, "demo", true).expect("enable");
    assert!(mgr.get_config("demo").enabled);
    assert_eq!(
        mgr.get_config("demo").config.get("greeting"),
        Some(&serde_json::Value::String("hi".into())),
        "开开关不清 config"
    );

    // 卸载摘除开关条目（uninstall 尾部语义）。
    nemesis_plugins_wasm::install::uninstall_blocking(&mgr, "demo").expect("uninstall");
    assert!(
        !mgr.get_config("demo").enabled,
        "卸载后开关条目已摘（缺省关）"
    );
    assert!(
        mgr.get_config("demo").config.is_empty(),
        "卸载后 config 一并摘除"
    );
    // 无残留 tmp（原子写收尾）。
    let leftovers: Vec<_> = std::fs::read_dir(&config_dir)
        .expect("read config dir")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "tmp 残留: {leftovers:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn corrupt_switch_file_fails_closed() {
    let (_ws, mgr) = fresh_manager();
    // 损坏开关文件 → 全部视为未启用（与 native 的 fail-open 相反——wasm
    // 一律默认不加载，损坏时从紧）。
    let config_dir = mgr.workspace_root().join("config");
    std::fs::create_dir_all(&config_dir).expect("mkdir");
    std::fs::write(config_dir.join("config.plugin.wasm.json"), "{not json").expect("seed corrupt");
    assert!(
        !mgr.get_config("anything").enabled,
        "损坏开关文件必须 fail-closed"
    );
    // 不存在 = 缺省关（「默认不加载」语义反转：与旧实例文件缺省启用相反）。
    assert!(!mgr.get_config("fresh").enabled);
}

#[test]
fn disk_manifest_rejects_traversal_slug_before_path_use() {
    // BUG-HUNT R37：读写对称性——set_enabled / set_config_key /
    // unset_config_key 都先 valid_slug 再碰路径，disk_manifest
    // （wasm.config.get 读路径）曾漏。闸必须在拼路径**前**生效：错误是
    // slug 拒绝而非文件系统读取错（后者回显解析后绝对路径 = 存在性
    // oracle）；`C:evil` 盘符形态同样拒绝（join 遇带 prefix 成分整体截断，
    // 读任意盘相对目录）。
    let (_ws, mgr) = fresh_manager();

    for slug in ["../escape", "a/b", "..\\escape", "C:evil"] {
        let err = match mgr.disk_manifest(slug) {
            Ok(_) => panic!("traversal slug '{slug}' must be rejected"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(msg.contains("invalid slug"), "slug '{slug}': {msg}");
        assert!(
            !msg.contains("read "),
            "闸必须先于路径使用（错误不得含文件系统读取回显）: {msg}"
        );
    }

    // 合法 slug 不受影响：已装插件照常读盘（控制组）。
    seed_payload_dir(&mgr, "demo");
    let m = mgr.disk_manifest("demo").expect("valid installed slug");
    assert_eq!(m.slug, "demo");

    // 合法但未装：错误回落为文件系统读取错（到达路径层，非 slug 拒绝）。
    let err = mgr.disk_manifest("not-installed").expect_err("absent");
    assert!(err.to_string().contains("read "), "got: {err}");
}

// ---------------------------------------------------------------------------
// L1：install 漏斗 wasm 载荷文件名穿越拒绝
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn install_rejects_traversal_wasm_filename() {
    let (_ws, mgr) = fresh_manager();
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).expect("stage dir");
    let manifest = format!(
        "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"traversal\"\nkind = \"tool\"\n\
         name = \"Traversal\"\nversion = \"0.1.0\"\ndescription = \"probe\"\nwasm = \"../evil.wasm\"\n",
        "b".repeat(64)
    );
    std::fs::write(src.join("plugin.toml"), manifest).expect("write manifest");

    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    // Ok 侧类型（Arc<RegisteredPlugin>）无 Debug——match 拆而非 expect_err。
    let err = match installer.install(Path::new(&src), true).await {
        Ok(_) => panic!("traversal filename must be rejected"),
        Err(e) => e,
    };
    assert!(
        matches!(err, PluginError::Wasm(ref m) if m.contains("纯文件名")),
        "got: {err:?}"
    );
    assert!(mgr.get("traversal").is_none(), "被拒插件不得进入注册表");
}

// ---------------------------------------------------------------------------
// 2026-10-23 开关补漏斗收口：手工拷入无签名载荷的容忍判定
//（set_enabled 补漏斗路径；install_registry 钉从紧面，卡面救援全链归
// e2e_guest::hand_copied_unsigned_rescued_by_card_approval）
// ---------------------------------------------------------------------------

/// 无卡审批面（AutoApprove = CLI --yes / headless）+ 手工拷入无签名载荷：
/// 开关补漏斗维持从紧——trust 闸（漏斗②）在 wasm sha 重算（③）前短路，
/// 错误文案可行动（指向卡面 / `--allow-unsigned` / `plugin sign`）。
/// 回归锁：用户点开关弹「trust check failed: unsigned manifest and
/// allow_unsigned=false (strict)」事故——文案已改可行动，CLI 纪律不回退。
#[tokio::test(flavor = "multi_thread")]
async fn hand_copied_unsigned_without_card_stays_strict() {
    let (_ws, mgr) = fresh_manager();
    seed_payload_dir(&mgr, "stray"); // 无签名 manifest（trust 闸先于 wasm 读取）
    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    // Ok 侧类型无 Debug——match 拆。
    let err = match installer.set_enabled("stray", true).await {
        Ok(_) => panic!("unsigned hand-copied must stay strict without card"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("allow-unsigned"),
        "错误文案必须带可行动指引: {msg}"
    );
    // 开关先落盘语义不变：load 失败开关保持 true = 重启重试的诚实可见态。
    assert!(
        mgr.get_config("stray").enabled,
        "开关落盘在前（失败保持 true）"
    );
    assert!(mgr.get("stray").is_none(), "被拒插件不得进入注册表");
}

// ---------------------------------------------------------------------------
// 2026-10-23 工具接管层（接口可插拔托管）：manifest 声明校验 + deny-list +
// registry 接管裁决
// ---------------------------------------------------------------------------

use nemesis_plugins_wasm::manifest::{self, PluginManifest};

fn parse_manifest_with_takeover(kind: &str, takeover: &[&str]) -> Result<(), String> {
    let line = if takeover.is_empty() {
        String::new()
    } else {
        format!(
            "takeover = [{}]\n",
            takeover
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let raw = format!(
        "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"demo\"\nkind = \"{kind}\"\n\
         name = \"Demo\"\nversion = \"0.1.0\"\ndescription = \"demo\"\n{line}",
        "a".repeat(64)
    );
    match PluginManifest::parse(&raw) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[test]
fn takeover_manifest_validation_matrix() {
    // tool + 合法目标 → 过。
    assert!(parse_manifest_with_takeover("tool", &["web_fetch"]).is_ok());
    // 无声明 → 过（旧 manifest 兼容）。
    assert!(parse_manifest_with_takeover("tool", &[]).is_ok());
    // deny-list（宿主特权语义）→ loud 拒绝。
    let err = parse_manifest_with_takeover("tool", &["exec"]).unwrap_err();
    assert!(err.contains("denied"), "{err}");
    // plugin. 前缀（命名空间隔离不可破）→ 拒绝。
    let err = parse_manifest_with_takeover("tool", &["plugin.x.y"]).unwrap_err();
    assert!(err.contains("plugin namespace"), "{err}");
    // 非 tool kind 带声明 → 拒绝。
    let err = parse_manifest_with_takeover("observer", &["web_fetch"]).unwrap_err();
    assert!(err.contains("kind=tool"), "{err}");
    let err = parse_manifest_with_takeover("policy", &["web_fetch"]).unwrap_err();
    assert!(err.contains("kind=tool"), "{err}");
    // 空目标 → 拒绝。
    let err = parse_manifest_with_takeover("tool", &[""]).unwrap_err();
    assert!(err.contains("takeover target"), "{err}");
}

#[test]
fn takeover_deny_list_covers_host_privileged_family() {
    // 执行体分离高危族 + 进程/注册表/系统面全被钉。
    for t in manifest::TAKEOVER_DENIED_TOOLS {
        assert!(manifest::takeover_denied(t), "{t} 应在 deny-list");
    }
    assert!(manifest::takeover_denied("write_file"));
    assert!(manifest::takeover_denied("process_spawn"));
    // 沙盒可诚实兑现的信息面不在 deny。
    assert!(!manifest::takeover_denied("web_fetch"));
    assert!(!manifest::takeover_denied("sleep"));
    assert!(!manifest::takeover_denied("memory_search"));
}

#[test]
fn takeover_owner_resolves_enabled_tool_plugin_only() {
    use nemesis_plugins_wasm::PluginTrustState;
    use nemesis_plugins_wasm::registry::ToolMetaSnapshot;

    let (_ws, mgr) = fresh_manager();
    // 注册前：无接管者。
    assert!(mgr.takeover_owner("web_fetch").is_none());
    // 带 takeover 声明的 tool 插件（直注册——owner 裁决只看注册表面，无需
    // 真组件句柄；执行语义归 e2e 层）。
    let raw = format!(
        "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"fetcher\"\nkind = \"tool\"\n\
         name = \"Fetcher\"\nversion = \"0.1.0\"\ndescription = \"d\"\ntakeover = [\"web_fetch\"]\n",
        "a".repeat(64)
    );
    let manifest = PluginManifest::parse(&raw).expect("parse").0;
    let meta = ToolMetaSnapshot {
        name: "plugin.fetcher.fetch".into(),
        base: "fetch".into(),
        title: "Fetch".into(),
        description: "d".into(),
        parameters_json: "{}".into(),
        operation_type: "network".into(),
        min_tier: "big".into(),
    };
    let reg = mgr
        .register(
            manifest,
            PluginTrustState::ReviewRequired,
            1,
            Some(meta),
            None,
        )
        .expect("register");
    // 注册面缺省启用（开关记录的「默认不加载」由 installer 装载层裁决，
    // 直注册 = 已在场启用）→ 接管者裁决命中。
    let owner = mgr.takeover_owner("web_fetch").expect("owner");
    assert_eq!(owner.manifest.slug, "fetcher");
    // 开关关 → 无接管者（开关裁决策）。
    reg.enabled
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(mgr.takeover_owner("web_fetch").is_none(), "开关关 = 无接管");
    // 非声明名 → None。
    assert!(mgr.takeover_owner("sleep").is_none());
}

#[test]
fn takeover_owner_multi_declarer_arbitration_is_deterministic() {
    use nemesis_plugins_wasm::PluginTrustState;
    use nemesis_plugins_wasm::registry::ToolMetaSnapshot;

    // 注册表是无序 HashMap：多声明仲裁必须确定性（slug 字典序最小者胜），
    // 不允许「先注册者胜」这类注册表根本无法兑现的语义。
    let (_ws, mgr) = fresh_manager();
    let register_tool = |mgr: &Arc<PluginManager>, slug: &str| {
        let raw = format!(
            "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"{slug}\"\nkind = \"tool\"\n\
             name = \"{slug}\"\nversion = \"0.1.0\"\ndescription = \"d\"\ntakeover = [\"web_fetch\"]\n",
            "a".repeat(64)
        );
        let manifest = PluginManifest::parse(&raw).expect("parse").0;
        let meta = ToolMetaSnapshot {
            name: format!("plugin.{slug}.fetch"),
            base: "fetch".into(),
            title: slug.into(),
            description: "d".into(),
            parameters_json: "{}".into(),
            operation_type: "network".into(),
            min_tier: "big".into(),
        };
        mgr.register(
            manifest,
            PluginTrustState::ReviewRequired,
            1,
            Some(meta),
            None,
        )
        .expect("register")
    };
    // 先注册字典序大的 zeta，后注册 alpha——胜者恒为 alpha。
    let zeta = register_tool(&mgr, "zeta");
    let alpha = register_tool(&mgr, "alpha");
    for _ in 0..8 {
        let owner = mgr.takeover_owner("web_fetch").expect("owner");
        assert_eq!(
            owner.manifest.slug, "alpha",
            "多声明仲裁必须 slug 字典序最小者胜（HashMap 无序，重跑不得漂移）"
        );
    }
    // 胜者开关关 → 落到唯一剩余候选 zeta（仲裁与开关位正交）。
    alpha
        .enabled
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let owner = mgr.takeover_owner("web_fetch").expect("owner");
    assert_eq!(owner.manifest.slug, "zeta");
    // 两个全关 → None。
    zeta.enabled
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(mgr.takeover_owner("web_fetch").is_none());
}

// ---------------------------------------------------------------------------
// 2026-10-31 面板扩展批：manifest 声明校验（kind 门 + 视图词汇/绑定形态 +
// 剧本步骤词表/on_fail 分支 + 纯声明式插件最低声明面）
// ---------------------------------------------------------------------------

const PANEL_HEAD: &str = "api-version = 1\nslug = \"demo-panel\"\nkind = \"panel\"\n\
     name = \"Demo Panel\"\nversion = \"0.1.0\"\ndescription = \"demo\"\n";

fn parse_raw(raw: &str) -> Result<(), String> {
    match PluginManifest::parse(raw) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// 一页一 table 视图的最小合法页面段。
fn one_page() -> String {
    "[[ui.page]]\nroute = \"main\"\ntitle = \"主页\"\n\n\
     [[ui.page.views]]\nkind = \"table\"\nbind = \"cmd://list\"\n\n\
     [[ui.page.views.columns]]\nkey = \"id\"\nlabel = \"ID\"\n"
        .to_string()
}

#[test]
fn panel_manifest_validation_matrix() {
    // 纯声明式（无载荷）+ 页面 → 过。
    let raw = format!("{PANEL_HEAD}{}", one_page());
    assert!(parse_raw(&raw).is_ok(), "declarative panel must parse");

    // 纯声明式但零页面 → 拒绝（无载荷无声明 = 什么都没装）。
    let err = parse_raw(PANEL_HEAD).unwrap_err();
    assert!(err.contains("must declare at least one"), "{err}");

    // 有载荷面板零页面 → 过（guest 命令通道本体合法，页面可后续升级补）。
    let raw = format!("wasm-sha256 = \"{}\"\n{}", "a".repeat(64), PANEL_HEAD);
    assert!(
        parse_raw(&raw).is_ok(),
        "payload panel without pages is legal"
    );

    // 非 panel kind 携声明面 → 拒绝（扩展点不混装，与 takeover 同纪律）。
    let raw = format!(
        "wasm-sha256 = \"{}\"\n{}{}",
        "a".repeat(64),
        PANEL_HEAD,
        one_page()
    )
    .replacen("kind = \"panel\"", "kind = \"tool\"", 1);
    let err = parse_raw(&raw).unwrap_err();
    assert!(err.contains("requires kind=panel"), "{err}");

    let flow_seg = "[[flow.template]]\nid = \"fix\"\n\n\
                    [[flow.template.steps]]\nid = \"s1\"\nkind = \"connector_fetch\"\n";
    let raw = format!("wasm-sha256 = \"{}\"\n{}", "a".repeat(64), PANEL_HEAD).replacen(
        "kind = \"panel\"",
        "kind = \"observer\"",
        1,
    ) + flow_seg;
    let err = parse_raw(&raw).unwrap_err();
    assert!(err.contains("requires kind=panel"), "{err}");

    // 路由形态 + 跨页唯一。
    let bad_route = one_page().replacen("route = \"main\"", "route = \"Bad_Route\"", 1);
    let err = parse_raw(&format!("{PANEL_HEAD}{bad_route}")).unwrap_err();
    assert!(err.contains("invalid ui.page.route"), "{err}");
    let dup = format!("{PANEL_HEAD}{}{}", one_page(), one_page());
    let err = parse_raw(&dup).unwrap_err();
    assert!(err.contains("duplicate ui.page.route"), "{err}");
}

#[test]
fn panel_view_and_bind_validation_matrix() {
    let page_wrap =
        |views: &str| format!("{PANEL_HEAD}[[ui.page]]\nroute = \"m\"\ntitle = \"M\"\n{views}");

    // 视图词表外 kind → 拒绝。
    let err = parse_raw(&page_wrap(
        "[[ui.page.views]]\nkind = \"carousel\"\nbind = \"cmd://x\"\n",
    ))
    .unwrap_err();
    assert!(err.contains("invalid ui view kind"), "{err}");

    // bind 前缀缺失 → 拒绝。
    let err = parse_raw(&page_wrap(
        "[[ui.page.views]]\nkind = \"timeline\"\nbind = \"list-all\"\n",
    ))
    .unwrap_err();
    assert!(err.contains("flow:// or cmd://"), "{err}");

    // flow:// 单段 → 拒绝（缺 view 段）。
    let err = parse_raw(&page_wrap(
        "[[ui.page.views]]\nkind = \"timeline\"\nbind = \"flow://only-template\"\n",
    ))
    .unwrap_err();
    assert!(err.contains("flow bind"), "{err}");

    // cmd:// 空与带空白 → 拒绝。
    let err = parse_raw(&page_wrap(
        "[[ui.page.views]]\nkind = \"timeline\"\nbind = \"cmd://\"\n",
    ))
    .unwrap_err();
    assert!(err.contains("cmd bind"), "{err}");
    let err = parse_raw(&page_wrap(
        "[[ui.page.views]]\nkind = \"timeline\"\nbind = \"cmd://a b\"\n",
    ))
    .unwrap_err();
    assert!(err.contains("cmd bind"), "{err}");

    // table 缺列 → 拒绝。
    let err = parse_raw(&page_wrap(
        "[[ui.page.views]]\nkind = \"table\"\nbind = \"cmd://list\"\n",
    ))
    .unwrap_err();
    assert!(err.contains("needs at least one column"), "{err}");

    // form 字段类型出词表 → 拒绝；secret 合法。
    let err = parse_raw(&page_wrap(
        "[[ui.page.views]]\nkind = \"form\"\nbind = \"cmd://save\"\n\n\
         [[ui.page.views.fields]]\nkey = \"k\"\nlabel = \"K\"\nfield-type = \"blob\"\n",
    ))
    .unwrap_err();
    assert!(err.contains("invalid field type"), "{err}");
    assert!(
        parse_raw(&page_wrap(
            "[[ui.page.views]]\nkind = \"form\"\nbind = \"cmd://save\"\n\n\
         [[ui.page.views.fields]]\nkey = \"k\"\nlabel = \"K\"\nfield-type = \"secret\"\n"
        ))
        .is_ok()
    );

    // actions 空 bind 合法（语义在 per-button command；视图级 bind 无消费者），
    // 但缺按钮仍拒绝。
    assert!(
        parse_raw(&page_wrap(
            "[[ui.page.views]]\nkind = \"actions\"\nbind = \"\"\n\n\
         [[ui.page.views.buttons]]\nlabel = \"跑\"\ncommand = \"go\"\n"
        ))
        .is_ok()
    );
    let err = parse_raw(&page_wrap(
        "[[ui.page.views]]\nkind = \"actions\"\nbind = \"\"\n",
    ))
    .unwrap_err();
    assert!(err.contains("needs at least one button"), "{err}");

    // timeline 无专用段 → 直通。
    assert!(
        parse_raw(&page_wrap(
            "[[ui.page.views]]\nkind = \"timeline\"\nbind = \"cmd://events\"\n"
        ))
        .is_ok()
    );
}

#[test]
fn panel_flow_template_validation_matrix() {
    // 带 wasm-sha256（有载荷面板零页面合法；纯声明式必须带页——flow 矩阵
    // 不关心页面，走有载荷形态最省）。
    let flow_wrap = |steps: &str| {
        format!(
            "wasm-sha256 = \"{}\"\n{PANEL_HEAD}[[flow.template]]\nid = \"fix\"\n{}",
            "a".repeat(64),
            steps
        )
    };
    let step = |extra: &str| {
        format!("\n[[flow.template.steps]]\nid = \"s1\"\nkind = \"connector_fetch\"\n{extra}")
    };

    // 步骤词表外 kind → 拒绝。
    assert!(
        parse_raw(&flow_wrap(&step(""))).is_ok(),
        "legal step must pass"
    );
    let raw = flow_wrap(&step("")).replacen("kind = \"connector_fetch\"", "kind = \"magic\"", 1);
    let err = parse_raw(&raw).unwrap_err();
    assert!(err.contains("invalid kind"), "{err}");

    // gate 出词表 → 拒绝（空|manual 之外不收）。
    let raw = flow_wrap(&step("gate = \"auto\""));
    let err = parse_raw(&raw).unwrap_err();
    assert!(err.contains("invalid gate"), "{err}");
    assert!(parse_raw(&flow_wrap(&step("gate = \"manual\""))).is_ok());

    // on_fail 分支形态。
    for ok in [
        "",
        "on-fail = \"park_human\"\n",
        "on-fail = \"continue\"\n",
        "on-fail = \"retry:3\"\n",
    ] {
        assert!(
            parse_raw(&flow_wrap(&step(ok))).is_ok(),
            "on_fail '{ok}' must pass"
        );
    }
    for bad in [
        "on-fail = \"retry:0\"\n",
        "on-fail = \"retry:11\"\n",
        "on-fail = \"giveup\"\n",
    ] {
        let err = parse_raw(&flow_wrap(&step(bad))).unwrap_err();
        assert!(err.contains("invalid on_fail"), "{bad}: {err}");
    }

    // 模板/步骤 id 唯一。
    let dup_tpl = format!(
        "wasm-sha256 = \"{}\"\n{PANEL_HEAD}[[flow.template]]\nid = \"fix\"\n\n\
         [[flow.template.steps]]\nid = \"s1\"\nkind = \"connector_fetch\"\n\n\
         [[flow.template]]\nid = \"fix\"\n\n\
         [[flow.template.steps]]\nid = \"s1\"\nkind = \"connector_fetch\"\n",
        "a".repeat(64)
    );
    let err = parse_raw(&dup_tpl).unwrap_err();
    assert!(err.contains("duplicate flow.template.id"), "{err}");
    let dup_step = flow_wrap(
        "\n[[flow.template.steps]]\nid = \"s1\"\nkind = \"connector_fetch\"\n\n\
         [[flow.template.steps]]\nid = \"s1\"\nkind = \"connector_fetch\"\n",
    );
    let err = parse_raw(&dup_step).unwrap_err();
    assert!(err.contains("duplicate step id"), "{err}");

    // 零步骤模板 → 拒绝。
    let raw = format!(
        "wasm-sha256 = \"{}\"\n{PANEL_HEAD}[[flow.template]]\nid = \"empty\"\n",
        "a".repeat(64)
    );
    let err = parse_raw(&raw).unwrap_err();
    assert!(err.contains("needs at least one step"), "{err}");
}

// ---------------------------------------------------------------------------
// 2026-10-31 面板扩展批：面板注册面（声明式载荷诚实报错 + 路由跨插件唯一）
// ---------------------------------------------------------------------------

/// 面板 manifest 组装器（actions 视图 = 声明面最小合法形态）。
fn panel_manifest_raw(slug: &str, route: &str, payload: bool) -> String {
    let head = if payload {
        format!("wasm-sha256 = \"{}\"\n", "a".repeat(64))
    } else {
        String::new()
    };
    format!(
        "{head}api-version = 1\nslug = \"{slug}\"\nkind = \"panel\"\n\
         name = \"{slug}\"\nversion = \"0.1.0\"\ndescription = \"d\"\n\n\
         [[ui.page]]\nroute = \"{route}\"\ntitle = \"T\"\n\n\
         [[ui.page.views]]\nkind = \"actions\"\nbind = \"\"\n\n\
         [[ui.page.views.buttons]]\nlabel = \"跑\"\ncommand = \"go\"\n"
    )
}

#[tokio::test]
async fn panel_declarative_register_and_invoke_honest_error() {
    use nemesis_plugins_wasm::PluginTrustState;

    let (_ws, mgr) = fresh_manager();
    let manifest = PluginManifest::parse(&panel_manifest_raw("decl-panel", "main", false))
        .expect("parse")
        .0;
    // 声明式面板：component=0 哨兵直注册（无宿主壳依赖，纯注册面测试）。
    let reg = mgr
        .register(manifest, PluginTrustState::ReviewRequired, 0, None, None)
        .expect("register");
    assert_eq!(reg.manifest.kind.as_str(), "panel");
    assert_eq!(reg.component, 0);
    // 声明式 invoke → 诚实报错（无 guest 载荷，cmd:// 绑定不可用）。
    let err = mgr
        .invoke_ui("decl-panel", r#"{"command":"go"}"#.to_string())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("is declarative"), "{err}");
    // 未加载 slug → 诚实报错。
    let err = mgr.invoke_ui("absent", "{}".to_string()).await.unwrap_err();
    assert!(err.to_string().contains("not loaded"), "{err}");
}

#[test]
fn panel_route_collision_rejected_across_plugins() {
    use nemesis_plugins_wasm::PluginTrustState;

    let (_ws, mgr) = fresh_manager();
    let a = PluginManifest::parse(&panel_manifest_raw("panel-a", "main", false))
        .expect("parse")
        .0;
    mgr.register(a, PluginTrustState::ReviewRequired, 0, None, None)
        .expect("first register");
    // 同路由第二家 → 拒绝（/panel/<route> 动态挂载键全局唯一）。
    let b = PluginManifest::parse(&panel_manifest_raw("panel-b", "main", false))
        .expect("parse")
        .0;
    let err = mgr
        .register(b, PluginTrustState::ReviewRequired, 0, None, None)
        .err()
        .expect("duplicate route must be rejected");
    assert!(
        err.to_string().contains("already declared by plugin"),
        "{err}"
    );
    // 换路由 → 过（唯一性只锁路由，不锁面板插件数）。
    let c = PluginManifest::parse(&panel_manifest_raw("panel-b", "other", false))
        .expect("parse")
        .0;
    mgr.register(c, PluginTrustState::ReviewRequired, 0, None, None)
        .expect("other route ok");
}
