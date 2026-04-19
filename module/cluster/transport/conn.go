// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package transport

import (
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"sync"
	"sync/atomic"
	"time"
)

var (
	// ErrConnClosed is returned when operating on a closed connection
	ErrConnClosed = errors.New("connection is closed")
	// ErrSendTimeout is returned when send operation times out
	ErrSendTimeout = errors.New("send timeout")
)

// TCPConn represents a TCP connection with read/write goroutines
type TCPConn struct {
	// Core connection
	conn net.Conn

	// Identification
	nodeID  string
	address string

	// Channels
	sendChan  chan []byte      // Outgoing data
	recvChan  chan *RPCMessage // Incoming messages
	closeChan chan struct{}    // Close signal

	// State
	closed       atomic.Bool   // Connection closed flag
	started      atomic.Bool   // Goroutines started flag
	droppedCount atomic.Uint64 // Number of dropped messages (recvChan full)

	// Timing
	createdAt time.Time
	lastUsed  atomic.Value // time.Time

	// Configuration
	readBufferSize    int
	sendBufferSize    int
	sendTimeout       time.Duration
	idleTimeout       time.Duration
	heartbeatInterval time.Duration
	authToken         string // RPC authentication token

	// Synchronization
	wg sync.WaitGroup
	mu sync.RWMutex
}

// TCPConnConfig contains configuration for creating a new TCPConn
type TCPConnConfig struct {
	NodeID            string
	Address           string
	ReadBufferSize    int // Size of receive channel
	SendBufferSize    int // Size of send channel
	SendTimeout       time.Duration
	IdleTimeout       time.Duration
	HeartbeatInterval time.Duration
	AuthToken         string // RPC authentication token
}

// DefaultTCPConnConfig returns the default configuration
func DefaultTCPConnConfig(nodeID, address string) *TCPConnConfig {
	return &TCPConnConfig{
		NodeID:            nodeID,
		Address:           address,
		ReadBufferSize:    100,
		SendBufferSize:    100,
		SendTimeout:       10 * time.Second,
		IdleTimeout:       30 * time.Second,
		HeartbeatInterval: 0,   // Disabled by default
		AuthToken:         "", // No auth by default
	}
}

// NewTCPConn creates a new TCP connection wrapper
func NewTCPConn(conn net.Conn, config *TCPConnConfig) *TCPConn {
	if config == nil {
		config = DefaultTCPConnConfig("", conn.RemoteAddr().String())
	}

	tc := &TCPConn{
		conn:              conn,
		nodeID:            config.NodeID,
		address:           config.Address,
		sendChan:          make(chan []byte, config.SendBufferSize),
		recvChan:          make(chan *RPCMessage, config.ReadBufferSize),
		closeChan:         make(chan struct{}),
		createdAt:         time.Now(),
		readBufferSize:    config.ReadBufferSize,
		sendBufferSize:    config.SendBufferSize,
		sendTimeout:       config.SendTimeout,
		idleTimeout:       config.IdleTimeout,
		heartbeatInterval: config.HeartbeatInterval,
		authToken:         config.AuthToken, // Save auth token
	}

	tc.lastUsed.Store(time.Now())

	return tc
}

// Start starts the read and write goroutines
func (tc *TCPConn) Start() {
	if !tc.started.CompareAndSwap(false, true) {
		return // Already started
	}

	// Send authentication token if configured (client-side)
	if tc.authToken != "" {
		// Set write deadline for auth
		tc.conn.SetWriteDeadline(time.Now().Add(10 * time.Second))

		// Send token with newline
		_, err := tc.conn.Write([]byte(tc.authToken + "\n"))
		if err != nil {
			tc.Close()
			return
		}

		// Reset deadline
		tc.conn.SetWriteDeadline(time.Time{})
	}

	// Start read goroutine
	tc.wg.Add(1)
	go tc.readLoop()

	// Start write goroutine
	tc.wg.Add(1)
	go tc.writeLoop()

	// Start idle monitor
	if tc.idleTimeout > 0 {
		tc.wg.Add(1)
		go tc.idleMonitor()
	}
}

// readLoop continuously reads from the connection
func (tc *TCPConn) readLoop() {
	defer tc.wg.Done()

	fr := NewFrameReader(tc.conn)

	for {
		// Check if closed
		if tc.closed.Load() {
			return
		}

		// Read frame with deadline
		if tc.idleTimeout > 0 {
			tc.conn.SetReadDeadline(time.Now().Add(tc.idleTimeout))
		}

		data, err := fr.ReadFrame()
		if err != nil {
			if !tc.closed.Load() {
				// Connection error (not intentional close)
				tc.Close()
			}
			return
		}

		// Update last used time
		tc.lastUsed.Store(time.Now())

		// Parse as RPC message
		var msg RPCMessage
		if err := json.Unmarshal(data, &msg); err != nil {
			// Invalid message, log and continue
			continue
		}

		// Send to receive channel (non-blocking)
		select {
		case tc.recvChan <- &msg:
		default:
			// Channel full, drop message (backpressure)
			tc.droppedCount.Add(1)
		}
	}
}

// writeLoop continuously writes to the connection
func (tc *TCPConn) writeLoop() {
	defer tc.wg.Done()

	for {
		select {
		case <-tc.closeChan:
			return

		case data, ok := <-tc.sendChan:
			if !ok {
				return
			}

			// Set write deadline
			if tc.sendTimeout > 0 {
				tc.conn.SetWriteDeadline(time.Now().Add(tc.sendTimeout))
			}

			// Write frame
			if err := WriteFrame(tc.conn, data); err != nil {
				if !tc.closed.Load() {
					tc.Close()
				}
				return
			}

			// Update last used time
			tc.lastUsed.Store(time.Now())
		}
	}
}

// idleMonitor monitors the connection for idle timeout
func (tc *TCPConn) idleMonitor() {
	defer tc.wg.Done()

	ticker := time.NewTicker(tc.idleTimeout / 2)
	defer ticker.Stop()

	for {
		select {
		case <-tc.closeChan:
			return
		case <-ticker.C:
			lastUsed := tc.lastUsed.Load().(time.Time)
			if time.Since(lastUsed) > tc.idleTimeout {
				// Connection idle, close it
				tc.Close()
				return
			}
		}
	}
}

// Send sends a message through the connection
func (tc *TCPConn) Send(msg *RPCMessage) error {
	tc.mu.RLock()
	if tc.closed.Load() || tc.sendChan == nil {
		tc.mu.RUnlock()
		return ErrConnClosed
	}

	// Marshal to JSON
	data, err := msg.Bytes()
	if err != nil {
		tc.mu.RUnlock()
		return fmt.Errorf("failed to marshal message: %w", err)
	}

	// Send to write goroutine (non-blocking with timeout)
	select {
	case tc.sendChan <- data:
		tc.mu.RUnlock()
		return nil
	case <-tc.closeChan:
		tc.mu.RUnlock()
		return ErrConnClosed
	case <-time.After(tc.sendTimeout):
		tc.mu.RUnlock()
		return ErrSendTimeout
	}
}

// Receive returns a channel for receiving messages
func (tc *TCPConn) Receive() <-chan *RPCMessage {
	return tc.recvChan
}

// Close closes the connection gracefully
func (tc *TCPConn) Close() error {
	if !tc.closed.CompareAndSwap(false, true) {
		return nil // Already closed
	}

	// Signal close
	close(tc.closeChan)

	// Close send channel
	tc.mu.Lock()
	if tc.sendChan != nil {
		close(tc.sendChan)
		tc.sendChan = nil
	}
	tc.mu.Unlock()

	// Close underlying connection
	if tc.conn != nil {
		tc.conn.Close()
	}

	// Wait for goroutines to finish (including readLoop)
	done := make(chan struct{})
	go func() {
		tc.wg.Wait()
		close(done)
	}()

	// Wait with timeout
	select {
	case <-done:
		// readLoop has exited, safe to close recvChan
		close(tc.recvChan)
		return nil
	case <-time.After(5 * time.Second):
		// Timeout — close recvChan anyway to prevent consumer goroutine leaks
		close(tc.recvChan)
		return errors.New("timeout waiting for connection to close")
	}
}

// IsClosed returns true if the connection is closed
func (tc *TCPConn) IsClosed() bool {
	return tc.closed.Load()
}

// IsActive returns true if the connection is active (not closed and recently used)
func (tc *TCPConn) IsActive() bool {
	if tc.closed.Load() {
		return false
	}

	// Check last activity
	lastUsed := tc.lastUsed.Load().(time.Time)
	if tc.idleTimeout > 0 && time.Since(lastUsed) > tc.idleTimeout {
		return false
	}

	return true
}

// GetNodeID returns the node ID
func (tc *TCPConn) GetNodeID() string {
	return tc.nodeID
}

// GetAddress returns the remote address
func (tc *TCPConn) GetAddress() string {
	return tc.address
}

// GetLocalAddr returns the local address
func (tc *TCPConn) GetLocalAddr() net.Addr {
	if tc.conn != nil {
		return tc.conn.LocalAddr()
	}
	return nil
}

// GetRemoteAddr returns the remote address
func (tc *TCPConn) GetRemoteAddr() net.Addr {
	if tc.conn != nil {
		return tc.conn.RemoteAddr()
	}
	return nil
}

// GetCreatedAt returns the connection creation time
func (tc *TCPConn) GetCreatedAt() time.Time {
	return tc.createdAt
}

// GetLastUsed returns the last activity time
func (tc *TCPConn) GetLastUsed() time.Time {
	return tc.lastUsed.Load().(time.Time)
}

// UpdateLastUsed updates the last used timestamp
func (tc *TCPConn) UpdateLastUsed() {
	tc.lastUsed.Store(time.Now())
}

// SetNodeID sets the node ID
func (tc *TCPConn) SetNodeID(nodeID string) {
	tc.nodeID = nodeID
}

// GetDroppedCount returns the number of messages dropped due to full recvChan
func (tc *TCPConn) GetDroppedCount() uint64 {
	return tc.droppedCount.Load()
}

// RemoteAddr returns the remote address (for compatibility with old interface)
func (tc *TCPConn) RemoteAddr() net.Addr {
	return tc.GetRemoteAddr()
}

// LocalAddr returns the local address (for compatibility with old interface)
func (tc *TCPConn) LocalAddr() net.Addr {
	return tc.GetLocalAddr()
}
