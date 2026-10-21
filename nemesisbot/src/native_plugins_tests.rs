//! native_plugins 单测（临时目录隔离；签名闸三态随 security.signature_verify
//! 配置 + 编译期锚走——本仓库 certs/root_cert.der 入库 = build.rs 恒注入锚，
//! 开发/测试构建缺省落 **warn** 态；测试对锚形态做确定性分支断言而非假设
//! 休眠）。

use crate::native_plugins::{GateOutcome, gate_load, scan_unrecognized, status_list};
use nemesis_config::plugins_native::{NativePluginId, set_enabled};
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("nb_native_plugins_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 编译期事实 → 清单应报的签名 mode（锚在位 + verify 编译 = warn 缺省；
/// 否则 dormant）。与 signature_gate_mode 的输入同源，但期望值由测试独立
/// 推导（非回读实现）。
fn expected_default_mode() -> &'static str {
    #[cfg(feature = "verify")]
    {
        if crate::verify_policy::ROOT_ANCHOR.is_some() {
            "warn"
        } else {
            "dormant"
        }
    }
    #[cfg(not(feature = "verify"))]
    {
        "dormant"
    }
}

/// 开关关 = DisabledBySwitch，与签名形态无关（确定性分支）。
#[test]
fn gate_disabled_by_switch() {
    let config = temp_dir("switch_off");
    set_enabled(&config, NativePluginId::PluginUi, false).unwrap();
    assert_eq!(
        gate_load(&config, NativePluginId::PluginUi),
        GateOutcome::DisabledBySwitch
    );
    let _ = std::fs::remove_dir_all(&config);
}

/// 开关开 + 库文件缺席 = Allow（闸只管「找到了装不装」，缺席归装载点日志；
/// warn/enforce 态下缺席同样放行——拒绝发生在装载点的降级路径）。
#[test]
fn gate_allows_when_library_absent() {
    let config = temp_dir("absent");
    assert_eq!(
        gate_load(&config, NativePluginId::PluginWasm),
        GateOutcome::Allow
    );
    let _ = std::fs::remove_dir_all(&config);
}

/// 未知库扫描：只列平台库扩展名、排除四插件已知名（含连字符变体）、显式排序；
/// 非 DLL/SO/dylib 一律忽略。
#[test]
fn scan_unrecognized_filters_and_sorts() {
    let exe_dir = temp_dir("scan");
    let plugins = exe_dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    for name in [
        "plugin_onnx.dll",
        "plugin-ui.dll", // 连字符变体（已知）
        "rogue_tool.dll",
        "aaa_first.so",
        "zzz_last.dylib",
        "readme.txt",           // 非库扩展，忽略
        "eval_monitor_dll.dll", // 已知
        "plugin_wasm.dll",      // 已知
    ] {
        std::fs::write(plugins.join(name), b"x").unwrap();
    }
    std::fs::create_dir_all(plugins.join("subdir.dll")).unwrap(); // 目录不算
    let out = scan_unrecognized(&exe_dir);
    let names: Vec<&str> = out.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["aaa_first.so", "rogue_tool.dll", "zzz_last.dylib"]
    );
    assert!(out[0].path.contains("plugins"));
    let _ = std::fs::remove_dir_all(&exe_dir);
}

/// 空目录 / 缺目录 = 空清单，不炸。
#[test]
fn scan_unrecognized_tolerates_missing_dir() {
    let exe_dir = temp_dir("no_plugins_dir");
    assert!(scan_unrecognized(&exe_dir).is_empty());
    let _ = std::fs::remove_dir_all(&exe_dir);
}

/// 清单：目录身份全量 + found=false（临时 exe_dir 无库）+ 开关连通 + 即时生效
/// 标记 + 签名 mode 跟随编译期锚形态（无 security 配置 = warn 缺省 / 休眠）。
#[test]
fn status_list_catalog_and_switch() {
    let config = temp_dir("status");
    set_enabled(&config, NativePluginId::EvalMonitor, false).unwrap();
    let exe_dir = temp_dir("status_exe");
    let list = status_list(&config, &exe_dir);
    assert_eq!(list.len(), NativePluginId::ALL.len());
    let expected_mode = expected_default_mode();
    for entry in &list {
        assert!(!entry.found, "{} 临时目录不该找到库", entry.id);
        assert!(entry.path.is_none());
        assert_eq!(entry.signature.mode, expected_mode, "{}", entry.id);
        let expected_enabled = entry.id != "eval_monitor_dll";
        assert_eq!(entry.enabled, expected_enabled);
        assert_eq!(entry.immediate_effect, entry.id == "eval_monitor_dll");
    }
    let _ = std::fs::remove_dir_all(&config);
    let _ = std::fs::remove_dir_all(&exe_dir);
}

/// 三态跟随 security.signature_verify 配置：off → dormant；warn/缺省 → warn；
/// enforce → enforce；非法值 → 保守 warn（启动自验处 loud 拒绝）。无锚构建
/// 恒 dormant（确定性分支）。
#[test]
fn signature_gate_mode_follows_security_config_tri_state() {
    let config = temp_dir("tri_state");
    let write_security = |value: &str| {
        std::fs::write(
            config.join("config.security.json"),
            format!(r#"{{ "signature_verify": {value} }}"#),
        )
        .unwrap();
    };
    #[cfg(feature = "verify")]
    {
        use crate::native_plugins::signature_gate_mode_for_test;
        if crate::verify_policy::ROOT_ANCHOR.is_none() {
            // 无锚：任何配置都休眠（验签能力缺席）。
            write_security(r#""enforce""#);
            assert_eq!(signature_gate_mode_for_test(&config), "dormant");
        } else {
            write_security(r#""enforce""#);
            assert_eq!(signature_gate_mode_for_test(&config), "enforced");
            write_security(r#""off""#);
            assert_eq!(signature_gate_mode_for_test(&config), "dormant");
            write_security(r#""warn""#);
            assert_eq!(signature_gate_mode_for_test(&config), "warn");
            write_security(r#""bogus""#);
            assert_eq!(signature_gate_mode_for_test(&config), "warn");
        }
    }
    #[cfg(not(feature = "verify"))]
    {
        write_security(r#""enforce""#);
        // 无 verify 编译：清单 mode 恒 dormant（signature_status 短路）。
        let exe_dir = temp_dir("tri_state_exe");
        let list = status_list(&config, &exe_dir);
        assert!(list.iter().all(|e| e.signature.mode == "dormant"));
        let _ = std::fs::remove_dir_all(&exe_dir);
    }
    let _ = std::fs::remove_dir_all(&config);
}
