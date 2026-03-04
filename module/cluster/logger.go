// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package cluster

import (
	"fmt"
	"log"
	"os"
	"path/filepath"
	"sync"
)

// ClusterLogger manages logging for the cluster module
type ClusterLogger struct {
	discoveryLogger *logger
	rpcLogger       *logger
	mu              sync.Mutex
}

type logger struct {
	file   *os.File
	logger *log.Logger
}

// NewClusterLogger creates a new cluster logger
func NewClusterLogger(workspace string) (*ClusterLogger, error) {
	// Create log directory
	logDir := filepath.Join(workspace, "logs", "cluster")
	if err := os.MkdirAll(logDir, 0755); err != nil {
		return nil, fmt.Errorf("failed to create log directory: %w", err)
	}

	// Create discovery logger
	discoveryLog := filepath.Join(logDir, "discovery.log")
	discoveryLogger, err := newLogger(discoveryLog)
	if err != nil {
		return nil, fmt.Errorf("failed to create discovery logger: %w", err)
	}

	// Create RPC logger
	rpcLog := filepath.Join(logDir, "rpc.log")
	rpcLogger, err := newLogger(rpcLog)
	if err != nil {
		return nil, fmt.Errorf("failed to create RPC logger: %w", err)
	}

	return &ClusterLogger{
		discoveryLogger: discoveryLogger,
		rpcLogger:       rpcLogger,
	}, nil
}

func newLogger(path string) (*logger, error) {
	file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0644)
	if err != nil {
		return nil, err
	}

	return &logger{
		file:   file,
		logger: log.New(file, "", 0),
	}, nil
}

// Close closes all log files
func (l *ClusterLogger) Close() error {
	l.mu.Lock()
	defer l.mu.Unlock()

	var errs []error

	if err := l.discoveryLogger.file.Close(); err != nil {
		errs = append(errs, err)
	}

	if err := l.rpcLogger.file.Close(); err != nil {
		errs = append(errs, err)
	}

	if len(errs) > 0 {
		return fmt.Errorf("errors closing loggers: %v", errs)
	}

	return nil
}

// Discovery logging methods
func (l *ClusterLogger) DiscoveryInfo(format string, args ...interface{}) {
	l.discoveryLogger.logger.Printf("[INFO] "+format, args...)
}

func (l *ClusterLogger) DiscoveryError(format string, args ...interface{}) {
	l.discoveryLogger.logger.Printf("[ERROR] "+format, args...)
}

func (l *ClusterLogger) DiscoveryDebug(format string, args ...interface{}) {
	l.discoveryLogger.logger.Printf("[DEBUG] "+format, args...)
}

// RPC logging methods
func (l *ClusterLogger) RPCInfo(format string, args ...interface{}) {
	l.rpcLogger.logger.Printf("[INFO] "+format+"\n", args...)
	l.rpcLogger.file.Sync() // Force flush
}

func (l *ClusterLogger) RPCError(format string, args ...interface{}) {
	l.rpcLogger.logger.Printf("[ERROR] "+format+"\n", args...)
	l.rpcLogger.file.Sync() // Force flush
}

func (l *ClusterLogger) RPCDebug(format string, args ...interface{}) {
	l.rpcLogger.logger.Printf("[DEBUG] "+format+"\n", args...)
	l.rpcLogger.file.Sync() // Force flush
}

// LogRPCInfo logs an RPC info message (aliases for handlers.Logger interface)
func (l *ClusterLogger) LogRPCInfo(msg string, args ...interface{}) {
	l.RPCInfo(msg, args...)
}

// LogRPCError logs an RPC error message (aliases for handlers.Logger interface)
func (l *ClusterLogger) LogRPCError(msg string, args ...interface{}) {
	l.RPCError(msg, args...)
}

// LogRPCDebug logs an RPC debug message (aliases for handlers.Logger interface)
func (l *ClusterLogger) LogRPCDebug(msg string, args ...interface{}) {
	l.RPCDebug(msg, args...)
}

// log.Logger interface (minimal implementation)
type logLogger struct {
	logger *log.Logger
}

func newLogLogger(out *os.File) *logLogger {
	return &logLogger{
		logger: log.New(out, "CLUSTER: ", log.LstdFlags|log.Lmicroseconds),
	}
}

func (l *logLogger) Printf(format string, args ...interface{}) {
	l.logger.Printf(format, args...)
}
