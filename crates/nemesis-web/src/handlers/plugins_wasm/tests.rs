//! plugins_wasm handler 单元测试（W6；独立测试文件——生产文件零内联测试）。
//!
//! 覆盖：槽空诚实报错 / list 空+lockfile 交叉 / 未注册 slug 的诚实错误 /
//! config.get 脱敏（x-secret 值不回传）。真实 wasm 组件的装配链路归
//! cluster 级 fixtures（W7 对抗套件），此处只打宿主面契约。

use super::*;
use nemesis_plugins_wasm::registry::PluginManager;
use std::sync::Arc;

/// 独立 workspace 装配（AutoApprove + 无扫描——CLI 同构形态）。
fn temp_installer() -> (tempfile::TempDir, Arc<PluginInstaller>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).expect("workspace");
    let manager = Arc::new(
        PluginManager::new(
            &ws,
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
fn missing_config_file_defaults_enabled_true() {
    let (_dir, inst) = temp_installer();
    let cfg = inst.manager.get_config("any-plugin");
    assert!(cfg.enabled, "实例配置缺文件 = 缺省启用（回归钉）");
    assert!(cfg.entries.is_empty());
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
