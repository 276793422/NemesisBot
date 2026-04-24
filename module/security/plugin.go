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
	"github.com/276793422/NemesisBot/module/security/approval"
	"github.com/276793422/NemesisBot/module/security/command"
	"github.com/276793422/NemesisBot/module/security/credential"
	"github.com/276793422/NemesisBot/module/security/dlp"
	"github.com/276793422/NemesisBot/module/security/injection"
	"github.com/276793422/NemesisBot/module/security/integrity"
	"github.com/276793422/NemesisBot/module/security/scanner"
	"github.com/276793422/NemesisBot/module/security/ssrf"
)

// globalScannerChain is the shared scanner chain instance.
// Since multiple agents may be created during startup, we ensure only one
// scanner chain is initialized to avoid port conflicts (e.g., clamd on 3310).
var globalScannerChain *scanner.ScanChain
var globalScannerOnce sync.Once

// SecurityPlugin implements the plugin interface for security checks
type SecurityPlugin struct {
	*plugin.BasePlugin
	auditor     *SecurityAuditor
	approvalMgr approval.ApprovalManager
	scanChain   *scanner.ScanChain

	// Security layers
	injectionDetector *injection.Detector
	commandGuard      *command.Guard
	credentialScanner *credential.Scanner
	dlpEngine         *dlp.DLPEngine
	ssrfGuard         *ssrf.Guard
	auditChain        *integrity.AuditChain

	enabled     bool
	configPath  string
	mu          sync.RWMutex
	logFile     *os.File
	logFilePath string
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

		// Initialize approval manager
		if err := p.initApprovalManager(securityCfg); err != nil {
			logger.ErrorCF("security", "Failed to initialize approval manager", map[string]interface{}{
				"error": err.Error(),
			})
			// Continue without approval manager - will fall back to pending requests
		}

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

	// Initialize scanner chain (independent of security enabled flag)
	p.initScannerChain()

	// Initialize security layers
	p.initSecurityLayers(securityCfg)

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
// Security layers are applied in order:
// 1. InjectionDetector - intercept malicious input
// 2. CommandGuard - check dangerous commands
// 3. Auditor (ABAC) - attribute-based access control
// 4. CredentialScanner - detect leaked credentials
// 5. DLP - scan sensitive data
// 6. SSRFGuard - validate URLs for network operations
// 7. ScanChain - virus scanning
// 8. AuditChain - Merkle integrity audit log
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

	// Layer 1: Injection Detection
	if p.injectionDetector != nil {
		result, err := p.injectionDetector.AnalyzeToolInput(ctx, invocation.ToolName, invocation.Args)
		if err != nil {
			logger.WarnCF("security", "Injection detection error", map[string]interface{}{
				"error": err.Error(),
			})
		} else if result.IsInjection {
			logger.WarnCF("security", "Injection detected", map[string]interface{}{
				"tool": invocation.ToolName,
				"score": fmt.Sprintf("%.2f", result.Score),
				"level": result.Level,
				"patterns": len(result.MatchedPatterns),
			})
			return false, fmt.Errorf("operation blocked: potential prompt injection detected (score: %.2f, level: %s)", result.Score, result.Level), false
		}
	}

	// Layer 2: Command Guard
	if p.commandGuard != nil && (opType == OpProcessExec || opType == OpProcessSpawn) {
		if target != "" {
			if err := p.commandGuard.Check(ctx, target); err != nil {
				logger.WarnCF("security", "Dangerous command blocked", map[string]interface{}{
					"tool":    invocation.ToolName,
					"command": target,
					"error":   err.Error(),
				})
				return false, fmt.Errorf("operation blocked by command guard: %w", err), false
			}
		}
	}

	// Layer 3: ABAC (existing auditor)
	req := &OperationRequest{
		Type:        opType,
		DangerLevel: GetDangerLevel(opType),
		User:        invocation.User,
		Source:      invocation.Source,
		Target:      target,
		Context:     invocation.Metadata,
	}

	allowed, err, _ := p.auditor.RequestPermission(ctx, req)
	if !allowed {
		if err != nil {
			return false, err, false
		}
		return false, fmt.Errorf("operation denied by security policy"), false
	}

	// Layer 4: Credential Scanner (scan tool arguments for leaked credentials)
	if p.credentialScanner != nil {
		for _, v := range invocation.Args {
			if strVal, ok := v.(string); ok && len(strVal) > 10 {
				result, scanErr := p.credentialScanner.ScanContent(ctx, strVal)
				if scanErr != nil {
					logger.WarnCF("security", "Credential scan error", map[string]interface{}{
						"error": scanErr.Error(),
					})
					continue
				}
				if result.HasMatches && result.Action == "block" {
					logger.WarnCF("security", "Credential leak detected in input", map[string]interface{}{
						"tool":     invocation.ToolName,
						"matches":  len(result.Matches),
						"summary":  result.Summary,
					})
					return false, fmt.Errorf("operation blocked: potential credential leak detected in input (%s)", result.Summary), false
				}
			}
		}
	}

	// Layer 5: DLP (Data Loss Prevention)
	if p.dlpEngine != nil {
		dlpResult, dlpErr := p.dlpEngine.ScanToolInput(ctx, invocation.ToolName, invocation.Args)
		if dlpErr != nil {
			logger.WarnCF("security", "DLP scan error", map[string]interface{}{
				"error": dlpErr.Error(),
			})
		} else if dlpResult.HasMatches && dlpResult.Action == "block" {
			logger.WarnCF("security", "Sensitive data detected by DLP", map[string]interface{}{
				"tool":    invocation.ToolName,
				"matches": len(dlpResult.Matches),
				"summary": dlpResult.Summary,
			})
			return false, fmt.Errorf("operation blocked by DLP: sensitive data detected (%s)", dlpResult.Summary), false
		}
	}

	// Layer 6: SSRF Guard (for network operations)
	if p.ssrfGuard != nil {
		urlTarget := p.extractURL(invocation.ToolName, invocation.Args)
		if urlTarget != "" {
			if err := p.ssrfGuard.ValidateURL(ctx, urlTarget); err != nil {
				logger.WarnCF("security", "SSRF protection triggered", map[string]interface{}{
					"tool": invocation.ToolName,
					"url":  urlTarget,
					"error": err.Error(),
				})
				return false, fmt.Errorf("operation blocked by SSRF guard: %w", err), false
			}
		}
	}

	// Layer 7: Virus Scanner (existing scan chain)
	if p.scanChain != nil {
		clean, scanErr := p.scanChain.ScanToolInvocation(ctx, invocation.ToolName, invocation.Args)
		if !clean {
			if scanErr != nil {
				return false, scanErr, false
			}
			return false, fmt.Errorf("operation blocked by virus scanner"), false
		}
	}

	// Layer 8: Audit Chain (record approved operation)
	if p.auditChain != nil {
		event := &integrity.AuditEvent{
			Timestamp: time.Now(),
			Operation: string(opType),
			ToolName:  invocation.ToolName,
			User:      invocation.User,
			Source:    invocation.Source,
			Target:    target,
			Decision:  "allowed",
			Reason:    "passed all security layers",
		}
		if appendErr := p.auditChain.Append(ctx, event); appendErr != nil {
			logger.WarnCF("security", "Failed to append to audit chain", map[string]interface{}{
				"error": appendErr.Error(),
			})
		}
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

	// Stop approval manager
	if p.approvalMgr != nil {
		if err := p.approvalMgr.Stop(); err != nil {
			logger.ErrorCF("security", "Failed to stop approval manager", map[string]interface{}{
				"error": err.Error(),
			})
		}
		p.approvalMgr = nil
	}

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

	// Stop scanner chain
	if p.scanChain != nil {
		p.scanChain.Stop()
		p.scanChain = nil
	}

	// Close audit chain
	if p.auditChain != nil {
		if err := p.auditChain.Close(); err != nil {
			logger.ErrorCF("security", "Failed to close audit chain", map[string]interface{}{
				"error": err.Error(),
			})
		}
		p.auditChain = nil
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

// initScannerChain loads scanner configuration and starts the scan chain.
// Uses a singleton pattern to avoid starting multiple scanner instances
// (e.g., multiple agents during Bot startup all trying to bind port 3310).
func (p *SecurityPlugin) initScannerChain() {
	// If a global scanner chain already exists, reuse it
	if globalScannerChain != nil {
		p.scanChain = globalScannerChain
		return
	}

	scannerConfigPath := path.ResolveScannerConfigPath()
	scannerCfg, err := config.LoadScannerConfig(scannerConfigPath)
	if err != nil {
		logger.WarnCF("security", "Failed to load scanner config", map[string]interface{}{
			"error": err.Error(),
		})
		return
	}

	if len(scannerCfg.Enabled) == 0 {
		return
	}

	chain := scanner.NewScanChain()
	if err := chain.LoadFromConfig(scannerCfg); err != nil {
		logger.WarnCF("security", "Failed to load scan chain", map[string]interface{}{
			"error": err.Error(),
		})
		return
	}

	ctx, cancel := context.WithTimeout(context.Background(), 120*time.Second)
	defer cancel()

	globalScannerOnce.Do(func() {
		if err := chain.Start(ctx); err != nil {
			logger.WarnCF("security", "Failed to start scan chain", map[string]interface{}{
				"error": err.Error(),
			})
			return
		}
		globalScannerChain = chain
		logger.InfoCF("security", "Scanner chain initialized", map[string]interface{}{
			"engines": scannerCfg.Enabled,
		})
	})

	p.scanChain = globalScannerChain
}

// initApprovalManager initializes the approval manager for interactive approval dialogs
func (p *SecurityPlugin) initApprovalManager(cfg *config.SecurityConfig) error {
	if p.auditor == nil {
		return fmt.Errorf("auditor not initialized")
	}

	// Create approval configuration
	approvalConfig := &approval.ApprovalConfig{
		Enabled:         true,
		Timeout:         30 * time.Second,
		MinRiskLevel:    "MEDIUM",
		DialogWidth:     550,
		DialogHeight:    480,
		EnableSound:     true,
		EnableAnimation: true,
	}

	// Override from config if available
	if cfg.ApprovalTimeout > 0 {
		approvalConfig.Timeout = time.Duration(cfg.ApprovalTimeout) * time.Second
	}

	// Create approval manager
	p.approvalMgr = approval.NewApprovalManager(approvalConfig)
	if p.approvalMgr == nil {
		return fmt.Errorf("failed to create approval manager")
	}

	// Start approval manager
	if err := p.approvalMgr.Start(); err != nil {
		p.approvalMgr = nil
		return fmt.Errorf("failed to start approval manager: %w", err)
	}

	// Set approval manager to auditor
	p.auditor.SetApprovalManager(p.approvalMgr)

	logger.InfoC("security", "Approval manager initialized and started")
	return nil
}

// initSecurityLayers initializes all security layers based on configuration
func (p *SecurityPlugin) initSecurityLayers(cfg *config.SecurityConfig) {
	if cfg.Layers == nil {
		return
	}

	// Layer 1: Injection Detector
	if cfg.Layers.Injection != nil && cfg.Layers.Injection.Enabled {
		threshold := 0.7
		if val, ok := cfg.Layers.Injection.Extra["threshold"].(float64); ok {
			threshold = val
		}
		p.injectionDetector = injection.NewDetector(injection.Config{
			Enabled:       true,
			Threshold:     threshold,
			MaxInputLength: 100000,
		})
		logger.InfoC("security", "Injection detector initialized")
	}

	// Layer 2: Command Guard
	if cfg.Layers.CommandGuard != nil && cfg.Layers.CommandGuard.Enabled {
		guard, err := command.NewGuard(command.Config{Enabled: true})
		if err != nil {
			logger.ErrorCF("security", "Failed to initialize command guard", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			p.commandGuard = guard
			logger.InfoC("security", "Command guard initialized")
		}
	}

	// Layer 4: Credential Scanner
	if cfg.Layers.Credential != nil && cfg.Layers.Credential.Enabled {
		scanner, err := credential.NewScanner(&credential.Config{Enabled: true, Action: "block"})
		if err != nil {
			logger.ErrorCF("security", "Failed to initialize credential scanner", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			p.credentialScanner = scanner
			logger.InfoC("security", "Credential scanner initialized")
		}
	}

	// Layer 5: DLP Engine
	if cfg.Layers.DLP != nil && cfg.Layers.DLP.Enabled {
		dlpCfg := dlp.Config{
			Enabled: true,
			ActionOnMatch: cfg.Layers.DLP.Action,
		}
		if len(cfg.Layers.DLP.Rules) > 0 {
			dlpCfg.EnabledRules = cfg.Layers.DLP.Rules
		}
		if dlpCfg.ActionOnMatch == "" {
			dlpCfg.ActionOnMatch = "block"
		}
		p.dlpEngine = dlp.NewDLPEngine(dlpCfg)
		logger.InfoC("security", "DLP engine initialized")
	}

	// Layer 6: SSRF Guard
	if cfg.Layers.SSRF != nil && cfg.Layers.SSRF.Enabled {
		ssrfCfg := ssrf.DefaultConfig()
		ssrfCfg.Enabled = true
		guard, err := ssrf.NewGuard(ssrfCfg)
		if err != nil {
			logger.ErrorCF("security", "Failed to initialize SSRF guard", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			p.ssrfGuard = guard
			logger.InfoC("security", "SSRF guard initialized")
		}
	}

	// Layer 8: Audit Chain
	if cfg.Layers.AuditChain != nil && cfg.Layers.AuditChain.Enabled {
		auditDir := filepath.Join(path.DefaultPathManager().Workspace(), "security", "audit_chain")
		chain, err := integrity.NewAuditChain(integrity.AuditChainConfig{
			Enabled:       true,
			StoragePath:   auditDir,
			MaxFileSize:   50 * 1024 * 1024, // 50MB
			VerifyOnLoad:  false,
		})
		if err != nil {
			logger.ErrorCF("security", "Failed to initialize audit chain", map[string]interface{}{
				"error": err.Error(),
			})
		} else {
			p.auditChain = chain
			logger.InfoC("security", "Audit chain initialized")
		}
	}
}

// extractURL extracts URL from tool arguments for SSRF checking
func (p *SecurityPlugin) extractURL(toolName string, args map[string]interface{}) string {
	switch toolName {
	case "download", "upload", "http_request", "web_request":
		if url, ok := args["url"].(string); ok {
			return url
		}
	}
	return ""
}

// GetInjectionDetector returns the injection detector (for testing)
func (p *SecurityPlugin) GetInjectionDetector() *injection.Detector {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.injectionDetector
}

// GetCommandGuard returns the command guard (for testing)
func (p *SecurityPlugin) GetCommandGuard() *command.Guard {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.commandGuard
}

// GetCredentialScanner returns the credential scanner (for testing)
func (p *SecurityPlugin) GetCredentialScanner() *credential.Scanner {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.credentialScanner
}

// GetDLPEngine returns the DLP engine (for testing)
func (p *SecurityPlugin) GetDLPEngine() *dlp.DLPEngine {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.dlpEngine
}

// GetSSRFGuard returns the SSRF guard (for testing)
func (p *SecurityPlugin) GetSSRFGuard() *ssrf.Guard {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.ssrfGuard
}

// GetAuditChain returns the audit chain (for testing)
func (p *SecurityPlugin) GetAuditChain() *integrity.AuditChain {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.auditChain
}
