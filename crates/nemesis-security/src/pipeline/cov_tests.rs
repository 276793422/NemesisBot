// pipeline.rs 覆盖率补充测试（guardian_failure_policy 存取 / audit_logger
// 的 dir 缺省 None 臂 / 病毒层的感染拦截臂与全层放行收尾臂 /
// init_scanner_from_config 的计数 info 行 + 零引擎 warn 早退）。
//
// 豁免：38（layer_suggestion 的 `_ => None` 兜底臂——全部 7 个 deny_info
// 调用点的 layer 实参都在 match 列举内，该臂无任何可达调用路径，纯死防御）。

use super::*;
use crate::scanner::VirusScanner;
use async_trait::async_trait;
use std::collections::HashMap;

fn clean_config() -> SecurityPluginConfig {
    SecurityPluginConfig {
        enabled: true,
        ..Default::default()
    }
}

fn inv(tool: &str, args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        tool_name: tool.into(),
        args,
        user: "cov".into(),
        source: "cli".into(),
        metadata: Default::default(),
    }
}

/// 恒报感染的 mock 引擎（挂进共享链驱动病毒层拦截臂）。
struct InfectedEngine;

#[async_trait]
impl VirusScanner for InfectedEngine {
    fn name(&self) -> &str {
        "cov-infected"
    }
    async fn get_info(&self) -> crate::scanner::EngineInfo {
        crate::scanner::EngineInfo {
            name: "cov-infected".into(),
            version: String::new(),
            address: String::new(),
            ready: true,
            start_time: String::new(),
        }
    }
    async fn start(&self) -> Result<(), String> {
        Ok(())
    }
    async fn stop(&self) -> Result<(), String> {
        Ok(())
    }
    async fn is_ready(&self) -> bool {
        true
    }
    async fn scan_file(&self, path: &Path) -> crate::scanner::ScanResult {
        crate::scanner::ScanResult::with_threats(
            "cov-infected",
            "Cov.Test",
            &path.to_string_lossy(),
        )
    }
    async fn scan_content(&self, _content: &[u8]) -> crate::scanner::ScanResult {
        crate::scanner::ScanResult::with_threats("cov-infected", "Cov.Content", "")
    }
    async fn scan_directory(&self, _dir: &Path) -> Vec<crate::scanner::ScanResult> {
        Vec::new()
    }
    async fn get_database_status(&self) -> crate::scanner::DatabaseStatus {
        crate::scanner::DatabaseStatus::default()
    }
    async fn update_database(&self) -> Result<(), String> {
        Ok(())
    }
    fn get_stats(&self) -> HashMap<String, serde_json::Value> {
        HashMap::new()
    }
}

/// guardian_failure_policy 存取（trim + 小写归一）（983-989）。
#[test]
fn guardian_failure_policy_setter_getter_roundtrip() {
    let plugin = SecurityPlugin::new(clean_config());
    plugin.set_guardian_failure_policy("  ASK ");
    assert_eq!(plugin.guardian_failure_policy(), "ask");
    plugin.set_guardian_failure_policy("Deny");
    assert_eq!(plugin.guardian_failure_policy(), "deny");
}

/// audit_log_enabled = true 但 dir 缺省 → 构造期 None 臂（249）。
#[test]
fn audit_logger_none_when_enabled_without_dir() {
    let plugin = SecurityPlugin::new(SecurityPluginConfig {
        audit_log_enabled: true,
        audit_log_dir: None,
        ..clean_config()
    });
    assert!(plugin.audit_logger().is_none());
}

/// 病毒层：感染引擎 → deny_info("virus")（860-871 臂）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn virus_layer_blocks_infected_content() {
    let plugin = SecurityPlugin::new(clean_config());
    // 默认 default_action=deny 会在 ABAC 层先拦——放行到病毒层需显式放开。
    plugin.auditor().set_default_action("allow");
    {
        let chain = plugin.scan_chain();
        let mut c = chain.write().await;
        c.clear_engines();
        c.add_engine(Box::new(InfectedEngine));
        c.set_enabled(true);
    }

    let (allowed, deny) = plugin.execute(&inv(
        "write_file",
        serde_json::json!({"path": "a.txt", "content": "malicious payload"}),
    ));
    assert!(!allowed, "感染内容必须被病毒层拦下");
    let d = deny.expect("必须带 DenyInfo");
    assert_eq!(d.layer, "virus");
    assert_eq!(d.policy, "virus_scanner");
    assert!(d.summary.contains("cov-infected"), "{:?}", d.summary);
    assert!(d.suggestion.is_some(), "病毒层必须有固定建议");
}

/// 病毒层：默认 stub（恒干净）跑完全部扫描 → 全层放行收尾臂（892/895）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn virus_layer_passes_clean_content_through_stub_chain() {
    let plugin = SecurityPlugin::new(clean_config());
    plugin.auditor().set_default_action("allow");
    {
        let chain = plugin.scan_chain();
        let mut c = chain.write().await;
        c.clear_engines();
        c.add_engine(Box::new(crate::scanner::StubScanner));
        c.set_enabled(true);
    }

    let (allowed, deny) = plugin.execute(&inv(
        "write_file",
        serde_json::json!({"path": "ok.txt", "content": "harmless"}),
    ));
    assert!(allowed, "干净内容必须全层通过");
    assert!(deny.is_none());
}

/// init_scanner_from_config：stub 引擎装载 + 启用（计数 info 行 1125），
/// 以及零引擎时的 warn 早退（链保持禁用）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_scanner_from_config_counts_and_warns_on_empty() {
    let plugin = SecurityPlugin::new(clean_config());

    // 零引擎 → warn 早退，链保持禁用。
    plugin
        .init_scanner_from_config(&crate::scanner::ScannerFullConfig {
            enabled: vec![],
            engines: HashMap::new(),
        })
        .await;
    assert!(!plugin.scan_chain().read().await.is_enabled());

    // stub 引擎 → 装载 1 个 + 启用。
    plugin
        .init_scanner_from_config(&crate::scanner::ScannerFullConfig {
            enabled: vec!["stub".to_string()],
            engines: HashMap::from([("stub".to_string(), serde_json::json!({}))]),
        })
        .await;
    let chain = plugin.scan_chain();
    let c = chain.read().await;
    assert!(c.is_enabled());
    assert_eq!(c.engine_count(), 1);
}
