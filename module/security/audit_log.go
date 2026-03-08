// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package security provides centralized security controls for dangerous operations

package security

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// initAuditLogFile initializes the audit log file
func (sa *SecurityAuditor) initAuditLogFile() error {
	if sa.config == nil || sa.config.AuditLogDir == "" {
		return fmt.Errorf("audit log directory not configured")
	}

	// Create log directory if it doesn't exist
	if err := os.MkdirAll(sa.config.AuditLogDir, 0755); err != nil {
		return fmt.Errorf("failed to create audit log directory: %w", err)
	}

	// Generate log file name with current date
	dateStr := time.Now().Format("2006-01-02")
	logFileName := fmt.Sprintf("security_audit_%s.log", dateStr)
	sa.logFilePath = filepath.Join(sa.config.AuditLogDir, logFileName)

	// Open log file in append mode, create if not exists
	file, err := os.OpenFile(sa.logFilePath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0640)
	if err != nil {
		return fmt.Errorf("failed to open audit log file: %w", err)
	}

	sa.logFile = file

	// Write header if file is empty
	stat, err := file.Stat()
	if err == nil && stat.Size() == 0 {
		header := "# NemesisBot Security Audit Log\n" +
			"# Format: TIMESTAMP | EVENT_ID | DECISION | OPERATION | USER | SOURCE | TARGET | DANGER | REASON | POLICY\n" +
			"# " + strings.Repeat("=", 150) + "\n"
		if _, err := file.WriteString(header); err != nil {
			return fmt.Errorf("failed to write audit log header: %w", err)
		}
	}

	return nil
}

// logAuditEvent logs an audit event
func (sa *SecurityAuditor) logAuditEvent(event AuditEvent) {
	level := "info"
	if event.Decision == "denied" {
		level = "warn"
	}

	if level == "info" {
		logger.InfoCF("security", "Security audit event", map[string]interface{}{
			"event_id":  event.EventID,
			"decision":  event.Decision,
			"operation": event.Request.Type,
			"user":      event.Request.User,
			"source":    event.Request.Source,
			"target":    event.Request.Target,
			"danger":    event.Request.DangerLevel.String(),
			"reason":    event.Reason,
			"policy":    event.PolicyRule,
		})
	} else {
		logger.WarnCF("security", "Security audit event", map[string]interface{}{
			"event_id":  event.EventID,
			"decision":  event.Decision,
			"operation": event.Request.Type,
			"user":      event.Request.User,
			"source":    event.Request.Source,
			"target":    event.Request.Target,
			"danger":    event.Request.DangerLevel.String(),
			"reason":    event.Reason,
			"policy":    event.PolicyRule,
		})
	}

	// Write to audit log file if enabled
	if sa.config != nil && sa.config.AuditLogFileEnabled && sa.logFile != nil {
		sa.writeAuditLogToFile(event)
	}
}

// writeAuditLogToFile writes an audit event to the log file
func (sa *SecurityAuditor) writeAuditLogToFile(event AuditEvent) {
	if sa.logFile == nil {
		return
	}

	// Format: TIMESTAMP | EVENT_ID | DECISION | OPERATION | USER | SOURCE | TARGET | DANGER | REASON | POLICY
	timestamp := event.Timestamp.Format("2006-01-02 15:04:05.000")
	logLine := fmt.Sprintf("%s | %s | %s | %s | %s | %s | %s | %s | %s | %s\n",
		timestamp,
		event.EventID,
		event.Decision,
		event.Request.Type,
		event.Request.User,
		event.Request.Source,
		sanitizeLogTarget(event.Request.Target),
		event.Request.DangerLevel.String(),
		sanitizeLogReason(event.Reason),
		event.PolicyRule,
	)

	if _, err := sa.logFile.WriteString(logLine); err != nil {
		logger.ErrorCF("security", "Failed to write audit log to file", map[string]interface{}{
			"error": err.Error(),
			"path":  sa.logFilePath,
		})
	}
}

// sanitizeLogTarget sanitizes the target path for log output
func sanitizeLogTarget(target string) string {
	// Replace newlines and tabs with spaces
	target = strings.ReplaceAll(target, "\n", " ")
	target = strings.ReplaceAll(target, "\r", " ")
	target = strings.ReplaceAll(target, "\t", " ")
	// Limit length
	if len(target) > 200 {
		target = target[:200] + "..."
	}
	return target
}

// sanitizeLogReason sanitizes the reason for log output
func sanitizeLogReason(reason string) string {
	// Replace newlines and tabs with spaces
	reason = strings.ReplaceAll(reason, "\n", " ")
	reason = strings.ReplaceAll(reason, "\r", " ")
	reason = strings.ReplaceAll(reason, "\t", " ")
	// Limit length
	if len(reason) > 100 {
		reason = reason[:100] + "..."
	}
	return reason
}

func sanitizeCSV(s string) string {
	s = strings.ReplaceAll(s, "\"", "\"\"")
	if strings.ContainsAny(s, ",\"\n") {
		s = "\"" + s + "\""
	}
	return s
}
