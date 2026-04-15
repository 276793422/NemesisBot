// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package clamav

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sync"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// DaemonConfig configures the clamd daemon management
type DaemonConfig struct {
	// ClamAVPath is the directory containing ClamAV executables
	ClamAVPath string
	// ConfigFile is the path to clamd.conf
	ConfigFile string
	// DatabaseDir is the directory containing virus definitions
	DatabaseDir string
	// ListenAddr is the TCP address for clamd to listen on (default: 127.0.0.1:3310)
	ListenAddr string
	// TempDir is the temporary directory for ClamAV operations
	TempDir string
	// StartupTimeout is the maximum time to wait for clamd to become ready
	StartupTimeout time.Duration
}

// Daemon manages the clamd daemon lifecycle
type Daemon struct {
	config    *DaemonConfig
	client    *Client
	cmd       *exec.Cmd
	mu        sync.RWMutex
	running   bool
	stopCh    chan struct{}
	readyCh   chan struct{}
	readyOnce sync.Once
}

// NewDaemon creates a new clamd daemon manager
func NewDaemon(cfg *DaemonConfig) *Daemon {
	if cfg.ListenAddr == "" {
		cfg.ListenAddr = "127.0.0.1:3310"
	}
	if cfg.StartupTimeout == 0 {
		cfg.StartupTimeout = 120 * time.Second
	}

	return &Daemon{
		config:  cfg,
		client:  NewClient(cfg.ListenAddr),
		stopCh:  make(chan struct{}),
		readyCh: make(chan struct{}),
	}
}

// Client returns the clamd client for this daemon
func (d *Daemon) Client() *Client {
	return d.client
}

// Start starts the clamd daemon and waits for it to become ready
func (d *Daemon) Start(ctx context.Context) error {
	d.mu.Lock()
	defer d.mu.Unlock()

	if d.running {
		return fmt.Errorf("clamd daemon is already running")
	}

	// Validate ClamAV path
	clamdExe := d.findExecutable("clamd")
	// Resolve to absolute path for exec.CommandContext (Windows requires it)
	if absExe, err := filepath.Abs(clamdExe); err == nil {
		clamdExe = absExe
	}
	if _, err := os.Stat(clamdExe); err != nil {
		return fmt.Errorf("clamd executable not found at %s: %w", clamdExe, err)
	}

	// Ensure config file exists
	if d.config.ConfigFile == "" {
		return fmt.Errorf("clamd config file path is required")
	}
	// Resolve to absolute path for Windows compatibility
	if absCfg, err := filepath.Abs(d.config.ConfigFile); err == nil {
		d.config.ConfigFile = absCfg
	}
	if _, err := os.Stat(d.config.ConfigFile); err != nil {
		return fmt.Errorf("clamd config file not found at %s: %w", d.config.ConfigFile, err)
	}

	// Build command arguments
	args := []string{
		"--config-file", d.config.ConfigFile,
		"-F", // run in foreground (we manage the process)
	}

	// Use context.Background() for the process lifecycle so it survives
	// after the startup context is cancelled. The caller is responsible
	// for stopping the daemon via Stop().
	d.cmd = exec.CommandContext(context.Background(), clamdExe, args...)
	// Resolve working directory to absolute path for Windows compatibility
	if absDir, err := filepath.Abs(d.config.ClamAVPath); err == nil {
		d.cmd.Dir = absDir
	} else {
		d.cmd.Dir = d.config.ClamAVPath
	}

	// Set environment for ClamAV DLLs (Windows)
	if runtime.GOOS == "windows" {
		d.cmd.Env = append(os.Environ(),
			"PATH="+d.cmd.Dir+";"+os.Getenv("PATH"),
		)
	}

	// Capture output for logging
	d.cmd.Stdout = &logWriter{prefix: "clamd"}
	d.cmd.Stderr = &logWriter{prefix: "clamd"}

	if err := d.cmd.Start(); err != nil {
		return fmt.Errorf("failed to start clamd: %w", err)
	}

	d.running = true

	// Wait for daemon to become ready in a goroutine
	go d.waitForReady()

	// Wait for readiness or timeout
	select {
	case <-d.readyCh:
		logger.InfoC("clamav", "ClamAV daemon started and ready")
		return nil
	case <-time.After(d.config.StartupTimeout):
		// Try to stop the process
		d.stopProcess()
		return fmt.Errorf("clamd failed to become ready within %v", d.config.StartupTimeout)
	case <-ctx.Done():
		d.stopProcess()
		return fmt.Errorf("context cancelled while waiting for clamd: %w", ctx.Err())
	}
}

// Stop stops the clamd daemon
func (d *Daemon) Stop() error {
	d.mu.Lock()
	defer d.mu.Unlock()

	if !d.running {
		return nil
	}

	close(d.stopCh)
	err := d.stopProcess()
	d.running = false

	if err != nil {
		logger.ErrorCF("clamav", "Failed to stop clamd daemon", map[string]interface{}{
			"error": err.Error(),
		})
	} else {
		logger.InfoC("clamav", "ClamAV daemon stopped")
	}

	return err
}

// IsRunning returns whether the daemon is running
func (d *Daemon) IsRunning() bool {
	d.mu.RLock()
	defer d.mu.RUnlock()
	return d.running
}

// IsReady returns whether the daemon is running and responsive
func (d *Daemon) IsReady() bool {
	if !d.IsRunning() {
		return false
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	return d.client.Ping(ctx) == nil
}

// WaitForReady blocks until the daemon is ready or the context is cancelled
func (d *Daemon) WaitForReady(ctx context.Context) error {
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-d.readyCh:
			return nil
		default:
			if err := d.client.Ping(ctx); err == nil {
				d.readyOnce.Do(func() { close(d.readyCh) })
				return nil
			}
			time.Sleep(500 * time.Millisecond)
		}
	}
}

// waitForReady polls clamd until it responds to PING
func (d *Daemon) waitForReady() {
	ticker := time.NewTicker(1 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-d.stopCh:
			return
		case <-ticker.C:
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			if err := d.client.Ping(ctx); err == nil {
				d.readyOnce.Do(func() { close(d.readyCh) })
				cancel()
				return
			}
			cancel()
		}
	}
}

// stopProcess stops the clamd process
func (d *Daemon) stopProcess() error {
	if d.cmd == nil || d.cmd.Process == nil {
		return nil
	}

	// Try graceful termination first
	if err := d.cmd.Process.Signal(os.Interrupt); err != nil {
		// Fallback to kill
		return d.cmd.Process.Kill()
	}

	// Wait for process to exit with timeout
	done := make(chan error, 1)
	go func() {
		done <- d.cmd.Wait()
	}()

	select {
	case err := <-done:
		return err
	case <-time.After(10 * time.Second):
		return d.cmd.Process.Kill()
	}
}

// findExecutable finds the full path to a ClamAV executable
func (d *Daemon) findExecutable(name string) string {
	if runtime.GOOS == "windows" {
		return filepath.Join(d.config.ClamAVPath, name+".exe")
	}
	return filepath.Join(d.config.ClamAVPath, name)
}

// logWriter is an io.Writer that logs to the NemesisBot logger
type logWriter struct {
	prefix string
}

func (w *logWriter) Write(p []byte) (n int, err error) {
	msg := string(p)
	// Trim trailing newlines
	msg = trimTrailingNewlines(msg)
	if msg != "" {
		logger.DebugCF("clamav", fmt.Sprintf("[%s] %s", w.prefix, msg), nil)
	}
	return len(p), nil
}

func trimTrailingNewlines(s string) string {
	for len(s) > 0 && (s[len(s)-1] == '\n' || s[len(s)-1] == '\r') {
		s = s[:len(s)-1]
	}
	return s
}
