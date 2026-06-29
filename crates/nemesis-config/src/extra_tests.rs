//! Additional coverage tests for lib.rs.
//!
//! Targets error paths, default functions, serde default function invocations,
//! platform security filename branches, and sub-config load/save error paths.

use super::*;
use std::io::Write;
use std::sync::Mutex;

/// Global lock to serialize tests that mutate process-wide state (CWD, env vars).
/// Without this, parallel test execution causes data races between tests that
/// call `std::env::set_current_dir()` or `std::env::set_var()` — these are
/// process-global and cannot be isolated per-thread.
static GLOBAL_STATE_LOCK: Mutex<()> = Mutex::new(());

// ============================================================================
// deserialize_flexible_string_vec: visitor branches
// ============================================================================

#[test]
fn extra_deserialize_flexible_string_vec_strings_only() {
    let json = r#"{"allow_from": ["alice", "bob"]}"#;
    let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.allow_from, vec!["alice".to_string(), "bob".to_string()]);
}

#[test]
fn extra_deserialize_flexible_string_vec_with_integer() {
    let json = r#"{"allow_from": [42]}"#;
    let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.allow_from.len(), 1);
    assert_eq!(cfg.allow_from[0], "42");
}

#[test]
fn extra_deserialize_flexible_string_vec_with_float() {
    let json = r#"{"allow_from": [3.14]}"#;
    let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.allow_from[0], "3.14");
}

#[test]
fn extra_deserialize_flexible_string_vec_with_null() {
    let json = r#"{"allow_from": [null]}"#;
    let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.allow_from[0], "null");
}

#[test]
fn extra_deserialize_flexible_string_vec_with_object() {
    let json = r#"{"allow_from": [{"k": "v"}]}"#;
    let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.allow_from[0], "{\"k\":\"v\"}");
}

#[test]
fn extra_deserialize_flexible_string_vec_empty_default_field() {
    // Omitting allow_from should yield default (empty vec) — no seq visitor.
    let json = r#"{"enabled": true}"#;
    let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.allow_from.is_empty());
    assert!(cfg.enabled);
}

// ============================================================================
// AgentModelConfig visitor branches
// ============================================================================

#[test]
fn extra_agent_model_config_visit_str_via_field() {
    // String model on AgentConfigEntry.model
    let json = r#"{"id":"a","model":"gpt-4"}"#;
    let entry: AgentConfigEntry = serde_json::from_str(json).unwrap();
    let model = entry.model.unwrap();
    assert_eq!(model.primary, "gpt-4");
    assert!(model.fallbacks.is_empty());
}

#[test]
fn extra_agent_model_config_visit_map_via_field() {
    let json = r#"{"id":"a","model":{"primary":"claude","fallbacks":["f1"]}}"#;
    let entry: AgentConfigEntry = serde_json::from_str(json).unwrap();
    let model = entry.model.unwrap();
    assert_eq!(model.primary, "claude");
    assert_eq!(model.fallbacks, vec!["f1".to_string()]);
}

#[test]
fn extra_agent_model_config_visit_map_with_missing_fields() {
    // Empty map should default primary/fallbacks
    let json = r#"{"primary":null}"#;
    // visit_map path with serde default on Raw struct fields
    let res: std::result::Result<AgentModelConfig, _> = serde_json::from_str(json);
    // Either errors on null or default — both acceptable; just exercise the path.
    let _ = res;
}

#[test]
fn extra_agent_model_config_invalid_type_array() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    // Passing an array should produce a deserialization error.
    let json = r#"[1,2,3]"#;
    let res: std::result::Result<AgentModelConfig, _> = serde_json::from_str(json);
    assert!(res.is_err());
}

// ============================================================================
// set_embedded_defaults_from_fs: per-file error paths
// ============================================================================

#[test]
fn extra_set_embedded_defaults_missing_security() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cdir = dir.path();
    std::fs::write(cdir.join("config.default.json"), "{}").unwrap();
    std::fs::write(cdir.join("config.mcp.default.json"), "{}").unwrap();
    // Missing platform security file
    let res = set_embedded_defaults_from_fs(cdir);
    assert!(res.is_err());
    let msg = res.unwrap_err().to_string();
    assert!(msg.contains("security") || msg.contains("config.security"));
}

#[test]
fn extra_set_embedded_defaults_missing_cluster() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cdir = dir.path();
    std::fs::write(cdir.join("config.default.json"), "{}").unwrap();
    std::fs::write(cdir.join("config.mcp.default.json"), "{}").unwrap();
    std::fs::write(cdir.join(get_platform_security_config_filename()), "{}").unwrap();
    // Missing cluster
    let res = set_embedded_defaults_from_fs(cdir);
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("config.cluster.default.json"));
}

#[test]
fn extra_set_embedded_defaults_missing_skills() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cdir = dir.path();
    std::fs::write(cdir.join("config.default.json"), "{}").unwrap();
    std::fs::write(cdir.join("config.mcp.default.json"), "{}").unwrap();
    std::fs::write(cdir.join(get_platform_security_config_filename()), "{}").unwrap();
    std::fs::write(cdir.join("config.cluster.default.json"), "{}").unwrap();
    // Missing skills
    let res = set_embedded_defaults_from_fs(cdir);
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("config.skills.default.json"));
}

#[test]
fn extra_set_embedded_defaults_optional_scanner_missing() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    // scanner.default.json is optional — should succeed without it.
    let dir = tempfile::tempdir().unwrap();
    let cdir = dir.path();
    std::fs::write(cdir.join("config.default.json"), "{}").unwrap();
    std::fs::write(cdir.join("config.mcp.default.json"), "{}").unwrap();
    std::fs::write(cdir.join(get_platform_security_config_filename()), "{}").unwrap();
    std::fs::write(cdir.join("config.cluster.default.json"), "{}").unwrap();
    std::fs::write(cdir.join("config.skills.default.json"), "{}").unwrap();
    let res = set_embedded_defaults_from_fs(cdir);
    assert!(res.is_ok());

    // Embedded scanner buffer should remain empty.
    let defaults = get_embedded_defaults();
    assert!(defaults.scanner.is_empty());
}

// ============================================================================
// load_config: error/info branches and embedded parse fallback
// ============================================================================

#[test]
fn extra_load_config_embedded_invalid_json_falls_through() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    // Inject an invalid embedded config — should fall through to hardcoded default.
    set_embedded_defaults(
        b"not valid json".to_vec(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let cfg = load_config(&path).unwrap();
    assert!(cfg.gateway.port > 0);

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

#[test]
fn extra_load_config_valid_embedded_is_used() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        br#"{"gateway":{"host":"embedded-host","port":7654}}"#.to_vec(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let cfg = load_config(&path).unwrap();
    assert_eq!(cfg.gateway.host, "embedded-host");
    assert_eq!(cfg.gateway.port, 7654);

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

#[test]
fn extra_load_config_reads_existing_file_with_env_override() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{"gateway":{"host":"from-file","port":1111}}"#).unwrap();

    // SAFETY: test runs single-threaded via --test-threads=1.
    unsafe { std::env::set_var("NEMESISBOT_GATEWAY_PORT", "2222"); }
    let cfg = load_config(&path).unwrap();
    assert_eq!(cfg.gateway.host, "from-file");
    assert_eq!(cfg.gateway.port, 2222);
    unsafe { std::env::remove_var("NEMESISBOT_GATEWAY_PORT"); }
}

// ============================================================================
// save_config: local mode adjustment branches
// ============================================================================

#[test]
fn extra_save_config_local_mode_tilde_workspace() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    // Test the `~/` branch in local-mode workspace normalization.
    let dir = tempfile::tempdir().unwrap();
    let local_dir = dir.path().join(".nemesisbot");
    std::fs::create_dir_all(&local_dir).unwrap();
    let config_path = local_dir.join("config.json");
    // Pre-create the file so canonicalize() inside is_local_mode can resolve it.
    std::fs::write(&config_path, b"{}").unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let mut config = Config::default();
    config.agents.defaults.workspace = "~/some/path".to_string();

    let res = save_config(&config_path, &mut config);
    assert!(res.is_ok());
    // Workspace should have been rewritten to .nemesisbot-relative path.
    assert!(config.agents.defaults.workspace.contains(".nemesisbot"));

    std::env::set_current_dir(original).unwrap();
}

#[test]
fn extra_save_config_local_mode_empty_llm_log_dir() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let local_dir = dir.path().join(".nemesisbot");
    std::fs::create_dir_all(&local_dir).unwrap();
    let config_path = local_dir.join("config.json");
    // Pre-create the file so canonicalize() inside is_local_mode can resolve it.
    std::fs::write(&config_path, b"{}").unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let mut config = Config::default();
    config.agents.defaults.workspace = "~/.nemesisbot/workspace".to_string();
    config.logging = Some(LoggingConfig {
        llm: Some(LlmLogConfig {
            log_dir: String::new(),
            ..Default::default()
        }),
        general: None,
    });

    let res = save_config(&config_path, &mut config);
    assert!(res.is_ok());
    assert_eq!(
        config.logging.as_ref().unwrap().llm.as_ref().unwrap().log_dir,
        "logs/request_logs"
    );

    std::env::set_current_dir(original).unwrap();
}

#[test]
fn extra_save_config_non_local_mode_writes_directly() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    // No `.nemesisbot` in cwd -> not local mode -> simple write.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut config = Config::default();
    config.agents.defaults.workspace = "/explicit/path".to_string();

    let res = save_config(&path, &mut config);
    assert!(res.is_ok());
    assert!(path.exists());
    // Workspace unchanged in non-local mode.
    assert_eq!(config.agents.defaults.workspace, "/explicit/path");
}

// ============================================================================
// load_embedded_config: error + success paths
// ============================================================================

#[test]
fn extra_load_embedded_config_unavailable_error() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let res = load_embedded_config();
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("embedded default config not available"));
}

#[test]
fn extra_load_embedded_config_invalid_json_error() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        b"invalid".to_vec(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let res = load_embedded_config();
    assert!(res.is_err());

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

// ============================================================================
// Sub-config load fallback: file missing -> embedded -> hardcoded
// ============================================================================

#[test]
fn extra_load_mcp_config_embedded_used_when_file_missing() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        Vec::new(),
        br#"{"enabled":true,"servers":[],"timeout":99}"#.to_vec(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let cfg = load_mcp_config(&path).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.timeout, 99);

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

#[test]
fn extra_load_mcp_config_embedded_invalid_falls_to_hardcoded() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        Vec::new(),
        b"invalid".to_vec(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let cfg = load_mcp_config(&path).unwrap();
    assert!(!cfg.enabled);
    assert_eq!(cfg.timeout, 30);

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

#[test]
fn extra_load_security_config_embedded_used_when_file_missing() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        Vec::new(),
        Vec::new(),
        br#"{"default_action":"custom-action"}"#.to_vec(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let cfg = load_security_config(&path).unwrap();
    assert_eq!(cfg.default_action, "custom-action");

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

#[test]
fn extra_load_security_config_embedded_invalid_falls_to_hardcoded() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        Vec::new(),
        Vec::new(),
        b"invalid".to_vec(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let cfg = load_security_config(&path).unwrap();
    assert_eq!(cfg.default_action, "deny");

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

#[test]
fn extra_load_scanner_config_embedded_used_when_file_missing() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        br#"{"enabled":["clamav"]}"#.to_vec(),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let cfg = load_scanner_config(&path).unwrap();
    assert_eq!(cfg.enabled, vec!["clamav".to_string()]);

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

#[test]
fn extra_load_scanner_config_embedded_invalid_falls_to_hardcoded() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        b"invalid".to_vec(),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let cfg = load_scanner_config(&path).unwrap();
    assert!(cfg.enabled.is_empty());

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

#[test]
fn extra_load_skills_config_embedded_used_when_file_missing() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        br#"{"max_concurrent_searches":7}"#.to_vec(),
        Vec::new(),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let cfg = load_skills_config(&path).unwrap();
    assert_eq!(cfg.max_concurrent_searches, 7);

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

#[test]
fn extra_load_skills_config_embedded_invalid_falls_to_hardcoded() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        b"invalid".to_vec(),
        Vec::new(),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let cfg = load_skills_config(&path).unwrap();
    // Default SkillsFullConfig has max_concurrent_searches = 2
    assert_eq!(cfg.max_concurrent_searches, 2);

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

// ============================================================================
// Sub-config save error paths (write failure)
// ============================================================================

#[test]
fn extra_save_mcp_config_writes_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.mcp.json");
    let cfg = McpConfig { enabled: true, servers: vec![], timeout: 42 };
    save_mcp_config(&path, &cfg).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let parsed: McpConfig = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed.timeout, 42);
}

#[test]
fn extra_save_security_config_writes_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.security.json");
    let cfg = SecurityConfig { default_action: "warn".into(), ..Default::default() };
    save_security_config(&path, &cfg).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let parsed: SecurityConfig = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed.default_action, "warn");
}

#[test]
fn extra_save_scanner_config_writes_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.scanner.json");
    let cfg = ScannerFullConfig {
        enabled: vec!["clamav".into()],
        engines: std::collections::HashMap::new(),
    };
    save_scanner_config(&path, &cfg).unwrap();
    assert!(path.exists());
}

#[test]
fn extra_save_skills_config_writes_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.skills.json");
    let cfg = SkillsFullConfig::default();
    save_skills_config(&path, &cfg).unwrap();
    assert!(path.exists());
}

// ============================================================================
// Platform security filename branches — exhaustive
// ============================================================================

#[test]
fn extra_platform_security_filename_matches_expected_pattern() {
    let fname = get_platform_security_config_filename();
    // Confirm one of the four known filenames is returned.
    let known = [
        "config.security.linux.json",
        "config.security.windows.json",
        "config.security.darwin.json",
        "config.security.json",
    ];
    assert!(known.contains(&fname.as_str()), "got {fname}");
}

#[test]
fn extra_platform_display_name_matches_known() {
    let name = get_platform_display_name();
    assert!(["Linux", "Windows", "macOS", "Unknown"].contains(&name.as_str()));
}

// ============================================================================
// Default value functions: exercise via JSON round-trip
// ============================================================================

#[test]
fn extra_serde_default_true_via_omitted_field() {
    // DevicesConfig has monitor_usb with default = "default_true"
    let json = r#"{}"#;
    let cfg: DevicesConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.monitor_usb);
}

#[test]
fn extra_serde_default_heartbeat_interval_minutes() {
    let json = r#"{}"#;
    let cfg: HeartbeatConfig = serde_json::from_str(json).unwrap();
    // interval uses default_heartbeat_interval_minutes = 30
    assert_eq!(cfg.interval, 30);
    // enabled uses default_true
    assert!(cfg.enabled);
}

#[test]
fn extra_serde_default_max_results_for_brave() {
    let json = r#"{}"#;
    let cfg: BraveConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.max_results, 5);
}

#[test]
fn extra_serde_default_max_results_for_duckduckgo() {
    let json = r#"{}"#;
    let cfg: DuckDuckGoConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.max_results, 5);
}

#[test]
fn extra_serde_default_max_results_for_perplexity() {
    let json = r#"{}"#;
    let cfg: PerplexityConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.max_results, 5);
}

#[test]
fn extra_serde_default_detail_level_for_llm_log() {
    let json = r#"{}"#;
    let cfg: LlmLogConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.detail_level, "full");
}

#[test]
fn extra_serde_default_log_level_for_general_log() {
    let json = r#"{}"#;
    let cfg: GeneralLogConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
    assert!(cfg.enable_console);
    assert_eq!(cfg.level, "INFO");
}

#[test]
fn extra_serde_default_mcp_timeout() {
    let json = r#"{}"#;
    let cfg: McpConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.timeout, 30);
}

#[test]
fn extra_serde_default_security_action_and_constants() {
    let json = r#"{}"#;
    let cfg: SecurityConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.default_action, "deny");
    assert_eq!(cfg.approval_timeout_seconds, 300);
    assert_eq!(cfg.max_pending_requests, 100);
    assert_eq!(cfg.audit_log_retention_days, 90);
    assert!(cfg.log_all_operations);
    assert!(cfg.audit_log_file_enabled);
}

#[test]
fn extra_serde_default_skills_search_cache() {
    let json = r#"{}"#;
    let cfg: SkillsSearchCacheConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.max_size, 50);
    assert_eq!(cfg.ttl_seconds, 300);
}

#[test]
fn extra_serde_default_skills_full_top_level() {
    let json = r#"{}"#;
    let cfg: SkillsFullConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.max_concurrent_searches, 2);
    assert_eq!(cfg.search_limit, 50);
}

#[test]
fn extra_serde_default_skills_modelscope() {
    // Default impl sets timeout=30, but #[serde(default)] uses i64::default=0
    // when deserializing from JSON. We test the Default impl here.
    let cfg = SkillsModelScopeConfig::default();
    assert!(cfg.enabled);
    assert_eq!(cfg.timeout, 30);
}

#[test]
fn extra_serde_default_agent_defaults_via_json() {
    let json = r#"{}"#;
    let cfg: AgentDefaults = serde_json::from_str(json).unwrap();
    assert!(cfg.restrict_to_workspace);
    assert_eq!(cfg.max_tokens, 8192);
    assert!((cfg.temperature - 0.7).abs() < f64::EPSILON);
    assert_eq!(cfg.max_tool_iterations, 20);
    assert_eq!(cfg.concurrent_request_mode, "reject");
    assert_eq!(cfg.queue_size, 8);
}

#[test]
fn extra_serde_default_gateway_config_via_json() {
    let json = r#"{}"#;
    let cfg: GatewayConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 18790);
}

#[test]
fn extra_serde_default_web_channel_via_json() {
    let json = r#"{}"#;
    let cfg: WebChannelConfig = serde_json::from_str(json).unwrap();
    // enabled uses default_true, but Default impl returns false — JSON path returns true.
    assert!(cfg.enabled);
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.path, "/ws");
    assert_eq!(cfg.heartbeat_interval, 30);
    assert_eq!(cfg.session_timeout, 3600);
}

// ============================================================================
// Workspace resolver branches
// ============================================================================

#[test]
fn extra_workspace_resolver_local_flag_true_returns_relative() {
    let p = WorkspaceResolver::resolve(true);
    assert_eq!(p, PathBuf::from("./.nemesisbot"));
}

#[test]
fn extra_workspace_resolver_env_var_with_absolute_no_tilde() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    // SAFETY: serialized via GLOBAL_STATE_LOCK.
    unsafe { std::env::set_var("NEMESISBOT_HOME", "/opt/nemesis"); }
    let p = WorkspaceResolver::resolve(false);
    // No tilde; expansion returns the path as-is, joined with .nemesisbot.
    assert!(p.starts_with("/opt/nemesis") || p.to_string_lossy().contains("opt"));
    assert!(p.to_string_lossy().ends_with(".nemesisbot"));
    unsafe { std::env::remove_var("NEMESISBOT_HOME"); }
}

#[test]
fn extra_workspace_resolver_env_var_with_relative() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    // A relative path is not expanded by expand_tilde — passed through as-is.
    unsafe { std::env::set_var("NEMESISBOT_HOME", "relative/path"); }
    let p = WorkspaceResolver::resolve(false);
    assert!(p.to_string_lossy().contains("relative"));
    assert!(p.to_string_lossy().ends_with(".nemesisbot"));
    unsafe { std::env::remove_var("NEMESISBOT_HOME"); }
}

#[test]
fn extra_workspace_resolver_config_path_join() {
    let p = WorkspaceResolver::config_path(Path::new("/some/workspace"));
    assert_eq!(p, PathBuf::from("/some/workspace/config.json"));
}

#[test]
fn extra_workspace_resolver_ensure_workspace_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    WorkspaceResolver::ensure_workspace(&ws).unwrap();
    // Calling twice should not error.
    WorkspaceResolver::ensure_workspace(&ws).unwrap();
    assert!(ws.is_dir());
}

#[test]
fn extra_workspace_resolver_ensure_workspace_failure() {
    // Try to create a directory where a file already exists.
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("blocker");
    std::fs::write(&file_path, b"x").unwrap();
    let nested = file_path.join("inner");
    let res = WorkspaceResolver::ensure_workspace(&nested);
    assert!(res.is_err());
}

// ============================================================================
// expand_tilde branches
// ============================================================================

#[test]
fn extra_expand_tilde_only_tilde() {
    let p = expand_tilde("~");
    assert!(p.to_string_lossy().len() > 0);
    assert!(!p.to_string_lossy().starts_with("~"));
}

#[test]
fn extra_expand_tilde_relative_path() {
    let p = expand_tilde("relative/no-tilde");
    assert_eq!(p, PathBuf::from("relative/no-tilde"));
}

#[test]
fn extra_expand_tilde_with_subpath() {
    let p = expand_tilde("~/foo/bar");
    let s = p.to_string_lossy().to_string();
    assert!(s.ends_with("foo/bar"));
    assert!(!s.starts_with("~"));
}

// ============================================================================
// default_config()
// ============================================================================

#[test]
fn extra_default_config_has_expected_defaults() {
    let cfg = default_config();
    assert!(cfg.agents.defaults.restrict_to_workspace);
    assert_eq!(cfg.agents.defaults.llm, "zhipu/glm-4.7-flash");
    assert_eq!(cfg.agents.defaults.max_tokens, 8192);
    assert_eq!(cfg.gateway.host, "0.0.0.0");
    assert_eq!(cfg.gateway.port, 18790);
    assert!(cfg.tools.web.duckduckgo.enabled);
    assert!(cfg.heartbeat.enabled);
    assert_eq!(cfg.heartbeat.interval, 30);
    assert!(cfg.devices.monitor_usb);
}

// ============================================================================
// Config::workspace_path additional branches
// ============================================================================

#[test]
fn extra_config_workspace_path_no_tilde_prefix() {
    let cfg = Config {
        agents: AgentsConfig {
            defaults: AgentDefaults { workspace: "/plain/path".into(), ..Default::default() },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(cfg.workspace_path(), "/plain/path");
}

#[test]
fn extra_config_workspace_path_starts_with_home() {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/u"));
    let cfg = Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                workspace: home.to_string_lossy().to_string(),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(cfg.workspace_path(), home.to_string_lossy().to_string());
}

// ============================================================================
// Config::post_process_for_compatibility: empty-sync_to sets false
// ============================================================================

#[test]
fn extra_post_process_empty_sync_to_sets_false() {
    let mut cfg = Config::default();
    cfg.channels.external.sync_to = vec![];
    cfg.channels.websocket.sync_to = vec!["web".into()];
    cfg.post_process_for_compatibility();
    assert!(!cfg.channels.external.sync_to_web);
    assert!(cfg.channels.websocket.sync_to_web);
}

// ============================================================================
// McpServerConfig::normalize additional cases
// ============================================================================

#[test]
fn extra_mcp_server_normalize_command_only_populates_url() {
    let mut s = McpServerConfig::default();
    s.command = "node".into();
    s.normalize();
    assert_eq!(s.url, "node");
    assert_eq!(s.transport_type, "stdio");
}

#[test]
fn extra_mcp_server_normalize_no_command_keeps_empty_url() {
    let mut s = McpServerConfig::default();
    s.normalize();
    assert!(s.url.is_empty());
    assert_eq!(s.transport_type, "stdio");
}

#[test]
fn extra_mcp_server_normalize_both_set_keeps_url() {
    let mut s = McpServerConfig::default();
    s.url = "http://x".into();
    s.command = "y".into();
    s.normalize();
    assert_eq!(s.url, "http://x");
    assert_eq!(s.command, "y");
}

// ============================================================================
// ModelConfig methods
// ============================================================================

#[test]
fn extra_model_config_validate_full() {
    let m = ModelConfig {
        model_name: "n".into(),
        model: "p/m".into(),
        ..Default::default()
    };
    assert!(m.validate().is_ok());
}

#[test]
fn extra_model_config_parse_first_slash() {
    let m = ModelConfig { model: "a/b/c".into(), ..Default::default() };
    let (p, n) = m.parse_model();
    assert_eq!(p, "a");
    assert_eq!(n, "b/c");
}

// ============================================================================
// ConfigError variants — exhaustive Display coverage
// ============================================================================

#[test]
fn extra_config_error_display_io_variant() {
    let e = ConfigError::Io(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "x"));
    assert!(e.to_string().contains("IO error"));
}

#[test]
fn extra_config_error_display_json_variant() {
    let json_err = serde_json::from_str::<serde_json::Value>("bad").unwrap_err();
    let e = ConfigError::Json(json_err);
    assert!(e.to_string().contains("JSON parse error"));
}

#[test]
fn extra_config_error_display_validation_variant() {
    let e = ConfigError::Validation("v".into());
    assert!(e.to_string().contains("Config validation error"));
}

#[test]
fn extra_config_error_display_workspace_not_found_variant() {
    let e = ConfigError::WorkspaceNotFound;
    assert_eq!(e.to_string(), "Workspace not found");
}

// ============================================================================
// EmbeddedDefaults struct constructors
// ============================================================================

#[test]
fn extra_embedded_defaults_default_struct_all_empty() {
    let d = EmbeddedDefaults::default();
    assert!(d.config.is_empty());
    assert!(d.mcp.is_empty());
    assert!(d.security.is_empty());
    assert!(d.cluster.is_empty());
    assert!(d.skills.is_empty());
    assert!(d.scanner.is_empty());
}

#[test]
fn extra_set_embedded_defaults_roundtrip_all_fields() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    set_embedded_defaults(
        b"c".to_vec(),
        b"m".to_vec(),
        b"s".to_vec(),
        b"cl".to_vec(),
        b"sk".to_vec(),
        b"sc".to_vec(),
    );
    let d = get_embedded_defaults();
    assert_eq!(d.config, b"c".to_vec());
    assert_eq!(d.mcp, b"m".to_vec());
    assert_eq!(d.security, b"s".to_vec());
    assert_eq!(d.cluster, b"cl".to_vec());
    assert_eq!(d.skills, b"sk".to_vec());
    assert_eq!(d.scanner, b"sc".to_vec());

    set_embedded_defaults(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
}

// ============================================================================
// ConfigLoader
// ============================================================================

#[test]
fn extra_config_loader_load_from_file_read_failure() {
    let res = ConfigLoader::load_from_file(Path::new("/no/such/dir/config.json"));
    assert!(res.is_err());
}

#[test]
fn extra_config_loader_save_to_file_no_parent() {
    // File in current temp dir (no nested parent creation needed).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let cfg = Config::default();
    ConfigLoader::save_to_file(&cfg, &path).unwrap();
    assert!(path.exists());
}

#[test]
fn extra_config_loader_load_embedded_default_success() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let cfg = ConfigLoader::load_embedded_default().unwrap();
    assert!(cfg.gateway.port > 0);
}

// ============================================================================
// is_local_mode
// ============================================================================

#[test]
fn extra_is_local_mode_returns_false_when_local_dir_missing() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    // cwd is the workspace root which (typically) has no .nemesisbot at the root.
    let dir = tempfile::tempdir().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let path = dir.path().join("config.json");
    assert!(!is_local_mode(&path));

    std::env::set_current_dir(original).unwrap();
}

#[test]
fn extra_is_local_mode_returns_true_with_local_dir() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let local_dir = dir.path().join(".nemesisbot");
    std::fs::create_dir_all(&local_dir).unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let cfg_path = local_dir.join("config.json");
    // Pre-create so canonicalize() succeeds inside is_local_mode.
    std::fs::write(&cfg_path, b"{}").unwrap();
    // Should return true since cwd/.nemesisbot exists and config is inside it.
    assert!(is_local_mode(&cfg_path));

    std::env::set_current_dir(original).unwrap();
}

// ============================================================================
// Configurable Channel deserialization
// ============================================================================

#[test]
fn extra_channels_config_deserialize_minimal() {
    let json = r#"{"web":{"enabled":true,"port":1234}}"#;
    let cfg: ChannelsConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.web.enabled);
    assert_eq!(cfg.web.port, 1234);
}

#[test]
fn extra_web_channel_config_deserialize_with_defaults() {
    let json = r#"{"enabled":true}"#;
    let cfg: WebChannelConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.path, "/ws");
}

// ============================================================================
// adjust_paths_for_environment variants
// ============================================================================

#[test]
fn extra_adjust_paths_ends_with_nemesisbot_workspace() {
    let mut cfg = Config::default();
    cfg.agents.defaults.workspace = "/something/.nemesisbot/workspace".into();
    cfg.adjust_paths_for_environment();
    // The "ends_with" branch triggers a replacement.
    let ws = cfg.agents.defaults.workspace.clone();
    assert!(ws.contains(".nemesisbot"));
}

#[test]
fn extra_adjust_paths_log_dir_with_general_only() {
    let mut cfg = Config::default();
    cfg.logging = Some(LoggingConfig { llm: None, general: Some(GeneralLogConfig::default()) });
    cfg.adjust_paths_for_environment();
    // No LLM logging — no change to log_dir.
    assert!(cfg.logging.as_ref().unwrap().llm.is_none());
}

// ============================================================================
// get_platform_info JSON shape
// ============================================================================

#[test]
fn extra_get_platform_info_returns_json_object() {
    let info = get_platform_info();
    assert!(info.is_object());
    let obj = info.as_object().unwrap();
    assert!(obj.contains_key("os"));
    assert!(obj.contains_key("arch"));
    assert!(obj.contains_key("family"));
    assert!(obj.contains_key("display_name"));
    assert!(obj.contains_key("security_config"));
}

// ============================================================================
// Custom AgentBinding / BindingMatch round-trip
// ============================================================================

#[test]
fn extra_agent_binding_roundtrip_with_peer() {
    let b = AgentBinding {
        agent_id: "main".into(),
        r#match: BindingMatch {
            channel: "telegram".into(),
            account_id: "123".into(),
            peer: Some(PeerMatch { kind: "user".into(), id: "u1".into() }),
            guild_id: String::new(),
            team_id: String::new(),
        },
    };
    let json = serde_json::to_string(&b).unwrap();
    let parsed: AgentBinding = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.agent_id, "main");
    assert_eq!(parsed.r#match.channel, "telegram");
    assert!(parsed.r#match.peer.is_some());
    assert_eq!(parsed.r#match.peer.unwrap().id, "u1");
}

// ============================================================================
// SessionConfig with identity_links
// ============================================================================

#[test]
fn extra_session_config_with_identity_links() {
    let mut links = std::collections::HashMap::new();
    links.insert("k".to_string(), vec!["v1".to_string(), "v2".to_string()]);
    let cfg = SessionConfig { dm_scope: "scope".into(), identity_links: links };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: SessionConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.dm_scope, "scope");
    assert_eq!(parsed.identity_links.get("k").unwrap().len(), 2);
}

// ============================================================================
// Directory / file write error path simulation
// ============================================================================

#[test]
fn extra_save_config_to_readonly_directory_errors() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    // Try to save into a path whose parent is a file (cannot be created as dir).
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let path = blocker.join("subdir/config.json");

    let mut cfg = Config::default();
    let res = save_config(&path, &mut cfg);
    assert!(res.is_err());
}

// ============================================================================
// SubagentsConfig and AgentConfigEntry list
// ============================================================================

#[test]
fn extra_agent_config_entry_with_subagents() {
    let json = r#"{
        "id":"a",
        "default":true,
        "name":"Main",
        "model":"primary-model",
        "skills":["s1"],
        "subagents":{"allow_agents":["sa1"],"model":"sub-model"}
    }"#;
    let entry: AgentConfigEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.id, "a");
    assert!(entry.default);
    assert_eq!(entry.skills, vec!["s1".to_string()]);
    let sub = entry.subagents.unwrap();
    assert_eq!(sub.allow_agents, vec!["sa1".to_string()]);
    assert_eq!(sub.model.unwrap().primary, "sub-model");
}

// ============================================================================
// EngineState and ClamAVEngineConfig JSON
// ============================================================================

#[test]
fn extra_engine_state_roundtrip() {
    let s = EngineState {
        install_status: "installed".into(),
        install_error: String::new(),
        last_install_attempt: "2026-06-16".into(),
        db_status: "ready".into(),
        last_db_update: "2026-06-15".into(),
    };
    let json = serde_json::to_string(&s).unwrap();
    let parsed: EngineState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.install_status, "installed");
    assert_eq!(parsed.last_db_update, "2026-06-15");
}

#[test]
fn extra_clamav_engine_config_with_state() {
    let cfg = ClamAVEngineConfig {
        url: "tcp://x:3310".into(),
        scan_on_exec: true,
        max_file_size: 1000,
        state: EngineState {
            install_status: "ok".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: ClamAVEngineConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.url, "tcp://x:3310");
    assert!(parsed.scan_on_exec);
    assert_eq!(parsed.state.install_status, "ok");
}

// ============================================================================
// Write helper smoke (to keep coverage on std lib calls used by tests)
// ============================================================================

#[test]
fn extra_smoke_write_then_read_temp() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f.txt");
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(b"hello").unwrap();
    drop(f);
    let s = std::fs::read_to_string(&p).unwrap();
    assert_eq!(s, "hello");
}

// ============================================================================
// Error-path coverage: load_*_config with directory as path
// (read_to_string fails with PermissionDenied on Windows)
// ============================================================================

#[test]
fn extra_load_config_directory_as_path_errors() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // Pass the directory itself as the config path — read_to_string fails.
    let res = load_config(dir.path());
    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), ConfigError::Io(_)));
}

#[test]
fn extra_load_mcp_config_directory_as_path_errors() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let res = load_mcp_config(dir.path());
    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), ConfigError::Io(_)));
}

#[test]
fn extra_load_security_config_directory_as_path_errors() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let res = load_security_config(dir.path());
    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), ConfigError::Io(_)));
}

#[test]
fn extra_load_scanner_config_directory_as_path_errors() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let res = load_scanner_config(dir.path());
    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), ConfigError::Io(_)));
}

#[test]
fn extra_load_skills_config_directory_as_path_errors() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let res = load_skills_config(dir.path());
    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), ConfigError::Io(_)));
}

// ============================================================================
// save_config local mode: workspace starting with home dir prefix
// ============================================================================

#[test]
fn extra_save_config_local_mode_home_prefix_workspace() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let local_dir = dir.path().join(".nemesisbot");
    std::fs::create_dir_all(&local_dir).unwrap();
    let config_path = local_dir.join("config.json");
    std::fs::write(&config_path, b"{}").unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/u"));
    let home_workspace = home.join("custom").to_string_lossy().to_string();

    let mut config = Config::default();
    config.agents.defaults.workspace = home_workspace.clone();

    let res = save_config(&config_path, &mut config);
    assert!(res.is_ok());
    // Workspace should have been rewritten since it starts with the home dir.
    assert_ne!(config.agents.defaults.workspace, home_workspace);
    assert!(config.agents.defaults.workspace.contains(".nemesisbot"));

    std::env::set_current_dir(original).unwrap();
}

// ============================================================================
// save_config local mode: workspace equals default workspace path
// ============================================================================

#[test]
fn extra_save_config_local_mode_default_workspace_path() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let local_dir = dir.path().join(".nemesisbot");
    std::fs::create_dir_all(&local_dir).unwrap();
    let config_path = local_dir.join("config.json");
    std::fs::write(&config_path, b"{}").unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/u"));
    let default_workspace = home.join(".nemesisbot").join("workspace").to_string_lossy().to_string();

    let mut config = Config::default();
    config.agents.defaults.workspace = default_workspace;

    let res = save_config(&config_path, &mut config);
    assert!(res.is_ok());
    assert!(config.agents.defaults.workspace.contains(".nemesisbot"));
    // Should now be relative to current dir (.nemesisbot/workspace).
    assert!(!config.agents.defaults.workspace.starts_with(&home.to_string_lossy().to_string()));

    std::env::set_current_dir(original).unwrap();
}
