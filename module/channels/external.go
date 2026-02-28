// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// External Channel - Connects external input/output executables to NemesisBot

package channels

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"sync"
	"sync/atomic"
	"time"

	"github.com/276793422/NemesisBot/module/bus"
	"github.com/276793422/NemesisBot/module/config"
	"github.com/276793422/NemesisBot/module/logger"
)

// ExternalChannel manages communication with external input/output executables
// Input EXE: reads stdout and sends to message bus
// Output EXE: receives messages via stdin
type ExternalChannel struct {
	*BaseChannel
	config       *config.ExternalConfig
	inputCmd     *exec.Cmd
	outputCmd    *exec.Cmd
	inputPipe    io.WriteCloser
	outputReader io.ReadCloser
	outputWriter io.WriteCloser
	running      atomic.Bool
	stopped      chan struct{}
	wg           sync.WaitGroup
}

// NewExternalChannel creates a new external channel
func NewExternalChannel(cfg *config.ExternalConfig, messageBus *bus.MessageBus) (*ExternalChannel, error) {
	if cfg.InputEXE == "" || cfg.OutputEXE == "" {
		return nil, fmt.Errorf("both input_exe and output_exe must be specified")
	}

	// Validate executable paths
	if _, err := os.Stat(cfg.InputEXE); err != nil {
		return nil, fmt.Errorf("input exe not found: %s: %w", cfg.InputEXE, err)
	}
	if _, err := os.Stat(cfg.OutputEXE); err != nil {
		return nil, fmt.Errorf("output exe not found: %s: %w", cfg.OutputEXE, err)
	}

	base := NewBaseChannel("external", cfg, messageBus, cfg.AllowFrom)

	return &ExternalChannel{
		BaseChannel: base,
		config:      cfg,
		stopped:     make(chan struct{}),
	}, nil
}

// Start starts the external channel and launches both executables
func (c *ExternalChannel) Start(ctx context.Context) error {
	logger.InfoCF("external", "Starting external channel", map[string]interface{}{
		"input_exe":  c.config.InputEXE,
		"output_exe": c.config.OutputEXE,
		"chat_id":    c.config.ChatID,
		"sync_to":    c.config.SyncTo,
	})

	// Start input EXE
	if err := c.startInputEXE(ctx); err != nil {
		return fmt.Errorf("failed to start input exe: %w", err)
	}

	// Start output EXE
	if err := c.startOutputEXE(ctx); err != nil {
		c.stopInputEXE()
		return fmt.Errorf("failed to start output exe: %w", err)
	}

	c.running.Store(true)
	logger.InfoC("external", "External channel started successfully")
	return nil
}

// startInputEXE launches the input executable and reads from stdout
func (c *ExternalChannel) startInputEXE(ctx context.Context) error {
	c.inputCmd = exec.CommandContext(ctx, c.config.InputEXE)

	// Get stdin pipe for writing (if needed)
	stdin, err := c.inputCmd.StdinPipe()
	if err != nil {
		return fmt.Errorf("failed to get stdin pipe: %w", err)
	}
	c.inputPipe = stdin

	// Get stdout pipe for reading
	stdout, err := c.inputCmd.StdoutPipe()
	if err != nil {
		return fmt.Errorf("failed to get stdout pipe: %w", err)
	}

	// Redirect stderr to logger
	stderr, err := c.inputCmd.StderrPipe()
	if err != nil {
		return fmt.Errorf("failed to get stderr pipe: %w", err)
	}

	// Start the process
	if err := c.inputCmd.Start(); err != nil {
		return fmt.Errorf("failed to start input exe: %w", err)
	}

	// Start stderr reader in background
	c.wg.Add(1)
	go c.readStderr(c.inputCmd, stderr)

	// Start stdout reader in background
	c.wg.Add(1)
	go c.readInputEXEStdout(stdout)

	logger.InfoCF("external", "Input EXE started", map[string]interface{}{
		"pid": c.inputCmd.Process.Pid,
	})

	return nil
}

// startOutputEXE launches the output executable
func (c *ExternalChannel) startOutputEXE(ctx context.Context) error {
	c.outputCmd = exec.CommandContext(ctx, c.config.OutputEXE)

	// Get stdin pipe for writing
	stdin, err := c.outputCmd.StdinPipe()
	if err != nil {
		return fmt.Errorf("failed to get stdin pipe: %w", err)
	}
	c.outputWriter = stdin

	// Get stdout pipe for reading (optional, for logging/response)
	stdout, err := c.outputCmd.StdoutPipe()
	if err != nil {
		return fmt.Errorf("failed to get stdout pipe: %w", err)
	}
	c.outputReader = stdout

	// Get stderr pipe for logging
	stderr, err := c.outputCmd.StderrPipe()
	if err != nil {
		return fmt.Errorf("failed to get stderr pipe: %w", err)
	}

	// Start the process
	if err := c.outputCmd.Start(); err != nil {
		return fmt.Errorf("failed to start output exe: %w", err)
	}

	// Start stderr reader in background
	c.wg.Add(1)
	go c.readStderr(c.outputCmd, stderr)

	// Start stdout reader (for any responses from output exe)
	c.wg.Add(1)
	go c.readOutputEXEStdout(stdout)

	logger.InfoCF("external", "Output EXE started", map[string]interface{}{
		"pid": c.outputCmd.Process.Pid,
	})

	return nil
}

// readInputEXEStdout reads lines from input EXE's stdout and sends to message bus
func (c *ExternalChannel) readInputEXEStdout(reader io.Reader) {
	defer c.wg.Done()

	scanner := bufio.NewScanner(reader)
	for scanner.Scan() {
		line := scanner.Text()

		// Skip empty lines
		if line == "" {
			continue
		}

		logger.DebugCF("external", "Received from input EXE", map[string]interface{}{
			"content": line,
		})

		// Send to message bus
		c.HandleMessage(
			c.config.ChatID,
			c.config.ChatID,
			line,
			[]string{},
			nil,
		)

		// Sync to configured targets
		if len(c.config.SyncTo) > 0 {
			c.SyncToTargets("user", line)
		}
	}

	if err := scanner.Err(); err != nil {
		if c.running.Load() {
			logger.ErrorCF("external", "Error reading from input EXE stdout", map[string]interface{}{
				"error": err.Error(),
			})
		}
	}

	logger.InfoC("external", "Input EXE stdout reader stopped")
}

// readOutputEXEStdout reads responses from output EXE (for logging)
func (c *ExternalChannel) readOutputEXEStdout(reader io.Reader) {
	defer c.wg.Done()

	scanner := bufio.NewScanner(reader)
	for scanner.Scan() {
		line := scanner.Text()
		if line != "" {
			logger.DebugCF("external", "Output EXE response", map[string]interface{}{
				"content": line,
			})
		}
	}

	logger.InfoC("external", "Output EXE stdout reader stopped")
}

// readStderr reads and logs stderr from processes
func (c *ExternalChannel) readStderr(cmd *exec.Cmd, reader io.Reader) {
	defer c.wg.Done()

	scanner := bufio.NewScanner(reader)
	for scanner.Scan() {
		line := scanner.Text()
		if line != "" {
			logger.DebugCF("external", "Process stderr", map[string]interface{}{
				"process": cmd.Path,
				"content": line,
			})
		}
	}
}

// Send sends a message to the output EXE via stdin
func (c *ExternalChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
	if !c.running.Load() {
		return fmt.Errorf("external channel not running")
	}

	if msg.ChatID != c.config.ChatID {
		return fmt.Errorf("invalid chat ID: %s (expected: %s)", msg.ChatID, c.config.ChatID)
	}

	logger.DebugCF("external", "Sending to output EXE", map[string]interface{}{
		"content": msg.Content,
	})

	// Send to output EXE
	if c.outputWriter != nil {
		// Write message with newline
		if _, err := fmt.Fprintln(c.outputWriter, msg.Content); err != nil {
			logger.ErrorCF("external", "Failed to write to output EXE", map[string]interface{}{
				"error": err.Error(),
			})
			return fmt.Errorf("failed to write to output exe: %w", err)
		}

		// Flush the buffer
		if flusher, ok := c.outputWriter.(interface{ Flush() error }); ok {
			if err := flusher.Flush(); err != nil {
				logger.WarnCF("external", "Failed to flush output EXE stdin", map[string]interface{}{
					"error": err.Error(),
				})
			}
		}
	}

	// Sync to configured targets
	if len(c.config.SyncTo) > 0 {
		c.SyncToTargets("assistant", msg.Content)
	}

	return nil
}

// IsRunning returns whether the channel is running
func (c *ExternalChannel) IsRunning() bool {
	return c.running.Load()
}

// Stop stops the external channel and terminates both executables
func (c *ExternalChannel) Stop(ctx context.Context) error {
	logger.InfoC("external", "Stopping external channel")

	if !c.running.Load() {
		return nil
	}

	c.running.Store(false)

	// Stop both EXEs
	c.stopInputEXE()
	c.stopOutputEXE()

	// Wait for all goroutines to finish
	done := make(chan struct{})
	go func() {
		c.wg.Wait()
		close(done)
	}()

	select {
	case <-done:
		logger.InfoC("external", "External channel stopped")
		return nil
	case <-time.After(10 * time.Second):
		logger.WarnC("external", "Timeout waiting for external channel to stop")
		return fmt.Errorf("timeout stopping external channel")
	}
}

// stopInputEXE terminates the input executable
func (c *ExternalChannel) stopInputEXE() {
	if c.inputCmd == nil || c.inputCmd.Process == nil {
		return
	}

	logger.InfoC("external", "Stopping input EXE")

	// Close stdin
	if c.inputPipe != nil {
		c.inputPipe.Close()
	}

	// Try graceful shutdown first
	if err := c.inputCmd.Process.Signal(os.Interrupt); err != nil {
		// Force kill if interrupt fails
		c.inputCmd.Process.Kill()
	}

	// Wait for process to exit
	done := make(chan error, 1)
	go func() {
		done <- c.inputCmd.Wait()
	}()

	select {
	case <-done:
		logger.InfoC("external", "Input EXE stopped")
	case <-time.After(5 * time.Second):
		c.inputCmd.Process.Kill()
		logger.WarnC("external", "Input EXE force killed")
	}
}

// stopOutputEXE terminates the output executable
func (c *ExternalChannel) stopOutputEXE() {
	if c.outputCmd == nil || c.outputCmd.Process == nil {
		return
	}

	logger.InfoC("external", "Stopping output EXE")

	// Close stdin
	if c.outputWriter != nil {
		c.outputWriter.Close()
	}

	// Close stdout reader
	if c.outputReader != nil {
		c.outputReader.Close()
	}

	// Try graceful shutdown first
	if err := c.outputCmd.Process.Signal(os.Interrupt); err != nil {
		// Force kill if interrupt fails
		c.outputCmd.Process.Kill()
	}

	// Wait for process to exit
	done := make(chan error, 1)
	go func() {
		done <- c.outputCmd.Wait()
	}()

	select {
	case <-done:
		logger.InfoC("external", "Output EXE stopped")
	case <-time.After(5 * time.Second):
		c.outputCmd.Process.Kill()
		logger.WarnC("external", "Output EXE force killed")
	}
}
