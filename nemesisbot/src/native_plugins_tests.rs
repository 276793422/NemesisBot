//! native_plugins 单测（临时目录隔离；签名闸三态随 security.signature_verify
//! 配置 + 编译期锚走——本仓库 certs/root_cert.der 入库 = build.rs 恒注入锚，
//! 开发/测试构建缺省落 **warn** 态；测试对锚形态做确定性分支断言而非假设
//! 休眠）。

use crate::native_plugins::{
    GateOutcome, gate_load, gate_load_path, resolve_load_path, scan_unrecognized, status_list,
};
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
/// warn/enforce 态下缺席同样放行——拒绝发生在装载点的降级路径）。用
/// PluginUi 保证确定性（wasm 宿主壳在开发机上可能经 env/开发路径命中真
/// 产物——那也是合法的 Allow { path: Some }）。
#[test]
fn gate_allows_when_library_absent() {
    let config = temp_dir("absent");
    assert_eq!(
        gate_load(&config, NativePluginId::PluginUi),
        GateOutcome::Allow { path: None }
    );
    let _ = std::fs::remove_dir_all(&config);
}

/// R1 回归锁（三评 probe 转正）：enforce 态下 env 权威覆盖指向未签名
/// 文件 = 拒载（整改前闸只查 plugins/、查不到即 Allow，装载器却照装 env
/// 文件——验签对象 ≠ 装载对象）。warn 缺省态同文件放行且 `Allow { path }`
/// 携带的就是它（调用方按此定点装载）。
#[test]
fn enforce_blocks_unsigned_env_override_host() {
    #[cfg(all(feature = "verify", feature = "plugins-wasm"))]
    {
        if crate::verify_policy::ROOT_ANCHOR.is_none() {
            // 无锚构建恒休眠，enforce 不可达——本测试的前设不成立，诚实跳过。
            return;
        }
        let _guard = crate::WASM_HOST_DLL_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let config = temp_dir("enforce_env");
        let junk = temp_dir("enforce_env_dll").join("unsigned-review-host.dll");
        std::fs::write(&junk, b"not a signed pe").unwrap();
        // SAFETY: 进程级串行锁（WASM_HOST_DLL_LOCK，与 plugin_bridge 共享）
        // 内的 env 突变。
        unsafe {
            std::env::set_var("NEMESIS_WASM_HOST_DLL", &junk);
        }
        // enforce：env 文件未签名 → BlockedBySignature（fail-closed）。
        std::fs::write(
            config.join("config.security.json"),
            r#"{ "signature_verify": "enforce" }"#,
        )
        .unwrap();
        match gate_load(&config, NativePluginId::PluginWasm) {
            GateOutcome::BlockedBySignature(_) => {}
            other => panic!("enforce 态 env 未签名文件必须拒载，实得：{other:?}"),
        }
        // warn（缺省）：放行，且携带的路径 = env 文件本身（装载同源）。
        std::fs::remove_file(config.join("config.security.json")).unwrap();
        match gate_load(&config, NativePluginId::PluginWasm) {
            GateOutcome::Allow { path } => {
                assert_eq!(
                    path.as_deref(),
                    Some(junk.as_path()),
                    "Allow 必须携带 env 文件"
                );
            }
            other => panic!("warn 态必须放行，实得：{other:?}"),
        }
        // SAFETY: 同上，锁内清理。
        unsafe {
            std::env::remove_var("NEMESIS_WASM_HOST_DLL");
        }
        let _ = std::fs::remove_dir_all(&config);
        let _ = std::fs::remove_dir_all(junk.parent().unwrap());
    }
    #[cfg(not(all(feature = "verify", feature = "plugins-wasm")))]
    {
        // verify / plugins-wasm 未编译：签名闸休眠、wasm 装载面缺席——
        // 语义前提不成立，诚实跳过（保持编译无警告）。
        let config = temp_dir("enforce_env_skip");
        assert!(matches!(
            gate_load(&config, NativePluginId::PluginWasm),
            GateOutcome::Allow { .. }
        ));
        let _ = std::fs::remove_dir_all(&config);
    }
}

/// R1 回归锁：`gate_load_path` 对显式路径的三态行为——enforce 验的就是
/// 调用方给的那个文件（eval 监控 DLL 开发形态仓库路径受验的通道）；
/// 路径感知变体不自行选路。
#[test]
fn gate_load_path_verifies_explicit_file() {
    #[cfg(feature = "verify")]
    {
        if crate::verify_policy::ROOT_ANCHOR.is_some() {
            let config = temp_dir("path_explicit");
            let junk = temp_dir("path_explicit_dll").join("dev-shape-monitor.dll");
            std::fs::write(&junk, b"not a signed pe").unwrap();
            std::fs::write(
                config.join("config.security.json"),
                r#"{ "signature_verify": "enforce" }"#,
            )
            .unwrap();
            match gate_load_path(&config, NativePluginId::EvalMonitor, Some(&junk)) {
                GateOutcome::BlockedBySignature(_) => {}
                other => panic!("enforce 态显式未签名文件必须拒载，实得：{other:?}"),
            }
            // 路径缺席（None）= 闸不管（缺席归装载点）。
            assert_eq!(
                gate_load_path(&config, NativePluginId::EvalMonitor, None),
                GateOutcome::Allow { path: None }
            );
            let _ = std::fs::remove_dir_all(&config);
            let _ = std::fs::remove_dir_all(junk.parent().unwrap());
        }
    }
    #[cfg(not(feature = "verify"))]
    {
        let config = temp_dir("path_explicit_skip");
        let junk = temp_dir("path_explicit_dll").join("dev-shape-monitor.dll");
        std::fs::write(&junk, b"x").unwrap();
        assert!(matches!(
            gate_load_path(&config, NativePluginId::EvalMonitor, Some(&junk)),
            GateOutcome::Allow { .. }
        ));
        let _ = std::fs::remove_dir_all(&config);
        let _ = std::fs::remove_dir_all(junk.parent().unwrap());
    }
}

/// 统一解析入口：非 wasm 插件在无库环境解析为 None；解析结果与
/// `gate_load` 的 `Allow { path }` 同源（单一真相源契约）。
#[test]
fn resolve_load_path_matches_gate_allow_path() {
    let config = temp_dir("resolve_sync");
    let id = NativePluginId::PluginOnnx;
    match (resolve_load_path(id), gate_load(&config, id)) {
        (None, GateOutcome::Allow { path: None }) => {}
        (Some(p), GateOutcome::Allow { path: Some(q) }) => assert_eq!(p, q),
        (r, g) => panic!("解析与闸裁决不同源：resolve={r:?} gate={g:?}"),
    }
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
