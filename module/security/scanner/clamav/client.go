// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

// Package clamav provides a Go client for the ClamAV clamd daemon
package clamav

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"net"
	"strings"
	"time"
)

// ScanResult represents the result of a virus scan
type ScanResult struct {
	// Path is the scanned file path
	Path string
	// Infected indicates whether a virus was found
	Infected bool
	// Virus is the name of the detected virus (empty if clean)
	Virus string
	// Raw is the raw response from clamd
	Raw string
}

// Clean returns whether the scan result is clean (no virus found)
func (r *ScanResult) Clean() bool {
	return !r.Infected
}

// Client is a TCP client for the clamd daemon
type Client struct {
	// Address is the clamd TCP address (host:port)
	Address string
	// Timeout is the connection and I/O timeout
	Timeout time.Duration
}

// NewClient creates a new clamd client
func NewClient(address string) *Client {
	return &Client{
		Address: address,
		Timeout: 30 * time.Second,
	}
}

// NewClientWithTimeout creates a new clamd client with a custom timeout
func NewClientWithTimeout(address string, timeout time.Duration) *Client {
	return &Client{
		Address: address,
		Timeout: timeout,
	}
}

// Ping checks if clamd is alive and responsive
func (c *Client) Ping(ctx context.Context) error {
	resp, err := c.sendCommand(ctx, "PING")
	if err != nil {
		return fmt.Errorf("clamd ping failed: %w", err)
	}
	if resp != "PONG" {
		return fmt.Errorf("clamd unexpected ping response: %s", resp)
	}
	return nil
}

// Version returns the clamd version string
func (c *Client) Version(ctx context.Context) (string, error) {
	resp, err := c.sendCommand(ctx, "VERSION")
	if err != nil {
		return "", fmt.Errorf("clamd version failed: %w", err)
	}
	return resp, nil
}

// ScanFile scans a single file by path on the clamd host
func (c *Client) ScanFile(ctx context.Context, filePath string) (*ScanResult, error) {
	cmd := fmt.Sprintf("SCAN %s", filePath)
	resp, err := c.sendCommand(ctx, cmd)
	if err != nil {
		return nil, fmt.Errorf("clamd scan failed: %w", err)
	}
	return parseScanResponse(resp), nil
}

// ContScan scans a file or directory without stopping on infected files
func (c *Client) ContScan(ctx context.Context, path string) ([]*ScanResult, error) {
	cmd := fmt.Sprintf("CONTSCAN %s", path)
	resp, err := c.sendCommand(ctx, cmd)
	if err != nil {
		return nil, fmt.Errorf("clamd contscan failed: %w", err)
	}
	return parseMultiScanResponse(resp), nil
}

// ScanStream scans content streamed from the caller using the INSTREAM protocol.
func (c *Client) ScanStream(ctx context.Context, content io.Reader) (*ScanResult, error) {
	dialer := net.Dialer{Timeout: c.Timeout}
	conn, err := dialer.DialContext(ctx, "tcp", c.Address)
	if err != nil {
		return nil, fmt.Errorf("failed to connect to clamd: %w", err)
	}
	defer conn.Close()

	// Set deadlines from context
	if deadline, ok := ctx.Deadline(); ok {
		conn.SetDeadline(deadline)
	} else {
		conn.SetDeadline(time.Now().Add(c.Timeout))
	}

	// Send INSTREAM command
	if _, err := fmt.Fprintf(conn, "nINSTREAM\n"); err != nil {
		return nil, fmt.Errorf("failed to send INSTREAM command: %w", err)
	}

	// Stream the content with chunked encoding (clamd INSTREAM protocol)
	// Format: <length(4 bytes big-endian)><data>... <0 length to end>
	buf := make([]byte, 32*1024) // 32KB chunks
	for {
		n, readErr := content.Read(buf)
		if n > 0 {
			// Write 4-byte big-endian length
			lenBuf := []byte{
				byte(n >> 24),
				byte(n >> 16),
				byte(n >> 8),
				byte(n),
			}
			if _, err := conn.Write(lenBuf); err != nil {
				return nil, fmt.Errorf("failed to write chunk length: %w", err)
			}
			if _, err := conn.Write(buf[:n]); err != nil {
				return nil, fmt.Errorf("failed to write chunk data: %w", err)
			}
		}
		if readErr == io.EOF {
			break
		}
		if readErr != nil {
			return nil, fmt.Errorf("failed to read content: %w", readErr)
		}
	}

	// Send termination (0-length chunk)
	termBuf := []byte{0, 0, 0, 0}
	if _, err := conn.Write(termBuf); err != nil {
		return nil, fmt.Errorf("failed to send stream termination: %w", err)
	}

	// Read response
	conn.SetDeadline(time.Now().Add(c.Timeout))
	reader := bufio.NewReader(conn)
	respLine, err := reader.ReadString('\n')
	if err != nil {
		return nil, fmt.Errorf("failed to read scan response: %w", err)
	}

	return parseScanResponse(strings.TrimSpace(respLine)), nil
}

// Reload reloads the virus database
func (c *Client) Reload(ctx context.Context) error {
	resp, err := c.sendCommand(ctx, "RELOAD")
	if err != nil {
		return fmt.Errorf("clamd reload failed: %w", err)
	}
	if !strings.Contains(resp, "RELOADING") {
		return fmt.Errorf("unexpected reload response: %s", resp)
	}
	return nil
}

// Stats returns clamd statistics
func (c *Client) Stats(ctx context.Context) (string, error) {
	resp, err := c.sendCommand(ctx, "STATS")
	if err != nil {
		return "", fmt.Errorf("clamd stats failed: %w", err)
	}
	return resp, nil
}

// sendCommand sends a command to clamd and reads the response
func (c *Client) sendCommand(ctx context.Context, command string) (string, error) {
	dialer := net.Dialer{Timeout: c.Timeout}
	conn, err := dialer.DialContext(ctx, "tcp", c.Address)
	if err != nil {
		return "", fmt.Errorf("failed to connect to clamd at %s: %w", c.Address, err)
	}
	defer conn.Close()

	// Set deadlines
	if deadline, ok := ctx.Deadline(); ok {
		conn.SetDeadline(deadline)
	} else {
		conn.SetDeadline(time.Now().Add(c.Timeout))
	}

	// Send command with "n" prefix (non-blocking ID command style)
	cmd := fmt.Sprintf("n%s\n", command)
	if _, err := conn.Write([]byte(cmd)); err != nil {
		return "", fmt.Errorf("failed to send command: %w", err)
	}

	// Read response
	reader := bufio.NewReader(conn)
	var lines []string
	for {
		line, err := reader.ReadString('\n')
		if line != "" {
			lines = append(lines, strings.TrimSpace(line))
		}
		if err != nil {
			if err == io.EOF {
				break
			}
			// Timeout while reading may indicate end of response
			if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
				break
			}
			return "", fmt.Errorf("failed to read response: %w", err)
		}
		// For single-line responses (PING, VERSION, SCAN), return immediately
		if len(lines) == 1 && isSingleResponseCommand(command) {
			break
		}
	}

	if len(lines) == 0 {
		return "", fmt.Errorf("empty response from clamd")
	}

	return strings.Join(lines, "\n"), nil
}

// isSingleResponseCommand checks if a command expects a single-line response
func isSingleResponseCommand(cmd string) bool {
	switch {
	case cmd == "PING", cmd == "VERSION":
		return true
	case strings.HasPrefix(cmd, "SCAN "), strings.HasPrefix(cmd, "CONTSCAN "):
		return true
	default:
		return false
	}
}

// parseScanResponse parses a single-line clamd scan response
// Format: "path: virus_name FOUND" or "path: OK" or "path: virus_name FOUND ERROR"
func parseScanResponse(raw string) *ScanResult {
	result := &ScanResult{
		Raw: raw,
	}

	// Check for ERROR suffix
	if strings.HasSuffix(raw, " ERROR") {
		result.Raw = raw
		return result
	}

	// Check for FOUND (infected)
	if idx := strings.LastIndex(raw, ": "); idx != -1 {
		result.Path = raw[:idx]
		statusVirus := raw[idx+2:]

		if strings.HasSuffix(statusVirus, " FOUND") {
			result.Infected = true
			result.Virus = strings.TrimSuffix(statusVirus, " FOUND")
		} else if statusVirus == "OK" {
			result.Infected = false
		}
	} else {
		// Fallback: check whole string
		if strings.HasSuffix(raw, " FOUND") {
			result.Infected = true
			result.Virus = strings.TrimSuffix(raw, " FOUND")
		}
	}

	return result
}

// parseMultiScanResponse parses a multi-line clamd scan response
func parseMultiScanResponse(raw string) []*ScanResult {
	lines := strings.Split(raw, "\n")
	results := make([]*ScanResult, 0, len(lines))
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		results = append(results, parseScanResponse(line))
	}
	return results
}

// hostPart extracts the host portion from an address string
func hostPart(addr string) string {
	host, _, err := net.SplitHostPort(addr)
	if err != nil {
		return addr
	}
	return host
}
