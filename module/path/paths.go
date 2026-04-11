// Package path provides unified path management for NemesisBot.
//
// This package centralizes all path resolution logic, eliminating hardcoded
// .nemesisbot paths throughout the codebase. It supports environment variable
// overrides and provides a thread-safe PathManager with caching.
//
// Environment Variables:
//
//	NEMESISBOT_HOME
//	  Sets the root directory for NemesisBot project data.
//	  The actual project directory will be: $NEMESISBOT_HOME/.nemesisbot/
//
//	  Example:
//	    export NEMESISBOT_HOME=/opt/nemesisbot
//	    # Actual directory: /opt/nemesisbot/.nemesisbot/
//
//	NEMESISBOT_CONFIG
//	  Override the main config.json file path (advanced usage)
//
// Priority Order (for home directory resolution):
//  1. LocalMode flag (set by --local parameter)
//     → Uses ./.nemesisbot/ (current directory)
//  2. NEMESISBOT_HOME environment variable
//     → Uses $NEMESISBOT_HOME/.nemesisbot/
//  3. Auto-detection
//     → Uses ./.nemesisbot/ (if exists in current directory)
//  4. Default
//     → Uses ~/.nemesisbot/
//
// Directory Structure:
//
//	When NEMESISBOT_HOME is set:
//	$NEMESISBOT_HOME/
//	└── .nemesisbot/           ← Project directory
//	    ├── config.json       ← Main configuration
//	    └── workspace/        ← Agent workspace
//	        ├── cluster/
//	        ├── agents/
//	        └── logs/
//
//	This design ensures:
//	- All project data is contained within .nemesisbot/ directory
//	- Easy migration: just copy the .nemesisbot/ directory
//	- Multi-instance support: multiple .nemesisbot/ directories
//	- Clear separation between program and data
package path

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"sync"
)

const (
	// Environment variable names
	EnvHome           = "NEMESISBOT_HOME"
	EnvConfig         = "NEMESISBOT_CONFIG"
	EnvMCPConfig      = "NEMESISBOT_MCP_CONFIG"
	EnvSecurityConfig = "NEMESISBOT_SECURITY_CONFIG"
	EnvSkillsConfig   = "NEMESISBOT_SKILLS_CONFIG"

	// DefaultHomeDir is the default directory name in user's home directory
	DefaultHomeDir = ".nemesisbot"
)

var (
	defaultManager *PathManager
	managerOnce    sync.Once

	// LocalMode forces using current directory's .nemesisbot
	// Set by --local command-line flag
	LocalMode bool
)

// PathManager manages all NemesisBot paths with caching and thread-safety.
type PathManager struct {
	mu sync.RWMutex

	// Cached paths
	homeDir            string
	configPath         string
	mcpConfigPath      string
	securityConfigPath string
	skillsConfigPath   string
	workspace          string
	authPath           string
	auditLogDir        string
}

// NewPathManager creates a new PathManager with default home directory.
func NewPathManager() *PathManager {
	homeDir, _ := ResolveHomeDir()
	return NewPathManagerWithHome(homeDir)
}

// NewPathManagerWithHome creates a new PathManager with specified home directory.
func NewPathManagerWithHome(homeDir string) *PathManager {
	pm := &PathManager{
		homeDir: homeDir,
	}
	pm.initPaths()
	return pm
}

// initPaths initializes all derived paths.
func (pm *PathManager) initPaths() {
	pm.mu.Lock()
	defer pm.mu.Unlock()

	pm.workspace = filepath.Join(pm.homeDir, "workspace")
	pm.authPath = filepath.Join(pm.homeDir, "auth.json")
	pm.auditLogDir = filepath.Join(pm.homeDir, "workspace", "logs", "security_logs")
}

// HomeDir returns the NemesisBot home directory.
func (pm *PathManager) HomeDir() string {
	pm.mu.RLock()
	defer pm.mu.RUnlock()
	return pm.homeDir
}

// ConfigPath returns the main configuration file path.
func (pm *PathManager) ConfigPath() string {
	pm.mu.RLock()
	defer pm.mu.RUnlock()

	if pm.configPath != "" {
		return pm.configPath
	}

	// Check environment variable override
	if envPath := os.Getenv(EnvConfig); envPath != "" {
		return envPath
	}

	// Default path
	return filepath.Join(pm.homeDir, "config.json")
}

// SetConfigPath sets a custom config path (for testing or special cases).
func (pm *PathManager) SetConfigPath(path string) {
	pm.mu.Lock()
	defer pm.mu.Unlock()
	pm.configPath = path
}

// MCPConfigPath returns the MCP configuration file path.
func (pm *PathManager) MCPConfigPath() string {
	pm.mu.RLock()
	defer pm.mu.RUnlock()

	if pm.mcpConfigPath != "" {
		return pm.mcpConfigPath
	}

	// Check environment variable override
	if envPath := os.Getenv(EnvMCPConfig); envPath != "" {
		return envPath
	}

	// Default path
	return filepath.Join(pm.homeDir, "config.mcp.json")
}

// SetMCPConfigPath sets a custom MCP config path.
func (pm *PathManager) SetMCPConfigPath(path string) {
	pm.mu.Lock()
	defer pm.mu.Unlock()
	pm.mcpConfigPath = path
}

// SecurityConfigPath returns the security configuration file path.
func (pm *PathManager) SecurityConfigPath() string {
	pm.mu.RLock()
	defer pm.mu.RUnlock()

	if pm.securityConfigPath != "" {
		return pm.securityConfigPath
	}

	// Check environment variable override
	if envPath := os.Getenv(EnvSecurityConfig); envPath != "" {
		return envPath
	}

	// Default path
	return filepath.Join(pm.homeDir, "config.security.json")
}

// SetSecurityConfigPath sets a custom security config path.
func (pm *PathManager) SetSecurityConfigPath(path string) {
	pm.mu.Lock()
	defer pm.mu.Unlock()
	pm.securityConfigPath = path
}

// SkillsConfigPath returns the skills configuration file path.
func (pm *PathManager) SkillsConfigPath() string {
	pm.mu.RLock()
	defer pm.mu.RUnlock()

	if pm.skillsConfigPath != "" {
		return pm.skillsConfigPath
	}

	// Check environment variable override
	if envPath := os.Getenv(EnvSkillsConfig); envPath != "" {
		return envPath
	}

	// Default path
	return filepath.Join(pm.homeDir, "config.skills.json")
}

// SetSkillsConfigPath sets a custom skills config path.
func (pm *PathManager) SetSkillsConfigPath(path string) {
	pm.mu.Lock()
	defer pm.mu.Unlock()
	pm.skillsConfigPath = path
}

// Workspace returns the workspace directory path.
func (pm *PathManager) Workspace() string {
	pm.mu.RLock()
	defer pm.mu.RUnlock()
	return pm.workspace
}

// AuthPath returns the authentication storage file path.
func (pm *PathManager) AuthPath() string {
	pm.mu.RLock()
	defer pm.mu.RUnlock()
	return pm.authPath
}

// AuditLogDir returns the security audit log directory path.
func (pm *PathManager) AuditLogDir() string {
	pm.mu.RLock()
	defer pm.mu.RUnlock()
	return pm.auditLogDir
}

// TempDir returns the temporary directory path for downloads and temporary files.
// The temp directory is located at workspace/temp.
func (pm *PathManager) TempDir() string {
	pm.mu.RLock()
	defer pm.mu.RUnlock()
	return filepath.Join(pm.workspace, "temp")
}

// AgentWorkspace returns the workspace directory for a specific agent.
// For main/default agents, returns the main workspace.
// For other agents, returns a separate workspace directory.
func (pm *PathManager) AgentWorkspace(agentID string) string {
	pm.mu.RLock()
	defer pm.mu.RUnlock()

	// Default agents use main workspace
	if agentID == "" || agentID == "main" || agentID == "default" {
		return pm.workspace
	}

	// Other agents use separate workspace
	return filepath.Join(pm.homeDir, "workspace-"+agentID)
}

// ExpandHome expands the ~ symbol to the user's home directory.
// It supports both "~" and "~/path" formats (with both / and \ separators).
func ExpandHome(path string) string {
	if path == "" {
		return path
	}
	if path[0] == '~' {
		home, _ := os.UserHomeDir()
		if len(path) > 1 && (path[1] == filepath.Separator || path[1] == '/') {
			// Skip both / and \ separators
			return filepath.Join(home, path[2:])
		}
		return home
	}
	return path
}

// DetectLocal checks if there's a .nemesisbot directory in the current directory.
// This is used for automatic local mode detection.
func DetectLocal() bool {
	// Check if .nemesisbot exists in current directory
	localDir := filepath.Join(".", DefaultHomeDir)
	info, err := os.Stat(localDir)
	if err != nil {
		return false
	}
	return info.IsDir()
}

// ResolveHomeDir resolves the NemesisBot home directory.
//
// The NemesisBot home directory (.nemesisbot/) contains all project data:
//   - config.json: Main configuration file
//   - workspace/: Agent workspace directory
//
// Priority: LocalMode > NEMESISBOT_HOME > Auto-detect > Default
//
// When NEMESISBOT_HOME is set, the project directory is created as:
//
//	$NEMESISBOT_HOME/.nemesisbot/
//
// Examples:
//
//	NEMESISBOT_HOME=/opt/nemesisbot  →  /opt/nemesisbot/.nemesisbot/
//	LocalMode (--local)              →  ./.nemesisbot/
//	Auto-detect                       →  ./.nemesisbot/
//	Default                           →  ~/.nemesisbot/
func ResolveHomeDir() (string, error) {
	// 1. Check if LocalMode is explicitly set (highest priority)
	if LocalMode {
		// Use current directory's .nemesisbot
		cwd, err := os.Getwd()
		if err != nil {
			return "", err
		}
		return filepath.Join(cwd, DefaultHomeDir), nil
	}

	// 2. Check NEMESISBOT_HOME environment variable
	if envHome := os.Getenv(EnvHome); envHome != "" {
		// Create .nemesisbot directory under NEMESISBOT_HOME
		// This keeps all project data in a single .nemesisbot/ directory
		return filepath.Join(ExpandHome(envHome), DefaultHomeDir), nil
	}

	// 3. Auto-detect: check if .nemesisbot exists in current directory
	if DetectLocal() {
		cwd, err := os.Getwd()
		if err != nil {
			// If we can't get cwd, fall through to default
		} else {
			return filepath.Join(cwd, DefaultHomeDir), nil
		}
	}

	// 4. Use default home directory
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}

	return filepath.Join(home, DefaultHomeDir), nil
}

// ResolveConfigPath resolves the main configuration file path.
// Priority: NEMESISBOT_CONFIG > LocalMode/auto-detect > Default
func ResolveConfigPath() string {
	// Check config file environment variable (highest priority)
	if envPath := os.Getenv(EnvConfig); envPath != "" {
		return envPath
	}

	// Use ResolveHomeDir which handles LocalMode, env vars, and auto-detection
	homeDir, err := ResolveHomeDir()
	if err != nil {
		// Fallback to default
		home, _ := os.UserHomeDir()
		homeDir = filepath.Join(home, DefaultHomeDir)
	}

	return filepath.Join(homeDir, "config.json")
}

// ResolveMCPConfigPath resolves the MCP configuration file path.
// Priority: NEMESISBOT_MCP_CONFIG > workspace/config/config.mcp.json > Default
func ResolveMCPConfigPath() string {
	if envPath := os.Getenv(EnvMCPConfig); envPath != "" {
		return envPath
	}

	// Try to get workspace from main config
	homeDir, err := ResolveHomeDir()
	if err != nil {
		home, _ := os.UserHomeDir()
		homeDir = filepath.Join(home, DefaultHomeDir)
	}

	// Try to load main config to get workspace path
	configPath := filepath.Join(homeDir, "config.json")
	if cfg, err := loadConfigForWorkspace(configPath); err == nil {
		workspace := cfg.WorkspacePath()
		return filepath.Join(workspace, "config", "config.mcp.json")
	}

	// Fallback to old location (for backward compatibility)
	return filepath.Join(homeDir, "config.mcp.json")
}

// ResolveSecurityConfigPath resolves the security configuration file path.
// Priority: NEMESISBOT_SECURITY_CONFIG > workspace/config/config.security.json > Default
func ResolveSecurityConfigPath() string {
	if envPath := os.Getenv(EnvSecurityConfig); envPath != "" {
		return envPath
	}

	// Try to get workspace from main config
	homeDir, err := ResolveHomeDir()
	if err != nil {
		home, _ := os.UserHomeDir()
		homeDir = filepath.Join(home, DefaultHomeDir)
	}

	// Try to load main config to get workspace path
	configPath := filepath.Join(homeDir, "config.json")
	if cfg, err := loadConfigForWorkspace(configPath); err == nil {
		workspace := cfg.WorkspacePath()
		return filepath.Join(workspace, "config", "config.security.json")
	}

	// Fallback to old location (for backward compatibility)
	return filepath.Join(homeDir, "config.security.json")
}

// loadConfigForWorkspace is a helper to load main config for workspace path resolution.
// This avoids circular dependency by doing a minimal JSON load.
func loadConfigForWorkspace(configPath string) (*minimalConfig, error) {
	data, err := os.ReadFile(configPath)
	if err != nil {
		return nil, err
	}

	var cfg minimalConfig
	if err := json.Unmarshal(data, &cfg); err != nil {
		return nil, err
	}

	return &cfg, nil
}

// minimalConfig is a minimal config struct for workspace path resolution.
type minimalConfig struct {
	Agents struct {
		Defaults struct {
			Workspace string `json:"workspace"`
		} `json:"defaults"`
	} `json:"agents"`
}

// WorkspacePath returns the workspace path from minimal config.
func (c *minimalConfig) WorkspacePath() string {
	ws := c.Agents.Defaults.Workspace
	if ws == "" {
		return filepath.Join("~", ".nemesisbot", "workspace")
	}

	// Expand ~ if present
	if strings.HasPrefix(ws, "~/") {
		home, _ := os.UserHomeDir()
		return filepath.Join(home, ws[2:])
	}

	return ws
}

// DefaultPathManager returns the default singleton PathManager instance.
func DefaultPathManager() *PathManager {
	managerOnce.Do(func() {
		defaultManager = NewPathManager()
	})
	return defaultManager
}

// ResolveMCPConfigPathInWorkspace returns the MCP config path for a specific workspace.
// Usage: For runtime components that already know the workspace path.
func ResolveMCPConfigPathInWorkspace(workspace string) string {
	return filepath.Join(workspace, "config", "config.mcp.json")
}

// ResolveSecurityConfigPathInWorkspace returns the security config path for a specific workspace.
// Usage: For runtime components that already know the workspace path.
func ResolveSecurityConfigPathInWorkspace(workspace string) string {
	return filepath.Join(workspace, "config", "config.security.json")
}

// ResolveClusterConfigPathInWorkspace returns the cluster config path for a specific workspace.
// Usage: For runtime components that already know the workspace path.
func ResolveClusterConfigPathInWorkspace(workspace string) string {
	return filepath.Join(workspace, "config", "config.cluster.json")
}

// ResolveSkillsConfigPath resolves the skills configuration file path.
// Priority: NEMESISBOT_SKILLS_CONFIG > workspace/config/config.skills.json > Default
func ResolveSkillsConfigPath() string {
	if envPath := os.Getenv(EnvSkillsConfig); envPath != "" {
		return envPath
	}

	// Try to get workspace from main config
	homeDir, err := ResolveHomeDir()
	if err != nil {
		home, _ := os.UserHomeDir()
		homeDir = filepath.Join(home, DefaultHomeDir)
	}

	// Try to load main config to get workspace path
	configPath := filepath.Join(homeDir, "config.json")
	if cfg, err := loadConfigForWorkspace(configPath); err == nil {
		workspace := cfg.WorkspacePath()
		return filepath.Join(workspace, "config", "config.skills.json")
	}

	// Fallback to old location (for backward compatibility)
	return filepath.Join(homeDir, "config.skills.json")
}

// ResolveSkillsConfigPathInWorkspace returns the skills config path for a specific workspace.
// Usage: For runtime components that already know the workspace path.
func ResolveSkillsConfigPathInWorkspace(workspace string) string {
	return filepath.Join(workspace, "config", "config.skills.json")
}
