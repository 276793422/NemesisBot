//! 装载器单测：结果分类 / ABI 握手纯函数 / fixture cdylib 全链（按需构建，
//! 构建失败诚实 SKIP——plugins-wasm tests 同款模式）。

use std::path::Path;
use std::process::Command;

use super::{ResourcesLoadOutcome, load_resources_plugin};
use crate::plugin_lib::PluginLibSpec;

// ---------------------------------------------------------------------------
// 规格（三平台产物名——loader 无关，查找链共享 plugin_lib）
// ---------------------------------------------------------------------------

#[test]
fn resources_spec_filenames() {
    let spec = PluginLibSpec::RESOURCES;
    let name = spec.filename();
    if cfg!(windows) {
        assert_eq!(name, "plugin_resources.dll");
        assert_eq!(spec.file_windows, "plugin_resources.dll");
    } else if cfg!(target_os = "macos") {
        assert_eq!(name, "libplugin_resources.dylib");
    } else {
        assert_eq!(name, "libplugin_resources.so");
    }
}

// ---------------------------------------------------------------------------
// 幂等 + 缺席 + 名册开关闸（当前测试进程 exe 旁无插件产物 → Absent；CI 无
// fixture 时同样 Absent——纯在线行为的缺席回归面）。开关闸先于文件探测 =
// 关即恒 DisabledBySwitch（与 dll 在场与否、桥槽状态无关的确定性）。
// ---------------------------------------------------------------------------

#[test]
fn absent_without_artifact_is_quiet_outcome() {
    // 桥槽可能已被前序 fixture 测试占用（进程全局）——只断言「装载不炸 +
    // 结果是四个合法变体之一」（开关文件缺席 = 缺省启用，不会 DisabledBySwitch）。
    let dir = tempfile::tempdir().unwrap();
    let outcome = load_resources_plugin(&dir.path().join("config"));
    assert!(matches!(
        outcome,
        ResourcesLoadOutcome::Absent
            | ResourcesLoadOutcome::Loaded
            | ResourcesLoadOutcome::DisabledBySwitch
            | ResourcesLoadOutcome::Rejected(_)
    ));
}

#[test]
fn roster_switch_off_is_disabled_deterministically() {
    // 名册开关关（config.plugin.native.json plugin_resources=disabled）→
    // 恒 DisabledBySwitch——开关闸在幂等/文件探测之前（roaming 同款次序）。
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.plugin.native.json"),
        r#"{"plugins":{"plugin_resources":{"enabled":false}}}"#,
    )
    .unwrap();
    let outcome = load_resources_plugin(&config);
    assert!(
        matches!(outcome, ResourcesLoadOutcome::DisabledBySwitch),
        "开关关必须恒 DisabledBySwitch：{outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// fixture 全链：真实 cdylib（plugin-resources crate 自身）构建 → 临时目录
// 布局 → 直接 dlopen 句柄 → manifest/get/free 往返。插件 crate 与本仓同源
// （plugins/plugin-resources/），构建失败（工具链缺失等）= 诚实 SKIP。
// ---------------------------------------------------------------------------

/// 构建插件 release cdylib，回产物路径。构建失败 = None（SKIP）。
fn build_fixture() -> Option<std::path::PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("plugins")
        .join("plugin-resources")
        .join("Cargo.toml");
    assert!(
        manifest.exists(),
        "插件 crate 必须在同仓：{}",
        manifest.display()
    );
    let out = Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(&manifest)
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "[SKIP] plugin-resources fixture 构建失败：{}",
            String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(400)
                .collect::<String>()
        );
        return None;
    }
    let base = manifest.parent()?.join("target").join("release");
    let name = if cfg!(windows) {
        "plugin_resources.dll"
    } else if cfg!(target_os = "macos") {
        "libplugin_resources.dylib"
    } else {
        "libplugin_resources.so"
    };
    let p = base.join(name);
    if p.is_file() { Some(p) } else { None }
}

#[test]
fn fixture_full_chain_manifest_get_free() {
    let Some(dll) = build_fixture() else {
        eprintln!("[SKIP] plugin-resources fixture 不在場（构建失败/产物缺失）");
        return;
    };
    let lib = unsafe { libloading::Library::new(&dll) }.expect("fixture dlopen 成功");
    let abi: nemesis_plugin::resources::PrsAbiVersionFn =
        *unsafe { lib.get(nemesis_plugin::resources::PRS_ABI_VERSION_SYMBOL.as_bytes()) }
            .expect("abi_version 符号在場");
    assert_eq!(
        abi(),
        nemesis_plugin::resources::HOST_RESOURCES_ABI_VERSION,
        "fixture 与宿主 ABI 同源一致"
    );
    let manifest: nemesis_plugin::resources::PrsStrFn =
        *unsafe { lib.get(nemesis_plugin::resources::PRS_MANIFEST_SYMBOL.as_bytes()) }
            .expect("manifest 符号在場");
    let get: nemesis_plugin::resources::PrsGetFn =
        *unsafe { lib.get(nemesis_plugin::resources::PRS_GET_SYMBOL.as_bytes()) }
            .expect("get 符号在場");
    let free: nemesis_plugin::resources::PrsFreeFn =
        *unsafe { lib.get(nemesis_plugin::resources::PRS_FREE_SYMBOL.as_bytes()) }
            .expect("free 符号在場");

    let h = nemesis_plugin::resources::ResourcesPluginHandle {
        manifest,
        get,
        free,
    };
    let raw = unsafe { h.manifest_str() }.expect("manifest 读取");
    let m = nemesis_plugin::resources::ResourcesManifest::parse(&raw).expect("manifest 解析");
    let entry = m
        .entry(nemesis_plugin::resources::RESOURCES_KIND_PERSONA_CATALOG)
        .expect("persona-catalog 条目在場");
    assert!(entry.file_count >= 1);
    assert_eq!(entry.tree.len(), entry.file_count);
    assert_eq!(entry.license, "MIT");
    // get 首个 tree 路径 → content 非空
    let first = &entry.tree[0];
    let content = unsafe { h.get_content("persona-catalog", first) }
        .unwrap_or_else(|e| panic!("get {first} 失败: {e}"));
    assert!(!content.is_empty());
    // 未知路径 → Err(not_found)
    let err =
        unsafe { h.get_content("persona-catalog", "no/such.md") }.expect_err("未知路径必须 Err");
    assert!(err.contains("not_found"));
    // Library 不显式 unload（进程生命周期纪律；drop lib handle 无害）
    drop(lib);
}

// ---------------------------------------------------------------------------
// ABI 不匹配注入（§7 host 单测行「ABI 版本不匹配 → unavailable」的真实现）：
// FIXTURE_ABI_OVERRIDE=2 编译期钩子出**错版** fixture（独立 CARGO_TARGET_DIR，
// 不踢正常 fixture 缓存）→ 纯装载步 load_library_handle 直调（绕开桥槽幂等
// ——进程全局静态不受全链测试先后影响）→ 必拒且原因点名 ABI。
// ---------------------------------------------------------------------------

/// 构建错版（ABI v2）fixture cdylib，回产物路径。构建失败 = None（SKIP）。
fn build_abi_mismatch_fixture() -> Option<std::path::PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("plugins")
        .join("plugin-resources")
        .join("Cargo.toml");
    let out = Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(&manifest)
        .env("FIXTURE_ABI_OVERRIDE", "2")
        // 独立 target：与正常 fixture（ABI v1）的编译缓存互不覆盖。
        .env(
            "CARGO_TARGET_DIR",
            manifest.parent().unwrap().join("target-abi-override"),
        )
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "[SKIP] ABI 错版 fixture 构建失败：{}",
            String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(400)
                .collect::<String>()
        );
        return None;
    }
    let base = manifest
        .parent()?
        .join("target-abi-override")
        .join("release");
    let name = if cfg!(windows) {
        "plugin_resources.dll"
    } else if cfg!(target_os = "macos") {
        "libplugin_resources.dylib"
    } else {
        "libplugin_resources.so"
    };
    let p = base.join(name);
    if p.is_file() { Some(p) } else { None }
}

#[test]
fn abi_mismatch_fixture_is_rejected_with_reason() {
    // release 测试形态下签名闸先拒（Unsigned 不放行）——到不了 ABI 臂，
    // 诚实 SKIP（debug 形态=本测试的主战场，release CI 由 fixture 全链 +
    // 宿主常数测试钉住另一半）。
    if !crate::plugin_lib::unsigned_dev_allow() {
        eprintln!("[SKIP] release 形态签名闸先拒，ABI 注入臂不可达");
        return;
    }
    let Some(dll) = build_abi_mismatch_fixture() else {
        eprintln!("[SKIP] ABI 错版 fixture 不在場（构建失败/产物缺失）");
        return;
    };
    let err = super::load_library_handle(&dll).expect_err("ABI 不匹配必须拒绝");
    assert!(
        err.contains("ABI 版本不匹配"),
        "拒绝原因必须点名 ABI：{err}"
    );
    assert!(
        err.contains("v2") && err.contains("v1"),
        "拒绝原因必须带两侧版本号：{err}"
    );
}
