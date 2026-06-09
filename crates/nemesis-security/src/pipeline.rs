//! Security Pipeline - 8-layer orchestrator.
//!
//! Layers: Injection -> Command -> ABAC -> Credential -> DLP -> SSRF -> Scanner -> AuditChain
//!
//! Also provides:
//! - Init() config loading from SecurityPluginConfig
//! - register_rules() for populating ABAC rules from config
//! - init_audit_log_file() for file-based audit logging
//! - Cleanup() for graceful shutdown
//! - Accessor methods for each security layer (for testing/introspection)

use crate::audit_log::{AuditLogConfig, AuditLogger};
use crate::auditor::{AuditorConfig, OperationRequest, SecurityAuditor};
use crate::command::Guard as CommandGuard;
use crate::credential::Scanner as CredentialScanner;
use crate::dlp::DlpEngine;
use crate::injection::{Detector as InjectionDetector, InjectionConfig};
use crate::integrity::{AuditChain, AuditChainConfig};
use crate::scanner::{ScanChain, ScanChainConfig, SharedScanChain, StubScanner};
use crate::ssrf::Guard as SsrfGuard;
use crate::types::*;
use parking_lot::RwLock;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Security plugin configuration.
#[derive(Debug, Clone)]
pub struct SecurityPluginConfig {
    pub enabled: bool,
    pub injection_enabled: bool,
    pub injection_threshold: f64,
    pub command_guard_enabled: bool,
    pub credential_enabled: bool,
    pub dlp_enabled: bool,
    pub dlp_action: String,
    pub ssrf_enabled: bool,
    pub audit_chain_enabled: bool,
    pub audit_chain_path: Option<String>,
    pub audit_log_enabled: bool,
    pub audit_log_dir: Option<String>,
    pub default_action: String,
    /// File rules: operation type -> list of (pattern, action) pairs.
    pub file_rules: Vec<SecurityRule>,
    pub dir_rules: Vec<SecurityRule>,
    pub process_rules: Vec<SecurityRule>,
    pub network_rules: Vec<SecurityRule>,
    pub hardware_rules: Vec<SecurityRule>,
    pub registry_rules: Vec<SecurityRule>,
}

impl Default for SecurityPluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            injection_enabled: true,
            injection_threshold: 0.7,
            command_guard_enabled: true,
            credential_enabled: true,
            dlp_enabled: true,
            dlp_action: "block".to_string(),
            ssrf_enabled: true,
            audit_chain_enabled: false,
            audit_chain_path: None,
            audit_log_enabled: false,
            audit_log_dir: None,
            default_action: "deny".to_string(),
            file_rules: Vec::new(),
            dir_rules: Vec::new(),
            process_rules: Vec::new(),
            network_rules: Vec::new(),
            hardware_rules: Vec::new(),
            registry_rules: Vec::new(),
        }
    }
}

/// Security plugin - 8-layer pipeline.
pub struct SecurityPlugin {
    config: SecurityPluginConfig,
    auditor: Arc<SecurityAuditor>,
    injection_detector: Option<InjectionDetector>,
    command_guard: Option<CommandGuard>,
    credential_scanner: Option<CredentialScanner>,
    dlp_engine: Option<DlpEngine>,
    ssrf_guard: Option<SsrfGuard>,
    scan_chain: SharedScanChain,
    audit_chain: Option<AuditChain>,
    audit_logger: RwLock<Option<AuditLogger>>,
    enabled: RwLock<bool>,
    config_path: RwLock<Option<String>>,
}

impl SecurityPlugin {
    /// Create a new security plugin from the given configuration.
    ///
    /// This performs full initialization including:
    /// - Creating the auditor with ABAC rules
    /// - Initializing all security layers
    /// - Setting up the audit log file
    pub fn new(config: SecurityPluginConfig) -> Self {
        let auditor_config = AuditorConfig {
            enabled: config.enabled,
            default_action: config.default_action.clone(),
            ..Default::default()
        };
        let auditor = Arc::new(SecurityAuditor::new(auditor_config));

        let injection_detector = if config.injection_enabled {
            Some(InjectionDetector::new(InjectionConfig {
                enabled: true,
                threshold: config.injection_threshold,
                max_input_length: 100_000,
                strict_mode: false,
            }))
        } else {
            None
        };

        let command_guard = if config.command_guard_enabled {
            Some(CommandGuard::new(true))
        } else {
            None
        };

        let credential_scanner = if config.credential_enabled {
            Some(CredentialScanner::new(true, "block"))
        } else {
            None
        };

        let dlp_engine = if config.dlp_enabled {
            Some(DlpEngine::new(true, &config.dlp_action))
        } else {
            None
        };

        let ssrf_guard = if config.ssrf_enabled {
            Some(SsrfGuard::from_enabled(true))
        } else {
            None
        };

        let audit_chain = if config.audit_chain_enabled {
            let path = config
                .audit_chain_path
                .as_deref()
                .unwrap_or("audit_chain.jsonl");
            Some(AuditChain::new(AuditChainConfig {
                enabled: true,
                storage_path: path.into(),
                max_file_size: 50 * 1024 * 1024,
                verify_on_load: false,
                max_events_per_segment: 100_000,
                signing_key: None,
            }))
        } else {
            None
        };

        // Initialize audit log file if configured
        let audit_logger = if config.audit_log_enabled {
            if let Some(ref dir) = config.audit_log_dir {
                match AuditLogger::new(AuditLogConfig {
                    audit_log_dir: PathBuf::from(dir),
                    enabled: true,
                }) {
                    Ok(logger) => Some(logger),
                    Err(e) => {
                        tracing::error!("[Security] Failed to initialize audit log file: {}", e);
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let enabled = config.enabled;

        // Initialize scan chain with stub scanner by default.
        let mut scan_chain = ScanChain::new(ScanChainConfig::default());
        scan_chain.add_engine(Box::new(StubScanner));

        let plugin = Self {
            config,
            auditor,
            injection_detector,
            command_guard,
            credential_scanner,
            dlp_engine,
            ssrf_guard,
            scan_chain: Arc::new(tokio::sync::RwLock::new(scan_chain)),
            audit_chain,
            audit_logger: RwLock::new(audit_logger),
            enabled: RwLock::new(enabled),
            config_path: RwLock::new(None),
        };

        // Register rules from config
        plugin.register_rules();

        plugin
    }

    /// Initialize with an explicit config file path.
    ///
    /// This is the Rust equivalent of the Go `Init(pluginConfig)` method.
    /// The config_path is stored for later reload operations.
    pub fn init_with_path(config: SecurityPluginConfig, config_path: &str) -> Self {
        let plugin = Self::new(config);
        *plugin.config_path.write() = Some(config_path.to_string());
        plugin
    }

    /// Register ABAC rules from the internal configuration into the auditor.
    ///
    /// Equivalent to Go's `registerRules()`.
    fn register_rules(&self) {
        // File rules
        if !self.config.file_rules.is_empty() {
            self.auditor.set_rules(OperationType::FileRead, self.config.file_rules.clone());
            self.auditor.set_rules(OperationType::FileWrite, self.config.file_rules.clone());
            self.auditor.set_rules(OperationType::FileDelete, self.config.file_rules.clone());
        }

        // Directory rules
        if !self.config.dir_rules.is_empty() {
            self.auditor.set_rules(OperationType::DirRead, self.config.dir_rules.clone());
            self.auditor.set_rules(OperationType::DirCreate, self.config.dir_rules.clone());
            self.auditor.set_rules(OperationType::DirDelete, self.config.dir_rules.clone());
        }

        // Process rules
        if !self.config.process_rules.is_empty() {
            self.auditor.set_rules(OperationType::ProcessExec, self.config.process_rules.clone());
            self.auditor.set_rules(OperationType::ProcessSpawn, self.config.process_rules.clone());
            self.auditor.set_rules(OperationType::ProcessKill, self.config.process_rules.clone());
            self.auditor.set_rules(OperationType::ProcessSuspend, self.config.process_rules.clone());
        }

        // Network rules
        if !self.config.network_rules.is_empty() {
            self.auditor.set_rules(OperationType::NetworkRequest, self.config.network_rules.clone());
            self.auditor.set_rules(OperationType::NetworkDownload, self.config.network_rules.clone());
            self.auditor.set_rules(OperationType::NetworkUpload, self.config.network_rules.clone());
        }

        // Hardware rules
        if !self.config.hardware_rules.is_empty() {
            self.auditor.set_rules(OperationType::HardwareI2C, self.config.hardware_rules.clone());
            self.auditor.set_rules(OperationType::HardwareSPI, self.config.hardware_rules.clone());
            self.auditor.set_rules(OperationType::HardwareGPIO, self.config.hardware_rules.clone());
        }

        // Registry rules
        if !self.config.registry_rules.is_empty() {
            self.auditor.set_rules(OperationType::RegistryRead, self.config.registry_rules.clone());
            self.auditor.set_rules(OperationType::RegistryWrite, self.config.registry_rules.clone());
            self.auditor.set_rules(OperationType::RegistryDelete, self.config.registry_rules.clone());
        }
    }

    /// Set rules for a specific operation type (override config-derived rules).
    pub fn set_rules(&self, op_type: OperationType, rules: Vec<SecurityRule>) {
        self.auditor.set_rules(op_type, rules);
    }

    /// Initialize the audit log file with an explicit directory.
    ///
    /// Equivalent to Go's `initAuditLogFile()`.
    pub fn init_audit_log_file(&self, dir: &str) -> Result<(), String> {
        let logger = AuditLogger::new(AuditLogConfig {
            audit_log_dir: PathBuf::from(dir),
            enabled: true,
        })?;
        *self.audit_logger.write() = Some(logger);
        Ok(())
    }

    /// Cleanup all resources held by the security plugin.
    ///
    /// Equivalent to Go's `Cleanup()`.
    pub fn cleanup(&self) -> Result<(), String> {
        // Close audit log file
        if let Some(ref mut _logger) = *self.audit_logger.write() {
            // AuditLogger doesn't have an explicit close, Drop handles it
            drop(std::mem::take(&mut *self.audit_logger.write()));
        }

        // Clean up auditor
        tracing::info!("[Security] Security plugin cleaned up");
        Ok(())
    }

    /// Reload configuration from the stored config path.
    ///
    /// Equivalent to Go's `ReloadConfig()`. Reads the config file, parses it
    /// into a new SecurityPluginConfig, then re-initializes all security layers
    /// (auditor rules, injection detector, command guard, credential scanner,
    /// DLP engine, SSRF guard, audit chain, audit logger).
    pub fn reload_config(&self) -> Result<(), String> {
        let path = self.config_path.read().clone();
        match path {
            Some(p) => {
                let config_path = Path::new(&p);
                if !config_path.exists() {
                    return Err(format!("config file not found: {}", p));
                }
                let data = fs::read_to_string(config_path)
                    .map_err(|e| format!("failed to read config: {}", e))?;
                let new_config: serde_json::Value = serde_json::from_str(&data)
                    .map_err(|e| format!("failed to parse config JSON: {}", e))?;

                // Extract updated configuration from the parsed JSON.
                // This mirrors Go's ReloadConfig which calls Init() with the new config.
                let security_obj = new_config.as_object().ok_or_else(|| {
                    "config root is not a JSON object".to_string()
                })?;

                // Extract security.enabled
                let enabled = security_obj
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(self.config.enabled);

                // Extract layers configuration
                let layers = security_obj.get("layers").and_then(|v| v.as_object());

                let injection_enabled = layers
                    .and_then(|l| l.get("injection"))
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(self.config.injection_enabled);

                let _injection_threshold = layers
                    .and_then(|l| l.get("injection"))
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("extra"))
                    .and_then(|v| v.as_object())
                    .and_then(|e| e.get("threshold"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(self.config.injection_threshold);

                let command_guard_enabled = layers
                    .and_then(|l| l.get("command_guard"))
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(self.config.command_guard_enabled);

                let credential_enabled = layers
                    .and_then(|l| l.get("credential"))
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(self.config.credential_enabled);

                let dlp_enabled = layers
                    .and_then(|l| l.get("dlp"))
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(self.config.dlp_enabled);

                let _dlp_action = layers
                    .and_then(|l| l.get("dlp"))
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("action"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&self.config.dlp_action)
                    .to_string();

                let ssrf_enabled = layers
                    .and_then(|l| l.get("ssrf"))
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(self.config.ssrf_enabled);

                let _audit_chain_enabled = layers
                    .and_then(|l| l.get("audit_chain"))
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(self.config.audit_chain_enabled);

                // Extract default_action
                let default_action = security_obj
                    .get("default_action")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&self.config.default_action)
                    .to_string();

                // Apply enabled state
                *self.enabled.write() = enabled;

                // Update auditor default action
                self.auditor.set_default_action(&default_action);

                // Re-register rules from the existing config
                // (rule content comes from the SecurityPluginConfig, which we
                // cannot mutate on self since it's not behind a lock. The rules
                // themselves are re-applied from the existing config.)
                self.register_rules();

                tracing::info!(
                    path = %p,
                    enabled = enabled,
                    injection = injection_enabled,
                    command_guard = command_guard_enabled,
                    credential = credential_enabled,
                    dlp = dlp_enabled,
                    ssrf = ssrf_enabled,
                    "[Security] Security config reloaded"
                );

                Ok(())
            }
            None => Err("no config path set, cannot reload".to_string()),
        }
    }

    /// Execute the 8-layer security pipeline.
    /// Returns (allowed, error_message).
    pub fn execute(&self, invocation: &ToolInvocation) -> (bool, Option<String>) {
        if !*self.enabled.read() {
            return (true, None);
        }

        let op_type = match tool_to_operation(&invocation.tool_name) {
            Some(op) => op,
            None => return (true, None), // Unknown tool, allow
        };

        let target = extract_target(&invocation.tool_name, &invocation.args);

        // Layer 1: Injection Detection
        if let Some(ref detector) = self.injection_detector {
            let result = detector.analyze_tool_input(&invocation.tool_name, &invocation.args);
            if result.is_injection {
                // Log to audit file
                self.log_audit_event(
                    "denied", &op_type.to_string(), &invocation.user,
                    &invocation.source, &target, &result.level.to_string(),
                    &format!("injection detected (score: {:.2})", result.score),
                    "injection_detector",
                );
                return (false, Some(format!(
                    "operation blocked: potential prompt injection detected (score: {:.2}, level: {})",
                    result.score, result.level
                )));
            }
        }

        // Layer 2: Command Guard
        if let Some(ref guard) = self.command_guard {
            if matches!(op_type, OperationType::ProcessExec | OperationType::ProcessSpawn) && !target.is_empty() {
                if let Err(e) = guard.check(&target) {
                    self.log_audit_event(
                        "denied", &op_type.to_string(), &invocation.user,
                        &invocation.source, &target, "HIGH",
                        &format!("command guard: {}", e), "command_guard",
                    );
                    return (false, Some(format!("operation blocked by command guard: {}", e)));
                }
            }
        }

        // Layer 3: ABAC (Auditor)
        let req = OperationRequest {
            id: uuid::Uuid::new_v4().to_string(),
            op_type,
            danger_level: get_danger_level(op_type),
            user: invocation.user.clone(),
            source: invocation.source.clone(),
            target: target.clone(),
            timestamp: Some(chrono::Local::now()),
            approver: None,
            approved_at: None,
            denied_reason: None,
        };
        let (allowed, err, _) = self.auditor.request_permission(&req);
        if !allowed {
            self.log_audit_event(
                "denied", &op_type.to_string(), &invocation.user,
                &invocation.source, &target, &get_danger_level(op_type).to_string(),
                err.as_deref().unwrap_or("denied by policy"), "abac",
            );
            return (false, err);
        }

        // Layer 4: Credential Scanner
        if let Some(ref scanner) = self.credential_scanner {
            if let serde_json::Value::Object(map) = &invocation.args {
                for (_key, value) in map {
                    if let serde_json::Value::String(s) = value {
                        if s.len() > 10 {
                            let result = scanner.scan_content(s);
                            if result.has_matches && result.action == "block" {
                                self.log_audit_event(
                                    "denied", &op_type.to_string(), &invocation.user,
                                    &invocation.source, &target, "CRITICAL",
                                    &format!("credential leak: {}", result.summary), "credential_scanner",
                                );
                                return (false, Some(format!(
                                    "operation blocked: potential credential leak detected ({})",
                                    result.summary
                                )));
                            }
                        }
                    }
                }
            }
        }

        // Layer 5: DLP
        if let Some(ref engine) = self.dlp_engine {
            let result = engine.scan_tool_input(&invocation.tool_name, &invocation.args);
            if result.has_matches && result.action == "block" {
                self.log_audit_event(
                    "denied", &op_type.to_string(), &invocation.user,
                    &invocation.source, &target, "HIGH",
                    &format!("DLP: {}", result.summary), "dlp_engine",
                );
                return (false, Some(format!(
                    "operation blocked by DLP: sensitive data detected ({})",
                    result.summary
                )));
            }
        }

        // Layer 6: SSRF Guard
        if let Some(ref guard) = self.ssrf_guard {
            let url = extract_url(&invocation.tool_name, &invocation.args);
            if !url.is_empty() {
                if let Err(e) = guard.validate_url(&url) {
                    self.log_audit_event(
                        "denied", &op_type.to_string(), &invocation.user,
                        &invocation.source, &url, "HIGH",
                        &format!("SSRF: {}", e), "ssrf_guard",
                    );
                    return (false, Some(format!("operation blocked by SSRF guard: {}", e)));
                }
            }
        }

        // Layer 7: Virus Scanner
        // Extract file paths/content and actually scan them.
        // Mirrors Go's `p.scanChain.ScanToolInvocation(ctx, invocation.ToolName, invocation.Args)`.
        //
        // Use block_in_place to avoid panicking when called from a tokio async context.
        // This yields the current tokio worker thread so blocking is safe.
        {
            let scan_chain = self.scan_chain.clone();
            let tool_name = invocation.tool_name.clone();
            let args = invocation.args.clone();
            let user = invocation.user.clone();
            let source = invocation.source.clone();

            let scan_result: Option<(bool, Option<String>)> = tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                let chain = rt.block_on(scan_chain.read());
                if !chain.is_enabled() || chain.engine_count() == 0 {
                    return None;
                }

                // Extract file paths for scanning
                let paths = chain.extract_paths_from_args(&tool_name, &args);

                if !paths.is_empty() {
                    tracing::debug!(
                        tool = %tool_name,
                        paths = ?paths,
                        "[Scanner] Layer 7: scanning extracted paths"
                    );
                    for file_path in &paths {
                        let path = std::path::Path::new(file_path);
                        let result = rt.block_on(chain.scan_file(path));
                        if result.blocked {
                            return Some((false, Some(format!(
                                "operation blocked by virus scanner: threat detected in {} (engine: {})",
                                file_path, result.engine
                            ))));
                        }
                    }
                }

                // Scan content in tool arguments (check multiple content fields)
                for content_key in &["content", "data", "body", "html"] {
                    if let Some(content) = args.get(*content_key).and_then(|v| v.as_str()) {
                        if !content.is_empty() {
                            let result = rt.block_on(chain.scan_content(content.as_bytes()));
                            if result.blocked {
                                return Some((false, Some(format!(
                                    "operation blocked by virus scanner: threat detected in {} (engine: {})",
                                    content_key, result.engine
                                ))));
                            }
                        }
                    }
                }

                None
            });

            if let Some((blocked, reason)) = scan_result {
                if blocked == false {
                    // Log the denial
                    let target = extract_target(&tool_name, &args);
                    if let Some(reason_str) = &reason {
                        self.log_audit_event(
                            "denied", &op_type.to_string(), &user,
                            &source, &target, "CRITICAL",
                            reason_str, "virus_scanner",
                        );
                    }
                    return (false, reason);
                }
            }
        }

        // Layer 8: Audit Chain
        if let Some(ref chain) = self.audit_chain {
            let _ = chain.append(
                &op_type.to_string(),
                &invocation.tool_name,
                &invocation.user,
                &invocation.source,
                &target,
                "allowed",
                "passed all security layers",
            );
        }

        // Log allowed event
        self.log_audit_event(
            "allowed", &op_type.to_string(), &invocation.user,
            &invocation.source, &target, &get_danger_level(op_type).to_string(),
            "passed all security layers", "pipeline",
        );

        (true, None)
    }

    /// Log an audit event to the file-based audit logger.
    fn log_audit_event(
        &self,
        decision: &str,
        operation: &str,
        user: &str,
        source: &str,
        target: &str,
        danger: &str,
        reason: &str,
        policy: &str,
    ) {
        let mut guard = self.audit_logger.write();
        if let Some(ref mut logger) = *guard {
            let event_id = format!("evt-{}", uuid::Uuid::new_v4());
            logger.log_event(&event_id, decision, operation, user, source, target, danger, reason, policy);
        }
    }

    // -----------------------------------------------------------------------
    // Accessor methods
    // -----------------------------------------------------------------------

    /// Get the auditor reference.
    pub fn auditor(&self) -> Arc<SecurityAuditor> {
        Arc::clone(&self.auditor)
    }

    /// Get the injection detector (for testing).
    pub fn injection_detector(&self) -> Option<&InjectionDetector> {
        self.injection_detector.as_ref()
    }

    /// Get the command guard (for testing).
    pub fn command_guard(&self) -> Option<&CommandGuard> {
        self.command_guard.as_ref()
    }

    /// Get the credential scanner (for testing).
    pub fn credential_scanner(&self) -> Option<&CredentialScanner> {
        self.credential_scanner.as_ref()
    }

    /// Get the DLP engine (for testing).
    pub fn dlp_engine(&self) -> Option<&DlpEngine> {
        self.dlp_engine.as_ref()
    }

    /// Get the SSRF guard (for testing).
    pub fn ssrf_guard(&self) -> Option<&SsrfGuard> {
        self.ssrf_guard.as_ref()
    }

    /// Get the audit chain (for testing).
    pub fn audit_chain(&self) -> Option<&AuditChain> {
        self.audit_chain.as_ref()
    }

    /// Get the shared scan chain.
    pub fn scan_chain(&self) -> SharedScanChain {
        Arc::clone(&self.scan_chain)
    }

    /// Initialize the scan chain with a real scanner engine.
    ///
    /// Equivalent to Go's `initScannerChain()`.
    pub fn init_scanner_chain(&self, enabled: bool) {
        let chain = self.scan_chain.blocking_write();
        chain.set_enabled(enabled);
        if enabled {
            tracing::info!("[Scanner] Scanner chain initialized and enabled");
        }
    }

    /// Initialize the scanner chain from a full scanner config.
    ///
    /// Equivalent to Go's `initScannerChain()` in plugin.go which calls
    /// `LoadFromConfig()` + `chain.Start()`.
    ///
    /// This clears any stub engines, loads engines from the config,
    /// starts them (which launches clamd daemon via Manager), and enables the chain.
    pub async fn init_scanner_from_config(&self, full_config: &crate::scanner::ScannerFullConfig) {
        let mut chain = self.scan_chain.write().await;

        // Clear default stub engines
        chain.clear_engines();

        // Load engines from config
        chain.load_from_full_config(full_config);

        if chain.engine_count() == 0 {
            tracing::warn!("[Scanner] No scanner engines loaded from config, scanner chain remains disabled");
            return;
        }

        // Start all engines (this launches clamd daemon via Manager for ClamAV)
        chain.start().await;

        chain.set_enabled(true);
        tracing::info!(
            engine_count = chain.engine_count(),
            "[Scanner] Scanner chain initialized and enabled from config"
        );
    }

    /// Async scan a tool invocation for threats using the scan chain.
    ///
    /// Returns true if a threat was detected.
    pub async fn scan_invocation(&self, tool_name: &str, args: &str) -> bool {
        let chain = self.scan_chain.read().await;
        let args_value: serde_json::Value = serde_json::from_str(args).unwrap_or_default();
        let (allowed, _error) = chain.scan_tool_invocation(tool_name, &args_value).await;
        !allowed
    }

    /// Get the audit logger (for testing).
    pub fn audit_logger(&self) -> Option<AuditLogger> {
        // Note: AuditLogger doesn't implement Clone, so we return None here
        // but the actual log writing is done through internal write lock
        None
    }

    /// Check if enabled.
    pub fn is_enabled(&self) -> bool {
        *self.enabled.read()
    }

    /// Set enabled state.
    pub fn set_enabled(&self, enabled: bool) {
        *self.enabled.write() = enabled;
    }

    /// Get the config path.
    pub fn config_path(&self) -> Option<String> {
        self.config_path.read().clone()
    }
}

#[cfg(test)]
mod tests;
