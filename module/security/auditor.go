// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package security provides centralized security controls for dangerous operations

package security

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
	"github.com/276793422/NemesisBot/module/security/approval"
)

// OperationType represents the category of dangerous operation
type OperationType string

const (
	// File operations
	OpFileRead   OperationType = "file_read"
	OpFileWrite  OperationType = "file_write"
	OpFileDelete OperationType = "file_delete"

	// Directory operations
	OpDirRead   OperationType = "dir_read"
	OpDirCreate OperationType = "dir_create"
	OpDirDelete OperationType = "dir_delete"

	// Process operations
	OpProcessExec    OperationType = "process_exec"
	OpProcessSpawn   OperationType = "process_spawn"
	OpProcessKill    OperationType = "process_kill"
	OpProcessSuspend OperationType = "process_suspend"

	// Network operations
	OpNetworkDownload OperationType = "network_download"
	OpNetworkUpload   OperationType = "network_upload"
	OpNetworkRequest  OperationType = "network_request"

	// Hardware operations
	OpHardwareI2C  OperationType = "hardware_i2c"
	OpHardwareSPI  OperationType = "hardware_spi"
	OpHardwareGPIO OperationType = "hardware_gpio"

	// System operations
	OpSystemShutdown OperationType = "system_shutdown"
	OpSystemReboot   OperationType = "system_reboot"
	OpSystemConfig   OperationType = "system_config"
	OpSystemService  OperationType = "system_service"
	OpSystemInstall  OperationType = "system_install"

	// Registry operations (Windows)
	OpRegistryRead   OperationType = "registry_read"
	OpRegistryWrite  OperationType = "registry_write"
	OpRegistryDelete OperationType = "registry_delete"
)

// DangerLevel represents the risk level of an operation
type DangerLevel int

const (
	DangerLow      DangerLevel = iota // Safe operations with minimal risk
	DangerMedium                      // Operations that modify data/state
	DangerHigh                        // Operations that can cause significant damage
	DangerCritical                    // Operations that can compromise system security
)

func (d DangerLevel) String() string {
	switch d {
	case DangerLow:
		return "LOW"
	case DangerMedium:
		return "MEDIUM"
	case DangerHigh:
		return "HIGH"
	case DangerCritical:
		return "CRITICAL"
	default:
		return "UNKNOWN"
	}
}

// OperationRequest represents a request for a dangerous operation
type OperationRequest struct {
	ID           string                 // Unique request ID
	Type         OperationType          // Type of operation
	DangerLevel  DangerLevel            // Risk level
	User         string                 // User/agent requesting the operation
	Source       string                 // Source (cli, web, telegram, etc.)
	Target       string                 // Target of operation (file path, command, URL, etc.)
	Context      map[string]interface{} // Additional context
	Timestamp    time.Time              // When the request was made
	Approver     string                 // Who approved (if applicable)
	ApprovedAt   time.Time              // When approved
	DeniedReason string                 // Reason for denial (if denied)
	AuditLog     string                 // Audit trail entry
}

// Permission defines what operations are allowed
type Permission struct {
	AllowedTypes    map[OperationType]bool
	AllowedTargets  []string               // Whitelist patterns (regexp)
	DeniedTargets   []string               // Blacklist patterns (regexp)
	RequireApproval map[OperationType]bool // Ops requiring explicit approval
	MaxDangerLevel  DangerLevel            // Maximum danger level allowed
}

// AuditEvent represents an audit log entry
type AuditEvent struct {
	EventID    string           // Unique event ID
	Request    OperationRequest // Original request
	Decision   string           // "allowed", "denied", "approved", "pending"
	Reason     string           // Reason for decision
	Timestamp  time.Time        // When the decision was made
	Duration   time.Duration    // Time to process
	PolicyRule string           // Which policy rule matched
}

// Policy defines security rules
type Policy struct {
	Name          string
	Description   string
	Enabled       bool
	Rules         []PolicyRule
	DefaultAction string // "allow", "deny", "require_approval"
	LogOnly       bool   // If true, log but don't block
	RequireMFA    bool   // Require multi-factor approval for critical ops
}

// PolicyRule defines a single security rule
type PolicyRule struct {
	Name        string
	MatchOpType OperationType
	MatchTarget string      // Regexp pattern for target
	MatchUser   string      // Regexp pattern for user
	MatchSource string      // Regexp pattern for source
	MinDanger   DangerLevel // Minimum danger level to match
	Action      string      // "allow", "deny", "require_approval"
	Reason      string      // Explanation for this rule
}

// SecurityAuditor is the main security auditor
type SecurityAuditor struct {
	rules          map[OperationType][]config.SecurityRule
	defaultAction  string
	activeRequests map[string]*OperationRequest // Pending approval requests
	auditLog       []AuditEvent
	mu             sync.RWMutex
	config         *AuditorConfig
	enabled        bool
	logFile        *os.File
	logFilePath    string
	approvalMgr    approval.ApprovalManager // Approval dialog manager
}

// AuditorConfig configures the security auditor
type AuditorConfig struct {
	Enabled               bool
	LogAllOperations      bool
	LogDenialsOnly        bool
	ApprovalTimeout       time.Duration
	MaxPendingRequests    int
	AuditLogRetentionDays int
	AuditLogPath          string
	AuditLogFileEnabled   bool
	AuditLogDir           string
	SynchronousMode       bool
	DefaultAction         string
}

// DefaultDenyPatterns defines default dangerous patterns to block
var DefaultDenyPatterns = map[OperationType][]string{
	OpProcessExec: {
		`\brm\s+-[rf]{1,2}\b`,
		`\bdel\s+/[fq]\b`,
		`\b(format|mkfs|diskpart)\b`,
		`\bdd\s+if=`,
		`\b(shutdown|reboot|poweroff)\b`,
		`\bsudo\b`,
		`\bchmod\s+[0-7]{3,4}\b`,
		`\bchown\b`,
		`\bpkill\b`,
		`\bkillall\b`,
		`\bkill\s+-[9]\b`,
		`\bcurl\b.*\|\s*(sh|bash)`,
		`\bwget\b.*\|\s*(sh|bash)`,
		`\beval\b`,
		`\bsource\s+.*\.sh\b`,
	},
	OpFileWrite: {
		`\.\.[/\\]`, // Path traversal
		`^/etc/`,    // System config
		`^/sys/`,    // System filesystem
		`^/proc/`,   // Process filesystem
		`^/dev/`,    // Device files (except allowed)
		`C:\\Windows\\System32`,
		`C:\\Windows\\System32\\drivers\\etc\\hosts`,
	},
	OpNetworkDownload: {
		`file://`, // Local file access
		`ftp://`,  // Unencrypted FTP
	},
}

// NewSecurityAuditor creates a new security auditor
func NewSecurityAuditor(auditorConfig *AuditorConfig) *SecurityAuditor {
	if auditorConfig == nil {
		auditorConfig = &AuditorConfig{
			Enabled:               true,
			LogAllOperations:      true,
			ApprovalTimeout:       5 * time.Minute,
			MaxPendingRequests:    100,
			AuditLogRetentionDays: 90,
			DefaultAction:         "deny",
			AuditLogFileEnabled:   true,
		}
	}

	sa := &SecurityAuditor{
		rules:          make(map[OperationType][]config.SecurityRule),
		defaultAction:  auditorConfig.DefaultAction,
		activeRequests: make(map[string]*OperationRequest),
		auditLog:       make([]AuditEvent, 0),
		config:         auditorConfig,
		enabled:        auditorConfig.Enabled,
	}

	// Initialize audit log file if enabled
	if auditorConfig.AuditLogFileEnabled && auditorConfig.AuditLogDir != "" {
		if err := sa.initAuditLogFile(); err != nil {
			// Log error but don't fail initialization
			logger.ErrorCF("security", "Failed to initialize audit log file", map[string]interface{}{
				"error": err.Error(),
				"path":  auditorConfig.AuditLogDir,
			})
		}
	}

	return sa
}

// SetRules sets rules for a specific operation type
func (sa *SecurityAuditor) SetRules(opType OperationType, rules []config.SecurityRule) {
	sa.mu.Lock()
	defer sa.mu.Unlock()
	sa.rules[opType] = rules
}

// SetDefaultAction sets the default action for unmatched requests
func (sa *SecurityAuditor) SetDefaultAction(action string) {
	sa.mu.Lock()
	defer sa.mu.Unlock()
	sa.defaultAction = action
}

// SetApprovalManager sets the approval manager for interactive approval dialogs
func (sa *SecurityAuditor) SetApprovalManager(mgr approval.ApprovalManager) {
	sa.mu.Lock()
	defer sa.mu.Unlock()
	sa.approvalMgr = mgr
}

// GetApprovalManager returns the current approval manager
func (sa *SecurityAuditor) GetApprovalManager() approval.ApprovalManager {
	sa.mu.RLock()
	defer sa.mu.RUnlock()
	return sa.approvalMgr
}

// RequestPermission requests permission to perform a dangerous operation
// Returns: (allowed, error, requestID)
func (sa *SecurityAuditor) RequestPermission(ctx context.Context, req *OperationRequest) (bool, error, string) {
	// Auto-generate ID if not provided
	if req.ID == "" {
		req.ID = generateRequestID()
	}
	req.Timestamp = time.Now()

	// If auditor is disabled, allow everything
	if !sa.enabled {
		return true, nil, req.ID
	}

	sa.mu.Lock()
	defer sa.mu.Unlock()

	// Evaluate against policies
	decision, reason, policy := sa.evaluateRequest(req)

	// Create audit event
	event := AuditEvent{
		EventID:    generateEventID(),
		Request:    *req,
		Decision:   decision,
		Reason:     reason,
		Timestamp:  time.Now(),
		PolicyRule: policy,
	}

	// Log the event
	sa.auditLog = append(sa.auditLog, event)
	sa.logAuditEvent(event)

	// Handle decision
	switch decision {
	case "allowed":
		return true, nil, req.ID
	case "denied":
		return false, fmt.Errorf("operation denied: %s", reason), req.ID
	case "require_approval":
		// Try to use interactive approval dialog if available
		if sa.approvalMgr != nil && sa.approvalMgr.IsRunning() {
			// Convert OperationRequest to ApprovalRequest
			approvalReq := &approval.ApprovalRequest{
				RequestID:      req.ID,
				Operation:      string(req.Type),
				Target:         req.Target,
				RiskLevel:      req.DangerLevel.String(),
				Reason:         reason,
				Context:        convertContextToStringMap(req.Context),
				TimeoutSeconds: int(sa.config.ApprovalTimeout.Seconds()),
				Timestamp:      req.Timestamp.Unix(),
			}

			// Request user approval via dialog
			resp, err := sa.approvalMgr.RequestApproval(ctx, approvalReq)
			if err != nil {
				// Dialog failed, fall back to pending request
				sa.activeRequests[req.ID] = req
				return false, &ApprovalRequiredError{RequestID: req.ID, Reason: reason}, req.ID
			}

			// Handle response
			if resp.Approved {
				// User approved the operation
				return true, nil, req.ID
			} else if resp.TimedOut {
				// User didn't respond in time
				return false, fmt.Errorf("operation timed out waiting for approval: %s", reason), req.ID
			} else {
				// User explicitly denied
				return false, fmt.Errorf("operation denied by user: %s", reason), req.ID
			}
		}

		// No approval manager available, store as pending request
		sa.activeRequests[req.ID] = req
		return false, &ApprovalRequiredError{RequestID: req.ID, Reason: reason}, req.ID
	default:
		return false, fmt.Errorf("unknown decision: %s", decision), req.ID
	}
}

// convertContextToStringMap converts map[string]interface{} to map[string]string
func convertContextToStringMap(ctx map[string]interface{}) map[string]string {
	result := make(map[string]string)
	for k, v := range ctx {
		result[k] = fmt.Sprintf("%v", v)
	}
	return result
}

// ApproveRequest approves a pending operation request
func (sa *SecurityAuditor) ApproveRequest(requestID, approver string) error {
	sa.mu.Lock()
	defer sa.mu.Unlock()

	req, exists := sa.activeRequests[requestID]
	if !exists {
		return fmt.Errorf("request not found: %s", requestID)
	}

	req.Approver = approver
	req.ApprovedAt = time.Now()

	// Update audit log
	for i, event := range sa.auditLog {
		if event.Request.ID == requestID {
			sa.auditLog[i].Decision = "approved"
			sa.auditLog[i].Reason = fmt.Sprintf("Approved by %s", approver)
			break
		}
	}

	delete(sa.activeRequests, requestID)

	logger.InfoCF("security", "Operation approved", map[string]interface{}{
		"request_id": requestID,
		"approver":   approver,
		"operation":  req.Type,
		"target":     req.Target,
	})

	return nil
}

// DenyRequest denies a pending operation request
func (sa *SecurityAuditor) DenyRequest(requestID, approver, reason string) error {
	sa.mu.Lock()
	defer sa.mu.Unlock()

	req, exists := sa.activeRequests[requestID]
	if !exists {
		return fmt.Errorf("request not found: %s", requestID)
	}

	req.DeniedReason = reason

	// Update audit log
	for i, event := range sa.auditLog {
		if event.Request.ID == requestID {
			sa.auditLog[i].Decision = "denied"
			sa.auditLog[i].Reason = fmt.Sprintf("Denied by %s: %s", approver, reason)
			break
		}
	}

	delete(sa.activeRequests, requestID)

	logger.InfoCF("security", "Operation denied", map[string]interface{}{
		"request_id": requestID,
		"approver":   approver,
		"reason":     reason,
		"operation":  req.Type,
	})

	return nil
}

// GetAuditLog returns the audit log
func (sa *SecurityAuditor) GetAuditLog(filter AuditFilter) []AuditEvent {
	sa.mu.RLock()
	defer sa.mu.RUnlock()

	if filter.IsEmpty() {
		return sa.auditLog
	}

	var result []AuditEvent
	for _, event := range sa.auditLog {
		if filter.Matches(event) {
			result = append(result, event)
		}
	}
	return result
}

// GetPendingRequests returns all pending approval requests
func (sa *SecurityAuditor) GetPendingRequests() []*OperationRequest {
	sa.mu.RLock()
	defer sa.mu.RUnlock()

	result := make([]*OperationRequest, 0, len(sa.activeRequests))
	for _, req := range sa.activeRequests {
		result = append(result, req)
	}
	return result
}

// evaluateRequest evaluates a request against rules using sequential matching
func (sa *SecurityAuditor) evaluateRequest(req *OperationRequest) (decision, reason, matchedRule string) {
	// Get rules for this operation type
	rules, exists := sa.rules[req.Type]
	if !exists || len(rules) == 0 {
		// No rules configured, use default action
		return normalizeDecision(sa.defaultAction), "no rules configured, using default action", "default"
	}

	// Sequential rule matching - first match wins
	for i, rule := range rules {
		var matched bool

		// Choose matching function based on operation type
		switch req.Type {
		case OpFileRead, OpFileWrite, OpFileDelete:
			matched = MatchPattern(rule.Pattern, req.Target)
		case OpDirRead, OpDirCreate, OpDirDelete:
			matched = MatchPattern(rule.Pattern, req.Target)
		case OpProcessExec, OpProcessSpawn, OpProcessKill, OpProcessSuspend:
			matched = MatchCommandPattern(rule.Pattern, req.Target)
		case OpNetworkDownload, OpNetworkUpload, OpNetworkRequest:
			matched = MatchDomainPattern(rule.Pattern, req.Target)
		case OpHardwareI2C, OpHardwareSPI, OpHardwareGPIO:
			// Hardware operations usually don't have targets, match by pattern
			matched = rule.Pattern == "*" || MatchPattern(rule.Pattern, req.Target)
		case OpRegistryRead, OpRegistryWrite, OpRegistryDelete:
			matched = MatchPattern(rule.Pattern, req.Target)
		default:
			// Default to pattern matching
			matched = MatchPattern(rule.Pattern, req.Target)
		}

		if matched {
			reason := fmt.Sprintf("rule matched: pattern=%s", rule.Pattern)
			return normalizeDecision(rule.Action), reason, fmt.Sprintf("rule[%d]", i)
		}
	}

	// No rules matched, use default action
	return normalizeDecision(sa.defaultAction), "no rules matched, using default action", "default"
}

// normalizeDecision converts action values to canonical decision names
//
// "ask" action is mapped to "require_approval" to trigger the approval dialog.
func normalizeDecision(action string) string {
	switch action {
	case "allow", "allowed":
		return "allowed"
	case "deny", "denied":
		return "denied"
	case "ask":
		// Map ask to require_approval to trigger approval dialog
		return "require_approval"
	case "require_approval":
		return "require_approval"
	default:
		return action
	}
}

// ApprovalRequiredError is returned when approval is required
type ApprovalRequiredError struct {
	RequestID string
	Reason    string
}

func (e *ApprovalRequiredError) Error() string {
	return fmt.Sprintf("approval required: %s (request ID: %s)", e.Reason, e.RequestID)
}

func (e *ApprovalRequiredError) IsApprovalRequired() bool {
	return true
}

// Utility functions
func generateRequestID() string {
	return fmt.Sprintf("req-%d", time.Now().UnixNano())
}

func generateEventID() string {
	return fmt.Sprintf("evt-%d", time.Now().UnixNano())
}

// AuditFilter filters audit logs
type AuditFilter struct {
	OperationType OperationType
	User          string
	Source        string
	Decision      string
	StartTime     *time.Time
	EndTime       *time.Time
}

func (f *AuditFilter) IsEmpty() bool {
	return f.OperationType == "" &&
		f.User == "" &&
		f.Source == "" &&
		f.Decision == "" &&
		f.StartTime == nil &&
		f.EndTime == nil
}

func (f *AuditFilter) Matches(event AuditEvent) bool {
	if f.OperationType != "" && event.Request.Type != f.OperationType {
		return false
	}
	if f.User != "" && event.Request.User != f.User {
		return false
	}
	if f.Source != "" && event.Request.Source != "" {
		matched, _ := regexp.MatchString(f.Source, event.Request.Source)
		if !matched {
			return false
		}
	}
	if f.Decision != "" && event.Decision != f.Decision {
		return false
	}
	if f.StartTime != nil && event.Timestamp.Before(*f.StartTime) {
		return false
	}
	if f.EndTime != nil && event.Timestamp.After(*f.EndTime) {
		return false
	}
	return true
}

// GetDangerLevel returns the danger level for an operation type
func GetDangerLevel(opType OperationType) DangerLevel {
	switch opType {
	case OpFileRead, OpDirRead:
		return DangerLow
	case OpNetworkDownload, OpNetworkRequest:
		return DangerMedium
	case OpFileWrite, OpFileDelete, OpDirCreate, OpDirDelete, OpProcessSpawn:
		return DangerHigh
	case OpProcessExec, OpProcessKill, OpSystemShutdown, OpSystemReboot,
		OpSystemConfig, OpSystemService, OpSystemInstall,
		OpRegistryWrite, OpRegistryDelete:
		return DangerCritical
	default:
		return DangerMedium
	}
}

// ValidatePath checks if a path is safe for the given operation
func ValidatePath(path, workspace string, operation OperationType) (string, error) {
	// Convert to absolute path
	absPath, err := filepath.Abs(path)
	if err != nil {
		return "", fmt.Errorf("invalid path: %w", err)
	}

	// If workspace is specified, check if path is within workspace
	if workspace != "" {
		absWorkspace, err := filepath.Abs(workspace)
		if err != nil {
			return "", fmt.Errorf("invalid workspace: %w", err)
		}

		rel, err := filepath.Rel(absWorkspace, absPath)
		if err != nil || strings.HasPrefix(rel, "..") {
			return "", fmt.Errorf("access denied: path outside workspace")
		}
	}

	// Check for dangerous system paths
	dangerousPaths := []string{
		"/etc/passwd", "/etc/shadow", "/etc/sudoers",
		"C:\\Windows\\System32\\drivers\\etc\\hosts",
	}
	for _, dangerous := range dangerousPaths {
		// Check if the target path is a dangerous system path
		// Use exact match or prefix match with path separator
		if strings.HasPrefix(absPath, dangerous) {
			return "", fmt.Errorf("access denied: protected system path")
		}
	}

	return absPath, nil
}

// IsSafeCommand checks if a command is safe to execute
func IsSafeCommand(command string) (bool, string) {
	dangerousPatterns := []string{
		`\brm\s+-[rf]{1,2}\b`,
		`\bdel\s+/[fq]\b`,
		`\b(format|mkfs)\b`,
		`\bdd\s+if=`,
		`\b(shutdown|reboot|poweroff)\b`,
		`\bsudo\b`,
		`\bchmod\s+[0-7]{3,4}\b`,
		`\bchown\b`,
	}

	cmdLower := strings.ToLower(command)
	for _, pattern := range dangerousPatterns {
		matched, _ := regexp.MatchString(pattern, cmdLower)
		if matched {
			return false, "command contains dangerous pattern"
		}
	}

	return true, ""
}

// CleanupOldAuditLogs removes audit logs older than retention period
func (sa *SecurityAuditor) CleanupOldAuditLogs() error {
	sa.mu.Lock()
	defer sa.mu.Unlock()

	if sa.config.AuditLogRetentionDays <= 0 {
		return nil
	}

	cutoff := time.Now().AddDate(0, 0, -sa.config.AuditLogRetentionDays)
	var newLog []AuditEvent

	for _, event := range sa.auditLog {
		if event.Timestamp.After(cutoff) {
			newLog = append(newLog, event)
		}
	}

	removed := len(sa.auditLog) - len(newLog)
	sa.auditLog = newLog

	if removed > 0 {
		logger.InfoCF("security", "Cleaned up old audit logs", map[string]interface{}{
			"removed":   removed,
			"remaining": len(sa.auditLog),
		})
	}

	return nil
}

// GetStatistics returns security statistics
func (sa *SecurityAuditor) GetStatistics() map[string]interface{} {
	sa.mu.RLock()
	defer sa.mu.RUnlock()

	stats := map[string]interface{}{
		"total_events":     len(sa.auditLog),
		"pending_requests": len(sa.activeRequests),
		"enabled":          sa.enabled,
		"rule_types":       len(sa.rules),
	}

	decisionCounts := make(map[string]int)
	for _, event := range sa.auditLog {
		decisionCounts[event.Decision]++
	}
	stats["decision_counts"] = decisionCounts

	return stats
}

// ExportAuditLog exports audit log to a file
func (sa *SecurityAuditor) ExportAuditLog(filePath string) error {
	sa.mu.RLock()
	defer sa.mu.RUnlock()

	if err := os.MkdirAll(filepath.Dir(filePath), 0755); err != nil {
		return err
	}

	// Simple CSV export
	f, err := os.Create(filePath)
	if err != nil {
		return err
	}
	defer f.Close()

	// Write header
	f.WriteString("event_id,timestamp,decision,operation,user,source,target,danger,reason\n")

	// Write events
	for _, event := range sa.auditLog {
		line := fmt.Sprintf("%s,%s,%s,%s,%s,%s,%s,%s,%s\n",
			event.EventID,
			event.Timestamp.Format(time.RFC3339),
			event.Decision,
			event.Request.Type,
			event.Request.User,
			event.Request.Source,
			sanitizeCSV(event.Request.Target),
			event.Request.DangerLevel.String(),
			sanitizeCSV(event.Reason),
		)
		f.WriteString(line)
	}

	logger.InfoCF("security", "Audit log exported", map[string]interface{}{
		"path":    filePath,
		"entries": len(sa.auditLog),
	})

	return nil
}

// Enable enables the security auditor
func (sa *SecurityAuditor) Enable() {
	sa.mu.Lock()
	defer sa.mu.Unlock()
	sa.enabled = true
	logger.InfoC("security", "Security auditor enabled")
}

// Disable disables the security auditor (use with caution!)
func (sa *SecurityAuditor) Disable() {
	sa.mu.Lock()
	defer sa.mu.Unlock()
	sa.enabled = false
	logger.WarnC("security", "Security auditor DISABLED - all operations will be allowed!")
}

// Close closes the audit log file
func (sa *SecurityAuditor) Close() error {
	sa.mu.Lock()
	defer sa.mu.Unlock()

	if sa.logFile != nil {
		if err := sa.logFile.Close(); err != nil {
			return err
		}
		sa.logFile = nil
	}
	return nil
}
