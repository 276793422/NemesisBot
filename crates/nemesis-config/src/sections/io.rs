// 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

// 跨子文件引用（机械拆分）：serde default helper 定义在 agents.rs
use super::agents::default_executor_tool_timeout_secs;

/// Holds embedded default configuration data (set at compile time).
#[derive(Debug, Clone, Default)]
pub struct EmbeddedDefaults {
    pub config: Vec<u8>,
    pub mcp: Vec<u8>,
    pub security: Vec<u8>,
    pub cluster: Vec<u8>,
    pub skills: Vec<u8>,
    pub scanner: Vec<u8>,
}

/// Global embedded defaults storage.
static EMBEDDED_DEFAULTS: std::sync::OnceLock<std::sync::RwLock<EmbeddedDefaults>> =
    std::sync::OnceLock::new();

fn embedded_defaults() -> &'static std::sync::RwLock<EmbeddedDefaults> {
    EMBEDDED_DEFAULTS.get_or_init(|| std::sync::RwLock::new(EmbeddedDefaults::default()))
}

/// Get the current embedded defaults.
pub fn get_embedded_defaults() -> EmbeddedDefaults {
    embedded_defaults().read().unwrap().clone()
}

/// Set embedded defaults from byte arrays.
pub fn set_embedded_defaults(
    config: Vec<u8>,
    mcp: Vec<u8>,
    security: Vec<u8>,
    cluster: Vec<u8>,
    skills: Vec<u8>,
    scanner: Vec<u8>,
) {
    let mut guard = embedded_defaults().write().unwrap();
    guard.config = config;
    guard.mcp = mcp;
    guard.security = security;
    guard.cluster = cluster;
    guard.skills = skills;
    guard.scanner = scanner;
}

/// Set embedded defaults from a filesystem path (directory of config files).
///
/// This mirrors the Go `SetEmbeddedDefaultsFromFS` function. Reads the
/// standard set of configuration files from the given directory:
/// - `config.default.json`
/// - `config.mcp.default.json`
/// - platform-specific security config
/// - `config.cluster.default.json`
/// - `config.skills.default.json`
/// - `config.scanner.default.json` (optional)
pub fn set_embedded_defaults_from_fs(config_dir: &Path) -> Result<()> {
    // Read config.default.json
    let config_path = config_dir.join("config.default.json");
    let config_data = std::fs::read(&config_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.default.json: {}", e),
        ))
    })?;

    // Read config.mcp.default.json
    let mcp_path = config_dir.join("config.mcp.default.json");
    let mcp_data = std::fs::read(&mcp_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.mcp.default.json: {}", e),
        ))
    })?;

    // Read platform-specific security config
    let security_filename = get_platform_security_config_filename();
    let security_path = config_dir.join(&security_filename);
    let security_data = std::fs::read(&security_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read {}: {}", security_filename, e),
        ))
    })?;

    // Read config.cluster.default.json
    let cluster_path = config_dir.join("config.cluster.default.json");
    let cluster_data = std::fs::read(&cluster_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.cluster.default.json: {}", e),
        ))
    })?;

    // Read config.skills.default.json
    let skills_path = config_dir.join("config.skills.default.json");
    let skills_data = std::fs::read(&skills_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.skills.default.json: {}", e),
        ))
    })?;

    // Read config.scanner.default.json (optional -- don't fail if missing)
    let scanner_path = config_dir.join("config.scanner.default.json");
    let scanner_data = std::fs::read(&scanner_path).unwrap_or_default();

    set_embedded_defaults(
        config_data,
        mcp_data,
        security_data,
        cluster_data,
        skills_data,
        scanner_data,
    );

    Ok(())
}

// ============================================================================
// Provider resolution functions are now in the provider_resolver module.
// They are re-exported above for backward compatibility.
// ============================================================================

/// Load configuration from a file path.
/// Mirrors Go LoadConfig: tries file → embedded default → hardcoded default.
/// After loading, applies environment variable overrides (NEMESISBOT_*).
pub fn load_config(config_path: &Path) -> Result<Config> {
    info!("[Config] Loading config from: {:?}", config_path);
    if config_path.exists() {
        let content = nemesis_utils::fsutil::read_config_string(config_path).map_err(|e| {
            error!(
                "[Config] Failed to read config file {:?}: {}",
                config_path, e
            );
            e
        })?;
        let mut config: Config = serde_json::from_str(&content).map_err(|e| {
            error!("[Config] Failed to parse config JSON: {}", e);
            e
        })?;
        apply_env_overrides(&mut config);
        config.post_process_for_compatibility();
        config.adjust_paths_for_environment();
        info!("[Config] Config loaded successfully from file");
        return Ok(config);
    }

    info!(
        "[Config] Config file not found at {:?}, trying embedded default",
        config_path
    );
    let defaults = get_embedded_defaults();
    if !defaults.config.is_empty() {
        if let Ok(mut config) = serde_json::from_slice::<Config>(&defaults.config) {
            apply_env_overrides(&mut config);
            config.post_process_for_compatibility();
            config.adjust_paths_for_environment();
            info!("[Config] Config loaded successfully from embedded default");
            return Ok(config);
        } else {
            warn!(
                "[Config] Failed to parse embedded default config, falling back to hardcoded default"
            );
        }
    }

    info!("[Config] Using hardcoded default config");
    let mut config = default_config();
    apply_env_overrides(&mut config);
    config.post_process_for_compatibility();
    config.adjust_paths_for_environment();
    info!("[Config] Config loaded successfully (hardcoded default)");
    Ok(config)
}

/// Save configuration to a file path.
/// Mirrors Go SaveConfig: auto-adjusts paths for local mode before saving.
pub fn save_config(config_path: &Path, config: &mut Config) -> Result<()> {
    info!("[Config] Saving config to: {:?}", config_path);
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            error!("[Config] Failed to create config directory: {}", e);
            e
        })?;
    }

    // Auto-adjust paths for local mode before saving (mirrors Go behavior)
    // When --local is used or.nemesisbot is detected in current directory,
    // workspace and other paths should be relative to the config directory
    if is_local_mode(config_path) {
        let config_dir = config_path.parent().unwrap_or(Path::new("."));
        if config_dir.file_name().is_some_and(|n| n == ".nemesisbot") {
            // Get the expected default workspace path
            let user_home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
            let default_workspace_path = user_home.join(".nemesisbot").join("workspace");

            let ws = &config.agents.defaults.workspace;
            // 只重写「默认工作区」的等价拼法：旧
            // starts_with("~/")/starts_with(home) 会把任何 home 下的自定义
            // 路径（甚至仅共享字符串前缀的 C:\Users\me2 家族）误判成默认
            // 形态，每次落盘静默改写成.nemesisbot/workspace，用户自定义
            // workspace 被丢弃（会话/历史/记忆集体「消失」）。Path 组件相等
            // 比较天然免疫字符串前缀堂亲与分隔符/大小写变体；空串不在此
            // 处理（加载期 adjust_paths_for_environment 会解析默认值）。
            let is_default_workspace = ws == "~/.nemesisbot/workspace"
                || ws == "~\\.nemesisbot\\workspace"
                || Path::new(ws) == default_workspace_path;
            if is_default_workspace {
                config.agents.defaults.workspace = PathBuf::from(".nemesisbot")
                    .join("workspace")
                    .to_string_lossy()
                    .to_string();
            }

            // Normalize logging directory
            if let Some(ref mut logging) = config.logging
                && let Some(ref mut llm) = logging.llm
                && llm.log_dir.is_empty()
            {
                llm.log_dir = "logs/request_logs".to_string();
            }
        }
    }

    let content = serde_json::to_string_pretty(&*config).map_err(|e| {
        error!("[Config] Failed to serialize config: {}", e);
        e
    })?;
    // 统一原子写入（tmp+sync+rename，失败清理）。
    // 0600（unix）保护 API keys/tokens；Windows 权限为 no-op。
    nemesis_utils::write_file_atomic(&config_path.to_string_lossy(), content.as_bytes(), 0o600)
        .map_err(|e| {
            error!("[Config] Failed to write config file: {}", e);
            // helper 的 String 错误带步骤上下文（create temp/write/rename），
            // 包回 Io 变体保持错误分类语义。
            ConfigError::Io(std::io::Error::other(e))
        })?;
    info!("[Config] Config saved successfully");
    Ok(())
}

/// Check if we're in local mode (config is in current directory's.nemesisbot).
pub fn is_local_mode(config_path: &Path) -> bool {
    if let Ok(cwd) = std::env::current_dir() {
        let local_dir = cwd.join(".nemesisbot");
        if local_dir.exists()
            && let Ok(canonical_config) = config_path.canonicalize()
            && let Ok(canonical_local) = local_dir.canonicalize()
            && let Some(parent) = canonical_config.parent()
        {
            return parent == canonical_local;
        }
    }
    false
}

/// Apply environment variable overrides to the config.
/// Mirrors Go's `env.Parse(cfg)` which reads NEMESISBOT_* env vars.
///
/// Supported environment variables (matching Go's env tags):
/// - NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE
/// - NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE
/// - NEMESISBOT_AGENTS_DEFAULTS_LLM
/// - NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL
/// - NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS
/// - NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE
/// - NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS
/// - NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE
/// - NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE
/// - NEMESISBOT_CHANNELS_WEB_ENABLED
/// - NEMESISBOT_CHANNELS_WEB_HOST
/// - NEMESISBOT_CHANNELS_WEB_PORT
/// - NEMESISBOT_GATEWAY_HOST
/// - NEMESISBOT_GATEWAY_PORT
/// - NEMESISBOT_HEARTBEAT_ENABLED
/// - NEMESISBOT_HEARTBEAT_INTERVAL
/// - NEMESISBOT_SECURITY_ENABLED
/// - NEMESISBOT_FORGE_ENABLED
///
/// 布尔覆盖（`*_ENABLED` / `RESTRICT_TO_WORKSPACE`）的
/// 识别集 = Go `strconv.ParseBool` 全集（true/false/t/f/1/0，大小写不敏感）
/// 与项目自身宽容惯例 yes/no/on/off；值无法识别时**保留 config 文件值 +
/// warn 留痕**——与同函数数值臂的「解析失败 = 不动 config」惯例一致，
/// 严禁旧形态的 `parse.unwrap_or(硬编码默认)`（设 `=1` 想开安全会被
/// 静默关掉、设 `=0` 想关 web 会被静默打开——intent 反转 + fail-open）。
pub fn apply_env_overrides(config: &mut Config) {
    // Agent defaults
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE") {
        config.agents.defaults.workspace = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE") {
        match parse_env_bool(&v) {
            Some(b) => config.agents.defaults.restrict_to_workspace = b,
            None => warn!(
                "[Config] NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE={v:?} 值无法识别——保留 config 文件值 {}",
                config.agents.defaults.restrict_to_workspace
            ),
        }
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_LLM") {
        config.agents.defaults.llm = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL") {
        config.agents.defaults.image_model = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS")
        && let Ok(n) = v.parse()
    {
        config.agents.defaults.max_tokens = n;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE")
        && let Ok(f) = v.parse()
    {
        config.agents.defaults.temperature = f;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS")
        && let Ok(n) = v.parse()
    {
        config.agents.defaults.max_tool_iterations = n;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE") {
        config.agents.defaults.concurrent_request_mode = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE")
        && let Ok(n) = v.parse()
    {
        config.agents.defaults.queue_size = n;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_MAX_CONCURRENT_TURNS")
        && let Ok(n) = v.parse()
    {
        config.agents.defaults.max_concurrent_turns = n;
    }

    // Web channel
    if let Ok(v) = std::env::var("NEMESISBOT_CHANNELS_WEB_ENABLED") {
        match parse_env_bool(&v) {
            Some(b) => config.channels.web.enabled = b,
            None => warn!(
                "[Config] NEMESISBOT_CHANNELS_WEB_ENABLED={v:?} 值无法识别——保留 config 文件值 {}",
                config.channels.web.enabled
            ),
        }
    }
    if let Ok(v) = std::env::var("NEMESISBOT_CHANNELS_WEB_HOST") {
        config.channels.web.host = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_CHANNELS_WEB_PORT")
        && let Ok(n) = v.parse()
    {
        config.channels.web.port = n;
    }

    // Gateway
    if let Ok(v) = std::env::var("NEMESISBOT_GATEWAY_HOST") {
        config.gateway.host = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_GATEWAY_PORT")
        && let Ok(n) = v.parse()
    {
        config.gateway.port = n;
    }

    // Heartbeat
    if let Ok(v) = std::env::var("NEMESISBOT_HEARTBEAT_ENABLED") {
        match parse_env_bool(&v) {
            Some(b) => config.heartbeat.enabled = b,
            None => warn!(
                "[Config] NEMESISBOT_HEARTBEAT_ENABLED={v:?} 值无法识别——保留 config 文件值 {}",
                config.heartbeat.enabled
            ),
        }
    }
    if let Ok(v) = std::env::var("NEMESISBOT_HEARTBEAT_INTERVAL")
        && let Ok(n) = v.parse()
    {
        config.heartbeat.interval = n;
    }

    // Security
    if let Ok(v) = std::env::var("NEMESISBOT_SECURITY_ENABLED") {
        match parse_env_bool(&v) {
            Some(enabled) => config.security = Some(SecurityFlagConfig { enabled }),
            None => warn!(
                "[Config] NEMESISBOT_SECURITY_ENABLED={v:?} 值无法识别——保留 config 文件值（安全开关不会被静默翻转，当前 {:?}）",
                config.security.as_ref().map(|s| s.enabled)
            ),
        }
    }

    // Forge
    if let Ok(v) = std::env::var("NEMESISBOT_FORGE_ENABLED") {
        match parse_env_bool(&v) {
            Some(enabled) => config.forge = Some(ForgeFlagConfig { enabled }),
            None => warn!(
                "[Config] NEMESISBOT_FORGE_ENABLED={v:?} 值无法识别——保留 config 文件值（当前 {:?}）",
                config.forge.as_ref().map(|f| f.enabled)
            ),
        }
    }

    // Session
    if let Ok(v) = std::env::var("NEMESISBOT_SESSION_DM_SCOPE") {
        config.session.dm_scope = v;
    }
}

/// 布尔 env 覆盖的宽容识别。
///
/// 识别集：`true/false/t/f/1/0`（= Go `strconv.ParseBool` 全集，大小写
/// 不敏感）+ `yes/no/on/off`（项目宽容布尔惯例，同 `normalize_stream_value`
/// 的出发点）。其余值返回 `None`——调用方保留 config 文件值并 warn。
fn parse_env_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "t" | "1" | "yes" | "on" => Some(true),
        "false" | "f" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Load embedded default config.
pub fn load_embedded_config() -> Result<Config> {
    info!("[Config] Loading embedded config");
    let defaults = get_embedded_defaults();
    if defaults.config.is_empty() {
        error!("[Config] Embedded default config not available");
        return Err(ConfigError::Validation(
            "embedded default config not available".into(),
        ));
    }
    let mut config: Config = serde_json::from_slice(&defaults.config).map_err(|e| {
        error!("[Config] Failed to parse embedded config: {}", e);
        e
    })?;
    config.post_process_for_compatibility();
    config.adjust_paths_for_environment();
    info!("[Config] Embedded config loaded successfully");
    Ok(config)
}

/// Create a fully populated Config with all sensible defaults.
pub fn default_config() -> Config {
    let ws = WorkspaceResolver::resolve(false)
        .join("workspace")
        .to_string_lossy()
        .to_string();
    Config {
        agents: AgentsConfig {
            discipline: DisciplineConfig::default(),
            prompt_system: default_prompt_system(),
            cache_warmer: CacheWarmerConfig::default(),
            claude_code_tool: ClaudeCodeToolConfig::default(),
            codex_tool: CodexToolConfig::default(),
            lsp_tool: LspToolConfig::default(),
            tool_doc_folding: ToolDocFoldingConfig::default(),
            subagent: SubagentConfig::default(),
            fs_watcher: FsWatcherConfig::default(),
            hidden_tools: Vec::new(),
            mcp_lazy_supply: false,
            roles: RolesConfig::default(),
            small_model: None,
            guardian_model: None,
            doom_loop_approval: false,
            image_downscale: true,
            lease_enabled: true,
            codemode: CodeModeConfig::default(),
            delegate: DelegateToolConfig::default(),
            turn_wal: true,
            provider_stream_events: true,
            stream_concat_retry: true,
            profession_tool_fence: true,
            lazy_tool_schema: None,
            executor_tool_timeout_secs: default_executor_tool_timeout_secs(),
            defaults: AgentDefaults {
                workspace: ws,
                restrict_to_workspace: true,
                llm: "zhipu/glm-4.7-flash".to_string(),
                max_tokens: nemesis_types::capability::DEFAULT_MAX_TOKENS,
                temperature: 0.7,
                max_tool_iterations: 100,
                // concurrent_request_mode intentionally omitted — filled by
                // `..Default::default` → `default_concurrent_request_mode`
                // (single truth source; a stale "reject" literal lived here
                // until and silently outvoted the default fn).
                queue_size: 8,
                ..Default::default()
            },
            list: vec![],
        },
        bindings: vec![],
        session: SessionConfig::default(),
        channels: ChannelsConfig {
            whatsapp: WhatsAppConfig {
                bridge_url: "ws://localhost:3001".to_string(),
                ..Default::default()
            },
            maixcam: MaixCamConfig {
                host: "0.0.0.0".to_string(),
                // 与 MaixCamConfig 类型缺省（8888）对齐——旧值 18790 与
                // gateway.port 出厂值撞号，maixcam 使能即绑定冲突
                // （BUG-20261107-015）。
                port: 8888,
                ..Default::default()
            },
            line: LineConfig {
                webhook_host: "0.0.0.0".to_string(),
                webhook_port: 18791,
                webhook_path: "/webhook/line".to_string(),
                ..Default::default()
            },
            onebot: OneBotConfig {
                ws_url: "ws://127.0.0.1:3001".to_string(),
                reconnect_interval: 5,
                ..Default::default()
            },
            web: WebChannelConfig {
                enabled: true,
                host: "0.0.0.0".to_string(),
                port: 8080,
                path: "/ws".to_string(),
                heartbeat_interval: 30,
                session_timeout: 3600,
                ..Default::default()
            },
            external: ExternalConfig {
                chat_id: "external:main".to_string(),
                sync_to: vec!["web".to_string()],
                ..Default::default()
            },
            ..Default::default()
        },
        model_list: vec![],
        gateway: GatewayConfig {
            host: "0.0.0.0".to_string(),
            port: 18790,
        },
        tools: ToolsConfig {
            hardware: HardwareToolsConfig::default(),
            web: WebToolsConfig {
                duckduckgo: DuckDuckGoConfig {
                    enabled: true,
                    max_results: 5,
                },
                brave: BraveConfig {
                    max_results: 5,
                    ..Default::default()
                },
                perplexity: PerplexityConfig {
                    max_results: 5,
                    ..Default::default()
                },
            },
            cron: CronToolsConfig {
                exec_timeout_minutes: 5,
            },
            exec: ExecConfig {
                enable_deny_patterns: true,
                ..Default::default()
            },
            image_gen: ImageGenConfig::default(),
        },
        heartbeat: HeartbeatConfig {
            enabled: true,
            interval: 30,
        },
        devices: DevicesConfig {
            monitor_usb: true,
            ..Default::default()
        },
        logging: Some(LoggingConfig {
            llm: Some(LlmLogConfig {
                enabled: false,
                log_dir: "logs/request_logs".to_string(),
                detail_level: "full".to_string(),
                save_raw: false,
            }),
            general: None,
        }),
        security: Some(SecurityFlagConfig { enabled: false }),
        forge: Some(ForgeFlagConfig { enabled: false }),
        user_profile: None,
        cluster: None,
        memory: None,
        skills: None,
        mcp: None,
        executor: None,
        debug: None,
        board: None,
        usage: None,
        terminal: None,
        projects: None,
        bridge: None,
        ui: None,
        connectors: None,
        ziwei: None,
        health: None,
        logs: None,
        checkpoint: None,
        plugins: None,
    }
}
/// Load MCP configuration from a separate config.mcp.json file.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_mcp_config(path: &Path) -> Result<McpConfig> {
    info!("[Config] Loading MCP config from: {:?}", path);
    if path.exists() {
        let content = nemesis_utils::fsutil::read_config_string(path).map_err(|e| {
            error!("[Config] Failed to read MCP config: {}", e);
            e
        })?;
        let cfg: McpConfig = serde_json::from_str(&content).map_err(|e| {
            error!("[Config] Failed to parse MCP config JSON: {}", e);
            e
        })?;
        info!("[Config] MCP config loaded from file");
        return Ok(cfg);
    }

    info!("[Config] MCP config file not found, trying embedded default");
    let defaults = get_embedded_defaults();
    if !defaults.mcp.is_empty()
        && let Ok(cfg) = serde_json::from_slice::<McpConfig>(&defaults.mcp)
    {
        info!("[Config] MCP config loaded from embedded default");
        return Ok(cfg);
    }

    info!("[Config] Using hardcoded default MCP config");
    Ok(McpConfig {
        enabled: false,
        servers: vec![],
        timeout: 30,
    })
}

/// Save MCP configuration to a separate config.mcp.json file.
pub fn save_mcp_config(path: &Path, cfg: &McpConfig) -> Result<()> {
    info!("[Config] Saving MCP config to: {:?}", path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            error!("[Config] Failed to create MCP config directory: {}", e);
            e
        })?;
    }
    let content = serde_json::to_string_pretty(cfg).map_err(|e| {
        error!("[Config] Failed to serialize MCP config: {}", e);
        e
    })?;
    // 统一原子写入；0600（unix）——子系统配置同样可能含凭据。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600).map_err(
        |e| {
            error!("[Config] Failed to write MCP config: {}", e);
            ConfigError::Io(std::io::Error::other(e))
        },
    )?;
    info!("[Config] MCP config saved successfully");
    Ok(())
}

/// Load security configuration from config.security.json.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_security_config(path: &Path) -> Result<SecurityConfig> {
    info!("[Config] Loading security config from: {:?}", path);
    if path.exists() {
        let content = nemesis_utils::fsutil::read_config_string(path).map_err(|e| {
            error!("[Config] Failed to read security config: {}", e);
            e
        })?;
        let cfg: SecurityConfig = serde_json::from_str(&content).map_err(|e| {
            error!("[Config] Failed to parse security config JSON: {}", e);
            e
        })?;
        info!("[Config] Security config loaded from file");
        return Ok(cfg);
    }

    info!("[Config] Security config file not found, trying embedded default");
    let defaults = get_embedded_defaults();
    if !defaults.security.is_empty()
        && let Ok(cfg) = serde_json::from_slice::<SecurityConfig>(&defaults.security)
    {
        info!("[Config] Security config loaded from embedded default");
        return Ok(cfg);
    }

    info!("[Config] Using hardcoded default security config");
    Ok(SecurityConfig::default())
}

/// Save security configuration to config.security.json.
pub fn save_security_config(path: &Path, cfg: &SecurityConfig) -> Result<()> {
    info!("[Config] Saving security config to: {:?}", path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            error!("[Config] Failed to create security config directory: {}", e);
            e
        })?;
    }
    let content = serde_json::to_string_pretty(cfg).map_err(|e| {
        error!("[Config] Failed to serialize security config: {}", e);
        e
    })?;
    // 统一原子写入；0600（unix）——含 limits 规则与策略。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600).map_err(
        |e| {
            error!("[Config] Failed to write security config: {}", e);
            ConfigError::Io(std::io::Error::other(e))
        },
    )?;
    info!("[Config] Security config saved successfully");
    Ok(())
}

/// Load scanner configuration from config.scanner.json.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_scanner_config(path: &Path) -> Result<ScannerFullConfig> {
    info!("[Config] Loading scanner config from: {:?}", path);
    if path.exists() {
        let content = nemesis_utils::fsutil::read_config_string(path).map_err(|e| {
            error!("[Config] Failed to read scanner config: {}", e);
            e
        })?;
        let cfg: ScannerFullConfig = serde_json::from_str(&content).map_err(|e| {
            error!("[Config] Failed to parse scanner config JSON: {}", e);
            e
        })?;
        info!("[Config] Scanner config loaded from file");
        return Ok(cfg);
    }

    info!("[Config] Scanner config file not found, trying embedded default");
    let defaults = get_embedded_defaults();
    if !defaults.scanner.is_empty()
        && let Ok(cfg) = serde_json::from_slice::<ScannerFullConfig>(&defaults.scanner)
    {
        info!("[Config] Scanner config loaded from embedded default");
        return Ok(cfg);
    }

    info!("[Config] Using hardcoded default scanner config");
    Ok(ScannerFullConfig::default())
}

/// Save scanner configuration to config.scanner.json.
pub fn save_scanner_config(path: &Path, cfg: &ScannerFullConfig) -> Result<()> {
    info!("[Config] Saving scanner config to: {:?}", path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            error!("[Config] Failed to create scanner config directory: {}", e);
            e
        })?;
    }
    let content = serde_json::to_string_pretty(cfg).map_err(|e| {
        error!("[Config] Failed to serialize scanner config: {}", e);
        e
    })?;
    // 统一原子写入。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600).map_err(
        |e| {
            error!("[Config] Failed to write scanner config: {}", e);
            ConfigError::Io(std::io::Error::other(e))
        },
    )?;
    info!("[Config] Scanner config saved successfully");
    Ok(())
}

/// Load skills configuration from config.skills.json.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_skills_config(path: &Path) -> Result<SkillsFullConfig> {
    info!("[Config] Loading skills config from: {:?}", path);
    if path.exists() {
        let content = nemesis_utils::fsutil::read_config_string(path).map_err(|e| {
            error!("[Config] Failed to read skills config: {}", e);
            e
        })?;
        let cfg: SkillsFullConfig = serde_json::from_str(&content).map_err(|e| {
            error!("[Config] Failed to parse skills config JSON: {}", e);
            e
        })?;
        info!("[Config] Skills config loaded from file");
        return Ok(cfg);
    }

    info!("[Config] Skills config file not found, trying embedded default");
    let defaults = get_embedded_defaults();
    if !defaults.skills.is_empty()
        && let Ok(cfg) = serde_json::from_slice::<SkillsFullConfig>(&defaults.skills)
    {
        info!("[Config] Skills config loaded from embedded default");
        return Ok(cfg);
    }

    info!("[Config] Using hardcoded default skills config");
    Ok(SkillsFullConfig::default())
}

/// Save skills configuration to config.skills.json.
pub fn save_skills_config(path: &Path, cfg: &SkillsFullConfig) -> Result<()> {
    info!("[Config] Saving skills config to: {:?}", path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            error!("[Config] Failed to create skills config directory: {}", e);
            e
        })?;
    }
    let content = serde_json::to_string_pretty(cfg).map_err(|e| {
        error!("[Config] Failed to serialize skills config: {}", e);
        e
    })?;
    // 统一原子写入。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600).map_err(
        |e| {
            error!("[Config] Failed to write skills config: {}", e);
            ConfigError::Io(std::io::Error::other(e))
        },
    )?;
    info!("[Config] Skills config saved successfully");
    Ok(())
}

// ============================================================================
// Additional default value functions
// ============================================================================
/// Expand ~ in a path to the user's home directory.
/// Mirrors Go's `ExpandHome` function.
/// `~\` 反斜杠形态（Windows 手写惯例）与 `~/` 同判——
/// 此前只认 `~/`，与 nemesisbot/src/commands/log.rs 的本地 expand_tilde
/// （认 `~\`）分叉，同义两形态异判。`~\` 前两字节是 ASCII，`&path[2..]`
/// 切片安全（`~x` 无斜杠仍按字面量，多字节 panic 面与 paths::expand_home
/// 同源已封）。
pub fn expand_tilde(path: &str) -> PathBuf {
    if (path.starts_with("~/") || path.starts_with("~\\") || path == "~")
        && let Some(home) = dirs::home_dir()
    {
        if path == "~" {
            return home;
        }
        return home.join(&path[2..]);
    }
    PathBuf::from(path)
}

/// Workspace path resolver.
pub struct WorkspaceResolver;

impl WorkspaceResolver {
    /// Resolve workspace directory path.
    /// Priority: local_flag > env_var > auto_detect > default
    pub fn resolve(local: bool) -> PathBuf {
        if local {
            return PathBuf::from("./.nemesisbot");
        }

        if let Ok(home) = std::env::var("NEMESISBOT_HOME") {
            // Expand ~ to user home, then append.nemesisbot (matching Go behavior)
            let expanded = expand_tilde(&home);
            return expanded.join(".nemesisbot");
        }

        // Auto-detect: if.nemesisbot exists in current directory
        let local_path = PathBuf::from("./.nemesisbot");
        if local_path.exists() {
            return local_path;
        }

        // Default: ~/.nemesisbot
        dirs::home_dir()
            .map(|h| h.join(".nemesisbot"))
            .unwrap_or(local_path)
    }

    /// Get the config file path within the workspace.
    pub fn config_path(workspace: &Path) -> PathBuf {
        workspace.join("config.json")
    }

    /// Ensure workspace directory exists.
    pub fn ensure_workspace(workspace: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(workspace)?;
        Ok(())
    }
}

/// Configuration loader.
pub struct ConfigLoader;

impl ConfigLoader {
    /// Load config from a file path.
    pub fn load_from_file(path: &Path) -> Result<Config> {
        let content = nemesis_utils::fsutil::read_config_string(path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Load config from workspace directory.
    /// Three-tier fallback: file -> embedded default -> hardcoded default.
    /// Mirrors Go LoadConfig behavior.
    pub fn load_from_workspace(workspace: &Path) -> Result<Config> {
        let config_path = WorkspaceResolver::config_path(workspace);
        load_config(&config_path)
    }

    /// Save config to a file.
    pub fn save_to_file(config: &Config, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(config)?;
        // 统一原子写入（与 save_config 同 0600）。
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
            .map_err(|e| ConfigError::Io(std::io::Error::other(e)))?;
        Ok(())
    }

    /// Save config to workspace.
    pub fn save_to_workspace(config: &Config, workspace: &Path) -> Result<()> {
        let config_path = WorkspaceResolver::config_path(workspace);
        Self::save_to_file(config, &config_path)
    }

    /// Load embedded default config (compile-time embedded).
    pub fn load_embedded_default() -> Result<Config> {
        let default_json = include_str!("../../../../nemesisbot/config/config.default.json");
        let config: Config =
            serde_json::from_str(default_json).unwrap_or_else(|_| Config::default());
        Ok(config)
    }
}

/// Get platform-specific security config filename.
/// On Linux: config.security.linux.json
/// On Windows: config.security.windows.json
/// On macOS: config.security.darwin.json
pub fn get_platform_security_config_filename() -> String {
    if cfg!(target_os = "linux") {
        "config.security.linux.json".to_string()
    } else if cfg!(target_os = "windows") {
        "config.security.windows.json".to_string()
    } else if cfg!(target_os = "macos") {
        "config.security.darwin.json".to_string()
    } else {
        "config.security.json".to_string()
    }
}

/// Get platform display name.
pub fn get_platform_display_name() -> String {
    if cfg!(target_os = "linux") {
        "Linux".to_string()
    } else if cfg!(target_os = "windows") {
        "Windows".to_string()
    } else if cfg!(target_os = "macos") {
        "macOS".to_string()
    } else {
        "Unknown".to_string()
    }
}

/// Get platform info as a JSON value.
pub fn get_platform_info() -> serde_json::Value {
    serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
        "display_name": get_platform_display_name(),
        "security_config": get_platform_security_config_filename(),
    })
}
