// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
// Package transport provides stdio transport for MCP client communication.
// This transport starts a subprocess and communicates via stdin/stdout using JSON-RPC.
package transport

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os/exec"
	"sync"

	"github.com/276793422/NemesisBot/module/logger"
)

// StdioTransport implements the Transport interface for subprocess communication.
// The MCP server is started as a subprocess and communicates via stdin/stdout.
type StdioTransport struct {
	command string
	args    []string
	env     []string

	cmd     *exec.Cmd
	stdin   io.WriteCloser
	stdout  io.Reader
	scanner *bufio.Scanner

	mu        sync.RWMutex
	connected bool
}

// NewStdioTransport creates a new stdio transport for the given command.
// The command will be started when Connect() is called.
func NewStdioTransport(command string, args []string, env []string) (*StdioTransport, error) {
	if command == "" {
		return nil, fmt.Errorf("command cannot be empty")
	}

	return &StdioTransport{
		command: command,
		args:    args,
		env:     env,
	}, nil
}

// Connect starts the subprocess and establishes communication.
func (t *StdioTransport) Connect(ctx context.Context) error {
	t.mu.Lock()
	defer t.mu.Unlock()

	if t.connected {
		return nil
	}

	logger.InfoCF("mcp.transport", "Starting MCP server subprocess",
		map[string]interface{}{
			"command": t.command,
			"args":    t.args,
		})

	// Create command without tying to context
	// The context is used for request timeouts, not subprocess lifecycle
	t.cmd = exec.Command(t.command, t.args...)

	// Set environment variables if provided
	if len(t.env) > 0 {
		t.cmd.Env = append(t.cmd.Env, t.env...)
	}

	// Create pipes for stdin/stdout
	stdin, err := t.cmd.StdinPipe()
	if err != nil {
		return fmt.Errorf("failed to create stdin pipe: %w", err)
	}
	t.stdin = stdin

	stdout, err := t.cmd.StdoutPipe()
	if err != nil {
		return fmt.Errorf("failed to create stdout pipe: %w", err)
	}
	t.stdout = stdout

	// Start the subprocess
	if err := t.cmd.Start(); err != nil {
		return fmt.Errorf("failed to start MCP server: %w", err)
	}

	// Create scanner for reading stdout line by line
	// JSON-RPC over stdio uses newline-delimited JSON
	t.scanner = bufio.NewScanner(t.stdout)
	t.scanner.Split(bufio.ScanLines)

	t.connected = true

	logger.InfoCF("mcp.transport", "MCP server subprocess started",
		map[string]interface{}{
			"pid": t.cmd.Process.Pid,
		})

	return nil
}

// Send sends a JSON-RPC request and waits for the response.
func (t *StdioTransport) Send(ctx context.Context, req *JSONRPCRequest) (*JSONRPCResponse, error) {
	// Check if connected
	t.mu.RLock()
	if !t.connected {
		t.mu.RUnlock()
		return nil, fmt.Errorf("transport is not connected")
	}
	stdin := t.stdin
	scanner := t.scanner
	t.mu.RUnlock()

	// Serialize request to JSON
	data, err := json.Marshal(req)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal request: %w", err)
	}

	logger.DebugCF("mcp.transport", "Sending JSON-RPC request",
		map[string]interface{}{
			"method": req.Method,
			"id":     req.ID,
		})

	// Send request followed by newline
	// JSON-RPC over stdio is newline-delimited
	if _, err := fmt.Fprintln(stdin, string(data)); err != nil {
		// Check if process died
		t.mu.RLock()
		connected := t.connected && t.cmd != nil && t.cmd.Process != nil
		t.mu.RUnlock()

		if connected {
			if ps, err := t.cmd.Process.Wait(); err == nil {
				if !ps.Success() {
					return nil, fmt.Errorf("MCP server process exited with code %d", ps.ExitCode())
				}
			}
		}

		return nil, fmt.Errorf("failed to send request: %w", err)
	}

	// Wait for response with timeout support
	// This blocks until a line is received, the process exits, or context times out
	type scanResult struct {
		line string
		err  error
	}

	resultChan := make(chan scanResult, 1)

	// Start scanning in a goroutine
	go func() {
		if !scanner.Scan() {
			err := scanner.Err()
			if err != nil {
				resultChan <- scanResult{err: err}
			} else {
				resultChan <- scanResult{err: fmt.Errorf("connection closed (EOF)")}
			}
			return
		}
		resultChan <- scanResult{line: scanner.Text()}
	}()

	// Wait for result or context timeout
	select {
	case <-ctx.Done():
		// Context timed out or was cancelled
		return nil, fmt.Errorf("request timeout: %w", ctx.Err())
	case result := <-resultChan:
		if result.err != nil {
			return nil, result.err
		}
		line := result.line

		logger.DebugCF("mcp.transport", "Received JSON-RPC response",
			map[string]interface{}{
				"id":  req.ID,
				"raw": line,
			})

		// Parse response
		var resp JSONRPCResponse
		if err := json.Unmarshal([]byte(line), &resp); err != nil {
			return nil, fmt.Errorf("failed to unmarshal response: %w (raw: %s)", err, line)
		}

		return &resp, nil
	}
}

// Close terminates the subprocess and cleans up resources.
func (t *StdioTransport) Close() error {
	t.mu.Lock()
	defer t.mu.Unlock()

	if !t.connected {
		return nil
	}

	logger.InfoCF("mcp.transport", "Stopping MCP server subprocess",
		map[string]interface{}{
			"pid": func() int {
				if t.cmd != nil && t.cmd.Process != nil {
					return t.cmd.Process.Pid
				}
				return 0
			}(),
		})

	t.connected = false

	// Close stdin to signal EOF to the subprocess
	if t.stdin != nil {
		if err := t.stdin.Close(); err != nil {
			logger.WarnCF("mcp.transport", "Error closing stdin",
				map[string]interface{}{
					"error": err.Error(),
				})
		}
	}

	// Wait for process to exit
	if t.cmd != nil && t.cmd.Process != nil {
		// Try to wait gracefully first
		state, err := t.cmd.Process.Wait()
		if err != nil {
			// Force kill if it doesn't exit
			logger.DebugCF("mcp.transport", "Force killing MCP server process",
				map[string]interface{}{
					"pid": t.cmd.Process.Pid,
				})
			t.cmd.Process.Kill()
		} else {
			logger.DebugCF("mcp.transport", "MCP server process exited",
				map[string]interface{}{
					"pid":       t.cmd.Process.Pid,
					"success":   state.Success(),
					"exit_code": state.ExitCode(),
				})
		}
	}

	return nil
}

// IsConnected returns true if the transport is connected.
func (t *StdioTransport) IsConnected() bool {
	t.mu.RLock()
	defer t.mu.RUnlock()
	return t.connected
}

// Name returns the transport type name.
func (t *StdioTransport) Name() string {
	return "stdio"
}
