// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// Package security provides security plugin for NemesisBot
package security

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/path"
	"github.com/276793422/NemesisBot/module/plugin"
)

// SecurityPlugin implements the plugin interface for security checks
type SecurityPlugin struct {
	*plugin.BasePlugin
	auditor      *SecurityAuditor
	enabled      bool
	configPath   string
	mu           sync.RWMutex
	logFile      *os.File
	logFilePath  string
}

// NewSecurityPlugin creates a new security plugin
func NewSecurityPlugin() *SecurityPlugin {
	return &SecurityPlugin{
		BasePlugin: plugin.NewBasePlugin("security", "1.0.0"),
		enabled:    false,
	}
}

// Init initializes the security plugin with configuration
func (p *SecurityPlugin) Init(pluginConfig map[string]interface{}) error {
	// Extract configuration
	configPath, ok := pluginConfig["config_path"].(string)
	if !ok {
		configPath = path.DefaultPathManager().SecurityConfigPath()
	}
	p.configPath = configPath

	enabled, _ := pluginConfig["enabled"].(bool)
	p.enabled = enabled

	// Load security configuration
	securityCfg, err := config.LoadSecurityConfig(configPath)
	if err != nil {
		return fmt.Errorf("failed to load security config: %w", err)
	}

	// Initialize auditor if enabled
	if p.enabled {
		auditLogDir := path.DefaultPathManager().AuditLogDir()

		auditorConfig := &AuditorConfig{
			Enabled:               true,
			LogAllOperations:      securityCfg.LogAllOperations,
			LogDenialsOnly:        securityCfg.LogDenialsOnly,
			ApprovalTimeout:       time.Duration(securityCfg.ApprovalTimeout) * time.Second,
			MaxPendingRequests:    securityCfg.MaxPendingRequests,
			AuditLogRetentionDays: securityCfg.AuditLogRetentionDays,
			AuditLogPath:          securityCfg.AuditLogPath,
			AuditLogFileEnabled:   securityCfg.AuditLogFileEnabled,
			AuditLogDir:           auditLogDir,
			SynchronousMode:       securityCfg.SynchronousMode,
			DefaultAction:         securityCfg.DefaultAction,
		}

		p.auditor = NewSecurityAuditor(auditorConfig)

		// Register rules
		p.registerRules(securityCfg)

		// Initialize log file
		if err := p.initAuditLogFile(); err != nil {
			logger.ErrorCF("security", "Failed to initialize audit log file", map[string]interface{}{
				"error": err.Error(),
			})
		}

		logger.InfoC("security", "Security plugin initialized")
	}

	return nil
}

// registerRules registers security rules from configuration
func (p *SecurityPlugin) registerRules(cfg *config.SecurityConfig) {
	if p.auditor == nil {
		return
	}

	// File rules
	if cfg.FileRules != nil {
		if len(cfg.FileRules.Read) > 0 {
			p.auditor.SetRules(OpFileRead, cfg.FileRules.Read)
		}
		if len(cfg.FileRules.Write) > 0 {
			p.auditor.SetRules(OpFileWrite, cfg.FileRules.Write)
		}
		if len(cfg.FileRules.Delete) > 0 {
			p.auditor.SetRules(OpFileDelete, cfg.FileRules.Delete)
		}
	}

	// Directory rules
	if cfg.DirectoryRules != nil {
		if len(cfg.DirectoryRules.Read) > 0 {
			p.auditor.SetRules(OpDirRead, cfg.DirectoryRules.Read)
		}
		if len(cfg.DirectoryRules.Create) > 0 {
			p.auditor.SetRules(OpDirCreate, cfg.DirectoryRules.Create)
		}
		if len(cfg.DirectoryRules.Delete) > 0 {
			p.auditor.SetRules(OpDirDelete, cfg.DirectoryRules.Delete)
		}
	}

	// Process rules
	if cfg.ProcessRules != nil {
		if len(cfg.ProcessRules.Exec) > 0 {
			p.auditor.SetRules(OpProcessExec, cfg.ProcessRules.Exec)
		}
		if len(cfg.ProcessRules.Spawn) > 0 {
			p.auditor.SetRules(OpProcessSpawn, cfg.ProcessRules.Spawn)
		}
		if len(cfg.ProcessRules.Kill) > 0 {
			p.auditor.SetRules(OpProcessKill, cfg.ProcessRules.Kill)
		}
		if len(cfg.ProcessRules.Suspend) > 0 {
			p.auditor.SetRules(OpProcessSuspend, cfg.ProcessRules.Suspend)
		}
	}

	// Network rules
	if cfg.NetworkRules != nil {
		if len(cfg.NetworkRules.Request) > 0 {
			p.auditor.SetRules(OpNetworkRequest, cfg.NetworkRules.Request)
		}
		if len(cfg.NetworkRules.Download) > 0 {
			p.auditor.SetRules(OpNetworkDownload, cfg.NetworkRules.Download)
		}
		if len(cfg.NetworkRules.Upload) > 0 {
			p.auditor.SetRules(OpNetworkUpload, cfg.NetworkRules.Upload)
		}
	}

	// Hardware rules
	if cfg.HardwareRules != nil {
		if len(cfg.HardwareRules.I2C) > 0 {
			p.auditor.SetRules(OpHardwareI2C, cfg.HardwareRules.I2C)
		}
		if len(cfg.HardwareRules.SPI) > 0 {
			p.auditor.SetRules(OpHardwareSPI, cfg.HardwareRules.SPI)
		}
		if len(cfg.HardwareRules.GPIO) > 0 {
			p.auditor.SetRules(OpHardwareGPIO, cfg.HardwareRules.GPIO)
		}
	}

	// Registry rules
	if cfg.RegistryRules != nil {
		if len(cfg.RegistryRules.Read) > 0 {
			p.auditor.SetRules(OpRegistryRead, cfg.RegistryRules.Read)
		}
		if len(cfg.RegistryRules.Write) > 0 {
			p.auditor.SetRules(OpRegistryWrite, cfg.RegistryRules.Write)
		}
		if len(cfg.RegistryRules.Delete) > 0 {
			p.auditor.SetRules(OpRegistryDelete, cfg.RegistryRules.Delete)
		}
	}
}

// Execute implements the plugin interface for security checks
func (p *SecurityPlugin) Execute(ctx context.Context, invocation *plugin.ToolInvocation) (bool, error, bool) {
	p.mu.RLock()
	defer p.mu.RUnlock()

	// If not enabled, allow everything
	if !p.enabled || p.auditor == nil {
		return true, nil, false
	}

	// Map tool name to operation type
	opType := p.toolToOperation(invocation.ToolName)
	if opType == "" {
		// Unknown tool, allow by default
		return true, nil, false
	}

	// Determine target from args
	target := p.extractTarget(invocation.ToolName, invocation.Args)

	// Create operation request
	req := &OperationRequest{
		Type:        opType,
		DangerLevel: GetDangerLevel(opType),
		User:        invocation.User,
		Source:      invocation.Source,
		Target:      target,
		Context:     invocation.Metadata,
	}

	// Request permission
	allowed, err, _ := p.auditor.RequestPermission(ctx, req)
	if !allowed {
		if err != nil {
			return false, err, false
		}
		return false, fmt.Errorf("operation denied by security policy"), false
	}

	return true, nil, false
}

// toolToOperation maps tool names to operation types
func (p *SecurityPlugin) toolToOperation(toolName string) OperationType {
	switch toolName {
	case "read_file":
		return OpFileRead
	case "write_file", "edit_file", "append_file":
		return OpFileWrite
	case "delete_file":
		return OpFileDelete
	case "list_directory", "list_dir":
		return OpDirRead
	case "create_directory", "create_dir":
		return OpDirCreate
	case "delete_directory", "delete_dir":
		return OpDirDelete
	case "exec", "execute_command":
		return OpProcessExec
	case "spawn":
		return OpProcessSpawn
	case "kill", "kill_process":
		return OpProcessKill
	case "download":
		return OpNetworkDownload
	case "upload":
		return OpNetworkUpload
	case "http_request", "web_request":
		return OpNetworkRequest
	default:
		return ""
	}
}

// extractTarget extracts the target from arguments based on tool name
func (p *SecurityPlugin) extractTarget(toolName string, args map[string]interface{}) string {
	switch toolName {
	case "read_file", "write_file", "edit_file", "append_file", "delete_file":
		if path, ok := args["path"].(string); ok {
			return path
		}
	case "list_directory", "list_dir", "create_directory", "create_dir", "delete_directory", "delete_dir":
		if path, ok := args["path"].(string); ok {
			return path
		}
	case "exec", "execute_command":
		if cmd, ok := args["command"].(string); ok {
			return cmd
		}
	case "spawn":
		if cmd, ok := args["command"].(string); ok {
			return cmd
		}
	case "download":
		if url, ok := args["url"].(string); ok {
			return url
		}
	case "upload":
		if url, ok := args["url"].(string); ok {
			return url
		}
	case "http_request", "web_request":
		if url, ok := args["url"].(string); ok {
			return url
		}
	}
	return ""
}

// initAuditLogFile initializes the audit log file
func (p *SecurityPlugin) initAuditLogFile() error {
	if p.auditor == nil || p.auditor.config == nil {
		return fmt.Errorf("auditor not initialized")
	}

	if !p.auditor.config.AuditLogFileEnabled || p.auditor.config.AuditLogDir == "" {
		return nil
	}

	// Create log directory
	if err := os.MkdirAll(p.auditor.config.AuditLogDir, 0755); err != nil {
		return fmt.Errorf("failed to create audit log directory: %w", err)
	}

	// Generate log file name with current date
	dateStr := time.Now().Format("2006-01-02")
	logFileName := fmt.Sprintf("security_audit_%s.log", dateStr)
	p.logFilePath = filepath.Join(p.auditor.config.AuditLogDir, logFileName)

	// Open log file in append mode
	file, err := os.OpenFile(p.logFilePath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0640)
	if err != nil {
		return fmt.Errorf("failed to open audit log file: %w", err)
	}

	p.logFile = file

	// Write header if file is empty
	stat, err := file.Stat()
	if err == nil && stat.Size() == 0 {
		header := "# NemesisBot Security Audit Log\n" +
			"# Format: TIMESTAMP | EVENT_ID | DECISION | OPERATION | USER | SOURCE | TARGET | DANGER | REASON | POLICY\n"
		if _, err := file.WriteString(header); err != nil {
			return fmt.Errorf("failed to write audit log header: %w", err)
		}
	}

	return nil
}

// Cleanup cleans up the security plugin
func (p *SecurityPlugin) Cleanup() error {
	p.mu.Lock()
	defer p.mu.Unlock()

	// Close audit log file
	if p.logFile != nil {
		if err := p.logFile.Close(); err != nil {
			return err
		}
		p.logFile = nil
	}

	// Clean up auditor
	if p.auditor != nil {
		if err := p.auditor.Close(); err != nil {
			return err
		}
	}

	return nil
}

// GetAuditor returns the security auditor (for backward compatibility)
func (p *SecurityPlugin) GetAuditor() *SecurityAuditor {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.auditor
}

// IsEnabled returns whether the plugin is enabled
func (p *SecurityPlugin) IsEnabled() bool {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.enabled
}

// SetEnabled sets the enabled state
func (p *SecurityPlugin) SetEnabled(enabled bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.enabled = enabled
}

// ReloadConfig reloads the security configuration
func (p *SecurityPlugin) ReloadConfig() error {
	p.mu.Lock()
	defer p.mu.Unlock()

	// Clean up existing resources
	if p.auditor != nil {
		p.auditor.Close()
	}

	// Reload configuration
	config := map[string]interface{}{
		"config_path": p.configPath,
		"enabled":     true,
	}

	if err := p.Init(config); err != nil {
		return fmt.Errorf("failed to reload security config: %w", err)
	}

	logger.InfoC("security", "Security configuration reloaded")
	return nil
}
