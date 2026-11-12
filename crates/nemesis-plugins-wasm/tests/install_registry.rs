//! 2026-09-29 交付审查修复的回归钉（宿主 crate 公开面）：
//! - egress 私网闸边界（v4-mapped / NAT64 归一 + v4 段表）；
//! - 开关记录真相源 `config.plugin.wasm.json`（3.2：幽灵/穿越拒绝 +
//!   config/enabled 独立读写 + 卸载摘除 + 损坏 fail-closed；2026-10-22
//!   「默认不加载」语义）；
//! - install 漏斗 wasm 载荷文件名穿越拒绝。
//!
//! ```text
//! cargo test -p nemesis-plugins-wasm --test install_registry
//! ```

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::Arc;

use nemesis_plugins_wasm::PluginError;
use nemesis_plugins_wasm::egress::{EgressPolicy, EgressRequest, is_private_ip, proxy_request};
use nemesis_plugins_wasm::host_impl::SecretResolver;
use nemesis_plugins_wasm::install::{AutoApprove, LockfileEntry, PluginInstaller, PluginLockfile};
use nemesis_plugins_wasm::limits::PluginLimits;
use nemesis_plugins_wasm::manifest::ConfigSchemaEntry;
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
// egress 私网闸边界（is_private_ip 段表逐项钉死）
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

    // 主案：IPv4-mapped 归一回 v4 判定（裸 v6 五项检查全不命中的绕过面）。
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

    // NAT64 64:ff9b:/96 内嵌 IPv4 归一。
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
// 出站代理臂（SC 套件批）：proxy 配置在场 = DNS 解析/私网闸/
// 钉地址整体移交代理侧——无真实代理也能行为区分（死代理端口秒级拒绝 vs
// 直连路径先走 DNS）。`.invalid` TLD 是 RFC 6761 保留域（名字服务器应
// NXDOMAIN），直连臂必然落「dns resolution failed」。
// ---------------------------------------------------------------------------

fn egress_request(host: &str) -> EgressRequest {
    EgressRequest {
        method: "GET".into(),
        url: format!("http://{host}/x"),
        headers: vec![],
        body: None,
    }
}

#[tokio::test]
async fn egress_proxy_arm_skips_local_dns() {
    let host = "sc-proxy-test.invalid";
    let limits = PluginLimits {
        egress_timeout_ms: 3_000,
        ..PluginLimits::default()
    };

    // 直连臂（None）：不可解析 host → 本机 DNS 失败（现行为锚点）。
    let direct = EgressPolicy {
        ..EgressPolicy::from_permissions(vec![host.into()], true)
    };
    let err = proxy_request(&direct, &limits, egress_request(host))
        .await
        .expect_err("直连臂必须失败（.invalid 不可解析）");
    assert!(
        err.contains("dns resolution failed"),
        "直连臂错误必须来自本机 DNS 解析：{err}"
    );

    // 代理臂：死代理 127.0.0.1:1（TCP 拒绝秒级）——错误必须来自连接代理
    // 而非本机 DNS（DNS 已移交代理侧，socks5h 语义成立）。
    let via_proxy = EgressPolicy {
        proxy: Some("http://127.0.0.1:1".into()),
        ..EgressPolicy::from_permissions(vec![host.into()], true)
    };
    let err = proxy_request(&via_proxy, &limits, egress_request(host))
        .await
        .expect_err("死代理臂必须失败（连接拒绝）");
    assert!(
        !err.contains("dns resolution failed"),
        "代理臂不得做本机 DNS 解析：{err}"
    );
    assert!(
        err.contains("request failed"),
        "代理臂错误必须来自代理连接失败：{err}"
    );

    // 畸形代理 URL：构造 client 之前诚实报错（reqwest 的 Proxy::all 只在
    // URL 解析层即时拒绝——残缺 IPv6 形态；未知 scheme 会推迟到请求期才报）。
    let bad_proxy = EgressPolicy {
        proxy: Some("http://[::1".into()),
        ..EgressPolicy::from_permissions(vec![host.into()], true)
    };
    let err = proxy_request(&bad_proxy, &limits, egress_request(host))
        .await
        .expect_err("畸形代理 URL 必须失败");
    assert!(
        err.contains("proxy url invalid"),
        "畸形代理 URL 错误必须带 proxy url invalid：{err}"
    );
}

// egress allowlist IPv6 条目形态（钉端口此前永不命中）
// ---------------------------------------------------------------------------

#[test]
fn egress_allowlist_ipv6_entry_forms() {
    use nemesis_plugins_wasm::egress::{EgressPolicy, allowlisted};

    // bracketed + 钉端口：端口精确命中 / 错端口不命中 / host 不同不命中。
    let pol = EgressPolicy::from_permissions(vec!["[2001:db8::5]:443".into()], false);
    assert!(
        allowlisted(&pol, "2001:db8::5", 443),
        "bracketed 钉端口命中"
    );
    assert!(
        !allowlisted(&pol, "2001:db8::5", 8443),
        "钉端口条目错端口不命中"
    );
    assert!(!allowlisted(&pol, "2001:db8::6", 443), "host 不同不命中");

    // bracketed 无端口：任意端口。
    let pol = EgressPolicy::from_permissions(vec!["[::1]".into()], true);
    assert!(allowlisted(&pol, "::1", 80));
    assert!(allowlisted(&pol, "::1", 8080));

    // 裸 IPv6 字面量（含冒号整串）：任意端口。
    let pol = EgressPolicy::from_permissions(vec!["2001:db8::9".into()], false);
    assert!(allowlisted(&pol, "2001:db8::9", 80));
    assert!(allowlisted(&pol, "2001:db8::9", 443));
    assert!(
        !allowlisted(&pol, "2001:db8::10", 80),
        "裸地址 host 不同不命中"
    );
    // `:1:8080` 整串是合法 IPv6 地址（末组 hex 8080）——按裸地址语义
    // 任意端口，钉端口必须走 bracketed 形态。
    let pol = EgressPolicy::from_permissions(vec!["::1:8080".into()], true);
    assert!(allowlisted(&pol, "::1:8080", 80));
    assert!(!allowlisted(&pol, "::1", 8080));

    // 域名形态不回归（v4/域名钉端口 + 裸域名任意端口）。
    let pol = EgressPolicy::from_permissions(
        vec!["api.example.com".into(), "pin.example.com:8443".into()],
        false,
    );
    assert!(allowlisted(&pol, "api.example.com", 80));
    assert!(allowlisted(&pol, "api.example.com", 443));
    assert!(allowlisted(&pol, "pin.example.com", 8443));
    assert!(
        !allowlisted(&pol, "pin.example.com", 443),
        "域名钉端口错端口不命中"
    );
    assert!(!allowlisted(&pol, "other.example.com", 80));

    // 大小写不敏感（条目 from_permissions 已归一小写，host 侧直给大写）。
    let pol = EgressPolicy::from_permissions(vec!["[2001:DB8::5]:443".into()], false);
    assert!(allowlisted(&pol, "2001:db8::5", 443));

    // 畸形条目不 panic、不误命中（>u16 端口不命中且不短路后续条目）。
    let pol = EgressPolicy::from_permissions(
        vec!["pin.example.com:99999".into(), "pin.example.com".into()],
        false,
    );
    assert!(
        allowlisted(&pol, "pin.example.com", 80),
        "超界端口条目不短路后续裸条目"
    );
}

// ---------------------------------------------------------------------------
// 3.2 /：开关记录写路径（config.plugin.wasm.json 单一权威定义；
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
                connectors: Vec::new(),
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
    // 读写对称性——set_enabled / set_config_key /
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
// install 漏斗 wasm 载荷文件名穿越拒绝
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
// 开关补漏斗收口：手工拷入无签名载荷的容忍判定
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
// 工具接管层（接口可插拔托管）：manifest 声明校验 + deny-list +
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
fn config_schema_entry_two_forms_and_defaults() {
    // 旧形态：纯描述字符串（零迁移，flow 剧本不得引用其缺省——无缺省可渲染）。
    let raw = format!(
        "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"cfg-a\"\nkind = \"tool\"\n\
         name = \"A\"\nversion = \"0.1.0\"\ndescription = \"d\"\n\
         \n[config-schema]\ngreeting = \"打招呼对象\"\n",
        "a".repeat(64)
    );
    let m = PluginManifest::parse(&raw).expect("parse").0;
    assert_eq!(m.config_schema["greeting"].desc(), "打招呼对象");
    assert_eq!(m.config_schema_defaults().get("greeting"), None);

    // 新形态：描述 + 缺省值（flow {{config.*}} 装载期渲染的供给面）。
    let raw = format!(
        "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"cfg-b\"\nkind = \"tool\"\n\
         name = \"B\"\nversion = \"0.1.0\"\ndescription = \"d\"\n\
         \n[config-schema]\nowner = {{ desc = \"属主\", default = \"octocat\" }}\n\
         empty = {{ desc = \"声明无缺省\" }}\n",
        "b".repeat(64)
    );
    let m = PluginManifest::parse(&raw).expect("parse").0;
    assert_eq!(m.config_schema["owner"].desc(), "属主");
    assert_eq!(
        m.config_schema_defaults().get("owner").map(String::as_str),
        Some("octocat")
    );
    assert_eq!(
        m.config_schema_defaults().get("empty"),
        None,
        "空缺省不入缺省表（渲染面 = 无缺省）"
    );
    // detail 面（wasm.config.get schema 投影源）。
    let detail = m.config_schema_detail();
    assert!(detail.contains(&(
        "owner".to_string(),
        "属主".to_string(),
        Some("octocat".to_string())
    )));
    assert!(detail.contains(&("empty".to_string(), "声明无缺省".to_string(), None)));

    // x 前缀键照拒（保留命名空间 = 凭据面；明文缺省值冒充凭据声明 loud 拒）。
    let raw = format!(
        "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"cfg-c\"\nkind = \"tool\"\n\
         name = \"C\"\nversion = \"0.1.0\"\ndescription = \"d\"\n\
         \n[config-schema]\nowner = {{ desc = \"属主\", default = \"o\" }}\n\
         x-token = {{ desc = \"冒充凭据\", default = \"t\" }}\n",
        "c".repeat(64)
    );
    match PluginManifest::parse(&raw) {
        Err(e) => assert!(e.to_string().contains("reserved"), "{e}"),
        Ok((m, _)) => {
            let err = m.validate().expect_err("x- 前缀 config-schema 键必须被拒");
            assert!(err.to_string().contains("reserved"), "{err}");
        }
    }
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
// 面板扩展批：manifest 声明校验（kind 门 + 视图词汇/绑定形态 +
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
// 决策 C：面板 LLM 总闸——manifest `x-llm` 声明位校验矩阵。
// （授予裁决本体在 registry::build_frame，端到端归 e2e_guest::panel_llm_dual_gate。）
// ---------------------------------------------------------------------------

#[test]
fn x_llm_manifest_validation_matrix() {
    // panel + x-llm 声明 → 解析过且落字段。
    let raw = format!(
        "wasm-sha256 = \"{}\"\n{PANEL_HEAD}\
         [permissions]\nx-llm = true\n\n{}",
        "a".repeat(64),
        one_page()
    );
    let m = PluginManifest::parse(&raw)
        .expect("panel x-llm must parse")
        .0;
    assert!(m.permissions.x_llm, "x-llm = true must land on x_llm");

    // panel + 显式 false → 过（等价未声明）。
    let raw = raw.replace("x-llm = true", "x-llm = false");
    let m = PluginManifest::parse(&raw)
        .expect("panel x-llm=false must parse")
        .0;
    assert!(!m.permissions.x_llm);

    // 非 panel kind 声明 → loud 拒（tool/observer 帧无 llm 面；policy 帧
    // llm 走 kind 硬规则，声明 = 死键 = manifest 写错信号）。
    for kind in ["tool", "observer", "policy"] {
        let raw = format!(
            "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"demo\"\nkind = \"{kind}\"\n\
             name = \"Demo\"\nversion = \"0.1.0\"\ndescription = \"d\"\n\
             [permissions]\nx-llm = true\n",
            "a".repeat(64)
        );
        let err = parse_raw(&raw).unwrap_err();
        assert!(
            err.contains("requires kind=panel"),
            "kind={kind} declaring x-llm must be rejected: {err}"
        );
    }

    // 缺省（无 permissions 段）→ x_llm = false（老 manifest 零迁移）。
    // 带载荷形态（纯声明式零页面在页数闸就被拒，到不了本断言）。
    let raw = format!("wasm-sha256 = \"{}\"\n{}", "a".repeat(64), PANEL_HEAD);
    let m = PluginManifest::parse(&raw).expect("parse").0;
    assert!(!m.permissions.x_llm, "absent key defaults false");
}

// ---------------------------------------------------------------------------
// 面板扩展批：面板注册面（声明式载荷诚实报错 + 路由跨插件唯一）
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
        .register(
            manifest,
            PluginTrustState::ReviewRequired,
            0,
            None,
            None,
            None,
        )
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

// ---------------------------------------------------------------------------
// 运行期总闸热翻（BUG-20261108-002）：execute_tool / invoke_ui manager 入口
// 自闸——config 翻关后六个 guest 执行入口必须全热停，面板通道此前漏接
//（其余五口已有闸），刷新 UI 照常执行 guest handle-ui-command。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_master_gate_hot_toggle_blocks_tool_and_panel_entries() {
    use nemesis_plugins_wasm::PluginTrustState;
    use nemesis_plugins_wasm::registry::ToolMetaSnapshot;
    use nemesis_plugins_wasm::runtime::ExecContext;

    // 壳 crate 独立 workspace，CI 的 build --workspace 不产出 host DLL；
    // 直构 manager 不经 fresh_manager，须自行 ensure（2026-11-08 CI 实锤：
    // 全新 checkout 无开发路径 DLL，构造即 NotAvailable）。
    // ensure 必须先于建目录（2026-11-08 CI 实锤二连）：冷缓存壳构建
    // ~8 分钟，建目录在前会把 mkdir→build_dirs canonicalize 暴露成
    // 8 分钟窗口，窗口内 workspace 目录消失 → "workspace root: os
    // error 3"；ensure 在前则窗口缩至亚秒，与其余 fresh_manager 用例
    // 同序。
    common::ensure_host_dll();
    // workspace 必须是 tmp 的子目录：runtime_enabled 读
    // `<workspace_root.parent()>/config.json`（fresh_manager 形态的 parent
    // 是共享 Temp，不能在此形态下写 config.json——污染并行测试的缺省 true
    // 读面）。
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path().join("workspace");
    std::fs::create_dir_all(&ws).expect("mkdir workspace");
    let mgr = Arc::new(
        PluginManager::new_with_dirs(
            &ws,
            &ws.join("plugins_wasm"),
            PluginLimits::default(),
            Arc::new(NoSecrets),
        )
        .expect("manager"),
    );

    // 注册面不受总闸影响（闸裁执行面不裁注册面）：声明式面板（component=0）
    // + tool（component=1 伪造句柄——只走闸面，不期待执行成功）。
    let manifest = PluginManifest::parse(&panel_manifest_raw("gate-panel", "main", false))
        .expect("parse")
        .0;
    mgr.register(
        manifest,
        PluginTrustState::ReviewRequired,
        0,
        None,
        None,
        None,
    )
    .expect("register panel");
    let raw = format!(
        "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"gate-tool\"\nkind = \"tool\"\n\
         name = \"GateTool\"\nversion = \"0.1.0\"\ndescription = \"d\"\n",
        "a".repeat(64)
    );
    let manifest = PluginManifest::parse(&raw).expect("parse").0;
    let meta = ToolMetaSnapshot {
        name: "plugin.gate-tool.run".into(),
        base: "run".into(),
        title: "Run".into(),
        description: "d".into(),
        parameters_json: "{}".into(),
        operation_type: "read".into(),
        min_tier: "big".into(),
    };
    mgr.register(
        manifest,
        PluginTrustState::ReviewRequired,
        1,
        Some(meta),
        None,
        None,
    )
    .expect("register tool");

    // 闸开（缺省 true）：面板走声明式诚实报错——本断言同时钉住闸序
    //（闸在 component 检查前，闸关时同一面板先报 master gate）。
    let err = mgr
        .invoke_ui("gate-panel", r#"{"command":"go"}"#.to_string())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("is declarative"), "{err}");

    // 翻关：真相源① = <workspace 父目录>/config.json 的
    // /plugins/wasm/enabled。
    std::fs::write(
        tmp.path().join("config.json"),
        r#"{"plugins":{"wasm":{"enabled":false}}}"#,
    )
    .expect("write config.json");

    let err = mgr
        .execute_tool(
            "plugin.gate-tool.run",
            "{}".to_string(),
            ExecContext::default(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("runtime master gate"),
        "execute_tool 必须被运行期总闸拦下: {err}"
    );
    let err = mgr
        .invoke_ui("gate-panel", r#"{"command":"go"}"#.to_string())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("runtime master gate"),
        "invoke_ui（面板命令通道）必须被运行期总闸拦下: {err}"
    );

    // 开关改回即时恢复：错误不再来自总闸（tool 落到伪造句柄的执行错误、
    // 面板落回声明式诚实报错）。
    std::fs::write(
        tmp.path().join("config.json"),
        r#"{"plugins":{"wasm":{"enabled":true}}}"#,
    )
    .expect("write config.json");
    let err = mgr
        .execute_tool(
            "plugin.gate-tool.run",
            "{}".to_string(),
            ExecContext::default(),
        )
        .await
        .unwrap_err();
    assert!(
        !err.to_string().contains("runtime master gate"),
        "开关改回必须即时恢复: {err}"
    );
    let err = mgr
        .invoke_ui("gate-panel", r#"{"command":"go"}"#.to_string())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("is declarative"),
        "开关改回必须即时恢复: {err}"
    );
}

#[test]
fn panel_route_collision_rejected_across_plugins() {
    use nemesis_plugins_wasm::PluginTrustState;

    let (_ws, mgr) = fresh_manager();
    let a = PluginManifest::parse(&panel_manifest_raw("panel-a", "main", false))
        .expect("parse")
        .0;
    mgr.register(a, PluginTrustState::ReviewRequired, 0, None, None, None)
        .expect("first register");
    // 同路由第二家 → 拒绝（/panel/<route> 动态挂载键全局唯一）。
    let b = PluginManifest::parse(&panel_manifest_raw("panel-b", "main", false))
        .expect("parse")
        .0;
    let err = mgr
        .register(b, PluginTrustState::ReviewRequired, 0, None, None, None)
        .expect_err("duplicate route must be rejected");
    assert!(
        err.to_string().contains("already declared by plugin"),
        "{err}"
    );
    // 换路由 → 过（唯一性只锁路由，不锁面板插件数）。
    let c = PluginManifest::parse(&panel_manifest_raw("panel-b", "other", false))
        .expect("parse")
        .0;
    mgr.register(c, PluginTrustState::ReviewRequired, 0, None, None, None)
        .expect("other route ok");
}

// ---------------------------------------------------------------------------
// guest step 批：`plugin:<slug>:<op>` 引用解析纯函数矩阵
// （valid_slug / valid_step_op / parse_plugin_step_ref）。安装期空词表/
// 非法词表的对账闸在 probe_and_reconcile FlowStep 臂，其 fixture 级覆盖
// 受 probe_frame 固定 InstanceConfig::default() 限制（无法经 config 种子
// 注入空词表组件），与 policy「至少一个策略点」闸同先例——纯函数矩阵 +
// e2e 全链（e2e_guest.rs stepdemo）作为覆盖面，诚实注记不装假绿。
// ---------------------------------------------------------------------------

#[test]
fn plugin_step_ref_parse_matrix() {
    use nemesis_plugins_wasm::manifest::{parse_plugin_step_ref, valid_slug, valid_step_op};

    // 合法形态：slug+op 基础款。
    assert_eq!(
        parse_plugin_step_ref("plugin:stepdemo:echo"),
        Some(("stepdemo", "echo"))
    );
    // op 段允许冒号（域内二级命名）；首个冒号切 slug，其余全归 op。
    assert_eq!(
        parse_plugin_step_ref("plugin:acme-jira:issue:fetch"),
        Some(("acme-jira", "issue:fetch"))
    );
    // slug 允许数字与内嵌连字符。
    assert_eq!(
        parse_plugin_step_ref("plugin:2fa-helper:t2"),
        Some(("2fa-helper", "t2"))
    );

    // 非本形态：缺前缀 / 缺 op 段 / 缺 slug 段。
    assert_eq!(parse_plugin_step_ref("fetch"), None);
    assert_eq!(parse_plugin_step_ref("plugin:stepdemo"), None);
    assert_eq!(parse_plugin_step_ref("plugin::echo"), None);
    assert_eq!(parse_plugin_step_ref("plugin:"), None);

    // slug 词表 [a-z0-9-]（不以 - 开头结尾、非 all）：大写/下划线/点号全拒。
    assert_eq!(parse_plugin_step_ref("plugin:Bad_Slug:echo"), None);
    assert_eq!(parse_plugin_step_ref("plugin:acme.jira:echo"), None);
    assert_eq!(parse_plugin_step_ref("plugin:-lead:echo"), None);
    assert_eq!(parse_plugin_step_ref("plugin:trail-:echo"), None);
    assert_eq!(parse_plugin_step_ref("plugin:all:echo"), None);

    // op 词表：非空、≤64、无空白/控制字符；冒号/中文/符号在界内合法。
    assert_eq!(parse_plugin_step_ref("plugin:s:has space"), None);
    assert_eq!(parse_plugin_step_ref("plugin:s:\t"), None);
    let long_op = "x".repeat(65);
    assert_eq!(parse_plugin_step_ref(&format!("plugin:s:{long_op}")), None);
    let max_op = "x".repeat(64);
    assert_eq!(
        parse_plugin_step_ref(&format!("plugin:s:{max_op}")),
        Some(("s", max_op.as_str()))
    );
    assert_eq!(
        parse_plugin_step_ref("plugin:s:拉取工单"),
        Some(("s", "拉取工单"))
    );

    // valid_slug / valid_step_op 直查边界。
    assert!(valid_slug("stepdemo"));
    assert!(!valid_slug(""));
    assert!(!valid_slug("STEPDEMO"));
    assert!(!valid_slug("a.b"));
    assert!(valid_step_op("issue:fetch"));
    assert!(!valid_step_op(""));
}

#[test]
fn flow_step_manifest_rejects_empty_kinds_at_probe_level_shape() {
    // 结构面：kind=flow-step 的 manifest 本体不带 kinds（词表来自 guest
    // 自报 get-step-metadata，manifest 只声明 kind）——parse 通过即声名
    // 面 ok；词表对账在 probe_and_reconcile（运行 probe 才能拿到 guest
    // 自报，fixture 级覆盖见 e2e_guest.rs stepdemo 全链 + 上方矩阵注记）。
    let raw = "wasm-sha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"\n\
               api-version = 1\n\
               slug = \"flowstep-demo\"\n\
               kind = \"flow-step\"\n\
               name = \"FS\"\n\
               version = \"0.1.0\"\n\
               description = \"shape probe\"\n";
    let (m, _) = PluginManifest::parse(raw).expect("flow-step manifest parses");
    assert_eq!(m.kind.as_str(), "flow-step");
    assert_eq!(m.slug, "flowstep-demo");
}

// ---------------------------------------------------------------------------
// 域侧批 H：connector 装配自动注册（install_core ①.5 预检 +
// ⑥.5 注册 + lockfile 记账 + 卸载保留提示）
// ---------------------------------------------------------------------------

/// 最小合法 connector.toml（tool 绑定 = 免 base/http 的最简形态；id/描述
/// 可参数化供冲突/一致性用例变化）。
fn connector_toml(id: &str, description: &str) -> String {
    format!(
        "[connector]\nid = \"{id}\"\nname = \"Demo\"\nversion = \"0.1.0\"\n\
         kind = \"rest\"\ndescription = \"{description}\"\n\n\
         [[op]]\nid = \"ping\"\nintent = [\"探活\"]\nrisk = \"low\"\nclass = \"network\"\n\
         tool = \"exec\"\nargs_schema = {{ type = \"object\", required = [], properties = {{}} }}\n"
    )
}

/// 纯声明式面板源目录（+ 可选包内 connector.toml）。
fn stage_declarative_with_connector(
    root: &Path,
    slug: &str,
    connector_body: Option<&str>,
) -> std::path::PathBuf {
    let dir = root.join("src");
    std::fs::create_dir_all(&dir).expect("stage dir");
    std::fs::write(
        dir.join("plugin.toml"),
        panel_manifest_raw(slug, "main", false),
    )
    .expect("write plugin.toml");
    if let Some(body) = connector_body {
        std::fs::write(dir.join("connector.toml"), body).expect("write connector.toml");
    }
    dir
}

fn read_lockfile_entry(mgr: &PluginManager, slug: &str) -> Option<LockfileEntry> {
    let s = std::fs::read_to_string(mgr.plugins_dir().join("lockfile.json")).ok()?;
    let lf: PluginLockfile = serde_json::from_str(&s).expect("lockfile parses");
    lf.plugins.get(slug).cloned()
}

#[tokio::test(flavor = "multi_thread")]
async fn install_registers_bundled_connector() {
    let (ws, mgr) = fresh_manager();
    let tmp = tempfile::tempdir().expect("stage tmp");
    let body = connector_toml("demo-conn", "装配自动注册样例");
    let src = stage_declarative_with_connector(tmp.path(), "conn-plugin", Some(&body));

    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer
        .install(Path::new(&src), true)
        .await
        .expect("install");

    // 落文件即注册：scan_connectors 在册。
    let found = nemesis_connectors::registry::scan_connectors(ws.path())
        .into_iter()
        .any(|c| c.connector.as_ref().map(|m| m.id.as_str()) == Some("demo-conn"));
    assert!(found, "connector 必须在 scan_connectors 可见");

    // 内容逐字节等于包内原文。
    let target = ws.path().join("connectors/demo-conn/connector.toml");
    assert_eq!(
        std::fs::read_to_string(&target).expect("connector file"),
        body,
        "注册内容必须逐字节等于包内 connector.toml"
    );

    // lockfile 记账（事实面：只记实际注册成功的 id）。
    let entry = read_lockfile_entry(&mgr, "conn-plugin").expect("lockfile entry");
    assert_eq!(entry.connectors, vec!["demo-conn".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn install_conflicting_connector_not_overwritten() {
    let (ws, mgr) = fresh_manager();
    // 用户已自定义 connector（描述是典型必改字段的替身）。
    let user_body = connector_toml("demo-conn", "用户自定义版");
    let cdir = ws.path().join("connectors/demo-conn");
    std::fs::create_dir_all(&cdir).expect("mkdir");
    std::fs::write(cdir.join("connector.toml"), &user_body).expect("seed user connector");

    let tmp = tempfile::tempdir().expect("stage tmp");
    let bundled = connector_toml("demo-conn", "插件包版本");
    let src = stage_declarative_with_connector(tmp.path(), "conn-plugin", Some(&bundled));

    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer
        .install(Path::new(&src), true)
        .await
        .expect("install");

    // 用户文件原样保留（覆盖即毁配置——核心冲突语义）。
    assert_eq!(
        std::fs::read_to_string(cdir.join("connector.toml")).expect("user connector"),
        user_body,
        "冲突时不得覆盖用户自定义 connector.toml"
    );
    // 事实面：未注册成功 = lockfile 不记。
    let entry = read_lockfile_entry(&mgr, "conn-plugin").expect("lockfile entry");
    assert!(
        entry.connectors.is_empty(),
        "冲突未注册不得进 lockfile 记账，got: {:?}",
        entry.connectors
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn install_same_content_connector_noop_reregisters() {
    let (ws, mgr) = fresh_manager();
    let tmp = tempfile::tempdir().expect("stage tmp");
    let body = connector_toml("demo-conn", "同内容重装");
    let src = stage_declarative_with_connector(tmp.path(), "conn-plugin", Some(&body));

    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer
        .install(Path::new(&src), true)
        .await
        .expect("first install");
    // 升级重装（同内容）：目标在场且逐字节一致 → no-op 且记账保留。
    installer
        .install(Path::new(&src), true)
        .await
        .expect("reinstall");

    assert_eq!(
        std::fs::read_to_string(ws.path().join("connectors/demo-conn/connector.toml"))
            .expect("connector file"),
        body,
        "no-op 路径内容不变"
    );
    let entry = read_lockfile_entry(&mgr, "conn-plugin").expect("lockfile entry");
    assert_eq!(
        entry.connectors,
        vec!["demo-conn".to_string()],
        "内容一致 = 事实在册，记账保留"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn install_rejects_broken_bundled_connector() {
    let (ws, mgr) = fresh_manager();
    let tmp = tempfile::tempdir().expect("stage tmp");
    // 坏 connector.toml（id 大写出词表 + 无 op）→ ①.5 fail-closed 拒装。
    let src = stage_declarative_with_connector(
        tmp.path(),
        "conn-plugin",
        Some(
            "[connector]\nid = \"BadConn\"\nname = \"B\"\nversion = \"0.1.0\"\n\
             kind = \"rest\"\ndescription = \"d\"\n",
        ),
    );

    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    let err = match installer.install(Path::new(&src), true).await {
        Ok(_) => panic!("broken bundled connector must reject install"),
        Err(e) => e,
    };
    assert!(
        matches!(err, PluginError::Manifest(ref m) if m.contains("connector.toml")),
        "got: {err:?}"
    );
    // 零突变：注册表/connector 目录/lockfile 三处全净。
    assert!(mgr.get("conn-plugin").is_none(), "被拒插件不得进入注册表");
    assert!(
        !ws.path().join("connectors").exists(),
        "被拒插件不得留下 connector 目录"
    );
    assert!(
        read_lockfile_entry(&mgr, "conn-plugin").is_none(),
        "被拒插件不得留 lockfile 条目"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn uninstall_keeps_registered_connector() {
    let (ws, mgr) = fresh_manager();
    let tmp = tempfile::tempdir().expect("stage tmp");
    let body = connector_toml("demo-conn", "卸载存活性样例");
    let src = stage_declarative_with_connector(tmp.path(), "conn-plugin", Some(&body));

    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer
        .install(Path::new(&src), true)
        .await
        .expect("install");

    nemesis_plugins_wasm::install::uninstall_blocking(&mgr, "conn-plugin").expect("uninstall");

    // connector 不随卸载删除（可能已被用户定制）。
    assert_eq!(
        std::fs::read_to_string(ws.path().join("connectors/demo-conn/connector.toml"))
            .expect("connector survives uninstall"),
        body,
        "卸载后 connector 必须存活"
    );
    assert!(
        read_lockfile_entry(&mgr, "conn-plugin").is_none(),
        "lockfile 已摘"
    );
}

#[test]
fn lockfile_connectors_field_backward_compat() {
    // 旧形态（无 connectors 键）→ serde default 空表，解析零迁移。
    let old = format!(
        r#"{{"version":1,"plugins":{{"demo":{{"version":"0.1.0","wasm_sha256":"{}","trust":"review-required","installed_at_ms":0,"signed_by":""}}}}}}"#,
        "a".repeat(64)
    );
    let lf: PluginLockfile = serde_json::from_str(&old).expect("old lockfile shape parses");
    assert!(
        lf.plugins["demo"].connectors.is_empty(),
        "缺 connectors 键必须落空表（#[serde(default)]）"
    );

    // 新形态 roundtrip 保留记账。
    let lf = PluginLockfile {
        version: 1,
        plugins: [(
            "demo".to_string(),
            LockfileEntry {
                version: "0.2.0".into(),
                wasm_sha256: String::new(),
                trust: "review-required".into(),
                installed_at_ms: 1,
                signed_by: String::new(),
                connectors: vec!["gitea".to_string()],
            },
        )]
        .into_iter()
        .collect(),
    };
    let s = serde_json::to_string(&lf).expect("serialize");
    let back: PluginLockfile = serde_json::from_str(&s).expect("roundtrip");
    assert_eq!(back.plugins["demo"].connectors, vec!["gitea".to_string()]);
}

// ---------------------------------------------------------------------------
// 自持c：issue-import 0.2.0 两件套（plugin.toml + guest 载荷）
// 盘上真相 drift 锁——shipped 原文永远可解析可装配，清单/哈希不同步时 CI
// 当场响（install 哈希对账吃真文件，不用等用户装机才红）
// ---------------------------------------------------------------------------

/// 仓库内 shipped 真相（`../../` = crate dir → 仓库根；测试只在本仓跑）。
fn shipped_issue_import_manifest_raw() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wasm/issue-import/plugin.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "shipped issue-import plugin.toml 不可读（{}）: {e}",
            path.display()
        )
    })
}

#[test]
fn shipped_issue_import_manifest_is_valid() {
    let raw = shipped_issue_import_manifest_raw();
    let (m, _) = PluginManifest::parse(&raw).expect("shipped 模板必须可解析");
    m.validate().expect("shipped 模板必须过 manifest.validate");
    assert_eq!(m.slug, "issue-import");
    assert_eq!(m.kind.as_str(), "panel");
    assert!(
        m.wasm_sha256.is_some(),
        "载荷插件必须带 wasm-sha256（guest 构建后回填）"
    );
    assert_eq!(m.ui.pages.len(), 1);
    // 自持面（0.2.0）：egress 只打实例基址（整条目 token），PAT 走 x-secret
    // （面板表单 secret-set 写 vault plugin/issue-import/pat）；自托管 Gitea
    // 常驻企业内网 → allow-private 是产品语义（allowlist 仍钉 base_url 精确值）。
    assert_eq!(m.permissions.egress, ["{{config.base_url}}"]);
    assert!(m.permissions.allow_private, "内网自托管 Gitea 是真实靶");
    assert_eq!(m.permissions.x_secret, ["pat"]);
    // 轻量模板形态：恰好 fetch→land 两步（0.2.0 fetch = guest 自报步，
    // 0.1.x connector 复用已退役）+ origin 去重 + on-transition 回写。
    assert_eq!(m.flow.templates.len(), 1, "单模板");
    let t = &m.flow.templates[0];
    assert_eq!(t.id, "issue-import");
    assert_eq!(t.steps.len(), 2);
    assert_eq!(t.steps[0].kind, "plugin:issue-import:fetch");
    assert_eq!(t.steps[1].kind, "board_land");
    assert!(!t.origin_id.is_empty(), "origin 去重键必声明");
    assert_eq!(t.origin_type, "gitea");
    assert_eq!(t.on_transition.op, "plugin:issue-import:comment");
    assert!(
        !t.on_transition.parse_origin_id.is_empty(),
        "parse-origin-id 兜旧单（C-2 声明面）"
    );
    // config-schema：base_url 无缺省（防假缺省 URL——未配置时 guest 诚实
    // 报错），owner/repo/state 带声明缺省。
    let base = m
        .config_schema
        .get("base_url")
        .expect("base_url 必在 schema");
    match base {
        ConfigSchemaEntry::Full { default, .. } => {
            assert!(default.is_empty(), "base_url 不得带缺省值（防假缺省）");
        }
        other => panic!("base_url 应为 Full 形态（带 desc），实际 {other:?}"),
    }
    for k in ["owner", "repo", "state"] {
        assert!(
            matches!(
                m.config_schema.get(k),
                Some(ConfigSchemaEntry::Full { default, .. }) if !default.is_empty()
            ),
            "{k} 必带声明缺省"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shipped_issue_import_install_e2e() {
    let (_ws, mgr) = fresh_manager();
    // shipped 原文 + 真载荷直接过漏斗（零改写 = drift 锁的装配半边）。
    // 声明式形态（0.1.x 只 stage plugin.toml）已随 connector 复用退役——
    // 平台对纯声明式面板的接受度由 panel_declarative_register_and_invoke_
    // honest_error + stage_declarative_with_connector 夹具家族继续钉住。
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).expect("stage dir");
    std::fs::write(src.join("plugin.toml"), shipped_issue_import_manifest_raw())
        .expect("write plugin.toml");
    let wasm_src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wasm/issue-import/plugin.wasm");
    std::fs::copy(&wasm_src, src.join("plugin.wasm"))
        .expect("stage shipped plugin.wasm（两件套必须同在）");

    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer
        .install(Path::new(&src), true)
        .await
        .expect("payload install（清单/哈希不同步在步骤③当场拒）");

    // 落位 + lockfile 记账（新装默认不加载：注册表缺席 = 正常）。
    let landed = mgr.plugins_dir().join("issue-import/plugin.toml");
    assert_eq!(
        std::fs::read_to_string(&landed).expect("landed manifest"),
        shipped_issue_import_manifest_raw(),
        "落位内容必须逐字节等于 shipped 原文"
    );
    let (m, _) = PluginManifest::parse(&shipped_issue_import_manifest_raw()).expect("parse");
    let entry = read_lockfile_entry(&mgr, "issue-import").expect("lockfile entry");
    assert_eq!(
        entry.wasm_sha256,
        m.wasm_sha256.clone().unwrap_or_default(),
        "lockfile 记账 = 清单哈希 = shipped 载荷实测（三方一致）"
    );
    assert_eq!(
        entry.trust, "review-required",
        "--allow-unsigned 语义落 lockfile"
    );
    assert!(mgr.get("issue-import").is_none(), "新装默认不加载");
}

// ---------------------------------------------------------------------------
// shipped jira-fix 域插件 drift 锁（两件套照 bugfix-flow 样板——0.2.0 自持
// 批 connector.toml 退役；改动清单/哈希不同步时 CI 当场响）
// ---------------------------------------------------------------------------

#[test]
fn shipped_jira_fix_manifest_is_valid() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wasm/jira-fix/plugin.toml");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "shipped jira-fix plugin.toml 不可读（{}）: {e}",
            path.display()
        )
    });
    let (m, _) = PluginManifest::parse(&raw).expect("shipped 模板必须可解析");
    m.validate().expect("shipped 模板必须过 manifest.validate");
    assert_eq!(m.slug, "jira-fix");
    assert_eq!(m.kind.as_str(), "panel");
    assert!(
        m.wasm_sha256.is_some(),
        "载荷插件必须带 wasm-sha256（guest 构建后回填）"
    );
    assert_eq!(m.ui.pages.len(), 1);
    assert_eq!(m.ui.pages[0].route, "jira-fix");
    // 自持面（0.2.0）：egress 只打实例基址（整条目 token），PAT 走 x-secret
    // （面板表单 secret-set 写 vault plugin/jira-fix/pat）；自托管 Jira
    // Server/DC 常驻企业内网 → allow-private 是产品语义（allowlist 仍钉
    // base_url 精确值）。
    assert_eq!(m.permissions.egress, ["{{config.base_url}}"]);
    assert!(m.permissions.allow_private, "内网自托管 Jira 是真实靶");
    assert_eq!(m.permissions.x_secret, ["pat"]);
    // 单模板五步：取→落→修复循环（人工闸）→验收→报告；无 publish 步
    // （Jira 不托管 git）。取数/回帖 = guest 自报步骤（plugin: 前缀）。
    assert_eq!(m.flow.templates.len(), 1, "单模板");
    let t = &m.flow.templates[0];
    assert_eq!(t.id, "jira-pipeline");
    let kinds: Vec<&str> = t.steps.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "plugin:jira-fix:fetch",
            "board_land",
            "agent_fixloop",
            "check_regress",
            "report"
        ]
    );
    assert!(!t.origin_id.is_empty(), "origin 去重键必声明");
    assert_eq!(t.origin_type, "jira");
    assert_eq!(t.on_transition.op, "plugin:jira-fix:comment");
    assert!(
        !t.on_transition.parse_origin_id.is_empty(),
        "parse-origin-id 兜旧单（C-2 声明面）"
    );
    // report 步走 guest 自报 comment op（plugin: 协议前缀）。
    let report = t.steps.last().expect("report step");
    let report_connector = report.args.get("connector").map(String::as_str);
    assert!(
        report_connector.is_some_and(|c| c.starts_with("plugin:")),
        "report 步必须声明 plugin: 协议（connector 字面键已退役）"
    );
    // config-schema：base_url 无缺省（防假缺省 URL——未配置时 guest 诚实
    // 报错），project_key/max_results 带声明缺省。
    let base = m
        .config_schema
        .get("base_url")
        .expect("base_url 必在 schema");
    match base {
        ConfigSchemaEntry::Full { default, .. } => {
            assert!(default.is_empty(), "base_url 不得带缺省值（防假缺省）");
        }
        other => panic!("base_url 应为 Full 形态（带 desc），实际 {other:?}"),
    }
    for k in ["project_key", "max_results"] {
        assert!(
            matches!(
                m.config_schema.get(k),
                Some(ConfigSchemaEntry::Full { default, .. }) if !default.is_empty()
            ),
            "{k} 必带声明缺省"
        );
    }
}

// ---------------------------------------------------------------------------
// shipped jira-gitlab-flow 域插件 drift 锁（两件套照 jira-fix/
// bugfix-flow 样板——Jira 取数 + GitLab 仓副本修复 + MR 发布全链；改动清单/
// 哈希不同步时 CI 当场响）
// ---------------------------------------------------------------------------

#[test]
fn shipped_jira_gitlab_flow_manifest_is_valid() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/wasm/jira-gitlab-flow/plugin.toml");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "shipped jira-gitlab-flow plugin.toml 不可读（{}）: {e}",
            path.display()
        )
    });
    let (m, _) = PluginManifest::parse(&raw).expect("shipped 模板必须可解析");
    m.validate().expect("shipped 模板必须过 manifest.validate");
    assert_eq!(m.slug, "jira-gitlab-flow");
    assert_eq!(m.kind.as_str(), "panel");
    assert!(
        m.wasm_sha256.is_some(),
        "载荷插件必须带 wasm-sha256（guest 构建后回填）"
    );
    assert_eq!(m.ui.pages.len(), 1);
    assert_eq!(m.ui.pages[0].route, "jira-gitlab-flow");
    // 自持面：egress 双基址（整条目 token，任一未配置 = 模板不注册），双 PAT
    // 走 x-secret（pat = GitLab PRIVATE-TOKEN 兼推送密码——publish 凭据推导
    // 钉死；jira_pat = Jira Bearer）；自托管双实例常驻企业内网 →
    // allow-private 是产品语义（allowlist 仍钉 base_url 精确值）。
    assert_eq!(
        m.permissions.egress,
        ["{{config.jira_base_url}}", "{{config.gitlab_base_url}}"]
    );
    assert!(m.permissions.allow_private, "内网自托管双实例是真实靶");
    assert_eq!(m.permissions.x_secret, ["pat", "jira_pat"]);
    // 单模板七步：取→仓副本→落→修复循环（唯一人工闸）→验收→发布（自动）
    // →报告；publish 无 gate（发布动作自动，修复内容人审）。
    assert_eq!(m.flow.templates.len(), 1, "单模板");
    let t = &m.flow.templates[0];
    assert_eq!(t.id, "jira-gitlab-pipeline");
    assert_eq!(t.origin_type, "jira");
    assert!(!t.origin_id.is_empty(), "origin 去重键必声明");
    assert!(
        t.origin_id.contains("{{config.jira_project_key}}") && t.origin_id.contains("{{key}}"),
        "origin-id 只可用 {{config.*}} 与 {{key}}（plugin fetch 专管 token 面）"
    );
    let kinds: Vec<&str> = t.steps.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "plugin:jira-gitlab-flow:fetch",
            "repo_sync",
            "board_land",
            "agent_fixloop",
            "check_regress",
            "publish",
            "report"
        ]
    );
    // 唯一人工闸 = fixloop；publish 无 gate。
    let by_id = |id: &str| {
        t.steps
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("缺步骤 {id}"))
    };
    assert_eq!(by_id("fixloop").gate, "manual", "fixloop = 唯一人工闸");
    assert_eq!(by_id("fixloop").on_fail, "park_human");
    assert!(
        by_id("publish").gate.is_empty(),
        "publish 自动不停（无 gate）"
    );
    assert_eq!(
        by_id("publish").on_fail,
        "continue",
        "发布失败缺口记账不停车"
    );
    assert_eq!(by_id("sync").on_fail, "park_human");
    // repo_sync 仓副本声明：project/url/cred_secret（凭据与 publish 同源）。
    let sync = by_id("sync");
    assert_eq!(
        sync.args.get("project").map(String::as_str),
        Some("{{config.board_project}}")
    );
    assert!(
        sync.args
            .get("url")
            .is_some_and(|u| u.contains("{{config.gitlab_base_url}}")),
        "sync url 必含 gitlab_base_url token"
    );
    assert_eq!(
        sync.args.get("cred_secret").map(String::as_str),
        Some("plugin/jira-gitlab-flow/pat"),
        "仓副本凭据 = publish 推送同源别名"
    );
    // publish 协议声明：双 op 均 plugin 引用 + result/reconcile 抽取坐标
    // （GitLab MR 顶层字段 iid/web_url/source_branch）。
    let publish = by_id("publish");
    assert_eq!(
        publish.args.get("create_op").map(String::as_str),
        Some("plugin:jira-gitlab-flow:create_mr")
    );
    assert_eq!(
        publish.args.get("reconcile_op").map(String::as_str),
        Some("plugin:jira-gitlab-flow:mr_list")
    );
    assert_eq!(
        publish.args.get("result_ref").map(String::as_str),
        Some("$.iid")
    );
    assert_eq!(
        publish.args.get("result_url").map(String::as_str),
        Some("$.web_url")
    );
    assert_eq!(
        publish.args.get("reconcile_match").map(String::as_str),
        Some("$.source_branch == {{branch}}")
    );
    // report 步走 guest report op（回帖 + 幂等转状态一体）。
    let report = by_id("report");
    assert_eq!(
        report.args.get("connector").map(String::as_str),
        Some("plugin:jira-gitlab-flow:report")
    );
    // on-transition 回写（guest comment op + segment 分解兜旧单）。
    assert_eq!(t.on_transition.op, "plugin:jira-gitlab-flow:comment");
    assert!(
        !t.on_transition.parse_origin_id.is_empty(),
        "parse-origin-id 兜旧单（C-2 声明面）"
    );
    // config-schema：双 base_url 无缺省（防假缺省 URL——未配置时模板不注册），
    // 其余六键带声明缺省。
    for k in ["jira_base_url", "gitlab_base_url"] {
        match m.config_schema.get(k).expect("base_url 必在 schema") {
            ConfigSchemaEntry::Full { default, .. } => {
                assert!(default.is_empty(), "{k} 不得带缺省值（防假缺省）");
            }
            other => panic!("{k} 应为 Full 形态（带 desc），实际 {other:?}"),
        }
    }
    for k in [
        "jira_project_key",
        "jira_transition_name",
        "gitlab_owner",
        "gitlab_repo",
        "gitlab_target_branch",
        "board_project",
    ] {
        assert!(
            matches!(
                m.config_schema.get(k),
                Some(ConfigSchemaEntry::Full { default, .. }) if !default.is_empty()
            ),
            "{k} 必带声明缺省"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shipped_jira_gitlab_flow_install_e2e() {
    let (_ws, mgr) = fresh_manager();
    // shipped 原文 + 真载荷直接过漏斗（零改写 = drift 锁的装配半边；
    // 清单/哈希不同步在哈希对账步骤当场拒）。
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).expect("stage dir");
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/wasm/jira-gitlab-flow/plugin.toml");
    let raw = std::fs::read_to_string(&manifest_path).expect("read shipped plugin.toml");
    std::fs::write(src.join("plugin.toml"), &raw).expect("write plugin.toml");
    let wasm_src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/wasm/jira-gitlab-flow/plugin.wasm");
    std::fs::copy(&wasm_src, src.join("plugin.wasm"))
        .expect("stage shipped plugin.wasm（两件套必须同在）");

    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer
        .install(Path::new(&src), true)
        .await
        .expect("payload install（清单/哈希不同步在步骤③当场拒）");

    // 落位 + lockfile 记账（新装默认不加载：注册表缺席 = 正常）。
    let landed = mgr.plugins_dir().join("jira-gitlab-flow/plugin.toml");
    assert_eq!(
        std::fs::read_to_string(&landed).expect("landed manifest"),
        raw,
        "落位内容必须逐字节等于 shipped 原文"
    );
    let (m, _) = PluginManifest::parse(&raw).expect("parse");
    let entry = read_lockfile_entry(&mgr, "jira-gitlab-flow").expect("lockfile entry");
    assert_eq!(
        entry.wasm_sha256,
        m.wasm_sha256.clone().unwrap_or_default(),
        "lockfile 记账 = 清单哈希 = shipped 载荷实测（三方一致）"
    );
    assert_eq!(
        entry.trust, "review-required",
        "--allow-unsigned 语义落 lockfile"
    );
    assert!(mgr.get("jira-gitlab-flow").is_none(), "新装默认不加载");
}

// ---------------------------------------------------------------------------
// shipped sc-bugfix 域插件 drift 锁（两件套照 jira-gitlab-flow
// 样板——Jira 取数 + GitLab/Gerrit 双臂仓副本修复 + MR/change 发布全链；
// 改动清单/哈希不同步时 CI 当场响）
// ---------------------------------------------------------------------------

#[test]
fn shipped_sc_bugfix_manifest_is_valid() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wasm/sc-bugfix/plugin.toml");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "shipped sc-bugfix plugin.toml 不可读（{}）: {e}",
            path.display()
        )
    });
    let (m, _) = PluginManifest::parse(&raw).expect("shipped 模板必须可解析");
    m.validate().expect("shipped 模板必须过 manifest.validate");
    assert_eq!(m.slug, "sc-bugfix");
    assert_eq!(m.kind.as_str(), "panel");
    assert!(
        m.wasm_sha256.is_some(),
        "载荷插件必须带 wasm-sha256（guest 构建后回填）"
    );
    assert_eq!(m.ui.pages.len(), 1);
    assert_eq!(m.ui.pages[0].route, "sc-bugfix");
    // 自持面：egress 三基址（整条目 token，未配置源 = 条目丢弃；全部未配置
    // = egress 整体关闭），四凭据走 x-secret（pat = GitLab PRIVATE-TOKEN
    // 兼推送密码——publish 凭据推导钉死；jira_pat = Jira Bearer；
    // jira_password = Jira Basic；gerrit_http_pass = Gerrit HTTP Basic）；
    // 自托管三实例常驻企业内网 → allow-private 是产品语义（allowlist 仍钉
    // base_url 精确值）。
    assert_eq!(
        m.permissions.egress,
        [
            "{{config.jira_base_url}}",
            "{{config.gitlab_base_url}}",
            "{{config.gerrit_base_url}}"
        ]
    );
    assert!(m.permissions.allow_private, "内网自托管实例是真实靶");
    assert_eq!(
        m.permissions.x_secret,
        ["pat", "jira_pat", "jira_password", "gerrit_http_pass"]
    );
    // 单模板双臂九步：取→仓副本（gitlab/gerrit 按 code_host 裁剪）→落→
    // 修复循环（唯一人工闸）→验收→发布（双臂，自动）→报告。
    assert_eq!(m.flow.templates.len(), 1, "单模板");
    let t = &m.flow.templates[0];
    assert_eq!(t.id, "sc-bugfix-pipeline");
    assert_eq!(t.origin_type, "jira");
    assert!(
        !t.origin_id.contains("{{connector}}") && !t.origin_id.contains("{{arg."),
        "origin-id 只可用 {{config.*}} 与 {{key}}（plugin fetch 专管 token 面）"
    );
    let by_id = |id: &str| {
        t.steps
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("缺步骤 {id}"))
    };
    let kinds: Vec<&str> = t.steps.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "plugin:sc-bugfix:fetch",
            "repo_sync",
            "repo_sync",
            "board_land",
            "agent_fixloop",
            "check_regress",
            "publish",
            "publish",
            "report"
        ]
    );
    // 双臂 when：sync_gitlab/sync_gerrit 与 publish_gitlab/publish_gerrit
    // 按 code_host 装载期裁剪（假臂 = Skipped 非失败）。
    assert_eq!(by_id("sync-gitlab").when, "{{config.code_host}} == gitlab");
    assert_eq!(by_id("sync-gerrit").when, "{{config.code_host}} == gerrit");
    assert_eq!(
        by_id("publish-gitlab").when,
        "{{config.code_host}} == gitlab"
    );
    assert_eq!(
        by_id("publish-gerrit").when,
        "{{config.code_host}} == gerrit"
    );
    // 唯一人工闸 = fixloop；publish 无 gate；sync 失败停车（没法修复），
    // publish 失败缺口记账不停车。
    assert_eq!(by_id("fixloop").gate, "manual", "fixloop = 唯一人工闸");
    assert_eq!(by_id("fixloop").on_fail, "park_human");
    assert_eq!(by_id("sync-gitlab").on_fail, "park_human");
    assert_eq!(by_id("sync-gerrit").on_fail, "park_human");
    assert!(
        by_id("publish-gitlab").gate.is_empty() && by_id("publish-gerrit").gate.is_empty(),
        "publish 自动不停（无 gate）"
    );
    assert_eq!(by_id("publish-gitlab").on_fail, "continue");
    assert_eq!(by_id("publish-gerrit").on_fail, "continue");
    // repo_sync 双臂声明：project/url/cred_secret/cred_user（凭据各臂独立，
    // 与 publish 同源别名）。
    for (id, secret, user_key) in [
        ("sync-gitlab", "plugin/sc-bugfix/pat", "gitlab_owner"),
        (
            "sync-gerrit",
            "plugin/sc-bugfix/gerrit_http_pass",
            "gerrit_username",
        ),
    ] {
        let s = by_id(id);
        assert_eq!(
            s.args.get("cred_secret").map(String::as_str),
            Some(secret),
            "{id} 仓副本凭据别名"
        );
        assert!(
            s.args
                .get("cred_user")
                .is_some_and(|u| u.contains(&format!("{{{{config.{user_key}}}}}"))),
            "{id} cred_user 必含 {user_key} token（GitLab HTTPS+PAT 任意非空用户名；Gerrit HTTP Basic 真用户名）"
        );
    }
    // publish_gitlab 协议声明（GitLab MR 顶层字段 iid/web_url/source_branch）。
    let pg = by_id("publish-gitlab");
    assert_eq!(
        pg.args.get("create_op").map(String::as_str),
        Some("plugin:sc-bugfix:create_mr")
    );
    assert_eq!(
        pg.args.get("reconcile_op").map(String::as_str),
        Some("plugin:sc-bugfix:mr_list")
    );
    assert_eq!(pg.args.get("result_ref").map(String::as_str), Some("$.iid"));
    assert_eq!(
        pg.args.get("result_url").map(String::as_str),
        Some("$.web_url")
    );
    assert_eq!(
        pg.args.get("reconcile_match").map(String::as_str),
        Some("$.source_branch == {{branch}}")
    );
    // publish_gerrit 协议声明：refs/for/ 推送（宿主 Gerrit 臂触发）+
    // change:<Change-Id> query-as-create + $.change_id 对账匹配 + Change-Id
    // 页脚（宿主 derive_change_id 确定性派生，重试 = 同 change 新 patchset）。
    let pr = by_id("publish-gerrit");
    assert!(
        pr.args
            .get("push_ref")
            .is_some_and(|r| r.starts_with("refs/for/")),
        "Gerrit 臂必须推 refs/for/（宿主 publish Gerrit 臂触发形态）"
    );
    assert_eq!(
        pr.args.get("create_op").map(String::as_str),
        Some("plugin:sc-bugfix:change_list")
    );
    assert_eq!(
        pr.args.get("reconcile_op").map(String::as_str),
        Some("plugin:sc-bugfix:change_list")
    );
    assert_eq!(
        pr.args.get("result_ref").map(String::as_str),
        Some("$.number")
    );
    assert_eq!(
        pr.args.get("result_url").map(String::as_str),
        Some("$.web_url")
    );
    assert_eq!(
        pr.args.get("reconcile_match").map(String::as_str),
        Some("$.change_id == {{change_id}}")
    );
    let commit_msg = pr
        .args
        .get("commit_message")
        .map(String::as_str)
        .unwrap_or_default();
    assert!(
        commit_msg.contains("Change-Id: {{change_id}}"),
        "commit_message 必带 Change-Id 页脚模板（宿主缺页脚时自动补，显式声明免歧义）"
    );
    // land 步六阶段契约前导（body_preamble → 宿主 board_land 注入 issue
    // description 头部；worker 读单即读纪律）。静态纪律文本：六阶段词 +
    // 证据标记 [UNDERSTOOD]/[ROOTCAUSE] 必在场，不放行语义必须钉住（声明
    // 面 drift 锁）。
    let land_pre = by_id("land")
        .args
        .get("body_preamble")
        .map(String::as_str)
        .unwrap_or_default();
    assert!(
        land_pre.contains("六阶段")
            && land_pre.contains("[UNDERSTOOD]")
            && land_pre.contains("[ROOTCAUSE]"),
        "body_preamble 必带六阶段契约与证据标记"
    );
    assert!(
        land_pre.contains("证据标记缺失 = 验收不放行"),
        "前导必须钉住不放行语义"
    );
    // report 步走 guest report op（回帖 + 幂等转状态一体；转状态目标 =
    // jira_transition_name 配置，留空 = 只回帖）。
    assert_eq!(
        by_id("report").args.get("connector").map(String::as_str),
        Some("plugin:sc-bugfix:report")
    );
    // on-transition 回写（guest comment op + segment 分解兜旧单）。
    assert_eq!(t.on_transition.op, "plugin:sc-bugfix:comment");
    assert!(
        !t.on_transition.parse_origin_id.is_empty(),
        "parse-origin-id 兜旧单（C-2 声明面）"
    );
    // config-schema：三 base_url 无缺省（防假缺省 URL——未配置时 guest 诚实
    // 报错）；jira_transition_name 无缺省（留空 = 只回帖不转状态，语义键）；
    // 其余带声明缺省；code_host/jira_auth_mode 缺省即合法值域。
    for k in [
        "jira_base_url",
        "gitlab_base_url",
        "gerrit_base_url",
        "jira_transition_name",
    ] {
        match m.config_schema.get(k).expect("schema 键必在") {
            ConfigSchemaEntry::Full { default, .. } => {
                assert!(
                    default.is_empty(),
                    "{k} 不得带缺省值（防假缺省/防瞎转状态）"
                );
            }
            other => panic!("{k} 应为 Full 形态（带 desc），实际 {other:?}"),
        }
    }
    for k in [
        "code_host",
        "jira_project_key",
        "jira_auth_mode",
        "gitlab_owner",
        "gitlab_repo",
        "gitlab_target_branch",
        "gerrit_target_branch",
        "board_project",
    ] {
        assert!(
            matches!(
                m.config_schema.get(k),
                Some(ConfigSchemaEntry::Full { default, .. }) if !default.is_empty()
            ),
            "{k} 必带声明缺省"
        );
    }
    assert_eq!(
        m.config_schema.get("code_host").map(|e| match e {
            ConfigSchemaEntry::Full { default, .. } => default.as_str(),
            _ => "",
        }),
        Some("gitlab"),
        "code_host 缺省 = gitlab 臂"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn shipped_sc_bugfix_install_e2e() {
    let (_ws, mgr) = fresh_manager();
    // shipped 原文 + 真载荷直接过漏斗（零改写 = drift 锁的装配半边；
    // 清单/哈希不同步在哈希对账步骤当场拒）。
    let tmp = tempfile::tempdir().expect("stage tmp");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).expect("stage dir");
    let manifest_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wasm/sc-bugfix/plugin.toml");
    let raw = std::fs::read_to_string(&manifest_path).expect("read shipped plugin.toml");
    std::fs::write(src.join("plugin.toml"), &raw).expect("write plugin.toml");
    let wasm_src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/wasm/sc-bugfix/plugin.wasm");
    std::fs::copy(&wasm_src, src.join("plugin.wasm"))
        .expect("stage shipped plugin.wasm（两件套必须同在）");

    let installer = PluginInstaller::new(mgr.clone(), Arc::new(AutoApprove), None);
    installer
        .install(Path::new(&src), true)
        .await
        .expect("payload install（清单/哈希不同步在步骤③当场拒）");

    // 落位 + lockfile 记账（新装默认不加载：注册表缺席 = 正常）。
    let landed = mgr.plugins_dir().join("sc-bugfix/plugin.toml");
    assert_eq!(
        std::fs::read_to_string(&landed).expect("landed manifest"),
        raw,
        "落位内容必须逐字节等于 shipped 原文"
    );
    let (m, _) = PluginManifest::parse(&raw).expect("parse");
    let entry = read_lockfile_entry(&mgr, "sc-bugfix").expect("lockfile entry");
    assert_eq!(
        entry.wasm_sha256,
        m.wasm_sha256.clone().unwrap_or_default(),
        "lockfile 记账 = 清单哈希 = shipped 载荷实测（三方一致）"
    );
    assert_eq!(
        entry.trust, "review-required",
        "--allow-unsigned 语义落 lockfile"
    );
    assert!(mgr.get("sc-bugfix").is_none(), "新装默认不加载");
}

// ---------------------------------------------------------------------------
// 自持写通道批：egress `{{config.<key>}}` 整条目 token——
// manifest 校验（声明闸，validate 顺序在 x-secret 冲突检查之前）+
// render_egress_tokens（渲染丢弃语义：配置缺席 = 条目丢弃不破
// deny-by-default）
// ---------------------------------------------------------------------------

fn parse_manifest_with_egress(egress: &str, schema: &str, x_secret: &str) -> Result<(), String> {
    let raw = format!(
        "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"demo\"\nkind = \"tool\"\n\
         name = \"Demo\"\nversion = \"0.1.0\"\ndescription = \"demo\"\n\
         [config-schema]\n{schema}\n\
         [permissions]\n{x_secret}egress = [{egress}]\n",
        "a".repeat(64)
    );
    match PluginManifest::parse(&raw) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[test]
fn egress_config_token_manifest_validation() {
    // 声明键整条目 token → 过。
    assert!(
        parse_manifest_with_egress("\"{{config.base_host}}\"", "base_host = \"string\"", "")
            .is_ok()
    );
    // 未声明键 → loud 拒（渲染白名单 = config-schema 声明）。
    let err = parse_manifest_with_egress("\"{{config.base_host}}\"", "", "").unwrap_err();
    assert!(err.contains("undeclared"), "{err}");
    // x-secret 键引用 → loud 拒（凭据不进 allowlist；键须同时声明在
    // config-schema 才会走到 x-secret 臂——纯 x-secret 声明由 undeclared
    // 臂先拦，冲突检查在本闸之后才跑）。
    let err = parse_manifest_with_egress(
        "\"{{config.token}}\"",
        "token = \"string\"",
        "x-secret = [\"token\"]\n",
    )
    .unwrap_err();
    assert!(err.contains("x-secret"), "{err}");
    // 非整条目 token 形态（前缀混排/嵌套替换不做）→ loud 拒。
    let err =
        parse_manifest_with_egress("\"{{config.base_host}}/api\"", "base_host = \"string\"", "")
            .unwrap_err();
    assert!(err.contains("whole-entry"), "{err}");
    // `{{config.` 形态但缺右括号 → 同样 whole-entry 拒。
    let err = parse_manifest_with_egress("\"{{config.base_host\"", "base_host = \"string\"", "")
        .unwrap_err();
    assert!(err.contains("whole-entry"), "{err}");
    // 字面条目照旧：合法过、非法拒（原闸回归）。
    assert!(parse_manifest_with_egress("\"example.com\"", "", "").is_ok());
    let err = parse_manifest_with_egress("\"*.example.com\"", "", "").unwrap_err();
    assert!(err.contains("invalid egress"), "{err}");
}

#[test]
fn egress_ipv6_entry_forms_validate_and_matcher_align() {
    use nemesis_plugins_wasm::egress::allowlisted;

    // validate 矩阵：三分形态全放行（此前 `split(':').count() > 2` 把全部
    // 手写 IPv6 条目 loud 拒，bracketed 钉端口臂成死代码）。
    assert!(parse_manifest_with_egress("\"[2001:db8::9]:443\"", "", "").is_ok());
    assert!(parse_manifest_with_egress("\"[::1]\"", "", "").is_ok());
    assert!(parse_manifest_with_egress("\"2001:db8::9\"", "", "").is_ok());
    assert!(parse_manifest_with_egress("\"example.com:443\"", "", "").is_ok());
    // 畸形臂 loud 拒：bracketed 缺右括号 / 非 IPv6 主机 / 空端口尾巴 /
    // 裸含冒号垃圾 / 非 hex IPv6。
    let err = parse_manifest_with_egress("\"[2001:db8::9\"", "", "").unwrap_err();
    assert!(err.contains("invalid egress"), "{err}");
    let err = parse_manifest_with_egress("\"[example.com]:443\"", "", "").unwrap_err();
    assert!(err.contains("invalid egress"), "{err}");
    let err = parse_manifest_with_egress("\"[::1]:\"", "", "").unwrap_err();
    assert!(err.contains("invalid egress"), "{err}");
    let err = parse_manifest_with_egress("\"host::junk\"", "", "").unwrap_err();
    assert!(err.contains("invalid egress"), "{err}");
    let err = parse_manifest_with_egress("\"2001:db8::zz\"", "", "").unwrap_err();
    assert!(err.contains("invalid egress"), "{err}");

    // 对齐面：validate 放行的形态必须被 `allowlisted` 按文档语义命中
    //（deny-by-default 的完整闭环 = 能声明 ⇔ 能匹配，否则死条目）。
    let pol = |entries: &[&str]| {
        EgressPolicy::from_permissions(entries.iter().map(|s| s.to_string()).collect(), false)
    };
    // bracketed 钉端口：只命中钉住的端口。
    let p = pol(&["[2001:db8::9]:443"]);
    assert!(allowlisted(&p, "2001:db8::9", 443));
    assert!(!allowlisted(&p, "2001:db8::9", 8443));
    // bracketed 裸形态 = 任意端口。
    let p = pol(&["[::1]"]);
    assert!(allowlisted(&p, "::1", 9999));
    // 裸 IPv6 字面量 = 任意端口。
    let p = pol(&["2001:db8::9"]);
    assert!(allowlisted(&p, "2001:db8::9", 80));
    // 既有 host:port 语义对照（v4/域名钉端口不受 IPv6 扩展影响）。
    let p = pol(&["example.com:443"]);
    assert!(allowlisted(&p, "example.com", 443));
    assert!(!allowlisted(&p, "example.com", 80));
}

#[test]
fn egress_token_render_matrix() {
    use std::collections::BTreeMap;

    use nemesis_plugins_wasm::egress::render_egress_tokens;

    let mut cfg = BTreeMap::new();
    cfg.insert("base_host".to_string(), "jira.example.com".to_string());

    // 字面条目原样保留。
    assert_eq!(
        render_egress_tokens(&["example.com".to_string()], &cfg),
        vec!["example.com".to_string()]
    );
    // token 解析出实例配置值。
    assert_eq!(
        render_egress_tokens(&["{{config.base_host}}".to_string()], &cfg),
        vec!["jira.example.com".to_string()]
    );
    // 配置缺席 = 条目丢弃（allowlist 可被清空 = 出站能力整体关闭）。
    assert!(
        render_egress_tokens(&["{{config.absent}}".to_string()], &cfg).is_empty(),
        "unresolvable token must drop the entry, not pass it through"
    );
    // 非整条目 `{{` 形态防御性丢弃（manifest validate 已 loud 拒）。
    assert!(render_egress_tokens(&["{{config.base_host}}/api".to_string()], &cfg).is_empty());
    // 混合条目：字面保留 + token 解析 + 缺席丢弃，逐条独立裁决。
    assert_eq!(
        render_egress_tokens(
            &[
                "a.com".to_string(),
                "{{config.base_host}}".to_string(),
                "{{config.absent}}".to_string(),
            ],
            &cfg
        ),
        vec!["a.com".to_string(), "jira.example.com".to_string()]
    );
    // 空配置 = 全 token 条目清空。
    assert!(
        render_egress_tokens(&["{{config.base_host}}".to_string()], &BTreeMap::new()).is_empty()
    );

    // URL 形态配置值提取 host:port（demo 实证缺口：自持故事
    // 操作员贴的自然是完整 URL——scheme 前缀混进 host 段会让 allowlisted
    // 永不匹配）。
    cfg.insert("base_url".to_string(), "http://127.0.0.1:46395".to_string());
    assert_eq!(
        render_egress_tokens(&["{{config.base_url}}".to_string()], &cfg),
        vec!["127.0.0.1:46395".to_string()]
    );
    // 无显式端口 → 裸 host 条目（scheme 默认端口不钉）。
    cfg.insert("base_url".to_string(), "https://jira.corp.com".to_string());
    assert_eq!(
        render_egress_tokens(&["{{config.base_url}}".to_string()], &cfg),
        vec!["jira.corp.com".to_string()]
    );
    // URL 带路径 + 首尾空白（粘贴形态）→ 同样提取。
    cfg.insert(
        "base_url".to_string(),
        " http://10.0.0.9:8123/rest/api/2/ ".to_string(),
    );
    assert_eq!(
        render_egress_tokens(&["{{config.base_url}}".to_string()], &cfg),
        vec!["10.0.0.9:8123".to_string()]
    );
    // 含:// 但解析失败（空 host）= 条目丢弃。
    cfg.insert("base_url".to_string(), "http://:46395".to_string());
    assert!(render_egress_tokens(&["{{config.base_url}}".to_string()], &cfg).is_empty());
    // 空值 = 条目丢弃（未配置语义，deny-by-default）。
    cfg.insert("base_url".to_string(), String::new());
    assert!(render_egress_tokens(&["{{config.base_url}}".to_string()], &cfg).is_empty());
    // IPv6 无显式端口 → 裸 host（与请求侧 host_str 去括号形态一致，精确
    // 臂命中）；带显式端口条目语法无法表达 → 丢弃从严。
    cfg.insert("base_url".to_string(), "http://[::1]/".to_string());
    assert_eq!(
        render_egress_tokens(&["{{config.base_url}}".to_string()], &cfg),
        vec!["::1".to_string()]
    );
    cfg.insert("base_url".to_string(), "http://[::1]:8080/".to_string());
    assert!(render_egress_tokens(&["{{config.base_url}}".to_string()], &cfg).is_empty());
}

// ---------------------------------------------------------------------------
// 组合 world 批：register 的 step 元数据闸矩阵。组合形态
// （kind=panel 双导出 flow-step）的组件同报 step 快照——注册面按快照认，
// manifest 仍是 kind=panel。
// ---------------------------------------------------------------------------

#[test]
fn register_step_meta_gate_matrix() {
    use nemesis_plugins_wasm::PluginTrustState;
    use nemesis_plugins_wasm::manifest::PluginManifest;
    use nemesis_plugins_wasm::registry::StepMetaSnapshot;

    let (_ws, mgr) = fresh_manager();
    let parse = |slug: &str, kind: &str| {
        let raw = format!(
            "wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"{slug}\"\nkind = \"{kind}\"\n\
             name = \"{slug}\"\nversion = \"0.1.0\"\ndescription = \"d\"\n",
            "a".repeat(64)
        );
        PluginManifest::parse(&raw).expect("parse").0
    };

    // ① kind=Panel + Some(step_meta)：组合形态接受，快照进注册表。
    let reg = mgr
        .register(
            parse("combo", "panel"),
            PluginTrustState::ReviewRequired,
            1,
            None,
            None,
            Some(StepMetaSnapshot {
                kinds: vec!["fetch".into()],
            }),
        )
        .expect("panel + step_meta accepted");
    assert_eq!(
        reg.step_meta.as_ref().expect("step meta").kinds,
        vec!["fetch"]
    );

    // ② kind=Observer + Some(step_meta)：非 flow-step 载 step 元数据 = 拒
    //    （observer 可过 tool/policy 闸，正好命中 step 闸的 kind 检查；
    //    tool kind 会被更靠前的「缺 tool 快照」闸先拦，打不到这层）。
    match mgr.register(
        parse("misfit", "observer"),
        PluginTrustState::ReviewRequired,
        1,
        None,
        None,
        Some(StepMetaSnapshot {
            kinds: vec!["fetch".into()],
        }),
    ) {
        Err(PluginError::Manifest(m)) => assert!(
            m.contains("step metadata provided for non-flow-step plugin"),
            "{m}"
        ),
        Err(other) => panic!("observer + step_meta rejected with unexpected error: {other:?}"),
        Ok(_) => panic!("observer + step_meta must be rejected"),
    }

    // ③ kind=FlowStep + None：缺快照 = 对账拒。
    match mgr.register(
        parse("barestep", "flow-step"),
        PluginTrustState::ReviewRequired,
        1,
        None,
        None,
        None,
    ) {
        Err(PluginError::MetadataMismatch(m)) => {
            assert!(
                m.contains("flow-step plugin missing metadata snapshot"),
                "{m}"
            )
        }
        Err(other) => {
            panic!("flow-step without snapshot rejected with unexpected error: {other:?}")
        }
        Ok(_) => panic!("flow-step without snapshot must be rejected"),
    }

    // ④ kind=Panel + None：纯面板合法（无步骤面）。
    let reg = mgr
        .register(
            parse("plain-panel", "panel"),
            PluginTrustState::ReviewRequired,
            1,
            None,
            None,
            None,
        )
        .expect("pure panel accepted");
    assert!(reg.step_meta.is_none(), "纯面板不得带 step 快照");
}

// 横向排查定性钉（非缺陷修复）：三方开发者以记事本
// （带 BOM UTF-8）手写 plugin.toml 后，manifest 装载照常解析（canonical
// loader 单点：load_from_dir）。实测 toml crate parse 容忍前导 BOM——
// 本测试把容忍行为钉死，crate 升级若翻转此处先行红出来。
#[test]
fn bom_prefixed_manifest_parses_via_load_from_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = format!(
        "\u{feff}wasm-sha256 = \"{}\"\napi-version = 1\nslug = \"bom-plugin\"\nkind = \"tool\"\n\
         name = \"Demo\"\nversion = \"0.1.0\"\ndescription = \"demo\"\nwasm = \"bom-plugin.wasm\"\n\
         \n[config-schema]\ngreeting = \"string\"\n",
        "a".repeat(64)
    );
    std::fs::write(dir.path().join("plugin.toml"), manifest).expect("write manifest");
    let (m, _) = PluginManifest::load_from_dir(dir.path()).expect("BOM 前缀不得拒绝合法 manifest");
    assert_eq!(m.slug, "bom-plugin");
}
