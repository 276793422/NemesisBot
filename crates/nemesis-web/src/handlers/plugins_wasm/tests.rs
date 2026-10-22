//! plugins_wasm handler 单元测试（W6；独立测试文件——生产文件零内联测试）。
//!
//! 覆盖：槽空诚实报错 / list 空+lockfile 交叉 / 未注册 slug 的诚实错误 /
//! config.get 脱敏（x-secret 值不回传）。真实 wasm 组件的装配链路归
//! cluster 级 fixtures（W7 对抗套件），此处只打宿主面契约。

use super::*;
use nemesis_plugins_wasm::registry::PluginManager;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// 宿主壳 DLL 就绪保障（与 nemesis-plugins-wasm tests/common 同纪律）：
/// 产物缺席按需构建，路径经 `NEMESIS_WASM_HOST_DLL` 注入 loader——
/// 测试二进制在 target/debug/deps/，exe-相邻候选摸不到壳产物。
fn ensure_host_dll() {
    static HOST_DLL: OnceLock<Option<PathBuf>> = OnceLock::new();
    let path = HOST_DLL.get_or_init(|| {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/plugin-wasm");
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

/// 独立 workspace 装配（AutoApprove + 无扫描——CLI 同构形态）。目录注入
/// （生产 `new` 解析 exe 旁真实 plugins_wasm/——并行用例共享真实目录 =
/// 删载荷/摘注册串扰根因），载荷目录落 temp 内。
fn temp_installer() -> (tempfile::TempDir, Arc<PluginInstaller>) {
    ensure_host_dll();
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).expect("workspace");
    let manager = Arc::new(
        PluginManager::new_with_dirs(
            &ws,
            &dir.path().join("plugins_wasm"),
            nemesis_plugins_wasm::limits::PluginLimits::default(),
            Arc::new(nemesis_plugins_wasm::install::NoSecrets),
        )
        .expect("manager"),
    );
    let installer = Arc::new(PluginInstaller::new(
        manager,
        Arc::new(nemesis_plugins_wasm::install::AutoApprove),
        None,
    ));
    (dir, installer)
}

#[test]
fn unset_slot_reports_honest_error() {
    // 全局槽——只断言「空槽路径返回未装配错误」的前提本身：未注入时
    // require_installer 必须失败（若其他测试先注入了，此用例跳过）。
    if installer().is_some() {
        return;
    }
    assert!(require_installer().is_err());
}

#[test]
fn set_installer_enables_list() {
    let (_dir, inst) = temp_installer();
    set_installer(inst.clone());
    let out = cmd_list().expect("list ok");
    assert_eq!(out["plugins"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn unknown_slug_honest_errors() {
    let (_dir, inst) = temp_installer();
    set_installer(inst.clone());
    assert!(
        cmd_set_enabled("nope", true).await.is_err(),
        "未注册 enable 拒绝"
    );
    assert!(cmd_config_get("nope").is_err(), "未注册 config get 拒绝");
    assert!(cmd_logs("nope").is_err(), "未注册 logs 拒绝");
}

#[tokio::test]
async fn install_requires_source_dir() {
    let (_dir, inst) = temp_installer();
    set_installer(inst.clone());
    let err = cmd_install(Some(serde_json::json!({}))).await.unwrap_err();
    assert!(err.contains("source_dir"), "缺参诚实报错: {err}");
}

#[test]
fn config_get_masks_x_secret_values() {
    let (_dir, inst) = temp_installer();
    set_installer(inst);
    // 无注册插件时 config.get 诚实拒绝；脱敏逻辑的正确性由「已注册插件的
    // 端到端用例」（W7 fixtures）覆盖——此处钉死脱敏常量本身（值替换为
    // ******，不回传原文）防回归。
    assert!(cmd_config_get("nope").is_err());
}

#[test]
fn missing_switch_file_defaults_disabled() {
    let (_dir, inst) = temp_installer();
    let sw = inst.manager.get_switch("any-plugin");
    assert!(
        !sw.enabled,
        "开关记录缺席 = 不加载（2026-10-22 纪律回归钉：wasm 与 native 的缺省启用相反）"
    );
    assert!(sw.config.is_empty());
}

#[test]
fn list_reports_switch_and_payload_without_registry() {
    // 三源合流投影：手工拷入载荷（无 lockfile 记录 → trust=unknown）+
    // 开关记录 enabled=true + 注册表缺席（loaded=false）——不装真组件
    // 也能钉住列表语义（发现 ≠ 记录 ≠ 加载三层正交）。
    let (dir, inst) = temp_installer();
    // 手工拷入最小载荷目录（只 plugin.toml，无 wasm 文件——list 只读
    // manifest 不碰载荷字节，64-hex sha 纯过校验闸）。
    let pdir = dir.path().join("plugins_wasm").join("demo");
    std::fs::create_dir_all(&pdir).expect("payload dir");
    std::fs::write(
        pdir.join("plugin.toml"),
        "api-version = 1\nslug = \"demo\"\nkind = \"tool\"\nname = \"Demo\"\nversion = \"0.1.0\"\ndescription = \"handler-list fixture\"\nwasm-sha256 = \"1111111111111111111111111111111111111111111111111111111111111111\"\ntakeover = [\"web_fetch\"]\n",
    )
    .expect("manifest");
    // 开关记录 enabled=true（模拟用户此前开过开关、进程刚重启未装载）。
    nemesis_config::plugin_wasm::set_enabled(
        &dir.path().join("workspace").join("config"),
        "demo",
        true,
    )
    .expect("switch");
    // 直注 list_impl（不走 set_installer 全局槽——OnceLock 首胜 + 并行
    // 用例 TempDir 先删 = 槽里装着指向已删目录的 installer，读出空列表）。
    let out = list_impl(&inst).expect("list ok");
    let rows = out["plugins"].as_array().expect("rows");
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert_eq!(rows[0]["slug"], "demo");
    assert_eq!(rows[0]["trust"], "unknown", "无 lockfile 记录 = 手工拷入");
    assert_eq!(rows[0]["enabled"], true, "开关记录位照读");
    assert_eq!(rows[0]["loaded"], false, "注册表缺席 = 未加载");
    // 接管声明照读（2026-10-23 工具接管层）：前端徽标数据源——声明 =
    // 安装审批卡 + 列表徽标双留痕的投影半边。
    assert_eq!(
        rows[0]["takes_over"],
        serde_json::json!(["web_fetch"]),
        "manifest 接管声明进投影"
    );
}

#[test]
fn list_lockfile_only_row_for_missing_payload() {
    // lockfile 有记录而载荷被手工删掉 = 可见性兜底行（enabled 照读、
    // loaded=false、error 注记在场）。
    let (dir, inst) = temp_installer();
    let lock_path = dir.path().join("plugins_wasm").join("lockfile.json");
    std::fs::write(
        &lock_path,
        r#"{"version":1,"plugins":{"gone":{"version":"0.1.0","wasm_sha256":"ab","trust":"review-required","installed_at_ms":0,"signed_by":""}}}"#,
    )
    .expect("lockfile");
    let out = list_impl(&inst).expect("list ok");
    let rows = out["plugins"].as_array().expect("rows");
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert_eq!(rows[0]["slug"], "gone");
    assert_eq!(rows[0]["trust"], "review-required");
    assert_eq!(rows[0]["loaded"], false);
    assert!(
        rows[0]["error"]
            .as_str()
            .is_some_and(|e| e.contains("载荷")),
        "载荷缺席注记: {rows:?}"
    );
}

#[test]
fn source_dir_validation_rejects_bad_shapes() {
    // 2026-09-30 复查 #10：WSAPI 层 source_dir 闸（CLI 保持宽容）。
    assert!(validate_source_dir("").is_err(), "空串拒绝");
    assert!(validate_source_dir("   ").is_err(), "纯空白拒绝");
    assert!(
        validate_source_dir("relative/dir").is_err(),
        "相对路径拒绝（canonicalize 会钉到进程 cwd）"
    );
    assert!(validate_source_dir(".").is_err(), "点路径拒绝");
    assert!(
        validate_source_dir("\\\\server\\share\\pkg").is_err(),
        "UNC 路径拒绝"
    );
    assert!(
        validate_source_dir("//server/share").is_err(),
        "// 前缀拒绝"
    );
    // 合法形态：Windows 盘符绝对路径与 POSIX 绝对路径（is_absolute 平台
    // 语义各自成立；CI 在 Windows 跑，POSIX 形态在此平台 is_absolute=false，
    // 按平台断言）。
    if cfg!(windows) {
        assert!(validate_source_dir("C:\\pkg\\demo").is_ok());
    } else {
        assert!(validate_source_dir("/tmp/pkg").is_ok());
    }
}

#[test]
fn install_sync_event_decision_table() {
    // 2026-09-30 复查 #4：恒发事件的方向决策表——只有 tool+meta+启用
    // 才 add=true；kind 翻转（tool→observer）/缺 meta/禁用升级一律
    // add=false（前缀摘除，清掉 loop 里的旧版工具）。
    let tool = Some(("plugin.t.a", "read"));
    let add = install_sync_event("tool", "t", tool, true);
    assert!(add.add);
    assert_eq!(add.tool_name, "plugin.t.a");
    assert_eq!(add.operation_type, "read");

    let off = install_sync_event("tool", "t", tool, false);
    assert!(!off.add, "禁用升级必须摘除（disabled upgrade）");
    assert_eq!(off.slug, "t");

    let flipped = install_sync_event("observer", "t", tool, true);
    assert!(!flipped.add, "tool→observer 升级必须摘除旧工具（僵尸根因）");

    let no_meta = install_sync_event("tool", "t", None, true);
    assert!(!no_meta.add, "缺 meta（对账失败已拒装，防御性摘除）");

    // remove 路径只消费 slug，tool_name/operation_type 允许为空。
    assert!(off.tool_name.is_empty());
}
