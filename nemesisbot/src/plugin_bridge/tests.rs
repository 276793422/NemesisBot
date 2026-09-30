//! plugin_bridge 单元测试（W5-3 附属；独立测试文件——生产文件零内联测试）。

use super::*;
use nemesis_agent::r#loop::Tool as _;
use std::sync::Arc;

fn temp_home() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    // PluginManager::new 对 workspace root canonicalize——目录必须先在场。
    std::fs::create_dir_all(common::workspace_path(&home)).expect("workspace dir");
    (dir, home)
}

fn meta(name: &str, operation_type: &str, parameters_json: &str) -> ToolMetaSnapshot {
    ToolMetaSnapshot {
        name: name.to_string(),
        base: "base".to_string(),
        title: "t".to_string(),
        description: "d".to_string(),
        parameters_json: parameters_json.to_string(),
        operation_type: operation_type.to_string(),
        min_tier: String::new(),
    }
}

#[test]
fn runtime_config_defaults_when_file_missing() {
    let (_dir, home) = temp_home();
    let rc = read_runtime_config(&home);
    assert!(rc.enabled, "缺省 enabled=true");
    let d = PluginLimits::default();
    assert_eq!(rc.limits.fuel, d.fuel);
    assert_eq!(rc.limits.memory_bytes, d.memory_bytes);
    assert_eq!(rc.limits.timeout_ms, d.timeout_ms);
}

#[test]
fn runtime_config_tightening_applied_and_loosening_ignored() {
    let (_dir, home) = temp_home();
    let d = PluginLimits::default();
    let cfg = serde_json::json!({
        "plugins": { "wasm": {
            "enabled": false,
            "limits": {
                "fuel": d.fuel / 2,
                "max_memory_bytes": d.memory_bytes / 2,
                "call_timeout_secs": 5,
                "max_host_calls_per_frame": 10,
                // 放宽请求（≥ 默认）——必须被忽略
                "max_instances": d.max_instances as u64 + 10,
                "observer_queue_depth": d.observer_queue_depth as u64 + 10,
            }
        }}
    });
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(common::config_path(&home), cfg.to_string()).unwrap();
    let rc = read_runtime_config(&home);
    assert!(!rc.enabled, "显式关闭生效");
    assert_eq!(rc.limits.fuel, d.fuel / 2, "收紧生效");
    assert_eq!(rc.limits.memory_bytes, d.memory_bytes / 2);
    assert_eq!(rc.limits.timeout_ms, 5000, "秒→毫秒换算");
    assert_eq!(rc.limits.host_call_budget, 10);
    assert_eq!(rc.limits.max_instances, d.max_instances, "放宽被忽略");
    assert_eq!(rc.limits.observer_queue_depth, d.observer_queue_depth);
}

#[test]
fn operation_mapping_covers_wordlist() {
    use nemesis_security::types::OperationType;
    assert_eq!(
        PluginToolBridge::map_operation("read"),
        Some(OperationType::FileRead)
    );
    assert_eq!(
        PluginToolBridge::map_operation("write"),
        Some(OperationType::FileWrite)
    );
    assert_eq!(
        PluginToolBridge::map_operation("exec"),
        Some(OperationType::ProcessExec)
    );
    assert_eq!(
        PluginToolBridge::map_operation("network"),
        Some(OperationType::NetworkRequest)
    );
    // 未声明（空串）/未知 = None：不声明 → dispatch 未注册名 fail-closed
    // CRITICAL（WIT 合同口径，2026-09-30 #3 从紧）。大小写敏感不猜。
    assert_eq!(PluginToolBridge::map_operation(""), None);
    assert_eq!(PluginToolBridge::map_operation("filesystem"), None);
    assert_eq!(PluginToolBridge::map_operation("Read"), None);
}

#[test]
fn bridge_parameters_fallback_on_invalid_json() {
    let (_dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let bridge = PluginToolBridge::new(meta("plugin.x.base", "read", "not-json"), manager);
    let params = bridge.parameters();
    assert!(params.is_object(), "坏 JSON 回退空 object schema");
    assert!(params.get("properties").is_some());
}

#[test]
fn bridge_is_read_only_follows_declaration() {
    let (_dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let ro = PluginToolBridge::new(meta("plugin.x.a", "read", "{}"), manager.clone());
    let rw = PluginToolBridge::new(meta("plugin.x.b", "write", "{}"), manager.clone());
    let undeclared = PluginToolBridge::new(meta("plugin.x.c", "", "{}"), manager);
    assert!(ro.is_read_only());
    assert!(!rw.is_read_only());
    assert!(!undeclared.is_read_only(), "未声明不作只读（从紧）");
}

#[test]
fn bridge_min_tier_follows_manifest_snapshot() {
    // min-tier 接线（2026-09-30 #2）：桥按安装期对账快照声明最低档，
    // 数据随 meta 走（无边车表无漂移）。
    let (_dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let mut m = meta("plugin.x.a", "read", "{}");
    m.min_tier = "normal".to_string();
    let bridge = PluginToolBridge::new(m, manager);
    assert_eq!(bridge.min_tier(), Some("normal"));
}

#[tokio::test]
async fn install_approver_unbound_is_fail_closed() {
    let gate = LateInstallApprover::new(1);
    let review = InstallReview {
        slug: "demo".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
        kind: "tool".into(),
        trust_state: "review-required".into(),
        trust_level: String::new(),
        signer_key: String::new(),
        egress: vec![],
        x_secret: vec![],
        wasm_bytes: 1,
        limits_ignored: vec![],
        source_dir: "C:\\tmp".into(),
    };
    let verdict = gate.approve(&review).await;
    assert!(verdict.is_err(), "未 bind = 诚实拒绝而非放行");
}

#[test]
fn vault_secrets_missing_vault_resolves_none() {
    let (_dir, home) = temp_home();
    let secrets = VaultPluginSecrets::for_home(&home);
    assert!(secrets.resolve("vault:plugin/demo/key").is_none());
}

#[tokio::test]
async fn estop_watcher_initial_alignment() {
    // spawn_estop_watcher 首行对齐初始态：已触发的 estop → 子系统冻结。
    let (_dir, home) = temp_home();
    let manager = Arc::new(
        PluginManager::new(
            &common::workspace_path(&home),
            PluginLimits::default(),
            Arc::new(VaultPluginSecrets::for_home(&home)),
        )
        .expect("manager"),
    );
    let estop = Arc::new(nemesis_agent::EstopState::new());
    estop.trigger();
    spawn_estop_watcher(&manager, &estop);
    assert!(!manager.is_enabled(), "启动时已急停 = 子系统初始冻结");
}
