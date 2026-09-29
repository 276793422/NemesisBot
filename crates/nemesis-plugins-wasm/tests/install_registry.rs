//! 2026-09-29 交付审查修复的回归钉（宿主 crate 公开面）：
//! - egress 私网闸边界（H1：v4-mapped / NAT64 归一 + v4 段表）；
//! - `set_enabled_file_only`（3.2：幽灵配置拒绝 + entries 保留 + 原子写）；
//! - 实例配置损坏 fail-closed（L6）；
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

struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

fn fresh_manager() -> (tempfile::TempDir, Arc<PluginManager>) {
    let ws = tempfile::tempdir().expect("tempdir");
    let mgr = Arc::new(
        PluginManager::new(ws.path(), PluginLimits::default(), Arc::new(NoSecrets))
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
// 3.2 / L6：实例配置写路径（set_enabled_file_only 单一真相源）
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

#[test]
fn set_enabled_file_only_ghost_rejected() {
    let (_ws, mgr) = fresh_manager();
    let err = mgr
        .set_enabled_file_only("never-installed", true)
        .expect_err("ghost slug must be rejected");
    assert!(matches!(err, PluginError::NotAvailable(_)), "got: {err:?}");

    let err = mgr
        .set_enabled_file_only("../escape", true)
        .expect_err("invalid slug must be rejected");
    assert!(matches!(err, PluginError::Manifest(_)), "got: {err:?}");
}

#[test]
fn set_enabled_file_only_writes_and_preserves_entries() {
    let (_ws, mgr) = fresh_manager();
    seed_lockfile(&mgr, "demo");
    // 预置实例配置（enabled=true + 业务 entries）。
    let cfg_path = mgr
        .plugins_dir()
        .parent()
        .expect("workspace")
        .join("config/plugins/demo.json");
    std::fs::create_dir_all(cfg_path.parent().expect("config dir")).expect("mkdir");
    std::fs::write(
        &cfg_path,
        r#"{"enabled": true, "entries": {"greeting": "hi"}}"#,
    )
    .expect("seed cfg");

    mgr.set_enabled_file_only("demo", false).expect("disable");
    let cfg = mgr.get_config("demo");
    assert!(!cfg.enabled);
    assert_eq!(cfg.entries.get("greeting").map(String::as_str), Some("hi"));

    mgr.set_enabled_file_only("demo", true).expect("enable");
    assert!(mgr.get_config("demo").enabled);
    // 无残留 tmp（原子写收尾）。
    let leftovers: Vec<_> = std::fs::read_dir(cfg_path.parent().expect("config dir"))
        .expect("read_dir")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "tmp 残留: {leftovers:?}");
}

#[test]
fn corrupt_instance_config_fails_closed() {
    let (_ws, mgr) = fresh_manager();
    // 损坏文件 → 按禁用处理（L6：存在但损坏 ≠ 缺省启用）。
    let cfg_dir = mgr
        .plugins_dir()
        .parent()
        .expect("workspace")
        .join("config/plugins");
    std::fs::create_dir_all(&cfg_dir).expect("mkdir");
    std::fs::write(cfg_dir.join("broken.json"), "{not json").expect("seed corrupt");
    assert!(
        !mgr.get_config("broken").enabled,
        "损坏配置必须 fail-closed"
    );
    // 不存在 = 全新安装语义（缺省启用）。
    assert!(mgr.get_config("fresh").enabled);
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
