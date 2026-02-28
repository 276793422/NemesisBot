// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package config

import (
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sync"

	"github.com/caarlos0/env/v11"
)

// FlexibleStringSlice is a []string that also accepts JSON numbers,
// so allow_from can contain both "123" and 123.
type FlexibleStringSlice []string

func (f *FlexibleStringSlice) UnmarshalJSON(data []byte) error {
	// Try []string first
	var ss []string
	if err := json.Unmarshal(data, &ss); err == nil {
		*f = ss
		return nil
	}

	// Try []interface{} to handle mixed types
	var raw []interface{}
	if err := json.Unmarshal(data, &raw); err != nil {
		return err
	}

	result := make([]string, 0, len(raw))
	for _, v := range raw {
		switch val := v.(type) {
		case string:
			result = append(result, val)
		case float64:
			result = append(result, fmt.Sprintf("%.0f", val))
		default:
			result = append(result, fmt.Sprintf("%v", val))
		}
	}
	*f = result
	return nil
}

// Embedded default configurations (set via SetEmbeddedDefaults)
var embeddedDefaults struct {
	config   []byte
	mcp      []byte
	security []byte
	mu       sync.RWMutex
}

// GetEmbeddedDefaults returns the embedded default configurations.
// This allows other packages to access the embedded config files.
func GetEmbeddedDefaults() EmbeddedDefaults {
	embeddedDefaults.mu.RLock()
	defer embeddedDefaults.mu.RUnlock()

	return EmbeddedDefaults{
		Config:   embeddedDefaults.config,
		MCP:      embeddedDefaults.mcp,
		Security: embeddedDefaults.security,
	}
}

// EmbeddedDefaults holds the embedded default configuration data.
type EmbeddedDefaults struct {
	Config   []byte
	MCP      []byte
	Security []byte
}

// SetEmbeddedDefaults sets the embedded default configuration files.

// SetEmbeddedDefaults sets the embedded default configuration files.
// This should be called from main() before any config loading happens.
func SetEmbeddedDefaults(configFS fs.FS) error {
	embeddedDefaults.mu.Lock()
	defer embeddedDefaults.mu.Unlock()

	// Read config/config.default.json
	data, err := fs.ReadFile(configFS, "config/config.default.json")
	if err != nil {
		return fmt.Errorf("failed to read config/config.default.json: %w", err)
	}
	embeddedDefaults.config = data

	// Read config/config.mcp.default.json
	data, err = fs.ReadFile(configFS, "config/config.mcp.default.json")
	if err != nil {
		return fmt.Errorf("failed to read config/config.mcp.default.json: %w", err)
	}
	embeddedDefaults.mcp = data

	// Read config/config.security.default.json
	data, err = fs.ReadFile(configFS, "config/config.security.default.json")
	if err != nil {
		return fmt.Errorf("failed to read config/config.security.default.json: %w", err)
	}
	embeddedDefaults.security = data

	return nil
}

// LoadEmbeddedConfig loads the embedded default configuration.
// This reads the config/config.default.json that was embedded at compile time.
// Returns the same result as LoadConfig() when the config file doesn't exist.
//
// This is the preferred way to get default configuration, as it uses the
// single source of truth (config/config.default.json) rather than hardcoded values.
func LoadEmbeddedConfig() (*Config, error) {
	embeddedDefaults.mu.RLock()
	defer embeddedDefaults.mu.RUnlock()

	// Read embedded default config
	defaultData := embeddedDefaults.config

	if len(defaultData) == 0 {
		return nil, fmt.Errorf("embedded default config not available")
	}

	cfg := &Config{}
	if err := json.Unmarshal(defaultData, cfg); err != nil {
		return nil, fmt.Errorf("failed to parse embedded default config: %w", err)
	}

	// Post-process: populate deprecated fields from new fields for backward compatibility
	cfg.postProcessForCompatibility()

	return cfg, nil
}

type Config struct {
	Agents    AgentsConfig  `json:"agents"`
	Bindings  []AgentBinding `json:"bindings,omitempty"`
	Session   SessionConfig  `json:"session,omitempty"`
	Channels  ChannelsConfig `json:"channels"`
	ModelList []ModelConfig  `json:"model_list"` // Model-centric provider configuration
	Gateway   GatewayConfig  `json:"gateway"`
	Tools     ToolsConfig    `json:"tools"`
	Heartbeat HeartbeatConfig `json:"heartbeat"`
	Devices   DevicesConfig  `json:"devices"`
	Logging   *LoggingConfig      `json:"logging,omitempty"`
	Security  *SecurityFlagConfig `json:"security,omitempty"`
	mu        sync.RWMutex
}

type AgentsConfig struct {
	Defaults AgentDefaults `json:"defaults"`
	List     []AgentConfig `json:"list,omitempty"`
}

// AgentModelConfig supports both string and structured model config.
// String format: "gpt-4" (just primary, no fallbacks)
// Object format: {"primary": "gpt-4", "fallbacks": ["claude-haiku"]}
type AgentModelConfig struct {
	Primary   string   `json:"primary,omitempty"`
	Fallbacks []string `json:"fallbacks,omitempty"`
}

func (m *AgentModelConfig) UnmarshalJSON(data []byte) error {
	var s string
	if err := json.Unmarshal(data, &s); err == nil {
		m.Primary = s
		m.Fallbacks = nil
		return nil
	}
	type raw struct {
		Primary   string   `json:"primary"`
		Fallbacks []string `json:"fallbacks"`
	}
	var r raw
	if err := json.Unmarshal(data, &r); err != nil {
		return err
	}
	m.Primary = r.Primary
	m.Fallbacks = r.Fallbacks
	return nil
}

func (m AgentModelConfig) MarshalJSON() ([]byte, error) {
	if len(m.Fallbacks) == 0 && m.Primary != "" {
		return json.Marshal(m.Primary)
	}
	type raw struct {
		Primary   string   `json:"primary,omitempty"`
		Fallbacks []string `json:"fallbacks,omitempty"`
	}
	return json.Marshal(raw{Primary: m.Primary, Fallbacks: m.Fallbacks})
}

type AgentConfig struct {
	ID        string            `json:"id"`
	Default   bool              `json:"default,omitempty"`
	Name      string            `json:"name,omitempty"`
	Workspace string            `json:"workspace,omitempty"`
	Model     *AgentModelConfig `json:"model,omitempty"`
	Skills    []string          `json:"skills,omitempty"`
	Subagents *SubagentsConfig  `json:"subagents,omitempty"`
}

type SubagentsConfig struct {
	AllowAgents []string          `json:"allow_agents,omitempty"`
	Model       *AgentModelConfig `json:"model,omitempty"`
}

type PeerMatch struct {
	Kind string `json:"kind"`
	ID   string `json:"id"`
}

type BindingMatch struct {
	Channel   string     `json:"channel"`
	AccountID string     `json:"account_id,omitempty"`
	Peer      *PeerMatch `json:"peer,omitempty"`
	GuildID   string     `json:"guild_id,omitempty"`
	TeamID    string     `json:"team_id,omitempty"`
}

type AgentBinding struct {
	AgentID string       `json:"agent_id"`
	Match   BindingMatch `json:"match"`
}

type SessionConfig struct {
	DMScope       string              `json:"dm_scope,omitempty"`
	IdentityLinks map[string][]string `json:"identity_links,omitempty"`
}

type AgentDefaults struct {
	Workspace             string   `json:"workspace" env:"NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE"`
	RestrictToWorkspace   bool     `json:"restrict_to_workspace" env:"NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE"`
	LLM                   string   `json:"llm,omitempty" env:"NEMESISBOT_AGENTS_DEFAULTS_LLM"` // Format: "provider/model" (e.g., "openai/gpt-4")
	ImageModel            string   `json:"image_model,omitempty" env:"NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL"`
	ImageModelFallbacks   []string `json:"image_model_fallbacks,omitempty"`
	MaxTokens             int      `json:"max_tokens" env:"NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS"`
	Temperature           float64  `json:"temperature" env:"NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE"`
	MaxToolIterations     int      `json:"max_tool_iterations" env:"NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS"`
	ConcurrentRequestMode string   `json:"concurrent_request_mode" env:"NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE"` // "reject" or "queue"
	QueueSize             int      `json:"queue_size" env:"NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE"`                         // Only effective in queue mode
}

type ChannelsConfig struct {
	WhatsApp WhatsAppConfig `json:"whatsapp"`
	Telegram TelegramConfig `json:"telegram"`
	Feishu   FeishuConfig   `json:"feishu"`
	Discord  DiscordConfig  `json:"discord"`
	MaixCam  MaixCamConfig  `json:"maixcam"`
	QQ       QQConfig       `json:"qq"`
	DingTalk DingTalkConfig `json:"dingtalk"`
	Slack    SlackConfig    `json:"slack"`
	LINE     LINEConfig     `json:"line"`
	OneBot   OneBotConfig   `json:"onebot"`
	Web      WebChannelConfig `json:"web"`
	WebSocket WebSocketChannelConfig `json:"websocket"`
	External ExternalConfig `json:"external"`
}

type WhatsAppConfig struct {
	Enabled   bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_WHATSAPP_ENABLED"`
	BridgeURL string              `json:"bridge_url" env:"NEMESISBOT_CHANNELS_WHATSAPP_BRIDGE_URL"`
	AllowFrom FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_WHATSAPP_ALLOW_FROM"`
	SyncTo    []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_WHATSAPP_SYNC_TO"`
}

type TelegramConfig struct {
	Enabled   bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_TELEGRAM_ENABLED"`
	Token     string              `json:"token" env:"NEMESISBOT_CHANNELS_TELEGRAM_TOKEN"`
	Proxy     string              `json:"proxy" env:"NEMESISBOT_CHANNELS_TELEGRAM_PROXY"`
	AllowFrom FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_TELEGRAM_ALLOW_FROM"`
	SyncTo    []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_TELEGRAM_SYNC_TO"`
}

type FeishuConfig struct {
	Enabled           bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_FEISHU_ENABLED"`
	AppID             string              `json:"app_id" env:"NEMESISBOT_CHANNELS_FEISHU_APP_ID"`
	AppSecret         string              `json:"app_secret" env:"NEMESISBOT_CHANNELS_FEISHU_APP_SECRET"`
	EncryptKey        string              `json:"encrypt_key" env:"NEMESISBOT_CHANNELS_FEISHU_ENCRYPT_KEY"`
	VerificationToken string              `json:"verification_token" env:"NEMESISBOT_CHANNELS_FEISHU_VERIFICATION_TOKEN"`
	AllowFrom         FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_FEISHU_ALLOW_FROM"`
	SyncTo            []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_FEISHU_SYNC_TO"`
}

type DiscordConfig struct {
	Enabled   bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_DISCORD_ENABLED"`
	Token     string              `json:"token" env:"NEMESISBOT_CHANNELS_DISCORD_TOKEN"`
	AllowFrom FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_DISCORD_ALLOW_FROM"`
	SyncTo    []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_DISCORD_SYNC_TO"`
}

type MaixCamConfig struct {
	Enabled   bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_MAIXCAM_ENABLED"`
	Host      string              `json:"host" env:"NEMESISBOT_CHANNELS_MAIXCAM_HOST"`
	Port      int                 `json:"port" env:"NEMESISBOT_CHANNELS_MAIXCAM_PORT"`
	AllowFrom FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_MAIXCAM_ALLOW_FROM"`
	SyncTo    []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_MAIXCAM_SYNC_TO"`
}

type QQConfig struct {
	Enabled     bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_QQ_ENABLED"`
	AppID       string              `json:"app_id" env:"NEMESISBOT_CHANNELS_QQ_APP_ID"`
	AppSecret   string              `json:"app_secret" env:"NEMESISBOT_CHANNELS_QQ_APP_SECRET"`
	AllowFrom   FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_QQ_ALLOW_FROM"`
	SyncTo      []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_QQ_SYNC_TO"`
}

type DingTalkConfig struct {
	Enabled      bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_DINGTALK_ENABLED"`
	ClientID     string              `json:"client_id" env:"NEMESISBOT_CHANNELS_DINGTALK_CLIENT_ID"`
	ClientSecret string              `json:"client_secret" env:"NEMESISBOT_CHANNELS_DINGTALK_CLIENT_SECRET"`
	AllowFrom    FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_DINGTALK_ALLOW_FROM"`
	SyncTo       []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_DINGTALK_SYNC_TO"`
}

type SlackConfig struct {
	Enabled   bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_SLACK_ENABLED"`
	BotToken  string              `json:"bot_token" env:"NEMESISBOT_CHANNELS_SLACK_BOT_TOKEN"`
	AppToken  string              `json:"app_token" env:"NEMESISBOT_CHANNELS_SLACK_APP_TOKEN"`
	AllowFrom FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_SLACK_ALLOW_FROM"`
	SyncTo    []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_SLACK_SYNC_TO"`
}

type LINEConfig struct {
	Enabled            bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_LINE_ENABLED"`
	ChannelSecret      string              `json:"channel_secret" env:"NEMESISBOT_CHANNELS_LINE_CHANNEL_SECRET"`
	ChannelAccessToken string              `json:"channel_access_token" env:"NEMESISBOT_CHANNELS_LINE_CHANNEL_ACCESS_TOKEN"`
	WebhookHost        string              `json:"webhook_host" env:"NEMESISBOT_CHANNELS_LINE_WEBHOOK_HOST"`
	WebhookPort        int                 `json:"webhook_port" env:"NEMESISBOT_CHANNELS_LINE_WEBHOOK_PORT"`
	WebhookPath        string              `json:"webhook_path" env:"NEMESISBOT_CHANNELS_LINE_WEBHOOK_PATH"`
	AllowFrom          FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_LINE_ALLOW_FROM"`
	SyncTo             []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_LINE_SYNC_TO"`
}

type OneBotConfig struct {
	Enabled            bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_ONEBOT_ENABLED"`
	WSUrl              string              `json:"ws_url" env:"NEMESISBOT_CHANNELS_ONEBOT_WS_URL"`
	AccessToken        string              `json:"access_token" env:"NEMESISBOT_CHANNELS_ONEBOT_ACCESS_TOKEN"`
	ReconnectInterval  int                 `json:"reconnect_interval" env:"NEMESISBOT_CHANNELS_ONEBOT_RECONNECT_INTERVAL"`
	GroupTriggerPrefix []string            `json:"group_trigger_prefix" env:"NEMESISBOT_CHANNELS_ONEBOT_GROUP_TRIGGER_PREFIX"`
	AllowFrom          FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_ONEBOT_ALLOW_FROM"`
	SyncTo             []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_ONEBOT_SYNC_TO"`
}

type WebChannelConfig struct {
	Enabled           bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_WEB_ENABLED"`
	Host              string              `json:"host" env:"NEMESISBOT_CHANNELS_WEB_HOST"`
	Port              int                 `json:"port" env:"NEMESISBOT_CHANNELS_WEB_PORT"`
	Path              string              `json:"path" env:"NEMESISBOT_CHANNELS_WEB_PATH"`
	AuthToken         string              `json:"auth_token" env:"NEMESISBOT_CHANNELS_WEB_AUTH_TOKEN"`
	AllowFrom         FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_WEB_ALLOW_FROM"`
	HeartbeatInterval int                 `json:"heartbeat_interval" env:"NEMESISBOT_CHANNELS_WEB_HEARTBEAT_INTERVAL"` // seconds
	SessionTimeout    int                 `json:"session_timeout" env:"NEMESISBOT_CHANNELS_WEB_SESSION_TIMEOUT"`      // seconds
	SyncTo            []string            `json:"sync_to,omitempty" env:"NEMESISBOT_CHANNELS_WEB_SYNC_TO"`
}

// ExternalConfig configures the external kit channel (input/output EXE pair)
type ExternalConfig struct {
	Enabled   bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_EXTERNAL_ENABLED"`
	InputEXE  string              `json:"input_exe" env:"NEMESISBOT_CHANNELS_EXTERNAL_INPUT_EXE"`
	OutputEXE string              `json:"output_exe" env:"NEMESISBOT_CHANNELS_EXTERNAL_OUTPUT_EXE"`
	ChatID    string              `json:"chat_id" env:"NEMESISBOT_CHANNELS_EXTERNAL_CHAT_ID"`
	AllowFrom FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_EXTERNAL_ALLOW_FROM"`
	// SyncTo specifies channels to sync messages to (e.g., ["web"])
	SyncTo []string `json:"sync_to" env:"NEMESISBOT_CHANNELS_EXTERNAL_SYNC_TO"`
	// Deprecated: Use SyncTo instead. These fields are auto-populated from SyncTo.
	SyncToWeb    bool   `json:"sync_to_web,omitempty" env:"NEMESISBOT_CHANNELS_EXTERNAL_SYNC_TO_WEB"`
	WebSessionID string `json:"web_session_id,omitempty" env:"NEMESISBOT_CHANNELS_EXTERNAL_WEB_SESSION_ID"`
}

// WebSocketChannelConfig configures the standalone WebSocket channel for external program integration
type WebSocketChannelConfig struct {
	Enabled    bool                `json:"enabled" env:"NEMESISBOT_CHANNELS_WEBSOCKET_ENABLED"`
	Host       string              `json:"host" env:"NEMESISBOT_CHANNELS_WEBSOCKET_HOST"`
	Port       int                 `json:"port" env:"NEMESISBOT_CHANNELS_WEBSOCKET_PORT"`
	Path       string              `json:"path" env:"NEMESISBOT_CHANNELS_WEBSOCKET_PATH"`
	AuthToken  string              `json:"auth_token" env:"NEMESISBOT_CHANNELS_WEBSOCKET_AUTH_TOKEN"`
	AllowFrom  FlexibleStringSlice `json:"allow_from" env:"NEMESISBOT_CHANNELS_WEBSOCKET_ALLOW_FROM"`
	// SyncTo specifies channels to sync messages to (e.g., ["web"])
	SyncTo []string `json:"sync_to" env:"NEMESISBOT_CHANNELS_WEBSOCKET_SYNC_TO"`
	// Deprecated: Use SyncTo instead. These fields are auto-populated from SyncTo.
	SyncToWeb    bool   `json:"sync_to_web,omitempty" env:"NEMESISBOT_CHANNELS_WEBSOCKET_SYNC_TO_WEB"`
	WebSessionID string `json:"web_session_id,omitempty" env:"NEMESISBOT_CHANNELS_WEBSOCKET_WEB_SESSION_ID"`
}

type HeartbeatConfig struct {
	Enabled  bool `json:"enabled" env:"NEMESISBOT_HEARTBEAT_ENABLED"`
	Interval int  `json:"interval" env:"NEMESISBOT_HEARTBEAT_INTERVAL"` // minutes, min 5
}

type DevicesConfig struct {
	Enabled    bool `json:"enabled" env:"NEMESISBOT_DEVICES_ENABLED"`
	MonitorUSB bool `json:"monitor_usb" env:"NEMESISBOT_DEVICES_MONITOR_USB"`
}

// ModelConfig represents a model-centric provider configuration.
// It allows adding new providers (especially OpenAI-compatible ones) via configuration only.
// The model field uses protocol prefix format: [protocol/]model-identifier
// Supported protocols: openai, anthropic, codex-cli, claude-cli, github-copilot
// Default protocol is "openai" if no prefix is specified.
type ModelConfig struct {
	// Required fields
	ModelName string `json:"model_name"` // User-facing alias for the model
	Model     string `json:"model"`      // Protocol/model-identifier (e.g., "openai/gpt-4o", "anthropic/claude-sonnet-4.6")

	// HTTP-based providers
	APIBase string `json:"api_base,omitempty"` // API endpoint URL
	APIKey  string `json:"api_key"`            // API authentication key
	Proxy   string `json:"proxy,omitempty"`    // HTTP proxy URL

	// Special providers (CLI-based, OAuth, etc.)
	AuthMethod  string `json:"auth_method,omitempty"`  // Authentication method: oauth, token
	ConnectMode string `json:"connect_mode,omitempty"` // Connection mode: stdio, grpc
	Workspace   string `json:"workspace,omitempty"`    // Workspace path for CLI-based providers
}

// Validate checks if the ModelConfig has all required fields.
func (c *ModelConfig) Validate() error {
	if c.ModelName == "" {
		return fmt.Errorf("model_name is required")
	}
	if c.Model == "" {
		return fmt.Errorf("model is required")
	}
	return nil
}

type GatewayConfig struct {
	Host string `json:"host" env:"NEMESISBOT_GATEWAY_HOST"`
	Port int    `json:"port" env:"NEMESISBOT_GATEWAY_PORT"`
}

type BraveConfig struct {
	Enabled    bool   `json:"enabled" env:"NEMESISBOT_TOOLS_WEB_BRAVE_ENABLED"`
	APIKey     string `json:"api_key" env:"NEMESISBOT_TOOLS_WEB_BRAVE_API_KEY"`
	MaxResults int    `json:"max_results" env:"NEMESISBOT_TOOLS_WEB_BRAVE_MAX_RESULTS"`
}

type DuckDuckGoConfig struct {
	Enabled    bool `json:"enabled" env:"NEMESISBOT_TOOLS_WEB_DUCKDUCKGO_ENABLED"`
	MaxResults int  `json:"max_results" env:"NEMESISBOT_TOOLS_WEB_DUCKDUCKGO_MAX_RESULTS"`
}

type PerplexityConfig struct {
	Enabled    bool   `json:"enabled" env:"NEMESISBOT_TOOLS_WEB_PERPLEXITY_ENABLED"`
	APIKey     string `json:"api_key" env:"NEMESISBOT_TOOLS_WEB_PERPLEXITY_API_KEY"`
	MaxResults int    `json:"max_results" env:"NEMESISBOT_TOOLS_WEB_PERPLEXITY_MAX_RESULTS"`
}

type WebToolsConfig struct {
	Brave      BraveConfig      `json:"brave"`
	DuckDuckGo DuckDuckGoConfig `json:"duckduckgo"`
	Perplexity PerplexityConfig `json:"perplexity"`
}

type CronToolsConfig struct {
	ExecTimeoutMinutes int `json:"exec_timeout_minutes" env:"NEMESISBOT_TOOLS_CRON_EXEC_TIMEOUT_MINUTES"` // 0 means no timeout
}

type ExecConfig struct {
	EnableDenyPatterns bool     `json:"enable_deny_patterns" env:"NEMESISBOT_TOOLS_EXEC_ENABLE_DENY_PATTERNS"`
	CustomDenyPatterns []string `json:"custom_deny_patterns" env:"NEMESISBOT_TOOLS_EXEC_CUSTOM_DENY_PATTERNS"`
}

type ToolsConfig struct {
	Web  WebToolsConfig  `json:"web"`
	Cron CronToolsConfig `json:"cron"`
	Exec ExecConfig      `json:"exec"`
}

// MCPConfig holds Model Context Protocol (MCP) server configurations
type MCPConfig struct {
	Enabled bool             `json:"enabled" env:"NEMESISBOT_MCP_ENABLED"`
	Servers []MCPServerConfig `json:"servers"`
	Timeout int              `json:"timeout" env:"NEMESISBOT_MCP_TIMEOUT"` // seconds, default 30
}

// MCPServerConfig holds configuration for a single MCP server
type MCPServerConfig struct {
	Name    string   `json:"name"`
	Command string   `json:"command"`
	Args    []string `json:"args"`
	Env     []string `json:"env,omitempty"`
	Timeout int      `json:"timeout,omitempty"` // overrides global timeout, 0 means use global
}

// LoggingConfig holds logging configuration
type LoggingConfig struct {
	LLMRequests bool   `json:"llm_requests" env:"NEMESISBOT_LOGGING_LLM_REQUESTS"`
	LogDir      string `json:"log_dir" env:"NEMESISBOT_LOGGING_LOG_DIR"`
	DetailLevel string `json:"detail_level" env:"NEMESISBOT_LOGGING_DETAIL_LEVEL"` // "full" or "truncated"
}

// SecurityFlagConfig is a simple flag in main config to enable/disable security
type SecurityFlagConfig struct {
	Enabled bool `json:"enabled" env:"NEMESISBOT_SECURITY_ENABLED"`
}

// DefaultConfig creates a new Config struct with sensible default values.
// This provides a working out-of-the-box configuration that can be customized.
//
// Returns:
//   A Config instance with default settings for all components
//
// Default values include:
//   - Workspace: ~/.nemesisbot/workspace
//   - Model: glm-4.7-flash (智谱)
//   - MaxTokens: 8192
//   - Temperature: 0.7
//   - MaxToolIterations: 20
//   - Web channel enabled on port 8080
//   - All other channels disabled by default
//   - Security disabled by default
//
// The returned config can be customized and saved using SaveConfig()
func DefaultConfig() *Config {
	return &Config{
		Agents: AgentsConfig{
			Defaults: AgentDefaults{
				Workspace:             "~/.nemesisbot/workspace",
				RestrictToWorkspace:   true,
				LLM:                   "zhipu/glm-4.7-flash", // New format: "provider/model"
				MaxTokens:             8192,
				Temperature:           0.7,
				MaxToolIterations:     20,
				ConcurrentRequestMode: "reject",
				QueueSize:             8,
			},
		},
		Channels: ChannelsConfig{
			WhatsApp: WhatsAppConfig{
				Enabled:   false,
				BridgeURL: "ws://localhost:3001",
				AllowFrom: FlexibleStringSlice{},
			},
			Telegram: TelegramConfig{
				Enabled:   false,
				Token:     "",
				AllowFrom: FlexibleStringSlice{},
			},
			Feishu: FeishuConfig{
				Enabled:           false,
				AppID:             "",
				AppSecret:         "",
				EncryptKey:        "",
				VerificationToken: "",
				AllowFrom:         FlexibleStringSlice{},
			},
			Discord: DiscordConfig{
				Enabled:   false,
				Token:     "",
				AllowFrom: FlexibleStringSlice{},
			},
			MaixCam: MaixCamConfig{
				Enabled:   false,
				Host:      "0.0.0.0",
				Port:      18790,
				AllowFrom: FlexibleStringSlice{},
			},
			QQ: QQConfig{
				Enabled:   false,
				AppID:     "",
				AppSecret: "",
				AllowFrom: FlexibleStringSlice{},
			},
			DingTalk: DingTalkConfig{
				Enabled:      false,
				ClientID:     "",
				ClientSecret: "",
				AllowFrom:    FlexibleStringSlice{},
			},
			Slack: SlackConfig{
				Enabled:   false,
				BotToken:  "",
				AppToken:  "",
				AllowFrom: FlexibleStringSlice{},
			},
			LINE: LINEConfig{
				Enabled:            false,
				ChannelSecret:      "",
				ChannelAccessToken: "",
				WebhookHost:        "0.0.0.0",
				WebhookPort:        18791,
				WebhookPath:        "/webhook/line",
				AllowFrom:          FlexibleStringSlice{},
			},
			OneBot: OneBotConfig{
				Enabled:            false,
				WSUrl:              "ws://127.0.0.1:3001",
				AccessToken:        "",
				ReconnectInterval:  5,
				GroupTriggerPrefix: []string{},
				AllowFrom:          FlexibleStringSlice{},
			},
			Web: WebChannelConfig{
				Enabled:           true, // 默认启用
				Host:              "0.0.0.0",
				Port:              8080,
				Path:              "/ws",
				AuthToken:         "",
				AllowFrom:         FlexibleStringSlice{},
				HeartbeatInterval: 30,
				SessionTimeout:    3600,
			},
			External: ExternalConfig{
				Enabled:     false,
				InputEXE:    "",
				OutputEXE:   "",
				ChatID:      "external:main",
				AllowFrom:   FlexibleStringSlice{},
				SyncTo:      []string{"web"},
			},
		},
		ModelList: []ModelConfig{}, // Empty model list by default
		Gateway: GatewayConfig{
			Host: "0.0.0.0",
			Port: 18790,
		},
		Tools: ToolsConfig{
			Web: WebToolsConfig{
				Brave: BraveConfig{
					Enabled:    false,
					APIKey:     "",
					MaxResults: 5,
				},
				DuckDuckGo: DuckDuckGoConfig{
					Enabled:    true,
					MaxResults: 5,
				},
				Perplexity: PerplexityConfig{
					Enabled:    false,
					APIKey:     "",
					MaxResults: 5,
				},
			},
			Cron: CronToolsConfig{
				ExecTimeoutMinutes: 5, // default 5 minutes for LLM operations
			},
			Exec: ExecConfig{
				EnableDenyPatterns: true,
			},
		},
		Heartbeat: HeartbeatConfig{
			Enabled:  true,
			Interval: 30, // default 30 minutes
		},
		Devices: DevicesConfig{
			Enabled:    false,
			MonitorUSB: true,
		},
		Logging: &LoggingConfig{
			LLMRequests: false,
			LogDir:      "~/.nemesisbot/workspace/logs/request_logs",
			DetailLevel: "full",
		},
		Security: &SecurityFlagConfig{
			Enabled: false, // Default disabled for backward compatibility
		},
	}
}

// LoadConfig loads the NemesisBot configuration from a JSON file.
// If the file doesn't exist, returns default configuration.
// Environment variables override file values.
//
// Parameters:
//   - path: Path to the configuration file (e.g., ~/.nemesisbot/config.json)
//
// Returns:
//   - cfg: The loaded configuration with defaults applied
//   - error: Any error reading/parsing the file (excluding file not found)
//
// Behavior:
//   - If file doesn't exist, returns DefaultConfig()
//   - JSON values are merged with defaults
//   - Environment variables override JSON values
//   - Supports env tags for flexible configuration
func LoadConfig(path string) (*Config, error) {
	var cfg *Config

	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			// Try to use embedded default config
			embeddedDefaults.mu.RLock()
			defaultData := embeddedDefaults.config
			embeddedDefaults.mu.RUnlock()

			if len(defaultData) > 0 {
				cfg = &Config{}
				if err := json.Unmarshal(defaultData, cfg); err != nil {
					// Fallback to hardcoded default if embedded fails
					cfg = DefaultConfig()
				}
			} else {
				cfg = DefaultConfig()
			}
			// Post-process for compatibility
			cfg.postProcessForCompatibility()
			return cfg, nil
		}
		return nil, err
	}

	cfg = &Config{}
	if err := json.Unmarshal(data, cfg); err != nil {
		return nil, err
	}

	if err := env.Parse(cfg); err != nil {
		return nil, err
	}

	// Post-process for compatibility
	cfg.postProcessForCompatibility()

	return cfg, nil
}

// SaveConfig saves the NemesisBot configuration to a JSON file.
// The file is written with secure permissions (0600) to protect sensitive data.
//
// Parameters:
//   - path: Path where the configuration should be saved
//   - cfg: The configuration to save
//
// Returns:
//   - error: Any error during file writing or directory creation
//
// Behavior:
//   - Creates parent directories if they don't exist
//   - Writes file with permissions 0600 (owner read/write only)
//   - Formats JSON with 2-space indentation for readability
//   - Thread-safe: uses internal read lock
func SaveConfig(path string, cfg *Config) error {
	cfg.mu.RLock()
	defer cfg.mu.RUnlock()

	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return err
	}

	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	return os.WriteFile(path, data, 0600)
}

func (c *Config) WorkspacePath() string {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return expandHome(c.Agents.Defaults.Workspace)
}

// GetModelByModelName finds a model configuration by model name or vendor/model prefix.
func (c *Config) GetModelByModelName(modelRef string) (*ModelConfig, error) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	// First try exact match with model_name
	for i := range c.ModelList {
		if c.ModelList[i].ModelName == modelRef {
			return &c.ModelList[i], nil
		}
	}

	// Then try prefix match with model field (vendor/model)
	for i := range c.ModelList {
		if c.ModelList[i].Model == modelRef {
			return &c.ModelList[i], nil
		}
	}

	return nil, fmt.Errorf("model %q not found in model_list", modelRef)
}

// GetModelConfig returns the model configuration for a given model name.
// Searches model_list and returns the matching ModelConfig.
func (c *Config) GetModelConfig(modelName string) (*ModelConfig, error) {
	c.mu.RLock()
	defer c.mu.RUnlock()

	// Search in ModelList
	for i := range c.ModelList {
		if c.ModelList[i].ModelName == modelName {
			return &c.ModelList[i], nil
		}
	}

	return nil, fmt.Errorf("model %q not found in model_list", modelName)
}

func expandHome(path string) string {
	if path == "" {
		return path
	}
	if path[0] == '~' {
		home, _ := os.UserHomeDir()
		if len(path) > 1 && path[1] == '/' {
			return home + path[1:]
		}
		return home
	}
	return path
}

// LoadMCPConfig loads MCP configuration from a separate config.mcp.json file.
// If the file doesn't exist, it returns a default disabled configuration.
func LoadMCPConfig(path string) (*MCPConfig, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			// Try to use embedded default config
			embeddedDefaults.mu.RLock()
			defaultData := embeddedDefaults.mcp
			embeddedDefaults.mu.RUnlock()

			if len(defaultData) > 0 {
				var cfg MCPConfig
				if json.Unmarshal(defaultData, &cfg) == nil {
					return &cfg, nil
				}
			}
			// Fallback to hardcoded default
			return &MCPConfig{
				Enabled: false,
				Servers: []MCPServerConfig{},
				Timeout: 30,
			}, nil
		}
		return nil, err
	}

	var cfg MCPConfig
	if err := json.Unmarshal(data, &cfg); err != nil {
		return nil, err
	}

	return &cfg, nil
}

// SaveMCPConfig saves MCP configuration to a separate config.mcp.json file.
func SaveMCPConfig(path string, cfg *MCPConfig) error {
	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return err
	}

	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	return os.WriteFile(path, data, 0600)
}

// SecurityConfig holds detailed security configuration
type SecurityConfig struct {
	DefaultAction         string  `json:"default_action"` // "allow", "deny", "ask"
	LogAllOperations      bool    `json:"log_all_operations"`
	LogDenialsOnly        bool    `json:"log_denials_only"`
	ApprovalTimeout       int     `json:"approval_timeout_seconds"`
	MaxPendingRequests    int     `json:"max_pending_requests"`
	AuditLogRetentionDays int     `json:"audit_log_retention_days"`
	AuditLogPath          string  `json:"audit_log_path,omitempty"`
	AuditLogFileEnabled   bool    `json:"audit_log_file_enabled"`
	SynchronousMode       bool    `json:"synchronous_mode"`
	FileRules             *FileSecurityRules        `json:"file_rules,omitempty"`
	DirectoryRules        *DirectorySecurityRules   `json:"directory_rules,omitempty"`
	ProcessRules          *ProcessSecurityRules     `json:"process_rules,omitempty"`
	NetworkRules          *NetworkSecurityRules     `json:"network_rules,omitempty"`
	HardwareRules         *HardwareSecurityRules    `json:"hardware_rules,omitempty"`
	RegistryRules         *RegistrySecurityRules    `json:"registry_rules,omitempty"`
}

// SecurityRule defines a single security rule with pattern and action
type SecurityRule struct {
	Pattern string `json:"pattern"` // Pattern supporting * and ** wildcards
	Action  string `json:"action"`  // "allow", "deny", "ask"
}

// FileSecurityRules defines file operation rules
type FileSecurityRules struct {
	Read   []SecurityRule `json:"read,omitempty"`
	Write  []SecurityRule `json:"write,omitempty"`
	Delete []SecurityRule `json:"delete,omitempty"`
}

// DirectorySecurityRules defines directory operation rules
type DirectorySecurityRules struct {
	Read   []SecurityRule `json:"read,omitempty"`
	Create []SecurityRule `json:"create,omitempty"`
	Delete []SecurityRule `json:"delete,omitempty"`
}

// ProcessSecurityRules defines process execution rules
type ProcessSecurityRules struct {
	Exec    []SecurityRule `json:"exec,omitempty"`
	Spawn   []SecurityRule `json:"spawn,omitempty"`
	Kill    []SecurityRule `json:"kill,omitempty"`
	Suspend []SecurityRule `json:"suspend,omitempty"`
}

// NetworkSecurityRules defines network operation rules
type NetworkSecurityRules struct {
	Request  []SecurityRule `json:"request,omitempty"`
	Download []SecurityRule `json:"download,omitempty"`
	Upload   []SecurityRule `json:"upload,omitempty"`
}

// HardwareSecurityRules defines hardware operation rules
type HardwareSecurityRules struct {
	I2C  []SecurityRule `json:"i2c,omitempty"`
	SPI  []SecurityRule `json:"spi,omitempty"`
	GPIO []SecurityRule `json:"gpio,omitempty"`
}

// RegistrySecurityRules defines registry operation rules (Windows)
type RegistrySecurityRules struct {
	Read  []SecurityRule `json:"read,omitempty"`
	Write []SecurityRule `json:"write,omitempty"`
	Delete []SecurityRule `json:"delete,omitempty"`
}

// LoadSecurityConfig loads security configuration from a separate config.security.json file.
// If the file doesn't exist, it returns a default disabled configuration.
func LoadSecurityConfig(path string) (*SecurityConfig, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			// Try to use embedded default config
			embeddedDefaults.mu.RLock()
			defaultData := embeddedDefaults.security
			embeddedDefaults.mu.RUnlock()

			if len(defaultData) > 0 {
				var cfg SecurityConfig
				if json.Unmarshal(defaultData, &cfg) == nil {
					return &cfg, nil
				}
			}
			// Fallback to hardcoded default
			return &SecurityConfig{
				DefaultAction:         "deny",
				LogAllOperations:      true,
				LogDenialsOnly:        false,
				ApprovalTimeout:       300,
				MaxPendingRequests:    100,
				AuditLogRetentionDays: 90,
				AuditLogFileEnabled:   true,
				SynchronousMode:       false,
				FileRules: &FileSecurityRules{
					Read: []SecurityRule{
						{Pattern: "/workspace/", Action: "allow"},
						{Pattern: "*.log", Action: "ask"},
					},
					Write: []SecurityRule{
						{Pattern: "/workspace/**", Action: "allow"},
						{Pattern: "*.key", Action: "deny"},
						{Pattern: "/etc/**", Action: "deny"},
					},
					Delete: []SecurityRule{
						{Pattern: "/workspace/tmp/**", Action: "allow"},
					},
				},
				DirectoryRules: &DirectorySecurityRules{
					Read: []SecurityRule{
						{Pattern: "/workspace/**", Action: "allow"},
					},
					Create: []SecurityRule{
						{Pattern: "/workspace/**", Action: "allow"},
					},
					Delete: []SecurityRule{
						{Pattern: "/workspace/tmp/**", Action: "allow"},
					},
				},
				ProcessRules: &ProcessSecurityRules{
					Exec: []SecurityRule{
						{Pattern: "git *", Action: "allow"},
						{Pattern: "npm *", Action: "allow"},
						{Pattern: "go run *", Action: "allow"},
						{Pattern: "rm -rf *", Action: "deny"},
						{Pattern: "*sudo*", Action: "deny"},
						{Pattern: "format *", Action: "deny"},
					},
				},
				NetworkRules: &NetworkSecurityRules{
					Request: []SecurityRule{
						{Pattern: "*.github.com", Action: "allow"},
						{Pattern: "*.openai.com", Action: "allow"},
						{Pattern: "*.anthropic.com", Action: "allow"},
					},
					Download: []SecurityRule{
						{Pattern: "*", Action: "ask"},
					},
				},
				HardwareRules: &HardwareSecurityRules{
					I2C:  []SecurityRule{{Pattern: "*", Action: "allow"}},
					SPI:  []SecurityRule{{Pattern: "*", Action: "allow"}},
					GPIO: []SecurityRule{{Pattern: "*", Action: "allow"}},
				},
				RegistryRules: &RegistrySecurityRules{
					Read: []SecurityRule{
						{Pattern: "HKEY_CURRENT_USER/**", Action: "allow"},
						{Pattern: "HKEY_LOCAL_MACHINE/**", Action: "ask"},
					},
					Write: []SecurityRule{
						{Pattern: "HKEY_CURRENT_USER/**", Action: "ask"},
						{Pattern: "HKEY_LOCAL_MACHINE/**", Action: "deny"},
					},
				},
			}, nil
		}
		return nil, err
	}

	var cfg SecurityConfig
	if err := json.Unmarshal(data, &cfg); err != nil {
		return nil, err
	}

	return &cfg, nil
}

// SaveSecurityConfig saves security configuration to a separate config.security.json file.
func SaveSecurityConfig(path string, cfg *SecurityConfig) error {
	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return err
	}

	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	return os.WriteFile(path, data, 0600)
}

// postProcessForCompatibility populates deprecated fields from new fields for backward compatibility
func (c *Config) postProcessForCompatibility() {
	// External channel: populate SyncToWeb from SyncTo
	if len(c.Channels.External.SyncTo) > 0 {
		c.Channels.External.SyncToWeb = true
	} else {
		c.Channels.External.SyncToWeb = false
	}

	// WebSocket channel: populate SyncToWeb from SyncTo
	if len(c.Channels.WebSocket.SyncTo) > 0 {
		c.Channels.WebSocket.SyncToWeb = true
	} else {
		c.Channels.WebSocket.SyncToWeb = false
	}
}
