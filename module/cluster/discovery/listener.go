// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors

package discovery

import (
	"encoding/json"
	"fmt"
	"net"
	"sync"
	"time"
)

// UDPListener handles UDP broadcast discovery
type UDPListener struct {
	conn       *net.UDPConn
	port       int
	mu         sync.RWMutex
	running    bool
	stopCh     chan struct{}
	onMessage  func(*DiscoveryMessage, *net.UDPAddr)
}

// NewUDPListener creates a new UDP listener
func NewUDPListener(port int) (*UDPListener, error) {
	// Listen on all interfaces
	addr, err := net.ResolveUDPAddr("udp", fmt.Sprintf(":%d", port))
	if err != nil {
		return nil, fmt.Errorf("failed to resolve UDP address: %w", err)
	}

	conn, err := net.ListenUDP("udp", addr)
	if err != nil {
		return nil, fmt.Errorf("failed to listen on UDP port %d: %w", port, err)
	}

	return &UDPListener{
		conn:  conn,
		port:  port,
		stopCh: make(chan struct{}),
	}, nil
}

// SetMessageHandler sets the callback for received messages
func (l *UDPListener) SetMessageHandler(handler func(*DiscoveryMessage, *net.UDPAddr)) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.onMessage = handler
}

// Start starts the listener
func (l *UDPListener) Start() error {
	l.mu.Lock()
	if l.running {
		l.mu.Unlock()
		return fmt.Errorf("listener already running")
	}
	l.running = true
	l.mu.Unlock()

	// Start receive loop
	go l.receiveLoop()

	return nil
}

// Stop stops the listener
func (l *UDPListener) Stop() error {
	l.mu.Lock()
	if !l.running {
		l.mu.Unlock()
		return fmt.Errorf("listener not running")
	}
	l.running = false

	// Signal stop
	close(l.stopCh)

	l.mu.Unlock()

	// Close connection
	return l.conn.Close()
}

// receiveLoop receives UDP messages
func (l *UDPListener) receiveLoop() {
	buf := make([]byte, 1024)

	for {
		select {
		case <-l.stopCh:
			return
		default:
			// Set read deadline to allow checking stopCh
			l.conn.SetReadDeadline(time.Now().Add(1 * time.Second))

			n, addr, err := l.conn.ReadFromUDP(buf)
			if err != nil {
				// Timeout is expected, continue
				if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
					continue
				}
				// Connection closed
				return
			}

			// Parse message
			var msg DiscoveryMessage
			if err := json.Unmarshal(buf[:n], &msg); err != nil {
				// Invalid message, skip
				continue
			}

			// Validate message
			if err := msg.Validate(); err != nil {
				// Invalid message, skip
				continue
			}

			// Call handler if set
			l.mu.RLock()
			handler := l.onMessage
			l.mu.RUnlock()

			if handler != nil {
				handler(&msg, addr)
			}
		}
	}
}

// Broadcast sends a message to the broadcast address
func (l *UDPListener) Broadcast(msg *DiscoveryMessage) error {
	data, err := msg.Bytes()
	if err != nil {
		return fmt.Errorf("failed to marshal message: %w", err)
	}

	// Broadcast to local network
	broadcastAddr, err := net.ResolveUDPAddr("udp", "255.255.255.255:49100")
	if err != nil {
		return fmt.Errorf("failed to resolve broadcast address: %w", err)
	}

	_, err = l.conn.WriteToUDP(data, broadcastAddr)
	if err != nil {
		return fmt.Errorf("failed to send broadcast: %w", err)
	}

	return nil
}

// IsRunning returns true if the listener is running
func (l *UDPListener) IsRunning() bool {
	l.mu.RLock()
	defer l.mu.RUnlock()
	return l.running
}

// GetPort returns the listening port
func (l *UDPListener) GetPort() int {
	return l.port
}
