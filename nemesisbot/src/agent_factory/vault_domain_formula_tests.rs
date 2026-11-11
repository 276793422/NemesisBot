//! L26 路径分裂修复（2026-11-11）公式对齐锁：dispatch 闸的绑定路径
//! （装配点 `SharedResources::vault_domain_config_path`）与 CLI `vault
//! bind` 写入路径（`commands::vault::vault_domain_config_path`）必须对
//! 同一 home 推导出**同一个文件**——BUG 实录：两侧各拼各的路径，绑定在
//! 生产永不生效。无 feature 门（纯路径公式，默认构建必跑）。

use super::SharedResources;
use std::path::PathBuf;
use std::sync::Arc;

fn make_shared(home: &std::path::Path) -> Arc<SharedResources> {
    let (outbound_tx, _rx) = tokio::sync::mpsc::channel(16);
    Arc::new(SharedResources {
        home: home.to_path_buf(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        ..Default::default()
    })
}

/// 同一 home 下闸读路径 == CLI 写路径（单一文件，写读两侧不再各奔东西）。
#[test]
fn gate_path_matches_cli_bind_path_for_same_home() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();

    let gate = make_shared(&home).vault_domain_config_path();
    let cli = crate::commands::vault::vault_domain_config_path(&home).expect("cli formula");

    assert_eq!(gate, cli, "gate and CLI must resolve the same sidecar file");
    // 形状也钉住：<home>/workspace/config/config.vault.json。
    assert_eq!(
        gate,
        home.join("workspace")
            .join("config")
            .join(nemesis_config::VAULT_DOMAIN_CONFIG_FILE)
    );
}

/// workspace 显式覆盖时闸锚到**执行根**的 config 目录（workspace_dir
/// 语义，headless `run --workspace` 同罩）；CLI bind 仍锚 home 标准工作区
/// ——两侧各自跟随本表面的工作区根，属刻意分工，勿「修直」。
#[test]
fn workspace_override_anchors_gate_to_override_config_dir() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let override_ws = tmp.path().join("proj");
    std::fs::create_dir_all(&override_ws).unwrap();

    let (outbound_tx, _rx) = tokio::sync::mpsc::channel(16);
    let shared = SharedResources {
        home: home.clone(),
        workspace: override_ws.clone(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        ..Default::default()
    };

    let gate = shared.vault_domain_config_path();
    assert_eq!(
        gate,
        nemesis_path::workspace_config_dir(&override_ws)
            .join(nemesis_config::VAULT_DOMAIN_CONFIG_FILE),
        "gate follows the execution root when workspace override is set"
    );
    assert_ne!(
        gate,
        PathBuf::from(&home)
            .join("workspace")
            .join("config")
            .join(nemesis_config::VAULT_DOMAIN_CONFIG_FILE),
        "override must not silently fall back to the home anchor"
    );
}
