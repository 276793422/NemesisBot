// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package security provides middleware for wrapping dangerous tools

package security

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// Global security auditor instance
var globalAuditor *SecurityAuditor
var auditorOnce sync.Once

// InitGlobalAuditor initializes the global security auditor
func InitGlobalAuditor(config *AuditorConfig) *SecurityAuditor {
	auditorOnce.Do(func() {
		globalAuditor = NewSecurityAuditor(config)
	})
	return globalAuditor
}

// GetGlobalAuditor returns the global security auditor
func GetGlobalAuditor() *SecurityAuditor {
	if globalAuditor == nil {
		return InitGlobalAuditor(nil)
	}
	return globalAuditor
}

// SecureFileWrapper wraps file operations with security checks
type SecureFileWrapper struct {
	auditor  *SecurityAuditor
	user     string
	source   string
	workspace string
}

func NewSecureFileWrapper(auditor *SecurityAuditor, user, source, workspace string) *SecureFileWrapper {
	return &SecureFileWrapper{
		auditor:  auditor,
		user:     user,
		source:   source,
		workspace: workspace,
	}
}

// ReadFile reads a file with security check
func (w *SecureFileWrapper) ReadFile(path string) ([]byte, error) {
	// Validate path
	validPath, err := ValidatePath(path, w.workspace, OpFileRead)
	if err != nil {
		return nil, err
	}

	// Request permission
	req := &OperationRequest{
		Type:        OpFileRead,
		DangerLevel: GetDangerLevel(OpFileRead),
		User:        w.user,
		Source:      w.source,
		Target:      validPath,
		Context: map[string]interface{}{
			"workspace": w.workspace,
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return nil, fmt.Errorf("permission denied for read operation on: %s", path)
	}

	// Perform the operation
	return os.ReadFile(validPath)
}

// WriteFile writes a file with security check
func (w *SecureFileWrapper) WriteFile(path string, content []byte) error {
	// Validate path
	validPath, err := ValidatePath(path, w.workspace, OpFileWrite)
	if err != nil {
		return err
	}

	// Create directory if needed
	dir := filepath.Dir(validPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	// Request permission
	req := &OperationRequest{
		Type:        OpFileWrite,
		DangerLevel: GetDangerLevel(OpFileWrite),
		User:        w.user,
		Source:      w.source,
		Target:      validPath,
		Context: map[string]interface{}{
			"workspace": w.workspace,
			"size":      len(content),
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return fmt.Errorf("permission denied for write operation on: %s", path)
	}

	// Perform the operation
	return os.WriteFile(validPath, content, 0644)
}

// EditFile edits a file with security check
func (w *SecureFileWrapper) EditFile(path, oldText, newText string) error {
	// Validate path
	validPath, err := ValidatePath(path, w.workspace, OpFileWrite)
	if err != nil {
		return err
	}

	// Read current content
	content, err := os.ReadFile(validPath)
	if err != nil {
		return err
	}

	contentStr := string(content)
	if !strings.Contains(contentStr, oldText) {
		return fmt.Errorf("old_text not found in file")
	}

	newContent := strings.Replace(contentStr, oldText, newText, 1)

	// Request permission
	req := &OperationRequest{
		Type:        OpFileWrite,
		DangerLevel: GetDangerLevel(OpFileWrite),
		User:        w.user,
		Source:      w.source,
		Target:      validPath,
		Context: map[string]interface{}{
			"workspace": w.workspace,
			"old_size":  len(oldText),
			"new_size":  len(newText),
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return fmt.Errorf("permission denied for edit operation on: %s", path)
	}

	return os.WriteFile(validPath, []byte(newContent), 0644)
}

// AppendFile appends to a file with security check
func (w *SecureFileWrapper) AppendFile(path string, content []byte) error {
	// Validate path
	validPath, err := ValidatePath(path, w.workspace, OpFileWrite)
	if err != nil {
		return err
	}

	// Request permission
	req := &OperationRequest{
		Type:        OpFileWrite,
		DangerLevel: GetDangerLevel(OpFileWrite),
		User:        w.user,
		Source:      w.source,
		Target:      validPath,
		Context: map[string]interface{}{
			"workspace": w.workspace,
			"size":      len(content),
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return fmt.Errorf("permission denied for append operation on: %s", path)
	}

	// Perform the operation
	f, err := os.OpenFile(validPath, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0644)
	if err != nil {
		return err
	}
	defer f.Close()

	_, err = f.Write(content)
	return err
}

// DeleteFile deletes a file with security check
func (w *SecureFileWrapper) DeleteFile(path string) error {
	// Validate path
	validPath, err := ValidatePath(path, w.workspace, OpFileDelete)
	if err != nil {
		return err
	}

	// Request permission
	req := &OperationRequest{
		Type:        OpFileDelete,
		DangerLevel: GetDangerLevel(OpFileDelete),
		User:        w.user,
		Source:      w.source,
		Target:      validPath,
		Context: map[string]interface{}{
			"workspace": w.workspace,
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return fmt.Errorf("permission denied for delete operation on: %s", path)
	}

	return os.Remove(validPath)
}

// ReadDirectory reads a directory with security check
func (w *SecureFileWrapper) ReadDirectory(path string) ([]string, error) {
	// Validate path
	validPath, err := ValidatePath(path, w.workspace, OpDirRead)
	if err != nil {
		return nil, err
	}

	// Request permission
	req := &OperationRequest{
		Type:        OpDirRead,
		DangerLevel: GetDangerLevel(OpDirRead),
		User:        w.user,
		Source:      w.source,
		Target:      validPath,
		Context: map[string]interface{}{
			"workspace": w.workspace,
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return nil, fmt.Errorf("permission denied for read operation on: %s", path)
	}

	// Perform the operation
	entries, err := os.ReadDir(validPath)
	if err != nil {
		return nil, err
	}

	result := make([]string, 0, len(entries))
	for _, entry := range entries {
		result = append(result, entry.Name())
	}
	return result, nil
}

// CreateDirectory creates a directory with security check
func (w *SecureFileWrapper) CreateDirectory(path string) error {
	// Validate path
	validPath, err := ValidatePath(path, w.workspace, OpDirCreate)
	if err != nil {
		return err
	}

	// Request permission
	req := &OperationRequest{
		Type:        OpDirCreate,
		DangerLevel: GetDangerLevel(OpDirCreate),
		User:        w.user,
		Source:      w.source,
		Target:      validPath,
		Context: map[string]interface{}{
			"workspace": w.workspace,
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return fmt.Errorf("permission denied for create directory operation on: %s", path)
	}

	return os.MkdirAll(validPath, 0755)
}

// DeleteDirectory deletes a directory with security check
func (w *SecureFileWrapper) DeleteDirectory(path string) error {
	// Validate path
	validPath, err := ValidatePath(path, w.workspace, OpDirDelete)
	if err != nil {
		return err
	}

	// Request permission
	req := &OperationRequest{
		Type:        OpDirDelete,
		DangerLevel: GetDangerLevel(OpDirDelete),
		User:        w.user,
		Source:      w.source,
		Target:      validPath,
		Context: map[string]interface{}{
			"workspace": w.workspace,
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return fmt.Errorf("permission denied for delete directory operation on: %s", path)
	}

	return os.RemoveAll(validPath)
}

// SecureProcessWrapper wraps process operations with security checks
type SecureProcessWrapper struct {
	auditor    *SecurityAuditor
	user       string
	source     string
	workingDir string
	timeout    int // seconds
}

func NewSecureProcessWrapper(auditor *SecurityAuditor, user, source, workingDir string) *SecureProcessWrapper {
	return &SecureProcessWrapper{
		auditor:    auditor,
		user:       user,
		source:     source,
		workingDir: workingDir,
		timeout:    60,
	}
}

// ExecuteCommand executes a command with security check
func (w *SecureProcessWrapper) ExecuteCommand(command string) (string, error) {
	// Check if command is safe
	safe, reason := IsSafeCommand(command)
	if !safe {
		return "", fmt.Errorf("command blocked: %s", reason)
	}

	// Request permission
	req := &OperationRequest{
		Type:        OpProcessExec,
		DangerLevel: GetDangerLevel(OpProcessExec),
		User:        w.user,
		Source:      w.source,
		Target:      command,
		Context: map[string]interface{}{
			"working_dir": w.workingDir,
			"timeout":     w.timeout,
		},
	}

	allowed, _, requestID := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return "", fmt.Errorf("permission denied for command execution (request ID: %s)", requestID)
	}

	// Perform the operation (this would call the actual exec tool)
	// For now, return a placeholder
	return fmt.Sprintf("Command execution would happen here: %s", command), nil
}

// SecureNetworkWrapper wraps network operations with security checks
type SecureNetworkWrapper struct {
	auditor *SecurityAuditor
	user    string
	source  string
}

func NewSecureNetworkWrapper(auditor *SecurityAuditor, user, source string) *SecureNetworkWrapper {
	return &SecureNetworkWrapper{
		auditor: auditor,
		user:    user,
		source:  source,
	}
}

// DownloadURL downloads a file from URL with security check
func (w *SecureNetworkWrapper) DownloadURL(url string, savePath string) error {
	// Check URL scheme
	if !strings.HasPrefix(url, "http://") && !strings.HasPrefix(url, "https://") {
		return fmt.Errorf("only http/https URLs are allowed")
	}

	// Request permission
	req := &OperationRequest{
		Type:        OpNetworkDownload,
		DangerLevel: GetDangerLevel(OpNetworkDownload),
		User:        w.user,
		Source:      w.source,
		Target:      url,
		Context: map[string]interface{}{
			"save_path": savePath,
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return fmt.Errorf("permission denied for download from: %s", url)
	}

	// Perform the operation (placeholder)
	logger.InfoCF("security", "Download approved", map[string]interface{}{
		"url":       url,
		"save_path": savePath,
		"user":      w.user,
	})

	return nil
}

// SecureHardwareWrapper wraps hardware operations with security checks
type SecureHardwareWrapper struct {
	auditor *SecurityAuditor
	user    string
	source  string
}

func NewSecureHardwareWrapper(auditor *SecurityAuditor, user, source string) *SecureHardwareWrapper {
	return &SecureHardwareWrapper{
		auditor: auditor,
		user:    user,
		source:  source,
	}
}

// I2CWrite performs I2C write with security check
func (w *SecureHardwareWrapper) I2CWrite(bus string, address int, data []byte) error {
	req := &OperationRequest{
		Type:        OpHardwareI2C,
		DangerLevel: GetDangerLevel(OpHardwareI2C),
		User:        w.user,
		Source:      w.source,
		Target:      fmt.Sprintf("i2c-%s:0x%x", bus, address),
		Context: map[string]interface{}{
			"bus":     bus,
			"address": address,
			"data":    data,
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return fmt.Errorf("permission denied for I2C write")
	}

	return nil
}

// SPIWrite performs SPI write with security check
func (w *SecureHardwareWrapper) SPIWrite(device string, data []byte) error {
	req := &OperationRequest{
		Type:        OpHardwareSPI,
		DangerLevel: GetDangerLevel(OpHardwareSPI),
		User:        w.user,
		Source:      w.source,
		Target:      fmt.Sprintf("spidev%s", device),
		Context: map[string]interface{}{
			"device": device,
			"data":   data,
		},
	}

	allowed, _, _ := w.auditor.RequestPermission(context.Background(), req)
	if !allowed {
		return fmt.Errorf("permission denied for SPI write")
	}

	return nil
}

// SecurityMiddleware provides a unified security interface for tools
type SecurityMiddleware struct {
	auditor  *SecurityAuditor
	user     string
	source   string
	workspace string
}

func NewSecurityMiddleware(auditor *SecurityAuditor, user, source, workspace string) *SecurityMiddleware {
	return &SecurityMiddleware{
		auditor:  auditor,
		user:     user,
		source:   source,
		workspace: workspace,
	}
}

// File returns the file operations wrapper
func (sm *SecurityMiddleware) File() *SecureFileWrapper {
	return NewSecureFileWrapper(sm.auditor, sm.user, sm.source, sm.workspace)
}

// Process returns the process operations wrapper
func (sm *SecurityMiddleware) Process() *SecureProcessWrapper {
	return NewSecureProcessWrapper(sm.auditor, sm.user, sm.source, sm.workspace)
}

// Network returns the network operations wrapper
func (sm *SecurityMiddleware) Network() *SecureNetworkWrapper {
	return NewSecureNetworkWrapper(sm.auditor, sm.user, sm.source)
}

// Hardware returns the hardware operations wrapper
func (sm *SecurityMiddleware) Hardware() *SecureHardwareWrapper {
	return NewSecureHardwareWrapper(sm.auditor, sm.user, sm.source)
}

// BatchOperationRequest represents a batch of operations to be approved together
type BatchOperationRequest struct {
	ID          string
	Operations  []*OperationRequest
	User        string
	Source      string
	Description string
}

// RequestBatchPermission requests permission for multiple operations at once
func (sm *SecurityMiddleware) RequestBatchPermission(ctx context.Context, batch *BatchOperationRequest) (bool, error, string) {
	if len(batch.Operations) == 0 {
		return false, fmt.Errorf("no operations in batch"), ""
	}

	// Use the highest danger level from all operations
	maxDanger := DangerLow
	for _, op := range batch.Operations {
		if op.DangerLevel > maxDanger {
			maxDanger = op.DangerLevel
		}
		op.User = sm.user
		op.Source = sm.source
	}

	// Create a summary request for the batch
	summaryReq := &OperationRequest{
		ID:          batch.ID,
		Type:        "batch_operation",
		DangerLevel: maxDanger,
		User:        sm.user,
		Source:      sm.source,
		Target:      fmt.Sprintf("%d operations", len(batch.Operations)),
		Context: map[string]interface{}{
			"operations":  batch.Operations,
			"description": batch.Description,
		},
	}

	// Request permission for the batch
	allowed, err, requestID := sm.auditor.RequestPermission(ctx, summaryReq)
	if !allowed {
		return false, err, requestID
	}

	// If batch is approved, approve all individual operations silently
	for _, op := range batch.Operations {
		sm.auditor.RequestPermission(ctx, op)
	}

	return true, nil, requestID
}

// GetSecuritySummary returns a summary of security status
func (sm *SecurityMiddleware) GetSecuritySummary() map[string]interface{} {
	stats := sm.auditor.GetStatistics()
	pending := sm.auditor.GetPendingRequests()

	summary := map[string]interface{}{
		"statistics":      stats,
		"pending_requests": len(pending),
		"user":            sm.user,
		"source":          sm.source,
		"workspace":       sm.workspace,
	}

	// Add pending request summaries
	pendingSummaries := make([]map[string]interface{}, 0, len(pending))
	for _, req := range pending {
		pendingSummaries = append(pendingSummaries, map[string]interface{}{
			"id":          req.ID,
			"type":        req.Type,
			"target":      req.Target,
			"danger":      req.DangerLevel.String(),
			"timestamp":   req.Timestamp,
		})
	}
	summary["pending"] = pendingSummaries

	return summary
}

// ApprovePendingRequest approves a pending request (for user interaction)
func (sm *SecurityMiddleware) ApprovePendingRequest(requestID string) error {
	return sm.auditor.ApproveRequest(requestID, sm.user)
}

// DenyPendingRequest denies a pending request (for user interaction)
func (sm *SecurityMiddleware) DenyPendingRequest(requestID, reason string) error {
	return sm.auditor.DenyRequest(requestID, sm.user, reason)
}

// GetAuditLog returns the audit log with optional filtering
func (sm *SecurityMiddleware) GetAuditLog(filter AuditFilter) []AuditEvent {
	return sm.auditor.GetAuditLog(filter)
}

// ExportAuditLog exports the audit log to a file
func (sm *SecurityMiddleware) ExportAuditLog(filePath string) error {
	return sm.auditor.ExportAuditLog(filePath)
}

// CreateCLIPermission creates permission for CLI user (less restrictive)
func CreateCLIPermission() *Permission {
	return &Permission{
		AllowedTypes: map[OperationType]bool{
			OpFileRead:         true,
			OpFileWrite:        true,
			OpFileDelete:       true,
			OpDirRead:          true,
			OpDirCreate:        true,
			OpProcessExec:      true,
			OpNetworkDownload:  true,
			OpNetworkRequest:   true,
		},
		DeniedTargets: []string{
			`^/etc/sudoers`,
			`^/etc/passwd$`,
			`^C:\\Windows\\System32\\drivers\\etc\\hosts`,
		},
		RequireApproval: map[OperationType]bool{
			OpProcessKill:   true,
			OpSystemShutdown: true,
			OpSystemReboot:   true,
		},
		MaxDangerLevel: DangerHigh,
	}
}

// CreateWebPermission creates permission for web users (more restrictive)
func CreateWebPermission() *Permission {
	return &Permission{
		AllowedTypes: map[OperationType]bool{
			OpFileRead:   true,
			OpFileWrite:  true,
			OpDirRead:    true,
			OpDirCreate:  true,
		},
		RequireApproval: map[OperationType]bool{
			OpFileDelete:      true,
			OpProcessExec:     true,
			OpNetworkDownload: true,
		},
		MaxDangerLevel: DangerMedium,
	}
}

// CreateAgentPermission creates permission for AI agents (context-aware)
func CreateAgentPermission(agentID string) *Permission {
	return &Permission{
		AllowedTypes: map[OperationType]bool{
			OpFileRead:        true,
			OpFileWrite:       true,
			OpDirRead:         true,
			OpDirCreate:       true,
			OpProcessExec:     true,
			OpNetworkRequest:  true,
		},
		RequireApproval: map[OperationType]bool{
			OpFileDelete:      true,
			OpProcessKill:     true,
			OpSystemShutdown:  true,
			OpNetworkDownload: true,
		},
		MaxDangerLevel: DangerHigh,
	}
}

// MonitorSecurityStatus continuously monitors and logs security status
func MonitorSecurityStatus(ctx context.Context, auditor *SecurityAuditor, interval time.Duration) {
	ticker := time.NewTicker(interval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			stats := auditor.GetStatistics()
			logger.InfoCF("security", "Security status", stats)

			// Clean up old audit logs periodically
			if err := auditor.CleanupOldAuditLogs(); err != nil {
				logger.ErrorCF("security", "Failed to cleanup audit logs", map[string]interface{}{
					"error": err.Error(),
				})
			}
		}
	}
}
