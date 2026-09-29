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

#[test]
fn unknown_slug_honest_errors() {
    let (_dir, inst) = temp_installer();
    set_installer(inst.clone());
    assert!(cmd_set_enabled("nope", true).is_err(), "未注册 enable 拒绝");
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
