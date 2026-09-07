//! SecurityPlugin 构造的单一真相源（K1，devtool-upgrade 阶段 4）。
//!
//! 原本内联在 `commands/gateway.rs` Step 9b/9c 的整段装配逻辑原样抽出，
//! 供 `gateway` 与 headless `run` 两个入口共用——headless 也必须吃到与
//! gateway 完全一致的 9 层安全（安全 8 层 + scanner 链），不允许「无端口
//! 形态悄悄降级」。
//!
//! 职责（与原 gateway 注释一致）：
//! 1. 读 `config.security.json`（raw JSON）：DLP 层配置 + 其余 layer 开关
//!    （构造期是唯一生效路径）+ audit_chain 开关与路径。
//! 2. `SecurityPlugin::new` → `load_security_rules`（ABAC 规则）→
//!    `init_audit_log_file`。
//! 3. Step 9c：scanner 链从 `config.scanner.json` 初始化。
//!
//! 测试：`commands/gateway/tests.rs` 的 load_security_rules /
//! apply_security_layer_switches / load_scanner_full_config 系列继续以
//! `crate::security_setup::` 全路径覆盖这些函数（随代码同源验证）。

// use 语句随 security feature 走——feature off 时只剩空桩，模块级 use
// 会变成 unused import warning（--no-default-features 编译门干净）。
#[cfg(feature = "security")]
use std::sync::Arc;

#[cfg(feature = "security")]
use tracing::{info, warn};

#[cfg(feature = "security")]
use crate::common;

/// Build the SecurityPlugin exactly as the gateway's Step 9b + 9c: layer
/// switches + DLP config from `config.security.json`, ABAC rules, audit log
/// file, scanner chain from `config.scanner.json`.
///
/// `security_enabled=false`（或 `security` feature 编译掉）返回 `None`，
/// 与 gateway 原禁用臂同语义。
#[cfg(feature = "security")]
pub(crate) async fn build_security_plugin(
    home: &std::path::Path,
    security_enabled: bool,
) -> Option<Arc<nemesis_security::pipeline::SecurityPlugin>> {
    if !security_enabled {
        info!("[Security] plugin disabled by configuration");
        return None;
    }

    // Read audit_chain_enabled from `config.security.json`. The file is read
    // raw (since rules are loaded from it dynamically in `load_security_rules`);
    // we read it once more here to avoid reordering init (the SecurityPlugin
    // must be constructed before rules can be loaded onto it).
    let mut security_config = nemesis_security::pipeline::SecurityPluginConfig::default();
    let sec_config_path = common::security_config_path(home);
    // Read config.security.json once; pull both audit_chain and the DLP
    // layer config from it. Previously the plugin was built from default()
    // and the DLP layer config (`layers.dlp`) was never read anywhere — so
    // the engine always ran every rule with action=block, with no way to
    // configure a rule whitelist or low-confidence / inbound actions.
    let sec_json: Option<serde_json::Value> = if sec_config_path.exists() {
        std::fs::read_to_string(&sec_config_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    } else {
        None
    };
    if let Some(ref v) = sec_json {
        if let Some(dlp) = v
            .get("layers")
            .and_then(|l| l.get("dlp"))
            .and_then(|d| d.as_object())
        {
            if let Some(b) = dlp.get("enabled").and_then(|x| x.as_bool()) {
                security_config.dlp_enabled = b;
            }
            if let Some(s) = dlp.get("action").and_then(|x| x.as_str()) {
                security_config.dlp_action = s.to_string();
            }
            if let Some(arr) = dlp.get("rules").and_then(|x| x.as_array()) {
                security_config.dlp_enabled_rules = arr
                    .iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect();
            }
            if let Some(s) = dlp.get("low_confidence_action").and_then(|x| x.as_str()) {
                security_config.dlp_low_confidence_action = s.to_string();
            }
            if let Some(s) = dlp.get("inbound_action").and_then(|x| x.as_str()) {
                security_config.dlp_inbound_action = s.to_string();
            }
        }
        // 其余 layer 开关（injection/command_guard/credential/ssrf）：
        // 构造期是唯一生效路径（reload 只打日志不重建 layer）。
        apply_security_layer_switches(v, &mut security_config);
    }
    let audit_chain_enabled = sec_json
        .as_ref()
        .and_then(|v| v.get("audit_chain_enabled"))
        .and_then(|f| f.as_bool())
        .unwrap_or(false);
    if audit_chain_enabled {
        security_config.audit_chain_enabled = true;
        let chain_path = format!(
            "{}/workspace/logs/security_logs/audit_chain.jsonl",
            home.display()
        );
        security_config.audit_chain_path = Some(chain_path.clone());
        info!("[Security] Audit chain enabled at {}", chain_path);
    }
    let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        security_config,
    ));

    // Load security rules from config.security.json (mirrors Go's config loading)
    let sec_config_path = common::security_config_path(home);
    load_security_rules(&plugin, &sec_config_path);

    // Initialize audit log file.
    // The JSON config field is "audit_log_file_enabled"; default is true.
    // Log directory is always `{home}/workspace/logs/security_logs/`.
    // 委托 nemesis-path 唯一拼接点（web Logs 页 security 源同源读取）。
    let audit_dir = nemesis_path::resolve_audit_log_dir_in_workspace(&common::workspace_path(home))
        .to_string_lossy()
        .to_string();
    if let Err(e) = plugin.init_audit_log_file(&audit_dir) {
        warn!("[Security] Failed to initialize security audit log: {}", e);
    } else {
        info!("[Security] Security audit log initialized: {}", audit_dir);
    }

    info!("[Security] plugin enabled (injection handled by factory)");

    // Step 9c: Initialize scanner chain from config.scanner.json
    // Mirrors Go's initScannerChain() which calls LoadFromConfig() + chain.Start()
    let scanner_config_path = common::scanner_config_path(home);
    if scanner_config_path.exists() {
        if let Some(full_config) = load_scanner_full_config(&scanner_config_path)
            && !full_config.enabled.is_empty()
        {
            info!("[Security] Initializing scanner chain from config...");
            plugin.init_scanner_from_config(&full_config).await;
        }
    } else {
        info!(
            "[Security] Scanner config file not found: {}, scanner chain not initialized",
            scanner_config_path.display()
        );
    }

    Some(plugin)
}

/// `security` feature 编译掉时的空桩：headless `run` 与 gateway 共用同一
/// 调用点，返回 `None` = 无安全插件（与 feature 裁剪语义一致）。
#[cfg(not(feature = "security"))]
pub(crate) async fn build_security_plugin(
    _home: &std::path::Path,
    _security_enabled: bool,
) -> Option<()> {
    None
}

/// Apply the `layers.<name>.enabled` switches from `config.security.json`
/// onto a [`nemesis_security::pipeline::SecurityPluginConfig`] **before**
/// plugin construction — construction time is the only place layer engines
/// can be turned off (reload cannot rebuild layers).
///
/// real-machine e2e: `layers.ssrf.enabled=false` was silently ignored.
///
/// Absent keys keep the current value (caller passes defaults), mirroring
/// reload_config's `unwrap_or(current)` semantics.
#[cfg(feature = "security")]
pub(crate) fn apply_security_layer_switches(
    sec_json: &serde_json::Value,
    config: &mut nemesis_security::pipeline::SecurityPluginConfig,
) {
    let Some(layers) = sec_json.get("layers").and_then(|l| l.as_object()) else {
        return;
    };
    let flag = |key: &str, slot: &mut bool| {
        if let Some(b) = layers
            .get(key)
            .and_then(|d| d.get("enabled"))
            .and_then(|x| x.as_bool())
        {
            *slot = b;
        }
    };
    flag("injection", &mut config.injection_enabled);
    flag("command_guard", &mut config.command_guard_enabled);
    flag("credential", &mut config.credential_enabled);
    flag("ssrf", &mut config.ssrf_enabled);
}

/// Load security rules from `config.security.json` and apply to the SecurityPlugin.
///
/// Parses the JSON config file's `file_rules`, `dir_rules`, `process_rules`, etc.
/// and registers them as ABAC rules on the auditor. Also sets `default_action`.
///
/// Note: layer on/off toggles (`layers.*.enabled`) are NOT applied here —
/// layers are constructed (or not) at `SecurityPlugin::new()` time, so the
/// toggles are applied by [`apply_security_layer_switches`] before construction.
#[cfg(feature = "security")]
pub(crate) fn load_security_rules(
    plugin: &Arc<nemesis_security::pipeline::SecurityPlugin>,
    config_path: &std::path::Path,
) {
    use nemesis_security::types::{OperationType, SecurityRule};

    if !config_path.exists() {
        info!(
            "[Security] config file not found: {}, using defaults",
            config_path.display()
        );
        return;
    }

    let data = match std::fs::read_to_string(config_path) {
        Ok(d) => d,
        Err(e) => {
            warn!("[Security] Failed to read security config: {}", e);
            return;
        }
    };

    let config: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(e) => {
            warn!("[Security] Failed to parse security config JSON: {}", e);
            return;
        }
    };

    // Set default_action
    if let Some(action) = config.get("default_action").and_then(|v| v.as_str()) {
        plugin.auditor().set_default_action(action);
        info!("[Security] default_action: {}", action);
    }

    // Helper: parse rules from JSON array of {pattern, action}
    fn parse_rules(value: &serde_json::Value) -> Vec<SecurityRule> {
        value
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        Some(SecurityRule {
                            pattern: item.get("pattern")?.as_str()?.to_string(),
                            action: item.get("action")?.as_str()?.to_string(),
                            comment: item
                                .get("comment")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    // File rules
    if let Some(file_rules) = config.get("file_rules") {
        let read_rules = parse_rules(file_rules.get("read").unwrap_or(&serde_json::Value::Null));
        let write_rules = parse_rules(file_rules.get("write").unwrap_or(&serde_json::Value::Null));
        let delete_rules =
            parse_rules(file_rules.get("delete").unwrap_or(&serde_json::Value::Null));
        let append_rules =
            parse_rules(file_rules.get("append").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::FileRead, read_rules);
        plugin.set_rules(OperationType::FileWrite, write_rules.clone());
        plugin.set_rules(OperationType::FileDelete, delete_rules);
        if !append_rules.is_empty() {
            // append uses FileWrite rules as well
            let mut combined = write_rules;
            combined.extend(append_rules);
            plugin.set_rules(OperationType::FileWrite, combined);
        }
        info!("[Security] file_rules loaded");
    }

    // Dir rules
    if let Some(dir_rules) = config.get("dir_rules") {
        let read_rules = parse_rules(dir_rules.get("read").unwrap_or(&serde_json::Value::Null));
        let create_rules = parse_rules(dir_rules.get("create").unwrap_or(&serde_json::Value::Null));
        let delete_rules = parse_rules(dir_rules.get("delete").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::DirRead, read_rules);
        plugin.set_rules(OperationType::DirCreate, create_rules);
        plugin.set_rules(OperationType::DirDelete, delete_rules);
        info!("[Security] dir_rules loaded");
    }

    // Process rules
    if let Some(proc_rules) = config.get("process_rules") {
        let exec_rules = parse_rules(proc_rules.get("exec").unwrap_or(&serde_json::Value::Null));
        let spawn_rules = parse_rules(proc_rules.get("spawn").unwrap_or(&serde_json::Value::Null));
        let kill_rules = parse_rules(proc_rules.get("kill").unwrap_or(&serde_json::Value::Null));
        let suspend_rules = parse_rules(
            proc_rules
                .get("suspend")
                .unwrap_or(&serde_json::Value::Null),
        );

        plugin.set_rules(OperationType::ProcessExec, exec_rules);
        plugin.set_rules(OperationType::ProcessSpawn, spawn_rules);
        plugin.set_rules(OperationType::ProcessKill, kill_rules);
        plugin.set_rules(OperationType::ProcessSuspend, suspend_rules);
        info!("[Security] process_rules loaded");
    }

    // Network rules
    if let Some(net_rules) = config.get("network_rules") {
        let request_rules =
            parse_rules(net_rules.get("request").unwrap_or(&serde_json::Value::Null));
        let download_rules = parse_rules(
            net_rules
                .get("download")
                .unwrap_or(&serde_json::Value::Null),
        );
        let upload_rules = parse_rules(net_rules.get("upload").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::NetworkRequest, request_rules);
        plugin.set_rules(OperationType::NetworkDownload, download_rules);
        plugin.set_rules(OperationType::NetworkUpload, upload_rules);
        info!("[Security] network_rules loaded");
    }

    // Hardware rules
    if let Some(hw_rules) = config.get("hardware_rules") {
        let i2c_rules = parse_rules(hw_rules.get("i2c").unwrap_or(&serde_json::Value::Null));
        let spi_rules = parse_rules(hw_rules.get("spi").unwrap_or(&serde_json::Value::Null));
        let gpio_rules = parse_rules(hw_rules.get("gpio").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::HardwareI2C, i2c_rules);
        plugin.set_rules(OperationType::HardwareSPI, spi_rules);
        plugin.set_rules(OperationType::HardwareGPIO, gpio_rules);
        info!("[Security] hardware_rules loaded");
    }

    // Registry rules
    if let Some(reg_rules) = config.get("registry_rules") {
        let read_rules = parse_rules(reg_rules.get("read").unwrap_or(&serde_json::Value::Null));
        let write_rules = parse_rules(reg_rules.get("write").unwrap_or(&serde_json::Value::Null));
        let delete_rules = parse_rules(reg_rules.get("delete").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::RegistryRead, read_rules);
        plugin.set_rules(OperationType::RegistryWrite, write_rules);
        plugin.set_rules(OperationType::RegistryDelete, delete_rules);
        info!("[Security] registry_rules loaded");
    }

    info!("[Security] config loaded from {}", config_path.display());
}

/// Load scanner full config from `config.scanner.json`.
///
/// Returns None if the file doesn't exist or can't be parsed.
#[cfg(feature = "security")]
pub(crate) fn load_scanner_full_config(
    config_path: &std::path::Path,
) -> Option<nemesis_security::scanner::ScannerFullConfig> {
    let data = std::fs::read_to_string(config_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;

    let enabled: Vec<String> = json
        .get("enabled")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let engines: std::collections::HashMap<String, serde_json::Value> = json
        .get("engines")
        .and_then(|v| v.as_object())
        .map(|map| map.clone().into_iter().collect())
        .unwrap_or_default();

    Some(nemesis_security::scanner::ScannerFullConfig { enabled, engines })
}
